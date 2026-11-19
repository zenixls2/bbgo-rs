//! MaiCoin MAX V3 spot public API.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-max")]
pub use maicoin_max as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://max-api.maicoin.com";
    fn markets_request() -> Request {
        ("/api/v3/markets".into(), vec![])
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok(("/api/v3/ticker".into(), vec![("market", s.into())]))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(p, "/api/v3/markets" | "/api/v3/ticker")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let data = parse(body)?;
        records(&data)?;
        let mut out = BTreeMap::new();
        for row in array(&data)? {
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "id")?.into(),
                    base_currency: text(row, "base_unit")?.into(),
                    quote_currency: text(row, "quote_unit")?.into(),
                    trading: text(row, "market_status")? == "active",
                    tick_size: Some(precision(&row["quote_unit_precision"])?),
                    step_size: Some(precision(&row["base_unit_precision"])?),
                    min_quantity: optional(row, "min_base_amount")?,
                    max_quantity: None,
                    min_notional: optional(row, "min_quote_amount")?,
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
        if text(&v, "market")? != s {
            return Err(invalid("ticker symbol mismatch"));
        }
        quote(
            s,
            [
                decimal(&v, "buy")?,
                decimal(&v, "buy_vol")?,
                decimal(&v, "sell")?,
                decimal(&v, "sell_vol")?,
            ],
            Some(time(&v["at"], 1)?),
            received,
            generation,
        )
    }
}
