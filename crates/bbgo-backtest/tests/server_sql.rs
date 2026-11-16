//! Explicit opt-in integration tests. These CREATE tables in a disposable EMPTY
//! database named bbgo_backtest_test. Never point them at a collector database.
mod common;
use bbgo_backtest::{
    source::{BacktestSource, Frame},
    sql::{CollectorDatabase, open_collector},
    sqlite::{SqlTickSource, Window},
};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::sync::Once;
fn placeholders(template: &str, postgres: bool) -> String {
    let mut n = 0;
    template
        .chars()
        .map(|c| {
            if c == '?' && postgres {
                n += 1;
                format!("${n}")
            } else {
                c.to_string()
            }
        })
        .collect()
}
async fn frames(source: &mut dyn BacktestSource) -> Vec<Frame> {
    let mut rows = Vec::new();
    while let Some(mut row) = source.next_frame().await.unwrap() {
        row.position.source.clear(); // Backend identity differs; payloads must not.
        rows.push(row);
    }
    source.close().await.unwrap();
    rows
}
async fn integration(postgres: bool) {
    assert_eq!(
        std::env::var("BBGO_ALLOW_TEST_DATABASE_WRITES").as_deref(),
        Ok("1")
    );
    let key = if postgres {
        "BBGO_TEST_POSTGRES_URL"
    } else {
        "BBGO_TEST_MYSQL_URL"
    };
    let url = std::env::var(key).expect("explicit disposable database URL required");
    assert_eq!(
        url.split('?').next().unwrap().rsplit('/').next(),
        Some("bbgo_backtest_test")
    );
    static DRIVERS: Once = Once::new();
    DRIVERS.call_once(sqlx::any::install_default_drivers);
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let ddl = if postgres {
        include_str!("../schemas/postgres-v1.sql")
    } else {
        include_str!("../schemas/mysql-v1.sql")
    };
    // CREATE (not IF NOT EXISTS) prevents adopting/overwriting an existing DB.
    sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO capture_meta VALUES('symbol','BTCJPY')")
        .execute(&pool)
        .await
        .unwrap();
    let fixture = common::Database::fixture();
    populate(&pool, postgres, &fixture.0).await;
    let window = Window {
        from_ns: 0,
        through_ns: 9_000_000_000,
        gap_threshold_ms: 5000,
    };
    let database = || {
        if postgres {
            CollectorDatabase::Postgres {
                url: url.clone(),
                identity: "test-postgres".into(),
            }
        } else {
            CollectorDatabase::MySql {
                url: url.clone(),
                identity: "test-mysql".into(),
            }
        }
    };
    let mut native = open_collector(database(), "BTCJPY", window).await.unwrap();
    // Opening pins a repeatable snapshot before this later valid append.
    let insert = placeholders(
        "INSERT INTO capture_events(symbol,stream,writer_run_id,source_id,received_at_ns,processing_at_ns,ingested_at_ns,source_schema_version) VALUES('BTCJPY','bookticker','run-2','run-2:bookticker',?,?,?,'capture-csv-v1')",
        postgres,
    );
    sqlx::query(&insert)
        .bind(8_000_000_000_i64)
        .bind(8_000_000_000_i64)
        .bind(8_000_000_000_i64)
        .execute(&pool)
        .await
        .unwrap();
    let seq: i64 = sqlx::query("SELECT MAX(ingest_seq) FROM capture_events")
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    sqlx::query(&placeholders("INSERT INTO capture_event_availability VALUES(?,'capture_service',8000000000,'collector_callback_receive','observed')", postgres)).bind(seq).execute(&pool).await.unwrap();
    sqlx::query(&placeholders(
        "INSERT INTO capture_bookticker VALUES(?,'102','2','104','3')",
        postgres,
    ))
    .bind(seq)
    .execute(&pool)
    .await
    .unwrap();
    let mut sqlite = SqlTickSource::open(&fixture.0, "BTCJPY", window).unwrap();
    assert_eq!(frames(&mut *native).await, frames(&mut sqlite).await);
    let mut fresh = open_collector(database(), "BTCJPY", window).await.unwrap();
    assert_eq!(frames(&mut *fresh).await.len(), 8);
    // Read-only transaction is enforced by the engine, not only source convention.
    let mut read_only = pool
        .begin_with(if postgres {
            "BEGIN READ ONLY"
        } else {
            "START TRANSACTION READ ONLY"
        })
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE capture_meta SET value='wrong'")
            .execute(&mut *read_only)
            .await
            .is_err()
    );
    read_only.rollback().await.unwrap();
    sqlx::query(
        "UPDATE capture_event_availability SET availability_basis='unknown' WHERE ingest_seq=1",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(open_collector(database(), "BTCJPY", window).await.is_err());
    pool.close().await;
}
async fn populate(pool: &AnyPool, postgres: bool, fixture: &std::path::Path) {
    // Copy the synthetic SQL fixture as records, never via CSV. Tests compare
    // the exact same native collector rows across all storage backends.
    let sqlite = rusqlite::Connection::open(fixture).unwrap();
    for (table, columns, types) in [
        (
            "capture_writer_runs",
            "writer_run_id,symbol,started_at_ns,ended_at_ns,status",
            "ssnns",
        ),
        (
            "capture_sources",
            "source_id,symbol,stream,source_kind,writer_run_id,locator,source_schema_version,completeness_status,created_at_ns",
            "ssssssssn",
        ),
        (
            "capture_events",
            "ingest_seq,symbol,stream,writer_run_id,source_id,event_time_ns,received_at_ns,processing_at_ns,ingested_at_ns,gap_before_ms,gap_clock,source_schema_version",
            "nssssnnnnnss",
        ),
        (
            "capture_event_availability",
            "ingest_seq,consumer_id,available_at_ns,availability_basis,status",
            "nsnss",
        ),
        (
            "capture_bookticker",
            "ingest_seq,bid_price,bid_quantity,ask_price,ask_quantity",
            "nssss",
        ),
        (
            "capture_trades",
            "ingest_seq,trade_id_text,price,quantity,side,aggregate_id_text,first_trade_id_text,last_trade_id_text",
            "nsssssss",
        ),
    ] {
        let query = format!("SELECT {columns} FROM {table}");
        let mut statement = sqlite.prepare(&query).unwrap();
        let mut rows = statement.query([]).unwrap();
        let marks = vec!["?"; types.len()].join(",");
        let insert = placeholders(
            &format!("INSERT INTO {table}({columns}) VALUES({marks})"),
            postgres,
        );
        while let Some(row) = rows.next().unwrap() {
            let mut query = sqlx::query(&insert);
            for (i, kind) in types.bytes().enumerate() {
                query = if kind == b'n' {
                    query.bind(row.get::<_, Option<i64>>(i).unwrap())
                } else {
                    query.bind(row.get::<_, Option<String>>(i).unwrap())
                };
            }
            query.execute(pool).await.unwrap();
        }
    }
    if postgres {
        sqlx::query("SELECT setval(pg_get_serial_sequence('capture_events','ingest_seq'),(SELECT MAX(ingest_seq) FROM capture_events))").execute(pool).await.unwrap();
    }
}
#[tokio::test]
#[ignore = "requires explicit empty disposable PostgreSQL database; see crate README"]
async fn postgres_collector_contract() {
    integration(true).await;
}
#[tokio::test]
#[ignore = "requires explicit empty disposable MySQL database; see crate README"]
async fn mysql_collector_contract() {
    integration(false).await;
}
