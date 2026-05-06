//! Binance USDⓈ-M futures: query build → HMAC-SHA256 → REST `POST /fapi/v1/order`.

use hmac::{Hmac, Mac};
use reqwest::header::CONTENT_TYPE;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload,
};
use crate::orderbook::book::Exchange;

#[derive(Debug, Clone)]
pub(crate) struct BinanceFuturesExecutor {
    api_key: String,
    api_secret: String,
    http: reqwest::Client,
    base_url: &'static str,
}

impl BinanceFuturesExecutor {
    pub(crate) fn new(api_key: String, api_secret: String) -> Self {
        let http = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest binance client");
        Self {
            api_key,
            api_secret,
            http,
            base_url: "https://fapi.binance.com",
        }
    }
}

impl OrderExecutor for BinanceFuturesExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Binance
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        let sym = req.symbol.to_uppercase();
        let symbol = if sym.ends_with("USDT") {
            sym
        } else {
            format!("{sym}USDT")
        };
        let side = match req.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };
        let price = format!("{:.2}", (req.price_cents as f64) / 100.0);
        let quantity = format!("{:.8}", (req.qty_sats as f64) / 100_000_000.0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ExecError::SendFailed("clock".to_string()))?
            .as_millis();

        let mut body = format!(
            "symbol={symbol}&side={side}&type=LIMIT&timeInForce=GTC&price={price}&quantity={quantity}&timestamp={timestamp}"
        );
        if req.post_only {
            body.push_str("&timeInForce=GTX");
        }
        if req.reduce_only {
            body.push_str("&reduceOnly=true");
        }
        Ok(BuiltPayload {
            venue: Exchange::Binance,
            endpoint: "/fapi/v1/order",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.api_secret.as_bytes())
            .map_err(|_| ExecError::SendFailed("bad binance secret".to_string()))?;
        mac.update(built.body.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature,
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let url = format!("{}{}", self.base_url, signed.endpoint);
        let body = format!("{}&signature={}", signed.body, signed.signature);
        let resp = self
            .http
            .post(url)
            .header("X-MBX-APIKEY", &self.api_key)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("binance http: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read body: {e}>"));

        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "binance status {status}, body: {body}"
            )));
        }

        Ok(OrderAck {
            exchange: Exchange::Binance,
            client_order_id: "binance-http-ack".to_string(),
            filled_qty_e8: None,
            venue_order_id: None,
        })
    }
}
