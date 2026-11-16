#![allow(dead_code)]
use bbgo_types::{accounting::Ledger, *};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
pub fn v(value: &str) -> Value {
    value.parse().unwrap()
}
pub fn markets() -> BTreeMap<String, Market> {
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
pub fn seed() -> Ledger {
    Ledger::new(
        BTreeMap::from([(
            "JPY".into(),
            Balance {
                available: v("1000"),
                locked: Value::ZERO,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap()
}
pub fn order(id: &str) -> SubmitOrder {
    SubmitOrder {
        client_order_id: id.into(),
        symbol: "BTCJPY".into(),
        side: Side::Buy,
        order_type: OrderType::Limit,
        price: v("100"),
        quantity: v("1"),
    }
}
pub struct Database(pub PathBuf);
impl Database {
    pub fn fixture() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/tmp/opencode/bbgo-capture-{}-{}.sqlite3",
            std::process::id(),
            ID.fetch_add(1, Ordering::SeqCst)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, include_bytes!("../fixtures/capture-v1.sqlite3"))
            .unwrap();
        Self(path)
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
pub fn append_book(connection: &rusqlite::Connection, at: i64) {
    connection.execute("INSERT INTO capture_events(symbol,stream,writer_run_id,source_id,received_at_ns,processing_at_ns,ingested_at_ns,gap_before_ms,gap_clock,source_schema_version) VALUES('BTCJPY','bookticker','run-2','run-2:bookticker',?1,?1,?1,0,'local_receive','capture-csv-v1')", [at]).unwrap();
    let seq = connection.last_insert_rowid();
    connection.execute("INSERT INTO capture_event_availability VALUES(?1,'capture_service',?2,'collector_callback_receive','observed')", rusqlite::params![seq, at]).unwrap();
    connection
        .execute(
            "INSERT INTO capture_bookticker VALUES(?1,'102','2','104','3')",
            [seq],
        )
        .unwrap();
}
