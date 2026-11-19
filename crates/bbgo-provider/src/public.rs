//! Shared, read-only spot adapter infrastructure. No credentials or order path.
use async_trait::async_trait;
use bbgo_types::{
    BookTicker, Error, Market, MarketDataService, OrderType, Result, Ticker, Timestamp, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::{
    collections::BTreeMap,
    marker::PhantomData,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use crate::binance::PublicTransport;
pub const MAX_BODY: usize = 8 * 1024 * 1024;

/// Provider-independent public discovery/BBO interface, including untimed quotes.
#[async_trait]
pub trait SpotMarketDataService: MarketDataService {
    async fn query_instruments(&self) -> Result<BTreeMap<String, SpotInstrument>>;
    async fn query_book_ticker(&self, symbol: &str) -> Result<BookTicker>;
}

/// Venue-native identifiers are deliberately not concatenated or currency-renamed.
/// Missing bounds remain unknown, never substituted with invented execution limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpotInstrument {
    pub symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub trading: bool,
    pub tick_size: Option<Value>,
    pub step_size: Option<Value>,
    pub min_quantity: Option<Value>,
    pub max_quantity: Option<Value>,
    pub min_notional: Option<Value>,
}
impl SpotInstrument {
    pub fn validate(&self) -> Result<()> {
        symbol(&self.symbol)?;
        symbol(&self.base_currency)?;
        symbol(&self.quote_currency)?;
        if self.base_currency == self.quote_currency
            || self.tick_size.is_some_and(|v| v <= Value::ZERO)
            || self.step_size.is_some_and(|v| v <= Value::ZERO)
            || [self.min_quantity, self.max_quantity, self.min_notional]
                .into_iter()
                .flatten()
                .any(|v| v < Value::ZERO)
            || matches!((self.min_quantity, self.max_quantity), (Some(a), Some(b)) if b < a)
        {
            return Err(invalid("invalid spot instrument filters"));
        }
        Ok(())
    }
    /// Fail closed when the venue's response cannot express BBGO's static filters.
    /// Even complete projections are not account permissions or full venue rules.
    pub fn to_market(&self) -> Result<Market> {
        self.validate()?;
        let required = |value: Option<Value>| {
            value.ok_or_else(|| {
                Error::Unsupported(format!(
                    "{} has incomplete static filters; use query_instruments",
                    self.symbol
                ))
            })
        };
        Ok(Market {
            symbol: self.symbol.clone(),
            base_currency: self.base_currency.clone(),
            quote_currency: self.quote_currency.clone(),
            spot_enabled: true,
            trading: self.trading,
            order_types: vec![OrderType::Limit],
            tick_size: required(self.tick_size)?,
            step_size: required(self.step_size)?,
            min_quantity: required(self.min_quantity)?,
            max_quantity: required(self.max_quantity)?,
            min_notional: required(self.min_notional)?,
        })
    }
}

pub type Request = (String, Vec<(&'static str, String)>);
/// Provider implementations use fixed origins and allowlisted paths.
pub trait Specification: Send + Sync + 'static {
    const ORIGIN: &'static str;
    fn markets_request() -> Request;
    fn quote_request(symbol: &str) -> Result<Request>;
    fn allowed_path(path: &str) -> bool;
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>>;
    fn decode_book(
        symbol: &str,
        body: &[u8],
        received: Timestamp,
        generation: u64,
    ) -> Result<BookTicker>;
}

pub struct HttpTransport<S> {
    client: reqwest::Client,
    marker: PhantomData<S>,
}
impl<S: Specification> HttpTransport<S> {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("bbgo-rs/", env!("CARGO_PKG_VERSION")))
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(adapter)?,
            marker: PhantomData,
        })
    }
}
#[async_trait]
impl<S: Specification> PublicTransport for HttpTransport<S> {
    async fn get(&self, path: &str, parameters: &[(&str, String)]) -> Result<Vec<u8>> {
        if !S::allowed_path(path) {
            return Err(Error::Unsupported("public endpoint not allowed".into()));
        }
        let mut response = self
            .client
            .get(format!("{}{path}", S::ORIGIN))
            .query(parameters)
            .send()
            .await
            .map_err(adapter)?;
        if !response.status().is_success() {
            return Err(Error::Adapter(format!("public HTTP {}", response.status())));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(invalid("public response too large"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(adapter)? {
            if chunk.len() > MAX_BODY - body.len() {
                return Err(invalid("public response too large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}
pub struct PublicClient<S, T> {
    transport: T,
    marker: PhantomData<S>,
}
impl<S: Specification, T: PublicTransport> PublicClient<S, T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            marker: PhantomData,
        }
    }
    pub async fn query_instruments(&self) -> Result<BTreeMap<String, SpotInstrument>> {
        let (path, parameters) = S::markets_request();
        S::decode_instruments(&self.transport.get(&path, &parameters).await?)
    }
    pub async fn query_book_ticker(&self, symbol: &str) -> Result<BookTicker> {
        let (path, parameters) = S::quote_request(symbol)?;
        let body = self.transport.get(&path, &parameters).await?;
        let received = now()?;
        S::decode_book(symbol, &body, received, 0)
    }
}
#[async_trait]
impl<S: Specification, T: PublicTransport> SpotMarketDataService for PublicClient<S, T> {
    async fn query_instruments(&self) -> Result<BTreeMap<String, SpotInstrument>> {
        PublicClient::query_instruments(self).await
    }
    async fn query_book_ticker(&self, symbol: &str) -> Result<BookTicker> {
        PublicClient::query_book_ticker(self, symbol).await
    }
}
#[async_trait]
impl<S: Specification, T: PublicTransport> MarketDataService for PublicClient<S, T> {
    async fn query_markets(&self) -> Result<BTreeMap<String, Market>> {
        self.query_instruments()
            .await?
            .into_iter()
            .map(|(key, instrument)| Ok((key, instrument.to_market()?)))
            .collect()
    }
    async fn query_ticker(&self, symbol: &str) -> Result<Ticker> {
        let quote = self.query_book_ticker(symbol).await?;
        Ok(Ticker {
            symbol: quote.symbol,
            bid: quote.bid,
            ask: quote.ask,
            exchange_time: quote.exchange_time.ok_or_else(|| {
                Error::Unsupported(
                    "quote has no exchange-event timestamp; use query_book_ticker".into(),
                )
            })?,
            receive_time: quote.receive_time,
            generation: quote.generation,
        })
    }
}
pub(crate) fn adapter(error: impl std::fmt::Display) -> Error {
    Error::Adapter(error.to_string())
}
pub(crate) fn now() -> Result<Timestamp> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(adapter)?
        .as_millis();
    i64::try_from(millis).map_err(adapter)
}
pub(crate) fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
pub(crate) fn symbol(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
    {
        return Err(invalid("invalid venue symbol"));
    }
    Ok(())
}
pub(crate) fn parse(body: &[u8]) -> Result<Json> {
    if body.len() > MAX_BODY {
        return Err(invalid("public response too large"));
    }
    serde_json::from_slice(body).map_err(adapter)
}
pub(crate) fn text<'a>(value: &'a Json, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| invalid("missing string field"))
}
pub(crate) fn decimal(value: &Json, key: &str) -> Result<Value> {
    number(value.get(key).ok_or_else(|| invalid("missing decimal"))?)
}
/// Numeric JSON is preserved lexically, not converted through f64.
pub(crate) fn number(value: &Json) -> Result<Value> {
    let owned;
    let text = if let Some(s) = value.as_str() {
        s
    } else if value.is_number() {
        owned = value.to_string();
        &owned
    } else {
        return Err(invalid("invalid decimal"));
    };
    if text.len() > 256 || text.ends_with('%') {
        return Err(invalid("decimal too long or percentage used as amount"));
    }
    // JSON permits scientific notation even when the legacy decimal parser
    // does not. Expand its lexical digits exactly, never via a binary float.
    let expanded;
    let text = if let Some((mantissa, exponent)) = text.split_once(['e', 'E']) {
        let exponent: i32 = exponent.parse().map_err(adapter)?;
        if !(-256..=256).contains(&exponent) {
            return Err(invalid("decimal exponent out of bounds"));
        }
        let (sign, unsigned) = if let Some(s) = mantissa.strip_prefix('-') {
            ("-", s)
        } else {
            ("", mantissa.strip_prefix('+').unwrap_or(mantissa))
        };
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        if whole.is_empty() && fraction.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(invalid("invalid decimal mantissa"));
        }
        let digits = format!("{whole}{fraction}");
        let point = whole.len() as i32 + exponent;
        expanded = if point <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        } else if point as usize >= digits.len() {
            format!(
                "{sign}{digits}{}",
                "0".repeat(point as usize - digits.len())
            )
        } else {
            format!(
                "{sign}{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        };
        &expanded
    } else {
        text
    };
    Value::parse_exact(text).map_err(Into::into)
}
pub(crate) fn optional(value: &Json, key: &str) -> Result<Option<Value>> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.is_empty() => Ok(None),
        Some(v) => Ok(Some(number(v)?)),
    }
}
pub(crate) fn integer(value: &Json) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| invalid("invalid integer"))
}
pub(crate) fn time(value: &Json, divisor: i64) -> Result<Timestamp> {
    let raw = integer(value)?;
    if raw <= 0 || raw / divisor <= 0 {
        return Err(invalid("invalid exchange timestamp"));
    }
    Ok(raw / divisor)
}
pub(crate) fn precision(value: &Json) -> Result<Value> {
    let n = integer(value)?;
    if !(0..=128).contains(&n) {
        return Err(invalid("invalid decimal precision"));
    }
    let s = if n == 0 {
        "1".to_owned()
    } else {
        format!("0.{}1", "0".repeat(n as usize - 1))
    };
    Value::parse_exact(&s).map_err(Into::into)
}
pub(crate) fn array(value: &Json) -> Result<&Vec<Json>> {
    value.as_array().ok_or_else(|| invalid("expected array"))
}
pub(crate) fn records(value: &Json) -> Result<()> {
    if array(value)?.len() > 100_000 {
        return Err(invalid("too many public records"));
    }
    Ok(())
}
pub(crate) fn insert(
    markets: &mut BTreeMap<String, SpotInstrument>,
    instrument: SpotInstrument,
) -> Result<()> {
    instrument.validate()?;
    if markets
        .insert(instrument.symbol.clone(), instrument)
        .is_some()
    {
        return Err(invalid("duplicate venue symbol"));
    }
    Ok(())
}
pub(crate) fn quote(
    symbol_id: &str,
    prices: [Value; 4],
    exchange: Option<Timestamp>,
    received: Timestamp,
    generation: u64,
) -> Result<BookTicker> {
    symbol(symbol_id)?;
    if received < 0 {
        return Err(invalid("negative receive timestamp"));
    }
    let [bid, bid_quantity, ask, ask_quantity] = prices;
    let quote = BookTicker {
        symbol: symbol_id.into(),
        bid,
        bid_quantity,
        ask,
        ask_quantity,
        exchange_time: exchange,
        receive_time: received,
        generation,
    };
    quote.midpoint()?;
    Ok(quote)
}
pub(crate) fn single<'a>(value: &'a Json, key: &str, symbol: &str) -> Result<&'a Json> {
    let rows = array(value)?;
    if rows.len() != 1 || text(&rows[0], key)? != symbol {
        return Err(invalid("unexpected ticker identity or count"));
    }
    Ok(&rows[0])
}
pub(crate) fn success<'a>(value: &'a Json, key: &str, code: &str, data: &str) -> Result<&'a Json> {
    let actual = value
        .get(key)
        .ok_or_else(|| invalid("missing API status"))?;
    let valid = actual.as_str().map(|s| s == code).unwrap_or_else(|| {
        actual
            .as_i64()
            .is_some_and(|number| code.parse::<i64>() == Ok(number))
    });
    if !valid {
        return Err(Error::Adapter("exchange API rejected request".into()));
    }
    value
        .get(data)
        .ok_or_else(|| invalid("missing response data"))
}
pub(crate) fn path_symbol(path: &str, prefix: &str, suffix: &str) -> bool {
    path.strip_prefix(prefix)
        .and_then(|p| p.strip_suffix(suffix))
        .is_some_and(|s| symbol(s).is_ok())
}

// Each module gets a conventional, statically specialized client and transport.
macro_rules! provider_api {
    () => {
        pub type PublicClient<T> = crate::public::PublicClient<Specification, T>;
        pub type HttpTransport = crate::public::HttpTransport<Specification>;
        pub use crate::public::PublicTransport;
        pub fn decode_instruments(
            body: &[u8],
        ) -> bbgo_types::Result<std::collections::BTreeMap<String, crate::public::SpotInstrument>> {
            <Specification as crate::public::Specification>::decode_instruments(body)
        }
        pub fn decode_book_ticker(
            symbol: &str,
            body: &[u8],
            received: bbgo_types::Timestamp,
            generation: u64,
        ) -> bbgo_types::Result<bbgo_types::BookTicker> {
            <Specification as crate::public::Specification>::decode_book(
                symbol, body, received, generation,
            )
        }
    };
}
pub(crate) use provider_api;
