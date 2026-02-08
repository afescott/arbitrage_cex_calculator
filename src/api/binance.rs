use crate::{api::ExchangePrice, util::parse_price_cents};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@depth20@100ms";

pub struct BinanceClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

impl BinanceClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        BinanceClient { tx }
    }
    pub async fn listen_btc_usdt(&self) {
        info!("[Binance] Connecting to BTC/USDT orderbook depth stream...");

        match connect_async(BINANCE_WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[Binance] Connected successfully");
                let (_write, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Capture timestamp immediately when message received
                            let received_at = Instant::now();
                            if let Err(e) = self.handle_message(&text, received_at).await {
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

    async fn handle_message(
        &self,
        text: &str,
        received_at: Instant,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Basic validation - prevent injection attacks
        if text.len() > 100_000 {
            return Err("Message too large".into());
        }

        // Parse depth update data
        let depth: serde_json::Value = serde_json::from_str(text)?;

        // Binance depth stream format: { "e": "depthUpdate", "bids": [[price, qty], ...], "asks": [[price, qty], ...] }
        if depth.get("e").and_then(|e| e.as_str()) != Some("depthUpdate") {
            // Skip non-depth messages
            return Ok(());
        }

        let exchange_timestamp = depth.get("E").and_then(|e| e.as_u64());

        // Process bids (we want to buy at these prices)
        if let Some(bids) = depth.get("bids").and_then(|b| b.as_array()) {
            for bid in bids {
                if let Some(bid_array) = bid.as_array() {
                    if bid_array.len() >= 2 {
                        if let (Some(price_str), Some(qty_str)) = (
                            bid_array[0].as_str(),
                            bid_array[1].as_str(),
                        ) {
                            if let (Some(price), Some(quantity)) = (
                                parse_price_cents(price_str),
                                crate::util::parse_quantity_smallest_unit(qty_str, 8), // BTC has 8 decimals
                            ) {
                                self.tx.send(ExchangePrice::Binance {
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

        // Process asks (we want to sell at these prices)
        if let Some(asks) = depth.get("asks").and_then(|a| a.as_array()) {
            for ask in asks {
                if let Some(ask_array) = ask.as_array() {
                    if ask_array.len() >= 2 {
                        if let (Some(price_str), Some(qty_str)) = (
                            ask_array[0].as_str(),
                            ask_array[1].as_str(),
                        ) {
                            if let (Some(price), Some(quantity)) = (
                                parse_price_cents(price_str),
                                crate::util::parse_quantity_smallest_unit(qty_str, 8),
                            ) {
                                self.tx.send(ExchangePrice::Binance {
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

        Ok(())
    }
}
