use bbgo_core::OrderExecutor;
use bbgo_replay::{ReplayExecutor, TradePrint};
use bbgo_types::{accounting::Ledger, *};
use futures::executor::block_on;
use std::collections::BTreeMap;
fn v(s: &str) -> Value {
    s.parse().unwrap()
}
#[test]
fn constrained_batches_recover_eligibility_and_rollback_atomically() {
    let executor = setup("1000");
    executor
        .submit_limit(order("first", Side::Buy, "100", "1"))
        .unwrap();
    executor
        .submit_limit(order("second", Side::Buy, "100", "1"))
        .unwrap();
    let eligible = std::collections::BTreeSet::from(["second".into()]);
    let before = executor.checkpoint().unwrap();
    assert!(
        executor
            .on_trades_for_orders(
                vec![
                    print("a", Side::Sell, "99", "0.5", 1),
                    print("b", Side::Sell, "99", "-1", 2),
                ],
                &eligible
            )
            .is_err()
    );
    assert_eq!(before, executor.checkpoint().unwrap());
    executor
        .on_trades_for_orders(vec![print("a", Side::Sell, "99", "0.5", 1)], &eligible)
        .unwrap();
    assert_eq!(executor.orders().unwrap()[0].executed_quantity, Value::ZERO);
    assert_eq!(executor.orders().unwrap()[1].executed_quantity, v("0.5"));
    let checkpoint = executor.checkpoint().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&checkpoint).unwrap();
    assert_eq!(json["version"], 2);
    let restored =
        ReplayExecutor::from_checkpoint(markets(), seed("1000"), v("0.01"), &checkpoint).unwrap();
    assert_eq!(restored.orders().unwrap(), executor.orders().unwrap());
    assert_eq!(restored.ledger().unwrap(), executor.ledger().unwrap());
    assert!(
        restored
            .on_trades_for_orders(
                vec![print("a", Side::Sell, "99", "0.5", 1)],
                &std::collections::BTreeSet::from(["first".into()])
            )
            .unwrap()
            .is_empty()
    );
}
fn setup(cash: &str) -> ReplayExecutor {
    ReplayExecutor::new(markets(), seed(cash), v("0.01")).unwrap()
}
fn markets() -> BTreeMap<String, Market> {
    let market = Market {
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
    };
    BTreeMap::from([(market.symbol.clone(), market)])
}
fn seed(cash: &str) -> Ledger {
    Ledger::new(
        BTreeMap::from([(
            "JPY".into(),
            Balance {
                available: v(cash),
                locked: Value::ZERO,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap()
}
fn order(id: &str, side: Side, price: &str, quantity: &str) -> SubmitOrder {
    SubmitOrder {
        client_order_id: id.into(),
        symbol: "BTCJPY".into(),
        side,
        order_type: OrderType::Limit,
        price: v(price),
        quantity: v(quantity),
    }
}
fn print(id: &str, aggressor: Side, price: &str, quantity: &str, time: i64) -> TradePrint {
    TradePrint {
        id: id.into(),
        symbol: "BTCJPY".into(),
        aggressor,
        price: v(price),
        quantity: v(quantity),
        exchange_time: time,
    }
}
#[test]
fn reservation_partial_fill_cancel_sell_fees_and_pnl() {
    block_on(async {
        let executor = setup("1000");
        let buy = executor
            .submit(order("buy", Side::Buy, "100", "2"))
            .await
            .unwrap();
        assert_eq!(
            executor.ledger().unwrap().balances()["JPY"].locked,
            v("202")
        );
        assert_eq!(executor.submit(buy.request.clone()).await.unwrap(), buy);
        let events = executor
            .on_trade(print("1", Side::Sell, "99", "1", 100))
            .unwrap();
        assert!(
            matches!(&events[0], UserEvent::Trade(t) if t.price == v("100") && t.fee == v("1"))
        );
        assert_eq!(
            executor.orders().unwrap()[0].status,
            OrderStatus::PartiallyFilled
        );
        assert_eq!(
            executor.ledger().unwrap().balances()["JPY"].locked,
            v("101")
        );
        executor.cancel(&[buy]).await.unwrap();
        assert_eq!(
            executor.ledger().unwrap().balances()["JPY"].locked,
            Value::ZERO
        );
        let sell = executor
            .submit(order("sell", Side::Sell, "120", "1"))
            .await
            .unwrap();
        assert_eq!(executor.ledger().unwrap().balances()["BTC"].locked, v("1"));
        assert!(
            executor
                .submit(order("short", Side::Sell, "120", "1"))
                .await
                .is_err()
        );
        executor
            .on_trade(print("2", Side::Buy, "121", "1", 200))
            .unwrap();
        let ledger = executor.ledger().unwrap();
        assert_eq!(ledger.positions()["BTCJPY"].realized_pnl, v("17.8"));
        assert_eq!(ledger.balances()["JPY"].available, v("1017.8"));
        assert_eq!(ledger.balances()["BTC"].total().unwrap(), Value::ZERO);
        executor.cancel(&[sell]).await.unwrap(); // filled cancellation is a no-op
    });
}
#[test]
fn fifo_shared_liquidity_dedup_and_failed_requests_are_atomic() {
    block_on(async {
        let executor = setup("303");
        let first = executor
            .submit(order("z-first", Side::Buy, "100", "2"))
            .await
            .unwrap();
        let second = executor
            .submit(order("a-second", Side::Buy, "100", "1"))
            .await
            .unwrap();
        let before = executor.ledger().unwrap();
        assert!(
            executor
                .submit(order("too-much", Side::Buy, "100", "1"))
                .await
                .is_err()
        );
        assert_eq!(executor.ledger().unwrap(), before);
        let tick = print("1", Side::Sell, "100", "2.5", 100);
        executor.on_trade(tick.clone()).unwrap();
        let orders = executor.orders().unwrap();
        assert_eq!(orders[0].status, OrderStatus::Filled);
        assert_eq!(orders[1].executed_quantity, v("0.5"));
        assert!(executor.on_trade(tick).unwrap().is_empty());
        let before = executor.ledger().unwrap();
        assert!(
            executor
                .on_trade(print("1", Side::Sell, "100", "3", 100))
                .is_err()
        );
        assert!(
            executor
                .on_trade(print("old", Side::Sell, "100", "1", 99))
                .is_err()
        );
        assert_eq!(executor.ledger().unwrap(), before);
        let mut unknown = second.clone();
        unknown.id = "unknown".into();
        assert!(executor.cancel(&[second.clone(), unknown]).await.is_err());
        assert_eq!(executor.ledger().unwrap(), before);
        executor.cancel(&[first, second]).await.unwrap();
        assert_eq!(
            executor.ledger().unwrap().balances()["JPY"].locked,
            Value::ZERO
        );
    });
}
#[test]
fn noncrossing_wrong_aggressor_and_bad_filters_do_not_fill() {
    block_on(async {
        let executor = setup("1000");
        assert!(
            executor
                .submit(order("bad", Side::Buy, "100.5", "1"))
                .await
                .is_err()
        );
        executor
            .submit(order("buy", Side::Buy, "100", "1"))
            .await
            .unwrap();
        assert!(
            executor
                .on_trade(print("1", Side::Buy, "99", "1", 100))
                .unwrap()
                .is_empty()
        );
        assert!(
            executor
                .on_trade(print("2", Side::Sell, "101", "1", 101))
                .unwrap()
                .is_empty()
        );
        assert_eq!(executor.orders().unwrap()[0].executed_quantity, Value::ZERO);
    });
}

fn recovered(bytes: &[u8]) -> Result<ReplayExecutor> {
    ReplayExecutor::from_checkpoint(markets(), seed("1000"), v("0.01"), bytes)
}

#[test]
fn recovery_preserves_fifo_partial_reservations_fees_ids_and_pnl() {
    block_on(async {
        let original = setup("1000");
        original
            .submit(order("z-first", Side::Buy, "100", "2"))
            .await
            .unwrap();
        original
            .submit(order("a-second", Side::Buy, "100", "1"))
            .await
            .unwrap();
        let tick = print("partial", Side::Sell, "99", "0.50000001", 100);
        original.on_trade(tick.clone()).unwrap();
        let bytes = original.checkpoint().unwrap();
        let resumed = recovered(&bytes).unwrap();
        assert_eq!(resumed.checkpoint().unwrap(), bytes);
        assert_eq!(resumed.orders().unwrap(), original.orders().unwrap());
        assert_eq!(resumed.ledger().unwrap(), original.ledger().unwrap());
        assert!(resumed.on_trade(tick).unwrap().is_empty());
        assert!(
            resumed
                .on_trade(print("partial", Side::Sell, "99", "1", 100))
                .is_err()
        );
        assert!(
            resumed
                .on_trade(print("old", Side::Sell, "99", "1", 99))
                .is_err()
        );
        assert_eq!(resumed.checkpoint().unwrap(), bytes);
        for executor in [&original, &resumed] {
            // Duplicate submissions do not reserve again or reorder the queue.
            executor
                .submit(order("z-first", Side::Buy, "100", "2"))
                .await
                .unwrap();
        }
        for tick in [
            print("fill", Side::Sell, "99", "2", 101),
            print("nonfill", Side::Buy, "80", "1", 102),
        ] {
            assert_eq!(
                serde_json::to_value(original.on_trade(tick.clone()).unwrap()).unwrap(),
                serde_json::to_value(resumed.on_trade(tick).unwrap()).unwrap()
            );
        }
        for executor in [&original, &resumed] {
            let second = executor.orders().unwrap()[1].clone();
            executor.cancel(&[second]).await.unwrap();
            executor
                .submit(order("sell", Side::Sell, "120", "2.5"))
                .await
                .unwrap();
        }
        let bytes = resumed.checkpoint().unwrap();
        let again = recovered(&bytes).unwrap();
        let tick = print("exit", Side::Buy, "121", "2.5", 103);
        let expected = serde_json::to_value(original.on_trade(tick.clone()).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(resumed.on_trade(tick.clone()).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(again.on_trade(tick).unwrap()).unwrap(),
            expected
        );
        assert_eq!(again.ledger().unwrap(), original.ledger().unwrap());
        assert_eq!(again.orders().unwrap(), original.orders().unwrap());
        assert!(again.ledger().unwrap().positions()["BTCJPY"].realized_pnl > Value::ZERO);
        assert_eq!(again.checkpoint().unwrap(), original.checkpoint().unwrap());
    });
}

#[test]
fn recovery_rejects_bad_config_account_and_impossible_history() {
    block_on(async {
        let executor = setup("1000");
        let submitted = executor
            .submit(order("one", Side::Buy, "100", "1"))
            .await
            .unwrap();
        executor
            .on_trade(print("fill", Side::Sell, "99", "0.5", 100))
            .unwrap();
        executor.cancel(&[submitted]).await.unwrap();
        let bytes = executor.checkpoint().unwrap();
        assert!(
            ReplayExecutor::from_checkpoint(markets(), seed("999"), v("0.01"), &bytes).is_err()
        );
        assert!(
            ReplayExecutor::from_checkpoint(markets(), seed("1000"), v("0.02"), &bytes).is_err()
        );
        let mut changed = markets();
        changed.get_mut("BTCJPY").unwrap().tick_size = v("2");
        assert!(ReplayExecutor::from_checkpoint(changed, seed("1000"), v("0.01"), &bytes).is_err());
        for (pointer, value) in [
            ("/version", serde_json::json!(2)),
            ("/actions/0/request/quantity", serde_json::json!("100")),
            ("/actions/1/print/quantity", serde_json::json!("-1")),
            ("/actions/2/ids/0", serde_json::json!("unknown")),
        ] {
            let mut corrupt: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            *corrupt.pointer_mut(pointer).unwrap() = value;
            assert!(
                recovered(&serde_json::to_vec(&corrupt).unwrap()).is_err(),
                "{pointer}"
            );
        }
        let mut duplicate: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let first = duplicate["actions"][0].clone();
        duplicate["actions"].as_array_mut().unwrap().push(first);
        assert!(recovered(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        let mut excessive: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        excessive["actions"] = serde_json::json!(vec![
            excessive["actions"][0].clone();
            bbgo_replay::MAX_RECOVERY_ACTIONS + 1
        ]);
        assert!(recovered(&serde_json::to_vec(&excessive).unwrap()).is_err());
        assert!(recovered(b"{").is_err());
        assert!(recovered(&vec![b' '; bbgo_replay::MAX_CHECKPOINT_BYTES + 1]).is_err());
        assert_eq!(executor.checkpoint().unwrap(), bytes);
    });
}

#[test]
fn durable_replay_checkpoint_survives_store_reopen() {
    block_on(async {
        use bbgo_core::{CheckpointStore, checkpoint::FileStore};
        let directory = std::path::PathBuf::from(format!(
            "/tmp/opencode/bbgo-replay-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = FileStore::new(&directory).unwrap();
        assert!(
            ReplayExecutor::load_checkpoint(markets(), seed("1000"), v("0.01"), &store, "replay")
                .await
                .unwrap()
                .is_none()
        );
        let executor = setup("1000");
        executor
            .submit(order("one", Side::Buy, "100", "1"))
            .await
            .unwrap();
        executor
            .on_trade(print("partial", Side::Sell, "100", "0.3", 10))
            .unwrap();
        executor.save_checkpoint(&store, "replay").await.unwrap();
        drop(store);
        let reopened = FileStore::new(&directory).unwrap();
        let restored = ReplayExecutor::load_checkpoint(
            markets(),
            seed("1000"),
            v("0.01"),
            &reopened,
            "replay",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(restored.ledger().unwrap(), executor.ledger().unwrap());
        assert_eq!(restored.orders().unwrap(), executor.orders().unwrap());
        reopened.save("bad", b"invalid").await.unwrap();
        assert!(
            ReplayExecutor::load_checkpoint(markets(), seed("1000"), v("0.01"), &reopened, "bad")
                .await
                .is_err()
        );
        // Remove only this test's individual files, never a broad recursive path.
        for entry in std::fs::read_dir(&directory).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    });
}

#[test]
fn deserialized_invalid_seed_is_not_adopted() {
    let mut invalid = serde_json::to_value(seed("1000")).unwrap();
    invalid["balances"]["JPY"]["available"] = serde_json::json!("-1");
    let ledger: Ledger = serde_json::from_value(invalid).unwrap();
    assert!(ReplayExecutor::new(markets(), ledger, v("0.01")).is_err());
}

#[test]
fn seeded_fill_history_cannot_collide_with_new_generated_ids() {
    block_on(async {
        let previous = setup("1000");
        previous
            .submit(order("buy", Side::Buy, "100", "1"))
            .await
            .unwrap();
        previous
            .on_trade(print("old-fill", Side::Sell, "100", "1", 1))
            .unwrap();
        let seed = previous.ledger().unwrap();
        let executor = ReplayExecutor::new(markets(), seed.clone(), v("0.01")).unwrap();
        executor
            .submit(order("sell", Side::Sell, "120", "1"))
            .await
            .unwrap();
        let bytes = executor.checkpoint().unwrap();
        let restored = ReplayExecutor::from_checkpoint(markets(), seed, v("0.01"), &bytes).unwrap();
        let events = restored
            .on_trade(print("new-fill", Side::Buy, "120", "1", 2))
            .unwrap();
        assert!(matches!(&events[0], UserEvent::Trade(t) if t.id == "replay-trade:2"));
        assert_eq!(
            restored.ledger().unwrap().positions()["BTCJPY"].quantity,
            Value::ZERO
        );
        assert_eq!(
            restored.ledger().unwrap().balances()["JPY"].available,
            v("1017.8")
        );
    });
}
