//! Read-only adapter for BBGO's per-symbol capture schema v1, not CSV exports.
//! Holds one SQLite read snapshot for the entire run. Use frozen copies for
//! repeatable research; long reads against live WAL databases retain WAL pages.
use crate::{Model, source::*};
use bbgo_replay::TradePrint;
use bbgo_types::*;
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, path::Path, time::Duration};

const MIGRATION_HASH: &str = "89e36bca7907f4bdc2a97b2b5fa6a7e9370ec1f5c9eec89c851f2a0d1a52c79c";
const DDL_HASH: &str = "b9d0e38f2e6317fad2b050ec3c7f766e0532be810df1d2338210871996248040";
const PAGE_SIZE: usize = 256;
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub from_ns: i64,
    pub through_ns: i64,
    /// Collector stores ordinary inter-arrival intervals too, not just outages.
    /// Match its warning threshold (5000 ms) unless research specifies otherwise.
    pub gap_threshold_ms: i64,
}
pub struct SqlTickSource {
    connection: Option<Connection>,
    identity: String,
    symbol: String,
    window: Window,
    high_water: i64,
    after: (i64, i64, i64),
    buffer: VecDeque<Frame>,
    writer: Option<String>,
    generation: u64,
    exhausted: bool,
}
fn db(error: rusqlite::Error) -> Error {
    Error::Adapter(format!("capture SQLite: {error}"))
}
fn invalid(message: &str) -> Error {
    Error::Invalid(format!("capture SQLite: {message}"))
}
impl SqlTickSource {
    pub fn open(path: impl AsRef<Path>, symbol: &str, window: Window) -> Result<Self> {
        if symbol.is_empty()
            || symbol.len() > 32
            || !symbol
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || window.from_ns < 0
            || window.through_ns < window.from_ns
            || window.gap_threshold_ms <= 0
        {
            return Err(invalid("invalid symbol/window/gap threshold"));
        }
        let path = path.as_ref();
        let file = std::fs::symlink_metadata(path).map_err(|e| Error::Adapter(e.to_string()))?;
        if !file.file_type().is_file() {
            return Err(invalid("database must be a regular non-symlink file"));
        }
        let path = path
            .canonicalize()
            .map_err(|e| Error::Adapter(e.to_string()))?;
        let connection =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db)?;
        connection
            .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED")
            .map_err(db)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db)?;
        let actual_symbol: String = connection
            .query_row(
                "SELECT value FROM capture_meta WHERE key='symbol'",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        let migration: String = connection
            .query_row(
                "SELECT migration_sha256 FROM schema_migrations WHERE version=1",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        if version != 1
            || actual_symbol != symbol
            || !migration.eq_ignore_ascii_case(MIGRATION_HASH)
        {
            return Err(invalid("schema version, fingerprint or symbol mismatch"));
        }
        let mut digest = Sha256::new();
        {
            let mut statement = connection.prepare("SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name").map_err(db)?;
            let mut rows = statement.query([]).map_err(db)?;
            while let Some(row) = rows.next().map_err(db)? {
                let fields: Vec<String> = (0..4)
                    .map(|i| row.get(i))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(db)?;
                digest.update(format!(
                    "{}\0{}\0{}\0{}\n",
                    fields[0], fields[1], fields[2], fields[3]
                ));
            }
        }
        if format!("{:x}", digest.finalize()) != DDL_HASH {
            return Err(invalid("DDL fingerprint mismatch"));
        }
        let high_water: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(ingest_seq),0) FROM capture_events",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        // No inner join can hide a missing availability/source assertion. For
        // live captures validate even rows without a classifiable window clock.
        let bad: i64 = connection.query_row(r#"
SELECT EXISTS(
 SELECT 1 FROM capture_events e
 LEFT JOIN capture_sources s ON s.source_id=e.source_id
 LEFT JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
 LEFT JOIN capture_writer_runs r ON r.writer_run_id=e.writer_run_id
 WHERE e.ingest_seq<=?1 AND (
 e.symbol!=?2 OR s.source_id IS NULL OR s.symbol!=e.symbol OR s.stream!=e.stream OR
 (s.source_kind='live_ws' AND (
 s.completeness_status NOT IN ('active','validated') OR
 e.source_schema_version!='capture-csv-v1' OR s.source_schema_version!='capture-csv-v1' OR
 e.writer_run_id IS NULL OR e.writer_run_id='' OR r.writer_run_id IS NULL OR r.symbol!=e.symbol OR
 s.writer_run_id IS NULL OR s.writer_run_id!=e.writer_run_id OR r.status NOT IN ('running','closed') OR
 a.ingest_seq IS NULL OR a.status!='observed' OR a.availability_basis!='collector_callback_receive' OR
 a.available_at_ns IS NULL OR e.received_at_ns IS NULL OR a.available_at_ns!=e.received_at_ns OR
 e.received_at_ns<0 OR e.processing_at_ns IS NULL OR e.processing_at_ns<e.received_at_ns OR e.ingested_at_ns<e.processing_at_ns OR
 length(e.source_id)>512 OR length(e.writer_run_id)>512 OR
 (e.gap_before_ms IS NOT NULL AND (e.gap_before_ms<0 OR COALESCE(e.gap_clock,'')!='local_receive'))
 ))
 ))"#, params![high_water, symbol], |r| r.get(0)).map_err(db)?;
        if bad != 0 {
            return Err(invalid("invalid source/writer/availability contract"));
        }
        // Sorting must not silently repair a regressed receive clock within a
        // stream's ingest order. Ties remain allowed and causally ambiguous.
        let regressed: i64 = connection
            .query_row(
                r#"
SELECT EXISTS(SELECT 1 FROM (
 SELECT a.available_at_ns AS at,
 LAG(a.available_at_ns) OVER(PARTITION BY e.stream ORDER BY e.ingest_seq) AS previous
 FROM capture_events e JOIN capture_sources s ON s.source_id=e.source_id
 JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
 WHERE e.ingest_seq<=?1 AND s.source_kind='live_ws'
) WHERE at<previous)"#,
                [high_water],
                |r| r.get(0),
            )
            .map_err(db)?;
        if regressed != 0 {
            return Err(invalid("per-stream receive clock regressed"));
        }
        let identity = format!(
            "capture-v1:{}:{symbol}:{}:{}:{high_water}:{}",
            path.display(),
            window.from_ns,
            window.through_ns,
            window.gap_threshold_ms
        );
        Ok(Self {
            connection: Some(connection),
            identity,
            symbol: symbol.into(),
            window,
            high_water,
            after: (-1, -1, 0),
            buffer: VecDeque::new(),
            writer: None,
            generation: 0,
            exhausted: false,
        })
    }
    fn page(&mut self) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| invalid("source closed"))?;
        let mut statement = connection.prepare(r#"
SELECT e.ingest_seq,e.stream,e.source_id,e.writer_run_id,e.event_time_ns,e.received_at_ns,e.gap_before_ms,
 a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,
 substr(b.bid_price,1,129),substr(b.bid_quantity,1,129),substr(b.ask_price,1,129),substr(b.ask_quantity,1,129),
 substr(t.trade_id_text,1,129),substr(t.price,1,129),substr(t.quantity,1,129),t.side,substr(t.aggregate_id_text,1,129)
FROM capture_events e
JOIN capture_sources s ON s.source_id=e.source_id AND s.source_kind='live_ws'
JOIN capture_event_availability a ON a.ingest_seq=e.ingest_seq AND a.consumer_id='capture_service'
LEFT JOIN capture_bookticker b ON b.ingest_seq=e.ingest_seq
LEFT JOIN capture_trades t ON t.ingest_seq=e.ingest_seq
WHERE e.ingest_seq<=?1 AND a.available_at_ns BETWEEN ?2 AND ?3
 AND (a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,e.ingest_seq)>(?4,?5,?6)
ORDER BY a.available_at_ns,CASE e.stream WHEN 'bookticker' THEN 0 ELSE 1 END,e.ingest_seq LIMIT ?7
"#).map_err(db)?;
        let mut rows = statement
            .query(params![
                self.high_water,
                self.window.from_ns,
                self.window.through_ns,
                self.after.0,
                self.after.1,
                self.after.2,
                PAGE_SIZE as i64
            ])
            .map_err(db)?;
        let mut page = VecDeque::new();
        let mut after = self.after;
        while let Some(row) = rows.next().map_err(db)? {
            let sequence: i64 = row.get(0).map_err(db)?;
            let stream: String = row.get(1).map_err(db)?;
            let exchange_time_ns: Option<i64> = row.get(4).map_err(db)?;
            let received_at_ns: i64 = row.get(5).map_err(db)?;
            let available_at_ns: i64 = row.get(7).map_err(db)?;
            let priority: u8 = row.get(8).map_err(db)?;
            if sequence <= 0 || exchange_time_ns.is_some_and(|t| t <= 0) {
                return Err(invalid("invalid event sequence/time"));
            }
            let metadata = Metadata {
                source_id: row.get(2).map_err(db)?,
                writer_run_id: row.get(3).map_err(db)?,
                exchange_time_ns,
                received_at_ns,
                generation: 0,
                gap_before_ms: row.get(6).map_err(db)?,
            };
            let value = |column| -> Result<Value> {
                let text: String = row.get(column).map_err(db)?;
                if text.len() > 128 {
                    return Err(invalid("oversized decimal"));
                }
                text.parse().map_err(Error::from)
            };
            let tick = match stream.as_str() {
                "bookticker" => {
                    let quote = BookTicker {
                        symbol: self.symbol.clone(),
                        bid: value(9)?,
                        bid_quantity: value(10)?,
                        ask: value(11)?,
                        ask_quantity: value(12)?,
                        exchange_time: exchange_time_ns.map(|t| t / 1_000_000),
                        receive_time: received_at_ns / 1_000_000,
                        generation: 0,
                    };
                    quote.midpoint()?;
                    Tick::Book { quote, metadata }
                }
                "trades" => {
                    let id: Option<String> = row.get(13).map_err(db)?;
                    let aggregate: Option<String> = row.get(17).map_err(db)?;
                    let parse_id = |text: &str| -> Result<u64> {
                        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(invalid("invalid trade identifier"));
                        }
                        text.parse()
                            .map_err(|_| invalid("trade identifier overflow"))
                    };
                    let trade_id = id
                        .as_deref()
                        .map(parse_id)
                        .transpose()?
                        .ok_or_else(|| invalid("missing trade identifier"))?;
                    let aggregate_id = aggregate
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(parse_id)
                        .transpose()?;
                    let id = match aggregate_id.filter(|id| *id != 0) {
                        Some(id) => format!("aggregate:{id}"),
                        None => format!("trade:{trade_id}"),
                    };
                    let side: String = row.get(16).map_err(db)?;
                    let aggressor = match side.as_str() {
                        "BUY" => Side::Buy,
                        "SELL" => Side::Sell,
                        _ => return Err(invalid("invalid trade side")),
                    };
                    let print = TradePrint {
                        id,
                        symbol: self.symbol.clone(),
                        aggressor,
                        price: value(14)?,
                        quantity: value(15)?,
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
            after = (available_at_ns, i64::from(priority), sequence);
            page.push_back(Frame {
                position: Position {
                    available_at_ns,
                    priority,
                    source: self.identity.clone(),
                    sequence: sequence as u64,
                },
                input: Input::Tick(tick),
            });
        }
        self.exhausted = page.len() < PAGE_SIZE;
        self.after = after;
        self.buffer = page;
        Ok(())
    }
}
#[async_trait::async_trait]
impl BacktestSource for SqlTickSource {
    fn model(&self) -> Model {
        Model::Event
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.connection.is_none() {
            return Err(invalid("source closed"));
        }
        if self.buffer.is_empty() && !self.exhausted {
            self.page()?;
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
            .is_some_and(|writer| writer != &metadata.writer_run_id)
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
        if let Some(connection) = self.connection.take() {
            connection.execute_batch("ROLLBACK").map_err(db)?;
        }
        Ok(())
    }
}
