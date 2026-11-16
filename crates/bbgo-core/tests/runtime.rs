use async_trait::async_trait;
use bbgo_core::{
    checkpoint::FileStore,
    session::{self, SessionEvent},
    *,
};
use bbgo_types::*;
use futures::executor::block_on;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio_util::sync::CancellationToken;

#[test]
fn files_survive_reopen_reject_corruption_and_keys_cannot_escape() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::path::Path::new("/tmp/opencode").join(format!(
        "bbgo-checkpoint-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    block_on(async {
        let store = FileStore::new(&dir).unwrap();
        assert!(store.load("missing").await.unwrap().is_none());
        store.save("../state/grid", b"first").await.unwrap();
        store.save("../state/grid", b"second").await.unwrap();
        drop(store);
        let store = FileStore::new(&dir).unwrap();
        assert_eq!(
            store.load("../state/grid").await.unwrap().unwrap(),
            b"second"
        );
        assert!(store.save("", b"x").await.is_err());
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|f| f.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let path = &files[0];
        let bytes = std::fs::read(path).unwrap();
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        envelope["payload"] = serde_json::json!([1, 2, 3]);
        std::fs::write(path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(store.load("../state/grid").await.is_err());
        std::fs::write(path, b"{truncated").unwrap();
        assert!(store.load("../state/grid").await.is_err());
        std::fs::remove_file(path).unwrap();
    });
    std::fs::remove_dir(dir).unwrap();
}
struct Quiet;
#[async_trait]
impl Notifier for Quiet {
    async fn notify(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct TestStrategy {
    calls: Vec<&'static str>,
    fail_shutdown: bool,
}
#[async_trait]
impl Strategy for TestStrategy {
    fn id(&self) -> &'static str {
        "test"
    }
    fn instance_id(&self) -> &str {
        "test"
    }
    fn subscriptions(&self, _: &BTreeMap<String, Market>) -> Result<Vec<String>> {
        Ok(vec![])
    }
    async fn start(&mut self, _: &StrategyContext<'_>) -> Result<()> {
        self.calls.push("start");
        Ok(())
    }
    async fn on_market_event(&mut self, _: &MarketEvent, _: &StrategyContext<'_>) -> Result<()> {
        self.calls.push("market");
        Ok(())
    }
    async fn on_user_event(&mut self, _: &UserEvent, _: &StrategyContext<'_>) -> Result<()> {
        self.calls.push("user");
        Ok(())
    }
    async fn shutdown(&mut self, _: &StrategyContext<'_>) -> Result<()> {
        self.calls.push("shutdown");
        if self.fail_shutdown {
            Err(Error::Adapter("shutdown failed".into()))
        } else {
            Ok(())
        }
    }
    async fn reconnect(&mut self, _: u64, _: &StrategyContext<'_>) -> Result<()> {
        self.calls.push("reconnect");
        Ok(())
    }
}
struct Stream {
    events: VecDeque<SessionEvent>,
    closed: bool,
    fail: bool,
}
#[async_trait]
impl EventStream<SessionEvent> for Stream {
    async fn next_event(&mut self) -> Result<Option<SessionEvent>> {
        if self.fail {
            Err(Error::Adapter("transport failed".into()))
        } else {
            Ok(self.events.pop_front())
        }
    }
    async fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}
fn quote(generation: u64) -> SessionEvent {
    SessionEvent::Market(MarketEvent::Quote(Ticker {
        symbol: "BTCJPY".into(),
        bid: Value::ONE,
        ask: Value::ONE,
        exchange_time: 0,
        receive_time: 0,
        generation,
    }))
}
#[tokio::test]
async fn session_reconnect_failure_cleanup_and_cancellation() {
    let orders = PaperExecutor::new(RunMode::Replay).unwrap();
    let store = MemoryStore::default();
    let context = StrategyContext {
        mode: RunMode::Replay,
        orders: &orders,
        checkpoints: &store,
        notifier: &Quiet,
    };
    let mut strategy = TestStrategy::default();
    let mut stream = Stream {
        events: VecDeque::from([
            quote(0),
            SessionEvent::Disconnected,
            SessionEvent::Reconnected { generation: 1 },
            quote(1),
        ]),
        closed: false,
        fail: false,
    };
    session::run(
        &mut strategy,
        &mut stream,
        &context,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        strategy.calls,
        ["start", "market", "reconnect", "market", "shutdown"]
    );
    assert!(stream.closed);
    let mut strategy = TestStrategy {
        fail_shutdown: true,
        ..Default::default()
    };
    let mut stream = Stream {
        events: VecDeque::new(),
        closed: false,
        fail: true,
    };
    let error = session::run(
        &mut strategy,
        &mut stream,
        &context,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("transport failed") && error.contains("shutdown failed"));
    assert_eq!(strategy.calls, ["start", "shutdown"]);
    assert!(stream.closed);
    let token = CancellationToken::new();
    token.cancel();
    let mut strategy = TestStrategy::default();
    let mut stream = Stream {
        events: VecDeque::from([quote(0)]),
        closed: false,
        fail: false,
    };
    session::run(&mut strategy, &mut stream, &context, &token)
        .await
        .unwrap();
    assert_eq!(strategy.calls, ["start", "shutdown"]);
    assert!(stream.closed);
}
#[tokio::test]
async fn session_refuses_disconnected_or_wrong_generation_data() {
    let orders = PaperExecutor::new(RunMode::Replay).unwrap();
    let store = MemoryStore::default();
    let context = StrategyContext {
        mode: RunMode::Replay,
        orders: &orders,
        checkpoints: &store,
        notifier: &Quiet,
    };
    for events in [
        VecDeque::from([SessionEvent::Disconnected, quote(0)]),
        VecDeque::from([quote(1)]),
    ] {
        let mut strategy = TestStrategy::default();
        let mut stream = Stream {
            events,
            closed: false,
            fail: false,
        };
        assert!(
            session::run(
                &mut strategy,
                &mut stream,
                &context,
                &CancellationToken::new()
            )
            .await
            .is_err()
        );
        assert_eq!(strategy.calls, ["start", "shutdown"]);
        assert!(stream.closed);
    }
}
