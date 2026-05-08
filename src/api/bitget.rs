//! Bitget mix (perps) public orderbook feed for BTCUSDT.
//!
//! Uses WebSocket public endpoint and subscribes to `books5` for USDT-FUTURES.

use crate::{
    api::{ExchangePrice, Side},
    util::parse_price_cents,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const BITGET_WS_PUBLIC: &str = "wss://ws.bitget.com/v2/ws/public";

pub struct BitgetClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

impl BitgetClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        BitgetClient { tx }
    }

    pub async fn listen_btc_usdt(&self) {
        match connect_async(BITGET_WS_PUBLIC).await {
            Ok((mut ws_stream, _)) => {
                let subscribe_msg = serde_json::json!({
                    "op": "subscribe",
                    "args": [
                        {
                            "instType": "USDT-FUTURES",
                            "channel": "books5",
                            "instId": "BTCUSDT"
                        }
                    ]
                });
                let _ = ws_stream
                    .send(Message::Text(subscribe_msg.to_string()))
                    .await;

                let (_write, mut read) = ws_stream.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            let received_at = Instant::now();
                            let _ = self.handle_message(&text, received_at).await;
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
    ) -> Result<(), Box<dyn std::error::Error>> {
        if text.len() > 200_000 {
            return Err("bitget ws message too large".into());
        }
        if text == "pong" || text == "ping" {
            return Ok(());
        }

        let v: Value = serde_json::from_str(text)?;

        // Push format:
        // { "action":"snapshot|update", "data":[{"bids":[["27000","1.2"],...], "asks":[...], "ts":"169..." }], "ts": 169... }
        let data0 = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first());
        let ts = data0
            .and_then(|d| d.get("ts"))
            .and_then(|t| t.as_str())
            .and_then(|s| s.parse::<u64>().ok());

        let bids = data0.and_then(|d| d.get("bids")).and_then(|b| b.as_array());
        if let Some(levels) = bids {
            for lvl in levels.iter().take(10) {
                if let Some(arr) = lvl.as_array() {
                    if arr.len() >= 2 {
                        let p = arr[0].as_str().unwrap_or("");
                        let q = arr[1].as_str().unwrap_or("");
                        if let (Some(price), Some(quantity)) = (
                            parse_price_cents(p),
                            crate::util::parse_quantity_smallest_unit(q, 8),
                        ) {
                            let _ = self
                                .tx
                                .send(ExchangePrice::Bitget {
                                    price,
                                    quantity,
                                    exchange_timestamp: ts,
                                    received_at,
                                    side: Side::Buy,
                                })
                                .await;
                        }
                    }
                }
            }
        }

        let asks = data0.and_then(|d| d.get("asks")).and_then(|a| a.as_array());
        if let Some(levels) = asks {
            for lvl in levels.iter().take(10) {
                if let Some(arr) = lvl.as_array() {
                    if arr.len() >= 2 {
                        let p = arr[0].as_str().unwrap_or("");
                        let q = arr[1].as_str().unwrap_or("");
                        if let (Some(price), Some(quantity)) = (
                            parse_price_cents(p),
                            crate::util::parse_quantity_smallest_unit(q, 8),
                        ) {
                            let _ = self
                                .tx
                                .send(ExchangePrice::Bitget {
                                    price,
                                    quantity,
                                    exchange_timestamp: ts,
                                    received_at,
                                    side: Side::Sell,
                                })
                                .await;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

