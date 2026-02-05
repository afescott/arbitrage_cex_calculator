use crate::{api::ExchangePrice, util::parse_price_cents};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const COINBASE_WS_URL: &str = "wss://ws-feed.exchange.coinbase.com";

pub struct CoinbaseClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

impl CoinbaseClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        CoinbaseClient { tx }
    }
    
    pub async fn listen_btc_usdt(&self) {
        info!("[Coinbase] Connecting to BTC/USDT orderbook depth stream...");
        
        match connect_async(COINBASE_WS_URL).await {
            Ok((mut ws_stream, _)) => {
                info!("[Coinbase] Connected successfully");
                
                // Subscribe to BTC-USD level2 orderbook (Coinbase uses BTC-USD, not BTC-USDT)
                let subscribe_msg = serde_json::json!({
                    "type": "subscribe",
                    "product_ids": ["BTC-USD"],
                    "channels": ["level2"]
                });
                
                // Send subscription message
                if let Err(e) = ws_stream.send(Message::Text(subscribe_msg.to_string())).await {
                    error!("[Coinbase] Failed to send subscription: {}", e);
                    return;
                }
                
                let (_write, mut read) = ws_stream.split();
                
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Capture timestamp immediately when message received
                            let received_at = Instant::now();
                            if let Err(e) = self.handle_message(&text, received_at).await {
                                warn!("[Coinbase] Error handling message: {}", e);
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            info!("[Coinbase] Received ping");
                        }
                        Ok(Message::Close(_)) => {
                            warn!("[Coinbase] Connection closed");
                            break;
                        }
                        Err(e) => {
                            error!("[Coinbase] WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("[Coinbase] Failed to connect: {}", e);
            }
        }
    }
    
    async fn handle_message(
        &self,
        text: &str,
        received_at: Instant,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Basic validation - prevent injection attacks
        if text.len() > 100_000 {
            return Err("Message too large".into());
        }
        
        // Parse ticker data
        let ticker: serde_json::Value = serde_json::from_str(text)?;
        
        // Handle subscription confirmation
        if let Some(msg_type) = ticker.get("type").and_then(|t| t.as_str()) {
            if msg_type == "subscriptions" {
                info!("[Coinbase] Subscription confirmed");
                return Ok(());
            }
        }
        
        // Handle ticker updates
        if let Some(msg_type) = ticker.get("type").and_then(|t| t.as_str()) {
            if msg_type == "ticker" {
                if let (Some(product_id), Some(price_str)) = (
                    ticker.get("product_id").and_then(|p| p.as_str()),
                    ticker.get("price").and_then(|p| p.as_str()),
                ) {
                    // Fast u64 parsing - avoids f64 overhead for low-latency
                    if let Some(price) = parse_price_cents(price_str) {
                        // Parse exchange timestamp (time field = ISO 8601, convert to ms)
                        // Coinbase provides "time" field but it's ISO 8601 string, not ms
                        // For now, we'll capture receive time and can parse exchange time later if needed
                        // Coinbase provides "time" field as ISO 8601 string
                        // For now, we'll use None (full implementation would parse ISO 8601 to Unix ms)
                        // The received_at timestamp is sufficient for latency measurement
                        let exchange_timestamp = None;
                        
                        // Include both exchange timestamp (for ordering) and receive timestamp (for latency)
                        self.tx.send(ExchangePrice::Coinbase {
                            price,
                            exchange_timestamp, // Coinbase uses ISO 8601, would need parsing
                            received_at,
                        }).await.ok();
                        info!("[Coinbase] {}: ${}", product_id, price_str);
                    }
                }
            }
        }
        
        Ok(())
    }
}

