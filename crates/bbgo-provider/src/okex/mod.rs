//! OKX V5 spot instruments and BBO. Also exported as `okx`.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-okx")]
pub use rust_okx as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://www.okx.com";
    fn markets_request() -> Request {
        (
            "/api/v5/public/instruments".into(),
            vec![("instType", "SPOT".into())],
        )
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok(("/api/v5/market/ticker".into(), vec![("instId", s.into())]))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(p, "/api/v5/public/instruments" | "/api/v5/market/ticker")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let v = parse(body)?;
        let data = success(&v, "code", "0", "data")?;
        records(data)?;
        let mut out = BTreeMap::new();
        for row in array(data)? {
            if text(row, "instType")? != "SPOT" {
                continue;
            }
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "instId")?.into(),
                    base_currency: text(row, "baseCcy")?.into(),
                    quote_currency: text(row, "quoteCcy")?.into(),
                    trading: text(row, "state")? == "live",
                    tick_size: Some(decimal(row, "tickSz")?),
                    step_size: Some(decimal(row, "lotSz")?),
                    min_quantity: optional(row, "minSz")?,
                    max_quantity: optional(row, "maxLmtSz")?,
                    min_notional: None,
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
        let row = single(success(&v, "code", "0", "data")?, "instId", s)?;
        if text(row, "instType")? != "SPOT" {
            return Err(invalid("not spot ticker"));
        }
        quote(
            s,
            [
                decimal(row, "bidPx")?,
                decimal(row, "bidSz")?,
                decimal(row, "askPx")?,
                decimal(row, "askSz")?,
            ],
            Some(time(&row["ts"], 1)?),
            received,
            generation,
        )
    }
}
