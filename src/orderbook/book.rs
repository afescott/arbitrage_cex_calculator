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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exchange {
    Binance,
    Coinbase,
    Kraken,
}

/// The OrderBook manages a collection of price levels for both bid and ask sides.
/// It supports adding, cancelling, and matching orders with lock-free operations where possible.
pub struct OrderBook {
    /// The symbol or identifier for this order book
    pub symbol: String,
    // BTreeMap keeps prices sorted (bids: highest first, asks: lowest first) and maps price → quantity.
    pub exchange_bids_price_level: DashMap<(u64, Exchange), BTreeMap<u64, u64>>,

    pub exchange_asks_price_level: DashMap<(u64, Exchange), BTreeMap<u64, u64>>,

    pub cached_best_bid: DashMap<Exchange, AtomicU64>,

    pub cached_best_ask: DashMap<Exchange, AtomicU64>,

    /// Best bid across all exchanges. Returns None if no data available.
    /// The tuple contains (exchange, price), where price of 0 means no data.
    pub best_bid_all_exchanges: (Exchange, AtomicU64),

    /// Best ask across all exchanges. Returns None if no data available.
    /// The tuple contains (exchange, price), where price of 0 means no data.
    pub best_ask_all_exchanges: (Exchange, AtomicU64),
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            exchange_bids_price_level: DashMap::new(),
            exchange_asks_price_level: DashMap::new(),
            cached_best_bid: DashMap::new(),
            cached_best_ask: DashMap::new(),
            best_bid_all_exchanges: (Exchange::Binance, AtomicU64::new(0)),
            best_ask_all_exchanges: (Exchange::Binance, AtomicU64::new(0)),
        }
    }

    pub fn best_bid(&self, exchange: Exchange) -> Option<u64> {
        let best_bid = self.cached_best_bid.get(&exchange)?;

        Some(best_bid.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn best_ask(&self, exchange: Exchange) -> Option<u64> {
        let best_ask = self.cached_best_ask.get(&exchange)?;

        Some(best_ask.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Returns the best bid price across all exchanges, or None if no data is available.
    /// A price of 0 is treated as "no data" since it's invalid for trading.
    pub fn best_bid_all_exchanges(&self) -> Option<(u64, Exchange)> {
        let price = self
            .best_bid_all_exchanges
            .1
            .load(std::sync::atomic::Ordering::Relaxed);
        if price == 0 {
            None
        } else {
            Some((price, self.best_bid_all_exchanges.0))
        }
    }

    /// Returns the best ask price across all exchanges, or None if no data is available.
    /// A price of 0 is treated as "no data" since it's invalid for trading.
    pub fn best_ask_all_exchanges(&self) -> Option<(u64, Exchange)> {
        let price = self
            .best_ask_all_exchanges
            .1
            .load(std::sync::atomic::Ordering::Relaxed);
        if price == 0 {
            None
        } else {
            Some((price, self.best_ask_all_exchanges.0))
        }
    }

    pub fn check_for_immediate_purchase(
        &self,
        price: u64,
        exchange: Exchange,
        side: Side,
        quantity: u64,
    ) {
        match side {
            Side::Buy => {
                let val = self.best_ask_all_exchanges();
                if let Some(best_ask_exchange) = val {
                    if best_ask_exchange.1 != exchange && best_ask_exchange.0 < price {
                        println!("Best ask: {:?} from exchange: {:?}, is better higher than our bid: {:?}, from exchange: {:?}",
                            best_ask_exchange.0, best_ask_exchange.1, exchange, price);
                    }
                }
            }
            Side::Sell => {
                let val = self.best_bid_all_exchanges();
                if let Some(best_bid_exchange) = val {
                    if best_bid_exchange.1 != exchange && best_bid_exchange.0 > price {}
                    println!("Best bid: {:?} from exchange: {:?}, is better lower than our ask: {:?}, from exchange: {:?}",
                            best_bid_exchange.0, best_bid_exchange.1, exchange, price);
                }
            }
        }
    }

    pub fn add_exchange_price_level(
        &self,
        price: u64,
        exchange: Exchange,
        side: Side,
        quantity: u64,
    ) {
        match side {
            Side::Buy => {
                let key = (price, exchange);
                let mut price_level = self
                    .exchange_bids_price_level
                    .entry(key)
                    .or_insert_with(BTreeMap::new);
                let entry = price_level.entry(price).or_insert(0);
                *entry += quantity;
            }
            Side::Sell => {
                let key = (price, exchange);
                let mut price_level = self
                    .exchange_asks_price_level
                    .entry(key)
                    .or_insert_with(BTreeMap::new);
                let entry = price_level.entry(price).or_insert(0);
                *entry += quantity;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::{sync::Arc, time::Duration};

    use pricelevel::Side;
    use tokio::sync::mpsc::channel;

    use crate::orderbook::book::{ExchangePriceList, OrderBook};

    #[test]
    fn test_add_exchange_price_level_different_exchanges() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add same price to different exchanges - should be separate
        order_book.add_exchange_price_level(50000, ExchangePriceList::Binance, Side::Buy, 10);
        order_book.add_exchange_price_level(50000, ExchangePriceList::Coinbase, Side::Buy, 20);

        let binance_key = (50000, ExchangePriceList::Binance);
        let coinbase_key = (50000, ExchangePriceList::Coinbase);

        assert!(order_book
            .exchange_bids_price_level
            .contains_key(&binance_key));
        assert!(order_book
            .exchange_bids_price_level
            .contains_key(&coinbase_key));

        let binance_level = order_book
            .exchange_bids_price_level
            .get(&binance_key)
            .unwrap();
        let coinbase_level = order_book
            .exchange_bids_price_level
            .get(&coinbase_key)
            .unwrap();

        assert_eq!(binance_level.get(&50000), Some(&10));
        assert_eq!(coinbase_level.get(&50000), Some(&20));
    }

    #[test]
    fn test_add_exchange_price_level_bid() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add bid for Binance
        order_book.add_exchange_price_level(50000, ExchangePriceList::Binance, Side::Buy, 10);

        let key = (50000, ExchangePriceList::Binance);
        assert!(order_book.exchange_bids_price_level.contains_key(&key));

        let price_level = order_book.exchange_bids_price_level.get(&key).unwrap();
        assert_eq!(price_level.get(&50000), Some(&10));
    }

    #[test]
    fn test_add_exchange_price_level_ask() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add ask for Coinbase
        order_book.add_exchange_price_level(50100, ExchangePriceList::Coinbase, Side::Sell, 5);

        let key = (50100, ExchangePriceList::Coinbase);
        assert!(order_book.exchange_asks_price_level.contains_key(&key));

        let price_level = order_book.exchange_asks_price_level.get(&key).unwrap();
        assert_eq!(price_level.get(&50100), Some(&5));
    }

    #[test]
    fn test_add_exchange_price_level_quantity_accumulation() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add same price level multiple times - quantities should accumulate
        order_book.add_exchange_price_level(50000, ExchangePriceList::Binance, Side::Buy, 10);
        order_book.add_exchange_price_level(50000, ExchangePriceList::Binance, Side::Buy, 5);
        order_book.add_exchange_price_level(50000, ExchangePriceList::Binance, Side::Buy, 3);

        let key = (50000, ExchangePriceList::Binance);
        let price_level = order_book.exchange_bids_price_level.get(&key).unwrap();
        assert_eq!(price_level.get(&50000), Some(&18)); // 10 + 5 + 3
    }

    #[tokio::test]
    async fn test_add_exchange_price_level_concurrent() {
        let order_book = Arc::new(OrderBook::new("ETH/USD".to_string()));
        let book_1 = Arc::clone(&order_book);
        let book_2 = Arc::clone(&order_book);
        let (tx, mut rx) = channel::<u64>(1);

        let task = tokio::spawn(async move {
            book_1.add_exchange_price_level(2000, ExchangePriceList::Binance, Side::Sell, 13);

            let key = (2000, ExchangePriceList::Binance);
            let price_level = book_1.exchange_asks_price_level.get(&key).unwrap();
            let quantity = price_level.get(&2000).unwrap();

            tokio::time::sleep(Duration::from_secs(1)).await;

            tx.send(*quantity).await.unwrap();
        });

        let task_2 = tokio::spawn(async move {
            book_2.add_exchange_price_level(2000, ExchangePriceList::Binance, Side::Sell, 13);
        });

        tokio::join!(task, task_2);

        while let Some(val) = rx.recv().await {
            // Quantities should accumulate: 13 + 13 = 26
            assert_eq!(val, 26);
        }
    }
}
