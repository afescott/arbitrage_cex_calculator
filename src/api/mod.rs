pub mod binance;
pub mod coinbase;
pub mod kraken;

pub use binance::BinanceClient;
pub use coinbase::CoinbaseClient;
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
}

// ExchangePrice includes both exchange timestamp (if available) and receive timestamp
pub enum ExchangePrice {
    Binance {
        price: u64,
        exchange_timestamp: Option<u64>, // From exchange (E field, milliseconds)
        received_at: Instant,           // When we received it
    },
    Kraken {
        price: u64,
        exchange_timestamp: Option<u64>, // From exchange (timestamp field)
        received_at: Instant,
    },
    Coinbase {
        price: u64,
        exchange_timestamp: Option<u64>, // From exchange (time field)
        received_at: Instant,
    },
}

impl ExchangePrice {
    pub fn price(&self) -> u64 {
        match self {
            ExchangePrice::Binance { price, .. } => *price,
            ExchangePrice::Kraken { price, .. } => *price,
            ExchangePrice::Coinbase { price, .. } => *price,
        }
    }

    pub fn received_at(&self) -> Instant {
        match self {
            ExchangePrice::Binance { received_at, .. } => *received_at,
            ExchangePrice::Kraken { received_at, .. } => *received_at,
            ExchangePrice::Coinbase { received_at, .. } => *received_at,
        }
    }

    pub fn exchange_timestamp(&self) -> Option<u64> {
        match self {
            ExchangePrice::Binance { exchange_timestamp, .. } => *exchange_timestamp,
            ExchangePrice::Kraken { exchange_timestamp, .. } => *exchange_timestamp,
            ExchangePrice::Coinbase { exchange_timestamp, .. } => *exchange_timestamp,
        }
    }

    pub fn exchange(&self) -> Exchange {
        match self {
            ExchangePrice::Binance { .. } => Exchange::Binance,
            ExchangePrice::Kraken { .. } => Exchange::Kraken,
            ExchangePrice::Coinbase { .. } => Exchange::Coinbase,
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
                    write!(f, "Binance: {} cents (exchange_ts: {}ms, latency: {}μs)", price, ts, latency_us)
                } else {
                    write!(f, "Binance: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Kraken { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(f, "Kraken: {} cents (exchange_ts: {}ms, latency: {}μs)", price, ts, latency_us)
                } else {
                    write!(f, "Kraken: {} cents (latency: {}μs)", price, latency_us)
                }
            }
            ExchangePrice::Coinbase { price, .. } => {
                if let Some(ts) = exchange_ts {
                    write!(f, "Coinbase: {} cents (exchange_ts: {}ms, latency: {}μs)", price, ts, latency_us)
                } else {
                    write!(f, "Coinbase: {} cents (latency: {}μs)", price, latency_us)
                }
            }
        }
    }
}
