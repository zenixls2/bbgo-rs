# bbgo-rs

Utility-first Rust migration of [c9s/bbgo](https://github.com/c9s/bbgo).
**This is an incremental migration, not a feature-complete BBGO replacement.**
Public market-data access is read-only. There is no live order-submission path.

## Workspace

| Package | Responsibility |
| --- | --- |
| `bbgo-fixedpoint` | Static `Decimal` trait; checked eight-decimal and 16-significant-digit dnum backends |
| `bbgo-util` | Hashes, string helpers, nonce, resettable once, cancellable retries and pacing |
| `bbgo-types` | Spot contracts, sequenced books, intervals, subscriptions, ledger, fees and position/PnL |
| `bbgo-core` | Strategy/session lifecycle, cancellation, reconnect hooks, memory and atomic file checkpoints |
| `bbgo-indicator` | Incremental SMA/EWMA and bounded rolling series |
| `bbgo-backtest` | Unified candle/tick runner, replay matching/accounting/recovery and SQLite/MySQL/PostgreSQL collector sources |
| `bbgo-persistence` | Atomic versioned/checksummed SQLite/MySQL/PostgreSQL checkpoints |
| `bbgo-model` | Feature/inference, training-job, artifact and third-party model integration interfaces |
| `bbgo-xtask` | Cargo entry points for disposable database integration tests |
| `bbgo-provider` | Nine public spot adapters and opt-in signed private REST; discovery/BBO, limit orders/lookups/cancels, optional Rust SDKs |

The dependency direction is `fixedpoint → types → core`. Utilities and indicators
are independent of the engine. Service contracts use async traits; the supplied
session runner/retry helpers use Tokio. The Binance HTTP transport uses rustls.

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Rust 2024 edition. Provider/backtest/persistence dependency graphs require Rust 1.88 or newer;
other packages declare 1.85. Verification currently uses Rust 1.97.1, not an
independent minimum-supported-version build.
Committed lockfiles pin dependency resolution.

## Numeric backends and build flags

The default `Value` is `Legacy` (signed 1e8 scale). The additive **`dnum`**
Cargo feature switches that alias throughout the dependency graph to `Dnum`
(16 significant digits, BBGO's coefficient/exponent arithmetic):

```sh
cargo test --workspace --all-targets --locked
cargo test --workspace --all-targets --locked --features dnum
cargo build --workspace --release --locked --features dnum
cargo test-sql-swarm --features dnum
```

An application can use `bbgo-types = { ..., features = ["dnum"] }`, or enable the
forwarded feature on core/backtest/provider/persistence/model. Both concrete
types are also available together through `bbgo-fixedpoint::{Legacy, Dnum}`;
`fn calculate<N: Decimal>(...)` is monomorphized, without trait objects.
Cargo features unify: enabling `dnum` anywhere selects it for the whole graph.

See [numeric semantics](crates/bbgo-fixedpoint/README.md) and the
[Go build-flag coverage table](docs/BUILD_FLAGS.md). CI tests both numeric backends
in dev/release profiles and both against native MySQL/PostgreSQL. Unmigrated Go
web/profiling/strategy flags are explicitly recorded, not advertised as working
Rust features.

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
profitability backtest. The `bbgo_backtest::replay` executor simulates fills;
the gamma example deliberately submits no orders.
Replay executor recovery reconstructs account/orders from validated operations;
an application must atomically coordinate those bytes with strategy state and
the input cursor. See [replay recovery](crates/bbgo-backtest/REPLAY.md).

## Backtesting

Replay and backtesting belong to the same `bbgo-backtest` package:
`bbgo_backtest::replay::{ReplayExecutor, TradePrint}` exposes the reusable
matcher, while `bbgo_backtest::Backtest` runs complete strategies.

## Exchange providers

`bbgo-provider` groups Binance, Backpack, Bitfinex, Bitget, Bybit, Coinbase,
KuCoin, MAX and OKX into provider submodules. `public_client(ProviderId)` provides
a shared read-only spot discovery/BBO interface; missing exchange timestamps and
execution filters are never invented. All eight new providers have optional
Rust SDK dependencies/re-exports. `private_client` adds authenticated balances,
GTC limit orders, open orders, lookups and cancel acknowledgments with explicit
write opt-in, trusted filters and a per-order cap. Uncertain writes are not retried.
Account/pagination limitations fail closed; private streams, durable live execution
and production acceptance remain outstanding. An SDK dependency is not an
execution adapter.
See [provider contracts](crates/bbgo-provider/README.md).

## Backtest sources

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
python3 tools/generate-fixedpoint-fixtures.py ../bbgo
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
