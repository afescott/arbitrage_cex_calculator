use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const COINBASE_WS_URL: &str = "wss://ws-feed.exchange.coinbase.com";

pub struct CoinbaseClient;

impl CoinbaseClient {
    pub async fn listen_btc_usdt() {
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
                            if let Err(e) = Self::handle_message(&text).await {
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
    
    async fn handle_message(text: &str) -> Result<(), Box<dyn std::error::Error>> {
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
                if let (Some(product_id), Some(price)) = (
                    ticker.get("product_id").and_then(|p| p.as_str()),
                    ticker.get("price").and_then(|p| p.as_str()),
                ) {
                    info!("[Coinbase] {}: ${}", product_id, price);
                }
            }
        }
        
        Ok(())
    }
}

