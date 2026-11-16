//! Native MySQL/PostgreSQL collector sources plus database-independent factory.
//! Readers never create/migrate tables. See schemas/ for the native v1 contract.
use crate::{
    Model,
    source::*,
    sqlite::{SqlTickSource, Window},
};
use bbgo_replay::TradePrint;
use bbgo_types::*;
use sqlx::{
    Any, AnyPool, Row, Transaction,
    any::{AnyPoolOptions, AnyRow},
};
use std::{collections::VecDeque, path::PathBuf, sync::Once, time::Duration};

pub enum CollectorDatabase {
    Sqlite(PathBuf),
    MySql { url: String, identity: String },
    Postgres { url: String, identity: String },
}
/// Same runner/source interface regardless of storage backend. URLs are never
/// used as source identities, avoiding credentials in reports/checkpoints.
pub async fn open_collector(
    database: CollectorDatabase,
    symbol: &str,
    window: Window,
) -> Result<Box<dyn BacktestSource>> {
    match database {
        CollectorDatabase::Sqlite(path) => Ok(Box::new(SqlTickSource::open(path, symbol, window)?)),
        CollectorDatabase::MySql { url, identity } => Ok(Box::new(
            ServerTickSource::open(Dialect::MySql, &url, identity, symbol, window).await?,
        )),
        CollectorDatabase::Postgres { url, identity } => Ok(Box::new(
            ServerTickSource::open(Dialect::Postgres, &url, identity, symbol, window).await?,
        )),
    }
}
#[derive(Clone, Copy)]
enum Dialect {
    MySql,
    Postgres,
}
impl Dialect {
    fn sql(self, template: &str) -> String {
        match self {
            Self::MySql => template.into(),
            Self::Postgres => {
                let mut index = 0;
                template
                    .chars()
                    .map(|c| {
                        if c == '?' {
                            index += 1;
                            format!("${index}")
                        } else {
                            c.to_string()
                        }
                    })
                    .collect()
            }
        }
    }
    fn meta_key(self) -> &'static str {
        match self {
            Self::MySql => "`key`",
            Self::Postgres => "\"key\"",
        }
    }
}
fn db(error: sqlx::Error) -> Error {
    Error::Adapter(format!("capture SQL: {error}"))
}
fn invalid(message: &str) -> Error {
    Error::Invalid(format!("capture SQL: {message}"))
}
fn integer(row: &AnyRow, column: usize) -> Result<i64> {
    row.try_get::<i64, _>(column)
        .or_else(|_| row.try_get::<i32, _>(column).map(i64::from))
        .map_err(db)
}
// MySQL's binary text collation is exposed by SQLx Any as BLOB. Decode
// strictly as UTF-8 rather than changing the case-sensitive schema contract.
fn text(row: &AnyRow, column: usize) -> Result<String> {
    row.try_get::<String, _>(column)
        .or_else(|_| {
            let bytes: Vec<u8> = row.try_get(column)?;
            String::from_utf8(bytes).map_err(|error| sqlx::Error::Decode(Box::new(error)))
        })
        .map_err(db)
}
fn optional_text(row: &AnyRow, column: usize) -> Result<Option<String>> {
    use sqlx::ValueRef;
    if row.try_get_raw(column).map_err(db)?.is_null() {
        Ok(None)
    } else {
        text(row, column).map(Some)
    }
}
pub struct ServerTickSource {
    pool: AnyPool,
    transaction: Option<Transaction<'static, Any>>,
    dialect: Dialect,
    identity: String,
    symbol: String,
    window: Window,
    high_water: i64,
    after: (i64, i64, i64),
    buffer: VecDeque<Frame>,
    exhausted: bool,
    writer: Option<String>,
    generation: u64,
}
impl ServerTickSource {
    async fn open(
        dialect: Dialect,
        url: &str,
        identity: String,
        symbol: &str,
        window: Window,
    ) -> Result<Self> {
        if identity.is_empty()
            || identity.contains("://")
            || identity.len() > 256
            || symbol.is_empty()
            || symbol.len() > 32
            || !symbol
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || window.from_ns < 0
            || window.through_ns < window.from_ns
            || window.gap_threshold_ms <= 0
        {
            return Err(invalid("invalid identity/symbol/window/gap threshold"));
        }
        let correct_scheme = match dialect {
            Dialect::MySql => url.starts_with("mysql://"),
            Dialect::Postgres => url.starts_with("postgres://") || url.starts_with("postgresql://"),
        };
        if !correct_scheme {
            return Err(invalid("database URL/backend mismatch"));
        }
        static DRIVERS: Once = Once::new();
        DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(15))
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    let setup = match dialect {
                        Dialect::MySql => "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ",
                        Dialect::Postgres => "SET statement_timeout='30s'",
                    };
                    sqlx::query(setup).execute(&mut *connection).await?;
                    if matches!(dialect, Dialect::MySql) {
                        sqlx::query("SET SESSION MAX_EXECUTION_TIME=30000")
                            .execute(connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(db)?;
        let begin = match dialect {
            Dialect::MySql => "START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY",
            Dialect::Postgres => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
        };
        let mut transaction = pool.begin_with(begin).await.map_err(db)?;
        let metadata_sql = format!(
            "SELECT value FROM capture_meta WHERE {}=?",
            dialect.meta_key()
        );
        let actual_symbol = text(
            &sqlx::query(&dialect.sql(&metadata_sql))
                .bind("symbol")
                .fetch_one(&mut *transaction)
                .await
                .map_err(db)?,
            0,
        )?;
        let contract = text(
            &sqlx::query(&dialect.sql(&metadata_sql))
                .bind("schema_contract")
                .fetch_one(&mut *transaction)
                .await
                .map_err(db)?,
            0,
        )?;
        if actual_symbol != symbol || contract != "bbgo-capture-v1" {
            return Err(invalid("symbol/native schema contract mismatch"));
        }
        let high_water = integer(
            &sqlx::query("SELECT COALESCE(MAX(ingest_seq),0) FROM capture_events")
                .fetch_one(&mut *transaction)
                .await
                .map_err(db)?,
            0,
        )?;
        let bad = integer(
            &sqlx::query(&dialect.sql(CONTRACT_CHECK))
                .bind(high_water)
                .bind(symbol)
                .fetch_one(&mut *transaction)
                .await
                .map_err(db)?,
            0,
        )?;
        if bad != 0 {
            return Err(invalid("invalid source/writer/availability contract"));
        }
        let regressed = integer(
            &sqlx::query(&dialect.sql(REGRESSION_CHECK))
                .bind(high_water)
                .fetch_one(&mut *transaction)
                .await
                .map_err(db)?,
            0,
        )?;
        if regressed != 0 {
            return Err(invalid("per-stream receive clock regressed"));
        }
        Ok(Self {
            pool,
            transaction: Some(transaction),
            dialect,
            identity: format!(
                "capture-v1:{identity}:{symbol}:{}:{}:{high_water}:{}",
                window.from_ns, window.through_ns, window.gap_threshold_ms
            ),
            symbol: symbol.into(),
            window,
            high_water,
            after: (-1, -1, 0),
            buffer: VecDeque::new(),
            exhausted: false,
            writer: None,
            generation: 0,
        })
    }
    async fn page(&mut self) -> Result<()> {
        let transaction = self
            .transaction
            .as_mut()
            .ok_or_else(|| invalid("source closed"))?;
        let rows = sqlx::query(&self.dialect.sql(PAGE))
            .bind(self.high_water)
            .bind(self.window.from_ns)
            .bind(self.window.through_ns)
            .bind(self.after.0)
            .bind(self.after.1)
            .bind(self.after.2)
            .fetch_all(&mut **transaction)
            .await
            .map_err(db)?;
        let mut page = VecDeque::new();
        let mut after = self.after;
        for row in &rows {
            let sequence = integer(row, 0)?;
            let stream = text(row, 1)?;
            let exchange_time_ns: Option<i64> = row.try_get(4).map_err(db)?;
            let received_at_ns = integer(row, 5)?;
            let available_at_ns = integer(row, 7)?;
            let priority = integer(row, 8)?;
            if sequence <= 0 || exchange_time_ns.is_some_and(|t| t <= 0) {
                return Err(invalid("invalid event sequence/time"));
            }
            let metadata = Metadata {
                source_id: text(row, 2)?,
                writer_run_id: text(row, 3)?,
                exchange_time_ns,
                received_at_ns,
                generation: 0,
                gap_before_ms: row.try_get(6).map_err(db)?,
            };
            let decimal = |column| -> Result<Value> {
                let text = text(row, column)?;
                if text.len() > 128 {
                    return Err(invalid("oversized decimal"));
                }
                text.parse().map_err(Error::from)
            };
            let tick = match stream.as_str() {
                "bookticker" => {
                    let quote = BookTicker {
                        symbol: self.symbol.clone(),
                        bid: decimal(9)?,
                        bid_quantity: decimal(10)?,
                        ask: decimal(11)?,
                        ask_quantity: decimal(12)?,
                        exchange_time: exchange_time_ns.map(|t| t / 1_000_000),
                        receive_time: received_at_ns / 1_000_000,
                        generation: 0,
                    };
                    quote.midpoint()?;
                    Tick::Book { quote, metadata }
                }
                "trades" => {
                    let trade = text(row, 13)?;
                    let aggregate = optional_text(row, 17)?;
                    let parse_id = |text: &str| -> Result<u64> {
                        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(invalid("invalid trade identifier"));
                        }
                        text.parse()
                            .map_err(|_| invalid("trade identifier overflow"))
                    };
                    let trade = parse_id(&trade)?;
                    let aggregate = aggregate
                        .as_deref()
                        .filter(|t| !t.is_empty())
                        .map(parse_id)
                        .transpose()?;
                    let id = match aggregate.filter(|id| *id != 0) {
                        Some(id) => format!("aggregate:{id}"),
                        None => format!("trade:{trade}"),
                    };
                    let side = text(row, 16)?;
                    let aggressor = match side.as_str() {
                        "BUY" => Side::Buy,
                        "SELL" => Side::Sell,
                        _ => return Err(invalid("invalid trade side")),
                    };
                    let print = TradePrint {
                        id,
                        symbol: self.symbol.clone(),
                        aggressor,
                        price: decimal(14)?,
                        quantity: decimal(15)?,
                        exchange_time: exchange_time_ns
                            .ok_or_else(|| invalid("trade missing exchange time"))?
                            / 1_000_000,
                    };
                    if print.price <= Value::ZERO || print.quantity <= Value::ZERO {
                        return Err(invalid("invalid trade price/quantity"));
                    }
                    Tick::Trade { print, metadata }
                }
                _ => return Err(invalid("unsupported stream")),
            };
            after = (available_at_ns, priority, sequence);
            page.push_back(Frame {
                position: Position {
                    available_at_ns,
                    priority: priority as u8,
                    source: self.identity.clone(),
                    sequence: sequence as u64,
                },
                input: Input::Tick(tick),
            });
        }
        self.exhausted = rows.len() < 256;
        self.after = after;
        self.buffer = page;
        Ok(())
    }
}
#[async_trait::async_trait]
impl BacktestSource for ServerTickSource {
    fn model(&self) -> Model {
        Model::Event
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.transaction.is_none() {
            return Err(invalid("source closed"));
        }
        if self.buffer.is_empty() && !self.exhausted {
            self.page().await?;
        }
        let Some(mut frame) = self.buffer.pop_front() else {
            return Ok(None);
        };
        let Input::Tick(tick) = &mut frame.input else {
            unreachable!()
        };
        let metadata = tick.metadata();
        if self
            .writer
            .as_ref()
            .is_some_and(|old| old != &metadata.writer_run_id)
            || metadata
                .gap_before_ms
                .is_some_and(|gap| gap >= self.window.gap_threshold_ms)
        {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| invalid("generation overflow"))?;
        }
        self.writer = Some(metadata.writer_run_id.clone());
        match tick {
            Tick::Book { quote, metadata } => {
                quote.generation = self.generation;
                metadata.generation = self.generation;
            }
            Tick::Trade { metadata, .. } => metadata.generation = self.generation,
        }
        Ok(Some(frame))
    }
    async fn close(&mut self) -> Result<()> {
        self.buffer.clear();
        let rollback = if let Some(transaction) = self.transaction.take() {
            transaction.rollback().await.map_err(db)
        } else {
            Ok(())
        };
        self.pool.close().await;
        rollback
    }
}

const CONTRACT_CHECK: &str = r#"
SELECT CASE WHEN EXISTS(
 SELECT 1 FROM capture_events e
 LEFT JOIN capture_sources s ON s.source_id=e.source_id
 LEFT JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
 LEFT JOIN capture_writer_runs r ON r.writer_run_id=e.writer_run_id
 WHERE e.ingest_seq<=? AND (
 e.symbol<>? OR s.source_id IS NULL OR s.symbol<>e.symbol OR s.stream<>e.stream OR
 (s.source_kind='live_ws' AND (
 s.completeness_status NOT IN ('active','validated') OR
 COALESCE(e.source_schema_version,'')<>'capture-csv-v1' OR COALESCE(s.source_schema_version,'')<>'capture-csv-v1' OR
 e.writer_run_id IS NULL OR e.writer_run_id='' OR r.writer_run_id IS NULL OR r.symbol<>e.symbol OR
 s.writer_run_id IS NULL OR s.writer_run_id<>e.writer_run_id OR r.status NOT IN ('running','closed') OR
 a.ingest_seq IS NULL OR a.status<>'observed' OR a.availability_basis<>'collector_callback_receive' OR
 a.available_at_ns IS NULL OR e.received_at_ns IS NULL OR a.available_at_ns<>e.received_at_ns OR
 e.received_at_ns<0 OR e.processing_at_ns IS NULL OR e.processing_at_ns<e.received_at_ns OR e.ingested_at_ns<e.processing_at_ns OR
 length(e.source_id)>512 OR length(e.writer_run_id)>512 OR
 (e.gap_before_ms IS NOT NULL AND (e.gap_before_ms<0 OR COALESCE(e.gap_clock,'')<>'local_receive'))
 ))
 )) THEN 1 ELSE 0 END"#;
const REGRESSION_CHECK: &str = r#"
SELECT CASE WHEN EXISTS(SELECT 1 FROM (
 SELECT a.available_at_ns AS at,
 LAG(a.available_at_ns) OVER(PARTITION BY e.stream ORDER BY e.ingest_seq) AS previous
 FROM capture_events e JOIN capture_sources s ON s.source_id=e.source_id
 JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
 WHERE e.ingest_seq<=? AND s.source_kind='live_ws'
) AS ordered_events WHERE at<previous) THEN 1 ELSE 0 END"#;
const PAGE: &str = r#"
SELECT e.ingest_seq,e.stream,e.source_id,e.writer_run_id,e.event_time_ns,e.received_at_ns,e.gap_before_ms,
 a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,
 substr(b.bid_price,1,129),substr(b.bid_quantity,1,129),substr(b.ask_price,1,129),substr(b.ask_quantity,1,129),
 substr(t.trade_id_text,1,129),substr(t.price,1,129),substr(t.quantity,1,129),t.side,substr(t.aggregate_id_text,1,129)
FROM capture_events e
JOIN capture_sources s ON s.source_id=e.source_id AND s.source_kind='live_ws'
JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
LEFT JOIN capture_bookticker b ON b.ingest_seq=e.ingest_seq
LEFT JOIN capture_trades t ON t.ingest_seq=e.ingest_seq
WHERE e.ingest_seq<=? AND a.available_at_ns BETWEEN ? AND ?
 AND (a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,e.ingest_seq)>(?,?,?)
ORDER BY a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,e.ingest_seq LIMIT 256"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binds_are_backend_native() {
        assert_eq!(Dialect::Postgres.sql("x=? AND y=?"), "x=$1 AND y=$2");
        assert_eq!(Dialect::MySql.sql("x=? AND y=?"), "x=? AND y=?");
        assert_eq!(PAGE.matches('?').count(), 6);
        assert_eq!(CONTRACT_CHECK.matches('?').count(), 2);
    }
}
