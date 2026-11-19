//! Authenticated spot REST. Writes require explicit opt-in; no automatic retries.
mod auth;
mod decode;
mod requests;
mod transport;

pub use transport::{HttpTransport, Method, PrivateTransport, Response, SignedRequest};

use crate::{ProviderId, public};
use async_trait::async_trait;
use bbgo_types::{
    AccountService, Balances, Error, Market, Order, OrderQueryService, OrderType, Result,
    SubmitOrder, TradeService, Value,
};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

/// Never serialized, cloned, or printed. Pass credentials explicitly, not in URLs.
pub struct Credentials {
    key: Zeroizing<String>,
    secret: Zeroizing<String>,
    passphrase: Zeroizing<String>,
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}
impl Credentials {
    /// Coinbase: base64 secret; Backpack: base64 32-byte Ed25519 seed and public key.
    /// Other providers use the literal HMAC secret. Passphrase is required for
    /// Bitget, Coinbase Exchange, KuCoin (key version 2), and OKX.
    pub fn new(key: String, secret: String, passphrase: String) -> Self {
        Self {
            key: Zeroizing::new(key),
            secret: Zeroizing::new(secret),
            passphrase: Zeroizing::new(passphrase),
        }
    }
    fn validate(&self, provider: ProviderId) -> Result<()> {
        for value in [&self.key, &self.secret, &self.passphrase] {
            if value.len() > 4096 || value.bytes().any(|b| b.is_ascii_control()) {
                return Err(public::invalid("invalid credential encoding"));
            }
        }
        if self.key.is_empty() || self.secret.is_empty() {
            return Err(public::invalid("API key and secret required"));
        }
        if matches!(
            provider,
            ProviderId::Bitget | ProviderId::Coinbase | ProviderId::Kucoin | ProviderId::Okex
        ) && self.passphrase.is_empty()
        {
            return Err(public::invalid("API passphrase required"));
        }
        auth::validate_key(provider, self)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum WritePolicy {
    #[default]
    ReadOnly,
    /// Per-order cap, NOT a portfolio risk engine, reservation, or live acceptance gate.
    Enabled { max_order_notional: Value },
}

/// Inject a trusted clock for tests. Production clocks must be synchronized.
pub trait Clock: Send + Sync {
    fn unix_millis(&self) -> Result<i64>;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn unix_millis(&self) -> Result<i64> {
        public::now()
    }
}

struct Attempt {
    request: SubmitOrder,
    result: Option<Order>,
}
pub struct PrivateClient<T> {
    provider: ProviderId,
    credentials: Credentials,
    markets: BTreeMap<String, Market>,
    policy: WritePolicy,
    transport: T,
    clock: Arc<dyn Clock>,
    // Keep nonce signing and transmission in order on this client.
    send_lock: tokio::sync::Mutex<()>,
    last_nonce: Mutex<i64>,
    attempts: Mutex<BTreeMap<String, Attempt>>,
}
impl<T: PrivateTransport> PrivateClient<T> {
    /// Execution filters must be trusted, current, and complete. Discovery-only
    /// SpotInstrument values with missing filters cannot authorize an order.
    pub fn new(
        provider: ProviderId,
        credentials: Credentials,
        markets: BTreeMap<String, Market>,
        policy: WritePolicy,
        transport: T,
    ) -> Result<Self> {
        Self::with_clock(
            provider,
            credentials,
            markets,
            policy,
            transport,
            Arc::new(SystemClock),
        )
    }
    pub fn with_clock(
        provider: ProviderId,
        credentials: Credentials,
        markets: BTreeMap<String, Market>,
        policy: WritePolicy,
        transport: T,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        credentials.validate(provider)?;
        if let WritePolicy::Enabled { max_order_notional } = policy
            && (max_order_notional <= Value::ZERO || markets.is_empty())
        {
            return Err(public::invalid(
                "writes need a positive cap and execution markets",
            ));
        }
        if markets.iter().any(|(key, market)| key != &market.symbol) {
            return Err(public::invalid("market identity mismatch"));
        }
        Ok(Self {
            provider,
            credentials,
            markets,
            policy,
            transport,
            clock,
            send_lock: tokio::sync::Mutex::new(()),
            last_nonce: Mutex::new(0),
            attempts: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn provider(&self) -> ProviderId {
        self.provider
    }
    fn writes(&self) -> Result<Value> {
        match self.policy {
            WritePolicy::ReadOnly => Err(Error::Unsupported("private writes disabled".into())),
            WritePolicy::Enabled { max_order_notional } => Ok(max_order_notional),
        }
    }
    fn validate_order(&self, order: &SubmitOrder) -> Result<()> {
        let cap = self.writes()?;
        public::symbol(&order.symbol)?;
        requests::client_id(self.provider, &order.client_order_id)?;
        if order.order_type != OrderType::Limit {
            return Err(Error::Unsupported(
                "only spot GTC limit orders are supported".into(),
            ));
        }
        let market = self
            .markets
            .get(&order.symbol)
            .ok_or_else(|| public::invalid("unknown execution market"))?;
        if market.normalize_limit(order)? != *order {
            return Err(public::invalid(
                "order is not exactly aligned to execution filters",
            ));
        }
        if public::number(&serde_json::Value::String(order.price.to_string()))? != order.price
            || order.price.checked_mul_ceil(order.quantity)? > cap
        {
            return Err(public::invalid("order exceeds notional cap"));
        }
        Ok(())
    }
    async fn send(&self, request: requests::Request) -> Result<Response> {
        let _guard = self.send_lock.lock().await;
        let now = self.clock.unix_millis()?;
        if now <= 0 {
            return Err(public::invalid("private request needs a positive clock"));
        }
        let nonce = {
            let mut last = self
                .last_nonce
                .lock()
                .map_err(|_| public::invalid("nonce poisoned"))?;
            let wall = if self.provider == ProviderId::Bitfinex {
                now.checked_mul(1000)
                    .ok_or_else(|| public::invalid("nonce overflow"))?
            } else {
                now
            };
            let next = wall.max(
                last.checked_add(1)
                    .ok_or_else(|| public::invalid("nonce overflow"))?,
            );
            if self.provider == ProviderId::Max && next - now > 5000 {
                return Err(public::invalid("nonce drift requires clock recovery"));
            }
            *last = next;
            next
        };
        let signed = auth::sign(self.provider, &self.credentials, request, now, nonce)?;
        let write = signed.is_write();
        let response = self
            .transport
            .execute(self.provider, signed)
            .await
            .map_err(|_| {
                Error::Adapter(if write {
                    "private write outcome unknown; reconcile by client/order ID before retrying"
                        .into()
                } else {
                    "private transport failed".into()
                })
            })?;
        if response.body.len() > public::MAX_BODY {
            return Err(Error::Adapter(
                "private response too large; reconcile any write".into(),
            ));
        }
        if !(200..300).contains(&response.status) {
            // Never include server bodies, keys, signed URLs, or response headers.
            return Err(Error::Adapter(format!(
                "private HTTP {}; reconcile any attempted write",
                response.status
            )));
        }
        Ok(response)
    }
}
#[async_trait]
impl<T: PrivateTransport> AccountService for PrivateClient<T> {
    async fn query_balances(&self) -> Result<Balances> {
        let response = self.send(requests::balances(self.provider)).await?;
        decode::balances(self.provider, &response.body)
    }
}
#[async_trait]
impl<T: PrivateTransport> TradeService for PrivateClient<T> {
    async fn submit_order(&self, order: SubmitOrder) -> Result<Order> {
        self.validate_order(&order)?;
        {
            let mut attempts = self
                .attempts
                .lock()
                .map_err(|_| public::invalid("attempts poisoned"))?;
            if let Some(attempt) = attempts.get(&order.client_order_id) {
                if attempt.request != order {
                    return Err(public::invalid("client ID reused with a different intent"));
                }
                return attempt.result.clone().ok_or_else(|| {
                    Error::Adapter(
                        "submission already attempted; reconcile its outcome, do not resubmit"
                            .into(),
                    )
                });
            }
            // Insert BEFORE awaiting: a dropped future must not permit duplicate writes.
            attempts.insert(
                order.client_order_id.clone(),
                Attempt {
                    request: order.clone(),
                    result: None,
                },
            );
        }
        let request = requests::submit(self.provider, &order)?;
        let response = self.send(request).await?;
        let observed = decode::submitted(self.provider, &response.body, &order)?;
        self.attempts
            .lock()
            .map_err(|_| public::invalid("attempts poisoned"))?
            .get_mut(&order.client_order_id)
            .ok_or_else(|| public::invalid("missing attempt"))?
            .result = Some(observed.clone());
        Ok(observed)
    }
    async fn query_open_orders(&self, symbol: &str) -> Result<Vec<Order>> {
        public::symbol(symbol)?;
        let response = self.send(requests::open(self.provider, symbol)).await?;
        if response.has_more {
            return Err(Error::Unsupported(
                "open orders require pagination; refusing a partial snapshot".into(),
            ));
        }
        decode::open(self.provider, &response.body, symbol)
    }
    /// Sequential single-order cancel acknowledgments, NOT confirmed cancellations.
    /// A failure may follow earlier successes; reconcile the whole batch.
    async fn cancel_orders(&self, orders: &[Order]) -> Result<()> {
        self.writes()?;
        if orders.len() > 1000 {
            return Err(public::invalid("cancel batch too large"));
        }
        // Validate the ENTIRE batch before any side effects.
        let requests = orders
            .iter()
            .map(|order| requests::cancel(self.provider, order))
            .collect::<Result<Vec<_>>>()?;
        for (order, request) in orders.iter().zip(requests) {
            let response = self.send(request).await?;
            decode::cancelled(self.provider, &response.body, order)?;
        }
        Ok(())
    }
}
#[async_trait]
impl<T: PrivateTransport> OrderQueryService for PrivateClient<T> {
    async fn query_order(&self, symbol: &str, order_id: &str) -> Result<Order> {
        let request = requests::lookup(self.provider, symbol, order_id, false)?;
        let mut response = self.send(request).await?;
        if self.provider == ProviderId::Bitfinex && decode::empty_orders(&response.body)? {
            response = self
                .send(requests::bitfinex_history(symbol, order_id)?)
                .await?;
        }
        decode::lookup(self.provider, &response.body, symbol, order_id, false)
    }
    async fn query_order_by_client_id(&self, symbol: &str, client_order_id: &str) -> Result<Order> {
        let request = requests::lookup(self.provider, symbol, client_order_id, true)?;
        let response = self.send(request).await?;
        decode::lookup(self.provider, &response.body, symbol, client_order_id, true)
    }
}

/// Fixed-origin HTTPS factory. Constructing a client does not contact a venue.
pub fn private_client(
    provider: ProviderId,
    credentials: Credentials,
    markets: BTreeMap<String, Market>,
    policy: WritePolicy,
) -> Result<PrivateClient<HttpTransport>> {
    PrivateClient::new(
        provider,
        credentials,
        markets,
        policy,
        HttpTransport::new()?,
    )
}
