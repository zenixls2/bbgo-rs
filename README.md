# bbgo-rs

Utility-first Rust migration of [c9s/bbgo](https://github.com/c9s/bbgo).
**This is an incremental migration, not a feature-complete BBGO replacement.**
Public market-data access is read-only. There is no live order-submission path.

## Workspace

| Package | Responsibility |
| --- | --- |
| `bbgo-fixedpoint` | Checked, finite eight-decimal values; string serialization; tick rounding |
| `bbgo-util` | Hashes, string helpers, nonce, resettable once, cancellable retries and pacing |
| `bbgo-types` | Spot contracts, sequenced books, intervals, subscriptions, ledger, fees and position/PnL |
| `bbgo-core` | Strategy/session lifecycle, cancellation, reconnect hooks, memory and atomic file checkpoints |
| `bbgo-indicator` | Incremental SMA/EWMA and bounded rolling series |
| `bbgo-replay` | Deterministic limit matching, FIFO liquidity, reservations, partial fills, accounting and journal recovery |
| `bbgo-backtest` | Unified candle/tick strategy runner; SQLite/MySQL/PostgreSQL collector sources |
| `bbgo-persistence` | Atomic versioned/checksummed SQLite/MySQL/PostgreSQL checkpoints |
| `bbgo-model` | Feature/inference, training-job, artifact and third-party model integration interfaces |
| `bbgo-xtask` | Cargo entry points for disposable database integration tests |
| `bbgo-binance` | Read-only bounded HTTPS metadata/depth client and timestamped depth-event decoding |

The dependency direction is `fixedpoint → types → core`. Utilities and indicators
are independent of the engine. Service contracts use async traits; the supplied
session runner/retry helpers use Tokio. The Binance HTTP transport uses rustls.

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Rust 2024 edition. Binance/backtest/persistence dependency graphs require Rust 1.88 or newer;
other packages declare 1.85. Verification currently uses Rust 1.97.1, not an
independent minimum-supported-version build.
Committed lockfiles pin dependency resolution.

## Gamma-capture

The sibling repository has a separate application workspace at
`../gamma-capture/rust`, using path dependencies on these shared packages:

```sh
cargo test --manifest-path ../gamma-capture/rust/Cargo.toml --locked
cargo run --manifest-path ../gamma-capture/rust/Cargo.toml --locked --example replay
```

It contains JPY spot discovery, a causal logarithmic crossing engine, bounded
intensity/Skellam/first-passage model ports, execution preflight gates and an
observation-only strategy lifecycle with versioned grid/model checkpoints.
The Go application is unchanged. The example is synthetic replay, not a
profitability backtest. The separate `bbgo-replay` executor simulates fills;
the gamma example deliberately submits no orders.
Replay executor recovery reconstructs account/orders from validated operations;
an application must atomically coordinate those bytes with strategy state and
the input cursor. See [replay recovery](crates/bbgo-replay/README.md).

## Backtesting

`bbgo-backtest::Backtest::run` accepts the same strategy/services for both:

- `Model::Kline`: streaming closed candles, synthetic OHLC limit matching.
- `Model::Event`: recorded collector BBO/trades from **SQLite, MySQL or
  PostgreSQL**, with nanosecond availability ordering and trade-volume fills.

No CSV reader is included. See [backtest contracts, SQL schemas and tests](crates/bbgo-backtest/README.md).
For disposable local MySQL/PostgreSQL integration tests with Docker Engine:

```sh
cargo sql-swarm-init # once, if not already a Swarm manager
cargo test-sql-swarm
```

The test command creates and cleans up its own stack and volumes. Published
ports listen on all host interfaces; use a trusted/firewalled development host.

Without Docker or sudo, Ubuntu 24.04 can run the same collector/checkpoint
contracts against unpacked server packages:

```sh
cargo test-sql-local --bootstrap-ubuntu /tmp/opencode/bbgo-sql-packages
# Later runs reuse those binaries, never previous database contents:
cargo test-sql-local --tools-root /tmp/opencode/bbgo-sql-packages/root
```

The bootstrap path must not exist. Downloads use configured apt repositories;
packages are extracted locally, **not installed**. Servers bind only loopback
on temporary ports, have random passwords, and stop before their owned temporary
data is removed. No existing databases or system services are used.
Both native MySQL 8 and PostgreSQL 16 contracts have also been run locally.
See [SQL checkpoint contracts](crates/bbgo-persistence/README.md).

Gamma includes a `sql_backtest` example using this shared runner; untimed BBO
requires explicitly configured receive-clock modeling, never invented exchange time.

Golden vectors are generated from the original Go utility/model implementations:

```sh
python3 tools/generate-util-fixtures.py
python3 tools/generate-gamma-fixtures.py
```

These require Go and the sibling source checkout. Ordinary Rust tests use the
checked-in fixtures and do not require Go, exchange credentials or network access.

See [docs/MIGRATION.md](docs/MIGRATION.md) for source provenance, compatibility
differences, limitations and the remaining migration order.

## Model/plugin boundaries

[`bbgo-model`](crates/bbgo-model/README.md) provides backend-neutral contracts for
feature transforms, local/remote inference, datasets, training jobs and artifact
repositories. `bbgo-core::recovery` defines generic checkpoint participants and
isolated restore factories. No further gamma implementation or ML framework is
required by these interfaces; application adapters own concrete integrations.

## License

AGPL-3.0-only, retaining BBGO's license. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
