use crate::{ProviderId, public};
use async_trait::async_trait;
use bbgo_types::{Error, Result};
use std::{collections::BTreeMap, fmt, time::Duration};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}
impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}
/// Sensitive signed request. Debug is redacted; never log getters in a transport.
/// Only the internal signer can construct one.
pub struct SignedRequest {
    pub(super) provider: ProviderId,
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) query: Zeroizing<String>,
    pub(super) headers: BTreeMap<String, Zeroizing<String>>,
    pub(super) body: Vec<u8>,
    pub(super) write: bool,
}
impl fmt::Debug for SignedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SignedRequest([REDACTED])")
    }
}
impl SignedRequest {
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
    pub fn is_write(&self) -> bool {
        self.write
    }
    pub fn provider(&self) -> ProviderId {
        self.provider
    }
}
/// `has_more` must preserve pagination headers (Coinbase CB-AFTER). No Debug body.
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub has_more: bool,
}
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Response([REDACTED])")
    }
}
#[async_trait]
pub trait PrivateTransport: Send + Sync {
    /// Transmit exactly once, with no redirects/retries/logging. Return an error
    /// on transport uncertainty; never translate it into a successful empty body.
    async fn execute(&self, provider: ProviderId, request: SignedRequest) -> Result<Response>;
}
pub struct HttpTransport {
    client: reqwest::Client,
}
impl HttpTransport {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("bbgo-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| Error::Adapter("private HTTP initialization failed".into()))?;
        Ok(Self { client })
    }
}
pub(super) fn origin(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Binance => "https://api.binance.com",
        ProviderId::Backpack => "https://api.backpack.exchange",
        ProviderId::Bitfinex => "https://api.bitfinex.com",
        ProviderId::Bitget => "https://api.bitget.com",
        ProviderId::Bybit => "https://api.bybit.com",
        ProviderId::Coinbase => "https://api.exchange.coinbase.com",
        ProviderId::Kucoin => "https://api.kucoin.com",
        ProviderId::Max => "https://max-api.maicoin.com",
        ProviderId::Okex => "https://www.okx.com",
    }
}
#[async_trait]
impl PrivateTransport for HttpTransport {
    async fn execute(&self, provider: ProviderId, request: SignedRequest) -> Result<Response> {
        if provider != request.provider {
            return Err(public::invalid("private credential/origin mismatch"));
        }
        let mut url = format!("{}{}", origin(provider), request.path);
        if !request.query.is_empty() {
            url.push('?');
            url.push_str(&request.query);
        }
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut builder = self.client.request(method, &url);
        for (name, value) in request.headers() {
            let mut header = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| public::invalid("invalid private header"))?;
            header.set_sensitive(true);
            builder = builder.header(name, header);
        }
        // Never use reqwest's error display: it can contain signed URLs.
        let mut response = builder
            .body(request.body)
            .send()
            .await
            .map_err(|_| Error::Adapter("private HTTP request failed".into()))?;
        let status = response.status().as_u16();
        let has_more = response
            .headers()
            .get("cb-after")
            .is_some_and(|v| !v.is_empty());
        if response
            .content_length()
            .is_some_and(|n| n > public::MAX_BODY as u64)
        {
            return Err(public::invalid("private response too large"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Adapter("private HTTP body failed".into()))?
        {
            if chunk.len() > public::MAX_BODY - body.len() {
                return Err(public::invalid("private response too large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            body,
            has_more,
        })
    }
}
