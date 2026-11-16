use crate::{
    Model,
    source::{Frame, Input, Tick},
};
use async_trait::async_trait;
use bbgo_core::OrderExecutor;
use bbgo_replay::{ReplayExecutor, TradePrint};
use bbgo_types::{accounting::Ledger, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

struct Clock {
    now: Option<i64>,
    accepted: BTreeMap<String, i64>,
    candles: BTreeMap<String, Timestamp>,
    prints: BTreeMap<(String, String), TradePrint>,
}
pub struct Batch {
    pub user: Vec<UserEvent>,
    pub market: Option<MarketEvent>,
}
pub struct Simulation {
    model: Model,
    account: ReplayExecutor,
    clock: Mutex<Clock>,
    symbols: BTreeSet<String>,
}
impl Simulation {
    pub fn new(
        model: Model,
        markets: BTreeMap<String, Market>,
        seed: Ledger,
        fee: Value,
    ) -> Result<Self> {
        Ok(Self {
            model,
            symbols: markets.keys().cloned().collect(),
            account: ReplayExecutor::new(markets, seed, fee)?,
            clock: Mutex::new(Clock {
                now: None,
                accepted: BTreeMap::new(),
                candles: BTreeMap::new(),
                prints: BTreeMap::new(),
            }),
        })
    }
    pub fn model(&self) -> Model {
        self.model
    }
    pub fn ledger(&self) -> Result<Ledger> {
        self.account.ledger()
    }
    pub fn orders(&self) -> Result<Vec<Order>> {
        self.account.orders()
    }
    pub(crate) fn set_callback_clock(&self, at: i64) -> Result<()> {
        let mut clock = self
            .clock
            .lock()
            .map_err(|_| Error::Adapter("simulation lock poisoned".into()))?;
        if at < 0 || clock.now.is_some_and(|old| at < old) {
            return Err(Error::Invalid("invalid/regressed callback clock".into()));
        }
        clock.now = Some(at);
        Ok(())
    }
    pub fn process(&self, frame: &Frame) -> Result<Batch> {
        let mut clock = self
            .clock
            .lock()
            .map_err(|_| Error::Adapter("simulation lock poisoned".into()))?;
        if frame.position.available_at_ns < 0
            || clock
                .now
                .is_some_and(|at| at > frame.position.available_at_ns)
        {
            return Err(Error::Invalid("invalid/regressed simulation clock".into()));
        }
        let mut cutoff = frame.position.available_at_ns;
        let (prints, market) = match (&frame.input, self.model) {
            (Input::Tick(tick), Model::Event) => {
                let metadata = tick.metadata();
                if metadata.received_at_ns != cutoff
                    || metadata.source_id.is_empty()
                    || metadata.writer_run_id.is_empty()
                    || metadata.exchange_time_ns.is_some_and(|t| t <= 0)
                    || metadata.gap_before_ms.is_some_and(|gap| gap < 0)
                {
                    return Err(Error::Invalid("invalid tick clock/provenance".into()));
                }
                match tick {
                    Tick::Book { quote, metadata } => {
                        quote.midpoint()?;
                        if quote.receive_time != cutoff / 1_000_000
                            || quote.exchange_time
                                != metadata.exchange_time_ns.map(|t| t / 1_000_000)
                            || quote.generation != metadata.generation
                        {
                            return Err(Error::Invalid("BBO metadata mismatch".into()));
                        }
                        (Vec::new(), MarketEvent::BookTicker(quote.clone()))
                    }
                    Tick::Trade { print, metadata } => {
                        if metadata.exchange_time_ns.map(|t| t / 1_000_000)
                            != Some(print.exchange_time)
                        {
                            return Err(Error::Invalid("trade event time mismatch".into()));
                        }
                        (
                            vec![print.clone()],
                            MarketEvent::PublicTrade(PublicTrade {
                                id: print.id.clone(),
                                symbol: print.symbol.clone(),
                                side: print.aggressor,
                                price: print.price,
                                quantity: print.quantity,
                                exchange_time: print.exchange_time,
                                receive_time: cutoff / 1_000_000,
                                generation: metadata.generation,
                            }),
                        )
                    }
                }
            }
            (Input::Candle(candle), Model::Kline) => {
                if !candle.closed
                    || candle.start_time < 0
                    || candle.end_time < candle.start_time
                    || candle.open <= Value::ZERO
                    || candle.low <= Value::ZERO
                    || candle.high < candle.low
                    || candle.open > candle.high
                    || candle.open < candle.low
                    || candle.close > candle.high
                    || candle.close < candle.low
                    || candle.volume < Value::ZERO
                    || candle.end_time.checked_mul(1_000_000) != Some(cutoff)
                    || clock
                        .candles
                        .get(&candle.symbol)
                        .is_some_and(|end| candle.start_time <= *end)
                {
                    return Err(Error::Invalid("invalid/overlapping closed candle".into()));
                }
                // Orders accepted exactly at the bar's start are eligible; no
                // submissions can occur during this synthetic intrabar batch.
                cutoff = candle
                    .start_time
                    .checked_mul(1_000_000)
                    .ok_or_else(|| Error::Invalid("candle start overflow".into()))?;
                let quantity = self
                    .account
                    .orders()?
                    .iter()
                    .filter(|o| {
                        matches!(o.status, OrderStatus::New | OrderStatus::PartiallyFilled)
                            && o.request.symbol == candle.symbol
                    })
                    .try_fold(Value::ZERO, |sum, o| {
                        sum.checked_add(o.request.quantity.checked_sub(o.executed_quantity)?)
                    })?;
                let path = if candle.close >= candle.open {
                    [candle.open, candle.low, candle.high, candle.close]
                } else {
                    [candle.open, candle.high, candle.low, candle.close]
                };
                let mut prints = Vec::new();
                if quantity > Value::ZERO {
                    for (step, price) in path.into_iter().enumerate() {
                        for aggressor in [Side::Sell, Side::Buy] {
                            prints.push(TradePrint {
                                id: format!(
                                    "kline:{}:{}:{}:{step}:{aggressor:?}",
                                    frame.position.source, frame.position.sequence, candle.symbol
                                ),
                                symbol: candle.symbol.clone(),
                                aggressor,
                                price,
                                quantity,
                                exchange_time: candle.end_time,
                            });
                        }
                    }
                }
                (prints, MarketEvent::Candle(candle.clone()))
            }
            _ => return Err(Error::Invalid("input/execution model mismatch".into())),
        };
        let eligible: BTreeSet<_> = clock
            .accepted
            .iter()
            .filter(|(_, at)| match self.model {
                Model::Event => **at < cutoff,
                Model::Kline => **at <= cutoff,
            })
            .map(|(id, _)| id.clone())
            .collect();
        let duplicate = if let Input::Tick(Tick::Trade { print, .. }) = &frame.input {
            let key = (print.symbol.clone(), print.id.clone());
            if let Some(old) = clock.prints.get(&key) {
                if old != print {
                    return Err(Error::Invalid("conflicting public trade identity".into()));
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        let symbol = match &market {
            MarketEvent::BookTicker(quote) => &quote.symbol,
            MarketEvent::PublicTrade(trade) => &trade.symbol,
            MarketEvent::Candle(candle) => &candle.symbol,
            MarketEvent::Quote(quote) => &quote.symbol,
        };
        if !self.symbols.contains(symbol) {
            return Err(Error::Invalid(
                "input symbol not in backtest markets".into(),
            ));
        }
        let user = self.account.on_trades_for_orders(prints, &eligible)?;
        if let Input::Tick(Tick::Trade { print, .. }) = &frame.input {
            clock
                .prints
                .insert((print.symbol.clone(), print.id.clone()), print.clone());
        }
        clock.now = Some(frame.position.available_at_ns);
        if let Input::Candle(candle) = &frame.input {
            clock.candles.insert(candle.symbol.clone(), candle.end_time);
        }
        Ok(Batch {
            user,
            market: (!duplicate).then_some(market),
        })
    }
}
#[async_trait]
impl OrderExecutor for Simulation {
    async fn submit(&self, request: SubmitOrder) -> Result<Order> {
        let mut clock = self
            .clock
            .lock()
            .map_err(|_| Error::Adapter("simulation lock poisoned".into()))?;
        let order = self.account.submit_limit(request)?;
        let now = clock.now.unwrap_or(i64::MIN);
        clock
            .accepted
            .entry(order.request.client_order_id.clone())
            .or_insert(now);
        Ok(order)
    }
    async fn cancel(&self, orders: &[Order]) -> Result<()> {
        let _clock = self
            .clock
            .lock()
            .map_err(|_| Error::Adapter("simulation lock poisoned".into()))?;
        self.account.cancel_orders(orders)
    }
}
