# Replay execution (`bbgo_backtest::replay`)

The `dnum` Cargo feature forwards the workspace numeric selection. Checkpoints
record `numeric_backend`; unlabelled historical checkpoints are legacy-only.
Recovery rejects a different selected backend rather than replaying identical
decimal strings with different arithmetic. See [numeric semantics](../bbgo-fixedpoint/README.md).

Deterministic offline, long-only spot limit matching. No credentials, network or
live-order path. Public prints allocate liquidity in order-acceptance FIFO;
fills use the resting limit, not optimistic price improvement. This does not
model exchange queue priority, latency or a production venue's complete filters.

## Recovery

- `checkpoint()` produces a versioned JSON journal of successful submissions,
  unique public prints (including non-fills) and effective cancellation batches.
  Ordinary journals retain version 1. Availability-constrained print journals use
  version 2 and retain each print's eligible client IDs for exact reconstruction.
- `from_checkpoint(markets, seed, fee_rate, bytes)` requires the **trusted original**
  market metadata, seed ledger and fee rate. All must match the checkpoint.
- Recovery reconstructs a new executor through normal validation/matching paths.
  Derived balances, locks, order status, fill IDs, cost basis, fee accumulators
  and print clocks are never adopted directly from serialized output state.
- `save_checkpoint` / `load_checkpoint` use `bbgo_core::CheckpointStore`.
  `FileStore` provides checksummed atomic durable writes; a missing key returns
  `None`, whereas corruption/config mismatch returns an error.
- The seed ledger is structurally validated, but its cost bases and balances
  still need independent reconciliation. Existing replay fill IDs in seed history
  advance the generator to avoid silently deduplicating a new fill.

At a **quiesced event boundary**, put executor bytes, strategy checkpoint, input
cursor and immutable tape/config identity into **one application-owned payload**
and save it atomically. Separate keys are not a transaction. Resume only from the
same input identity and cursor, and match each print before dispatching it to the
strategy. Do not use this journal to reconstruct or authorize real venue orders.

This is a bounded recovery foundation, not a compacted WAL: at most 10,000
operations and 4 MiB per serialized checkpoint. Invalid/noncanonical operations
fail recovery without exposing a partially rebuilt executor. Runtime histories
are not compacted or automatically stopped at those save limits; callers must
monitor capacity and save/rotate runs in advance. Rebuilding uses synchronous
work and bounded per-operation state clones, so large histories need profiling
or execution outside a latency-sensitive async task. Protect the local store:
checksums detect corruption, not malicious rewriting or rollback.

`bbgo-backtest` is the shared candle/event runner built on this accounting
executor. Its constrained print batches are atomic; no ineligible/same-time
reaction order can retroactively consume a print. Account recovery alone still
does not restore that runner's source positions and callback eligibility clocks.
