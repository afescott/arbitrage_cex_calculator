//! # Order Book Module
//!
//! This module provides the core order book functionality for the matching engine.
//! It includes:
//! - Order book data structures and management
//! - Order matching and execution logic
//! - Price level management for bids and asks
//! - Order modification and cancellation operations
//! - Error handling for order book operations
//!
//! The module uses concurrent data structures for high-performance order processing
//! and supports both limit and market order types.
#![allow(dead_code)]

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ::pricelevel::{MatchResult, Side as PriceLevelSide};

pub mod book;
mod modifications;

use crate::{
    api::{ExchangePrice, Side as ApiSide},
    calculation::BuyExchangeSellExchange,
    telemetry::{Stage, Telemetry},
};

fn book_exchange_of(price: &ExchangePrice) -> book::Exchange {
    match price {
        ExchangePrice::Binance { .. } => book::Exchange::Binance,
        ExchangePrice::Kraken { .. } => book::Exchange::Kraken,
        ExchangePrice::Coinbase { .. } => book::Exchange::Coinbase,
        ExchangePrice::Hyperliquid { .. } => book::Exchange::Hyperliquid,
        #[cfg(feature = "dydx")]
        ExchangePrice::Dydx { .. } => book::Exchange::Dydx,
        ExchangePrice::Bitget { .. } => book::Exchange::Bitget,
    }
}

/// Apply one exchange price update: arbitrage check, then merge into the book.
fn apply_exchange_price_update(
    orderbook: &book::OrderBook,
    telemetry: &Telemetry,
    exchange: book::Exchange,
    price: u64,
    quantity: u64,
    side: ApiSide,
    received_at: Instant,
    tx: &tokio::sync::mpsc::Sender<BuyExchangeSellExchange>,
) {
    telemetry.record(Stage::WsToAggregator, received_at.elapsed());

    let orderbook_side = match side {
        ApiSide::Buy => PriceLevelSide::Buy,
        ApiSide::Sell => PriceLevelSide::Sell,
    };

    let t_arb = Instant::now();
    let maybe_route =
        orderbook.check_for_immediate_purchase(price, exchange, orderbook_side, quantity);
    telemetry.record(Stage::ArbDetect, t_arb.elapsed());

    if let Some(route) = maybe_route {
        telemetry.n_arb_found.fetch_add(1, Ordering::Relaxed);
        // Never block the aggregator on a slow purchase loop; drop stale routes instead.
        match tx.try_send(route) {
            Ok(()) => {
                telemetry.n_routes_emitted.fetch_add(1, Ordering::Relaxed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let n = telemetry
                    .n_routes_dropped_full
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                if n == 1 || n % 100 == 0 {
                    eprintln!(
                        "purchase route channel full; dropped {n} route(s) (purchase loop busy)"
                    );
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                eprintln!("purchase route channel closed");
            }
        }
    }

    let t_book = Instant::now();
    orderbook.add_exchange_price_level(price, exchange, orderbook_side, quantity);
    telemetry.record(Stage::BookUpdate, t_book.elapsed());
    telemetry.n_book_updates.fetch_add(1, Ordering::Relaxed);
}

/// Spawns a task that consumes [`ExchangePrice`] messages and updates [`book::OrderBook`].
pub fn spawn_exchange_price_aggregator(
    orderbook: book::OrderBook,
    telemetry: Arc<Telemetry>,
    mut rx: tokio::sync::mpsc::Receiver<ExchangePrice>,
    tx: tokio::sync::mpsc::Sender<BuyExchangeSellExchange>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(price) = rx.recv().await {
            telemetry.n_ws_msgs.fetch_add(1, Ordering::Relaxed);
            let exchange = book_exchange_of(&price);
            apply_exchange_price_update(
                &orderbook,
                &telemetry,
                exchange,
                price.price(),
                price.quantity(),
                price.side(),
                price.received_at(),
                &tx,
            );
        }
    })
}

#[derive(Debug)]
pub enum FillResponse {
    Fill(MatchResult),
    PartialFill(MatchResult),
    Error(anyhow::Error),
}

pub fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}
