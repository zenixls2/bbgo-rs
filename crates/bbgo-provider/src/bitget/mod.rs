//! Bitget V2 spot public API.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-bitget")]
pub use bitget_sdk as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api.bitget.com";
    fn markets_request() -> Request {
        ("/api/v2/spot/public/symbols".into(), vec![])
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        Ok((
            "/api/v2/spot/market/tickers".into(),
            vec![("symbol", s.into())],
        ))
    }
    fn allowed_path(p: &str) -> bool {
        matches!(
            p,
            "/api/v2/spot/public/symbols" | "/api/v2/spot/market/tickers"
        )
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let v = parse(body)?;
        let data = success(&v, "code", "00000", "data")?;
        records(data)?;
        let mut out = BTreeMap::new();
        for row in array(data)? {
            insert(
                &mut out,
                SpotInstrument {
                    symbol: text(row, "symbol")?.into(),
                    base_currency: text(row, "baseCoin")?.into(),
                    quote_currency: text(row, "quoteCoin")?.into(),
                    trading: text(row, "status")? == "online",
                    tick_size: Some(precision(&row["pricePrecision"])?),
                    step_size: Some(precision(&row["quantityPrecision"])?),
                    min_quantity: optional(row, "minTradeAmount")?,
                    max_quantity: optional(row, "maxTradeAmount")?,
                    // minTradeUSDT is denominated in USDT, not necessarily the quote currency.
                    min_notional: if text(row, "quoteCoin")? == "USDT" {
                        optional(row, "minTradeUSDT")?
                    } else {
                        None
                    },
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
        let row = single(success(&v, "code", "00000", "data")?, "symbol", s)?;
        quote(
            s,
            [
                decimal(row, "bidPr")?,
                decimal(row, "bidSz")?,
                decimal(row, "askPr")?,
                decimal(row, "askSz")?,
            ],
            Some(time(&row["ts"], 1)?),
            received,
            generation,
        )
    }
}
