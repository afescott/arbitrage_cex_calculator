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

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
