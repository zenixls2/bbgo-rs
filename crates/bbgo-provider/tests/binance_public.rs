//! Synthetic Binance wire fixtures; these are not claimed to be captured traffic.
use async_trait::async_trait;
use bbgo_provider::binance::{
    PublicClient, PublicTransport, decode_depth, decode_markets, decode_snapshot,
};
use bbgo_types::{MarketDataService, Result, Value};

const MARKETS: &[u8] = br#"{"symbols":[{
 "symbol":"BTCJPY","status":"TRADING","baseAsset":"BTC","quoteAsset":"JPY",
 "isSpotTradingAllowed":true,"orderTypes":["LIMIT","MARKET","LIMIT_MAKER","STOP_LOSS"],
 "filters":[
 {"filterType":"PRICE_FILTER","minPrice":"1","maxPrice":"90000000","tickSize":"1.00000000"},
 {"filterType":"LOT_SIZE","minQty":"0.00010000","maxQty":"100","stepSize":"0.00010000"},
 {"filterType":"MIN_NOTIONAL","minNotional":"100"},
 {"filterType":"NOTIONAL","minNotional":"200","maxNotional":"1000000"}
 ]}]}"#;
const SNAPSHOT: &[u8] = br#"{"lastUpdateId":10,"bids":[["100","2"]],"asks":[["102","3"]]}"#;
const UPDATE: &[u8] =
    br#"{"e":"depthUpdate","E":1000,"s":"BTCJPY","U":10,"u":11,"b":[["101","1"]],"a":[]}"#;

struct FixtureTransport;
#[async_trait]
impl PublicTransport for FixtureTransport {
    async fn get(&self, path: &str, parameters: &[(&str, String)]) -> Result<Vec<u8>> {
        match path {
            "/api/v3/exchangeInfo" => {
                assert!(parameters.is_empty());
                Ok(MARKETS.to_vec())
            }
            "/api/v3/depth" => {
                assert_eq!(parameters[0], ("symbol", "BTCJPY".into()));
                assert!(matches!(parameters[1].1.as_str(), "5" | "100"));
                Ok(SNAPSHOT.to_vec())
            }
            _ => panic!("unexpected path"),
        }
    }
}
#[tokio::test]
async fn metadata_transport_and_untimed_quote_rejection() {
    let client = PublicClient::new(FixtureTransport);
    let markets = client.query_markets().await.unwrap();
    let market = &markets["BTCJPY"];
    assert_eq!(market.min_notional.to_string(), "200");
    assert_eq!(market.order_types.len(), 3);
    assert!(market.spot_enabled && market.trading);
    assert_eq!(
        client.depth("BTCJPY", 100).await.unwrap().sequence(),
        Some(10)
    );
    assert!(client.depth("../BTC", 100).await.is_err());
    assert!(client.depth("BTCJPY", 99).await.is_err());
    assert!(client.query_ticker("BTCJPY").await.is_err());
    let service: &dyn bbgo_provider::public::SpotMarketDataService = &client;
    let instruments = service.query_instruments().await.unwrap();
    assert_eq!(
        instruments["BTCJPY"].min_notional,
        Some(market.min_notional)
    );
    let quote = service.query_book_ticker("BTCJPY").await.unwrap();
    assert_eq!(quote.bid.to_string(), "100");
    assert_eq!(quote.ask.to_string(), "102");
    assert_eq!(quote.bid_quantity.to_string(), "2");
    assert_eq!(quote.exchange_time, None);
    assert!(quote.receive_time > 0);
}

#[test]
fn synchronized_depth_produces_causal_quotes_and_fails_on_gap() {
    let mut book = decode_snapshot("BTCJPY", SNAPSHOT).unwrap();
    let event = decode_depth(UPDATE, 1002, 4).unwrap();
    let quote = event.apply(&mut book).unwrap().unwrap();
    assert_eq!(quote.bid.to_string(), "101");
    assert_eq!(quote.ask.to_string(), "102");
    assert_eq!(
        (quote.exchange_time, quote.receive_time, quote.generation),
        (1000, 1002, 4)
    );
    assert!(event.apply(&mut book).unwrap().is_none());
    let gap = decode_depth(
        br#"{"e":"depthUpdate","E":1003,"s":"BTCJPY","U":13,"u":14,"b":[],"a":[]}"#,
        1005,
        4,
    )
    .unwrap();
    assert!(gap.apply(&mut book).is_err());
    assert!(book.best_bid().is_none());
    assert!(event.apply(&mut book).is_err());
}

#[test]
fn rejects_malformed_precision_crossed_books_and_metadata() {
    assert_eq!(
        decode_snapshot(
            "BTCJPY",
            br#"{"lastUpdateId":1,"bids":[["100","0.000000001"]],"asks":[]}"#
        )
        .is_ok(),
        Value::BACKEND == "dnum-16"
    );
    assert!(
        decode_snapshot(
            "BTCJPY",
            br#"{"lastUpdateId":1,"bids":[["103","1"]],"asks":[["102","1"]]}"#
        )
        .is_err()
    );
    assert!(
        decode_snapshot(
            "BTCJPY",
            br#"{"lastUpdateId":1,"bids":[["100","0.000000000000000012345678901234567"]],"asks":[]}"#
        )
        .is_err()
    );
    assert!(
        decode_depth(
            br#"{"e":"bookTicker","E":1,"s":"BTCJPY","U":1,"u":2,"b":[],"a":[]}"#,
            2,
            0
        )
        .is_err()
    );
    let bad = String::from_utf8(MARKETS.to_vec())
        .unwrap()
        .replace("\"tickSize\":\"1.00000000\"", "\"tickSize\":\"0\"");
    assert!(decode_markets(bad.as_bytes()).is_err());
    assert!(decode_markets(br#"{"symbols":[{"symbol":"BTCJPY"}]}"#).is_err());
}
