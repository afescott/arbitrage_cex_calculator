use crate::{api::ExchangePrice, util::parse_price_cents};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

// Using regular depth stream - @depth20@100ms sends aggregated updates which might have different format
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

        // Debug: log first message to see format
        static FIRST_MESSAGE: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(true);
        if FIRST_MESSAGE.swap(false, std::sync::atomic::Ordering::Relaxed) {
            info!(
                "[Binance] First message sample (first 500 chars): {}",
                &text[..text.len().min(500)]
            );
        }

        // Parse depth update data
        let depth: serde_json::Value = serde_json::from_str(text)?;

        // Binance depth stream format:
        // - Snapshot: { "lastUpdateId": ..., "bids": [[price, qty], ...], "asks": [[price, qty], ...] }
        // - Updates: { "e": "depthUpdate", "bids": [[price, qty], ...], "asks": [[price, qty], ...] }
        let event_type = depth.get("e").and_then(|e| e.as_str());
        let is_snapshot = depth.get("lastUpdateId").is_some();
        let is_update = event_type == Some("depthUpdate");
        
        // Only process depth snapshots and updates
        if !is_snapshot && !is_update {
            // Skip other message types
            return Ok(());
        }

        let exchange_timestamp = depth.get("E").and_then(|e| e.as_u64());

        // Process bids (we want to buy at these prices)
        let bids_opt = depth.get("bids").and_then(|b| b.as_array());
        if bids_opt.is_none() {
            warn!("[Binance] No 'bids' array found in depthUpdate message");
        }
        
        if let Some(bids) = bids_opt {
            info!("[Binance] Processing {} bids", bids.len());
            for bid in bids {
                if let Some(bid_array) = bid.as_array() {
                    if bid_array.len() >= 2 {
                        if let (Some(price_str), Some(qty_str)) =
                            (bid_array[0].as_str(), bid_array[1].as_str())
                        {
                            let price_opt = parse_price_cents(price_str);
                            let quantity_opt =
                                crate::util::parse_quantity_smallest_unit(qty_str, 8); // BTC has 8 decimals
                                                                                       //
                            println!("Binance bid: price_str={}, qty_str={}", price_str, qty_str);

                            if let (Some(price), Some(quantity)) = (price_opt, quantity_opt) {
                                info!(
                                    "[Binance] Bid: price={}, qty_str={}, quantity={}",
                                    price_str, qty_str, quantity
                                );
                                self.tx
                                    .send(ExchangePrice::Binance {
                                        price,
                                        quantity,
                                        exchange_timestamp,
                                        received_at,
                                    })
                                    .await
                                    .ok();
                            } else {
                                warn!("[Binance] Failed to parse bid: price_str={:?} (parsed: {:?}), qty_str={:?} (parsed: {:?})", 
                                    price_str, price_opt, qty_str, quantity_opt);
                            }
                        }
                    }
                }
            }
        } else {
            warn!("[Binance] No bids array in depthUpdate message");
        }

        // Process asks (we want to sell at these prices)
        let asks_opt = depth.get("asks").and_then(|a| a.as_array());
        if asks_opt.is_none() {
            warn!("[Binance] No 'asks' array found in depthUpdate message");
        }
        
        if let Some(asks) = asks_opt {
            info!("[Binance] Processing {} asks", asks.len());
            for ask in asks {
                if let Some(ask_array) = ask.as_array() {
                    if ask_array.len() >= 2 {
                        if let (Some(price_str), Some(qty_str)) =
                            (ask_array[0].as_str(), ask_array[1].as_str())
                        {
                            let price_opt = parse_price_cents(price_str);
                            let quantity_opt =
                                crate::util::parse_quantity_smallest_unit(qty_str, 8);

                            if let (Some(price), Some(quantity)) = (price_opt, quantity_opt) {
                                info!(
                                    "[Binance] Ask: price={}, qty_str={}, quantity={}",
                                    price_str, qty_str, quantity
                                );
                                self.tx
                                    .send(ExchangePrice::Binance {
                                        price,
                                        quantity,
                                        exchange_timestamp,
                                        received_at,
                                    })
                                    .await
                                    .ok();
                            } else {
                                warn!("[Binance] Failed to parse ask: price_str={:?} (parsed: {:?}), qty_str={:?} (parsed: {:?})", 
                                    price_str, price_opt, qty_str, quantity_opt);
                            }
                        }
                    }
                }
            }
        } else {
            warn!("[Binance] No asks array in depthUpdate message");
        }

        Ok(())
    }
}
