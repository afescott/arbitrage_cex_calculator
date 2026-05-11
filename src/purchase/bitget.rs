//! Bitget USDT-margined perpetual execution (mix v2).
//!
//! Implements `OrderExecutor` using Bitget REST `POST /api/v2/mix/order/place-order`.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use sha2::Sha256;

use super::{BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload};
use crate::orderbook::book::Exchange;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub(crate) struct BitgetExecutor {
    api_key: String,
    api_secret: String,
    passphrase: String,
    http: reqwest::Client,
    base_url: String,
}

impl BitgetExecutor {
    pub(crate) fn new(api_key: String, api_secret: String, passphrase: String) -> Self {
        Self {
            api_key,
            api_secret,
            passphrase,
            http: reqwest::Client::new(),
            base_url: "https://api.bitget.com".to_string(),
        }
    }

    fn sign_request(&self, ts_ms: i64, method: &str, request_path: &str, body: &str) -> String {
        let prehash = format!("{ts_ms}{method}{request_path}{body}");
        let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes())
            .expect("hmac can take key of any size");
        mac.update(prehash.as_bytes());
        let sig = mac.finalize().into_bytes();
        BASE64_STANDARD.encode(sig)
    }
}

impl OrderExecutor for BitgetExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Bitget
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        // Map base qty_e8 to decimal size string.
        let size = super::qty_sats_to_decimal_string(req.qty_sats);
        let price = format!("{:.2}", (req.price_cents as f64) / 100.0);
        let side = match req.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let force = if req.post_only { "post_only" } else { "ioc" };
        // Bitget position-mode nuance:
        // - In hedge-mode, `tradeSide` is required ("open"/"close") and `reduceOnly` is not used.
        // - In one-way-mode, Bitget ignores `tradeSide` and uses `reduceOnly` for close-only behavior.
        //
        // We always send `tradeSide` and omit `reduceOnly` to be compatible with hedge-mode accounts.
        // (One-way-mode will ignore `tradeSide`.)
        let trade_side = if req.reduce_only { "close" } else { "open" };

        let body = serde_json::json!({
            "symbol": "BTCUSDT",
            "productType": "USDT-FUTURES",
            "marginMode": "crossed",
            "marginCoin": "USDT",
            "size": size,
            "price": price,
            "side": side,
            "tradeSide": trade_side,
            "orderType": "limit",
            "force": force,
            "clientOid": format!("bot-{}", Utc::now().timestamp_millis()),
        })
        .to_string();

        Ok(BuiltPayload {
            venue: Exchange::Bitget,
            endpoint: "/api/v2/mix/order/place-order",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: "bitget:hmac-in-send".to_string(),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let ts_ms = Utc::now().timestamp_millis();
        let method = "POST";
        let request_path = signed.endpoint;
        let sign = self.sign_request(ts_ms, method, request_path, &signed.body);

        // Debug: show the planned order (no secrets).
        if let Ok(v) = serde_json::from_str::<Value>(&signed.body) {
            let side = v.get("side").and_then(|x| x.as_str()).unwrap_or("?");
            let size = v.get("size").and_then(|x| x.as_str()).unwrap_or("?");
            let price = v.get("price").and_then(|x| x.as_str()).unwrap_or("?");
            let force = v.get("force").and_then(|x| x.as_str()).unwrap_or("?");
            let trade_side = v.get("tradeSide").and_then(|x| x.as_str()).unwrap_or("?");
            eprintln!(
                "bitget place-order: side={} tradeSide={} size={} price={} force={}",
                side, trade_side, size, price, force
            );
        }

        let url = format!("{}{}", self.base_url, request_path);
        let resp = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header("ACCESS-KEY", &self.api_key)
            .header("ACCESS-SIGN", sign)
            .header("ACCESS-TIMESTAMP", ts_ms.to_string())
            .header("ACCESS-PASSPHRASE", &self.passphrase)
            .body(signed.body)
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("bitget http: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("bitget body: {e}")))?;

        // Debug: show response code/msg/orderId if available.
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("");
            let msg = v.get("msg").and_then(|x| x.as_str()).unwrap_or("");
            let order_id = v
                .get("data")
                .and_then(|d| d.get("orderId").or(d.get("order_id")))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            eprintln!("bitget response: http={} code={} msg={} orderId={}", status, code, msg, order_id);
        } else {
            eprintln!(
                "bitget response: http={} body_snip={}",
                status,
                text.chars().take(300).collect::<String>()
            );
        }

        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "bitget HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }

        // Prefer returning Bitget orderId if present.
        let client_order_id = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("data")
                    .and_then(|d| d.get("orderId").or(d.get("clientOid")))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        Ok(OrderAck {
            exchange: Exchange::Bitget,
            client_order_id,
            filled_qty_e8: None,
            venue_order_id: None,
        })
    }
}

