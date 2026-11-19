//! Bybit V5 spot public API. Envelope server time is NOT a quote-event time.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-bybit")]
pub use ::bybit as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api.bybit.com";
    fn markets_request() -> Request {
        (
            "/v5/market/instruments-info".into(),
            vec![("category", "spot".into())],
        )
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok((
            "/v5/market/tickers".into(),
            vec![("category", "spot".into()), ("symbol", s.into())],
        ))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(p, "/v5/market/instruments-info" | "/v5/market/tickers")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let v = parse(body)?;
        let result = success(&v, "retCode", "0", "result")?;
        if text(result, "category")? != "spot"
            || result
                .get("nextPageCursor")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            return Err(invalid(
                "unexpected category or incomplete instruments page",
            ));
        }
        records(&result["list"])?;
        let mut out = BTreeMap::new();
        for row in array(&result["list"])? {
            let lot = &row["lotSizeFilter"];
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "symbol")?.into(),
                    base_currency: text(row, "baseCoin")?.into(),
                    quote_currency: text(row, "quoteCoin")?.into(),
                    trading: text(row, "status")? == "Trading",
                    tick_size: Some(decimal(&row["priceFilter"], "tickSize")?),
                    step_size: Some(decimal(lot, "basePrecision")?),
                    min_quantity: optional(lot, "minOrderQty")?,
                    max_quantity: optional(lot, "maxLimitOrderQty")?
                        .or(optional(lot, "maxOrderQty")?),
                    min_notional: optional(lot, "minOrderAmt")?,
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
        let result = success(&v, "retCode", "0", "result")?;
        if text(result, "category")? != "spot" {
            return Err(invalid("not spot ticker"));
        }
        let row = single(&result["list"], "symbol", s)?;
        quote(
            s,
            [
                decimal(row, "bid1Price")?,
                decimal(row, "bid1Size")?,
                decimal(row, "ask1Price")?,
                decimal(row, "ask1Size")?,
            ],
            None,
            received,
            generation,
        )
    }
}
