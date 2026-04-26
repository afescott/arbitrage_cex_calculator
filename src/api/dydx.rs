//! dYdX v4 [indexer](https://docs.dydx.xyz/interaction/endpoints) WebSocket order book feed.
//!
//! Subscribes to `v4_orderbook` for `BTC-USD` (perpetuals). Message shapes follow the public docs:
//! `contents.bids` / `contents.asks` as objects `{ "price", "size" }` or arrays `[price, size, offset]`.

use crate::{
    api::{ExchangePrice, Side},
    util::parse_price_cents,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::args::Net;

pub struct DydxClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
    network: Net,
}

impl DydxClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>, network: Net) -> Self {
        DydxClient { tx, network }
    }

    /// Subscribe to BTC-USD perpetual order book (`BTC-USD` market id on v4).
    pub async fn listen_btc_usdt(&self) {
        let url = match self.network {
            Net::Testnet => "wss://indexer.v4testnet.dydx.exchange/v4/ws",
            Net::Mainnet => "wss://indexer.dydx.trade/v4/ws",
        };
        match connect_async(url).await {
            Ok((ws_stream, _)) => {
                let (mut write, mut read) = ws_stream.split();
                let mut subscribed = false;

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            let received_at = Instant::now();
                            if let Err(e) = self
                                .handle_message(&text, received_at, &mut write, &mut subscribed)
                                .await
                            {
                                let _ = e;
                            }
                        }
                        Ok(Message::Ping(_)) => {}
                        Ok(Message::Close(_)) => break,
                        Err(_) => break,
                        _ => {}
                    }
                }
            }
            Err(_) => {}
        }
    }

    async fn handle_message(
        &self,
        text: &str,
        received_at: Instant,
        write: &mut (impl SinkExt<Message> + Unpin),
        subscribed: &mut bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if text.len() > 100_000 {
            return Err("Message too large".into());
        }

        let value: Value = serde_json::from_str(text)?;

        if value.get("type").and_then(|t| t.as_str()) == Some("connected") && !*subscribed {
            let sub = serde_json::json!({
                "type": "subscribe",
                "channel": "v4_orderbook",
                "id": "BTC-USD",
                "batched": false
            });
            write
                .send(Message::Text(sub.to_string()))
                .await
                .map_err(|_| "dydx: failed to send subscribe")?;
            *subscribed = true;
            return Ok(());
        }

        if value.get("channel").and_then(|c| c.as_str()) != Some("v4_orderbook") {
            return Ok(());
        }

        let exchange_timestamp = value
            .get("message_id")
            .and_then(|m| m.as_u64())
            .or_else(|| {
                value
                    .get("message_id")
                    .and_then(|m| m.as_i64())
                    .map(|i| i as u64)
            });

        let contents = match value.get("contents") {
            Some(c) => c,
            None => return Ok(()),
        };

        if let Some(bids) = contents.get("bids").and_then(|b| b.as_array()) {
            for level in bids {
                if let Some((price_s, size_s)) = orderbook_level_price_size(level) {
                    if is_zero_size(&size_s) {
                        continue;
                    }
                    if let (Some(price), Some(quantity)) = (
                        parse_price_cents(&price_s),
                        crate::util::parse_quantity_smallest_unit(&size_s, 8),
                    ) {
                        self.tx
                            .send(ExchangePrice::Dydx {
                                price,
                                quantity,
                                exchange_timestamp,
                                received_at,
                                side: Side::Buy,
                            })
                            .await
                            .ok();
                    }
                }
            }
        }

        if let Some(asks) = contents.get("asks").and_then(|a| a.as_array()) {
            for level in asks {
                if let Some((price_s, size_s)) = orderbook_level_price_size(level) {
                    if is_zero_size(&size_s) {
                        continue;
                    }
                    if let (Some(price), Some(quantity)) = (
                        parse_price_cents(&price_s),
                        crate::util::parse_quantity_smallest_unit(&size_s, 8),
                    ) {
                        self.tx
                            .send(ExchangePrice::Dydx {
                                price,
                                quantity,
                                exchange_timestamp,
                                received_at,
                                side: Side::Sell,
                            })
                            .await
                            .ok();
                    }
                }
            }
        }

        Ok(())
    }
}

fn json_scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn orderbook_level_price_size(level: &Value) -> Option<(String, String)> {
    if let Some(obj) = level.as_object() {
        let p = json_scalar_to_string(obj.get("price")?)?;
        let s = json_scalar_to_string(obj.get("size")?)?;
        return Some((p, s));
    }
    if let Some(arr) = level.as_array() {
        if arr.len() >= 2 {
            let p = json_scalar_to_string(&arr[0])?;
            let s = json_scalar_to_string(&arr[1])?;
            return Some((p, s));
        }
    }
    None
}

fn is_zero_size(s: &str) -> bool {
    s.parse::<f64>().map(|x| x <= 0.0).unwrap_or(true)
}
