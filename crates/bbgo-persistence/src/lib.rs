//! Atomic single-key SQL checkpoints. Not a coordinated replay recovery bundle.
use async_trait::async_trait;
use bbgo_core::CheckpointStore;
use bbgo_types::{Error, Result};
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::{sync::Once, time::Duration};

pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy)]
enum Backend {
    Sqlite,
    MySql,
    Postgres,
}
/// Caller-owned dedicated database/schema; explicit initialization, no collector
/// migrations. Concurrent saves are last-writer-wins, not compare-and-swap.
pub struct SqlCheckpointStore {
    pool: AnyPool,
    backend: Backend,
}
fn adapter(error: impl std::fmt::Display) -> Error {
    Error::Adapter(format!("SQL checkpoint: {error}"))
}
fn key_hash(key: &str) -> Result<Vec<u8>> {
    if key.is_empty() || key.len() > 1024 {
        return Err(Error::Invalid("invalid checkpoint key".into()));
    }
    Ok(Sha256::digest(key.as_bytes()).to_vec())
}
impl SqlCheckpointStore {
    /// SQLite URLs must opt into file creation with `?mode=rwc` when necessary.
    /// No schema is created until initialize(). URLs are never stored/reported.
    pub async fn connect(url: &str) -> Result<Self> {
        let backend = if url.starts_with("sqlite:") {
            Backend::Sqlite
        } else if url.starts_with("mysql://") {
            Backend::MySql
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Backend::Postgres
        } else {
            return Err(Error::Invalid("unsupported checkpoint database URL".into()));
        };
        static DRIVERS: Once = Once::new();
        DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            // One connection also gives sqlite::memory: stable contents.
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(15))
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    let sql = match backend {
                        Backend::Sqlite => "PRAGMA busy_timeout=5000",
                        Backend::MySql => "SET SESSION innodb_lock_wait_timeout=5",
                        Backend::Postgres => "SET statement_timeout='30s'",
                    };
                    sqlx::query(sql).execute(&mut *connection).await?;
                    if matches!(backend, Backend::Sqlite) {
                        sqlx::query("PRAGMA synchronous=FULL")
                            .execute(&mut *connection)
                            .await?;
                    }
                    if matches!(backend, Backend::MySql) {
                        sqlx::query("SET SESSION MAX_EXECUTION_TIME=30000")
                            .execute(connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(adapter)?;
        Ok(Self { pool, backend })
    }
    /// Explicit opt-in DDL. Existing incompatible tables are not altered.
    pub async fn initialize(&self) -> Result<()> {
        let ddl = match self.backend {
            Backend::Sqlite => include_str!("../schemas/sqlite-v1.sql"),
            Backend::MySql => include_str!("../schemas/mysql-v1.sql"),
            Backend::Postgres => include_str!("../schemas/postgres-v1.sql"),
        };
        sqlx::raw_sql(ddl)
            .execute(&self.pool)
            .await
            .map_err(adapter)?;
        Ok(())
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
    fn sql(&self, template: &str) -> String {
        if !matches!(self.backend, Backend::Postgres) {
            return template.into();
        }
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
#[async_trait]
impl CheckpointStore for SqlCheckpointStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let hash = key_hash(key)?;
        let length = match self.backend {
            Backend::Sqlite => "length",
            _ => "octet_length",
        };
        // SQLx Any maps MySQL VARBINARY/BINARY (including CASE results) to
        // text. Transport binary fields as bounded ASCII hex on that backend,
        // then decode strictly; arbitrary checkpoint bytes must never be UTF-8.
        let field = |name: &str| {
            if matches!(self.backend, Backend::MySql) {
                format!("HEX({name})")
            } else {
                name.into()
            }
        };
        let key_field = field("key_bytes");
        let payload_field = field("payload");
        let digest_field = field("digest");
        // Guard in the SELECT, not just Rust: malformed/externally modified rows
        // cannot force unbounded payload/key/digest allocation.
        let query = format!(
            "SELECT version, CASE WHEN {length}(key_bytes)<=1024 THEN {key_field} ELSE NULL END, \
             CASE WHEN {length}(payload)<=4194304 THEN {payload_field} ELSE NULL END, \
             CASE WHEN {length}(digest)=32 THEN {digest_field} ELSE NULL END \
             FROM bbgo_checkpoints_v1 WHERE key_hash=?"
        );
        let Some(row) = sqlx::query(&self.sql(&query))
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter)?
        else {
            return Ok(None);
        };
        let version: i64 = row.try_get(0).map_err(adapter)?;
        let bytes = |column| -> Result<Vec<u8>> {
            if matches!(self.backend, Backend::MySql) {
                let ascii = row
                    .try_get::<Vec<u8>, _>(column)
                    .or_else(|_| row.try_get::<String, _>(column).map(String::into_bytes))
                    .map_err(adapter)?;
                hex::decode(ascii).map_err(adapter)
            } else {
                row.try_get(column).map_err(adapter)
            }
        };
        let stored_key = bytes(1)?;
        let payload = bytes(2)?;
        let digest = bytes(3)?;
        if version != 1
            || stored_key != key.as_bytes()
            || payload.len() > MAX_PAYLOAD
            || digest != Sha256::digest(&payload).as_slice()
        {
            return Err(adapter("version, key, size or checksum mismatch"));
        }
        Ok(Some(payload))
    }
    async fn save(&self, key: &str, value: &[u8]) -> Result<()> {
        let hash = key_hash(key)?;
        if value.len() > MAX_PAYLOAD {
            return Err(Error::Invalid("checkpoint payload too large".into()));
        }
        let suffix = match self.backend {
            Backend::MySql => {
                "ON DUPLICATE KEY UPDATE key_bytes=VALUES(key_bytes),version=VALUES(version),payload=VALUES(payload),digest=VALUES(digest)"
            }
            _ => {
                "ON CONFLICT(key_hash) DO UPDATE SET key_bytes=excluded.key_bytes,version=excluded.version,payload=excluded.payload,digest=excluded.digest"
            }
        };
        let query = self.sql(&format!(
            "INSERT INTO bbgo_checkpoints_v1(key_hash,key_bytes,version,payload,digest) VALUES(?,?,1,?,?) {suffix}"
        ));
        sqlx::query(&query)
            .bind(hash)
            .bind(key.as_bytes())
            .bind(value)
            .bind(Sha256::digest(value).to_vec())
            .execute(&self.pool)
            .await
            .map_err(adapter)?;
        Ok(())
    }
}
