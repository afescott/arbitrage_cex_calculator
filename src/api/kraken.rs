use crate::{api::ExchangePrice, util::parse_price_cents};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const KRAKEN_WS_URL: &str = "wss://ws.kraken.com";

pub struct KrakenClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

impl KrakenClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        KrakenClient { tx }
    }

    pub async fn listen_btc_usdt(&self) {
        info!("[Kraken] Connecting to BTC/USDT orderbook depth stream...");

        match connect_async(KRAKEN_WS_URL).await {
            Ok((mut ws_stream, _)) => {
                info!("[Kraken] Connected successfully");

                // Subscribe to XBT/USD orderbook (Kraken uses XBT for Bitcoin)
                let subscribe_msg = serde_json::json!({
                    "event": "subscribe",
                    "pair": ["XBT/USD"],
                    "subscription": {
                        "name": "book"
                    }
                });

                // Send subscription message
                if let Err(e) = ws_stream
                    .send(Message::Text(subscribe_msg.to_string()))
                    .await
                {
                    error!("[Kraken] Failed to send subscription: {}", e);
                    return;
                }

                let (_write, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Capture timestamp immediately when message received
                            let received_at = Instant::now();
                            if let Err(e) = self.handle_message(&text, received_at).await {
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

    async fn handle_message(
        &self,
        text: &str,
        received_at: Instant,
    ) -> Result<(), Box<dyn std::error::Error>> {
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

        // Handle book data (array format)
        // Kraken format: [channelID, {data}, channelName, pair]
        // Book data: { "bids": [["price", "volume", "timestamp"], ...], "asks": [...] }
        if let Some(array) = value.as_array() {
            if array.len() >= 4 {
                if let Some(book_data) = array[1].as_object() {
                    // Process bids
                    if let Some(bids) = book_data.get("bids").and_then(|b| b.as_array()) {
                        for bid in bids {
                            if let Some(bid_array) = bid.as_array() {
                                if bid_array.len() >= 2 {
                                    if let (Some(price_str), Some(volume_str)) = (
                                        bid_array[0].as_str(),
                                        bid_array[1].as_str(),
                                    ) {
                                        // Skip if volume is "0.00000000" (removal)
                                        if volume_str == "0.00000000" || volume_str == "0" {
                                            continue;
                                        }
                                        
                                        if let (Some(price), Some(quantity)) = (
                                            parse_price_cents(price_str),
                                            crate::util::parse_quantity_smallest_unit(volume_str, 8),
                                        ) {
                                            // Parse timestamp if available (3rd element)
                                            let exchange_timestamp = bid_array.get(2)
                                                .and_then(|t| t.as_str())
                                                .and_then(|s| s.parse::<u64>().ok());
                                            
                                            self.tx.send(ExchangePrice::Kraken {
                                                price,
                                                quantity,
                                                exchange_timestamp,
                                                received_at,
                                            }).await.ok();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    // Process asks
                    if let Some(asks) = book_data.get("asks").and_then(|a| a.as_array()) {
                        for ask in asks {
                            if let Some(ask_array) = ask.as_array() {
                                if ask_array.len() >= 2 {
                                    if let (Some(price_str), Some(volume_str)) = (
                                        ask_array[0].as_str(),
                                        ask_array[1].as_str(),
                                    ) {
                                        // Skip if volume is "0.00000000" (removal)
                                        if volume_str == "0.00000000" || volume_str == "0" {
                                            continue;
                                        }
                                        
                                        if let (Some(price), Some(quantity)) = (
                                            parse_price_cents(price_str),
                                            crate::util::parse_quantity_smallest_unit(volume_str, 8),
                                        ) {
                                            // Parse timestamp if available (3rd element)
                                            let exchange_timestamp = ask_array.get(2)
                                                .and_then(|t| t.as_str())
                                                .and_then(|s| s.parse::<u64>().ok());
                                            
                                            self.tx.send(ExchangePrice::Kraken {
                                                price,
                                                quantity,
                                                exchange_timestamp,
                                                received_at,
                                            }).await.ok();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
