pub mod binance;
pub mod coinbase;
pub mod kraken;

pub use binance::BinanceClient;
pub use coinbase::CoinbaseClient;
pub use kraken::KrakenClient;

pub enum ExchangePrice {
    Binance(u64),
    Kraken(u64),
    Coinbase(u64),
}

impl std::fmt::Display for ExchangePrice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExchangePrice::Binance(price) => write!(f, "Binance: {} cents", price),
            ExchangePrice::Kraken(price) => write!(f, "Kraken: {} cents", price),
            ExchangePrice::Coinbase(price) => write!(f, "Coinbase: {} cents", price),
        }
    }
}
