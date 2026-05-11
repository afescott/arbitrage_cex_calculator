//! Bitget mix (perps) public orderbook feed for BTCUSDT.
//!
//! Uses WebSocket public endpoint and subscribes to `books5` for USDT-FUTURES.

use crate::{
    api::{ExchangePrice, Side},
    util::parse_price_cents,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::time::{interval, MissedTickBehavior};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const BITGET_WS_PUBLIC: &str = "wss://ws.bitget.com/v2/ws/public";

pub struct BitgetClient {
    tx: tokio::sync::mpsc::Sender<ExchangePrice>,
}

static BITGET_FIRST_BOOK_PRINTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

impl BitgetClient {
    pub fn new(tx: tokio::sync::mpsc::Sender<ExchangePrice>) -> Self {
        BitgetClient { tx }
    }

    pub async fn listen_btc_usdt(&self) {
        loop {
            match self.run_one_connection().await {
                Ok(()) => eprintln!("Bitget WS: connection ended, reconnecting…"),
                Err(e) => eprintln!("Bitget WS: {e}, reconnecting…"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// One WebSocket session until disconnect/error. Must answer frame pings and Bitget `"ping"`.
    async fn run_one_connection(&self) -> Result<(), &'static str> {
        let (mut ws_stream, _) = connect_async(BITGET_WS_PUBLIC)
            .await
            .map_err(|_| "connect failed")?;

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
        ws_stream
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|_| "subscribe send failed")?;

        let (mut write, mut read) = ws_stream.split();

        let mut keepalive = interval(Duration::from_secs(25));
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = keepalive.tick() => {
                    let _ = write.send(Message::Text("ping".into())).await;
                }
                msg = read.next() => {
                    let Some(msg) = msg else { return Err("read stream ended"); };
                    match msg {
                        Ok(Message::Text(text)) => {
                            if text == "ping" {
                                let _ = write.send(Message::Text("pong".into())).await;
                                continue;
                            }
                            if text == "pong" {
                                continue;
                            }
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
            }
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
        let v: Value = serde_json::from_str(text)?;

        // Push format:
        // { "action":"snapshot|update", "data":[{"bids":[["27000","1.2"],...], "asks":[...], "ts":"169..." }], "ts": 169... }
        let data0 = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first());
        let ts = data0
            .and_then(|d| d.get("ts"))
            .and_then(|t| t.as_str())
            .and_then(|s| s.parse::<u64>().ok());

        let bids = data0.and_then(|d| d.get("bids")).and_then(|b| b.as_array());
        let mut best_bid: Option<(u64, u64)> = None;
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
                            if best_bid.is_none() {
                                best_bid = Some((price, quantity));
                            }
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
        let mut best_ask: Option<(u64, u64)> = None;
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
                            if best_ask.is_none() {
                                best_ask = Some((price, quantity));
                            }
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

        if let (Some((bp, bq)), Some((ap, aq))) = (best_bid, best_ask) {
            if !BITGET_FIRST_BOOK_PRINTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "Bitget books5 BTCUSDT: best_bid={} (qty_e8={}) best_ask={} (qty_e8={}) ts={:?}",
                    bp, bq, ap, aq, ts
                );
            }
        }
        Ok(())
    }
}

