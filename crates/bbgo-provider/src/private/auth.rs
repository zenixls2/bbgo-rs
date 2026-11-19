use super::{Credentials, Method, SignedRequest, requests::Request};
use crate::{ProviderId, public};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use bbgo_types::Result;
use ed25519_dalek::{Signer, SigningKey};
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha384};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

fn unbase64(secret: &str) -> Result<Zeroizing<Vec<u8>>> {
    BASE64
        .decode(secret)
        .map(Zeroizing::new)
        .map_err(|_| public::invalid("invalid base64 credential"))
}
fn backpack_key(c: &Credentials) -> Result<SigningKey> {
    let bytes = unbase64(&c.secret)?;
    let seed = Zeroizing::new(
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| public::invalid("Backpack secret must be a 32-byte Ed25519 seed"))?,
    );
    let key = SigningKey::from_bytes(&seed);
    let public = unbase64(&c.key)?;
    if public.as_slice() != key.verifying_key().as_bytes() {
        return Err(public::invalid("Backpack public key does not match seed"));
    }
    Ok(key)
}
pub(super) fn validate_key(p: ProviderId, c: &Credentials) -> Result<()> {
    match p {
        ProviderId::Backpack => {
            backpack_key(c)?;
        }
        ProviderId::Coinbase if unbase64(&c.secret)?.is_empty() => {
            return Err(public::invalid("empty decoded secret"));
        }
        _ => {}
    }
    Ok(())
}
fn mac256(secret: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length.
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}
fn query(params: &BTreeMap<String, String>) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish()
}
fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>> {
    let map = value
        .as_object()
        .ok_or_else(|| public::invalid("expected flat request object"))?;
    let sorted = map.iter().collect::<BTreeMap<_, _>>();
    serde_json::to_vec(&sorted).map_err(|_| public::invalid("invalid request body"))
}
pub(super) fn sign(
    p: ProviderId,
    c: &Credentials,
    mut r: Request,
    ms: i64,
    nonce: i64,
) -> Result<SignedRequest> {
    let mut headers = BTreeMap::<String, Zeroizing<String>>::new();
    let mut header = |k: &str, v: String| {
        headers.insert(k.into(), Zeroizing::new(v));
    };
    header("Content-Type", "application/json".into());
    header("Accept", "application/json".into());
    let mut body = if r.body.is_null() {
        Vec::new()
    } else {
        canonical_json(&r.body)?
    };
    match p {
        ProviderId::Binance => {
            r.params.insert("timestamp".into(), ms.to_string());
            r.params.insert("recvWindow".into(), "5000".into());
            let sig = hex::encode(mac256(c.secret.as_bytes(), query(&r.params).as_bytes()));
            // signature is appended AFTER canonical parameters, not re-sorted.
            let query = format!("{}&signature={sig}", query(&r.params));
            header("X-MBX-APIKEY", c.key.to_string());
            return Ok(SignedRequest {
                provider: p,
                method: r.method,
                path: r.path,
                query: Zeroizing::new(query),
                headers,
                body,
                write: r.write,
            });
        }
        ProviderId::Backpack => {
            let mut params = r.params.clone();
            if let Some(obj) = r.body.as_object() {
                for (k, v) in obj {
                    params.insert(
                        k.clone(),
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    );
                }
            }
            let mut preimage = format!("instruction={}", r.instruction);
            for (k, v) in params {
                preimage.push_str(&format!("&{k}={v}"));
            }
            preimage.push_str(&format!("&timestamp={ms}&window=5000"));
            header("X-API-Key", c.key.to_string());
            header("X-Timestamp", ms.to_string());
            header("X-Window", "5000".into());
            header(
                "X-Signature",
                BASE64.encode(backpack_key(c)?.sign(preimage.as_bytes()).to_bytes()),
            );
        }
        ProviderId::Bitfinex => {
            let mut mac =
                Hmac::<Sha384>::new_from_slice(c.secret.as_bytes()).expect("HMAC key length");
            mac.update(format!("/api{}{nonce}", r.path).as_bytes());
            mac.update(&body);
            header("bfx-apikey", c.key.to_string());
            header("bfx-nonce", nonce.to_string());
            header("bfx-signature", hex::encode(mac.finalize().into_bytes()));
        }
        ProviderId::Max => {
            let mut payload = r
                .params
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect::<serde_json::Map<_, _>>();
            if let Some(obj) = r.body.as_object() {
                payload.extend(obj.clone());
            }
            payload.insert("nonce".into(), nonce.into());
            payload.insert("path".into(), r.path.clone().into());
            let encoded = BASE64.encode(canonical_json(&serde_json::Value::Object(payload))?);
            header("X-MAX-ACCESSKEY", c.key.to_string());
            header(
                "X-MAX-SIGNATURE",
                hex::encode(mac256(c.secret.as_bytes(), encoded.as_bytes())),
            );
            header("X-MAX-PAYLOAD", encoded);
            r.params.insert("nonce".into(), nonce.to_string());
        }
        ProviderId::Bybit => {
            let data = if r.method == Method::Get {
                query(&r.params).into_bytes()
            } else {
                body.clone()
            };
            let mut preimage = format!("{ms}{}5000", c.key.as_str()).into_bytes();
            preimage.extend_from_slice(&data);
            header("X-BAPI-API-KEY", c.key.to_string());
            header("X-BAPI-TIMESTAMP", ms.to_string());
            header("X-BAPI-RECV-WINDOW", "5000".into());
            header(
                "X-BAPI-SIGN",
                hex::encode(mac256(c.secret.as_bytes(), &preimage)),
            );
        }
        ProviderId::Bitget | ProviderId::Kucoin | ProviderId::Coinbase | ProviderId::Okex => {
            let timestamp = match p {
                ProviderId::Coinbase => format!("{}.{:03}", ms / 1000, ms % 1000),
                ProviderId::Okex => {
                    let t =
                        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
                            .map_err(|_| public::invalid("timestamp outside RFC3339 domain"))?;
                    let format = time::format_description::parse_borrowed::<2>(
                        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
                    )
                    .map_err(|_| public::invalid("timestamp format"))?;
                    t.format(&format)
                        .map_err(|_| public::invalid("timestamp format"))?
                }
                _ => ms.to_string(),
            };
            let q = query(&r.params);
            let mut preimage = format!("{timestamp}{}{}", r.method.as_str(), r.path).into_bytes();
            if !q.is_empty() {
                preimage.extend_from_slice(format!("?{q}").as_bytes());
            }
            preimage.extend_from_slice(&body);
            let secret = if p == ProviderId::Coinbase {
                unbase64(&c.secret)?
            } else {
                Zeroizing::new(c.secret.as_bytes().to_vec())
            };
            let sig = BASE64.encode(mac256(&secret, &preimage));
            let (key, sign, stamp, pass) = match p {
                ProviderId::Bitget => (
                    "ACCESS-KEY",
                    "ACCESS-SIGN",
                    "ACCESS-TIMESTAMP",
                    "ACCESS-PASSPHRASE",
                ),
                ProviderId::Kucoin => (
                    "KC-API-KEY",
                    "KC-API-SIGN",
                    "KC-API-TIMESTAMP",
                    "KC-API-PASSPHRASE",
                ),
                ProviderId::Coinbase => (
                    "CB-ACCESS-KEY",
                    "CB-ACCESS-SIGN",
                    "CB-ACCESS-TIMESTAMP",
                    "CB-ACCESS-PASSPHRASE",
                ),
                _ => (
                    "OK-ACCESS-KEY",
                    "OK-ACCESS-SIGN",
                    "OK-ACCESS-TIMESTAMP",
                    "OK-ACCESS-PASSPHRASE",
                ),
            };
            header(key, c.key.to_string());
            header(sign, sig);
            header(stamp, timestamp);
            header(
                pass,
                if p == ProviderId::Kucoin {
                    BASE64.encode(mac256(c.secret.as_bytes(), c.passphrase.as_bytes()))
                } else {
                    c.passphrase.to_string()
                },
            );
            if p == ProviderId::Kucoin {
                header("KC-API-KEY-VERSION", "2".into());
            }
        }
    }
    // GET requests must not carry an artificial JSON null/empty object body.
    if r.method == Method::Get {
        body.clear();
    }
    Ok(SignedRequest {
        provider: p,
        method: r.method,
        path: r.path,
        query: Zeroizing::new(query(&r.params)),
        headers,
        body,
        write: r.write,
    })
}
