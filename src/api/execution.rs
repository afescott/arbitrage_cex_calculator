//! Order execution scaffolding: build payload → sign → send.
//!
//! This module focuses on API *shape* and call flow. `send()` is stubbed to keep
//! the project offline-safe until real REST/WS trading calls are wired in.
//!
//! ## Hyperliquid
//! `POST https://api.hyperliquid.xyz/exchange` — body is JSON with `action`, `nonce`, `signature`
//! per [Exchange endpoint](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint).
//! Many markets enforce a **~$10 minimum order notional**; smaller sizes are for schema testing only.
//!
//! ## dYdX v4
//! Orders are **protobuf `MsgPlaceOrder`** transactions broadcast to the chain (not a single REST JSON).
//! `DydxExecutor::build_payload` emits a **planning JSON** shaped like the protocol fields; wire the real
//! client from [integration trade](https://docs.dydx.xyz/interaction/integration/integration-trade).

use crate::{args::Args, orderbook::book::Exchange};
#[cfg(feature = "cex")]
use hmac::{Hmac, Mac};
#[cfg(feature = "cex")]
use sha2::Sha256;
#[cfg(feature = "cex")]
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::{json, Value};
use std::time::{SystemTime as StdSystemTime, UNIX_EPOCH as StdUNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct LimitOrderRequest {
    pub exchange: Exchange,
    /// Base asset (e.g. `BTC`, `SOL`); Binance futures mapping appends `USDT` when missing.
    pub symbol: String,
    pub side: OrderSide,
    pub price_cents: u64,
    /// Base quantity × 1e8 (used for BTC-style sizing and Binance `quantity` decimals).
    pub qty_sats: u64,
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
    /// `meta.universe` index; if `None` and `perp_symbol == "BTC"`, [`resolve_asset_index`] uses `0`.
    asset_index: Option<u32>,
    perp_symbol: String,
}

impl HyperliquidExecutor {
    pub fn new(private_key: String, asset_index: Option<u32>, perp_symbol: String) -> Self {
        Self {
            private_key,
            asset_index,
            perp_symbol,
        }
    }

    fn resolve_asset_index(&self) -> Result<u32, ExecError> {
        if let Some(i) = self.asset_index {
            return Ok(i);
        }
        if self.perp_symbol == "BTC" {
            // Default guess only — confirm via `https://api.hyperliquid.xyz/info` `meta` response.
            return Ok(0);
        }
        Err(ExecError::SendFailed(format!(
            "Hyperliquid: set --hyperliquid-asset-id for perp_symbol={}",
            self.perp_symbol
        )))
    }
}

fn hl_nonce_ms() -> Result<u64, ExecError> {
    StdSystemTime::now()
        .duration_since(StdUNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|_| ExecError::SendFailed("clock".into()))
}

/// Hyperliquid `p` / `s` fields are decimal strings.
fn hl_decimal_size_from_qty_e8(qty_sats: u64) -> String {
    let int = qty_sats / 100_000_000;
    let frac = qty_sats % 100_000_000;
    if frac == 0 {
        return int.to_string();
    }
    let mut s = format!("{}.{:08}", int, frac);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn hl_price_decimal_from_cents(price_cents: u64) -> String {
    let dollars = price_cents / 100;
    let cents = price_cents % 100;
    if cents == 0 {
        dollars.to_string()
    } else if cents % 10 == 0 {
        format!("{}.{:01}", dollars, cents / 10)
    } else {
        format!("{}.{:02}", dollars, cents)
    }
}

impl OrderExecutor for HyperliquidExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        let asset = self.resolve_asset_index()?;
        let is_buy = matches!(req.side, OrderSide::Buy);
        let tif = if req.post_only { "Alo" } else { "Gtc" };
        let action = json!({
            "type": "order",
            "orders": [{
                "a": asset,
                "b": is_buy,
                "p": hl_price_decimal_from_cents(req.price_cents),
                "s": hl_decimal_size_from_qty_e8(req.qty_sats),
                "r": req.reduce_only,
                "t": { "limit": { "tif": tif } }
            }],
            "grouping": "na"
        });
        let nonce = hl_nonce_ms()?;
        let root = json!({
            "action": action,
            "nonce": nonce
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid json: {e}")))?;
        Ok(BuiltPayload {
            venue: Exchange::Hyperliquid,
            endpoint: "/exchange",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        let mut root: Value = serde_json::from_str(&built.body)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid parse: {e}")))?;
        // Stub: real signing follows the Python SDK (EIP-712 / L1 action hash). See Hyperliquid docs.
        root["signature"] = json!({
            "r": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "s": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "v": 27
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid json: {e}")))?;
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body,
            signature: format!("hyperliquid-stub-ecdsa:pk_len={}", self.private_key.len()),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let _ = signed;
        Ok(OrderAck {
            exchange: Exchange::Hyperliquid,
            client_order_id: "hyperliquid-order-stub".to_string(),
        })
    }
}

#[cfg(feature = "cex")]
#[derive(Debug, Clone)]
pub struct BinanceFuturesExecutor {
    api_key: String,
    api_secret: String,
    http: reqwest::Client,
    base_url: &'static str,
}

#[cfg(feature = "cex")]
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

#[cfg(feature = "cex")]
impl OrderExecutor for BinanceFuturesExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Binance
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        // Binance Futures expects: symbol (e.g. BTCUSDT), side, type, timeInForce, price, quantity, timestamp.
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

#[derive(Debug, Clone)]
pub struct DydxExecutor {
    private_key: String,
}

impl DydxExecutor {
    pub fn new(private_key: String) -> Self {
        Self { private_key }
    }
}

impl OrderExecutor for DydxExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Dydx
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        let market = if req.symbol.contains('-') {
            req.symbol.clone()
        } else {
            format!("{}-USD", req.symbol)
        };
        let is_buy = matches!(req.side, OrderSide::Buy);
        // Planning JSON only — matches the *shape* of protocol fields (see v4 `MsgPlaceOrder` / `Order`).
        let root = json!({
            "_comment": "Not a live HTTP body: sign and broadcast protobuf MsgPlaceOrder via dYdX v4 node/composite client.",
            "msg_type": "dydxprotocol.clob.MsgPlaceOrder",
            "order": {
                "order_id": {
                    "subaccount_id": {
                        "owner": "<0x... from wallet>",
                        "number": 0
                    },
                    "client_id": 0,
                    "order_flags": 64,
                    "clob_pair_id": 0
                },
                "side": if is_buy { "SIDE_BUY" } else { "SIDE_SELL" },
                "quantums": hl_decimal_size_from_qty_e8(req.qty_sats),
                "subticks": "<u64: price in subticks; resolve from clob pair + tick size>",
                "good_til_block": 0,
                "good_til_block_time": 0,
                "time_in_force": if req.post_only { "TIME_IN_FORCE_POST_ONLY" } else { "TIME_IN_FORCE_UNSPECIFIED" },
                "reduce_only": req.reduce_only,
                "client_metadata": 0,
                "condition_type": "CONDITION_TYPE_UNSPECIFIED",
                "conditional_order_trigger_subticks": "0",
                "order_router_address": ""
            },
            "market_ticker": market,
            "price_cents_internal": req.price_cents
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("dydx json: {e}")))?;
        Ok(BuiltPayload {
            venue: Exchange::Dydx,
            endpoint: "<chain broadcast MsgPlaceOrder>",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: format!("dydx-stub-cosmos:pk_len={}", self.private_key.len()),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let _ = signed;
        Ok(OrderAck {
            exchange: Exchange::Dydx,
            client_order_id: "dydx-order-stub".to_string(),
        })
    }
}

/// Execution context holding pre-built venue executors.
#[derive(Debug, Clone)]
pub struct ExecutorContext {
    hyperliquid: Option<HyperliquidExecutor>,
    #[cfg(feature = "cex")]
    binance: Option<BinanceFuturesExecutor>,
    dydx: Option<DydxExecutor>,
}

impl ExecutorContext {
    pub fn new(args: &Args) -> Self {
        let hyperliquid = args.hyperliquid_private_key.clone().map(|pk| {
            HyperliquidExecutor::new(
                pk,
                args.hyperliquid_asset_id,
                args.perp_symbol.clone(),
            )
        });
        #[cfg(feature = "cex")]
        let binance = match (
            args.binance_api_key.clone(),
            args.binance_api_secret.clone(),
        ) {
            (Some(k), Some(s)) => Some(BinanceFuturesExecutor::new(k, s)),
            _ => None,
        };
        let dydx = args.dydx_private_key.clone().map(DydxExecutor::new);
        Self {
            hyperliquid,
            #[cfg(feature = "cex")]
            binance,
            dydx,
        }
    }

    pub fn has_hyperliquid(&self) -> bool {
        self.hyperliquid.is_some()
    }

    #[cfg(feature = "cex")]
    pub fn has_binance(&self) -> bool {
        self.binance.is_some()
    }

    #[cfg(not(feature = "cex"))]
    pub fn has_binance(&self) -> bool {
        false
    }

    pub fn has_dydx(&self) -> bool {
        self.dydx.is_some()
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
        #[cfg(feature = "cex")]
        Exchange::Binance => {
            let ex = order.binance.as_ref().ok_or(ExecError::MissingCredentials(
                "--binance-api-key/--binance-api-secret",
            ))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        #[cfg(not(feature = "cex"))]
        Exchange::Binance => Err(ExecError::UnsupportedExchange(Exchange::Binance)),
        Exchange::Dydx => {
            let ex = order
                .dydx
                .as_ref()
                .ok_or(ExecError::MissingCredentials("--dydx-private-key"))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        other => Err(ExecError::UnsupportedExchange(other)),
    }
}
