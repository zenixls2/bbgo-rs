# Unified offline backtesting

`Backtest::run` is the **single strategy lifecycle/accounting interface**:

| Execution model | Input source | Fill assumptions |
| --- | --- | --- |
| `Model::Kline` | `KlineSource` over a streaming closed-candle iterator | Synthetic OHLC path, full resting-limit fills |
| `Model::Event` | `open_collector` over SQLite, MySQL or PostgreSQL | Recorded trade-print liquidity only; BBO never fills |

Both use `bbgo-core::Strategy`, one `OrderExecutor`, reservations, FIFO orders,
partial fills, cumulative quote fees and long-only account/PnL from this package's
`bbgo_backtest::replay` module. Replay is not a separate Cargo package.
There is no live execution or CSV implementation.

Use `bbgo_backtest::replay::{ReplayExecutor, TradePrint}` for the standalone
matcher and [replay checkpoint contracts](REPLAY.md) for account recovery.

```rust,ignore
let mut source = open_collector(
    CollectorDatabase::Postgres {
        url: std::env::var("CAPTURE_DATABASE_URL")?,
        identity: "btc-capture-2026-11-16".into(), // never use a credential URL
    },
    "BTCJPY",
    Window { from_ns, through_ns, gap_threshold_ms: 5000 },
).await?;
let mut backtest = Backtest::new(Model::Event, markets, seed, fee_rate)?;
let report = backtest.run(
    &mut strategy, &mut *source, &checkpoints, &notifier, &cancel,
).await?;
```

Replace the source with `KlineSource::new(identity, candles.into_iter())` and
select `Model::Kline` for the candle model. Sources implement the same async
`BacktestSource`. `MergedSource` merges multiple same-model/symbol-database
inputs with one lookahead each. The runner handles EOF, cancellation, errors,
shutdown and closing all inputs. A runner instance cannot be reused.
Reports distinguish exhausted input from cancellation and include final account,
orders and last fully dispatched position.

## SQL tick contract

- **SQLite:** reads the actual schema v1 in
  `../bbgo/cmd/gammacapture-capture/capture_schema_v1.sql`; validates its migration
  and DDL hashes and opens a read-only transaction.
- **MySQL >= 8.0.16 / PostgreSQL:** native relational schemas are provided in
  [`schemas/mysql-v1.sql`](schemas/mysql-v1.sql) and
  [`schemas/postgres-v1.sql`](schemas/postgres-v1.sql). Apply them explicitly
  using a migration account; add `capture_meta` key `symbol` with the captured
  symbol. Readers only validate/read; they never migrate or create tables.
- The inspected Go collector currently writes SQLite. Adding native Rust readers
  **does not change that Go writer**. Server producers/importers must populate
  the documented native contract, preserving exact identifiers, decimal strings,
  timestamps and availability assertions; arbitrary unrelated SQL tick schemas
  are not guessed. The native schema marker is `bbgo-capture-v1`; unlike SQLite,
  native DDL is not compared to SQLite's fingerprint.
- Tables read: `capture_meta`, `capture_writer_runs`, `capture_sources`,
  `capture_events`, `capture_event_availability`, `capture_bookticker`,
  `capture_trades`. Records must have `live_ws` sources in `active`/`validated`
  state; archive/CSV/backfill source kinds are excluded, not assigned invented
  local availability. The legacy value `capture-csv-v1` remains a producer schema
  identifier in SQL; no CSV file is opened.
- Requires observed `capture_service` availability with basis
  `collector_callback_receive`, equal to `received_at_ns`; processing/ingestion
  clocks and writer/source identities must be consistent. Missing availability,
  failed/quarantined writers/sources and per-stream receive-clock regressions
  fail closed. This is **not** the Go reader's full heartbeat/health acceptance
  gate, or proof a capture has no dropped packets.
- One repeatable **read-only snapshot**, pinned high-water sequence, 256-row
  keyset pages, exact decimals, inclusive availability-time window. Use a frozen
  capture for reproducibility. Long transactions retain SQLite WAL pages /
  server MVCC history. Native queries have a 30-second statement limit; no
  automatic query retries. SQLite page queries are synchronous offline work,
  not a latency-sensitive service.
- Use SELECT-only server credentials and verified TLS for remote databases
  (`ssl-mode=VERIFY_IDENTITY` for MySQL, `sslmode=verify-full` for PostgreSQL,
  with the deployment's CA configuration). No database URLs appear in identities.
  Native schemas are an append-only producer contract, not a port of SQLite's
  immutability triggers; restrict writer privileges accordingly.

## Causality and limits

Nanosecond availability determines ordering and fill eligibility. BBO sorts
before trades on equal timestamps for deterministic replay, **not** as proof of
causal order: an order submitted at that exact nanosecond cannot consume that
trade. A print one nanosecond later can, even within the same millisecond.
Matching precedes user-event callbacks, which precede market-event callbacks.
Duplicate trade identifiers do not fill twice or trigger duplicate strategy
callbacks; conflicting identities fail. Fills use resting limit prices.

Real exchange time stays optional for BBO; receive time is not relabeled.
Public strategy timestamps currently project to milliseconds; the input envelope
retains the original nanoseconds. Gamma explicitly selects `Receive` observation
clock and labels its crossings. Writer changes or gaps >= configured threshold
trigger reconnect/warmup resets. Ordinary positive inter-arrival intervals below
the threshold are not outages. Merged feeds map local generations into a
monotonic session generation; reconnect conservatively resets strategy state.

Candle model: up bars follow open→low→high→close, down bars
open→high→low→close; orders at/before bar start are eligible and candle-close
reactions wait for a future bar. Candles must be closed, valid and nonoverlapping
per symbol. Synthetic full fills deliberately ignore candle volume/queueing.
This is an explicit limited simulator, **not complete Go matching parity**:
market/stop orders, taker fees, pessimistic penetration mode, real queue position,
market impact and intra-candle order generation remain unsupported.

Only normalized spot limits, long inventory, base/quote currencies; no shorts.
Source position in a report is **not a restart bundle**. Strategy checkpoints,
account journals and source/session eligibility clocks are not automatically
committed together. Histories are unbounded/uncompacted and current matching
clones state for atomicity: large-capture performance still needs optimization.
No profitability/live/paper acceptance is implied.

## Tests

Ordinary offline tests use a synthetic SQLite fixture built from the actual Go
DDL (not recorded traffic):

```sh
cargo test -p bbgo-backtest --locked
python3 tools/generate-capture-fixture.py # optional; needs sibling bbgo source
```

Native server tests are intentionally ignored without explicit disposable
databases. CI provisions MySQL 8.0 and PostgreSQL 16.
Both collector contracts have also passed locally on MySQL 8.0.46 and
PostgreSQL 16.15; they are no longer unverified server-only implementations.

### Without Docker or sudo (Ubuntu 24.04)

```sh
cargo test-sql-local --bootstrap-ubuntu /tmp/opencode/bbgo-sql-packages
cargo test-sql-local --tools-root /tmp/opencode/bbgo-sql-packages/root
```

The first command requires a **new** directory and downloads/extracts apt
packages, without installing them or modifying services. The second reuses
only binaries. Each run starts fresh PostgreSQL/MySQL data directories,
random credentials, and loopback-only temporary ports, tests both collector
and checkpoint contracts, then stops only its own processes before deleting
its temporary data. SIGKILL/host failure can require manual cleanup of owned
`/tmp/opencode/bbgo-native-sql-*` directories after servers are stopped.
Binary compatibility assumes Ubuntu 24.04 and its installed system libraries.
Optional Cargo arguments: `--cargo-test-args --offline`.

### Disposable local Docker Swarm

From the workspace root, with Docker Engine running on a local Unix socket:

```sh
cargo sql-swarm-init # once, only if this host is not already a Swarm manager
cargo test-sql-swarm
```

The dependency-free `bbgo-xtask` Cargo script deploys
`scripts/sql-swarm.yml`: MySQL 8.0 and PostgreSQL 16, pinned to the local
manager, fresh per-run named volumes, random test credentials, and readiness
checks before the ignored integration tests. A uniquely named stack and its
two volumes are removed on success, failure, SIGINT or SIGTERM. The Swarm
itself is **not** removed or reconfigured. SIGKILL/host failure requires manual
cleanup of the printed stack name and its `<stack>_mysql-data` and
`<stack>_postgres-data` volumes after tasks have stopped. No prune commands
or existing database volumes are used. A working image registry connection is
required on the first run.
This command now runs collector **and** shared SQL checkpoint contracts.

Default host ports are 13306 and 15432; override with
`BBGO_SWARM_MYSQL_PORT` / `BBGO_SWARM_POSTGRES_PORT`. Swarm host-mode ports
listen on **all host interfaces**, not just localhost: run only on a trusted,
firewalled development machine. Remote Docker endpoints are rejected.
Additional arguments are forwarded as Cargo test options, e.g.
`cargo test-sql-swarm --offline`.

Offline script lifecycle checks (mock Docker/Cargo; not database validation):

```sh
python3 scripts/test_sql_swarm_script.py
```

### Existing disposable servers

Alternatively, create
**empty disposable databases named `bbgo_backtest_test`**, then:

```sh
export BBGO_ALLOW_TEST_DATABASE_WRITES=1
export BBGO_TEST_MYSQL_URL='mysql://USER:PASSWORD@localhost/bbgo_backtest_test'
export BBGO_TEST_POSTGRES_URL='postgres://USER:PASSWORD@localhost/bbgo_backtest_test'
cargo test -p bbgo-backtest --test server_sql --locked -- --ignored
```

These tests create the native schema and compare the same fixture records,
ordering, optional clocks and generations against SQLite; verify snapshot
isolation, read-only transactions and invalid availability rejection. They
refuse a different database name and do not adopt existing tables. Use a fresh
disposable database for each run. Do not use collector/production credentials.
