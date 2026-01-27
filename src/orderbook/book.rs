//! # Order Book Data Structures
//!
//! This module defines the core OrderBook data structure and its associated types.
//! The OrderBook manages:
//! - Concurrent bid and ask price levels using DashMap for lock-free operations
//! - Order tracking and management with unique order IDs
//! - Best bid/ask price calculation
//! - Transaction ID generation for order matching
//! - Last traded timestamp tracking
//!
//! The implementation uses concurrent data structures to support high-throughput
//! order processing in a multi-threaded environment.

use crossbeam_queue::SegQueue;
use dashmap::DashMap;
use pricelevel::{OrderId, PriceLevel, Side, UuidGenerator};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicU64, Arc},
};
use uuid::Uuid;

#[warn(clippy::too_many_lines)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillType {
    /// Indicates a partial fill of an order
    Partial(Vec<OrderId>),

    /// Indicates a full fill of an order
    Full(Vec<OrderId>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExchangePriceList {
    Binance,
    Coinbase,
}

/// The OrderBook manages a collection of price levels for both bid and ask sides.
/// It supports adding, cancelling, and matching orders with lock-free operations where possible.
pub struct OrderBook {
    /// The symbol or identifier for this order book
    pub symbol: String,
    // BTreeMap keeps prices sorted (bids: highest first, asks: lowest first) and maps price → quantity.
    pub exchange_bids_price_level: DashMap<(u64, ExchangePriceList), BTreeMap<u64, u64>>,

    pub exchange_asks_price_level: DashMap<(u64, ExchangePriceList), BTreeMap<u64, u64>>,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            exchange_bids_price_level: DashMap::new(),
            exchange_asks_price_level: DashMap::new(),
        }
    }

    pub fn add_exchange_price_level(
        &self,
        price: u64,
        exchange: ExchangePriceList,
        side: Side,
        quantity: u64,
    ) {
        let entry = match side {
            Side::Buy => {
                let key = (price, exchange);
                let mut price_level = self
                    .exchange_bids_price_level
                    .entry(key)
                    .or_insert_with(BTreeMap::new);
                let entry = price_level.entry(price).or_insert(0);
                *entry += quantity
            }
            Side::Sell => {
                let key = (price, exchange);
                let mut price_level = self
                    .exchange_asks_price_level
                    .entry(key)
                    .or_insert_with(BTreeMap::new);
                let entry = price_level.entry(price).or_insert(0);
                *entry += quantity
            }
        };
        /* if entry as u64 < quantity {
            println!(
                "Overflow occurred when adding quantity: {} at price: {} on side: {:?}",
                quantity, price, side
            );
        } */
    }
}

#[cfg(test)]
mod test {
    use std::{sync::Arc, time::Duration};

    use pricelevel::OrderId;
    use tokio::sync::mpsc::channel;
    use uuid::Uuid;

    use crate::orderbook::book::OrderBook;

    #[test]
    fn test_order_book_add_limit_order() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        let order_id = OrderId::default();
        let result = order_book.add_to_limit_order(order_id, 100, 1, pricelevel::Side::Buy);
        assert!(result.is_ok());

        assert!(order_book.bids.contains_key(&100));
        assert!(order_book.asks.is_empty());

        order_book
            .submit_market_order(
                pricelevel::OrderId::Uuid(Uuid::new_v4()),
                1,
                pricelevel::Side::Sell,
            )
            .unwrap();

        //Bid successfully removed
        assert!(!order_book.bids.contains_key(&100));

        order_book
            .add_to_limit_order(order_id, 50, 6, pricelevel::Side::Buy)
            .unwrap();

        order_book
            .submit_market_order(
                pricelevel::OrderId::Uuid(Uuid::new_v4()),
                4,
                pricelevel::Side::Sell,
            )
            .unwrap();

        assert!(order_book.bids.contains_key(&50));
        //remaining quantity should be 2
        assert_eq!(order_book.bids.get(&50).unwrap().total_quantity(), 2);
    }

    #[tokio::test]
    async fn test_order_book_submit_market_order() {
        let order_book = Arc::new(OrderBook::new("ETH/USD".to_string()));
        let book_1 = Arc::clone(&order_book);
        let book_2 = Arc::clone(&order_book);
        let (tx, mut rx) = channel::<u64>(1);
        let order_id = OrderId::default();
        let task = tokio::spawn(async move {
            book_1
                .add_to_limit_order(order_id, 2000, 13, pricelevel::Side::Sell)
                .unwrap();

            assert_eq!(
                book_1.asks.iter().next().unwrap().value().total_quantity(),
                13
            );

            tokio::time::sleep(Duration::from_secs(1)).await;

            tx.send(book_1.asks.iter().next().unwrap().value().total_quantity())
                .await
                .unwrap();
        });

        let task_2 = tokio::spawn(async move {
            book_2
                .add_to_limit_order(order_id, 2000, 13, pricelevel::Side::Sell)
                .unwrap();
        });

        tokio::join!(task, task_2);

        while let Some(val) = rx.recv().await {
            assert_eq!(val, 26);
        }
    }
}
