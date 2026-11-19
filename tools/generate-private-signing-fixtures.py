#!/usr/bin/env python3
"""Offline, synthetic signing vectors: independent Python encoding + Go stdlib crypto."""
import base64
import json
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlencode

MS = 1700000000123
KEY = "test-key"
SECRET = "test-secret"
PASS = "test-passphrase"
SEED = bytes(range(32))
# RFC8032-compatible Ed25519 public key for the deterministic synthetic seed.
PUBLIC = "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="
ROOT = Path(__file__).resolve().parents[1]


def compact(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


def cases():
    # provider, method, path, params, body, Backpack instruction
    return [
        ("binance", "GET", "/api/v3/account", {}, None, ""),
        ("binance", "POST", "/api/v3/order", dict(symbol="BTCUSDT", side="BUY", type="LIMIT", timeInForce="GTC", quantity="2", price="100", newClientOrderId="test1", newOrderRespType="RESULT"), None, ""),
        ("binance", "DELETE", "/api/v3/order", dict(symbol="BTCUSDT", orderId="42"), None, ""),
        ("backpack", "GET", "/api/v1/capital", {}, None, "balanceQuery"),
        ("backpack", "POST", "/api/v1/order", {}, dict(symbol="BTC_USDC", side="Bid", orderType="Limit", quantity="2", price="100", timeInForce="GTC", clientId=123, autoBorrow=False, autoBorrowRepay=False, autoLend=False, autoLendRedeem=False), "orderExecute"),
        ("backpack", "DELETE", "/api/v1/order", {}, dict(symbol="BTC_USDC", orderId="42"), "orderCancel"),
        ("bitfinex", "POST", "/v2/auth/r/wallets", {}, {}, ""),
        ("bitfinex", "POST", "/v2/auth/w/order/submit", {}, dict(type="EXCHANGE LIMIT", symbol="tBTCUSD", amount="2", price="100", cid=123), ""),
        ("bitfinex", "POST", "/v2/auth/w/order/cancel", {}, dict(id=42), ""),
        ("bitget", "GET", "/api/v2/spot/account/assets", dict(assetType="all"), None, ""),
        ("bitget", "POST", "/api/v2/spot/trade/place-order", {}, dict(symbol="BTCUSDT", side="buy", orderType="limit", force="gtc", price="100", size="2", clientOid="test1"), ""),
        ("bitget", "POST", "/api/v2/spot/trade/cancel-order", {}, dict(symbol="BTCUSDT", orderId="42"), ""),
        ("bybit", "GET", "/v5/account/wallet-balance", dict(accountType="UNIFIED"), None, ""),
        ("bybit", "POST", "/v5/order/create", {}, dict(category="spot", symbol="BTCUSDT", side="Buy", orderType="Limit", qty="2", price="100", timeInForce="GTC", orderLinkId="test1", isLeverage=0), ""),
        ("bybit", "POST", "/v5/order/cancel", {}, dict(category="spot", symbol="BTCUSDT", orderId="42"), ""),
        ("coinbase", "GET", "/accounts", {}, None, ""),
        ("coinbase", "POST", "/orders", {}, dict(product_id="BTC-USD", side="buy", type="limit", size="2", price="100", time_in_force="GTC", client_oid="00000000-0000-4000-8000-000000000001"), ""),
        ("coinbase", "DELETE", "/orders/42", {}, None, ""),
        ("kucoin", "GET", "/api/v1/accounts", dict(type="trade"), None, ""),
        ("kucoin", "POST", "/api/v1/orders", {}, dict(symbol="BTC-USDT", side="buy", type="limit", size="2", price="100", timeInForce="GTC", clientOid="test1"), ""),
        ("kucoin", "DELETE", "/api/v1/orders/42", {}, None, ""),
        ("max", "GET", "/api/v3/wallet/spot/accounts", {}, None, ""),
        ("max", "POST", "/api/v3/wallet/spot/order", {}, dict(market="btctwd", side="buy", ord_type="limit", volume="2", price="100", client_oid="test1"), ""),
        ("max", "DELETE", "/api/v3/order", {}, dict(id=42), ""),
        ("okex", "GET", "/api/v5/account/balance", {}, None, ""),
        ("okex", "POST", "/api/v5/trade/order", {}, dict(instId="BTC-USDT", tdMode="cash", side="buy", ordType="limit", sz="2", px="100", clOrdId="test1"), ""),
        ("okex", "POST", "/api/v5/trade/cancel-order", {}, dict(instId="BTC-USDT", ordId="42"), ""),
    ]


GO = r'''package main
import("crypto/ed25519";"crypto/hmac";"crypto/sha256";"crypto/sha512";"encoding/base64";"encoding/hex";"encoding/json";"os")
type Job struct { Algorithm, Secret, Preimage string }
func main(){var jobs []Job; if err:=json.NewDecoder(os.Stdin).Decode(&jobs);err!=nil{panic(err)};out:=[]string{};for _,j:=range jobs {
 var sig string
 switch j.Algorithm {
 case "ed25519":seed,err:=base64.StdEncoding.DecodeString(j.Secret);if err!=nil{panic(err)};sig=base64.StdEncoding.EncodeToString(ed25519.Sign(ed25519.NewKeyFromSeed(seed),[]byte(j.Preimage)))
 case "sha384":m:=hmac.New(sha512.New384,[]byte(j.Secret));m.Write([]byte(j.Preimage));sig=hex.EncodeToString(m.Sum(nil))
 default: secret:=[]byte(j.Secret);if j.Algorithm=="coinbase"{secret,_=base64.StdEncoding.DecodeString(j.Secret)};m:=hmac.New(sha256.New,secret);m.Write([]byte(j.Preimage));if j.Algorithm=="hex"{sig=hex.EncodeToString(m.Sum(nil))}else{sig=base64.StdEncoding.EncodeToString(m.Sum(nil))}
 };out=append(out,sig)};json.NewEncoder(os.Stdout).Encode(out)}
'''


def main():
    vectors, jobs = [], []
    for i, (provider, method, path, params, obj, instruction) in enumerate(cases()):
        params = dict(params)
        body = "" if obj is None else compact(obj)
        algorithm, secret = "base64", SECRET
        if provider == "binance":
            params.update(timestamp=str(MS), recvWindow="5000")
            preimage = urlencode(sorted(params.items()))
            algorithm, header = "hex", "signature-query"
        elif provider == "backpack":
            pairs = dict(params)
            pairs.update(obj or {})
            val = lambda v: str(v).lower() if isinstance(v, bool) else str(v)
            preimage = "instruction=" + instruction + "".join("&" + k + "=" + val(v) for k, v in sorted(pairs.items())) + f"&timestamp={MS}&window=5000"
            algorithm, secret, header = "ed25519", base64.b64encode(SEED).decode(), "X-Signature"
        elif provider == "bitfinex":
            preimage, algorithm, header = "/api" + path + str(MS * 1000) + body, "sha384", "bfx-signature"
        elif provider == "max":
            payload = dict(params)
            payload.update(obj or {})
            payload.update(nonce=MS, path=path)
            encoded = base64.b64encode(compact(payload).encode()).decode()
            preimage, algorithm, header = encoded, "hex", "X-MAX-SIGNATURE"
            params["nonce"] = str(MS)
        elif provider == "bybit":
            preimage = str(MS) + KEY + "5000" + (urlencode(sorted(params.items())) if method == "GET" else body)
            algorithm, header = "hex", "X-BAPI-SIGN"
        else:
            timestamp = "2023-11-14T22:13:20.123Z" if provider == "okex" else "1700000000.123" if provider == "coinbase" else str(MS)
            q = urlencode(sorted(params.items()))
            preimage = timestamp + method + path + ("?" + q if q else "") + body
            header = dict(bitget="ACCESS-SIGN", kucoin="KC-API-SIGN", coinbase="CB-ACCESS-SIGN", okex="OK-ACCESS-SIGN")[provider]
            if provider == "coinbase":
                algorithm, secret = "coinbase", base64.b64encode(SECRET.encode()).decode()
        vectors.append(dict(provider=provider, operation=["balances", "submit", "cancel"][i % 3], method=method, path=path, query=urlencode(sorted(params.items())), body=body, signature_header=header, preimage=preimage))
        jobs.append(dict(Algorithm=algorithm, Secret=secret, Preimage=preimage))
    with tempfile.TemporaryDirectory(prefix="bbgo-signatures-", dir="/tmp/opencode") as folder:
        source = Path(folder) / "main.go"
        source.write_text(GO)
        result = subprocess.run(["go", "run", str(source)], input=json.dumps(jobs), text=True, capture_output=True, check=True)
    for vector, signature in zip(vectors, json.loads(result.stdout), strict=True):
        vector["signature"] = signature
        if vector["signature_header"] == "signature-query":
            vector["query"] += "&signature=" + signature
    destination = ROOT / "crates/bbgo-provider/tests/fixtures/private-signing.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(vectors, indent=2) + "\n")
    print(f"Wrote {len(vectors)} synthetic Go-stdlib signing vectors: {destination}")


if __name__ == "__main__":
    main()
