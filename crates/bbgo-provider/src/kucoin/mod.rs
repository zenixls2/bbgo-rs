//! KuCoin spot symbols and Level 1 BBO.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-kucoin")]
pub use kucoin_rs as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api.kucoin.com";
    fn markets_request() -> Request {
        ("/api/v2/symbols".into(), vec![])
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok((
            "/api/v1/market/orderbook/level1".into(),
            vec![("symbol", s.into())],
        ))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(p, "/api/v2/symbols" | "/api/v1/market/orderbook/level1")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let v = parse(body)?;
        let data = success(&v, "code", "200000", "data")?;
        records(data)?;
        let mut out = BTreeMap::new();
        for row in array(data)? {
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "symbol")?.into(),
                    base_currency: text(row, "baseCurrency")?.into(),
                    quote_currency: text(row, "quoteCurrency")?.into(),
                    trading: row["enableTrading"]
                        .as_bool()
                        .ok_or_else(|| invalid("missing trading status"))?,
                    tick_size: Some(decimal(row, "priceIncrement")?),
                    step_size: Some(decimal(row, "baseIncrement")?),
                    min_quantity: optional(row, "baseMinSize")?,
                    max_quantity: optional(row, "baseMaxSize")?,
                    min_notional: optional(row, "minFunds")?,
                },
            )?;
        }
        Ok(out)
    }
    fn decode_book(
        s: &str,
        body: &[u8],
        received: Timestamp,
        generation: u64,
    ) -> Result<BookTicker> {
        symbol(s)?;
        let v = parse(body)?;
        let row = success(&v, "code", "200000", "data")?;
        quote(
            s,
            [
                decimal(row, "bestBid")?,
                decimal(row, "bestBidSize")?,
                decimal(row, "bestAsk")?,
                decimal(row, "bestAskSize")?,
            ],
            Some(time(&row["time"], 1)?),
            received,
            generation,
        )
    }
}
