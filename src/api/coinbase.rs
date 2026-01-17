use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::util::parse_price_cents;

const COINBASE_WS_URL: &str = "wss://ws-feed.exchange.coinbase.com";

pub struct CoinbaseClient {
    tx: tokio::sync::mpsc::Sender<u64>,
}

impl CoinbaseClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<u64>) -> Self {
        CoinbaseClient { tx }
    }
    
    pub async fn listen_btc_usdt(&self) {
        info!("[Coinbase] Connecting to BTC/USDT ticker stream...");
        
        match connect_async(COINBASE_WS_URL).await {
            Ok((mut ws_stream, _)) => {
                info!("[Coinbase] Connected successfully");
                
                // Subscribe to BTC-USD ticker (Coinbase uses BTC-USD, not BTC-USDT)
                let subscribe_msg = serde_json::json!({
                    "type": "subscribe",
                    "product_ids": ["BTC-USD"],
                    "channels": ["ticker"]
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
                            if let Err(e) = self.handle_message(&text).await {
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
    
    async fn handle_message(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
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
                        self.tx.send(price).await.ok();
                        info!("[Coinbase] {}: ${}", product_id, price_str);
                    }
                }
            }
        }
        
        Ok(())
    }
}

