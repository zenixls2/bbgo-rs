use crate::{ProviderId, public::*};
use bbgo_types::{
    Balance, Balances, Error, Order, OrderStatus, OrderType, Result, Side, SubmitOrder, Value,
};
use serde_json::Value as Json;
use std::collections::BTreeSet;

fn parse_private(body: &[u8]) -> Result<Json> {
    parse(body).map_err(|_| invalid("invalid private JSON response"))
}
fn data(p: ProviderId, v: &Json) -> Result<&Json> {
    match p {
        ProviderId::Bitget => success(v, "code", "00000", "data"),
        ProviderId::Bybit => success(v, "retCode", "0", "result"),
        ProviderId::Kucoin => success(v, "code", "200000", "data"),
        ProviderId::Okex => success(v, "code", "0", "data"),
        _ => {
            if v.get("error").is_some()
                || v.get("errors").is_some()
                || (v.get("code").is_some() && !matches!(p, ProviderId::Backpack))
            {
                return Err(Error::Adapter("private API rejected request".into()));
            }
            Ok(v)
        }
    }
}
fn id(v: &Json) -> Result<String> {
    let id = match v {
        Json::String(s) => s.clone(),
        Json::Number(n) if n.as_u64().is_some() => n.to_string(),
        _ => return Err(invalid("missing order identity")),
    };
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid("invalid response identity"));
    }
    Ok(id)
}
fn field_id(v: &Json, key: &str) -> Result<String> {
    id(&v[key])
}
fn optional_id(v: &Json, key: &str) -> Result<String> {
    match v.get(key) {
        None | Some(Json::Null) => Ok(String::new()),
        Some(Json::String(s)) if s.is_empty() => Ok(String::new()),
        Some(n) => id(n),
    }
}
fn insert_balance(out: &mut Balances, ccy: &str, available: Value, locked: Value) -> Result<()> {
    symbol(ccy)?;
    if available < Value::ZERO || locked < Value::ZERO {
        return Err(invalid("negative cash balance"));
    }
    available.checked_add(locked)?;
    if out
        .insert(ccy.into(), Balance { available, locked })
        .is_some()
    {
        return Err(invalid("duplicate currency balance"));
    }
    Ok(())
}
pub(super) fn balances(p: ProviderId, body: &[u8]) -> Result<Balances> {
    let v = parse_private(body)?;
    let d = data(p, &v)?;
    let mut out = Balances::new();
    match p {
        ProviderId::Backpack => {
            let obj = d
                .as_object()
                .ok_or_else(|| invalid("invalid balance map"))?;
            for (ccy, row) in obj {
                insert_balance(
                    &mut out,
                    ccy,
                    decimal(row, "available")?,
                    decimal(row, "locked")?.checked_add(decimal(row, "staked")?)?,
                )?;
            }
        }
        ProviderId::Bitfinex => {
            for row in array(d)? {
                let r = array(row)?;
                if r.len() < 5 {
                    return Err(invalid("invalid wallet row"));
                }
                if r[0].as_str() != Some("exchange") {
                    continue;
                }
                let currency = r[1]
                    .as_str()
                    .ok_or_else(|| invalid("invalid wallet currency"))?;
                let total = number(&r[2])?;
                let available = number(&r[4])?; // null is unknown, never total-as-available.
                insert_balance(&mut out, currency, available, total.checked_sub(available)?)?;
            }
        }
        ProviderId::Okex => {
            let rows = array(d)?;
            if rows.len() != 1 {
                return Err(invalid("expected one OKX trading account"));
            }
            for row in array(&rows[0]["details"])? {
                for key in ["liab", "crossLiab", "isoLiab"] {
                    if optional(row, key)?.is_some_and(|n| n != Value::ZERO) {
                        return Err(Error::Unsupported(
                            "margin liabilities cannot be represented as cash balances".into(),
                        ));
                    }
                }
                insert_balance(
                    &mut out,
                    text(row, "ccy")?,
                    decimal(row, "availBal")?,
                    decimal(row, "frozenBal")?,
                )?;
            }
        }
        ProviderId::Bybit => {
            let accounts = array(&d["list"])?;
            if accounts.len() != 1 || text(&accounts[0], "accountType")? != "UNIFIED" {
                return Err(invalid("unexpected Bybit account"));
            }
            for row in array(&accounts[0]["coin"])? {
                for key in ["borrowAmount", "spotBorrow", "totalPositionIM"] {
                    if optional(row, key)?.is_some_and(|n| n != Value::ZERO) {
                        return Err(Error::Unsupported(
                            "Bybit margin exposure cannot be represented as cash balances".into(),
                        ));
                    }
                }
                // Unified walletBalance/availableToWithdraw are NOT free spot cash.
                // Do not invent spendable balances when the API omits the free field.
                let free = optional(row, "free")?.ok_or_else(|| {
                    Error::Unsupported(
                        "Bybit UNIFIED response does not expose unambiguous spot free balance"
                            .into(),
                    )
                })?;
                insert_balance(&mut out, text(row, "coin")?, free, decimal(row, "locked")?)?;
            }
        }
        _ => {
            let rows = if p == ProviderId::Binance {
                &d["balances"]
            } else {
                d
            };
            records(rows)?;
            for row in array(rows)? {
                let (ccy, available, locked) = match p {
                    ProviderId::Binance => (
                        text(row, "asset")?,
                        decimal(row, "free")?,
                        decimal(row, "locked")?,
                    ),
                    ProviderId::Bitget => (
                        text(row, "coin")?,
                        decimal(row, "available")?,
                        decimal(row, "frozen")?.checked_add(decimal(row, "locked")?)?,
                    ),
                    ProviderId::Coinbase => (
                        text(row, "currency")?,
                        decimal(row, "available")?,
                        decimal(row, "hold")?,
                    ),
                    ProviderId::Kucoin => {
                        if text(row, "type")? != "trade" {
                            return Err(invalid("non-trade KuCoin wallet in response"));
                        }
                        (
                            text(row, "currency")?,
                            decimal(row, "available")?,
                            decimal(row, "holds")?,
                        )
                    }
                    ProviderId::Max => (
                        text(row, "currency")?,
                        decimal(row, "balance")?,
                        decimal(row, "locked")?,
                    ),
                    _ => unreachable!(),
                };
                insert_balance(&mut out, ccy, available, locked)?;
            }
        }
    }
    Ok(out)
}
fn status(raw: &str) -> Result<OrderStatus> {
    Ok(match raw {
        "NEW" | "New" | "new" | "open" | "live" | "wait" | "ACTIVE" => OrderStatus::New,
        "PARTIALLY_FILLED" | "PartiallyFilled" | "partially_filled" | "partial_fill" => {
            OrderStatus::PartiallyFilled
        }
        "FILLED" | "Filled" | "filled" | "FilledOrder" | "done" => OrderStatus::Filled,
        "CANCELED" | "CANCELLED" | "Cancelled" | "Canceled" | "cancelled" | "canceled"
        | "cancel" | "EXPIRED" | "Expired" | "EXPIRED_IN_MATCH" | "DEACTIVATED" => {
            OrderStatus::Canceled
        }
        "REJECTED" | "Rejected" | "rejected" => OrderStatus::Rejected,
        "pending" | "Pending" | "NewOrder" => OrderStatus::Pending,
        _ => return Err(Error::Unsupported("unknown private order status".into())),
    })
}
fn validate_order(o: Order) -> Result<Order> {
    symbol(&o.request.symbol)?;
    if o.request.price <= Value::ZERO
        || o.request.quantity <= Value::ZERO
        || o.executed_quantity < Value::ZERO
        || o.executed_quantity > o.request.quantity
    {
        return Err(invalid("invalid private order amounts"));
    }
    if (o.status == OrderStatus::Filled && o.executed_quantity != o.request.quantity)
        || (o.status == OrderStatus::New && o.executed_quantity != Value::ZERO)
        || (o.status == OrderStatus::PartiallyFilled
            && (o.executed_quantity == Value::ZERO || o.executed_quantity == o.request.quantity))
    {
        return Err(invalid("inconsistent private order state"));
    }
    Ok(o)
}
fn order(p: ProviderId, v: &Json) -> Result<Order> {
    if p == ProviderId::Bitfinex {
        return bitfinex_order(v);
    }
    let (id_key, cid, sym, side_key, type_key, qty, exec, price, state) = match p {
        ProviderId::Binance => (
            "orderId",
            "clientOrderId",
            "symbol",
            "side",
            "type",
            "origQty",
            "executedQty",
            "price",
            "status",
        ),
        ProviderId::Backpack => (
            "id",
            "clientId",
            "symbol",
            "side",
            "orderType",
            "quantity",
            "executedQuantity",
            "price",
            "status",
        ),
        ProviderId::Bitget => (
            "orderId",
            "clientOid",
            "symbol",
            "side",
            "orderType",
            "size",
            "baseVolume",
            "priceAvg",
            "status",
        ),
        ProviderId::Bybit => (
            "orderId",
            "orderLinkId",
            "symbol",
            "side",
            "orderType",
            "qty",
            "cumExecQty",
            "price",
            "orderStatus",
        ),
        ProviderId::Coinbase => (
            "id",
            "client_oid",
            "product_id",
            "side",
            "type",
            "size",
            "filled_size",
            "price",
            "status",
        ),
        ProviderId::Kucoin => (
            "id",
            "clientOid",
            "symbol",
            "side",
            "type",
            "size",
            "dealSize",
            "price",
            "",
        ),
        ProviderId::Max => (
            "id",
            "client_oid",
            "market",
            "side",
            "ord_type",
            "volume",
            "executed_volume",
            "price",
            "state",
        ),
        ProviderId::Okex => (
            "ordId",
            "clOrdId",
            "instId",
            "side",
            "ordType",
            "sz",
            "accFillSz",
            "px",
            "state",
        ),
        _ => unreachable!(),
    };
    let ty = text(v, type_key)?;
    if !matches!(ty, "LIMIT" | "Limit" | "limit") {
        return Err(Error::Unsupported(
            "non-GTC/limit or conditional order in private snapshot".into(),
        ));
    }
    if let Some(tif) = v
        .get("timeInForce")
        .or_else(|| v.get("time_in_force"))
        .and_then(Json::as_str)
        && tif != "GTC"
    {
        return Err(Error::Unsupported(
            "non-GTC order in private snapshot".into(),
        ));
    }
    if p == ProviderId::Okex && (text(v, "instType")? != "SPOT" || text(v, "tdMode")? != "cash") {
        return Err(Error::Unsupported("non-cash OKX order".into()));
    }
    if p == ProviderId::Bybit
        && v.get("isLeverage")
            .and_then(Json::as_str)
            .is_some_and(|s| s != "0")
    {
        return Err(Error::Unsupported("leveraged Bybit order".into()));
    }
    let quantity = decimal(v, qty)?;
    let executed = decimal(v, exec)?;
    let mut st = if p == ProviderId::Kucoin {
        if v.get("isActive").and_then(Json::as_bool) == Some(true) {
            if executed > Value::ZERO {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }
        } else if v.get("cancelExist").and_then(Json::as_bool) == Some(true) {
            OrderStatus::Canceled
        } else if executed == quantity {
            OrderStatus::Filled
        } else {
            return Err(invalid("ambiguous KuCoin order state"));
        }
    } else if p == ProviderId::Coinbase && text(v, state)? == "done" {
        match text(v, "done_reason")? {
            "filled" => OrderStatus::Filled,
            "canceled" => OrderStatus::Canceled,
            _ => return Err(invalid("unknown Coinbase done reason")),
        }
    } else {
        status(text(v, state)?)?
    };
    // Some venues use an active state for both zero and nonzero partial executions.
    if st == OrderStatus::New
        && executed > Value::ZERO
        && matches!(
            p,
            ProviderId::Max | ProviderId::Coinbase | ProviderId::Backpack
        )
    {
        st = OrderStatus::PartiallyFilled;
    }
    let side = match text(v, side_key)? {
        "BUY" | "Buy" | "buy" | "Bid" => Side::Buy,
        "SELL" | "Sell" | "sell" | "Ask" => Side::Sell,
        _ => return Err(invalid("invalid order side")),
    };
    validate_order(Order {
        id: field_id(v, id_key)?,
        request: SubmitOrder {
            client_order_id: optional_id(v, cid)?,
            symbol: text(v, sym)?.into(),
            side,
            order_type: OrderType::Limit,
            price: decimal(v, price)?,
            quantity,
        },
        status: st,
        executed_quantity: executed,
    })
}
fn bitfinex_order(v: &Json) -> Result<Order> {
    let row = array(v)?;
    if row.len() < 17 || row[8].as_str() != Some("EXCHANGE LIMIT") {
        return Err(Error::Unsupported(
            "non-spot Bitfinex order or invalid row".into(),
        ));
    }
    // Flags must not conceal reduce-only, post-only, OCO, or other unsupported intent.
    if integer(&row[12])? != 0 {
        return Err(Error::Unsupported("flagged Bitfinex order".into()));
    }
    let native = row[3]
        .as_str()
        .and_then(|s| s.strip_prefix('t'))
        .ok_or_else(|| invalid("invalid Bitfinex symbol"))?;
    let original = number(&row[7])?;
    let remaining = number(&row[6])?;
    let side = if original > Value::ZERO {
        Side::Buy
    } else {
        Side::Sell
    };
    let abs = |v: Value| -> Result<Value> {
        Ok(if v < Value::ZERO {
            Value::ZERO.checked_sub(v)?
        } else {
            v
        })
    };
    if (original < Value::ZERO) != (remaining < Value::ZERO) && remaining != Value::ZERO {
        return Err(invalid("Bitfinex amount sign mismatch"));
    }
    let qty = abs(original)?;
    let executed = qty.checked_sub(abs(remaining)?)?;
    let raw = row[13]
        .as_str()
        .ok_or_else(|| invalid("invalid Bitfinex status"))?;
    let state = if raw.starts_with("EXECUTED") {
        OrderStatus::Filled
    } else if raw.contains("CANCELED") {
        OrderStatus::Canceled
    } else if raw.starts_with("PARTIALLY FILLED") {
        OrderStatus::PartiallyFilled
    } else if raw.starts_with("ACTIVE") {
        if executed > Value::ZERO {
            OrderStatus::PartiallyFilled
        } else {
            OrderStatus::New
        }
    } else {
        return Err(Error::Unsupported("unknown Bitfinex order state".into()));
    };
    validate_order(Order {
        id: id(&row[0])?,
        request: SubmitOrder {
            client_order_id: if row[2].is_null() {
                String::new()
            } else {
                id(&row[2])?
            },
            symbol: native.into(),
            side,
            order_type: OrderType::Limit,
            price: number(&row[16])?,
            quantity: qty,
        },
        status: state,
        executed_quantity: executed,
    })
}
fn notification(v: &Json) -> Result<&Json> {
    let row = array(v)?;
    if row.len() < 8 || row[6].as_str() != Some("SUCCESS") {
        return Err(Error::Adapter("Bitfinex rejected notification".into()));
    }
    let d = &row[4];
    if d.get(0).is_some_and(Json::is_array) {
        let rows = array(d)?;
        if rows.len() != 1 {
            return Err(invalid("unexpected Bitfinex notification count"));
        }
        Ok(&rows[0])
    } else {
        Ok(d)
    }
}
pub(super) fn submitted(p: ProviderId, body: &[u8], requested: &SubmitOrder) -> Result<Order> {
    let v = parse_private(body)?;
    let d = data(p, &v)?;
    let result = match p {
        ProviderId::Bitget | ProviderId::Bybit | ProviderId::Kucoin | ProviderId::Okex => {
            let (row, key, cid) = if p == ProviderId::Okex {
                let rows = array(d)?;
                if rows.len() != 1 || text(&rows[0], "sCode")? != "0" {
                    return Err(Error::Adapter("OKX order rejected".into()));
                }
                (&rows[0], "ordId", "clOrdId")
            } else {
                (
                    d,
                    "orderId",
                    if p == ProviderId::Bybit {
                        "orderLinkId"
                    } else {
                        "clientOid"
                    },
                )
            };
            if let Some(value) = row.get(cid)
                && id(value)? != requested.client_order_id
            {
                return Err(invalid("acknowledgment client ID mismatch"));
            }
            // An accepted async request does NOT prove the order is resting or unfilled.
            Order {
                id: field_id(row, key)?,
                request: requested.clone(),
                status: OrderStatus::Pending,
                executed_quantity: Value::ZERO,
            }
        }
        ProviderId::Bitfinex => order(p, notification(d)?)?,
        _ => order(p, d)?,
    };
    let mut result = result;
    if result.request.client_order_id.is_empty() {
        result.request.client_order_id = requested.client_order_id.clone();
    }
    if result.request != *requested {
        return Err(invalid(
            "order acknowledgment does not match submitted intent",
        ));
    }
    Ok(result)
}
pub(super) fn open(p: ProviderId, body: &[u8], requested_symbol: &str) -> Result<Vec<Order>> {
    let v = parse_private(body)?;
    let d = data(p, &v)?;
    let rows = match p {
        ProviderId::Bybit => {
            if text(d, "category")? != "spot" {
                return Err(invalid("non-spot Bybit snapshot"));
            }
            if d.get("nextPageCursor")
                .and_then(Json::as_str)
                .is_some_and(|s| !s.is_empty())
            {
                return Err(Error::Unsupported(
                    "Bybit open orders need pagination".into(),
                ));
            }
            &d["list"]
        }
        ProviderId::Kucoin => {
            if integer(&d["totalPage"])? > 1 {
                return Err(Error::Unsupported(
                    "KuCoin open orders need pagination".into(),
                ));
            }
            if integer(&d["currentPage"])? != 1
                || integer(&d["totalNum"])? != array(&d["items"])?.len() as i64
            {
                return Err(invalid("incomplete KuCoin order snapshot"));
            }
            &d["items"]
        }
        _ => d,
    };
    records(rows)?;
    let rows = array(rows)?;
    let limit = match p {
        ProviderId::Bitget | ProviderId::Coinbase | ProviderId::Okex => 100,
        ProviderId::Bybit => 50,
        ProviderId::Max => 1000,
        _ => 100_000,
    };
    if rows.len() >= limit {
        return Err(Error::Unsupported(
            "order snapshot may be truncated; pagination required".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    rows.iter()
        .map(|row| {
            let o = order(p, row)?;
            if o.request.symbol != requested_symbol || !ids.insert(o.id.clone()) {
                return Err(invalid("unexpected/duplicate order in symbol snapshot"));
            }
            if !matches!(
                o.status,
                OrderStatus::New | OrderStatus::PartiallyFilled | OrderStatus::Pending
            ) {
                return Err(invalid("terminal order in open-order snapshot"));
            }
            Ok(o)
        })
        .collect()
}
pub(super) fn cancelled(p: ProviderId, body: &[u8], requested: &Order) -> Result<()> {
    let v = parse_private(body)?;
    let d = data(p, &v)?;
    let actual = match p {
        ProviderId::Coinbase => id(d)?,
        ProviderId::Kucoin => {
            let ids = array(&d["cancelledOrderIds"])?;
            if ids.len() != 1 {
                return Err(invalid("unexpected KuCoin cancellation count"));
            }
            id(&ids[0])?
        }
        ProviderId::Okex => {
            let rows = array(d)?;
            if rows.len() != 1 || text(&rows[0], "sCode")? != "0" {
                return Err(Error::Adapter("OKX cancellation rejected".into()));
            }
            field_id(&rows[0], "ordId")?
        }
        ProviderId::Bybit | ProviderId::Bitget => field_id(d, "orderId")?,
        ProviderId::Bitfinex => order(p, notification(d)?)?.id,
        _ => order(p, d)?.id,
    };
    if actual != requested.id {
        return Err(invalid("cancellation acknowledgment ID mismatch"));
    }
    Ok(())
}
pub(super) fn lookup(
    p: ProviderId,
    body: &[u8],
    symbol: &str,
    identity: &str,
    client: bool,
) -> Result<Order> {
    let v = parse_private(body)?;
    let d = data(p, &v)?;
    let row = match p {
        ProviderId::Bybit => {
            if text(d, "category")? != "spot" {
                return Err(invalid("non-spot lookup response"));
            }
            let rows = array(&d["list"])?;
            if rows.len() != 1 {
                return Err(invalid(
                    "order lookup not found or ambiguous; reconcile venue history",
                ));
            }
            &rows[0]
        }
        ProviderId::Okex | ProviderId::Bitget | ProviderId::Bitfinex => {
            let rows = array(d)?;
            if rows.len() != 1 {
                return Err(invalid("order lookup not found or ambiguous"));
            }
            &rows[0]
        }
        _ => d,
    };
    let o = order(p, row)?;
    if o.request.symbol != symbol
        || (if client {
            &o.request.client_order_id
        } else {
            &o.id
        }) != identity
    {
        return Err(invalid("lookup identity mismatch"));
    }
    Ok(o)
}
pub(super) fn empty_orders(body: &[u8]) -> Result<bool> {
    Ok(array(&parse_private(body)?)?.is_empty())
}
