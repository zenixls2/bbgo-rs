# Migration record

## Integration scope

Further checkpoint/ML work is exposed as reusable interfaces rather than
additional gamma-specific implementation. `bbgo-core::recovery` defines
versioned component snapshots and isolated restore factories. `bbgo-model`
defines schema/timing-aware feature and inference contracts, artifact storage,
dataset readers and asynchronous third-party training jobs. It supplies validation
and adapter contract tests, not a training engine, model SDK, checkpoint
coordinator, or automatic trading policy. Existing gamma code is unchanged.

## Source and scope

Upstream project: `github.com/c9s/bbgo`.
Initial inspected source: the existing `~/src/gamma-capture` fork,
`d50c3747c0baf8087b9edac411f895faad022398`
(`https://github.com/zenixls2/gamma-capture`).
This identifier is a **fork commit**, not a claim of upstream release parity.
The fork already contains extensive gamma-specific extensions.
Collector schema/reader and candle-matching references additionally use the newer
sibling `~/src/bbgo` checkout at `a135236d38d10b56f5f1bacbd0306de9e8059040`.
The checked-in SQL fixture is synthetic data created with that collector's actual
schema DDL, not captured exchange traffic.

The migration moves reusable utilities, domain/accounting services, offline
execution, lifecycle/persistence and initial public-data/model components. It does not translate the
whole framework, replace its Go binary, or bridge Go to Rust through FFI/RPC.
Sibling application dependencies resolve locally; for independent distribution,
publish/version the packages or pin a real bbgo-rs repository revision.

## Implemented mapping and deliberate differences

| Go source | Rust destination | Compatibility |
| --- | --- | --- |
| `pkg/fixedpoint/convert.go` | `bbgo-fixedpoint::Legacy` (default `Value`) | Same 1e8 scale; checked integer arithmetic, finite values only |
| `pkg/fixedpoint/{dec,div128}.go` (`dnum`) | `bbgo-fixedpoint::Dnum`, feature-selected `Value`, static `Decimal` trait | 16-digit coefficient/exponent arithmetic; 1,007 Go-generated operation vectors; finite checked subset |
| `pkg/util/fnv.go` | `bbgo-util::{fnv32,fnv64}` | FNV-1a, wrapping arithmetic, UTF-8 bytes |
| `pkg/util/string.go` | `mask_key`, `split_by_length` | ASCII masking matches; Unicode masking uses scalar values; invalid chunk sizes rejected; oversized codepoints emitted alone |
| `pkg/util/reonce.go` | `Reonce` | Serialized reset/callback; panic poisons mutex instead of silently marking done |
| `pkg/nonce/millisecond.go` | `MillisecondNonce` | Milliseconds × 1000; linearizable CAS update prevents concurrent clock/store regression; exhaustion is explicit |
| `pkg/util/backoff/general.go` | `Backoff`, `runtime::retry` | Capped deterministic delays, bounded attempts, cancellation and overall deadline; no jitter or exact Go retry defaults |
| `pkg/types/{market,order,trade,balance,kline,exchange}.go` | `bbgo-types` | Initial spot subset, not full schema/wire compatibility |
| `pkg/bbgo` lifecycle/executor/persistence/notification | `bbgo-core` traits | Rust service boundaries; not a full Environment/Trader implementation |
| `pkg/indicator/{sma,ewma}.go` | `bbgo-indicator` | Rolling formulas; no callback/history API; SMA returns partial-window averages during warmup (`ready()` distinguishes them) |
| `pkg/strategy/gammacapture/crossing.go` | sibling `rust/src/crossing.rs` | Causal grid, dwell, cap/gap and epoch semantics; adds explicit initialization, stale-event/generation rejection and bounded gap work |
| Utility/series helpers | `bbgo-indicator::series`, `bbgo-util::runtime` | Bounded finite rolling data, cancellation-aware no-burst rate pacing |
| `pkg/types` books, intervals and accounting | `bbgo-types::{book,subscription,accounting}` | Sequence-invalidating level-2 book; fixed UTC intervals; long-only ledger, base/quote fees, deduplication and reconciliation differences |
| BBGO order execution/trade collection | `bbgo-backtest::replay` | Deterministic limited offline matcher and operation-journal recovery; not exchange queue emulation |
| BBGO persistence/session services | `bbgo-core::{checkpoint,session}` | Versioned checksummed atomic file replacement; explicit EOF/cancel/shutdown/reconnect handling |
| `pkg/exchange/binance` public data | `bbgo-provider::binance` | First provider submodule; metadata/depth REST and diff-depth wire decoding only, not a complete exchange adapter |
| Other registered exchange public data | `bbgo-provider::{backpack,bitfinex,bitget,bybit,coinbase,kucoin,max,okex}` | Public spot instruments/BBO with common factory/service, exact decoders and optional Rust SDK dependencies; incomplete filters fail closed |
| Registered exchange authentication / spot trading | `bbgo-provider::private` | Nine signed REST balance/limit-submit/open/cancel/order-lookup adapters; explicit write opt-in, cap and exact trusted filters, uncertain-write suppression; not private streams or a durable live engine |
| `pkg/strategy/gammacapture/model.go` | sibling `rust/src/model.rs` | Bounded intensity, Skellam and first-passage research subset; Go-generated numeric vectors |
| `pkg/backtest/matching.go`, collector SQL reader | `bbgo-backtest` | One candle/event runner and SQL tick sources for SQLite/MySQL/PostgreSQL; limited OHLC model, not full Go matching parity |

Decimal JSON values are strings; JSON numbers and nonfinite sentinels are rejected.
The default backend is eight-decimal checked integer arithmetic. The `dnum`
feature selects normalized 16-significant-digit arithmetic, including scientific
notation and sub-eight-decimal precision. Dnum's Go-compatible arithmetic has
different rounding from the legacy backend; exact wire/serde parsing rejects
precision loss rather than silently truncating inputs. Accounting uses
backend-specific conservative products and single-round allocations instead of
assuming raw 1e8 units. Both floor mathematically to a positive grid step.
See [numeric semantics](../crates/bbgo-fixedpoint/README.md) for finite-domain,
rounding and conversion limitations, and [build-flag coverage](BUILD_FLAGS.md)
for the complete inspected Go constraint inventory and unimplemented flag gaps.

## Execution safety and missing behavior

- `RunMode` defaults to replay. The supplied executor and gamma config reject live.
- `PaperExecutor` records/cancels limit intents with idempotent client IDs.
  It does **not** match fills, reserve balances, charge fees, update positions,
  validate venue filters or simulate queue priority.
- `ReplayExecutor` is separate: reserves funds atomically, shares public trade-print
  liquidity across FIFO eligible limit orders, emits partial fills/order/balance
  updates, applies cumulative quote-fee rounding and tracks long-only PnL.
  Matching must occur **before** strategy reaction to the same print. Fills use
  the resting limit price, not midpoint; no queue position, market impact or
  profitability claim. Replay order/trade-ID histories are unbounded for exact
  offline deduplication. Third-asset fees and short positions are unsupported.
- Gamma's lifecycle never submits orders. Its risk helper checks normalized
  JPY spot limits, freshness, liquidity inputs, order notional and spendable
  inventory/cash with a conservative fee cushion. Inputs must be independently
  sourced/validated; a future caller must atomically reserve funds and recheck
  before submission. This helper alone is not a production risk engine.
- Market discovery is an initial eligibility filter, not complete Binance Japan
  exchange/account validation. `bbgo-provider::binance` uses the fixed public HTTPS origin
  `api.binance.com`; Japan availability/account permissions have not been verified.
  It has injectable transport, time/body bounds and no redirects or credentials.
  Metadata maps tick/lot/minimum notional, not every exchange restriction (price
  bands, maximum notional and account-specific permissions remain unimplemented).
  REST bookTicker lacks exchange time, so `query_ticker` explicitly fails rather
  than fabricating a timestamp. Diff-depth plus a bridged snapshot produces
  timestamped quotes. Callers must buffer updates during snapshot acquisition,
  re-snapshot on gaps/reconnect and discard prior-generation data. There is no
   WebSocket transport/subscription loop. Signed private REST is separate in
   `bbgo-provider::private`; region/account acceptance remains outstanding.
- Private REST writes require explicit `WritePolicy::Enabled`, trusted current
  filters and a positive notional cap. Only spot GTC limits are supported.
  Async acknowledgment is `OrderStatus::Pending`, not a confirmed resting order
  or proof of zero fills. Per-client pre-await intent recording prevents blind
  resubmission after uncertain/dropped calls, but is not durable or cross-process.
  Cancel acknowledgments can race fills; batches are sequential, not atomic.
  Order lookup supports reconciliation, with Bitfinex day-scoped client-ID and
  Bybit realtime retention limitations. Incomplete pagination and ambiguous cash
  balances fail closed (notably modern Bybit UNIFIED responses lacking `free`).
  No fee/fill ledger, private stream, automatic live runner or acceptance gate is
  added. See [private contracts](../crates/bbgo-provider/PRIVATE.md).
- The sequential session runner supports cancellation, backpressure, shutdown on
  transport failure and explicit reconnect hooks. Actual network reconnection,
  resynchronization and private-order reconciliation remain adapter work.
- `MemoryStore` is process-local. `FileStore` uses bounded, versioned checksummed
  envelopes, private temporary files, file/directory sync and atomic replacement.
  It assumes a trusted local directory; it is not a distributed compare-and-swap
  store and uses synchronous bounded filesystem I/O.
- `bbgo-persistence::SqlCheckpointStore` adds explicit-schema SQLite/MySQL/PostgreSQL
  single-key atomic checkpoint replacement, byte-exact identities, 4 MiB payload
  limits and checked version/checksum loads. It is last-writer-wins, not CAS or a
  coordinated account/strategy/cursor transaction. Collector schemas remain separate.
- Gamma v3 checkpoints restore exact crossing-grid/count/intensity state for a continuous
  replay tape and validate configuration/symbol identities. The caller still owns
  tape position/stream identity and must not replay already-consumed input.
  Paper restarts and reconnects invalidate anchors and clear model exposure/events
  to force warmup, retaining cumulative counts. V2 checkpoints upgrade only with
  models disabled; configuration/model changes do not silently discard history.
  Replay orders/ledger have a separate validated journal checkpoint; they are not
  automatically saved by gamma shutdown. User-event accounting, Telegram and
  metrics are not integrated into gamma checkpoint/runtime state.
- Crossing events are observations, not statistical independence or profitability
  guarantees. Optional per-symbol intensity windows are wired into every gamma
  quote. Grid/window/count changes commit together or all roll back on error.
  Read-only snapshots cannot evict live events; first-passage queries require
  healthy sample counts. None of these are wired to order decisions.
  First-passage supports total intensity × horizon <= 100 and <= 4096 barrier
  states; Skellam supports means <= 100 and |k| <= 1000. Unsupported/nonfinite
  inputs fail explicitly. Intensity gap handling resets the entire sample
  (stricter than Go); event capacity fails instead of silently dropping data.
  Numerical fixture equivalence is not empirical model calibration.
- Replay account recovery replays successful operations from the trusted original
  seed/markets/fees instead of adopting derived balances, reservations or statuses.
  It preserves FIFO order, partial fills, cumulative fees, generated fill IDs and
  public-print deduplication/clocks. Invalid history/config/account identities fail
  before a rebuilt executor is exposed. Limits are 10,000 actions / 4 MiB;
  histories are not compacted. Seed structural validation is not reconciliation.
  Callers must quiesce and atomically bundle strategy, account, cursor and input
  identity: independent checkpoint keys are not a transaction. A synthetic gamma
   integration test demonstrates the combined payload. The shared backtest runner
   does not yet automatically commit/recover all components together.
- `bbgo-backtest` unifies strategy lifecycle, accounting and input dispatch for
  candle simulation and recorded event replay. SQL reads are snapshot-pinned,
  paged, receive-availability ordered with nanosecond order eligibility; same-time
  quote reactions cannot consume ambiguously ordered trades. SQLite reads the
  actual schema v1 from the newer sibling `~/src/bbgo/cmd/gammacapture-capture`;
  native MySQL/PostgreSQL schemas preserve its relational timestamp/payload
  contract. This does not change the existing SQLite-only Go collector writer.
  No CSV implementation is migrated. See the backtest README for fill assumptions,
  permissions, health-gate limitations, unbounded-history performance and recovery.
- Public BBO now preserves optional exchange time and quantities; public trades
  have a separate market-event type. Gamma's receive clock is explicitly configured;
  crossing timestamps are named `observation_time` with a clock label (legacy
  `exchange_time` checkpoint fields decode as exchange observations).

## Verification and compatibility coverage

- Fixedpoint fixtures execute both original Go build selections (`!dnum`, `dnum`).
  Both concrete implementations are tested on every build; the full workspace is
  additionally tested with each selected `Value` backend in dev/release profiles.
  Go legacy floating-path/sentinel behavior is intentionally not full parity.
  Native MySQL/PostgreSQL collector/checkpoint contracts passed with both backends.
- Utility fixtures execute the original Go FNV/string functions; Unicode masks
  intentionally differ, and fixtures avoid Go's oversized-codepoint chunk loop.
- Gamma fixtures execute original Go probability/intensity code with minimal type
  scaffolding. Rust tests compare finite supported-domain outputs to 1e-11.
- Accounting/replay tests cover reservations, partial fills, FIFO liquidity,
  fees, basis dust, PnL, duplicate/conflicting IDs and atomic failures. These
  are Rust invariants, **not yet cross-language Go execution-result fixtures**.
- Replay recovery tests compare resumed partial fills, fees, reservations, FIFO,
  trade IDs, cancellations and PnL against continuous execution; test seed-history
  ID collisions, malformed journals and durable store reopening. A gamma test
  resumes model/grid/account state and tape cursor from one combined payload.
- Restart tests compare uninterrupted and checkpoint-resumed crossing events and
  intensity snapshots, check model-state/config/grid identity validation and v2
  upgrades, and verify capacity rollback, gap resets and non-mutating projections.
  File corruption, session cleanup failures, cancellation and reconnects are tested.
- Binance fixtures are synthetic wire-shaped inputs, **not recorded exchange
  traffic**. Tests perform no live exchange calls. Region/account compatibility,
  recorded-fixture validation, out-of-sample evaluation and paper acceptance
   remain outstanding.
- Private adapter fixtures cover all nine venues and 27 independent Go-stdlib
  HMAC/Ed25519 signing vectors, intent/lookup identity, permissions, partial fills,
  uncertainty/cancellation, nonce sequencing, pagination and redaction. They are
  synthetic, not recorded private traffic; no credentials or live orders used.
- Backtest tests cover candle no-lookahead, nanosecond tick eligibility, deterministic
  ordering, SQL schema/availability validation, paging, read snapshots, cleanup and
  source merging. Collector fixtures use the original DDL and synthetic rows, not
  real traffic. Native server integration tests require explicit disposable
   databases and are provisioned in CI; ordinary offline tests leave them ignored.
   Collector and checkpoint contracts have now passed locally on MySQL 8.0.46
   and PostgreSQL 16.15 via unpacked Ubuntu packages without sudo or Docker.
   `cargo test-sql-local` reproduces this; `cargo test-sql-swarm` remains available.
- Intervals omit calendar months; weekly boundaries use Monday UTC rather than
  reproducing Go's month-relative weekly alignment.

## Roadmap status and remaining work

| Step | Status |
| --- | --- |
| 1. Utility fixtures, rolling series, fixed intervals, cancellation/rate/retry | Implemented scoped primitives; full Go utility surface still incomplete |
| 2. Books, fees, reconciliation, position/PnL, subscriptions | Implemented long-only spot subset; not all BBGO domain types/fee currencies |
| 3. Deterministic replay, reservations, trade collection | Shared candle/event backtest interface and SQLite/MySQL/PostgreSQL tick readers implemented; Go execution golden fixtures and realistic queue/latency modeling outstanding |
| 4. Durable checkpoints and session lifecycle | Implemented gamma model, offline account/order recovery and SQLite/MySQL/PostgreSQL checkpoints; automatic coordinated tape recovery, private-venue reconciliation and transport recovery outstanding |
| 5. Binance Japan public then private adapter | Partial public REST/wire plus signed private spot REST and order lookup; WebSocket transport, recorded traffic, Japan validation, durable reconciliation and live acceptance outstanding |
| 6. Gamma models and risk/execution | Research models/preflight, numeric parity, observation/forecast integration and model/grid restart tests implemented; alpha/execution policy, OOS tests and paper acceptance outstanding |
| 7. Other exchanges/indicators/strategies, CLI/config, SQL/API/UI/deployment | All nine have partial public spot and authenticated GTC REST providers, with optional SDK dependencies for eight; collector SQL and shared SQL checkpoints implemented. Private streams, full account/order coverage, remaining indicators/strategies, trade/order SQL services and application/API/UI remain |

No live acceptance gate has been passed by this foundation.
