//! Order execution scaffolding: build payload → sign → send.
//!
//! This module focuses on API *shape* and call flow. `send()` is stubbed to keep
//! the project offline-safe until real REST/WS trading calls are wired in.

use crate::{args::Args, orderbook::book::Exchange};

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
    SendFailed(&'static str),
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
}

impl BinanceFuturesExecutor {
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self {
            api_key,
            api_secret,
        }
    }
}

impl OrderExecutor for BinanceFuturesExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Binance
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        // Stub: in real Binance Futures, you’d map to `symbol=BTCUSDT`, `side=BUY/SELL`,
        // `type=LIMIT`, `timeInForce=GTC`, `price`, `quantity`, and `timestamp`.
        Ok(BuiltPayload {
            venue: Exchange::Binance,
            endpoint: "/fapi/v1/order",
            body: format!(
                "symbol={}&side={:?}&type=LIMIT&price_cents={}&qty_sats={}&postOnly={}&reduceOnly={}",
                req.symbol, req.side, req.price_cents, req.qty_sats, req.post_only, req.reduce_only
            ),
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        // Stub signature: real Binance uses HMAC-SHA256 over the query string with api_secret.
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: format!(
                "bn_sig_stub_{}_{}",
                self.api_key.len(),
                self.api_secret.len()
            ),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let _ = signed;
        Ok(OrderAck {
            exchange: Exchange::Binance,
            client_order_id: "binance-order-stub".to_string(),
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
