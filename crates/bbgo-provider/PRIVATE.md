# Authenticated spot REST

`bbgo_provider::private` supplies `PrivateClient<T>`, `private_client`,
`Credentials`, `WritePolicy`, `Clock`, and an injectable `PrivateTransport`.
It implements shared `AccountService`, `TradeService`, and `OrderQueryService`
for the nine registered providers. It does not enable the core session's live
mode or change the gamma application.

## Explicit authorization

```rust,no_run
# async fn example(credentials: bbgo_provider::private::Credentials,
# execution_markets: std::collections::BTreeMap<String, bbgo_types::Market>)
# -> bbgo_types::Result<()> {
use bbgo_provider::{ProviderId, private_client, private::WritePolicy};
use bbgo_types::{AccountService, OrderQueryService, TradeService, Value};

// ReadOnly is the default policy. Keys never come from implicit environment lookup.
let client = private_client(
    ProviderId::Binance, credentials, execution_markets, WritePolicy::ReadOnly,
)?;
let balances = client.query_balances().await?;
let open = client.query_open_orders("BTCUSDT").await?;
// Only if an application explicitly authorizes real writes, use instead:
let policy = WritePolicy::Enabled {
    max_order_notional: Value::parse_exact("100")?,
};
# let _ = (balances, open, policy);
# Ok(()) }
```

An enabled client can **place real orders and cancel them** when called. Library
features, construction and tests alone do not trade. Use API keys without
withdrawal/transfer permission, IP restrictions, a synchronized clock, and
application-owned live acceptance/risk gates. Production/fixed origins are used;
there is no sandbox/region URL override. Coinbase means **Exchange**, not
Advanced Trade; Bitget/KuCoin use the classic API contracts listed below, not UTA/HF.

Supply complete, current, trusted spot `Market` filters, keyed by native symbol.
The client rejects unknown markets, any normalization change, nonpositive
amounts, unsupported order types and orders above its conservative per-order
notional cap. **Only GTC limit orders** are supported: no market, maker-only,
stop, conditional, margin, borrowing, derivatives or leveraged order path.
Missing public discovery filters are not filled in automatically. Bitfinex's
significant-digit rules need caller validation in addition to any static grid.
The cap is not a balance reservation, aggregate exposure limit, fee allowance,
freshness gate, rate limiter, or prevention of oversells; those remain application
responsibilities. Venue/account constraints can be stricter than static filters.

## Endpoints and authentication

| Provider | Balances | Submit / open orders | Signing |
| --- | --- | --- | --- |
| Binance | `/api/v3/account` | `/api/v3/order`, `/api/v3/openOrders` | HMAC-SHA256 query, API-key header, milliseconds / 5s window |
| Backpack | `/api/v1/capital` | `/api/v1/order`, `/api/v1/orders` | Ed25519, endpoint instruction + sorted parameters + milliseconds / 5s window |
| Bitfinex | `/v2/auth/r/wallets` (exchange wallet only) | `/v2/auth/w/order/submit`, `/v2/auth/r/orders/t{symbol}` | HMAC-SHA384 over `/api` + path + monotonic nonce + exact body |
| Bitget | `/api/v2/spot/account/assets` | `/api/v2/spot/trade/place-order`, `/unfilled-orders` under the same trade prefix | Base64 HMAC-SHA256, timestamp/method/path/query/body + passphrase |
| Bybit | `/v5/account/wallet-balance`, UNIFIED | `/v5/order/create`, `/v5/order/realtime`, category spot, leverage disabled | HMAC-SHA256, milliseconds/key/5s-window/query-or-body |
| Coinbase Exchange | `/accounts` | `/orders` | Base64-decoded secret, base64 HMAC-SHA256 timestamp/method/path/query/body + passphrase |
| KuCoin classic | `/api/v1/accounts`, type trade | `/api/v1/orders` | Base64 HMAC-SHA256, key version 2, HMAC-protected passphrase |
| MAX | `/api/v3/wallet/spot/accounts` | `/api/v3/wallet/spot/order`, `/orders/open` under the same wallet prefix | Base64 nonce/path/parameters payload, hex HMAC-SHA256 |
| OKX | `/api/v5/account/balance` | `/api/v5/trade/order`, `/api/v5/trade/orders-pending`, cash/SPOT only | RFC3339 millisecond UTC timestamp, base64 HMAC-SHA256 + passphrase |

Single-order cancel routes use the same signing schemes; OKX also checks each
item's `sCode`, not just the envelope code. Native IDs are preserved. Backpack
client IDs must be canonical nonzero `u32`; Bitfinex IDs nonzero integers fitting
45 bits; Coinbase client IDs UUIDs; OKX client IDs alphanumeric up to 32 bytes.
Other providers enforce their local length/character bounds. No random IDs or
lossy hashes are substituted. Amounts are exact decimal strings without floating
point conversion or scientific wire notation, with either numeric backend.

Credentials have no serde/clone implementation and use zeroizing owned strings;
`Debug` redacts credentials, signed requests and responses. This does **not**
guarantee zeroization of every HTTP/crypto intermediate or caller-owned copy.
Backpack accepts a **base64 32-byte Ed25519 seed**, and verifies the supplied
base64 public key matches it; 64-byte expanded secrets are not accepted.

## State, reconciliation and uncertain writes

- Full order responses validate the returned symbol, side, type, quantity,
  price, execution bounds and client ID against the submitted intent.
- Bitget, Bybit, KuCoin and OKX submission APIs acknowledge asynchronously.
  They return **`OrderStatus::Pending`**, not a fabricated open/unfilled state.
  `executed_quantity = 0` on this acknowledgment is not evidence of no fills;
  obtain an observed state through lookup before accounting/reserving from it.
- `query_order(symbol, exchange_id)` and `query_order_by_client_id` support
  reconciliation. Bitfinex order-ID lookup checks active orders then history;
  client-ID lookup is explicitly unsupported without its required day scope.
  Bybit uses realtime order lookup (with venue retention limitations), not an
  unlimited archive. A not-found/error response is **never proof of no execution**.
- Per-client submission intent IDs are recorded **before any await**. An identical
  successful repeat returns the original acknowledgment, not current order state.
  A conflicting ID fails; any failed/dropped/uncertain attempt is blocked from
  resubmission by that client. Even a definitive rejection is retained
  conservatively. Reconcile with the venue; there is no automatic retry/unblock.
- This intent map is in-memory, unbounded, uncheckpointed and not cross-process.
  Reconstructing a client loses it. Applications must durably journal intent
  *before* network writes and restore/reconcile before enabling writes after
  restart. Client IDs alone do not guarantee venue-wide idempotency (notably
  terminal orders and Bitfinex's daily scope).
- Nonce signing/transmission is serialized per client. Bitfinex uses increasing
  microsecond-scale nonces; MAX uses increasing millisecond nonces and refuses
  >5s drift. **Use one client/process per API key**; separate clients/processes do
  not share nonce state. Clock rollback/server skew can still require recovery.
- Cancel batches validate all inputs first, then send single-order requests
  sequentially. `Ok(())` means acknowledgments, not observed terminal status.
  Failures may occur after earlier successes; batches are not atomic. Lookup and
  reconcile all affected orders, including fills racing cancellation.

## Deliberate fail-closed limitations

- Private snapshots do not coerce market, conditional, margin, unknown status or
  inconsistent orders into ordinary limits. A single unsupported order can fail
  the whole open-order snapshot.
- Paginated/truncated snapshots are **not** returned as complete. Coinbase's
  `CB-AFTER`, Bybit cursors, KuCoin page counts and saturated page lengths fail
  explicitly; automatic pagination is not yet implemented.
- Bybit unified `walletBalance` and `availableToWithdraw` do **not** unambiguously
  represent spendable spot cash. Missing `free`, borrow exposure or positions
  fail instead of guessing `walletBalance - locked`. Ordinary modern UNIFIED
  responses therefore commonly return `Unsupported` for `query_balances`; order
  APIs remain available, but a live risk engine needs a richer account adapter.
- Non-cash liabilities in OKX are unsupported. Null Bitfinex available balances
  are unknown, not replaced by total balances. Duplicate currencies, negative
  cash, unsupported precision and arithmetic overflow fail.
- No authenticated user WebSocket, trade/fill history, local ledger mutation,
  fee reconciliation, SDK delegation, retry, or durable execution coordinator is
  provided. Applications must not treat REST acknowledgment as fill accounting.

## Transport and verification

Fixed HTTPS origins, credential/provider binding, redirects and reqwest retries
disabled, 5s connect/15s total deadlines, incremental 8 MiB response bounds.
Errors never include signed URLs, private response bodies, keys or signatures.
Injectable transports are trusted secret-handling code and must preserve the same
no-retry, no-log, bounded-response and pagination-header contracts.

Offline tests cover all nine providers, read-only/write gates, filters/caps,
lookups, partial executions, malformed/mismatched responses, cancellation
validation, nonce serialization, ambiguous and dropped writes, credential/origin
binding, redaction, pagination and cash-account ambiguity. **27 signing vectors**
come from independent Python encoding and Go standard-library HMAC/Ed25519 crypto:

```bash
python3 tools/generate-private-signing-fixtures.py
cargo test -p bbgo-provider --all-targets --locked
cargo test -p bbgo-provider --all-targets --locked --features dnum
```

Fixtures use synthetic credentials and responses, not recorded private traffic.
No live credentials were loaded or private exchange requests/orders made.
Endpoint availability, account versions/regions and production behavior still
require operator-controlled acceptance testing. This is not full Go API parity
or authorization to enable an unattended live strategy.
