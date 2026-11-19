//! Transactional long-only spot accounting. Third-asset fees require external valuation.
use crate::{Balance, Balances, Error, Market, Result, Side, Trade, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Round nonnegative multiplication upward, for fee and reservation bounds.
pub fn multiply_ceil(a: Value, b: Value) -> Result<Value> {
    if a < Value::ZERO || b < Value::ZERO {
        return Err(Error::Invalid("negative rounded product".into()));
    }
    Ok(a.checked_mul_ceil(b)?)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub quantity: Value,
    pub cost_basis: Value,
    pub realized_pnl: Value,
}
impl Position {
    pub fn average_cost(&self) -> Result<Value> {
        if self.quantity == Value::ZERO {
            Ok(Value::ZERO)
        } else {
            Ok(self.cost_basis.checked_div(self.quantity)?)
        }
    }
    pub fn unrealized_pnl(&self, price: Value) -> Result<Value> {
        if price < Value::ZERO {
            return Err(Error::Invalid("negative mark price".into()));
        }
        Ok(self
            .quantity
            .checked_mul(price)?
            .checked_sub(self.cost_basis)?)
    }
    fn apply(&mut self, market: &Market, trade: &Trade) -> Result<()> {
        let quote = trade.quantity.checked_mul(trade.price)?;
        let base_fee = if trade.fee_currency == market.base_currency {
            trade.fee
        } else {
            Value::ZERO
        };
        let quote_fee = if trade.fee_currency == market.quote_currency {
            trade.fee
        } else {
            Value::ZERO
        };
        match trade.side {
            Side::Buy => {
                let acquired = trade.quantity.checked_sub(base_fee)?;
                if acquired <= Value::ZERO {
                    return Err(Error::Invalid(
                        "base fee consumes acquired inventory".into(),
                    ));
                }
                self.quantity = self.quantity.checked_add(acquired)?;
                self.cost_basis = self.cost_basis.checked_add(quote)?.checked_add(quote_fee)?;
            }
            Side::Sell => {
                let disposed = trade.quantity.checked_add(base_fee)?;
                if disposed > self.quantity {
                    return Err(Error::Invalid("sell exceeds tracked position".into()));
                }
                // Allocate from total basis; final disposal releases all rounding dust.
                let basis = if disposed == self.quantity {
                    self.cost_basis
                } else {
                    self.cost_basis.checked_mul_div(disposed, self.quantity)?
                };
                self.realized_pnl = self
                    .realized_pnl
                    .checked_add(quote.checked_sub(quote_fee)?.checked_sub(basis)?)?;
                self.quantity = self.quantity.checked_sub(disposed)?;
                self.cost_basis = self.cost_basis.checked_sub(basis)?;
            }
        }
        Ok(())
    }
}

/// Owns available/locked funds, seeded cost bases, and exact trade deduplication.
/// Reconciliation reports differences; it does not silently overwrite accounting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    balances: Balances,
    positions: BTreeMap<String, Position>,
    trades: BTreeMap<String, BTreeMap<String, Trade>>,
}
impl Ledger {
    pub fn new(balances: Balances, positions: BTreeMap<String, Position>) -> Result<Self> {
        let ledger = Self {
            balances,
            positions,
            trades: BTreeMap::new(),
        };
        ledger.validate()?;
        Ok(ledger)
    }
    /// Structural checks for externally decoded state. This does not prove that
    /// seeded cost bases or balances agree with an exchange's transaction history.
    pub fn validate(&self) -> Result<()> {
        for (currency, balance) in &self.balances {
            if currency.is_empty() {
                return Err(Error::Invalid("empty balance currency".into()));
            }
            if balance.available < Value::ZERO || balance.locked < Value::ZERO {
                return Err(Error::Invalid("negative account balance".into()));
            }
            balance.total()?;
        }
        for (symbol, p) in &self.positions {
            if symbol.is_empty()
                || p.quantity < Value::ZERO
                || p.cost_basis < Value::ZERO
                || (p.quantity == Value::ZERO && p.cost_basis != Value::ZERO)
            {
                return Err(Error::Invalid("invalid seed position".into()));
            }
        }
        for (symbol, trades) in &self.trades {
            for (id, trade) in trades {
                if symbol.is_empty()
                    || id.is_empty()
                    || trade.symbol != *symbol
                    || trade.id != *id
                    || trade.order_id.is_empty()
                    || trade.quantity <= Value::ZERO
                    || trade.price <= Value::ZERO
                    || trade.fee < Value::ZERO
                    || (trade.fee > Value::ZERO && trade.fee_currency.is_empty())
                {
                    return Err(Error::Invalid("invalid ledger trade history".into()));
                }
            }
        }
        Ok(())
    }
    pub fn balances(&self) -> &Balances {
        &self.balances
    }
    pub fn positions(&self) -> &BTreeMap<String, Position> {
        &self.positions
    }
    pub fn trades(&self) -> &BTreeMap<String, BTreeMap<String, Trade>> {
        &self.trades
    }
    pub fn reserve(&mut self, currency: &str, amount: Value) -> Result<()> {
        self.transfer(currency, amount, true)
    }
    pub fn release(&mut self, currency: &str, amount: Value) -> Result<()> {
        self.transfer(currency, amount, false)
    }
    fn transfer(&mut self, currency: &str, amount: Value, lock: bool) -> Result<()> {
        if amount < Value::ZERO {
            return Err(Error::Invalid("negative reservation".into()));
        }
        let mut balance = self.balances.get(currency).cloned().unwrap_or(Balance {
            available: Value::ZERO,
            locked: Value::ZERO,
        });
        let (from, to) = if lock {
            (&mut balance.available, &mut balance.locked)
        } else {
            (&mut balance.locked, &mut balance.available)
        };
        if *from < amount {
            return Err(Error::Invalid("insufficient funds".into()));
        }
        *from = from.checked_sub(amount)?;
        *to = to.checked_add(amount)?;
        self.balances.insert(currency.into(), balance);
        Ok(())
    }
    /// Returns false for an identical duplicate; conflicting IDs fail without mutation.
    /// Callers must release this fill's reservations before application.
    pub fn apply_trade(&mut self, market: &Market, trade: &Trade) -> Result<bool> {
        if let Some(old) = self
            .trades
            .get(&trade.symbol)
            .and_then(|trades| trades.get(&trade.id))
        {
            return if old == trade {
                Ok(false)
            } else {
                Err(Error::Invalid("conflicting trade ID".into()))
            };
        }
        if trade.id.is_empty()
            || trade.order_id.is_empty()
            || trade.symbol != market.symbol
            || market.base_currency.is_empty()
            || market.quote_currency.is_empty()
            || market.base_currency == market.quote_currency
            || trade.quantity <= Value::ZERO
            || trade.price <= Value::ZERO
            || trade.fee < Value::ZERO
        {
            return Err(Error::Invalid("invalid trade".into()));
        }
        if trade.fee != Value::ZERO
            && trade.fee_currency != market.base_currency
            && trade.fee_currency != market.quote_currency
        {
            return Err(Error::Unsupported("third-asset fee valuation".into()));
        }
        let mut next = self.clone();
        let quote = trade.quantity.checked_mul(trade.price)?;
        match trade.side {
            Side::Buy => {
                next.debit(&market.quote_currency, quote)?;
                next.credit(&market.base_currency, trade.quantity)?;
            }
            Side::Sell => {
                next.debit(&market.base_currency, trade.quantity)?;
                next.credit(&market.quote_currency, quote)?;
            }
        }
        if trade.fee > Value::ZERO {
            next.debit(&trade.fee_currency, trade.fee)?;
        }
        next.positions
            .entry(market.symbol.clone())
            .or_default()
            .apply(market, trade)?;
        next.trades
            .entry(trade.symbol.clone())
            .or_default()
            .insert(trade.id.clone(), trade.clone());
        *self = next;
        Ok(true)
    }
    fn debit(&mut self, currency: &str, amount: Value) -> Result<()> {
        let b = self.balances.entry(currency.into()).or_insert(Balance {
            available: Value::ZERO,
            locked: Value::ZERO,
        });
        if b.available < amount {
            return Err(Error::Invalid(format!("insufficient {currency}")));
        }
        b.available = b.available.checked_sub(amount)?;
        Ok(())
    }
    fn credit(&mut self, currency: &str, amount: Value) -> Result<()> {
        let b = self.balances.entry(currency.into()).or_insert(Balance {
            available: Value::ZERO,
            locked: Value::ZERO,
        });
        b.available = b.available.checked_add(amount)?;
        Ok(())
    }
    pub fn reconcile(&self, remote: &Balances) -> Result<Vec<BalanceDifference>> {
        let currencies: std::collections::BTreeSet<_> =
            self.balances.keys().chain(remote.keys()).collect();
        let zero = Balance {
            available: Value::ZERO,
            locked: Value::ZERO,
        };
        let mut differences = Vec::new();
        for currency in currencies {
            let local = self.balances.get(currency).unwrap_or(&zero);
            let actual = remote.get(currency).unwrap_or(&zero);
            if actual.available < Value::ZERO || actual.locked < Value::ZERO {
                return Err(Error::Invalid("negative remote balance".into()));
            }
            if local != actual {
                differences.push(BalanceDifference {
                    currency: currency.clone(),
                    local: local.clone(),
                    remote: actual.clone(),
                });
            }
        }
        Ok(differences)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceDifference {
    pub currency: String,
    pub local: Balance,
    pub remote: Balance,
}
