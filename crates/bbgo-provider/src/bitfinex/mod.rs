//! Bitfinex public pair configuration and aggregated book (V2).
//! Native currency aliases are preserved. Dynamic significant-digit price
//! precision cannot be represented as a single BBGO tick size.
use crate::public::*;
use bbgo_types::{BookTicker, Result, Timestamp, Value};
use std::collections::BTreeMap;
pub struct Specification;
provider_api!();
#[cfg(feature = "sdk-bitfinex")]
pub use bfx as sdk;
impl crate::public::Specification for Specification {
    const ORIGIN: &'static str = "https://api-pub.bitfinex.com";
    fn markets_request() -> Request {
        ("/v2/conf/pub:info:pair".into(), vec![])
    }
    fn quote_request(s: &str) -> Result<Request> {
        symbol(s)?;
        if s != s.to_ascii_uppercase() {
            return Err(invalid("Bitfinex pair must be uppercase without t prefix"));
        }
        Ok((format!("/v2/book/t{s}/P0"), vec![("len", "1".into())]))
    }
    fn allowed_path(p: &str) -> bool {
        p == "/v2/conf/pub:info:pair" || path_symbol(p, "/v2/book/t", "/P0")
    }
    fn decode_instruments(body: &[u8]) -> Result<BTreeMap<String, SpotInstrument>> {
        let data = parse(body)?;
        records(&data)?;
        let mut out = BTreeMap::new();
        if array(&data)?.len() != 1 {
            return Err(invalid("unexpected pair configuration wrapper"));
        }
        for row in array(&data[0])? {
            let row = array(row)?;
            if row.len() != 2 {
                return Err(invalid("invalid pair configuration"));
            }
            let pair = row[0]
                .as_str()
                .ok_or_else(|| invalid("missing pair"))?
                .to_ascii_uppercase();
            symbol(&pair)?;
            let config = array(&row[1])?;
            if config.len() < 5 {
                return Err(invalid("incomplete pair configuration"));
            }
            let (base, quote) = if let Some(parts) = pair.split_once(':') {
                parts
            } else if pair.len() == 6 {
                pair.split_at(3)
            } else {
                return Err(invalid(
                    "ambiguous Bitfinex pair; expected 3+3 or colon-delimited currencies",
                ));
            };
            insert(
                &mut out,
                SpotInstrument {
                    symbol: pair.clone(),
                    base_currency: base.into(),
                    quote_currency: quote.into(),
                    // Pair details list active exchange pairs; this is not account eligibility.
                    trading: true,
                    tick_size: None,
                    step_size: None,
                    min_quantity: Some(number(&config[3])?),
                    max_quantity: Some(number(&config[4])?),
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
        Self::quote_request(s)?;
        let v = parse(body)?;
        let mut bid = None;
        let mut ask = None;
        for row in array(&v)? {
            let row = array(row)?;
            if row.len() != 3 || integer(&row[1])? <= 0 {
                return Err(invalid("invalid book level"));
            }
            let price = number(&row[0])?;
            let amount = number(&row[2])?;
            if price <= Value::ZERO || amount == Value::ZERO {
                return Err(invalid("invalid book price or amount"));
            }
            let side = if amount > Value::ZERO {
                &mut bid
            } else {
                &mut ask
            };
            if side.is_some() {
                return Err(invalid("expected one level per side"));
            }
            *side = Some((
                price,
                if amount < Value::ZERO {
                    Value::ZERO.checked_sub(amount)?
                } else {
                    amount
                },
            ));
        }
        let (bid, bq) = bid.ok_or_else(|| invalid("missing bid"))?;
        let (ask, aq) = ask.ok_or_else(|| invalid("missing ask"))?;
        quote(s, [bid, bq, ask, aq], None, received, generation)
    }
}
