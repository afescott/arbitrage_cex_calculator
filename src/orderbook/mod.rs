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

use ::pricelevel::MatchResult;

pub mod book;
mod modifications;

pub use modifications::OrderModification;

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

/* #[cfg(test)]
mod test {
    use super::book::OrderBook;

    #[test]
    fn test_partial_fill() {
        let order_book = OrderBook::new("BTCUSD".to_string());

        let order_id = pricelevel::OrderId::default();

        let price = 1000;
        let quantity = 10;
        let _ = order_book
            .add_to_limit_order(order_id, price, quantity, pricelevel::Side::Buy)
            .unwrap();

        let _ = order_book
            .submit_market_order(pricelevel::OrderId::default(), 5, pricelevel::Side::Sell)
            .unwrap();
    }
} */
