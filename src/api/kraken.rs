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

        // Handle ticker data (array format)
        // Kraken format: [channelID, {data}, channelName, pair]
        if let Some(array) = value.as_array() {
            if array.len() >= 4 {
                if let Some(ticker_data) = array[1].as_object() {
                    // Price is in ticker_data["c"][0]
                    if let Some(price_str) = ticker_data
                        .get("c")
                        .and_then(|c| c.as_array())
                        .and_then(|a| a.get(0))
                        .and_then(|v| v.as_str())
                    {
                        // Fast u64 parsing - avoids f64 overhead for low-latency
                        if let Some(price) = parse_price_cents(price_str) {
                            // Kraken doesn't provide explicit timestamp in ticker, but we capture receive time
                            self.tx.send(ExchangePrice::Kraken {
                                price,
                                exchange_timestamp: None, // Kraken ticker doesn't include timestamp
                                received_at,
                            }).await.ok();
                            info!("[Kraken] XBT/USD: ${}", price_str);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
