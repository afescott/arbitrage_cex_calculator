use crate::{
    api::{ExchangePrice, Side},
    util::parse_price_cents,
};
use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::args::Net;

pub struct HyperliquidClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
    network: Net,
}

impl HyperliquidClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>, network: Net) -> Self {
        HyperliquidClient { tx, network }
    }

    pub async fn listen_btc_usdt(&self) {
        let url = match self.network {
            Net::Testnet => "wss://api.hyperliquid-testnet.xyz/ws",
            Net::Mainnet => "wss://api.hyperliquid.xyz/ws",
        };
        loop {
            match self.run_one_connection(url).await {
                Ok(()) => eprintln!("Hyperliquid WS: connection ended, reconnecting…"),
                Err(e) => eprintln!("Hyperliquid WS: {e}, reconnecting…"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn run_one_connection(&self, url: &'static str) -> Result<(), &'static str> {
        let (mut ws_stream, _) = connect_async(url)
            .await
            .map_err(|_| "connect failed")?;

        let subscribe_msg = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "l2Book",
                "coin": "BTC"
            }
        });

        ws_stream
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|_| "subscribe send failed")?;

        let (mut write, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let received_at = Instant::now();
                    let _ = self.handle_message(&text, received_at).await;
                }
                Ok(Message::Ping(data)) => {
                    let _ = write.send(Message::Pong(data)).await;
                }
                Ok(Message::Close(_)) => return Ok(()),
                Err(_) => return Err("websocket error"),
                _ => {}
            }
        }
        Err("read stream ended")
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
