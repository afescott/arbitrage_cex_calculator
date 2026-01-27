//! # Order Book Operations
//!
//! This module implements the core order book operations including:
//! - Adding limit orders to the order book
//! - Processing market orders with immediate execution
//! - Order matching and fill generation
//! - Order updates (price and quantity modifications)
//! - Order cancellation
//! - Price level management and cleanup
//!
//! The operations are designed for high-performance concurrent access and
//! maintain order book integrity while processing orders in real-time.

use crossbeam_queue::SegQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use anyhow::{anyhow, Context, Result};
use pricelevel::{OrderId, OrderType, OrderUpdate, PriceLevel, Side};
use std::sync::atomic::Ordering;
use tracing::{info, trace};

use super::book::OrderBook;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderModification {
    UpdatePrice {
        order_id: Uuid,
        new_price: u64,
    },
    UpdateQuantity {
        order_id: Uuid,
        new_quantity: u64,
    },
    UpdatePriceAndQuantity {
        order_id: Uuid,
        new_price: u64,
        new_quantity: u64,
    },
    Cancel {
        order_id: Uuid,
    },
}

#[cfg(test)]
mod test {
    use pricelevel::OrderId;

    use crate::orderbook::book::OrderBook;

    #[test]
    fn test_multiple_price_levels_market_order() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add multiple price levels
        order_book
            .add_to_limit_order(OrderId::default(), 100, 1, pricelevel::Side::Buy)
            .unwrap();
        order_book
            .add_to_limit_order(OrderId::default(), 105, 1, pricelevel::Side::Buy)
            .unwrap();
        order_book
            .add_to_limit_order(OrderId::default(), 110, 1, pricelevel::Side::Buy)
            .unwrap();

        // Submit a market order that matches the best bid
        let match_result = order_book
            .submit_market_order(OrderId::default(), 2, pricelevel::Side::Sell)
            .unwrap();

        assert_eq!(order_book.orders.len(), 1);
    }
    #[test]
    fn test_update_order() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add a limit order and get its ID
        let order_id = order_book
            .add_to_limit_order(OrderId::default(), 100, 1, pricelevel::Side::Buy)
            .unwrap();

        // Update the order's price
        order_book
            .update_order(
                pricelevel::OrderUpdate::UpdatePrice {
                    order_id,
                    new_price: 105,
                },
                order_id,
            )
            .unwrap();

        // Verify the order was updated
        assert!(
            order_book.bids.get(&105).is_some(),
            "Order should be updated to new price"
        );

        order_book
            .update_order(
                pricelevel::OrderUpdate::UpdateQuantity {
                    order_id,
                    new_quantity: 5,
                },
                order_id,
            )
            .unwrap();

        let price_level = order_book.bids.get(&105);

        assert!(
            price_level.is_some_and(|x| x.value().total_quantity() == 5),
            "Order should be updated to new price"
        );

        order_book
            .update_order(
                pricelevel::OrderUpdate::UpdatePriceAndQuantity {
                    order_id,
                    new_price: 200,
                    new_quantity: 25,
                },
                order_id,
            )
            .unwrap();

        assert!(
            order_book
                .bids
                .get(&200)
                .is_some_and(|x| x.value().total_quantity() == 25),
            "Order quantity and price should be updated"
        );

        order_book
            .update_order(pricelevel::OrderUpdate::Cancel { order_id }, order_id)
            .unwrap();

        assert!(
            order_book.bids.get(&200).is_none(),
            "Order should be removed after cancel update"
        );

        assert!(
            order_book.bids.is_empty(),
            "Order should be removed from orders map"
        );
    }

    #[test]
    fn test_last_trade_execution_tracking() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Initially, last_traded_at should be 0
        assert_eq!(order_book.last_traded_at(), 0);

        // Add a limit order (buy)
        let _order_id_1 = order_book
            .add_to_limit_order(OrderId::default(), 100, 5, pricelevel::Side::Buy)
            .unwrap();

        // No trade yet, last_traded_at should still be 0
        assert_eq!(order_book.last_traded_at(), 0);

        // Add a matching limit order (sell) that should trigger a trade
        let _order_id_2 = order_book
            .add_to_limit_order(OrderId::default(), 100, 3, pricelevel::Side::Sell)
            .unwrap();

        // Now we should have a trade execution timestamp
        let last_trade_time = order_book.last_traded_at();
        assert!(
            last_trade_time > 0,
            "Last trade time should be updated after matching limit orders"
        );

        // Add another matching order to verify timestamp updates
        std::thread::sleep(std::time::Duration::from_millis(1)); // Small delay to ensure different timestamp
        let _order_id_3 = order_book
            .add_to_limit_order(OrderId::default(), 100, 2, pricelevel::Side::Sell)
            .unwrap();

        let new_last_trade_time = order_book.last_traded_at();
        assert!(
            new_last_trade_time >= last_trade_time,
            "Last trade time should be updated with new trades"
        );

        // Add more liquidity for market order test
        let _order_id_4 = order_book
            .add_to_limit_order(OrderId::default(), 99, 5, pricelevel::Side::Sell)
            .unwrap();

        // Test market order execution
        std::thread::sleep(std::time::Duration::from_millis(1));
        let _market_result = order_book
            .submit_market_order(OrderId::default(), 1, pricelevel::Side::Buy)
            .unwrap();

        let market_trade_time = order_book.last_traded_at();
        assert!(
            market_trade_time >= new_last_trade_time,
            "Market order should also update last trade time"
        );
    }

    #[test]
    fn test_order_book_cancel_order() {
        let order_book = OrderBook::new("BTC/USD".to_string());

        // Add a limit order and get its ID
        let order_id_1 = order_book
            .add_to_limit_order(OrderId::default(), 100, 1, pricelevel::Side::Buy)
            .unwrap();
        let order_id_2 = order_book
            .add_to_limit_order(OrderId::default(), 105, 1, pricelevel::Side::Buy)
            .unwrap();

        assert!(
            order_book.bids.contains_key(&100),
            "Order should contain key 100 in bids"
        );

        // Cancel the order
        order_book
            .update_order(
                pricelevel::OrderUpdate::Cancel {
                    order_id: order_id_1,
                },
                order_id_1,
            )
            .unwrap();

        assert!(
            !order_book.bids.contains_key(&100),
            "Order should be removed from bids"
        );

        assert!(
            order_book.bids.contains_key(&105),
            "Order should contain key 105 in bids"
        );
        order_book
            .update_order(
                pricelevel::OrderUpdate::Cancel {
                    order_id: order_id_2,
                },
                order_id_2,
            )
            .unwrap();

        assert!(
            order_book.bids.is_empty(),
            "Bids should be empty after cancellation"
        );

        assert!(
            order_book.orders.is_empty(),
            "Order should be removed from orders map"
        );
    }
}
