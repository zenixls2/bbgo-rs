# bbgo-binance

Read-only Binance spot adapter. No private keys, order endpoints or live execution.

- `HttpTransport` uses fixed-origin HTTPS, no redirects, 5-second connection and
  15-second request timeouts, and an 8 MiB body limit. It performs no retries.
- `PublicClient` fetches exchange metadata and depth snapshots.
- `decode_depth` and `DepthEvent::apply` turn unwrapped diff-depth messages into
  sequence-validated timestamped quotes.
- `PublicTransport` allows offline fixture tests or a caller-owned transport.

REST bookTicker has no exchange timestamp: `query_ticker` deliberately returns
`Unsupported`, rather than mislabeling local receive time.

## Synchronization contract

1. Start the depth subscription and buffer events.
2. Fetch a snapshot.
3. Discard events whose final sequence is at/below the snapshot.
4. Require the first remaining event to bridge snapshot sequence + 1.
5. Apply further events in order; on any gap, resnapshot.
6. On reconnect, discard the old book and buffered events and increment generation.

The caller supplies the actual local receive timestamp and stream generation.
This crate does not provide a WebSocket connection/reconnect loop.

Metadata mapping covers tick/lot and minimum notional, not all price bands,
maximum notional or account permissions. Public origin access does not establish
Binance Japan execution eligibility. Synthetic wire fixtures are not recorded
traffic, and no live endpoint or regional account has been acceptance-tested.
