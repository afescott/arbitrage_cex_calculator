//! Exchange WebSocket clients and shared price types.
#![allow(dead_code)]

#[cfg(feature = "cex")]
pub mod binance;
#[cfg(feature = "bitget")]
pub mod bitget;
pub mod coinbase;
#[cfg(feature = "dydx")]
pub mod dydx;
pub mod hyperliquid;
#[cfg(feature = "cex")]
pub mod kraken;

#[cfg(feature = "cex")]
pub use binance::BinanceClient;
#[cfg(feature = "bitget")]
pub use bitget::BitgetClient;
pub use coinbase::CoinbaseClient;
#[cfg(feature = "dydx")]
pub use dydx::DydxClient;
pub use hyperliquid::HyperliquidClient;
#[cfg(feature = "cex")]
pub use kraken::KrakenClient;

use std::time::Instant;

pub struct PriceUpdate {
    pub exchange: Exchange,
    pub price: u64,
    pub received_at: Instant,
}

pub enum Exchange {
    Binance,
    Kraken,
    Coinbase,
    Hyperliquid,
    Dydx,
    Bitget,
}

#[derive(Debug, Clone, Copy)]
pub enum Side {
    /// Buy side (bids)
    Buy,
    /// Sell side (asks)
    Sell,
}

// ExchangePrice includes price, quantity, exchange timestamp, and receive timestamp
pub enum ExchangePrice {
    Binance {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>, // From exchange (E field, milliseconds)
        received_at: Instant,            // When we received it
        side: Side,                      // Buy or Sell side
    },
    Kraken {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>, // From exchange (timestamp field)
        received_at: Instant,
        side: Side, // Buy or Sell side
    },
    Coinbase {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>, // From exchange (time field)
        received_at: Instant,
        side: Side, // Buy or Sell side
    },
    Hyperliquid {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>, // From exchange (timestamp field)
        received_at: Instant,
        side: Side, // Buy or Sell side (perpetual futures)
    },
    #[cfg(feature = "dydx")]
    Dydx {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>, // indexer message_id as logical sequence
        received_at: Instant,
        side: Side,
    },
    Bitget {
        price: u64,
        quantity: u64,
        exchange_timestamp: Option<u64>,
        received_at: Instant,
        side: Side,
    },
}

impl ExchangePrice {
    pub fn price(&self) -> u64 {
        match self {
            ExchangePrice::Binance { price, .. } => *price,
            ExchangePrice::Kraken { price, .. } => *price,
            ExchangePrice::Coinbase { price, .. } => *price,
            ExchangePrice::Hyperliquid { price, .. } => *price,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { price, .. } => *price,
            ExchangePrice::Bitget { price, .. } => *price,
        }
    }

    pub fn quantity(&self) -> u64 {
        match self {
            ExchangePrice::Binance { quantity, .. } => *quantity,
            ExchangePrice::Kraken { quantity, .. } => *quantity,
            ExchangePrice::Coinbase { quantity, .. } => *quantity,
            ExchangePrice::Hyperliquid { quantity, .. } => *quantity,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { quantity, .. } => *quantity,
            ExchangePrice::Bitget { quantity, .. } => *quantity,
        }
    }

    pub fn received_at(&self) -> Instant {
        match self {
            ExchangePrice::Binance { received_at, .. } => *received_at,
            ExchangePrice::Kraken { received_at, .. } => *received_at,
            ExchangePrice::Coinbase { received_at, .. } => *received_at,
            ExchangePrice::Hyperliquid { received_at, .. } => *received_at,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { received_at, .. } => *received_at,
            ExchangePrice::Bitget { received_at, .. } => *received_at,
        }
    }

    pub fn exchange_timestamp(&self) -> Option<u64> {
        match self {
            ExchangePrice::Binance {
                exchange_timestamp, ..
            } => *exchange_timestamp,
            ExchangePrice::Kraken {
                exchange_timestamp, ..
            } => *exchange_timestamp,
            ExchangePrice::Coinbase {
                exchange_timestamp, ..
            } => *exchange_timestamp,
            ExchangePrice::Hyperliquid {
                exchange_timestamp, ..
            } => *exchange_timestamp,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx {
                exchange_timestamp, ..
            } => *exchange_timestamp,
            ExchangePrice::Bitget {
                exchange_timestamp, ..
            } => *exchange_timestamp,
        }
    }

    pub fn exchange(&self) -> Exchange {
        match self {
            ExchangePrice::Binance { .. } => Exchange::Binance,
            ExchangePrice::Kraken { .. } => Exchange::Kraken,
            ExchangePrice::Coinbase { .. } => Exchange::Coinbase,
            ExchangePrice::Hyperliquid { .. } => Exchange::Hyperliquid,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { .. } => Exchange::Dydx,
            ExchangePrice::Bitget { .. } => Exchange::Bitget,
        }
    }

    pub fn side(&self) -> Side {
        match self {
            ExchangePrice::Binance { side, .. } => *side,
            ExchangePrice::Kraken { side, .. } => *side,
            ExchangePrice::Coinbase { side, .. } => *side,
            ExchangePrice::Hyperliquid { side, .. } => *side,
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { side, .. } => *side,
            ExchangePrice::Bitget { side, .. } => *side,
        }
    }

    /// Calculate latency: time from exchange timestamp to when we received it
    /// Returns None if exchange timestamp not available
    pub fn network_latency_ms(&self) -> Option<u64> {
        let exchange_ts = self.exchange_timestamp()?;
        let received_ts = self.received_at();
        // Note: This is approximate - would need SystemTime conversion for exact calculation
        // For now, just return processing latency
        Some(received_ts.elapsed().as_millis() as u64)
    }
}

impl std::fmt::Display for ExchangePrice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let latency_us = self.received_at().elapsed().as_micros();
        let exchange_ts = self.exchange_timestamp();
        match self {
            ExchangePrice::Binance { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "Binance: {} cents (exchange_ts: {}ms, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(f, "Binance: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Kraken { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "Kraken: {} cents (exchange_ts: {}ms, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(f, "Kraken: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Coinbase { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "Coinbase: {} cents (exchange_ts: {}ms, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(f, "Coinbase: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Hyperliquid { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "Hyperliquid: {} cents (exchange_ts: {}ms, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(
                        f,
                        "Hyperliquid: {} cents (latency: {}μs)",
                        price, latency_us
                    )
                }
            }
            #[cfg(feature = "dydx")]
            ExchangePrice::Dydx { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "dYdX: {} cents (message_id: {}, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(f, "dYdX: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Bitget { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(
                        f,
                        "Bitget: {} cents (exchange_ts: {}ms, latency: {}μs)",
                        price, ts, latency_us
                    )
                } else {
                    write!(f, "Bitget: {} cents (latency: {}μs)", price, latency_us)
                }
            }
        }
    }
}
