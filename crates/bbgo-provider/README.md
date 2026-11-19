# bbgo-provider

Exchange providers live in provider-specific Rust submodules. All nine registered
BBGO exchanges now have **read-only public spot** adapters: `backpack`, `binance`,
`bitfinex`, `bitget`, `bybit`, `coinbase`, `kucoin`, `max`, and `okex` (also `okx`).
Authenticated spot REST is available separately through
[`private`](PRIVATE.md): signed balance, GTC limit submit, open-order, cancel and
order-lookup services. Writes require explicit opt-in and trusted execution
filters. This is not full Go exchange/private API parity or live acceptance.

```rust
use bbgo_provider::binance::{HttpTransport, PublicClient};
```

## Shared public interface

```rust,no_run
# async fn example() -> bbgo_types::Result<()> {
use bbgo_provider::{ProviderId, public_client};
let client = public_client(ProviderId::Kucoin)?;
let instruments = client.query_instruments().await?;
let quote = client.query_book_ticker("BTC-USDT").await?;
assert!(quote.receive_time > 0);
# Ok(())
# }
```

`ProviderId::ALL` lists the nine providers. The factory returns a
`dyn public::SpotMarketDataService`, extending `MarketDataService` with discovery
and BBO queries that do not require inventing exchange timestamps. Each module
also exposes `PublicClient<T>`, `HttpTransport`, `PublicTransport`,
`decode_instruments`, and `decode_book_ticker` (Binance retains its original
snapshot/diff-depth API and implements the shared service).

The eight new adapters use common fixed-origin HTTPS transport, a project
User-Agent, no redirects/retries, 5-second connect/15-second request deadlines,
and an 8 MiB response cap. Injectable transports must preserve successful-body
and size-bound contracts. Decoders enforce the same body bound, API status,
symbol identity where present, exact decimal precision, positive uncrossed
prices, nonnegative sizes and timestamp units. Numeric JSON is parsed lexically,
not rounded through `f64`. Native identifiers/currency aliases are preserved:
e.g. `BTC-USDT`, `BTC_USDC`, `btctwd`, `AAVE:UST`; Bitfinex expects uppercase
native pairs without a leading `t`.

| Module | Discovery endpoint | BBO source | Exchange-event clock |
| --- | --- | --- | --- |
| `backpack` | `/api/v1/markets`, SPOT only | `/api/v1/depth`, best level on each side | Microseconds converted to domain milliseconds |
| `bitfinex` | `/v2/conf/pub:info:pair` | `/v2/book/t{pair}/P0`, one level per side | Absent |
| `bitget` | `/api/v2/spot/public/symbols` | `/api/v2/spot/market/tickers` | Payload `ts`, milliseconds |
| `bybit` | `/v5/market/instruments-info`, category spot | `/v5/market/tickers`, category spot | Absent; envelope server time is not quote-event time |
| `coinbase` | `/products` | `/products/{id}/book`, level 1 | Absent; ticker last-trade time is not BBO time |
| `kucoin` | `/api/v2/symbols` | `/api/v1/market/orderbook/level1` | Payload `time`, milliseconds |
| `max` | `/api/v3/markets` | `/api/v3/ticker` | Payload `at`, milliseconds |
| `okex` / `okx` | `/api/v5/public/instruments`, SPOT | `/api/v5/market/ticker` | Payload `ts`, milliseconds |

REST responses are not sequenced stream synchronization. Client receive time is
sampled after body receipt and is not a latency/clock-synchronization guarantee.
Stateless REST queries use generation 0; offline decoders take caller-owned
receive time/generation. Domain clocks lose submillisecond precision.

### Incomplete rules fail closed

`SpotInstrument` retains missing filters as `None`. `to_market()` and the legacy
`query_markets()` projection return `Unsupported` if any static tick/lot/min/max
quantity/min-notional field is unknown; they do not invent a huge maximum order
or assume a zero minimum. Use `query_instruments()` for discovery independent of
execution completeness. One incomplete instrument fails the entire legacy map
projection rather than silently omitting it.

In particular, Backpack/OKX do not expose all required minimum-notional rules
here, MAX lacks maximum quantity, and Bitfinex uses dynamic significant-digit
pricing (not a fixed tick) and does not supply an exact quantity step. Current
Coinbase products may omit legacy base-size bounds, and Bybit/KuCoin responses
can omit/change bounds. Bitget's `minTradeUSDT` is used only for USDT quote pairs,
not mislabeled as a minimum in other quote currencies. No static projection
implements every price band, maximum notional, auction/post-only restriction,
account/region permission or derivatives filter. Only limit order type is
advertised by new static projections, not a claim of full venue order support.

## Optional Rust SDK dependencies

Each dependency is re-exported as `module::sdk` behind its own Cargo feature.
The bounded BBGO public adapters above remain SDK-independent: these re-exports
are integration building blocks, **not** SDK-backed implementations of BBGO
signing, orders or private streams; the project's private adapter is separate.
Enabling a feature performs no authentication,
network request, credential lookup or live trading.

| Feature | Dependency | Upstream |
| --- | --- | --- |
| `sdk-backpack` | `bpx-api-client 0.23` | [backpack-exchange/bpx-api-client](https://github.com/backpack-exchange/bpx-api-client), exchange-owned |
| `sdk-bitfinex` | `bfx 0.2` | [BreezeWhite/bfx-rs](https://github.com/BreezeWhite/bfx-rs), third party |
| `sdk-bitget` | `bitget_sdk 0.1` | [blockchain-toolbox/bitget-rs](https://github.com/blockchain-toolbox/bitget-rs), third party |
| `sdk-bybit` | `rs_bybit 0.4` | [unkuseni/rs_bybit](https://github.com/unkuseni/rs_bybit), third party |
| `sdk-coinbase` | `coinbase-pro-rs 0.8` | [inv2004/coinbase-pro-rs](https://github.com/inv2004/coinbase-pro-rs), third party, Exchange/Pro API, not Advanced Trade |
| `sdk-kucoin` | `kucoin_rs 0.4` | [escwdev/kucoin_rs](https://github.com/escwdev/kucoin_rs), third party |
| `sdk-max` | `maicoin_max 2.1` | [COLDTURNIP/max-exchange-api-rs](https://github.com/COLDTURNIP/max-exchange-api-rs), third party, Apache-2.0 license file |
| `sdk-okx` | `rust-okx 0.6` | [fwqaaq/rust-okx](https://github.com/fwqaaq/rust-okx), third party |

```bash
cargo check -p bbgo-provider --features sdk-backpack
cargo check -p bbgo-provider --features sdks,sdk-vendored-tls
cargo test -p bbgo-provider --all-features
```

No SDK is enabled by default. Several SDKs have older transitive crypto/HTTP
stacks and enable native TLS; use system OpenSSL development libraries/pkg-config
or `sdk-vendored-tls` (builds OpenSSL from source; needs C toolchain, make and Perl).
`sdks` enables all eight SDKs; `dnum` still selects BBGO's numeric backend.
SDK types/float values are **not automatically converted** into BBGO accounting
values. Review upstream maintenance, API version, dependency advisories and
credential/logging behavior before using an SDK, especially private endpoints.
Dependency availability and compile checks are not production security approval.

Offline tests exercise all eight public adapters with synthetic wire fixtures,
transport routing, optional timestamps, malformed responses, exact precision,
duplicates, status and incomplete filters. They require no credentials/network.
No new venue endpoint, regional account, private endpoint or WebSocket transport
has passed live acceptance testing.

## Binance

The `binance` submodule below is the read-only public adapter.
Authenticated Binance REST uses the separate [`private`](PRIVATE.md) client.

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
