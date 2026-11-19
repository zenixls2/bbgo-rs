use bbgo_types::{
    accounting::{Ledger, Position, multiply_ceil},
    book::{BookUpdate, Level, OrderBook},
    subscription::Interval,
    *,
};
use std::collections::BTreeMap;
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
        min_notional: v("1"),
    }
}
fn trade(id: &str, side: Side, price: &str, quantity: &str, fee: &str, currency: &str) -> Trade {
    Trade {
        id: id.into(),
        order_id: "order".into(),
        symbol: "BTCJPY".into(),
        side,
        price: v(price),
        quantity: v(quantity),
        fee: v(fee),
        fee_currency: currency.into(),
        exchange_time: 100,
    }
}
#[test]
fn accounting_dedup_fee_basis_pnl_and_atomic_failure() {
    let mut ledger = Ledger::new(
        BTreeMap::from([(
            "JPY".into(),
            Balance {
                available: v("1000"),
                locked: Value::ZERO,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap();
    let buy = trade("1", Side::Buy, "100", "2", "2", "JPY");
    assert!(ledger.apply_trade(&market(), &buy).unwrap());
    assert!(!ledger.apply_trade(&market(), &buy).unwrap());
    let p = &ledger.positions()["BTCJPY"];
    assert_eq!(p.average_cost().unwrap(), v("101"));
    assert_eq!(p.unrealized_pnl(v("110")).unwrap(), v("18"));
    ledger.reserve("BTC", v("1")).unwrap();
    let before = ledger.clone();
    assert!(
        ledger
            .apply_trade(&market(), &trade("2", Side::Sell, "120", "2", "0", "JPY"))
            .is_err()
    );
    assert_eq!(ledger, before);
    ledger.release("BTC", v("1")).unwrap();
    ledger
        .apply_trade(&market(), &trade("2", Side::Sell, "120", "1", "1", "JPY"))
        .unwrap();
    assert_eq!(ledger.positions()["BTCJPY"].realized_pnl, v("18"));
    assert_eq!(ledger.positions()["BTCJPY"].cost_basis, v("101"));
    let encoded = serde_json::to_vec(&ledger).unwrap();
    assert_eq!(serde_json::from_slice::<Ledger>(&encoded).unwrap(), ledger);
    assert!(ledger.reconcile(ledger.balances()).unwrap().is_empty());
    let before = ledger.clone();
    let mut conflict = buy;
    conflict.price = v("101");
    assert!(ledger.apply_trade(&market(), &conflict).is_err());
    assert!(
        ledger
            .apply_trade(&market(), &trade("3", Side::Buy, "1", "1", "0.1", "BNB"))
            .is_err()
    );
    assert_eq!(ledger, before);
}
#[test]
fn base_fees_and_final_basis_dust() {
    let mut ledger = Ledger::new(
        BTreeMap::from([(
            "JPY".into(),
            Balance {
                available: v("1000"),
                locked: Value::ZERO,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap();
    ledger
        .apply_trade(&market(), &trade("1", Side::Buy, "100", "1", "0.01", "BTC"))
        .unwrap();
    assert_eq!(ledger.positions()["BTCJPY"].quantity, v("0.99"));
    ledger
        .apply_trade(
            &market(),
            &trade("2", Side::Sell, "120", "0.98", "0.01", "BTC"),
        )
        .unwrap();
    assert_eq!(
        ledger.positions()["BTCJPY"],
        Position {
            quantity: Value::ZERO,
            cost_basis: Value::ZERO,
            realized_pnl: v("17.6")
        }
    );
    assert_eq!(
        multiply_ceil(v("0.00000001"), v("0.001")).unwrap(),
        v(if Value::BACKEND == "dnum-16" {
            "0.00000000001"
        } else {
            "0.00000001"
        })
    );
}
#[test]
fn book_duplicates_overlap_gap_invalid_levels_and_resnapshot() {
    let level = |price, quantity| Level {
        price: v(price),
        quantity: v(quantity),
    };
    let mut book = OrderBook::new("BTCJPY");
    book.snapshot("BTCJPY", 10, &[level("100", "1")], &[level("102", "2")])
        .unwrap();
    let mut update = BookUpdate {
        symbol: "BTCJPY".into(),
        first_sequence: 10,
        last_sequence: 12,
        bids: vec![level("100", "0"), level("101", "3")],
        asks: vec![],
    };
    assert!(book.update(&update).unwrap());
    assert_eq!(book.best_bid(), Some(level("101", "3")));
    assert!(!book.update(&update).unwrap());
    update.first_sequence = 14;
    update.last_sequence = 14;
    assert!(book.update(&update).is_err());
    assert_eq!(book.best_bid(), None);
    assert!(book.update(&update).is_err());
    book.snapshot("BTCJPY", 14, &[level("100", "1")], &[level("102", "1")])
        .unwrap();
    update.first_sequence = 15;
    update.last_sequence = 15;
    update.bids = vec![level("103", "1")];
    assert!(book.update(&update).is_err());
    assert_eq!(book.sequence(), None);
}
#[test]
fn interval_negative_times_weeks_and_overflow() {
    assert_eq!(Interval::Minute.floor(-1).unwrap(), -60_000);
    assert_eq!(Interval::Week.floor(0).unwrap(), -259_200_000);
    assert_eq!(Interval::Hour.floor(3_600_001).unwrap(), 3_600_000);
    assert!(Interval::Day.floor(i64::MIN).is_err());
    assert!("1mo".parse::<Interval>().is_err());
    for interval in [Interval::Second, Interval::Minute, Interval::Week] {
        assert_eq!(interval.to_string().parse::<Interval>().unwrap(), interval);
    }
}
