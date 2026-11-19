//! Shared spot-trading domain and adapter contracts, based on BBGO `pkg/types`.
use async_trait::async_trait;
pub use bbgo_fixedpoint::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod accounting;
pub mod book;
pub mod subscription;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Arithmetic(#[from] bbgo_fixedpoint::Error),
    #[error("invalid trading input: {0}")]
    Invalid(String),
    #[error("adapter failure: {0}")]
    Adapter(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}
pub type Result<T> = std::result::Result<T, Error>;
/// Unix milliseconds; exchange and receive times must remain distinct.
pub type Timestamp = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderType {
    Limit,
    Market,
    LimitMaker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Market {
    pub symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub spot_enabled: bool,
    pub trading: bool,
    pub order_types: Vec<OrderType>,
    pub tick_size: Value,
    pub step_size: Value,
    pub min_quantity: Value,
    pub max_quantity: Value,
    pub min_notional: Value,
}
impl Market {
    /// Normalize downward (never increase exposure), then validate venue filters.
    /// Market orders need a separate reference-price/notional policy.
    pub fn normalize_limit(&self, order: &SubmitOrder) -> Result<SubmitOrder> {
        if !self.spot_enabled
            || !self.trading
            || self.symbol != order.symbol
            || !self.order_types.contains(&order.order_type)
            || order.order_type == OrderType::Market
        {
            return Err(Error::Invalid("market or order type not eligible".into()));
        }
        if self.tick_size <= Value::ZERO
            || self.step_size <= Value::ZERO
            || self.min_quantity < Value::ZERO
            || self.max_quantity < self.min_quantity
            || self.min_notional < Value::ZERO
        {
            return Err(Error::Invalid("invalid market filters".into()));
        }
        let mut normalized = order.clone();
        normalized.price = order.price.floor_to(self.tick_size)?;
        normalized.quantity = order.quantity.floor_to(self.step_size)?;
        if normalized.price <= Value::ZERO
            || normalized.quantity <= Value::ZERO
            || normalized.quantity < self.min_quantity
            || normalized.quantity > self.max_quantity
            || normalized.price.checked_mul(normalized.quantity)? < self.min_notional
        {
            return Err(Error::Invalid(
                "price, quantity or notional fails filters".into(),
            ));
        }
        Ok(normalized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    pub available: Value,
    pub locked: Value,
}
impl Balance {
    pub fn total(&self) -> Result<Value> {
        Ok(self.available.checked_add(self.locked)?)
    }
}
pub type Balances = BTreeMap<String, Balance>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitOrder {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Value,
    pub quantity: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Submission acknowledged, but resting/fill state has not been observed.
    Pending,
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub request: SubmitOrder,
    pub status: OrderStatus,
    pub executed_quantity: Value,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub price: Value,
    pub quantity: Value,
    pub fee: Value,
    pub fee_currency: String,
    pub exchange_time: Timestamp,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticker {
    pub symbol: String,
    pub bid: Value,
    pub ask: Value,
    pub exchange_time: Timestamp,
    pub receive_time: Timestamp,
    pub generation: u64,
}
impl Ticker {
    pub fn midpoint(&self) -> Result<Value> {
        if self.bid <= Value::ZERO || self.ask < self.bid {
            return Err(Error::Invalid("invalid/crossed quote".into()));
        }
        Ok(self.bid.checked_add(
            self.ask
                .checked_sub(self.bid)?
                .checked_div(Value::from_integer(2)?)?,
        )?)
    }
}
/// Public BBO with explicit timestamp provenance. Binance bookTicker has no
/// exchange timestamp; a local receive time must not be relabeled as one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookTicker {
    pub symbol: String,
    pub bid: Value,
    pub bid_quantity: Value,
    pub ask: Value,
    pub ask_quantity: Value,
    pub exchange_time: Option<Timestamp>,
    pub receive_time: Timestamp,
    pub generation: u64,
}
impl BookTicker {
    pub fn midpoint(&self) -> Result<Value> {
        if self.bid_quantity < Value::ZERO
            || self.ask_quantity < Value::ZERO
            || self.bid <= Value::ZERO
            || self.ask <= self.bid
        {
            return Err(Error::Invalid("invalid public BBO".into()));
        }
        Ok(self.bid.checked_add(
            self.ask
                .checked_sub(self.bid)?
                .checked_div(Value::from_integer(2)?)?,
        )?)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicTrade {
    pub id: String,
    pub symbol: String,
    pub side: Side,
    pub price: Value,
    pub quantity: Value,
    pub exchange_time: Timestamp,
    pub receive_time: Timestamp,
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KLine {
    pub symbol: String,
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub open: Value,
    pub high: Value,
    pub low: Value,
    pub close: Value,
    pub volume: Value,
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    Quote(Ticker),
    BookTicker(BookTicker),
    PublicTrade(PublicTrade),
    Candle(KLine),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserEvent {
    Order(Order),
    Trade(Trade),
    Balances(Balances),
}

/// Adapters own transport retries, authentication and cancellation. Implementations
/// must document whether cancel is acknowledged or merely requested.
#[async_trait]
pub trait MarketDataService: Send + Sync {
    async fn query_markets(&self) -> Result<BTreeMap<String, Market>>;
    async fn query_ticker(&self, symbol: &str) -> Result<Ticker>;
}
#[async_trait]
pub trait AccountService: Send + Sync {
    async fn query_balances(&self) -> Result<Balances>;
}
#[async_trait]
pub trait TradeService: Send + Sync {
    async fn submit_order(&self, order: SubmitOrder) -> Result<Order>;
    async fn query_open_orders(&self, symbol: &str) -> Result<Vec<Order>>;
    async fn cancel_orders(&self, orders: &[Order]) -> Result<()>;
}
/// Authenticated lookup used to reconcile an uncertain submission/cancellation.
/// Not-found is an error, never proof that a timed-out write did not execute.
#[async_trait]
pub trait OrderQueryService: Send + Sync {
    async fn query_order(&self, symbol: &str, order_id: &str) -> Result<Order>;
    async fn query_order_by_client_id(&self, symbol: &str, client_order_id: &str) -> Result<Order>;
}
pub trait Exchange: MarketDataService + AccountService + TradeService {
    fn name(&self) -> &str;
}

/// Pull-based streams permit backpressure without coupling consumers to an executor.
#[async_trait]
pub trait EventStream<T>: Send {
    async fn next_event(&mut self) -> Result<Option<T>>;
    async fn close(&mut self) -> Result<()>;
}
