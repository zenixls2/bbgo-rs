mod common;
use bbgo_backtest::{
    source::{BacktestSource, Input, Tick},
    sqlite::{SqlTickSource, Window},
};
use common::*;
fn window() -> Window {
    Window {
        from_ns: 0,
        through_ns: 10_000_000_000,
        gap_threshold_ms: 5000,
    }
}

#[tokio::test]
async fn collector_schema_ordering_clocks_generations_and_windows() {
    let db = Database::fixture();
    let mut source = SqlTickSource::open(&db.0, "BTCJPY", window()).unwrap();
    let mut frames = Vec::new();
    while let Some(frame) = source.next_frame().await.unwrap() {
        frames.push(frame);
    }
    assert_eq!(
        frames
            .iter()
            .map(|f| f.position.sequence)
            .collect::<Vec<_>>(),
        [2, 1, 3, 4, 5, 6, 7]
    );
    let generations: Vec<_> = frames
        .iter()
        .map(|frame| {
            let Input::Tick(tick) = &frame.input else {
                panic!()
            };
            tick.metadata().generation
        })
        .collect();
    assert_eq!(generations, [0, 0, 0, 0, 1, 1, 2]);
    let Input::Tick(Tick::Book { quote, metadata }) = &frames[0].input else {
        panic!()
    };
    assert_eq!(quote.exchange_time, None);
    assert_eq!(metadata.exchange_time_ns, None);
    assert_eq!(metadata.received_at_ns, 1_000_000_100);
    assert_eq!(
        frames[2].position.available_at_ns - frames[1].position.available_at_ns,
        1
    );
    let Input::Tick(Tick::Trade { print, metadata }) = &frames[1].input else {
        panic!()
    };
    assert_eq!(print.id, "aggregate:10");
    assert_eq!(metadata.exchange_time_ns, Some(1_000_000_000));
    assert_eq!(print.exchange_time, 1000);
    source.close().await.unwrap();
    source.close().await.unwrap();
    assert!(source.next_frame().await.is_err());
    let mut source = SqlTickSource::open(
        &db.0,
        "BTCJPY",
        Window {
            from_ns: 1_000_000_101,
            through_ns: 1_000_000_101,
            ..window()
        },
    )
    .unwrap();
    assert_eq!(
        source
            .next_frame()
            .await
            .unwrap()
            .unwrap()
            .position
            .sequence,
        3
    );
    assert!(source.next_frame().await.unwrap().is_none());
}
#[test]
fn schema_and_unclassifiable_or_untrusted_records_fail_closed() {
    for mutation in [
        "PRAGMA user_version=2",
        "UPDATE schema_migrations SET migration_sha256='0000000000000000000000000000000000000000000000000000000000000000'",
        "CREATE TABLE unexpected(x)",
        "DELETE FROM capture_event_availability WHERE ingest_seq=1",
        "UPDATE capture_event_availability SET availability_basis='exchange_event'",
        "UPDATE capture_event_availability SET available_at_ns=1 WHERE ingest_seq=1",
        "UPDATE capture_writer_runs SET status='failed'",
        "UPDATE capture_sources SET completeness_status='quarantined'",
    ] {
        let db = Database::fixture();
        let connection = rusqlite::Connection::open(&db.0).unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        assert!(
            SqlTickSource::open(&db.0, "BTCJPY", window()).is_err(),
            "{mutation}"
        );
    }
    let db = Database::fixture();
    assert!(SqlTickSource::open(&db.0, "ETHJPY", window()).is_err());
    assert!(
        SqlTickSource::open(
            &db.0,
            "BTCJPY",
            Window {
                gap_threshold_ms: 0,
                ..window()
            }
        )
        .is_err()
    );
    assert!(
        SqlTickSource::open(
            "/tmp/opencode/does-not-exist-capture.sqlite3",
            "BTCJPY",
            window()
        )
        .is_err()
    );
}
#[tokio::test]
async fn paged_reader_and_snapshot_isolation() {
    let db = Database::fixture();
    let connection = rusqlite::Connection::open(&db.0).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    connection.execute_batch("BEGIN").unwrap();
    for offset in 0..600 {
        append_book(&connection, 8_000_000_000 + offset);
    }
    connection.execute_batch("COMMIT").unwrap();
    let mut source = SqlTickSource::open(&db.0, "BTCJPY", window()).unwrap();
    append_book(&connection, 8_000_001_000);
    let mut count = 0;
    while source.next_frame().await.unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 607);
    source.close().await.unwrap();
    let mut fresh = SqlTickSource::open(&db.0, "BTCJPY", window()).unwrap();
    let mut count = 0;
    while fresh.next_frame().await.unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 608);
}
#[tokio::test]
async fn malformed_payload_is_not_rounded_or_silently_skipped() {
    let db = Database::fixture();
    let connection = rusqlite::Connection::open(&db.0).unwrap();
    connection.execute("INSERT INTO capture_events(symbol,stream,writer_run_id,source_id,received_at_ns,processing_at_ns,ingested_at_ns,source_schema_version) VALUES('BTCJPY','bookticker','run-2','run-2:bookticker',8000000000,8000000000,8000000000,'capture-csv-v1')", []).unwrap();
    let seq = connection.last_insert_rowid();
    connection.execute("INSERT INTO capture_event_availability VALUES(?1,'capture_service',8000000000,'collector_callback_receive','observed')", [seq]).unwrap();
    connection
        .execute(
            "INSERT INTO capture_bookticker VALUES(?1,'99.000000001','1','101','1')",
            [seq],
        )
        .unwrap();
    let mut source = SqlTickSource::open(&db.0, "BTCJPY", window()).unwrap();
    assert!(source.next_frame().await.is_err());
}
