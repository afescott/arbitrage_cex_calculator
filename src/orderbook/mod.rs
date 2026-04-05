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

use std::time::{SystemTime, UNIX_EPOCH};

use ::pricelevel::{MatchResult, Side as PriceLevelSide};

pub mod book;
mod modifications;

pub use modifications::OrderModification;

use crate::{
    api::{ExchangePrice, Side as ApiSide},
    calculation::fees::PurchaseOption,
    orderbook::book::Exchange,
};

pub struct BuyExchangeSellExchange {
    buy_price: u64,
    profit_expect: u64,
    sell_price: u64,
    sell_exchange: Exchange,
    buy_exchange: Exchange,
}

/// Apply one exchange price update: arbitrage check, then merge into the book.
async fn apply_exchange_price_update(
    orderbook: &book::OrderBook,
    exchange: book::Exchange,
    price: u64,
    quantity: u64,
    side: ApiSide,
    tx: &tokio::sync::mpsc::Sender<BuyExchangeSellExchange>,
) {
    let orderbook_side = match side {
        ApiSide::Buy => PriceLevelSide::Buy,
        ApiSide::Sell => PriceLevelSide::Sell,
    };
    orderbook
        .check_for_immediate_purchase(price, exchange, orderbook_side, quantity, tx)
        .await;
    orderbook.add_exchange_price_level(price, exchange, orderbook_side, quantity);
}

/// Spawns a task that consumes [`ExchangePrice`] messages and updates [`book::OrderBook`].
pub fn spawn_exchange_price_aggregator(
    orderbook: book::OrderBook,
    mut rx: tokio::sync::mpsc::Receiver<ExchangePrice>,
    tx: tokio::sync::mpsc::Sender<(book::Exchange, u64)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(price) = rx.recv().await {
            match price {
                ExchangePrice::Binance {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    let result = apply_exchange_price_update(
                        &orderbook,
                        book::Exchange::Binance,
                        price,
                        quantity,
                        side,
                        &tx,
                    )
                    .await;
                }
                ExchangePrice::Kraken {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    apply_exchange_price_update(
                        &orderbook,
                        book::Exchange::Kraken,
                        price,
                        quantity,
                        side,
                        &tx,
                    )
                    .await;
                }
                ExchangePrice::Coinbase {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    apply_exchange_price_update(
                        &orderbook,
                        book::Exchange::Coinbase,
                        price,
                        quantity,
                        side,
                        &tx,
                    )
                    .await;
                }
                ExchangePrice::Hyperliquid {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    let result = apply_exchange_price_update(
                        &orderbook,
                        book::Exchange::Hyperliquid,
                        price,
                        quantity,
                        side,
                        &tx,
                    )
                    .await;
                }
                _ => {} // For now, we only process Hyperliquid prices. In the future, we can add support for other exchanges.
            }
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
