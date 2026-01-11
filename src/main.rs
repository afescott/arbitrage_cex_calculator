mod api;

use api::{BinanceClient, CoinbaseClient, KrakenClient};
use tracing::{info, Level};
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize tracing for tokio-console compatibility
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("Starting low-latency order book aggregator...");
    info!("Monitoring BTC/USDT pair across multiple exchanges");

    // Spawn tasks for each exchange
    let binance_handle = tokio::spawn(async {
        BinanceClient::listen_btc_usdt().await;
    });

    let kraken_handle = tokio::spawn(async {
        KrakenClient::listen_btc_usdt().await;
    });

    let coinbase_handle = tokio::spawn(async {
        CoinbaseClient::listen_btc_usdt().await;
    });

    // Wait for all tasks (they run indefinitely)
    tokio::select! {
        _ = binance_handle => {
            info!("Binance task ended");
        }
        _ = kraken_handle => {
            info!("Kraken task ended");
        }
        _ = coinbase_handle => {
            info!("Coinbase task ended");
        }
    }
}
