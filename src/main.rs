mod api;
mod args;
mod calculation;
mod orderbook;
mod purchase;
mod sizing;
mod util;

#[cfg(feature = "cex")]
use api::{BinanceClient, KrakenClient};
use api::{CoinbaseClient, DydxClient, ExchangePrice, HyperliquidClient};
use tracing::Level;
use tracing_subscriber;

use crate::{args::Args, orderbook::book::OrderBook, purchase::PurchaseManager};

#[tokio::main]
async fn main() {
    // Parse CLI credentials once at startup.
    // (Clients currently only use websockets for market data, so secrets aren't
    // consumed yet, but wiring this in makes later order execution work easier.)
    let args = crate::args::Args::from_env().unwrap_or_else(|e| {
        eprintln!("Argument error: {e}");
        std::process::exit(1);
    });

    let pair = args.pair.clone().unwrap_or_else(|| {
        eprintln!("Invalid trading pair args");
        std::process::exit(1);
    });

    run(pair, args).await;
}
async fn run(order_book_name: String, args: Args) {
    // Initialize tracing for tokio-console compatibility
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    // info!("Starting low-latency order book aggregator...");
    // info!("Monitoring BTC/USDT pair across multiple exchanges");
    let (tx, rx) = tokio::sync::mpsc::channel::<ExchangePrice>(1000);

    // Spawn tasks for each exchange
    #[cfg(feature = "cex")]
    let binance_handle = {
        let binance_tx = tx.clone();
        tokio::spawn(async move {
            BinanceClient::new(binance_tx).listen_btc_usdt().await;
        })
    };
    #[cfg(not(feature = "cex"))]
    let binance_handle = tokio::spawn(async { std::future::pending::<()>().await });

    #[cfg(feature = "cex")]
    let kraken_handle = {
        let kraken_tx = tx.clone();
        tokio::spawn(async move {
            KrakenClient::new(kraken_tx).listen_btc_usdt().await;
        })
    };
    #[cfg(not(feature = "cex"))]
    let kraken_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let coinbase_tx = tx.clone();
    let coinbase_handle = tokio::spawn(async move {
        CoinbaseClient::new(coinbase_tx).listen_btc_usdt().await;
    });

    let hyperliquid_tx = tx.clone();
    let hyperliquid_network = args.hyperliquid_network.clone();
    let hyperliquid_handle = tokio::spawn(async move {
        HyperliquidClient::new(hyperliquid_tx, hyperliquid_network)
            .listen_btc_usdt()
            .await;
    });

    let dydx_tx = tx.clone();
    let dydx_network = args.dydx_network.clone();
    let dydx_handle = tokio::spawn(async move {
        DydxClient::new(dydx_tx, dydx_network)
            .listen_btc_usdt()
            .await;
    });

    let (tx_purchase, rx_purchase) = tokio::sync::mpsc::channel(50);

    let orderbook = OrderBook::new(order_book_name);

    let aggregator_handle =
        crate::orderbook::spawn_exchange_price_aggregator(orderbook, rx, tx_purchase);

    let purchase_handle = tokio::spawn(async move {
        let mut pm = PurchaseManager::new(rx_purchase, args);
        pm.run_purchase_simulation().await;
    });

    // Wait for all tasks (they run indefinitely)
    tokio::select! {
        /* _ = binance_handle => {
            // info!("Binance task ended");
        }
        _ = kraken_handle => {
            // info!("Kraken task ended");
        }
        _ = coinbase_handle => {
            // info!("Coinbase task ended");
        } */
        _ = hyperliquid_handle => {
            // info!("Hyperliquid task ended");
        }
        _ = dydx_handle => {
            // info!("dYdX task ended");
        }
        _ = aggregator_handle => {
            // info!("Aggregator task ended");
        }
        _ = purchase_handle => {
            // info!("Purchase manager task ended");
        }
    }
}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn test_full_run() {}
}
