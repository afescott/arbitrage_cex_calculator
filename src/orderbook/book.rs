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

#![allow(dead_code)]

use crate::calculation::{ArbitrageDetector, BuyExchangeSellExchange};
use dashmap::DashMap;
use pricelevel::{OrderId, Side};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicU64, Arc, RwLock},
};

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
    Hyperliquid,
}

/// Represents the best price level with both price and quantity.
/// Uses AtomicU64 for lock-free reads in low-latency scenarios.
#[derive(Default)]
pub struct BestPriceLevel {
    pub price: AtomicU64,
    pub quantity: AtomicU64,
}

/// The OrderBook manages a collection of price levels for both bid and ask sides.
/// It supports adding, cancelling, and matching orders with lock-free operations where possible.
pub struct OrderBook {
    /// The symbol or identifier for this order book
    pub symbol: String,
    // BTreeMap keeps prices sorted (bids: highest first, asks: lowest first) and maps price → quantity.
    pub exchange_bids_price_level: DashMap<Exchange, Arc<RwLock<BTreeMap<u64, u64>>>>,
    // One BTreeMap per exchange, sorted by price,
    pub exchange_asks_price_level: DashMap<Exchange, Arc<RwLock<BTreeMap<u64, u64>>>>,

    pub cached_best_bid: DashMap<Exchange, BestPriceLevel>,

    pub cached_best_ask: DashMap<Exchange, BestPriceLevel>,

    /// Best bid across all exchanges. Returns None if no data available.
    /// The tuple contains (exchange, price, quantity), where price of 0 means no data.
    pub best_bid_all_exchanges: Arc<std::sync::Mutex<(Exchange, BestPriceLevel)>>,

    /// Best ask across all exchanges. Returns None if no data available.
    /// The tuple contains (exchange, price, quantity), where price of 0 means no data.
    pub best_ask_all_exchanges: Arc<std::sync::Mutex<(Exchange, BestPriceLevel)>>,

    /// Arbitrage detector for identifying profitable opportunities
    arbitrage_detector: ArbitrageDetector,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            exchange_bids_price_level: DashMap::new(),
            exchange_asks_price_level: DashMap::new(),
            cached_best_bid: DashMap::new(),
            cached_best_ask: DashMap::new(),
            best_bid_all_exchanges: Arc::new(std::sync::Mutex::new((
                Exchange::Binance,
                BestPriceLevel::default(),
            ))),
            best_ask_all_exchanges: Arc::new(std::sync::Mutex::new((
                Exchange::Binance,
                BestPriceLevel::default(),
            ))),
            arbitrage_detector: ArbitrageDetector::default(),
        }
    }

    pub fn best_bid(&self, exchange: Exchange) -> Option<(u64, u64)> {
        let best_bid = self.cached_best_bid.get(&exchange)?;

        let price = best_bid.price.load(std::sync::atomic::Ordering::Relaxed);
        let quantity = best_bid.quantity.load(std::sync::atomic::Ordering::Relaxed);

        if price == 0 {
            None
        } else {
            Some((price, quantity))
        }
    }

    pub fn best_ask(&self, exchange: Exchange) -> Option<(u64, u64)> {
        let best_ask = self.cached_best_ask.get(&exchange)?;

        let price = best_ask.price.load(std::sync::atomic::Ordering::Relaxed);
        let quantity = best_ask.quantity.load(std::sync::atomic::Ordering::Relaxed);

        if price == 0 {
            None
        } else {
            Some((price, quantity))
        }
    }

    /// Returns the best bid price and quantity across all exchanges, or None if no data is available.
    /// A price of 0 is treated as "no data" since it's invalid for trading.
    /// Returns (price, quantity, exchange).
    pub fn best_bid_all_exchanges(&self) -> Option<(u64, u64, Exchange)> {
        let guard = self.best_bid_all_exchanges.lock().unwrap();
        let price = guard.1.price.load(std::sync::atomic::Ordering::Relaxed);
        if price == 0 {
            None
        } else {
            let quantity = guard.1.quantity.load(std::sync::atomic::Ordering::Relaxed);
            Some((price, quantity, guard.0))
        }
    }

    /// Returns the best ask price and quantity across all exchanges, or None if no data is available.
    /// A price of 0 is treated as "no data" since it's invalid for trading.
    /// Returns (price, quantity, exchange).
    pub fn best_ask_all_exchanges(&self) -> Option<(u64, u64, Exchange)> {
        let guard = self.best_ask_all_exchanges.lock().unwrap();
        let price = guard.1.price.load(std::sync::atomic::Ordering::Relaxed);
        if price == 0 {
            None
        } else {
            let quantity = guard.1.quantity.load(std::sync::atomic::Ordering::Relaxed);
            Some((price, quantity, guard.0))
        }
    }

    /// Check if the orderbook has sufficient depth (minimum number of price levels)
    /// Returns true if depth is established, false otherwise
    fn has_sufficient_depth(&self, exchange: Exchange, min_levels: usize) -> bool {
        let bids_depth = self
            .exchange_bids_price_level
            .get(&exchange)
            .and_then(|map| map.value().read().ok().map(|guard| guard.len()))
            .unwrap_or(0);

        let asks_depth = self
            .exchange_asks_price_level
            .get(&exchange)
            .and_then(|map| map.value().read().ok().map(|guard| guard.len()))
            .unwrap_or(0);

        bids_depth >= min_levels && asks_depth >= min_levels
    }

    pub async fn check_for_immediate_purchase(
        &self,
        price: u64,
        exchange: Exchange,
        side: Side,
        quantity: u64,
        tx: &tokio::sync::mpsc::Sender<BuyExchangeSellExchange>,
    ) {
        // Only proceed if orderbook has established depth (at least 3 price levels on each side)
        const MIN_DEPTH_LEVELS: usize = 3;
        if !self.has_sufficient_depth(exchange, MIN_DEPTH_LEVELS) {
            return;
        }

        let opportunity = match side {
            Side::Buy => {
                // We want to buy at this exchange, check if we can buy cheaper elsewhere
                let best_ask = self.best_ask_all_exchanges();
                self.arbitrage_detector
                    .check_buy_opportunity(price, exchange, best_ask, quantity)
            }
            Side::Sell => {
                // We want to sell at this exchange, check if we can sell higher elsewhere
                let best_bid = self.best_bid_all_exchanges();
                self.arbitrage_detector
                    .check_sell_opportunity(price, exchange, best_bid, quantity)
            }
        };

        if let Some(opportunity) = opportunity {
            // Single println! for all arbitrage opportunities

            println!("Arbitrage Opportunity: Buy on {:?} at {:.4}CENTS, Sell on {:?} at ${:.4}, profit: {:?}CENTS bps : ${:?}",
 opportunity.buy_exchange, opportunity.buy_price as f64, opportunity.sell_exchange, opportunity.sell_price as f64,
              opportunity.profit_cents,                       opportunity.profit_bps(),);

            let buy_exchange_sell_exchange = BuyExchangeSellExchange {
                buy_price: opportunity.buy_price,
                profit_expect: opportunity.profit_bps(),
                sell_price: opportunity.sell_price,
                sell_exchange: opportunity.sell_exchange,
                buy_exchange: opportunity.buy_exchange,
            };
            //TODO: Struct this and use profit bps vs actual profit comparison
            tx.send(buy_exchange_sell_exchange)
                .await
                .unwrap_or_else(|e| eprintln!("Failed to send arbitrage opportunity: {}", e));
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
                let price_level = self
                    .exchange_bids_price_level
                    .entry(exchange)
                    .or_insert_with(|| Arc::new(RwLock::new(BTreeMap::new())));
                let mut guard = match (*price_level.value()).write() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let entry = guard.entry(price).or_insert(0);
                *entry += quantity;

                // Update cached best bid if this is the new best (highest price for bids)
                let best_bid = guard.keys().next_back().copied(); // BTreeMap is sorted, last is highest
                if let Some(best_price) = best_bid {
                    let best_quantity = guard.get(&best_price).copied().unwrap_or(0);
                    drop(guard); // Explicitly drop guard before calling update
                    self.update_cached_best_bid(exchange, best_price, best_quantity);
                }
            }
            Side::Sell => {
                let price_level = self
                    .exchange_asks_price_level
                    .entry(exchange)
                    .or_insert_with(|| Arc::new(RwLock::new(BTreeMap::new())));

                let mut guard = match (*price_level.value()).write() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let entry = guard.entry(price).or_insert(0);
                *entry += quantity;

                // Update cached best ask if this is the new best (lowest price for asks)
                let best_ask = guard.keys().next().copied(); // BTreeMap is sorted, first is lowest
                if let Some(best_price) = best_ask {
                    let best_quantity = guard.get(&best_price).copied().unwrap_or(0);
                    drop(guard); // Explicitly drop guard before calling update
                    self.update_cached_best_ask(exchange, best_price, best_quantity);
                }
            }
        }
    }

    /// Update cached best bid for an exchange and check if it's the global best
    fn update_cached_best_bid(&self, exchange: Exchange, price: u64, quantity: u64) {
        let level = self
            .cached_best_bid
            .entry(exchange)
            .or_insert_with(BestPriceLevel::default);
        level
            .price
            .store(price, std::sync::atomic::Ordering::Relaxed);
        level
            .quantity
            .store(quantity, std::sync::atomic::Ordering::Relaxed);

        // Check if this is the new global best bid (highest price across all exchanges)
        let mut global = self.best_bid_all_exchanges.lock().unwrap();
        let current_global_price = global.1.price.load(std::sync::atomic::Ordering::Relaxed);
        if price > current_global_price || current_global_price == 0 {
            global.0 = exchange;
            global
                .1
                .price
                .store(price, std::sync::atomic::Ordering::Relaxed);
            global
                .1
                .quantity
                .store(quantity, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Update cached best ask for an exchange and check if it's the global best
    fn update_cached_best_ask(&self, exchange: Exchange, price: u64, quantity: u64) {
        let level = self
            .cached_best_ask
            .entry(exchange)
            .or_insert_with(BestPriceLevel::default);
        level
            .price
            .store(price, std::sync::atomic::Ordering::Relaxed);
        level
            .quantity
            .store(quantity, std::sync::atomic::Ordering::Relaxed);

        // Check if this is the new global best ask (lowest price across all exchanges)
        let mut global = self.best_ask_all_exchanges.lock().unwrap();
        let current_global_price = global.1.price.load(std::sync::atomic::Ordering::Relaxed);
        if price < current_global_price || current_global_price == 0 {
            global.0 = exchange;
            global
                .1
                .price
                .store(price, std::sync::atomic::Ordering::Relaxed);
            global
                .1
                .quantity
                .store(quantity, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use pricelevel::Side;

    use crate::orderbook::book::{Exchange, OrderBook};

    #[test]
    fn test_add_exchange_price_level_different_exchanges() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add same price to different exchanges - should be separate
        order_book.add_exchange_price_level(50000, Exchange::Binance, Side::Buy, 10);
        order_book.add_exchange_price_level(50000, Exchange::Coinbase, Side::Buy, 20);

        assert!(order_book
            .exchange_bids_price_level
            .contains_key(&Exchange::Binance));
        assert!(order_book
            .exchange_bids_price_level
            .contains_key(&Exchange::Coinbase));

        let binance_level = order_book
            .exchange_bids_price_level
            .get(&Exchange::Binance)
            .unwrap();
        let coinbase_level = order_book
            .exchange_bids_price_level
            .get(&Exchange::Coinbase)
            .unwrap();

        let binance_map = binance_level.value().read().unwrap();
        let coinbase_map = coinbase_level.value().read().unwrap();
        assert_eq!(binance_map.get(&50000), Some(&10));
        assert_eq!(coinbase_map.get(&50000), Some(&20));
    }

    #[test]
    fn test_add_exchange_price_level_bid() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add bid for Binance
        order_book.add_exchange_price_level(50000, Exchange::Binance, Side::Buy, 10);

        assert!(order_book
            .exchange_bids_price_level
            .contains_key(&Exchange::Binance));

        let price_level = order_book
            .exchange_bids_price_level
            .get(&Exchange::Binance)
            .unwrap();
        let map = price_level.value().read().unwrap();
        assert_eq!(map.get(&50000), Some(&10));
    }

    #[test]
    fn test_add_exchange_price_level_ask() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add ask for Coinbase
        order_book.add_exchange_price_level(50100, Exchange::Coinbase, Side::Sell, 5);

        assert!(order_book
            .exchange_asks_price_level
            .contains_key(&Exchange::Coinbase));

        let price_level = order_book
            .exchange_asks_price_level
            .get(&Exchange::Coinbase)
            .unwrap();
        let map = price_level.value().read().unwrap();
        assert_eq!(map.get(&50100), Some(&5));
    }

    #[test]
    fn test_add_exchange_price_level_quantity_accumulation() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add same price level multiple times - quantities should accumulate
        order_book.add_exchange_price_level(50000, Exchange::Binance, Side::Buy, 10);
        order_book.add_exchange_price_level(50000, Exchange::Binance, Side::Buy, 5);
        order_book.add_exchange_price_level(50000, Exchange::Binance, Side::Buy, 3);

        let price_level = order_book
            .exchange_bids_price_level
            .get(&Exchange::Binance)
            .unwrap();
        let map = price_level.value().read().unwrap();
        assert_eq!(map.get(&50000), Some(&18)); // 10 + 5 + 3
    }

    #[tokio::test]
    async fn test_add_exchange_price_level_concurrent() {
        let order_book = Arc::new(OrderBook::new("ETH/USD".to_string()));
        let book_1 = Arc::clone(&order_book);
        let book_2 = Arc::clone(&order_book);

        let task_1 = tokio::spawn(async move {
            book_1.add_exchange_price_level(2000, Exchange::Binance, Side::Sell, 13);
        });

        let task_2 = tokio::spawn(async move {
            book_2.add_exchange_price_level(2000, Exchange::Binance, Side::Sell, 13);
        });

        let _ = tokio::join!(task_1, task_2);

        // After both tasks complete, check that quantities accumulated
        let price_level = order_book
            .exchange_asks_price_level
            .get(&Exchange::Binance)
            .unwrap();
        let map = price_level.value().read().unwrap();
        let quantity = map.get(&2000).unwrap();
        // Quantities should accumulate: 13 + 13 = 26
        assert_eq!(*quantity, 26);
    }
}
