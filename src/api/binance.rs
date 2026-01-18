use crate::{api::ExchangePrice, util::parse_price_cents};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@ticker";

pub struct BinanceClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

impl BinanceClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        BinanceClient { tx }
    }
    pub async fn listen_btc_usdt(&self) {
        info!("[Binance] Connecting to BTC/USDT ticker stream...");

        match connect_async(BINANCE_WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[Binance] Connected successfully");
                let (_write, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Parse and handle ticker data
                            if let Err(e) = self.handle_message(&text).await {
                                warn!("[Binance] Error handling message: {}", e);
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            info!("[Binance] Received ping");
                        }
                        Ok(Message::Close(_)) => {
                            warn!("[Binance] Connection closed");
                            break;
                        }
                        Err(e) => {
                            error!("[Binance] WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("[Binance] Failed to connect: {}", e);
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

        if let (Some(symbol), Some(price_str)) = (
            ticker.get("s").and_then(|s| s.as_str()),
            ticker.get("c").and_then(|c| c.as_str()),
        ) {
            // Fast u64 parsing - avoids f64 overhead for low-latency
            if let Some(price) = parse_price_cents(price_str) {
                self.tx.send(ExchangePrice::Binance(price)).await.ok();
                info!("[Binance] {}: ${}", symbol, price_str);
            }
        }

        Ok(())
    }
}
