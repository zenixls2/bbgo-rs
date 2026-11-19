//! Exchange providers, organized as provider-specific submodules.
//!
//! Providers implement shared service traits from `bbgo-types`; transport and
//! wire-format details stay inside each provider's module.
pub mod backpack;
pub mod binance;
pub mod bitfinex;
pub mod bitget;
pub mod bybit;
pub mod coinbase;
pub mod kucoin;
pub mod max;
pub mod okex;
/// OKX's current brand name; `okex` preserves BBGO's provider identifier.
pub use okex as okx;
pub mod private;
pub use private::private_client;
pub mod public;

/// Registered exchange identifiers, matching the original BBGO exchange factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Backpack,
    Binance,
    Bitfinex,
    Bitget,
    Bybit,
    Coinbase,
    Kucoin,
    Max,
    #[serde(alias = "okx")]
    Okex,
}
impl ProviderId {
    pub const ALL: [Self; 9] = [
        Self::Backpack,
        Self::Binance,
        Self::Bitfinex,
        Self::Bitget,
        Self::Bybit,
        Self::Coinbase,
        Self::Kucoin,
        Self::Max,
        Self::Okex,
    ];
}

/// Read-only public client factory. Never accepts credentials or enables trading.
pub fn public_client(
    provider: ProviderId,
) -> bbgo_types::Result<Box<dyn public::SpotMarketDataService>> {
    macro_rules! client {
        ($module:ident) => {
            Box::new($module::PublicClient::new($module::HttpTransport::new()?))
        };
    }
    Ok(match provider {
        ProviderId::Backpack => client!(backpack),
        ProviderId::Binance => client!(binance),
        ProviderId::Bitfinex => client!(bitfinex),
        ProviderId::Bitget => client!(bitget),
        ProviderId::Bybit => client!(bybit),
        ProviderId::Coinbase => client!(coinbase),
        ProviderId::Kucoin => client!(kucoin),
        ProviderId::Max => client!(max),
        ProviderId::Okex => client!(okex),
    })
}
