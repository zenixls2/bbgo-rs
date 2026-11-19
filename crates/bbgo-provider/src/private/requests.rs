use super::Method;
use crate::{ProviderId, public};
use bbgo_types::{Order, Result, Side, SubmitOrder};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

pub(super) struct Request {
    pub method: Method,
    pub path: String,
    pub params: BTreeMap<String, String>,
    pub body: Json,
    pub instruction: &'static str,
    pub write: bool,
}
fn request(
    method: Method,
    path: &str,
    params: &[(&str, &str)],
    body: Json,
    instruction: &'static str,
    write: bool,
) -> Request {
    Request {
        method,
        path: path.into(),
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body,
        instruction,
        write,
    }
}
pub(super) fn client_id(provider: ProviderId, id: &str) -> Result<()> {
    let max = match provider {
        ProviderId::Okex => 32,
        ProviderId::Binance | ProviderId::Bybit => 36,
        ProviderId::Bitget => 64,
        ProviderId::Kucoin => 40,
        ProviderId::Max => 36,
        ProviderId::Backpack | ProviderId::Bitfinex => 20,
        ProviderId::Coinbase => 36,
    };
    if id.is_empty()
        || id.len() > max
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(public::invalid("invalid venue client order ID"));
    }
    match provider {
        ProviderId::Backpack => {
            let n = id
                .parse::<u32>()
                .map_err(|_| public::invalid("Backpack client ID must be uint32"))?;
            if n == 0 || n.to_string() != id {
                return Err(public::invalid("noncanonical numeric client ID"));
            }
        }
        ProviderId::Bitfinex => {
            let n = id
                .parse::<u64>()
                .map_err(|_| public::invalid("Bitfinex client ID must be numeric"))?;
            if n == 0 || n > (1u64 << 45) - 1 || n.to_string() != id {
                return Err(public::invalid("Bitfinex client ID must fit 45 bits"));
            }
        }
        ProviderId::Coinbase => {
            if id.len() != 36
                || id.bytes().enumerate().any(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b != b'-'
                    } else {
                        !b.is_ascii_hexdigit()
                    }
                })
            {
                return Err(public::invalid("Coinbase client ID must be UUID"));
            }
        }
        ProviderId::Okex if !id.bytes().all(|b| b.is_ascii_alphanumeric()) => {
            return Err(public::invalid("OKX client ID must be alphanumeric"));
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn balances(p: ProviderId) -> Request {
    use Method::*;
    match p {
        ProviderId::Binance => request(Get, "/api/v3/account", &[], Json::Null, "", false),
        ProviderId::Backpack => request(
            Get,
            "/api/v1/capital",
            &[],
            Json::Null,
            "balanceQuery",
            false,
        ),
        ProviderId::Bitfinex => request(Post, "/v2/auth/r/wallets", &[], json!({}), "", false),
        ProviderId::Bitget => request(
            Get,
            "/api/v2/spot/account/assets",
            &[("assetType", "all")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Bybit => request(
            Get,
            "/v5/account/wallet-balance",
            &[("accountType", "UNIFIED")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Coinbase => request(Get, "/accounts", &[], Json::Null, "", false),
        ProviderId::Kucoin => request(
            Get,
            "/api/v1/accounts",
            &[("type", "trade")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Max => request(
            Get,
            "/api/v3/wallet/spot/accounts",
            &[],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Okex => request(Get, "/api/v5/account/balance", &[], Json::Null, "", false),
    }
}
pub(super) fn submit(p: ProviderId, o: &SubmitOrder) -> Result<Request> {
    let side = match o.side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let price = wire_decimal(o.price)?;
    let qty = wire_decimal(o.quantity)?;
    let cid = &o.client_order_id;
    let symbol = &o.symbol;
    use Method::*;
    Ok(match p {
        ProviderId::Binance => request(
            Post,
            "/api/v3/order",
            &[
                ("symbol", symbol),
                ("side", if o.side == Side::Buy { "BUY" } else { "SELL" }),
                ("type", "LIMIT"),
                ("timeInForce", "GTC"),
                ("quantity", &qty),
                ("price", &price),
                ("newClientOrderId", cid),
                ("newOrderRespType", "RESULT"),
            ],
            Json::Null,
            "",
            true,
        ),
        ProviderId::Backpack => request(
            Post,
            "/api/v1/order",
            &[],
            json!({"symbol":symbol,"side":if o.side==Side::Buy {"Bid"}else{"Ask"},"orderType":"Limit","quantity":qty,"price":price,"timeInForce":"GTC","clientId":cid.parse::<u32>().map_err(|_| public::invalid("invalid client ID"))?,"autoBorrow":false,"autoBorrowRepay":false,"autoLend":false,"autoLendRedeem":false}),
            "orderExecute",
            true,
        ),
        ProviderId::Bitfinex => request(
            Post,
            "/v2/auth/w/order/submit",
            &[],
            json!({"type":"EXCHANGE LIMIT","symbol":format!("t{symbol}"),"amount":if o.side==Side::Buy {qty}else{format!("-{qty}")},"price":price,"cid":cid.parse::<u64>().map_err(|_| public::invalid("invalid client ID"))?}),
            "",
            true,
        ),
        ProviderId::Bitget => request(
            Post,
            "/api/v2/spot/trade/place-order",
            &[],
            json!({"symbol":symbol,"side":side,"orderType":"limit","force":"gtc","price":price,"size":qty,"clientOid":cid}),
            "",
            true,
        ),
        ProviderId::Bybit => request(
            Post,
            "/v5/order/create",
            &[],
            json!({"category":"spot","symbol":symbol,"side":if o.side==Side::Buy {"Buy"}else{"Sell"},"orderType":"Limit","qty":qty,"price":price,"timeInForce":"GTC","orderLinkId":cid,"isLeverage":0}),
            "",
            true,
        ),
        ProviderId::Coinbase => request(
            Post,
            "/orders",
            &[],
            json!({"product_id":symbol,"side":side,"type":"limit","size":qty,"price":price,"time_in_force":"GTC","client_oid":cid}),
            "",
            true,
        ),
        ProviderId::Kucoin => request(
            Post,
            "/api/v1/orders",
            &[],
            json!({"symbol":symbol,"side":side,"type":"limit","size":qty,"price":price,"timeInForce":"GTC","clientOid":cid}),
            "",
            true,
        ),
        ProviderId::Max => request(
            Post,
            "/api/v3/wallet/spot/order",
            &[],
            json!({"market":symbol,"side":side,"ord_type":"limit","volume":qty,"price":price,"client_oid":cid}),
            "",
            true,
        ),
        ProviderId::Okex => request(
            Post,
            "/api/v5/trade/order",
            &[],
            json!({"instId":symbol,"tdMode":"cash","side":side,"ordType":"limit","sz":qty,"px":price,"clOrdId":cid}),
            "",
            true,
        ),
    })
}
// Decimal strings on trading APIs must not depend on the backend's scientific
// display thresholds. All supported exponents are bounded by the numeric backend.
fn wire_decimal(value: bbgo_types::Value) -> Result<String> {
    let raw = value.to_string();
    let Some((mantissa, exponent)) = raw.split_once('e') else {
        return Ok(raw);
    };
    let exponent = exponent
        .parse::<i32>()
        .map_err(|_| public::invalid("invalid decimal exponent"))?;
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map(|s| ("-", s))
        .unwrap_or(("", mantissa));
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let digits = mantissa.replace('.', "");
    if decimal.unsigned_abs() > 512 {
        return Err(public::invalid("wire decimal too large"));
    }
    let expanded = if decimal <= 0 {
        format!("{sign}0.{}{}", "0".repeat((-decimal) as usize), digits)
    } else if decimal as usize >= digits.len() {
        format!(
            "{sign}{digits}{}",
            "0".repeat(decimal as usize - digits.len())
        )
    } else {
        format!(
            "{sign}{}.{}",
            &digits[..decimal as usize],
            &digits[decimal as usize..]
        )
    };
    Ok(expanded)
}

pub(super) fn open(p: ProviderId, s: &str) -> Request {
    use Method::*;
    match p {
        ProviderId::Binance => request(
            Get,
            "/api/v3/openOrders",
            &[("symbol", s)],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Backpack => request(
            Get,
            "/api/v1/orders",
            &[("symbol", s), ("marketType", "SPOT")],
            Json::Null,
            "orderQueryAll",
            false,
        ),
        ProviderId::Bitfinex => request(
            Post,
            &format!("/v2/auth/r/orders/t{s}"),
            &[],
            json!({}),
            "",
            false,
        ),
        ProviderId::Bitget => request(
            Get,
            "/api/v2/spot/trade/unfilled-orders",
            &[("symbol", s), ("limit", "100")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Bybit => request(
            Get,
            "/v5/order/realtime",
            &[
                ("category", "spot"),
                ("symbol", s),
                ("openOnly", "0"),
                ("limit", "50"),
            ],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Coinbase => request(
            Get,
            "/orders",
            &[("product_id", s), ("status", "open"), ("limit", "100")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Kucoin => request(
            Get,
            "/api/v1/orders",
            &[
                ("symbol", s),
                ("status", "active"),
                ("pageSize", "500"),
                ("currentPage", "1"),
            ],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Max => request(
            Get,
            "/api/v3/wallet/spot/orders/open",
            &[("market", s), ("limit", "1000")],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Okex => request(
            Get,
            "/api/v5/trade/orders-pending",
            &[("instType", "SPOT"), ("instId", s), ("limit", "100")],
            Json::Null,
            "",
            false,
        ),
    }
}
pub(super) fn cancel(p: ProviderId, o: &Order) -> Result<Request> {
    public::symbol(&o.request.symbol)?;
    // IDs go into path components for Coinbase/KuCoin. Strict ASCII prevents traversal.
    if o.id.is_empty()
        || o.id.len() > 128
        || !o.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(public::invalid("invalid cancel order ID"));
    }
    let s = &o.request.symbol;
    let id = &o.id;
    use Method::*;
    Ok(match p {
        ProviderId::Binance => request(
            Delete,
            "/api/v3/order",
            &[("symbol", s), ("orderId", id)],
            Json::Null,
            "",
            true,
        ),
        ProviderId::Backpack => request(
            Delete,
            "/api/v1/order",
            &[],
            json!({"symbol":s,"orderId":id}),
            "orderCancel",
            true,
        ),
        ProviderId::Bitfinex => request(
            Post,
            "/v2/auth/w/order/cancel",
            &[],
            json!({"id":id.parse::<u64>().map_err(|_|public::invalid("Bitfinex order ID must be numeric"))?}),
            "",
            true,
        ),
        ProviderId::Bitget => request(
            Post,
            "/api/v2/spot/trade/cancel-order",
            &[],
            json!({"symbol":s,"orderId":id}),
            "",
            true,
        ),
        ProviderId::Bybit => request(
            Post,
            "/v5/order/cancel",
            &[],
            json!({"category":"spot","symbol":s,"orderId":id}),
            "",
            true,
        ),
        ProviderId::Coinbase => {
            request(Delete, &format!("/orders/{id}"), &[], Json::Null, "", true)
        }
        ProviderId::Kucoin => request(
            Delete,
            &format!("/api/v1/orders/{id}"),
            &[],
            Json::Null,
            "",
            true,
        ),
        ProviderId::Max => request(
            Delete,
            "/api/v3/order",
            &[],
            json!({"id":id.parse::<u64>().map_err(|_|public::invalid("MAX order ID must be numeric"))?}),
            "",
            true,
        ),
        ProviderId::Okex => request(
            Post,
            "/api/v5/trade/cancel-order",
            &[],
            json!({"instId":s,"ordId":id}),
            "",
            true,
        ),
    })
}
pub(super) fn lookup(p: ProviderId, s: &str, id: &str, client: bool) -> Result<Request> {
    public::symbol(s)?;
    if client {
        client_id(p, id)?;
    } else if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(public::invalid("invalid order lookup ID"));
    }
    use Method::*;
    Ok(match p {
        ProviderId::Binance => request(
            Get,
            "/api/v3/order",
            &[
                ("symbol", s),
                (
                    if client {
                        "origClientOrderId"
                    } else {
                        "orderId"
                    },
                    id,
                ),
            ],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Backpack => request(
            Get,
            "/api/v1/order",
            &[
                ("symbol", s),
                (if client { "clientId" } else { "orderId" }, id),
            ],
            Json::Null,
            "orderQuery",
            false,
        ),
        ProviderId::Bitfinex => {
            if client {
                return Err(bbgo_types::Error::Unsupported("Bitfinex client IDs are day-scoped; use exchange order ID or reconcile history with a date".into()));
            }
            id.parse::<u64>()
                .map_err(|_| public::invalid("Bitfinex order ID must be numeric"))?;
            request(
                Post,
                &format!("/v2/auth/r/orders/t{s}"),
                &[],
                json!({"id":[id.parse::<u64>().map_err(|_|public::invalid("invalid Bitfinex ID"))?]}),
                "",
                false,
            )
        }
        ProviderId::Bitget => request(
            Get,
            "/api/v2/spot/trade/orderInfo",
            &[(if client { "clientOid" } else { "orderId" }, id)],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Bybit => request(
            Get,
            "/v5/order/realtime",
            &[
                ("category", "spot"),
                ("symbol", s),
                (if client { "orderLinkId" } else { "orderId" }, id),
            ],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Coinbase => request(
            Get,
            &format!("/orders/{}{id}", if client { "client:" } else { "" }),
            &[],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Kucoin => request(
            Get,
            &if client {
                format!("/api/v1/order/client-order/{id}")
            } else {
                format!("/api/v1/orders/{id}")
            },
            &[],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Max => request(
            Get,
            "/api/v3/order",
            &[(if client { "client_oid" } else { "id" }, id)],
            Json::Null,
            "",
            false,
        ),
        ProviderId::Okex => request(
            Get,
            "/api/v5/trade/order",
            &[
                ("instId", s),
                (if client { "clOrdId" } else { "ordId" }, id),
            ],
            Json::Null,
            "",
            false,
        ),
    })
}

pub(super) fn bitfinex_history(s: &str, id: &str) -> Result<Request> {
    public::symbol(s)?;
    let id = id
        .parse::<u64>()
        .map_err(|_| public::invalid("invalid Bitfinex ID"))?;
    Ok(request(
        Method::Post,
        &format!("/v2/auth/r/orders/t{s}/hist"),
        &[],
        json!({"id":[id],"limit":1}),
        "",
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_decimals_are_plain_and_exact_for_the_selected_backend() {
        for text in ["0.00000001", "100", "123.45"] {
            let value = bbgo_types::Value::parse_exact(text).unwrap();
            assert_eq!(wire_decimal(value).unwrap(), text);
        }
        #[cfg(feature = "dnum")]
        {
            let value = bbgo_types::Value::parse_exact("1e-100").unwrap();
            let wire = wire_decimal(value).unwrap();
            assert!(!wire.contains('e'));
            assert_eq!(bbgo_types::Value::parse_exact(&wire).unwrap(), value);
        }
    }
}
