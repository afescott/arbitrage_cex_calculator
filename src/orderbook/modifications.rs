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

impl OrderModification {
    pub fn sample() -> Self {
        // Generate a random modification type
        let modification_type: u8 = rand::random::<u8>() % 4; // 4 different modification types

        match modification_type {
            0 => OrderModification::UpdatePrice {
                order_id: Uuid::new_v4(),
                new_price: rand::random::<u64>() % 1000 + 69000, // 69000..70000
            },
            1 => OrderModification::UpdateQuantity {
                order_id: Uuid::new_v4(),
                new_quantity: rand::random::<u64>() % 100 + 500, // 500..600
            },
            2 => OrderModification::UpdatePriceAndQuantity {
                order_id: Uuid::new_v4(),
                new_price: rand::random::<u64>() % 1000 + 69000, // 69000..70000
                new_quantity: rand::random::<u64>() % 100 + 500, // 500..600
            },
            3 => OrderModification::Cancel {
                order_id: Uuid::new_v4(),
            },
            _ => unreachable!(),
        }
    }
}

impl OrderBook {
    /// Get the timestamp of the last trade execution
    pub fn last_traded_at(&self) -> u64 {
        self.last_traded_at.load(Ordering::Relaxed)
    }

    /// Update the last trade timestamp to the current time
    fn update_last_trade_time(&self) {
        let current_time = super::current_time_millis();
        self.last_traded_at.store(current_time, Ordering::Relaxed);
    }

    pub fn process_modification(&self, modification: OrderModification) -> Result<()> {
        match modification {
            OrderModification::UpdatePrice {
                order_id,
                new_price,
            } => {
                // Handle price updates
                if let Err(e) = self.update_order(
                    pricelevel::OrderUpdate::UpdatePrice {
                        order_id: OrderId::Uuid(order_id),
                        new_price,
                    },
                    OrderId::Uuid(order_id),
                ) {
                    return Err(anyhow!(
                        "Failed to update order price ({:?}): {}",
                        new_price,
                        e
                    ));
                }
            }
            OrderModification::UpdateQuantity {
                order_id,
                new_quantity,
            } => {
                // Handle quantity updates
                if let Err(e) = self.update_order(
                    pricelevel::OrderUpdate::UpdateQuantity {
                        order_id: OrderId::Uuid(order_id),
                        new_quantity,
                    },
                    OrderId::Uuid(order_id),
                ) {
                    return Err(anyhow!("Failed to update order quantity: {}", e));
                }
            }
            OrderModification::UpdatePriceAndQuantity {
                order_id,
                new_price,
                new_quantity,
            } => {
                // Handle price and quantity updates
                if let Err(e) = self.update_order(
                    pricelevel::OrderUpdate::UpdatePriceAndQuantity {
                        order_id: OrderId::Uuid(order_id),
                        new_price,
                        new_quantity,
                    },
                    OrderId::Uuid(order_id),
                ) {
                    return Err(anyhow!(
                        "Failed to update order price ({:?}) and quantity: {}",
                        new_price,
                        e
                    ));
                }
            }
            OrderModification::Cancel { order_id } => {
                // Handle order cancellations
                if let Err(e) = self.update_order(
                    pricelevel::OrderUpdate::Cancel {
                        order_id: OrderId::Uuid(order_id),
                    },
                    OrderId::Uuid(order_id),
                ) {
                    return Err(anyhow!("Failed to cancel order: {}", e));
                }
            }
        }
        Ok(())
    }

    pub fn add_to_limit_order(
        &self,
        id: OrderId,
        price: u64,
        quantity: u64,
        side: Side,
    ) -> Result<OrderId> {
        let timestamp = chrono::Utc::now().timestamp_millis() as u64;
        let order_id = pricelevel::OrderType::Standard::<()> {
            id,
            price,
            quantity,
            side,
            timestamp,
            time_in_force: pricelevel::TimeInForce::Gtc,
            extra_fields: (),
        };
        self.add_order::<()>(order_id)
            .map_err(|err| anyhow!("Error adding limit order: {:?}", err))?;

        // After adding a limit order, retry unfilled market orders
        self.retry_unfilled_market_orders();

        Ok(order_id.id())
    }

    pub fn submit_market_order(&self, order_id: OrderId, quantity: u64, side: Side) -> Result<u64> {
        let result = self.submit_market_order_direct(order_id, quantity, side);

        // Only retry if this is not already a retry call
        if result.is_ok() {
            self.retry_unfilled_market_orders();
        }

        result
    }

    fn submit_market_order_direct(
        &self,
        order_id: OrderId,
        quantity: u64,
        side: Side,
    ) -> Result<u64> {
        // Market buy orders match against asks (sells), market sell orders match against bids (buys)
        let bids_or_asks = match side {
            Side::Buy => &self.asks,  // Buy orders match against asks
            Side::Sell => &self.bids, // Sell orders match against bids
        };

        let best_bid_or_ask = match side {
            Side::Buy => self.best_ask(),  // Buy orders match against best ask
            Side::Sell => self.best_bid(), // Sell orders match against best bid
        };
        if let Some(val) = best_bid_or_ask {
            let match_result = {
                let entry = bids_or_asks
                    .get_mut(&val)
                    .ok_or_else(|| anyhow!("Price level disappeared"))?;

                let match_result =
                    entry.match_order(quantity, order_id, &self.transaction_id_generator);

                // Check if we need to remove the price level after the match
                let should_remove = entry.order_count() == 0;

                // Drop the mutable reference before removing
                drop(entry);

                if should_remove {
                    // Now it's safe to remove the price level
                    bids_or_asks.remove(&val);
                    // Update cached best prices
                    /* match side {
                        Side::Buy => self.update_cached_best_ask_after_removal(),
                        Side::Sell => self.update_cached_best_bid_after_removal(),
                    } */
                }

                match_result
            };

            // Update last trade timestamp when market order matches
            self.update_last_trade_time();

            // Track filled orders to remove from tracking later
            for filled_order_id in &match_result.filled_order_ids {
                // TODO: Record removed orders for user history?
                if let Some(_) = self.orders.remove(filled_order_id) {
                    // Order successfully removed
                } else {
                    // Order already removed by another thread - this is OK
                    tracing::warn!(
                        "Order {} already removed by another thread",
                        filled_order_id
                    );
                }
            }

            if match_result.remaining_quantity == 0 {
                return Ok(match_result.remaining_quantity);
            }

            // Only add to queue if there's remaining quantity to fill
            if match_result.remaining_quantity > 0 {
                match side {
                    Side::Buy => self
                        .market_orders_bids
                        .push((order_id, match_result.remaining_quantity)),
                    Side::Sell => self
                        .market_orders_asks
                        .push((order_id, match_result.remaining_quantity)),
                }
            }

            Ok(match_result.remaining_quantity)
            /* if match_result.order_count() == 0 {
                // Remove the price level if no orders remain
                /*                     drop(entry); */

                // TODO: This might be a good idea if we need statistics and for testing
                //  bids_or_asks.remove(val);
            } else {
                // Update the price level in the map
            } */
        } else {
            match side {
                Side::Buy => self.market_orders_bids.push((order_id, quantity)),
                Side::Sell => self.market_orders_asks.push((order_id, quantity)),
            }

            Ok(quantity)
            // No more limit orders available - break out of loop
        }

        /*         Ok(false) */
        /*         } */

        // If we have any remaining quantity, the market order cannot be fully filled
        /* if remaining_quantity > 0 {
            println!("remaining_quantity not filled: {}", remaining_quantity);
            Err(anyhow!(
                "Insufficient liquidity: {} units could not be matched",
                remaining_quantity
            ))
        } else {
            // This should not happen as we return early when remaining_quantity == 0
            Err(anyhow!("No matching orders found"))
        } */
    }
    pub fn add_order<T>(&self, mut order: OrderType<()>) -> Result<OrderId> {
        trace!(
            "Order book {}: Adding order {} at price {}",
            self.symbol,
            order.id(),
            order.price()
        );
        let bids_or_asks = match order.side() {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };

        let match_order = match order.side() {
            Side::Buy => {
                if let Some(price) = self.best_ask() {
                    if price <= order.price() {
                        if let Some(price_level) = self.asks.get(&price) {
                            Some(price_level.match_order(
                                order.visible_quantity(),
                                order.id(),
                                &self.transaction_id_generator,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Side::Sell => {
                if let Some(price) = self.best_bid() {
                    if price >= order.price() {
                        if let Some(price_level) = self.bids.get(&price) {
                            Some(price_level.match_order(
                                order.visible_quantity(),
                                order.id(),
                                &self.transaction_id_generator,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };

        if let Some(match_result) = match_order {
            // Order was matched - update last trade timestamp
            self.update_last_trade_time();

            // Order was matched, handle remaining quantity
            for filled_order_id in &match_result.filled_order_ids {
                // TODO: Record removed orders for user history?
                self.orders.remove(filled_order_id).context(format!(
                    "Order with ID {} not found in orders map",
                    filled_order_id
                ))?;
            }

            if match_result.remaining_quantity > 0 {
                // Order was partially filled, add remaining quantity to the order book
                order = order.with_reduced_quantity(match_result.remaining_quantity);

                // Get or create the price level for the remaining order
                let price_level = bids_or_asks
                    .entry(order.price())
                    .or_insert_with(|| Arc::new(PriceLevel::new(order.price())));

                price_level.add_order(order);
                self.orders
                    .insert(order.id(), (order.price(), order.side()));

                // Update cached best prices
                match order.side() {
                    Side::Buy => self.update_cached_best_bid(order.price()),
                    Side::Sell => self.update_cached_best_ask(order.price()),
                }
            }
        } else {
            // No matching orders found, add to the appropriate price level
            let price_level = bids_or_asks
                .entry(order.price())
                .or_insert_with(|| Arc::new(PriceLevel::new(order.price())));

            price_level.add_order(order);
            self.orders
                .insert(order.id(), (order.price(), order.side()));

            // Update cached best prices
            match order.side() {
                Side::Buy => self.update_cached_best_bid(order.price()),
                Side::Sell => self.update_cached_best_ask(order.price()),
            }
        }

        Ok(order.id())
    }

    pub fn update_order(&self, update: OrderUpdate, order_id: OrderId) -> Result<()> {
        let order = self.orders.get_mut(&order_id);

        if let Some(mut order) = order {
            // Check if the new price is different from the current price
            let price_levels = match order.1 {
                Side::Buy => &self.bids,
                Side::Sell => &self.asks,
            };

            //TODO: Get this to stop failing. Firstly confirm updates are actually working
            let val = price_levels.get(&order.0);

            if let Some(val) = val {
                if val.order_count() == 0 {
                    price_levels.remove(val.key());
                }

                //TODO this is important. Probs gotta return to front_end
                let order_update = val.value().update_order(update)?;

                drop(val);

                //update
                if let Some(order_update) = order_update {
                    let mut order_update =
                        Arc::try_unwrap(order_update.clone()).unwrap_or_else(|arc| (*arc));
                    match update {
                        OrderUpdate::UpdatePrice { new_price, .. } => {
                            //fix and test
                            order.0 = new_price;
                            match &mut order_update {
                                OrderType::Standard { price, .. } => *price = new_price,
                                OrderType::IcebergOrder { price, .. } => *price = new_price,
                                OrderType::PostOnly { price, .. } => *price = new_price,
                                OrderType::TrailingStop { price, .. } => *price = new_price,
                                OrderType::PeggedOrder { price, .. } => *price = new_price,
                                OrderType::MarketToLimit { price, .. } => *price = new_price,
                                OrderType::ReserveOrder { price, .. } => *price = new_price,
                            }

                            price_levels
                                .entry(new_price)
                                .or_insert_with(|| Arc::new(PriceLevel::new(new_price)))
                                .add_order(order_update);
                        }
                        OrderUpdate::UpdateQuantity { new_quantity, .. } => {
                            // Update the quantity of the order
                            match &mut order_update {
                                OrderType::Standard { quantity, .. } => *quantity = new_quantity,
                                OrderType::IcebergOrder {
                                    hidden_quantity, ..
                                } => *hidden_quantity = new_quantity,
                                OrderType::PostOnly { quantity, .. } => *quantity = new_quantity,
                                OrderType::TrailingStop { quantity, .. } => {
                                    *quantity = new_quantity
                                }
                                OrderType::PeggedOrder { quantity, .. } => *quantity = new_quantity,
                                OrderType::MarketToLimit { quantity, .. } => {
                                    *quantity = new_quantity
                                }
                                OrderType::ReserveOrder {
                                    hidden_quantity, ..
                                } => *hidden_quantity = new_quantity,
                            }
                        }
                        OrderUpdate::UpdatePriceAndQuantity {
                            new_price,
                            new_quantity,
                            ..
                        } => {
                            order.0 = new_price;
                            match &mut order_update {
                                OrderType::Standard {
                                    price, quantity, ..
                                } => {
                                    *price = new_price;
                                    *quantity = new_quantity;
                                }
                                OrderType::IcebergOrder {
                                    price,

                                    hidden_quantity,
                                    ..
                                } => {
                                    *price = new_price;
                                    *hidden_quantity = new_quantity;
                                }
                                OrderType::PostOnly {
                                    price, quantity, ..
                                } => {
                                    *price = new_price;
                                    *quantity = new_quantity;
                                }
                                OrderType::TrailingStop {
                                    price, quantity, ..
                                } => {
                                    *price = new_price;
                                    *quantity = new_quantity;
                                }
                                OrderType::PeggedOrder {
                                    price, quantity, ..
                                } => {
                                    *price = new_price;
                                    *quantity = new_quantity;
                                }
                                OrderType::MarketToLimit {
                                    price, quantity, ..
                                } => {
                                    *price = new_price;
                                    *quantity = new_quantity;
                                }
                                OrderType::ReserveOrder {
                                    price,

                                    hidden_quantity,
                                    ..
                                } => {
                                    *price = new_price;
                                    *hidden_quantity = new_quantity;
                                }
                            }

                            price_levels
                                .entry(new_price)
                                .or_insert_with(|| Arc::new(PriceLevel::new(new_price)))
                                .add_order(order_update);
                        }
                        OrderUpdate::Cancel { order_id } => {
                            drop(order);
                            self.orders.remove(&order_id).ok_or_else(|| {
                                anyhow!("Order with ID {} disappeared from orders map", order_id)
                            })?;
                        }
                        _ => info!("Unsupported order update type: {:?}", update),
                    };
                    //this may cause performance issues
                    price_levels.retain(|_, v| v.order_count() > 0);
                } else {
                }
            } else {
                return Err(anyhow!(
                    "Order with ID {} not found in price levels",
                    order_id
                ));
            }
        } else {
            return Err(anyhow!("Order with ID {} not found", order_id));
        }

        Ok(())
    }

    /// Retry unfilled market orders when new liquidity becomes available
    pub fn retry_unfilled_market_orders(&self) {
        // Get retry limit from environment variable, default to 4
        let max_orders_per_retry = std::env::var("MAX_ORDERS_PER_RETRY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(4);

        // Retry market buy orders (Side::Buy) - they match against asks
        self.retry_market_orders_for_side(
            &self.market_orders_bids,
            Side::Buy,
            max_orders_per_retry,
        );

        // Retry market sell orders (Side::Sell) - they match against bids
        self.retry_market_orders_for_side(
            &self.market_orders_asks,
            Side::Sell,
            max_orders_per_retry,
        );
    }

    /// Retry market orders for a specific side
    fn retry_market_orders_for_side(
        &self,
        market_orders: &SegQueue<(OrderId, u64)>,
        side: Side,
        max_orders: usize,
    ) {
        let mut retry_orders = Vec::new();

        // Collect only a small number of unfilled market orders
        let mut count = 0;
        while let Some((order_id, remaining_quantity)) = market_orders.pop() {
            // Skip orders with 0 quantity (fully filled orders that shouldn't be in queue)
            if remaining_quantity == 0 {
                continue;
            }

            retry_orders.push((order_id, remaining_quantity));
            count += 1;
            if count >= max_orders {
                break;
            }
        }

        if retry_orders.is_empty() {
            return; // No more orders to retry
        }

        // Retry each collected order once
        for (order_id, remaining_quantity) in retry_orders {
            match self.submit_market_order_direct(order_id, remaining_quantity, side) {
                Ok(new_quantity) => {
                    if new_quantity == 0 {
                        // Fully filled - don't requeue
                    } else if new_quantity < remaining_quantity {
                        // Partially filled - order already requeued by submit_market_order_direct
                        // No additional action needed
                    } else {
                        // No fill - order already requeued by submit_market_order_direct
                        // No additional action needed
                    }
                }
                Err(_) => {
                    // Error - don't requeue
                }
            }
        }
    }
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
