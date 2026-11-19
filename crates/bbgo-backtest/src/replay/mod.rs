//! Deterministic long-only limit matching against public trade prints.
//! This models FIFO simulated orders, not exchange queue priority or real fills.
use async_trait::async_trait;
use bbgo_core::OrderExecutor;
use bbgo_types::{
    accounting::{Ledger, multiply_ceil},
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};
mod recovery;
use recovery::Action;
pub use recovery::{MAX_CHECKPOINT_BYTES, MAX_RECOVERY_ACTIONS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradePrint {
    pub id: String,
    pub symbol: String,
    /// Aggressor side: sell prints can execute resting buys and conversely.
    pub aggressor: Side,
    pub price: Value,
    pub quantity: Value,
    pub exchange_time: Timestamp,
}
#[derive(Debug, Clone)]
struct WorkingOrder {
    order: Order,
    currency: String,
    reserved: Value,
    notional: Value,
    fee: Value,
}
#[derive(Debug, Clone)]
struct State {
    ledger: Ledger,
    orders: Vec<WorkingOrder>,
    prints: BTreeMap<String, BTreeMap<String, TradePrint>>,
    clocks: BTreeMap<String, Timestamp>,
    next_trade: u64,
    journal: Vec<Action>,
}
pub struct ReplayExecutor {
    markets: BTreeMap<String, Market>,
    fee_rate: Value,
    seed: Ledger,
    state: Mutex<State>,
}
impl ReplayExecutor {
    pub fn new(markets: BTreeMap<String, Market>, ledger: Ledger, fee_rate: Value) -> Result<Self> {
        ledger.validate()?;
        if fee_rate < Value::ZERO
            || fee_rate >= Value::ONE
            || markets.is_empty()
            || markets.iter().any(|(key, market)| {
                key != &market.symbol
                    || market.base_currency.is_empty()
                    || market.quote_currency.is_empty()
                    || market.base_currency == market.quote_currency
            })
        {
            return Err(Error::Invalid("invalid replay markets or fee rate".into()));
        }
        // Existing locked funds need an external reservation owner; do not adopt them.
        if ledger.balances().values().any(|b| b.locked != Value::ZERO) {
            return Err(Error::Invalid(
                "replay requires no preexisting locked funds".into(),
            ));
        }
        // A seeded ledger may already contain replay-generated fills. Never
        // silently deduplicate a new fill against those historical identifiers.
        let next_trade = ledger
            .trades()
            .values()
            .flat_map(|trades| trades.keys())
            .filter_map(|id| id.strip_prefix("replay-trade:")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        Ok(Self {
            markets,
            fee_rate,
            seed: ledger.clone(),
            state: Mutex::new(State {
                ledger,
                orders: Vec::new(),
                prints: BTreeMap::new(),
                clocks: BTreeMap::new(),
                next_trade,
                journal: Vec::new(),
            }),
        })
    }
    pub fn ledger(&self) -> Result<Ledger> {
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?
            .ledger
            .clone())
    }
    pub fn orders(&self) -> Result<Vec<Order>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?
            .orders
            .iter()
            .map(|w| w.order.clone())
            .collect())
    }
    /// Atomic matching with shared print liquidity, acceptance-order priority and
    /// fills at the resting limit (no optimistic price improvement).
    /// Calling this after strategy dispatch would introduce lookahead; sessions
    /// must match a print BEFORE letting strategies react to that same print.
    pub fn on_trade(&self, print: TradePrint) -> Result<Vec<UserEvent>> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?;
        self.match_trade(&mut guard, print, None)
    }
    /// Match an atomic batch against only the specified preexisting client IDs.
    /// Backtest drivers use this to enforce availability-time eligibility.
    pub fn on_trades_for_orders(
        &self,
        prints: Vec<TradePrint>,
        eligible: &BTreeSet<String>,
    ) -> Result<Vec<UserEvent>> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?;
        if eligible.iter().any(|id| {
            !guard
                .orders
                .iter()
                .any(|w| &w.order.request.client_order_id == id)
        }) {
            return Err(Error::Invalid("unknown eligible replay order".into()));
        }
        let mut next = guard.clone();
        let mut events = Vec::new();
        for print in prints {
            events.extend(self.match_trade(&mut next, print, Some(eligible))?);
        }
        *guard = next;
        Ok(events)
    }
    fn match_trade(
        &self,
        guard: &mut State,
        print: TradePrint,
        eligible: Option<&BTreeSet<String>>,
    ) -> Result<Vec<UserEvent>> {
        let market = self
            .markets
            .get(&print.symbol)
            .ok_or_else(|| Error::Invalid("unknown replay symbol".into()))?;
        if print.id.is_empty() || print.price <= Value::ZERO || print.quantity <= Value::ZERO {
            return Err(Error::Invalid("invalid public trade".into()));
        }
        if let Some(old) = guard
            .prints
            .get(&print.symbol)
            .and_then(|v| v.get(&print.id))
        {
            return if old == &print {
                Ok(Vec::new())
            } else {
                Err(Error::Invalid("conflicting public trade ID".into()))
            };
        }
        if guard
            .clocks
            .get(&print.symbol)
            .is_some_and(|last| print.exchange_time < *last)
        {
            return Err(Error::Invalid("out-of-order replay print".into()));
        }
        let mut next = guard.clone();
        let mut available = print.quantity;
        let mut events = Vec::new();
        for working in &mut next.orders {
            let order = &mut working.order;
            if eligible.is_some_and(|ids| !ids.contains(&order.request.client_order_id)) {
                continue;
            }
            if available == Value::ZERO {
                break;
            }
            if order.request.symbol != print.symbol
                || !matches!(
                    order.status,
                    OrderStatus::New | OrderStatus::PartiallyFilled
                )
                || order.request.side == print.aggressor
            {
                continue;
            }
            let executable = match order.request.side {
                Side::Buy => print.price <= order.request.price,
                Side::Sell => print.price >= order.request.price,
            };
            if !executable {
                continue;
            }
            let remaining = order
                .request
                .quantity
                .checked_sub(order.executed_quantity)?;
            let quantity = remaining.min(available);
            let notional = quantity.checked_mul(order.request.price)?;
            // Cumulative rounding means splitting a fill cannot multiply fee atoms.
            let cumulative = working.notional.checked_add(notional)?;
            let total_fee = multiply_ceil(cumulative, self.fee_rate)?;
            let fee = total_fee.checked_sub(working.fee)?;
            next.next_trade = next
                .next_trade
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("trade sequence exhausted".into()))?;
            let trade = Trade {
                id: format!("replay-trade:{}", next.next_trade),
                order_id: order.id.clone(),
                symbol: print.symbol.clone(),
                side: order.request.side,
                price: order.request.price,
                quantity,
                fee,
                fee_currency: market.quote_currency.clone(),
                exchange_time: print.exchange_time,
            };
            next.ledger.release(&working.currency, working.reserved)?;
            if !next.ledger.apply_trade(market, &trade)? {
                return Err(Error::Invalid(
                    "generated replay trade ID already exists".into(),
                ));
            }
            order.executed_quantity = order.executed_quantity.checked_add(quantity)?;
            order.status = if order.executed_quantity == order.request.quantity {
                OrderStatus::Filled
            } else {
                OrderStatus::PartiallyFilled
            };
            working.reserved = if order.status == OrderStatus::Filled {
                Value::ZERO
            } else {
                working.reserved.checked_sub(match order.request.side {
                    Side::Buy => notional.checked_add(fee)?,
                    Side::Sell => quantity,
                })?
            };
            next.ledger.reserve(&working.currency, working.reserved)?;
            working.notional = cumulative;
            working.fee = total_fee;
            available = available.checked_sub(quantity)?;
            events.push(UserEvent::Trade(trade));
            events.push(UserEvent::Order(order.clone()));
        }
        next.clocks
            .insert(print.symbol.clone(), print.exchange_time);
        next.prints
            .entry(print.symbol.clone())
            .or_default()
            .insert(print.id.clone(), print.clone());
        next.journal.push(Action::Trade {
            print,
            eligible: eligible.cloned(),
        });
        if !events.is_empty() {
            events.push(UserEvent::Balances(next.ledger.balances().clone()));
        }
        *guard = next;
        Ok(events)
    }
    pub fn submit_limit(&self, request: SubmitOrder) -> Result<Order> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?;
        if let Some(working) = guard
            .orders
            .iter()
            .find(|w| w.order.request.client_order_id == request.client_order_id)
        {
            return if working.order.request == request {
                Ok(working.order.clone())
            } else {
                Err(Error::Invalid("client order ID conflict".into()))
            };
        }
        let market = self
            .markets
            .get(&request.symbol)
            .ok_or_else(|| Error::Invalid("unknown replay market".into()))?;
        if request.client_order_id.is_empty() || request.order_type != OrderType::Limit {
            return Err(Error::Unsupported(
                "replay supports identified limit orders only".into(),
            ));
        }
        if market.normalize_limit(&request)? != request {
            return Err(Error::Invalid(
                "normalize replay orders before submission".into(),
            ));
        }
        if request.side == Side::Sell
            && guard
                .ledger
                .positions()
                .get(&request.symbol)
                .map_or(Value::ZERO, |p| p.quantity)
                < request.quantity
        {
            return Err(Error::Invalid(
                "sell requires a seeded cost-basis position".into(),
            ));
        }
        let (currency, reserved) = match request.side {
            Side::Buy => {
                let cost = multiply_ceil(request.price, request.quantity)?;
                (
                    market.quote_currency.clone(),
                    cost.checked_add(multiply_ceil(cost, self.fee_rate)?)?,
                )
            }
            Side::Sell => (market.base_currency.clone(), request.quantity),
        };
        let order = Order {
            id: format!("replay:{}", request.client_order_id),
            request,
            status: OrderStatus::New,
            executed_quantity: Value::ZERO,
        };
        guard.ledger.reserve(&currency, reserved)?;
        guard.orders.push(WorkingOrder {
            order: order.clone(),
            currency,
            reserved,
            notional: Value::ZERO,
            fee: Value::ZERO,
        });
        guard.journal.push(Action::Submit {
            request: order.request.clone(),
        });
        Ok(order)
    }
    pub fn cancel_orders(&self, orders: &[Order]) -> Result<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?;
        let mut next = guard.clone();
        let mut changed = false;
        for requested in orders {
            let working = next
                .orders
                .iter_mut()
                .find(|w| w.order.id == requested.id && w.order.request == requested.request)
                .ok_or_else(|| Error::Invalid("unknown replay order".into()))?;
            if matches!(
                working.order.status,
                OrderStatus::New | OrderStatus::PartiallyFilled
            ) {
                next.ledger.release(&working.currency, working.reserved)?;
                working.reserved = Value::ZERO;
                working.order.status = OrderStatus::Canceled;
                changed = true;
            }
        }
        if changed {
            next.journal.push(Action::Cancel {
                ids: orders.iter().map(|o| o.id.clone()).collect(),
            });
        }
        *guard = next;
        Ok(())
    }
}
#[async_trait]
impl OrderExecutor for ReplayExecutor {
    async fn submit(&self, request: SubmitOrder) -> Result<Order> {
        self.submit_limit(request)
    }
    async fn cancel(&self, orders: &[Order]) -> Result<()> {
        self.cancel_orders(orders)
    }
}
