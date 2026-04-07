//! Minimal order-execution API spec (stubs).
//!
//! This is intentionally lightweight: it defines the request/response shape and
//! leaves transport/auth wiring for later.

use crate::orderbook::book::Exchange;

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
    pub qty_sats: u64,        // BTC smallest units (1e8)
    pub post_only: bool,
    pub reduce_only: bool,    // perps: set true for hedge leg if desired
}

#[derive(Debug, Clone)]
pub struct OrderAck {
    pub exchange: Exchange,
    pub client_order_id: String,
}

pub async fn submit_limit_order(req: LimitOrderRequest) -> Result<OrderAck, String> {
    // Stub: replace with signed REST / WS trading API calls.
    // Hyperliquid: typically signed action payload; Binance: signed REST endpoint.
    Ok(OrderAck {
        exchange: req.exchange,
        client_order_id: format!("{:?}-{:?}-{}", req.exchange, req.side, req.price_cents),
    })
}

