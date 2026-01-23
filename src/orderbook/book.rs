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
use std::sync::{atomic::AtomicU64, Arc};
use uuid::Uuid;

#[warn(clippy::too_many_lines)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillType {
    /// Indicates a partial fill of an order
    Partial(Vec<OrderId>),

    /// Indicates a full fill of an order
    Full(Vec<OrderId>),
}

/// The OrderBook manages a collection of price levels for both bid and ask sides.
/// It supports adding, cancelling, and matching orders with lock-free operations where possible.
pub struct OrderBook {
    /// The symbol or identifier for this order book
    pub symbol: String,

    /// Bid side price levels (buy orders), stored in a concurrent map for lock-free access
    /// The map is keyed by price levels and stores Arc references to PriceLevel instances
    pub bids: DashMap<u64, Arc<PriceLevel>>,

    /// Ask side price levels (sell orders), stored in a concurrent map for lock-free access
    /// The map is keyed by price levels and stores Arc references to PriceLevel instances
    pub asks: DashMap<u64, Arc<PriceLevel>>,

    /// Price_last_traded_at
    pub last_traded_at: AtomicU64,

    /// Order ID assocaited with the price and side (BID/ASK)
    pub orders: DashMap<OrderId, (u64, Side)>,

    pub market_orders_bids: SegQueue<(OrderId, u64)>,

    pub market_orders_asks: SegQueue<(OrderId, u64)>,

    pub transaction_counter: AtomicU64,

    /// Cached best bid price for O(1) access
    pub cached_best_bid: AtomicU64,

    /// Cached best ask price for O(1) access  
    pub cached_best_ask: AtomicU64,

    /// Generator for unique transaction IDs
    pub transaction_id_generator: UuidGenerator,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: DashMap::new(),
            asks: DashMap::new(),
            orders: DashMap::new(),
            last_traded_at: AtomicU64::new(0),
            transaction_id_generator: UuidGenerator::new(Uuid::new_v4()),
            transaction_counter: AtomicU64::new(0),
            market_orders_asks: SegQueue::new(),
            market_orders_bids: SegQueue::new(),
            cached_best_bid: AtomicU64::new(0),
            cached_best_ask: AtomicU64::new(u64::MAX),
        }
    }

    pub fn best_bid(&self) -> Option<u64> {
        let cached = self
            .cached_best_bid
            .load(std::sync::atomic::Ordering::Relaxed);
        if cached > 0 {
            // Verify the cached price still has orders
            if let Some(price_level) = self.bids.get(&cached) {
                if price_level.order_count() > 0 {
                    return Some(cached);
                }
            }
        }

        // Fallback to full scan and update cache
        let mut best_price = None;
        for item in self.bids.iter() {
            let price = *item.key();
            let price_level = item.value();

            if price_level.order_count() > 0
                && (best_price.is_none() || price > best_price.unwrap())
            {
                best_price = Some(price);
            }
        }

        if let Some(price) = best_price {
            self.cached_best_bid
                .store(price, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.cached_best_bid
                .store(0, std::sync::atomic::Ordering::Relaxed);
        }

        best_price
    }

    pub fn best_ask(&self) -> Option<u64> {
        let cached = self
            .cached_best_ask
            .load(std::sync::atomic::Ordering::Relaxed);
        if cached < u64::MAX {
            // Verify the cached price still has orders
            if let Some(price_level) = self.asks.get(&cached) {
                if price_level.order_count() > 0 {
                    return Some(cached);
                }
            }
        }

        // Fallback to full scan and update cache
        let mut lowest_ask = None;
        for item in self.asks.iter() {
            let price = *item.key();
            let price_level = item.value();

            if price_level.order_count() > 0
                && (lowest_ask.is_none() || price < lowest_ask.unwrap())
            {
                lowest_ask = Some(price);
            }
        }

        if let Some(price) = lowest_ask {
            self.cached_best_ask
                .store(price, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.cached_best_ask
                .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        }

        lowest_ask
    }

    /// Update cached best bid when a new bid order is added
    pub fn update_cached_best_bid(&self, price: u64) {
        let current = self
            .cached_best_bid
            .load(std::sync::atomic::Ordering::Relaxed);
        if price > current {
            self.cached_best_bid
                .store(price, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Update cached best ask when a new ask order is added
    pub fn update_cached_best_ask(&self, price: u64) {
        let current = self
            .cached_best_ask
            .load(std::sync::atomic::Ordering::Relaxed);
        if price < current {
            self.cached_best_ask
                .store(price, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Invalidate cache when orders are removed (forces recalculation)
    pub fn invalidate_best_price_cache(&self) {
        self.cached_best_bid
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.cached_best_ask
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
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
