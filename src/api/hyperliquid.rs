use crate::{
    api::{ExchangePrice, Side},
    util::parse_price_cents,
};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn hyperliquid_ws_url(network: &str) -> &'static str {
    match network {
        "testnet" => "wss://api.hyperliquid-testnet.xyz/ws",
        _ => "wss://api.hyperliquid.xyz/ws",
    }
}

pub struct HyperliquidClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
    network: String,
}

impl HyperliquidClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>, network: String) -> Self {
        HyperliquidClient { tx, network }
    }

    pub async fn listen_btc_usdt(&self) {
        // info!("[Hyperliquid] Connecting to BTC perpetual futures price feed...");

        let url = hyperliquid_ws_url(&self.network);
        match connect_async(url).await {
            Ok((mut ws_stream, _)) => {
                // info!("[Hyperliquid] Connected successfully");

                // Subscribe to BTC perpetual orderbook (l2Book)
                // Hyperliquid uses "BTC" as the coin name for Bitcoin perpetual
                // Format: {"method": "subscribe", "subscription": {"type": "l2Book", "coin": "BTC"}}
                let subscribe_msg = serde_json::json!({
                    "method": "subscribe",
                    "subscription": {
                        "type": "l2Book",
                        "coin": "BTC"
                    }
                });

                // Send subscription message
                if let Err(e) = ws_stream
                    .send(Message::Text(subscribe_msg.to_string()))
                    .await
                {
                    // error!("[Hyperliquid] Failed to send subscription: {}", e);
                    return;
                }

                let (_write, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Capture timestamp immediately when message received
                            let received_at = Instant::now();
                            if let Err(e) = self.handle_message(&text, received_at).await {
                                // warn!("[Hyperliquid] Error handling message: {}", e);
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            // info!("[Hyperliquid] Received ping");
                        }
                        Ok(Message::Close(_)) => {
                            // warn!("[Hyperliquid] Connection closed");
                            break;
                        }
                        Err(e) => {
                            // error!("[Hyperliquid] WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                // error!("[Hyperliquid] Failed to connect: {}", e);
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

        // Parse Hyperliquid message
        let value: serde_json::Value = serde_json::from_str(text)?;

        // Handle subscription confirmation
        // Format: {"channel":"subscriptionResponse","data":{"method":"subscribe","subscription":{...}}}
        if let Some(channel) = value.get("channel").and_then(|c| c.as_str()) {
            if channel == "subscriptionResponse" {
                // info!("[Hyperliquid] Subscription confirmed");
                return Ok(());
            }
            // Data messages have channel set to the subscription type (e.g., "l2Book")
            // Continue processing below
        }

        // Handle l2Book data
        // Format: {"channel":"l2Book","data":{"levels":[[bids],[asks]],"time":timestamp_ms,"coin":"BTC"}}
        // Where bids/asks are arrays of objects: {"px": "45000.50", "sz": "1.5", "n": 3}
        let data_obj = value
            .get("data")
            .and_then(|d| d.as_object())
            .or_else(|| value.as_object());

        if let Some(data) = data_obj {
            if let Some(coin) = data
                .get("coin")
                .or_else(|| value.get("coin"))
                .and_then(|c| c.as_str())
            {
                if coin == "BTC" {
                    let exchange_timestamp = data
                        .get("time")
                        .or_else(|| value.get("time"))
                        .and_then(|t| t.as_u64());

                    // Process levels array: [bids_array, asks_array]
                    if let Some(levels) = data
                        .get("levels")
                        .or_else(|| value.get("levels"))
                        .and_then(|l| l.as_array())
                    {
                        if levels.len() >= 2 {
                            // Process bids (first element) - buy side
                            if let Some(bids) = levels[0].as_array() {
                                for bid_obj in bids {
                                    if let Some(bid) = bid_obj.as_object() {
                                        if let (Some(px_str), Some(sz_str)) = (
                                            bid.get("px").and_then(|p| p.as_str()),
                                            bid.get("sz").and_then(|s| s.as_str()),
                                        ) {
                                            let price_opt = parse_price_cents(px_str);
                                            let quantity_opt =
                                                crate::util::parse_quantity_smallest_unit(
                                                    sz_str, 8,
                                                );

                                            if let (Some(price), Some(quantity)) =
                                                (price_opt, quantity_opt)
                                            {
                                                self.tx
                                                    .send(ExchangePrice::Hyperliquid {
                                                        price,
                                                        quantity,
                                                        exchange_timestamp,
                                                        received_at,
                                                        side: Side::Buy, // Bids are Buy side
                                                    })
                                                    .await
                                                    .ok();
                                            }
                                        }
                                    }
                                }
                            }

                            // Process asks (second element) - sell side
                            if let Some(asks) = levels[1].as_array() {
                                for ask_obj in asks {
                                    if let Some(ask) = ask_obj.as_object() {
                                        if let (Some(px_str), Some(sz_str)) = (
                                            ask.get("px").and_then(|p| p.as_str()),
                                            ask.get("sz").and_then(|s| s.as_str()),
                                        ) {
                                            let price_opt = parse_price_cents(px_str);
                                            let quantity_opt =
                                                crate::util::parse_quantity_smallest_unit(
                                                    sz_str, 8,
                                                );

                                            if let (Some(price), Some(quantity)) =
                                                (price_opt, quantity_opt)
                                            {
                                                self.tx
                                                    .send(ExchangePrice::Hyperliquid {
                                                        price,
                                                        quantity,
                                                        exchange_timestamp,
                                                        received_at,
                                                        side: Side::Sell, // Asks are Sell side
                                                    })
                                                    .await
                                                    .ok();
                                            }
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
