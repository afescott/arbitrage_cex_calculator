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
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Spawn tasks for each exchange
    let binance_tx = tx.clone();
    let binance_handle = tokio::spawn(async move {
        BinanceClient::new(binance_tx).listen_btc_usdt().await;
    });

    let kraken_tx = tx.clone();
    let kraken_handle = tokio::spawn(async move {
        KrakenClient::new(kraken_tx).listen_btc_usdt().await;
    });

    let coinbase_handle = tokio::spawn(async move {
        CoinbaseClient::new(tx).listen_btc_usdt().await;
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
