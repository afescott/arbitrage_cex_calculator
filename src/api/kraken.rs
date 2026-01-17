use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const KRAKEN_WS_URL: &str = "wss://ws.kraken.com";

pub struct KrakenClient {
    tx: tokio::sync::mpsc::Sender<u64>,
}

impl KrakenClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<u64>) -> Self {
        KrakenClient { tx }
    }
    
    pub async fn listen_btc_usdt(&self) {
        info!("[Kraken] Connecting to BTC/USDT ticker stream...");
        
        match connect_async(KRAKEN_WS_URL).await {
            Ok((mut ws_stream, _)) => {
                info!("[Kraken] Connected successfully");
                
                // Subscribe to XBT/USD ticker (Kraken uses XBT for Bitcoin)
                let subscribe_msg = serde_json::json!({
                    "event": "subscribe",
                    "pair": ["XBT/USD"],
                    "subscription": {
                        "name": "ticker"
                    }
                });
                
                // Send subscription message
                if let Err(e) = ws_stream.send(Message::Text(subscribe_msg.to_string())).await {
                    error!("[Kraken] Failed to send subscription: {}", e);
                    return;
                }
                
                let (_write, mut read) = ws_stream.split();
                
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Err(e) = self.handle_message(&text).await {
                                warn!("[Kraken] Error handling message: {}", e);
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            info!("[Kraken] Received ping");
                        }
                        Ok(Message::Close(_)) => {
                            warn!("[Kraken] Connection closed");
                            break;
                        }
                        Err(e) => {
                            error!("[Kraken] WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("[Kraken] Failed to connect: {}", e);
            }
        }
    }
    
    async fn handle_message(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Basic validation - prevent injection attacks
        if text.len() > 100_000 {
            return Err("Message too large".into());
        }
        
        // Parse Kraken message (can be array or object)
        let value: serde_json::Value = serde_json::from_str(text)?;
        
        // Handle subscription confirmation
        if let Some(event) = value.get("event").and_then(|e| e.as_str()) {
            info!("[Kraken] Event: {}", event);
            return Ok(());
        }
        
        // Handle ticker data (array format)
        if let Some(array) = value.as_array() {
            if array.len() >= 4 {
                if let Some(price_str) = array[1].as_object()
                    .and_then(|o| o.get("c"))
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.get(0))
                    .and_then(|v| v.as_str())
                {
                    if let Ok(price) = price_str.parse::<u64>() {
                        self.tx.send(price).await.ok();
                        info!("[Kraken] XBT/USD: ${}", price);
                    }
                }
            }
        }
        
        Ok(())
    }
}

