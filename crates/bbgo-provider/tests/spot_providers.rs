//! Synthetic wire fixtures, not recorded traffic or venue acceptance tests.
use async_trait::async_trait;
use bbgo_provider::{
    backpack, bitfinex, bitget, bybit, coinbase, kucoin, max, okex,
    public::{self, PublicClient, PublicTransport, Specification},
};
use bbgo_types::{Error, MarketDataService, Result, Value};
use std::sync::{Arc, Mutex};

const BACKPACK_MARKETS: &[u8] = br#"[{"symbol":"BTC_USDC","baseSymbol":"BTC","quoteSymbol":"USDC","marketType":"SPOT","orderBookState":"Open","filters":{"price":{"tickSize":"0.01"},"quantity":{"stepSize":"0.001","minQuantity":"0.001","maxQuantity":"100"}}},{"marketType":"PERP"}]"#;
const BACKPACK_QUOTE: &[u8] = br#"{"timestamp":"1700000000000123","lastUpdateId":"123","bids":[["98","2"],["99","3"]],"asks":[["101","4"],["102","5"]]}"#;
const BITFINEX_MARKETS: &[u8] =
    br#"[[["BTCUSD",[null,null,null,"0.001","100"]],["AAVE:UST",[null,null,null,"0.01","1000"]]]]"#;
const BITFINEX_QUOTE: &[u8] = br#"[[99,1,3],[101,2,-4]]"#;
const BITGET_MARKETS: &[u8] = br#"{"code":"00000","data":[{"symbol":"BTCUSDT","baseCoin":"BTC","quoteCoin":"USDT","status":"online","pricePrecision":"2","quantityPrecision":"3","minTradeAmount":"0.001","maxTradeAmount":"100","minTradeUSDT":"5"}]}"#;
const BITGET_QUOTE: &[u8] = br#"{"code":"00000","data":[{"symbol":"BTCUSDT","bidPr":"99","bidSz":"3","askPr":"101","askSz":"4","ts":"1700000000000"}]}"#;
const BYBIT_MARKETS: &[u8] = br#"{"retCode":0,"result":{"category":"spot","list":[{"symbol":"BTCUSDT","baseCoin":"BTC","quoteCoin":"USDT","status":"Trading","priceFilter":{"tickSize":"0.01"},"lotSizeFilter":{"basePrecision":"0.001","minOrderQty":"0.001","maxOrderQty":"100","minOrderAmt":"5"}}]}}"#;
const BYBIT_QUOTE: &[u8] = br#"{"retCode":0,"time":1700000000000,"result":{"category":"spot","list":[{"symbol":"BTCUSDT","bid1Price":"99","bid1Size":"3","ask1Price":"101","ask1Size":"4"}]}}"#;
const COINBASE_MARKETS: &[u8] = br#"[{"id":"BTC-USD","base_currency":"BTC","quote_currency":"USD","status":"online","trading_disabled":false,"cancel_only":false,"auction_mode":false,"quote_increment":"0.01","base_increment":"0.001","base_min_size":"0.001","base_max_size":"100","min_market_funds":"5"}]"#;
const COINBASE_QUOTE: &[u8] = br#"{"sequence":123,"bids":[["99","3",2]],"asks":[["101","4",3]]}"#;
const KUCOIN_MARKETS: &[u8] = br#"{"code":"200000","data":[{"symbol":"BTC-USDT","baseCurrency":"BTC","quoteCurrency":"USDT","enableTrading":true,"priceIncrement":"0.01","baseIncrement":"0.001","baseMinSize":"0.001","baseMaxSize":"100","minFunds":"5"}]}"#;
const KUCOIN_QUOTE: &[u8] = br#"{"code":"200000","data":{"bestBid":"99","bestBidSize":"3","bestAsk":"101","bestAskSize":"4","time":1700000000000}}"#;
const MAX_MARKETS: &[u8] = br#"[{"id":"btctwd","base_unit":"btc","quote_unit":"twd","market_status":"active","base_unit_precision":3,"quote_unit_precision":2,"min_base_amount":"0.001","min_quote_amount":"5"}]"#;
const MAX_QUOTE: &[u8] = br#"{"market":"btctwd","at":1700000000000,"buy":"99","buy_vol":"3","sell":"101","sell_vol":"4"}"#;
const OKX_MARKETS: &[u8] = br#"{"code":"0","data":[{"instType":"SPOT","instId":"BTC-USDT","baseCcy":"BTC","quoteCcy":"USDT","state":"live","tickSz":"0.01","lotSz":"0.001","minSz":"0.001","maxLmtSz":"100"},{"instType":"SWAP"}]}"#;
const OKX_QUOTE: &[u8] = br#"{"code":"0","data":[{"instType":"SPOT","instId":"BTC-USDT","bidPx":"99","bidSz":"3","askPx":"101","askSz":"4","ts":"1700000000000"}]}"#;

type Calls = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;
struct Mock<S> {
    markets: &'static [u8],
    quote: &'static [u8],
    calls: Calls,
    marker: std::marker::PhantomData<S>,
}
#[async_trait]
impl<S: Specification> PublicTransport for Mock<S> {
    async fn get(&self, path: &str, parameters: &[(&str, String)]) -> Result<Vec<u8>> {
        assert!(S::allowed_path(path));
        self.calls.lock().unwrap().push((
            path.into(),
            parameters
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        ));
        Ok(if path == S::markets_request().0 {
            self.markets
        } else {
            self.quote
        }
        .to_vec())
    }
}
async fn contract<S: Specification>(
    symbol: &str,
    markets: &'static [u8],
    body: &'static [u8],
    exchange_time: bool,
    complete: bool,
) {
    let calls = Calls::default();
    let client = PublicClient::<S, _>::new(Mock::<S> {
        markets,
        quote: body,
        calls: calls.clone(),
        marker: std::marker::PhantomData,
    });
    let instruments = client.query_instruments().await.unwrap();
    let instrument = &instruments[symbol];
    assert!(instrument.trading);
    assert_eq!(instrument.to_market().is_ok(), complete);
    assert_eq!(client.query_markets().await.is_ok(), complete);
    let quote = S::decode_book(symbol, body, 1700000000100, 7).unwrap();
    assert_eq!(quote.symbol, symbol);
    assert_eq!(quote.bid.to_string(), "99");
    assert_eq!(quote.bid_quantity.to_string(), "3");
    assert_eq!(quote.ask.to_string(), "101");
    assert_eq!(quote.ask_quantity.to_string(), "4");
    assert_eq!(quote.exchange_time, exchange_time.then_some(1700000000000));
    assert_eq!(quote.receive_time, 1700000000100);
    assert_eq!(quote.generation, 7);
    let live_quote = client.query_book_ticker(symbol).await.unwrap();
    assert!(live_quote.receive_time > 0);
    assert_eq!(live_quote.exchange_time.is_some(), exchange_time);
    assert_eq!(client.query_ticker(symbol).await.is_ok(), exchange_time);
    let expected = S::quote_request(symbol).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls[2].0, expected.0);
    assert_eq!(
        calls[2].1,
        expected
            .1
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect::<Vec<_>>()
    );
    assert!(S::quote_request("../account?secret=x").is_err());
    assert!(!S::allowed_path("/private/orders"));
    assert!(!S::allowed_path("//evil.example"));
    assert!(S::decode_book(symbol, body, -1, 0).is_err());
    assert!(S::decode_book(symbol, br#"{"error":"unavailable"}"#, 1, 0).is_err());
    assert!(S::decode_instruments(br#"{"error":"unavailable"}"#).is_err());
}
macro_rules! contract_test {
    ($name:ident, $provider:ident, $symbol:literal, $markets:ident, $quote:ident, $time:literal, $complete:literal) => {
        #[tokio::test]
        async fn $name() {
            contract::<$provider::Specification>($symbol, $markets, $quote, $time, $complete).await;
        }
    };
}
contract_test!(
    backpack_public,
    backpack,
    "BTC_USDC",
    BACKPACK_MARKETS,
    BACKPACK_QUOTE,
    true,
    false
);
contract_test!(
    bitfinex_public,
    bitfinex,
    "BTCUSD",
    BITFINEX_MARKETS,
    BITFINEX_QUOTE,
    false,
    false
);
contract_test!(
    bitget_public,
    bitget,
    "BTCUSDT",
    BITGET_MARKETS,
    BITGET_QUOTE,
    true,
    true
);
contract_test!(
    bybit_public,
    bybit,
    "BTCUSDT",
    BYBIT_MARKETS,
    BYBIT_QUOTE,
    false,
    true
);
contract_test!(
    coinbase_public,
    coinbase,
    "BTC-USD",
    COINBASE_MARKETS,
    COINBASE_QUOTE,
    false,
    true
);
contract_test!(
    kucoin_public,
    kucoin,
    "BTC-USDT",
    KUCOIN_MARKETS,
    KUCOIN_QUOTE,
    true,
    true
);
contract_test!(
    max_public,
    max,
    "btctwd",
    MAX_MARKETS,
    MAX_QUOTE,
    true,
    false
);
contract_test!(
    okx_public,
    okex,
    "BTC-USDT",
    OKX_MARKETS,
    OKX_QUOTE,
    true,
    false
);

#[test]
fn filters_status_precision_and_incomplete_projection() {
    let mut kucoin: serde_json::Value = serde_json::from_slice(KUCOIN_MARKETS).unwrap();
    kucoin["data"][0]["enableTrading"] = false.into();
    let decoded = kucoin::decode_instruments(&serde_json::to_vec(&kucoin).unwrap()).unwrap();
    assert!(!decoded["BTC-USDT"].trading);
    let mut instrument = decoded["BTC-USDT"].clone();
    instrument.max_quantity = None;
    assert!(matches!(instrument.to_market(), Err(Error::Unsupported(_))));
    instrument.tick_size = Some(Value::ZERO);
    assert!(instrument.validate().is_err());
    kucoin["data"][0]["priceIncrement"] = "0.000000000000000012345678901234567".into();
    assert!(kucoin::decode_instruments(&serde_json::to_vec(&kucoin).unwrap()).is_err());
    kucoin = serde_json::from_slice(KUCOIN_MARKETS).unwrap();
    let row = kucoin["data"][0].clone();
    kucoin["data"].as_array_mut().unwrap().push(row);
    assert!(kucoin::decode_instruments(&serde_json::to_vec(&kucoin).unwrap()).is_err());
    let mut bybit: serde_json::Value = serde_json::from_slice(BYBIT_MARKETS).unwrap();
    bybit["result"]["nextPageCursor"] = "page2".into();
    assert!(bybit::decode_instruments(&serde_json::to_vec(&bybit).unwrap()).is_err());
    let mut coinbase: serde_json::Value = serde_json::from_slice(COINBASE_MARKETS).unwrap();
    coinbase[0]["cancel_only"] = true.into();
    assert!(
        !coinbase::decode_instruments(&serde_json::to_vec(&coinbase).unwrap()).unwrap()["BTC-USD"]
            .trading
    );
    let bfx = bitfinex::decode_instruments(BITFINEX_MARKETS).unwrap();
    assert_eq!(bfx["AAVE:UST"].quote_currency, "UST"); // no implicit USDT alias
}

#[test]
fn malformed_quotes_and_envelopes_fail_closed() {
    let mut b: serde_json::Value = serde_json::from_slice(BITGET_QUOTE).unwrap();
    for (field, bad) in [
        ("symbol", "ETHUSDT"),
        ("bidPr", "102"),
        ("bidSz", "-1"),
        ("bidSz", "3%"),
        ("ts", "0"),
        ("askPr", "99.000000000000001"),
    ] {
        b["data"][0][field] = bad.into();
        assert!(
            bitget::decode_book_ticker("BTCUSDT", &serde_json::to_vec(&b).unwrap(), 10, 0).is_err()
        );
        b = serde_json::from_slice(BITGET_QUOTE).unwrap();
    }
    b["code"] = "40001".into();
    assert!(
        bitget::decode_book_ticker("BTCUSDT", &serde_json::to_vec(&b).unwrap(), 10, 0).is_err()
    );
    assert!(bitfinex::decode_book_ticker("BTCUSD", br#"[[99,0,3],[101,2,-4]]"#, 1, 0).is_err());
    assert!(coinbase::decode_book_ticker("BTC-USD", br#"{"bids":[],"asks":[]}"#, 1, 0).is_err());
    assert!(
        backpack::decode_book_ticker(
            "BTC_USDC",
            br#"{"timestamp":"999","bids":[["99","3"]],"asks":[["101","4"]]}"#,
            1,
            0
        )
        .is_err()
    );
    assert!(max::decode_book_ticker("different", MAX_QUOTE, 1, 0).is_err());
    assert!(!coinbase::Specification::allowed_path("/products/../book"));
    assert!(!bitfinex::Specification::allowed_path(
        "/v2/book/tBTCUSD/../../orders/P0"
    ));
    assert!(kucoin::decode_instruments(&vec![b' '; public::MAX_BODY + 1]).is_err());
}

#[test]
fn numeric_json_is_exact_not_f64_and_brand_alias_works() {
    let body = br#"[[99.00000001,1,3],[101.00000001,2,-4]]"#;
    let quote = bitfinex::decode_book_ticker("BTCUSD", body, 1, 0).unwrap();
    assert_eq!(quote.bid.to_string(), "99.00000001");
    let too_precise = br#"[[99.000000000000001,1,3],[101,2,-4]]"#;
    assert!(bitfinex::decode_book_ticker("BTCUSD", too_precise, 1, 0).is_err());
    let scientific =
        bitfinex::decode_book_ticker("BTCUSD", br#"[[99,1,1e-8],[101,2,-4e-8]]"#, 1, 0).unwrap();
    assert_eq!(scientific.bid_quantity.to_string(), "0.00000001");
    assert_eq!(scientific.ask_quantity.to_string(), "0.00000004");
    assert_eq!(
        bbgo_provider::okx::decode_instruments(OKX_MARKETS).unwrap(),
        okex::decode_instruments(OKX_MARKETS).unwrap()
    );
}

#[test]
fn registered_provider_factory_is_complete_and_read_only() {
    for provider in bbgo_provider::ProviderId::ALL {
        // Constructs bounded transports only: no DNS, network or credentials.
        bbgo_provider::public_client(provider).unwrap();
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(
            serde_json::from_str::<bbgo_provider::ProviderId>(&json).unwrap(),
            provider
        );
    }
    assert_eq!(
        serde_json::from_str::<bbgo_provider::ProviderId>("\"okx\"").unwrap(),
        bbgo_provider::ProviderId::Okex
    );
}
