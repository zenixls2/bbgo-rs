//! Read-only Binance spot REST adapter and public depth-stream decoding.
//!
//! No credentials, signing, private stream or order submission path. Exchange
//! metadata is a subset: price bands, maximum notional and account permissions
//! must still be checked by a future execution adapter.
use async_trait::async_trait;
use bbgo_types::{
    Error, Market, MarketDataService, OrderType, Result, Ticker, Timestamp, Value,
    book::{BookUpdate, Level, OrderBook},
};
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};

const MAX_BODY: usize = 8 * 1024 * 1024;

/// Implementations must return only successful HTTP bodies and enforce bounds.
#[async_trait]
pub trait PublicTransport: Send + Sync {
    async fn get(&self, path: &str, parameters: &[(&str, String)]) -> Result<Vec<u8>>;
}

/// Fixed HTTPS origin; redirects disabled, bounded response, no implicit retries.
pub struct HttpTransport {
    client: reqwest::Client,
}
impl HttpTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(adapter)?,
        })
    }
}
#[async_trait]
impl PublicTransport for HttpTransport {
    async fn get(&self, path: &str, parameters: &[(&str, String)]) -> Result<Vec<u8>> {
        if !matches!(path, "/api/v3/exchangeInfo" | "/api/v3/depth") {
            return Err(Error::Unsupported("public endpoint not allowed".into()));
        }
        let mut response = self
            .client
            .get(format!("https://api.binance.com{path}"))
            .query(parameters)
            .send()
            .await
            .map_err(adapter)?;
        // Do not expose response bodies (which can contain untrusted content).
        if !response.status().is_success() {
            return Err(Error::Adapter(format!(
                "Binance HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(Error::Adapter("public response too large".into()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(adapter)? {
            if chunk.len() > MAX_BODY - body.len() {
                return Err(Error::Adapter("public response too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}
fn adapter(error: impl std::fmt::Display) -> Error {
    Error::Adapter(error.to_string())
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn check_symbol(symbol: &str) -> Result<()> {
    if symbol.is_empty()
        || symbol.len() > 32
        || !symbol
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(invalid("invalid Binance symbol"));
    }
    Ok(())
}

pub struct PublicClient<T> {
    transport: T,
}
impl<T: PublicTransport> PublicClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
    /// REST depth has no exchange timestamp; never label receive time as one.
    /// Buffer depth events while fetching, discard events ending <= snapshot,
    /// then require an update bridging snapshot+1. Re-snapshot after any gap.
    pub async fn depth(&self, symbol: &str, limit: u16) -> Result<OrderBook> {
        check_symbol(symbol)?;
        if !matches!(limit, 5 | 10 | 20 | 50 | 100 | 500 | 1000 | 5000) {
            return Err(invalid("unsupported depth limit"));
        }
        let body = self
            .transport
            .get(
                "/api/v3/depth",
                &[("symbol", symbol.into()), ("limit", limit.to_string())],
            )
            .await?;
        decode_snapshot(symbol, &body)
    }
}
#[async_trait]
impl<T: PublicTransport> MarketDataService for PublicClient<T> {
    async fn query_markets(&self) -> Result<BTreeMap<String, Market>> {
        decode_markets(&self.transport.get("/api/v3/exchangeInfo", &[]).await?)
    }
    async fn query_ticker(&self, _symbol: &str) -> Result<Ticker> {
        Err(Error::Unsupported(
            "REST bookTicker has no exchange time; use synchronized timestamped depth events"
                .into(),
        ))
    }
}

#[async_trait]
impl<T: PublicTransport> crate::public::SpotMarketDataService for PublicClient<T> {
    async fn query_instruments(&self) -> Result<BTreeMap<String, crate::public::SpotInstrument>> {
        Ok(self
            .query_markets()
            .await?
            .into_iter()
            .map(|(key, market)| {
                (
                    key,
                    crate::public::SpotInstrument {
                        symbol: market.symbol,
                        base_currency: market.base_currency,
                        quote_currency: market.quote_currency,
                        trading: market.trading && market.spot_enabled,
                        tick_size: Some(market.tick_size),
                        step_size: Some(market.step_size),
                        min_quantity: Some(market.min_quantity),
                        max_quantity: Some(market.max_quantity),
                        min_notional: Some(market.min_notional),
                    },
                )
            })
            .collect())
    }
    async fn query_book_ticker(&self, symbol: &str) -> Result<bbgo_types::BookTicker> {
        let book = self.depth(symbol, 5).await?;
        let received = crate::public::now()?;
        let bid = book.best_bid().ok_or_else(|| invalid("empty bid side"))?;
        let ask = book.best_ask().ok_or_else(|| invalid("empty ask side"))?;
        crate::public::quote(
            symbol,
            [bid.price, bid.quantity, ask.price, ask.quantity],
            None,
            received,
            0,
        )
    }
}

#[derive(Deserialize)]
struct ExchangeInfo {
    symbols: Vec<Symbol>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Symbol {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
    #[serde(default)]
    is_spot_trading_allowed: bool,
    order_types: Vec<String>,
    filters: Vec<serde_json::Value>,
}
fn decimal(filter: &serde_json::Value, key: &str) -> Result<Value> {
    Value::parse_exact(
        filter
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("missing decimal filter"))?,
    )
    .map_err(Error::from)
}
pub fn decode_markets(body: &[u8]) -> Result<BTreeMap<String, Market>> {
    bounded(body)?;
    let info: ExchangeInfo = serde_json::from_slice(body).map_err(adapter)?;
    let mut markets = BTreeMap::new();
    for symbol in info.symbols {
        check_symbol(&symbol.symbol)?;
        if symbol.base_asset.is_empty()
            || symbol.quote_asset.is_empty()
            || symbol.base_asset == symbol.quote_asset
        {
            return Err(invalid("invalid market currencies"));
        }
        let filter = |kind: &str| -> Result<&serde_json::Value> {
            let mut matching = symbol
                .filters
                .iter()
                .filter(|f| f.get("filterType").and_then(serde_json::Value::as_str) == Some(kind));
            let value = matching
                .next()
                .ok_or_else(|| invalid("missing required filter"))?;
            if matching.next().is_some() {
                return Err(invalid("duplicate market filter"));
            }
            Ok(value)
        };
        let price = filter("PRICE_FILTER")?;
        let lot = filter("LOT_SIZE")?;
        let mut min_notional = Value::ZERO;
        for f in &symbol.filters {
            if matches!(
                f.get("filterType").and_then(serde_json::Value::as_str),
                Some("MIN_NOTIONAL" | "NOTIONAL")
            ) {
                let minimum = decimal(f, "minNotional")?;
                if minimum < Value::ZERO {
                    return Err(invalid("negative notional filter"));
                }
                min_notional = min_notional.max(minimum);
            }
        }
        let market = Market {
            symbol: symbol.symbol.clone(),
            base_currency: symbol.base_asset.clone(),
            quote_currency: symbol.quote_asset.clone(),
            spot_enabled: symbol.is_spot_trading_allowed,
            trading: symbol.status == "TRADING",
            order_types: symbol
                .order_types
                .iter()
                .filter_map(|s| match s.as_str() {
                    "LIMIT" => Some(OrderType::Limit),
                    "LIMIT_MAKER" => Some(OrderType::LimitMaker),
                    "MARKET" => Some(OrderType::Market),
                    _ => None,
                })
                .collect(),
            tick_size: decimal(price, "tickSize")?,
            step_size: decimal(lot, "stepSize")?,
            min_quantity: decimal(lot, "minQty")?,
            max_quantity: decimal(lot, "maxQty")?,
            min_notional,
        };
        if market.tick_size <= Value::ZERO
            || market.step_size <= Value::ZERO
            || market.min_quantity < Value::ZERO
            || market.max_quantity < market.min_quantity
        {
            return Err(invalid("invalid market filters"));
        }
        if markets.insert(market.symbol.clone(), market).is_some() {
            return Err(invalid("duplicate market symbol"));
        }
    }
    Ok(markets)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Depth {
    last_update_id: u64,
    bids: Vec<[Value; 2]>,
    asks: Vec<[Value; 2]>,
}
fn levels(rows: Vec<[Value; 2]>) -> Vec<Level> {
    rows.into_iter()
        .map(|[price, quantity]| Level { price, quantity })
        .collect()
}
fn bounded(body: &[u8]) -> Result<()> {
    if body.len() > MAX_BODY {
        return Err(invalid("public message too large"));
    }
    Ok(())
}
pub fn decode_snapshot(symbol: &str, body: &[u8]) -> Result<OrderBook> {
    check_symbol(symbol)?;
    bounded(body)?;
    let depth: Depth = serde_json::from_slice(body).map_err(adapter)?;
    let mut book = OrderBook::new(symbol);
    book.snapshot(
        symbol,
        depth.last_update_id,
        &levels(depth.bids),
        &levels(depth.asks),
    )?;
    Ok(book)
}

#[derive(Deserialize)]
struct DepthWire {
    e: String,
    #[serde(rename = "E")]
    exchange_time: Timestamp,
    s: String,
    #[serde(rename = "U")]
    first: u64,
    u: u64,
    b: Vec<[Value; 2]>,
    a: Vec<[Value; 2]>,
}
pub struct DepthEvent {
    pub update: BookUpdate,
    pub exchange_time: Timestamp,
    pub receive_time: Timestamp,
    pub generation: u64,
}
/// Decode an unwrapped Binance spot diff-depth event. No clock substitution.
pub fn decode_depth(body: &[u8], receive_time: Timestamp, generation: u64) -> Result<DepthEvent> {
    bounded(body)?;
    let event: DepthWire = serde_json::from_slice(body).map_err(adapter)?;
    check_symbol(&event.s)?;
    if event.e != "depthUpdate"
        || event.exchange_time < 0
        || receive_time < 0
        || event.first > event.u
    {
        return Err(invalid("invalid depth event"));
    }
    Ok(DepthEvent {
        update: BookUpdate {
            symbol: event.s,
            first_sequence: event.first,
            last_sequence: event.u,
            bids: levels(event.b),
            asks: levels(event.a),
        },
        exchange_time: event.exchange_time,
        receive_time,
        generation,
    })
}
impl DepthEvent {
    /// Only returns a quote after this event actually advances a valid book.
    pub fn apply(&self, book: &mut OrderBook) -> Result<Option<Ticker>> {
        if !book.update(&self.update)? {
            return Ok(None);
        }
        let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) else {
            return Ok(None);
        };
        Ok(Some(Ticker {
            symbol: self.update.symbol.clone(),
            bid: bid.price,
            ask: ask.price,
            exchange_time: self.exchange_time,
            receive_time: self.receive_time,
            generation: self.generation,
        }))
    }
}
