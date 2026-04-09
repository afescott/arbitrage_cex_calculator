//! Order execution scaffolding: build payload → sign → send.
//!
//! This module focuses on API *shape* and call flow. `send()` is stubbed to keep
//! the project offline-safe until real REST/WS trading calls are wired in.

use crate::{args::Args, orderbook::book::Exchange};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy)]
pub struct LimitOrderRequest {
    pub exchange: Exchange,
    pub symbol: &'static str, // e.g. "BTC"
    pub side: OrderSide,
    pub price_cents: u64,
    pub qty_sats: u64, // BTC smallest units (1e8)
    pub post_only: bool,
    pub reduce_only: bool, // perps: hedge leg can set true if desired
}

#[derive(Debug, Clone)]
pub struct OrderAck {
    pub exchange: Exchange,
    pub client_order_id: String,
}

#[derive(Debug)]
pub enum ExecError {
    MissingCredentials(&'static str),
    UnsupportedExchange(Exchange),
    SendFailed(String),
}

#[derive(Debug, Clone)]
pub struct BuiltPayload {
    pub venue: Exchange,
    pub endpoint: &'static str,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SignedPayload {
    pub venue: Exchange,
    pub endpoint: &'static str,
    pub body: String,
    pub signature: String,
}

pub trait OrderExecutor {
    fn venue(&self) -> Exchange;
    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError>;
    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError>;
    fn send(
        &self,
        signed: SignedPayload,
    ) -> impl std::future::Future<Output = Result<OrderAck, ExecError>> + Send;
}

#[derive(Debug, Clone)]
pub struct HyperliquidExecutor {
    private_key: String,
}

impl HyperliquidExecutor {
    pub fn new(private_key: String) -> Self {
        Self { private_key }
    }
}

impl OrderExecutor for HyperliquidExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        Ok(BuiltPayload {
            venue: Exchange::Hyperliquid,
            endpoint: "/exchange",
            body: format!(
                "{{\"type\":\"limit\",\"coin\":\"{}\",\"side\":\"{:?}\",\"px_cents\":{},\"qty_sats\":{},\"post_only\":{},\"reduce_only\":{}}}",
                req.symbol, req.side, req.price_cents, req.qty_sats, req.post_only, req.reduce_only
            ),
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        // Stub signature: real Hyperliquid uses an Ethereum-style signature over an action payload.
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: format!("hl_sig_stub_len{}", self.private_key.len()),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let _ = signed;
        Ok(OrderAck {
            exchange: Exchange::Hyperliquid,
            client_order_id: "hl-order-stub".to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct BinanceFuturesExecutor {
    api_key: String,
    api_secret: String,
    http: reqwest::Client,
    base_url: &'static str,
}

impl BinanceFuturesExecutor {
    pub fn new(api_key: String, api_secret: String) -> Self {
        let http = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest client");
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
        // Binance Futures expects: symbol (e.g. BTCUSDT), side, type, timeInForce, price, quantity, timestamp.
        let symbol = match req.symbol {
            "BTC" => "BTCUSDT",
            other => other,
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
        // Binance uses HMAC-SHA256 over the query string with api_secret.
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
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
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

        // We return a lightweight ack; parse response JSON later (orderId/clientOrderId).
        Ok(OrderAck {
            exchange: Exchange::Binance,
            client_order_id: "binance-http-ack".to_string(),
        })
    }
}

/// Execution context holding pre-built venue executors.
#[derive(Debug, Clone)]
pub struct ExecutorContext {
    hyperliquid: Option<HyperliquidExecutor>,
    binance: Option<BinanceFuturesExecutor>,
}

impl ExecutorContext {
    pub fn new(args: &Args) -> Self {
        let hyperliquid = args
            .hyperliquid_private_key
            .clone()
            .map(HyperliquidExecutor::new);
        let binance = match (
            args.binance_api_key.clone(),
            args.binance_api_secret.clone(),
        ) {
            (Some(k), Some(s)) => Some(BinanceFuturesExecutor::new(k, s)),
            _ => None,
        };
        Self {
            hyperliquid,
            binance,
        }
    }

    pub fn has_hyperliquid(&self) -> bool {
        self.hyperliquid.is_some()
    }

    pub fn has_binance(&self) -> bool {
        self.binance.is_some()
    }
}

pub async fn submit_limit_order(
    order: &ExecutorContext,
    req: LimitOrderRequest,
) -> Result<OrderAck, ExecError> {
    match req.exchange {
        Exchange::Hyperliquid => {
            let ex = order
                .hyperliquid
                .as_ref()
                .ok_or(ExecError::MissingCredentials("--hyperliquid-private-key"))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        Exchange::Binance => {
            let ex = order.binance.as_ref().ok_or(ExecError::MissingCredentials(
                "--binance-api-key/--binance-api-secret",
            ))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        other => Err(ExecError::UnsupportedExchange(other)),
    }
}
