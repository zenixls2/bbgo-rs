mod common;
use async_trait::async_trait;
use bbgo_backtest::{
    Backtest, Model,
    source::*,
    sqlite::{SqlTickSource, Window},
};
use bbgo_core::{MemoryStore, Notifier, Strategy, StrategyContext};
use bbgo_types::*;
use common::*;
use tokio_util::sync::CancellationToken;

struct Quiet;
#[async_trait]
impl Notifier for Quiet {
    async fn notify(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct StrategyFixture {
    trace: Vec<String>,
    submitted: bool,
    candle_orders: bool,
    stopped: bool,
    fail: bool,
}
#[async_trait]
impl Strategy for StrategyFixture {
    fn id(&self) -> &'static str {
        "backtest-fixture"
    }
    fn instance_id(&self) -> &str {
        "backtest-fixture"
    }
    fn subscriptions(
        &self,
        markets: &std::collections::BTreeMap<String, Market>,
    ) -> Result<Vec<String>> {
        Ok(markets.keys().cloned().collect())
    }
    async fn start(&mut self, context: &StrategyContext<'_>) -> Result<()> {
        self.trace.push("start".into());
        if self.candle_orders {
            context.orders.submit(order("initial")).await?;
        }
        Ok(())
    }
    async fn on_market_event(
        &mut self,
        event: &MarketEvent,
        context: &StrategyContext<'_>,
    ) -> Result<()> {
        self.trace.push(format!(
            "market:{}",
            match event {
                MarketEvent::BookTicker(_) => "book",
                MarketEvent::PublicTrade(_) => "trade",
                _ => "candle",
            }
        ));
        if self.fail {
            return Err(Error::Invalid("fixture handler failed".into()));
        }
        if !self.submitted {
            context.orders.submit(order("reaction")).await?;
            self.submitted = true;
        }
        Ok(())
    }
    async fn on_user_event(&mut self, event: &UserEvent, _: &StrategyContext<'_>) -> Result<()> {
        self.trace.push(format!(
            "user:{}",
            match event {
                UserEvent::Trade(_) => "trade",
                UserEvent::Order(_) => "order",
                _ => "balances",
            }
        ));
        Ok(())
    }
    async fn reconnect(&mut self, generation: u64, _: &StrategyContext<'_>) -> Result<()> {
        self.trace.push(format!("reconnect:{generation}"));
        Ok(())
    }
    async fn shutdown(&mut self, _: &StrategyContext<'_>) -> Result<()> {
        self.stopped = true;
        Ok(())
    }
}
fn candle(start: i64, end: i64, up: bool) -> KLine {
    KLine {
        symbol: "BTCJPY".into(),
        start_time: start,
        end_time: end,
        open: v(if up { "100" } else { "105" }),
        high: v("110"),
        low: v("90"),
        close: v(if up { "105" } else { "95" }),
        volume: v("0.1"),
        closed: true,
    }
}
#[tokio::test]
async fn single_runner_event_mode_no_same_timestamp_fill_and_volume_shared() {
    let db = Database::fixture();
    let mut source = SqlTickSource::open(
        &db.0,
        "BTCJPY",
        Window {
            from_ns: 0,
            through_ns: 1_000_000_102,
            gap_threshold_ms: 5000,
        },
    )
    .unwrap();
    let mut backtest = Backtest::new(Model::Event, markets(), seed(), v("0.01")).unwrap();
    let mut strategy = StrategyFixture::default();
    let report = backtest
        .run(
            &mut strategy,
            &mut source,
            &MemoryStore::default(),
            &Quiet,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.frames, 4);
    // Quote reaction cannot fill on aggregate:10 (same ns), but can fill one ns
    // later despite all three prints projecting to the same millisecond.
    assert_eq!(report.orders[0].executed_quantity, v("0.8"));
    assert_eq!(report.ledger.balances()["JPY"].locked, v("20.2"));
    assert_eq!(report.ledger.positions()["BTCJPY"].cost_basis, v("80.8"));
    assert_eq!(
        &strategy.trace[..3],
        ["start", "market:book", "market:trade"]
    );
    assert_eq!(strategy.trace[3], "user:trade");
    assert!(strategy.stopped);
    assert!(source.next_frame().await.is_err());
    assert!(
        backtest
            .run(
                &mut strategy,
                &mut source,
                &MemoryStore::default(),
                &Quiet,
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn same_interface_candle_mode_no_future_bar_lookahead() {
    let mut source = KlineSource::new(
        "candles".into(),
        vec![candle(0, 999, true), candle(1000, 1999, false)].into_iter(),
    )
    .unwrap();
    let mut backtest = Backtest::new(Model::Kline, markets(), seed(), v("0.01")).unwrap();
    let mut strategy = StrategyFixture {
        candle_orders: true,
        ..Default::default()
    };
    let report = backtest
        .run(
            &mut strategy,
            &mut source,
            &MemoryStore::default(),
            &Quiet,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.frames, 2);
    assert_eq!(report.orders.len(), 2);
    assert!(
        report
            .orders
            .iter()
            .all(|o| o.status == OrderStatus::Filled)
    );
    assert_eq!(report.ledger.positions()["BTCJPY"].quantity, v("2"));
    assert_eq!(report.ledger.positions()["BTCJPY"].cost_basis, v("202"));
    // Initial order fills before first candle callback; its reaction only fills
    // in the second bar, never using the first bar's already-known low.
    assert_eq!(
        &strategy.trace[..5],
        [
            "start",
            "user:trade",
            "user:order",
            "user:balances",
            "market:candle"
        ]
    );
}
#[tokio::test]
async fn merged_sources_reconnect_cleanup_and_cancellation() {
    let mut a = candle(1000, 1999, true);
    a.symbol = "ETHJPY".into();
    let mut merged = MergedSource::new(vec![
        Box::new(KlineSource::new("later".into(), vec![a].into_iter()).unwrap()),
        Box::new(
            KlineSource::new("first".into(), vec![candle(0, 999, false)].into_iter()).unwrap(),
        ),
    ])
    .unwrap();
    assert_eq!(
        merged.next_frame().await.unwrap().unwrap().position.source,
        "first"
    );
    assert_eq!(
        merged.next_frame().await.unwrap().unwrap().position.source,
        "later"
    );
    assert!(merged.next_frame().await.unwrap().is_none());
    merged.close().await.unwrap();
    let db = Database::fixture();
    let mut source = SqlTickSource::open(
        &db.0,
        "BTCJPY",
        Window {
            from_ns: 0,
            through_ns: 9_000_000_000,
            gap_threshold_ms: 5000,
        },
    )
    .unwrap();
    let mut backtest = Backtest::new(Model::Event, markets(), seed(), Value::ZERO).unwrap();
    let mut strategy = StrategyFixture::default();
    backtest
        .run(
            &mut strategy,
            &mut source,
            &MemoryStore::default(),
            &Quiet,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(strategy.trace.contains(&"reconnect:1".into()));
    assert!(strategy.trace.contains(&"reconnect:2".into()));
    let mut source =
        KlineSource::new("fail".into(), vec![candle(0, 999, true)].into_iter()).unwrap();
    let mut backtest = Backtest::new(Model::Kline, markets(), seed(), Value::ZERO).unwrap();
    let mut strategy = StrategyFixture {
        fail: true,
        ..Default::default()
    };
    assert!(
        backtest
            .run(
                &mut strategy,
                &mut source,
                &MemoryStore::default(),
                &Quiet,
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(strategy.stopped);
    assert!(source.next_frame().await.is_err());
    let mut source =
        KlineSource::new("cancel".into(), vec![candle(0, 999, true)].into_iter()).unwrap();
    let mut backtest = Backtest::new(Model::Kline, markets(), seed(), Value::ZERO).unwrap();
    let mut strategy = StrategyFixture::default();
    let token = CancellationToken::new();
    token.cancel();
    let report = backtest
        .run(
            &mut strategy,
            &mut source,
            &MemoryStore::default(),
            &Quiet,
            &token,
        )
        .await
        .unwrap();
    assert_eq!(report.frames, 0);
    assert!(strategy.stopped);
}
