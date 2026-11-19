//! Backpack spot instruments and timestamped depth-derived BBO.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-backpack")]
pub use bpx_api_client as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api.backpack.exchange";
    fn markets_request() -> Request {
        (
            "/api/v1/markets".into(),
            vec![("marketType", "SPOT".into())],
        )
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok((
            "/api/v1/depth".into(),
            vec![("symbol", s.into()), ("limit", "5".into())],
        ))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(p, "/api/v1/markets" | "/api/v1/depth")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let data = parse(body)?;
        records(&data)?;
        let mut out = BTreeMap::new();
        for v in array(&data)? {
            if text(v, "marketType")? != "SPOT" {
                continue;
            }
            let filters = &v["filters"];
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(v, "symbol")?.into(),
                    base_currency: text(v, "baseSymbol")?.into(),
                    quote_currency: text(v, "quoteSymbol")?.into(),
                    trading: text(v, "orderBookState")? == "Open",
                    tick_size: Some(decimal(&filters["price"], "tickSize")?),
                    step_size: Some(decimal(&filters["quantity"], "stepSize")?),
                    min_quantity: Some(decimal(&filters["quantity"], "minQuantity")?),
                    max_quantity: optional(&filters["quantity"], "maxQuantity")?,
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
        // Backpack returns both sides ascending; never assume the first bid is best.
        let best = |key: &str, bid: bool| -> Result<(bbgo_types::Value, bbgo_types::Value)> {
            let mut best = None;
            for level in array(&v[key])? {
                let row = array(level)?;
                if row.len() != 2 {
                    return Err(invalid("invalid depth level"));
                }
                let price = number(&row[0])?;
                let qty = number(&row[1])?;
                if price <= bbgo_types::Value::ZERO || qty <= bbgo_types::Value::ZERO {
                    return Err(invalid("invalid depth price or quantity"));
                }
                if best.is_none_or(|(p, _)| if bid { price > p } else { price < p }) {
                    best = Some((price, qty));
                }
            }
            best.ok_or_else(|| invalid("empty depth side"))
        };
        let (bid, bq) = best("bids", true)?;
        let (ask, aq) = best("asks", false)?;
        quote(
            s,
            [bid, bq, ask, aq],
            Some(time(&v["timestamp"], 1000)?),
            received,
            generation,
        )
    }
}
