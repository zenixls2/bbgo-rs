//! Coinbase Exchange (not Advanced Trade) products and Level 1 book.
//! The ticker's `time` is a last-trade timestamp, not a BBO-event timestamp.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-coinbase")]
pub use coinbase_pro_rs as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api.exchange.coinbase.com";
    fn markets_request() -> Request {
        ("/products".into(), vec![])
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok((format!("/products/{s}/book"), vec![("level", "1".into())]))
    }
    fn allowed_path(p: &str) -> bool {
        p == "/products" || path_symbol(p, "/products/", "/book")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let data = parse(body)?;
        records(&data)?;
        let mut out = BTreeMap::new();
        for row in array(&data)? {
            // Missing disable/auction flags are not silently interpreted as permission.
            let flag = |key| {
                row[key]
                    .as_bool()
                    .ok_or_else(|| invalid("missing Coinbase status flag"))
            };
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "id")?.into(),
                    base_currency: text(row, "base_currency")?.into(),
                    quote_currency: text(row, "quote_currency")?.into(),
                    trading: text(row, "status")? == "online"
                        && !flag("trading_disabled")?
                        && !flag("cancel_only")?
                        && !flag("auction_mode")?,
                    tick_size: Some(decimal(row, "quote_increment")?),
                    step_size: Some(decimal(row, "base_increment")?),
                    min_quantity: optional(row, "base_min_size")?,
                    max_quantity: optional(row, "base_max_size")?,
                    min_notional: optional(row, "min_market_funds")?,
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
        let level = |key| -> Result<_> {
            let levels = array(&v[key])?;
            if levels.len() != 1 {
                return Err(invalid("expected Level 1 book"));
            }
            let row = array(&levels[0])?;
            if row.len() != 3 || integer(&row[2])? <= 0 {
                return Err(invalid("invalid book count"));
            }
            Ok((number(&row[0])?, number(&row[1])?))
        };
        let (bid, bq) = level("bids")?;
        let (ask, aq) = level("asks")?;
        quote(s, [bid, bq, ask, aq], None, received, generation)
    }
}
