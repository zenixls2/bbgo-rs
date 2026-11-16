use bbgo_types::*;
fn v(s: &str) -> Value {
    s.parse().unwrap()
}
fn market() -> Market {
    Market {
        symbol: "BTCJPY".into(),
        base_currency: "BTC".into(),
        quote_currency: "JPY".into(),
        spot_enabled: true,
        trading: true,
        order_types: vec![OrderType::Limit],
        tick_size: v("1"),
        step_size: v("0.001"),
        min_quantity: v("0.001"),
        max_quantity: v("10"),
        min_notional: v("100"),
    }
}
#[test]
fn normalize_filters_without_increasing_order_exposure() {
    let market = market();
    let mut order = SubmitOrder {
        client_order_id: "test".into(),
        symbol: "BTCJPY".into(),
        side: Side::Buy,
        order_type: OrderType::Limit,
        price: v("1000.99"),
        quantity: v("0.123999"),
    };
    let normalized = market.normalize_limit(&order).unwrap();
    assert_eq!(normalized.price, v("1000"));
    assert_eq!(normalized.quantity, v("0.123"));
    order.quantity = v("0.099");
    assert!(market.normalize_limit(&order).is_err());
    order.quantity = v("11");
    assert!(market.normalize_limit(&order).is_err());
    order.quantity = v("-1");
    assert!(market.normalize_limit(&order).is_err());
    order.quantity = v("1");
    order.symbol = "ETHJPY".into();
    assert!(market.normalize_limit(&order).is_err());
}
#[test]
fn midpoint_rejects_invalid_quotes_and_avoids_sum_overflow() {
    let mut quote = Ticker {
        symbol: "BTCJPY".into(),
        bid: Value::from_raw(i64::MAX - 2),
        ask: Value::from_raw(i64::MAX),
        exchange_time: 0,
        receive_time: 0,
        generation: 1,
    };
    assert_eq!(quote.midpoint().unwrap(), Value::from_raw(i64::MAX - 1));
    quote.ask = Value::ONE;
    assert!(quote.midpoint().is_err());
    quote.bid = Value::ZERO;
    assert!(quote.midpoint().is_err());
}
