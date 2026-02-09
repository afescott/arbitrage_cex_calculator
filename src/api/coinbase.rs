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
        
        // Debug: log first Coinbase message
        static FIRST_COINBASE_MSG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
        if FIRST_COINBASE_MSG.swap(false, std::sync::atomic::Ordering::Relaxed) {
            info!("[Coinbase] First message sample (first 300 chars): {}", &text[..text.len().min(300)]);
        }
        
        // Parse level2 update data
        let update: serde_json::Value = serde_json::from_str(text)?;
        
        // Handle subscription confirmation
        if let Some(msg_type) = update.get("type").and_then(|t| t.as_str()) {
            info!("[Coinbase] Message type: {}", msg_type);
            if msg_type == "subscriptions" {
                info!("[Coinbase] Subscription confirmed");
                return Ok(());
            }
            
            // Handle level2 snapshot (initial state)
            if msg_type == "snapshot" {
                info!("[Coinbase] Received snapshot message");
                let bids_opt = update.get("bids").and_then(|b| b.as_array());
                if bids_opt.is_none() {
                    warn!("[Coinbase] No 'bids' array in snapshot");
                }
                
                if let Some(bids) = bids_opt {
                    info!("[Coinbase] Processing {} bids from snapshot", bids.len());
                    for bid in bids {
                        if let Some(bid_array) = bid.as_array() {
                            if bid_array.len() >= 2 {
                                if let (Some(price_str), Some(size_str)) = (
                                    bid_array[0].as_str(),
                                    bid_array[1].as_str(),
                                ) {
                                    println!("[Coinbase] Snapshot bid: price_str={}, size_str={}", price_str, size_str);
                                    
                                    let price_opt = parse_price_cents(price_str);
                                    let quantity_opt = crate::util::parse_quantity_smallest_unit(size_str, 8);
                                    
                                    if let (Some(price), Some(quantity)) = (price_opt, quantity_opt) {
                                        self.tx.send(ExchangePrice::Coinbase {
                                            price,
                                            quantity,
                                            exchange_timestamp: None,
                                            received_at,
                                        }).await.ok();
                                    }
                                }
                            }
                        }
                    }
                }
                
                let asks_opt = update.get("asks").and_then(|a| a.as_array());
                if asks_opt.is_none() {
                    warn!("[Coinbase] No 'asks' array in snapshot");
                }
                
                if let Some(asks) = asks_opt {
                    info!("[Coinbase] Processing {} asks from snapshot", asks.len());
                    for ask in asks {
                        if let Some(ask_array) = ask.as_array() {
                            if ask_array.len() >= 2 {
                                if let (Some(price_str), Some(size_str)) = (
                                    ask_array[0].as_str(),
                                    ask_array[1].as_str(),
                                ) {
                                    println!("[Coinbase] Snapshot ask: price_str={}, size_str={}", price_str, size_str);
                                    
                                    let price_opt = parse_price_cents(price_str);
                                    let quantity_opt = crate::util::parse_quantity_smallest_unit(size_str, 8);
                                    
                                    if let (Some(price), Some(quantity)) = (price_opt, quantity_opt) {
                                        self.tx.send(ExchangePrice::Coinbase {
                                            price,
                                            quantity,
                                            exchange_timestamp: None,
                                            received_at,
                                        }).await.ok();
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(());
            }
            
            // Handle level2 updates (incremental changes)
            if msg_type == "l2update" {
                let changes_opt = update.get("changes").and_then(|c| c.as_array());
                if changes_opt.is_none() {
                    warn!("[Coinbase] No 'changes' array in l2update");
                }
                
                if let Some(changes) = changes_opt {
                    info!("[Coinbase] Processing {} changes from l2update", changes.len());
                    for change in changes {
                        if let Some(change_array) = change.as_array() {
                            if change_array.len() >= 3 {
                                if let (Some(side_str), Some(price_str), Some(size_str)) = (
                                    change_array[0].as_str(),
                                    change_array[1].as_str(),
                                    change_array[2].as_str(),
                                ) {
                                    println!("[Coinbase] l2update: side={}, price_str={}, size_str={}", side_str, price_str, size_str);
                                    
                                    // Skip if size is "0" (removal)
                                    if size_str == "0" {
                                        continue;
                                    }
                                    
                                    let price_opt = parse_price_cents(price_str);
                                    let quantity_opt = crate::util::parse_quantity_smallest_unit(size_str, 8);
                                    
                                    if let (Some(price), Some(quantity)) = (price_opt, quantity_opt) {
                                        self.tx.send(ExchangePrice::Coinbase {
                                            price,
                                            quantity,
                                            exchange_timestamp: None,
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
        
        Ok(())
    }
}

