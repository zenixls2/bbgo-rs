# SQL checkpoint storage

`SqlCheckpointStore` implements `bbgo_core::CheckpointStore` for SQLite,
MySQL 8.0.16+ (InnoDB), and PostgreSQL. Its table is **separate from capture
data**. It does not migrate the Go collector, account history, SQL APIs, or UI.

```rust,ignore
let store = bbgo_persistence::SqlCheckpointStore::connect(
    "sqlite:///trusted/state.sqlite3?mode=rwc"
).await?;
store.initialize().await?; // explicit DDL opt-in, not automatic migration
```

Server URLs use `mysql://...` or `postgresql://...`; use dedicated application
credentials, a caller-owned database/schema, and TLS on untrusted networks.
Schema files are in `schemas/`. `initialize()` creates the v1 table if absent;
it does not inspect/repair an incompatible existing table. Production operators
can apply the schema separately and grant only SELECT/INSERT/UPDATE.

- Keys: 1–1024 UTF-8 bytes; SHA-256 primary key, original bytes verified on load.
  Case and Unicode bytes are preserved, without database collation folding.
- Payloads: arbitrary bytes, including empty values, at most 4 MiB.
- A version and SHA-256 checksum are verified on every load. SQL projections
  bound loaded fields even if a privileged writer bypasses constraints.
- Each save is one atomic upsert. Concurrent saves are **last-writer-wins**;
  there is no distributed lock, compare-and-swap, or multi-key transaction.
- MySQL binary fields cross SQLx Any as bounded hex, avoiding its
  VARBINARY-as-UTF-8 interpretation. This is transport encoding, not encryption.
- Checksums detect accidental corruption, not malicious modification or rollback.
  Use a trusted DB, file permissions (SQLite), and restricted SQL writers.
- SQLite uses default FULL synchronous durability and a 5-second busy timeout.
  Server durability depends on operator PostgreSQL WAL / MySQL fsync settings.
  PG statements have a 30-second timeout; MySQL reads 30 seconds and InnoDB lock
  waits 5 seconds. One connection per store; this is not a high-throughput journal.
- Close stores with `close().await` before removing test databases.

Independent keys do not atomically coordinate strategy, account, source identity
and cursor. A restored strategy checkpoint alone must not restart a fresh tape.
The gamma SQL example refuses pre-existing state for its instance ID.

Offline SQLite tests exercise limits, binary/Unicode/case identity, corruption,
reopening, concurrent replacement and untorn reads. Native server contracts run
alongside collector-reader tests via `cargo test-sql-swarm` or
`cargo test-sql-local`; both provision disposable databases.
