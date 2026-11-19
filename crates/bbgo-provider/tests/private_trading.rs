use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use bbgo_provider::{
    ProviderId,
    private::{
        Clock, Credentials, Method, PrivateClient, PrivateTransport, Response, SignedRequest,
        WritePolicy,
    },
};
use bbgo_types::{
    AccountService, Error, Market, Order, OrderQueryService, OrderStatus, OrderType, Result, Side,
    SubmitOrder, TradeService, Value,
};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn n(v: &str) -> Value {
    Value::parse_exact(v).unwrap()
}
fn native(p: ProviderId) -> &'static str {
    match p {
        ProviderId::Backpack => "BTC_USDC",
        ProviderId::Bitfinex => "BTCUSD",
        ProviderId::Coinbase => "BTC-USD",
        ProviderId::Kucoin | ProviderId::Okex => "BTC-USDT",
        ProviderId::Max => "btctwd",
        _ => "BTCUSDT",
    }
}
fn cid(p: ProviderId) -> &'static str {
    match p {
        ProviderId::Backpack | ProviderId::Bitfinex => "123",
        ProviderId::Coinbase => "00000000-0000-4000-8000-000000000001",
        _ => "test1",
    }
}
fn credentials(p: ProviderId) -> Credentials {
    let (key, secret) = if p == ProviderId::Backpack {
        (
            "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=".into(),
            BASE64.encode((0u8..32).collect::<Vec<_>>()),
        )
    } else if p == ProviderId::Coinbase {
        ("test-key".into(), BASE64.encode("test-secret"))
    } else {
        ("test-key".into(), "test-secret".into())
    };
    Credentials::new(key, secret, "test-passphrase".into())
}
fn intent(p: ProviderId) -> SubmitOrder {
    SubmitOrder {
        client_order_id: cid(p).into(),
        symbol: native(p).into(),
        side: Side::Buy,
        order_type: OrderType::Limit,
        price: n("100"),
        quantity: n("2"),
    }
}
fn order(p: ProviderId) -> Order {
    Order {
        id: "42".into(),
        request: intent(p),
        status: OrderStatus::New,
        executed_quantity: Value::ZERO,
    }
}
fn markets(p: ProviderId) -> BTreeMap<String, Market> {
    BTreeMap::from([(
        native(p).into(),
        Market {
            symbol: native(p).into(),
            base_currency: "BTC".into(),
            quote_currency: "USD".into(),
            trading: true,
            spot_enabled: true,
            order_types: vec![OrderType::Limit],
            tick_size: n("0.01"),
            step_size: n("0.01"),
            min_quantity: n("0.01"),
            max_quantity: n("100"),
            min_notional: n("1"),
        },
    )])
}
struct FixedClock;
impl Clock for FixedClock {
    fn unix_millis(&self) -> Result<i64> {
        Ok(1_700_000_000_123)
    }
}
fn client<T: PrivateTransport>(p: ProviderId, t: T, policy: WritePolicy) -> PrivateClient<T> {
    PrivateClient::with_clock(
        p,
        credentials(p),
        markets(p),
        policy,
        t,
        Arc::new(FixedClock),
    )
    .unwrap()
}
fn enabled() -> WritePolicy {
    WritePolicy::Enabled {
        max_order_notional: n("1000"),
    }
}
fn envelope(p: ProviderId, d: Json) -> Json {
    match p {
        ProviderId::Bitget => json!({"code":"00000","data":d}),
        ProviderId::Kucoin => json!({"code":"200000","data":d}),
        ProviderId::Bybit => json!({"retCode":0,"result":d}),
        ProviderId::Okex => json!({"code":"0","data":d}),
        _ => d,
    }
}
fn row(p: ProviderId, cancelled: bool) -> Json {
    let s = native(p);
    let c = cid(p);
    match p {
        ProviderId::Binance => {
            json!({"orderId":42,"symbol":s,"clientOrderId":c,"side":"BUY","type":"LIMIT","timeInForce":"GTC","price":"100","origQty":"2","executedQty":"0","status":if cancelled{"CANCELED"}else{"NEW"}})
        }
        ProviderId::Backpack => {
            json!({"id":"42","symbol":s,"clientId":123,"side":"Bid","orderType":"Limit","timeInForce":"GTC","price":"100","quantity":"2","executedQuantity":"0","status":if cancelled{"Cancelled"}else{"New"}})
        }
        ProviderId::Bitfinex => json!([
            42,
            null,
            123,
            format!("t{s}"),
            0,
            0,
            2,
            2,
            "EXCHANGE LIMIT",
            null,
            null,
            null,
            0,
            if cancelled { "CANCELED" } else { "ACTIVE" },
            null,
            null,
            100
        ]),
        ProviderId::Bitget => {
            json!({"orderId":"42","symbol":s,"clientOid":c,"side":"buy","orderType":"limit","priceAvg":"100","size":"2","baseVolume":"0","status":if cancelled{"cancelled"}else{"live"}})
        }
        ProviderId::Bybit => {
            json!({"orderId":"42","symbol":s,"orderLinkId":c,"side":"Buy","orderType":"Limit","price":"100","qty":"2","cumExecQty":"0","isLeverage":"0","orderStatus":if cancelled{"Cancelled"}else{"New"}})
        }
        ProviderId::Coinbase => {
            json!({"id":"42","product_id":s,"client_oid":c,"side":"buy","type":"limit","price":"100","size":"2","filled_size":"0","status":if cancelled{"done"}else{"open"},"done_reason":if cancelled{"canceled"}else{""}})
        }
        ProviderId::Kucoin => {
            json!({"id":"42","symbol":s,"clientOid":c,"side":"buy","type":"limit","price":"100","size":"2","dealSize":"0","isActive":!cancelled,"cancelExist":cancelled})
        }
        ProviderId::Max => {
            json!({"id":42,"market":s,"client_oid":c,"side":"buy","ord_type":"limit","price":"100","volume":"2","executed_volume":"0","state":if cancelled{"cancel"}else{"wait"}})
        }
        ProviderId::Okex => {
            json!({"ordId":"42","instId":s,"instType":"SPOT","tdMode":"cash","clOrdId":c,"side":"buy","ordType":"limit","px":"100","sz":"2","accFillSz":"0","state":if cancelled{"canceled"}else{"live"}})
        }
    }
}
fn balance_body(p: ProviderId) -> Json {
    envelope(
        p,
        match p {
            ProviderId::Binance => json!({"balances":[{"asset":"BTC","free":"3","locked":"1"}]}),
            ProviderId::Backpack => json!({"BTC":{"available":"3","locked":"0.5","staked":"0.5"}}),
            ProviderId::Bitfinex => {
                json!([["exchange", "BTC", 4, 0, 3], ["margin", "BTC", 10, 0, 10]])
            }
            ProviderId::Bitget => {
                json!([{"coin":"BTC","available":"3","frozen":"0.5","locked":"0.5"}])
            }
            // Deliberately synthetic: actual UNIFIED payloads without free must fail closed.
            ProviderId::Bybit => {
                json!({"list":[{"accountType":"UNIFIED","coin":[{"coin":"BTC","free":"3","locked":"1","walletBalance":"4","totalPositionIM":"0","spotBorrow":"0"}]}]})
            }
            ProviderId::Coinbase => json!([{"currency":"BTC","available":"3","hold":"1"}]),
            ProviderId::Kucoin => {
                json!([{"currency":"BTC","type":"trade","available":"3","holds":"1"}])
            }
            ProviderId::Max => json!([{"currency":"BTC","balance":"3","locked":"1"}]),
            ProviderId::Okex => {
                json!([{"details":[{"ccy":"BTC","availBal":"3","frozenBal":"1","liab":"0"}]}])
            }
        },
    )
}
fn notification(data: Json) -> Json {
    json!([1, "on-req", null, null, [data], null, "SUCCESS", "OK"])
}
fn submit_body(p: ProviderId) -> Json {
    envelope(
        p,
        match p {
            ProviderId::Bitget => json!({"orderId":"42","clientOid":cid(p)}),
            ProviderId::Bybit => json!({"orderId":"42","orderLinkId":cid(p)}),
            ProviderId::Kucoin => json!({"orderId":"42"}),
            ProviderId::Okex => json!([{"ordId":"42","clOrdId":cid(p),"sCode":"0"}]),
            ProviderId::Bitfinex => notification(row(p, false)),
            _ => row(p, false),
        },
    )
}
fn cancel_body(p: ProviderId) -> Json {
    envelope(
        p,
        match p {
            ProviderId::Coinbase => json!("42"),
            ProviderId::Kucoin => json!({"cancelledOrderIds":["42"]}),
            ProviderId::Bybit | ProviderId::Bitget => json!({"orderId":"42"}),
            ProviderId::Okex => json!([{"ordId":"42","sCode":"0"}]),
            ProviderId::Bitfinex => notification(row(p, true)),
            _ => row(p, true),
        },
    )
}
fn open_body(p: ProviderId) -> Json {
    let rows = json!([row(p, false)]);
    envelope(
        p,
        match p {
            ProviderId::Bybit => json!({"category":"spot","nextPageCursor":"","list":rows}),
            ProviderId::Kucoin => json!({"totalPage":1,"currentPage":1,"totalNum":1,"items":rows}),
            _ => rows,
        },
    )
}
fn lookup_body(p: ProviderId) -> Json {
    envelope(
        p,
        match p {
            ProviderId::Okex | ProviderId::Bitget | ProviderId::Bitfinex => json!([row(p, false)]),
            ProviderId::Bybit => json!({"category":"spot","list":[row(p,false)]}),
            _ => row(p, false),
        },
    )
}
#[derive(Deserialize)]
struct Vector {
    provider: ProviderId,
    operation: String,
    method: String,
    path: String,
    query: String,
    body: String,
    signature_header: String,
    signature: String,
}
struct Golden {
    vector: Vector,
    body: Json,
}
#[async_trait]
impl PrivateTransport for Golden {
    async fn execute(&self, p: ProviderId, r: SignedRequest) -> Result<Response> {
        assert_eq!(p, self.vector.provider);
        assert_eq!(r.method().as_str(), self.vector.method);
        assert_eq!(r.path(), self.vector.path);
        assert_eq!(r.query(), self.vector.query);
        assert_eq!(std::str::from_utf8(r.body()).unwrap(), self.vector.body);
        if self.vector.signature_header != "signature-query" {
            assert_eq!(
                r.headers()
                    .find(|(k, _)| *k == self.vector.signature_header)
                    .unwrap()
                    .1,
                self.vector.signature
            );
        }
        assert_eq!(format!("{r:?}"), "SignedRequest([REDACTED])");
        Ok(Response {
            status: 200,
            body: serde_json::to_vec(&self.body).unwrap(),
            has_more: false,
        })
    }
}
#[tokio::test]
async fn all_nine_signers_match_independent_go_vectors_and_trade_contracts() {
    let vectors: Vec<Vector> =
        serde_json::from_str(include_str!("fixtures/private-signing.json")).unwrap();
    assert_eq!(vectors.len(), 27);
    for vector in vectors {
        let p = vector.provider;
        let op = vector.operation.clone();
        let body = match op.as_str() {
            "balances" => balance_body(p),
            "submit" => submit_body(p),
            _ => cancel_body(p),
        };
        let c = client(p, Golden { vector, body }, enabled());
        match op.as_str() {
            "balances" => {
                let balances = c.query_balances().await.unwrap();
                assert_eq!(balances["BTC"].available, n("3"));
                assert_eq!(balances["BTC"].locked, n("1"));
            }
            "submit" => {
                let observed = c.submit_order(intent(p)).await.unwrap();
                assert_eq!(observed.id, "42");
                assert_eq!(observed.request, intent(p));
                let pending = matches!(
                    p,
                    ProviderId::Bitget | ProviderId::Bybit | ProviderId::Kucoin | ProviderId::Okex
                );
                assert_eq!(
                    observed.status,
                    if pending {
                        OrderStatus::Pending
                    } else {
                        OrderStatus::New
                    }
                );
                assert_eq!(c.submit_order(intent(p)).await.unwrap(), observed);
                let mut conflict = intent(p);
                conflict.quantity = n("3");
                assert!(c.submit_order(conflict).await.is_err());
            }
            _ => c.cancel_orders(&[order(p)]).await.unwrap(),
        }
    }
}
struct Mock {
    body: Json,
    calls: Arc<AtomicUsize>,
    fail: bool,
    has_more: bool,
}
#[async_trait]
impl PrivateTransport for Mock {
    async fn execute(&self, _: ProviderId, _: SignedRequest) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(Error::Adapter("THIS-SECRET-MUST-NOT-ESCAPE".into()));
        }
        Ok(Response {
            status: 200,
            body: serde_json::to_vec(&self.body).unwrap(),
            has_more: self.has_more,
        })
    }
}
fn mock(body: Json) -> Mock {
    Mock {
        body,
        calls: Arc::new(AtomicUsize::new(0)),
        fail: false,
        has_more: false,
    }
}
#[tokio::test]
async fn all_providers_open_and_order_lookups_validate_identity() {
    for p in ProviderId::ALL {
        let c = client(p, mock(open_body(p)), WritePolicy::ReadOnly);
        assert_eq!(
            c.query_open_orders(native(p)).await.unwrap(),
            vec![order(p)]
        );
        assert!(c.query_open_orders("UNEXPECTED").await.is_err());
        let c = client(p, mock(lookup_body(p)), WritePolicy::ReadOnly);
        assert_eq!(c.query_order(native(p), "42").await.unwrap(), order(p));
        assert!(c.query_order(native(p), "43").await.is_err());
        if p == ProviderId::Bitfinex {
            assert!(c.query_order_by_client_id(native(p), cid(p)).await.is_err());
        } else {
            assert_eq!(
                c.query_order_by_client_id(native(p), cid(p)).await.unwrap(),
                order(p)
            );
        }
    }
}
#[tokio::test]
async fn writes_opt_in_filters_caps_and_entire_cancel_batch_validate_before_io() {
    for p in ProviderId::ALL {
        let transport = mock(submit_body(p));
        let calls = transport.calls.clone();
        let c = client(p, transport, WritePolicy::ReadOnly);
        assert!(c.submit_order(intent(p)).await.is_err());
        assert!(c.cancel_orders(&[order(p)]).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let transport = mock(submit_body(p));
        let calls = transport.calls.clone();
        let c = client(p, transport, enabled());
        for change in 0..6 {
            let mut o = intent(p);
            match change {
                0 => o.price = n("100.001"),
                1 => o.quantity = n("0.001"),
                2 => o.quantity = n("11"),
                3 => o.order_type = OrderType::Market,
                4 => o.client_order_id = "../bad".into(),
                _ => o.symbol = "UNKNOWN".into(),
            }
            assert!(c.submit_order(o).await.is_err());
        }
        let mut bad = order(p);
        bad.id = "../42".into();
        assert!(c.cancel_orders(&[order(p), bad]).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn failed_or_cancelled_submission_is_never_blindly_retried() {
    let p = ProviderId::Binance;
    let mut transport = mock(Json::Null);
    transport.fail = true;
    let calls = transport.calls.clone();
    let c = client(p, transport, enabled());
    let err = c.submit_order(intent(p)).await.unwrap_err().to_string();
    assert!(err.contains("outcome unknown"));
    assert!(!err.contains("THIS-SECRET"));
    assert!(c.submit_order(intent(p)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    struct Hung {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl PrivateTransport for Hung {
        async fn execute(&self, _: ProviderId, _: SignedRequest) -> Result<Response> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let c = client(
        p,
        Hung {
            calls: calls.clone(),
        },
        enabled(),
    );
    let cancel = async {
        loop {
            if calls.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {biased; r=c.submit_order(intent(p))=>panic!("unexpected completion: {r:?}"), _=cancel=>{}}
    assert!(c.submit_order(intent(p)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn malformed_private_responses_fail_closed_and_are_not_cached_as_success() {
    for p in ProviderId::ALL {
        for bad in [
            json!({"error":"rejected"}),
            json!({"code":"failure"}),
            json!([]),
        ] {
            let c = client(p, mock(bad), enabled());
            assert!(c.submit_order(intent(p)).await.is_err());
            assert!(c.submit_order(intent(p)).await.is_err());
        }
        let mut wrong = row(p, false);
        if p == ProviderId::Bitfinex {
            wrong[0] = json!(43);
        } else {
            let key = match p {
                ProviderId::Binance | ProviderId::Bitget | ProviderId::Bybit => "orderId",
                ProviderId::Okex => "ordId",
                _ => "id",
            };
            wrong[key] = json!("43");
        }
        let body = envelope(
            p,
            match p {
                ProviderId::Bybit => json!({"category":"spot","list":[wrong]}),
                ProviderId::Bitget | ProviderId::Okex | ProviderId::Bitfinex => json!([wrong]),
                _ => wrong,
            },
        );
        let c = client(p, mock(body), WritePolicy::ReadOnly);
        assert!(c.query_order(native(p), "42").await.is_err());
    }
    let p = ProviderId::Okex;
    let c = client(
        p,
        mock(json!({"code":"0","data":[{"ordId":"42","sCode":"51000"}]})),
        enabled(),
    );
    assert!(c.submit_order(intent(p)).await.is_err());
    assert!(c.cancel_orders(&[order(p)]).await.is_err());
}
#[tokio::test]
async fn balance_ambiguity_debt_duplicates_and_pagination_are_errors() {
    let p = ProviderId::Bybit;
    let body = envelope(
        p,
        json!({"list":[{"accountType":"UNIFIED","coin":[{"coin":"BTC","walletBalance":"4","locked":"1","availableToWithdraw":"3"}]}]}),
    );
    assert!(
        client(p, mock(body), WritePolicy::ReadOnly)
            .query_balances()
            .await
            .is_err()
    );
    let p = ProviderId::Okex;
    let body = envelope(
        p,
        json!([{"details":[{"ccy":"BTC","availBal":"3","frozenBal":"1","liab":"0.1"}]}]),
    );
    assert!(
        client(p, mock(body), WritePolicy::ReadOnly)
            .query_balances()
            .await
            .is_err()
    );
    let p = ProviderId::Binance;
    for rows in [
        json!([{"asset":"BTC","free":"-1","locked":"0"}]),
        json!([{"asset":"BTC","free":"1","locked":"0"},{"asset":"BTC","free":"2","locked":"0"}]),
    ] {
        assert!(
            client(p, mock(json!({"balances":rows})), WritePolicy::ReadOnly)
                .query_balances()
                .await
                .is_err()
        );
    }
    for p in ProviderId::ALL {
        let mut t = mock(open_body(p));
        t.has_more = true;
        assert!(
            client(p, t, WritePolicy::ReadOnly)
                .query_open_orders(native(p))
                .await
                .is_err()
        );
    }
    for p in [
        ProviderId::Bitget,
        ProviderId::Okex,
        ProviderId::Bybit,
        ProviderId::Kucoin,
        ProviderId::Max,
    ] {
        let rows = vec![
            row(p, false);
            if p == ProviderId::Bybit {
                50
            } else if p == ProviderId::Max {
                1000
            } else {
                100
            }
        ];
        let body = envelope(
            p,
            if p == ProviderId::Bybit {
                json!({"category":"spot","nextPageCursor":"more","list":rows})
            } else if p == ProviderId::Kucoin {
                json!({"totalPage":2,"currentPage":1,"totalNum":1000,"items":rows})
            } else {
                json!(rows)
            },
        );
        assert!(
            client(p, mock(body), WritePolicy::ReadOnly)
                .query_open_orders(native(p))
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn nonce_clients_serialize_transmission_and_headers_are_redacted() {
    struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
    #[async_trait]
    impl PrivateTransport for Capture {
        async fn execute(&self, _: ProviderId, r: SignedRequest) -> Result<Response> {
            self.0
                .lock()
                .unwrap()
                .push(r.headers().map(|(k, v)| (k.into(), v.into())).collect());
            Ok(Response {
                status: 200,
                body: serde_json::to_vec(&balance_body(ProviderId::Bitfinex)).unwrap(),
                has_more: false,
            })
        }
    }
    let captured = Arc::new(Mutex::new(vec![]));
    let c = client(
        ProviderId::Bitfinex,
        Capture(captured.clone()),
        WritePolicy::ReadOnly,
    );
    let (a, b, d) = tokio::join!(c.query_balances(), c.query_balances(), c.query_balances());
    a.unwrap();
    b.unwrap();
    d.unwrap();
    let headers = captured.lock().unwrap();
    let nonces = headers
        .iter()
        .map(|h| h["bfx-nonce"].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        nonces,
        vec![
            1_700_000_000_123_000,
            1_700_000_000_123_001,
            1_700_000_000_123_002
        ]
    );
    drop(headers);
    assert_eq!(
        format!("{:?}", credentials(ProviderId::Binance)),
        "Credentials([REDACTED])"
    );
    assert!(
        PrivateClient::new(
            ProviderId::Backpack,
            Credentials::new("bad".into(), "bad".into(), "".into()),
            markets(ProviderId::Backpack),
            WritePolicy::ReadOnly,
            mock(Json::Null)
        )
        .is_err()
    );
    assert!(
        PrivateClient::new(
            ProviderId::Coinbase,
            Credentials::new("key".into(), "not base64".into(), "p".into()),
            markets(ProviderId::Coinbase),
            WritePolicy::ReadOnly,
            mock(Json::Null)
        )
        .is_err()
    );
    let _ = Method::Get;
}

#[tokio::test]
async fn bitfinex_lookup_checks_active_then_terminal_history_without_inventing_a_route() {
    struct History {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl PrivateTransport for History {
        async fn execute(&self, _: ProviderId, r: SignedRequest) -> Result<Response> {
            let count = self.calls.fetch_add(1, Ordering::SeqCst);
            let body = if count == 0 {
                assert_eq!(r.path(), "/v2/auth/r/orders/tBTCUSD");
                assert_eq!(
                    serde_json::from_slice::<Json>(r.body()).unwrap(),
                    json!({"id":[42]})
                );
                json!([])
            } else {
                assert_eq!(count, 1);
                assert_eq!(r.path(), "/v2/auth/r/orders/tBTCUSD/hist");
                assert_eq!(
                    serde_json::from_slice::<Json>(r.body()).unwrap(),
                    json!({"id":[42],"limit":1})
                );
                json!([row(ProviderId::Bitfinex, true)])
            };
            Ok(Response {
                status: 200,
                body: serde_json::to_vec(&body).unwrap(),
                has_more: false,
            })
        }
    }
    let c = client(
        ProviderId::Bitfinex,
        History {
            calls: AtomicUsize::new(0),
        },
        WritePolicy::ReadOnly,
    );
    assert_eq!(
        c.query_order("BTCUSD", "42").await.unwrap().status,
        OrderStatus::Canceled
    );
}

#[tokio::test]
async fn credential_origin_is_bound_and_http_errors_never_leak_signed_material() {
    struct WrongOrigin;
    #[async_trait]
    impl PrivateTransport for WrongOrigin {
        async fn execute(&self, p: ProviderId, r: SignedRequest) -> Result<Response> {
            assert_eq!(r.provider(), p);
            // Refused before any network request can be made.
            bbgo_provider::private::HttpTransport::new()?
                .execute(ProviderId::Bitget, r)
                .await
        }
    }
    let c = client(ProviderId::Binance, WrongOrigin, enabled());
    assert!(
        c.submit_order(intent(ProviderId::Binance))
            .await
            .unwrap_err()
            .to_string()
            .contains("outcome unknown")
    );
    struct HttpError;
    #[async_trait]
    impl PrivateTransport for HttpError {
        async fn execute(&self, _: ProviderId, _: SignedRequest) -> Result<Response> {
            Ok(Response {
                status: 503,
                body: b"SECRET-SHOULD-NOT-LEAK".to_vec(),
                has_more: false,
            })
        }
    }
    let c = client(ProviderId::Binance, HttpError, enabled());
    let error = c
        .submit_order(intent(ProviderId::Binance))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("503"));
    assert!(!error.contains("SECRET"));
    assert!(c.submit_order(intent(ProviderId::Binance)).await.is_err());
}

#[tokio::test]
async fn partial_fills_and_unsupported_order_snapshots_are_not_silently_rewritten() {
    for p in ProviderId::ALL {
        let mut r = row(p, false);
        match p {
            ProviderId::Binance => {
                r["executedQty"] = json!("0.5");
                r["status"] = json!("PARTIALLY_FILLED");
            }
            ProviderId::Backpack => {
                r["executedQuantity"] = json!("0.5");
                r["status"] = json!("PartiallyFilled");
            }
            ProviderId::Bitfinex => {
                r[6] = json!(1.5);
                r[13] = json!("PARTIALLY FILLED @ 100(0.5)");
            }
            ProviderId::Bitget => {
                r["baseVolume"] = json!("0.5");
                r["status"] = json!("partially_filled");
            }
            ProviderId::Bybit => {
                r["cumExecQty"] = json!("0.5");
                r["orderStatus"] = json!("PartiallyFilled");
            }
            ProviderId::Coinbase => r["filled_size"] = json!("0.5"),
            ProviderId::Kucoin => r["dealSize"] = json!("0.5"),
            ProviderId::Max => r["executed_volume"] = json!("0.5"),
            ProviderId::Okex => {
                r["accFillSz"] = json!("0.5");
                r["state"] = json!("partially_filled");
            }
        }
        let rows = json!([r]);
        let body = envelope(
            p,
            match p {
                ProviderId::Bybit => json!({"category":"spot","list":rows}),
                ProviderId::Kucoin => {
                    json!({"currentPage":1,"totalPage":1,"totalNum":1,"items":rows})
                }
                _ => rows,
            },
        );
        let observed = client(p, mock(body), WritePolicy::ReadOnly)
            .query_open_orders(native(p))
            .await
            .unwrap();
        assert_eq!(observed[0].status, OrderStatus::PartiallyFilled);
        assert_eq!(observed[0].executed_quantity, n("0.5"));
    }
    let mut bad = row(ProviderId::Binance, false);
    bad["type"] = json!("STOP_LOSS_LIMIT");
    assert!(
        client(
            ProviderId::Binance,
            mock(json!([bad])),
            WritePolicy::ReadOnly
        )
        .query_open_orders("BTCUSDT")
        .await
        .is_err()
    );
}
