mod api;
mod args;
mod calculation;
mod metrics;
mod orderbook;
mod purchase;
mod sizing;
mod util;

#[cfg(feature = "cex")]
use api::{BinanceClient, KrakenClient};
#[cfg(feature = "bitget")]
use api::BitgetClient;
use api::{CoinbaseClient, ExchangePrice, HyperliquidClient};
#[cfg(feature = "dydx")]
use api::DydxClient;
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

    #[cfg(feature = "bitget")]
    let bitget_handle = {
        let bitget_tx = tx.clone();
        tokio::spawn(async move {
            BitgetClient::new(bitget_tx).listen_btc_usdt().await;
        })
    };
    #[cfg(not(feature = "bitget"))]
    let bitget_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let hyperliquid_tx = tx.clone();
    let hyperliquid_network = args.hyperliquid_network.clone();
    let hyperliquid_handle = tokio::spawn(async move {
        HyperliquidClient::new(hyperliquid_tx, hyperliquid_network)
            .listen_btc_usdt()
            .await;
    });

    #[cfg(feature = "dydx")]
    let dydx_handle = {
        let dydx_tx = tx.clone();
        let dydx_network = args.dydx_network.clone();
        tokio::spawn(async move {
            DydxClient::new(dydx_tx, dydx_network)
                .listen_btc_usdt()
                .await;
        })
    };
    #[cfg(not(feature = "dydx"))]
    let dydx_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let (tx_purchase, rx_purchase) = tokio::sync::mpsc::channel(50);

    let orderbook = OrderBook::new(order_book_name);

    let aggregator_handle =
        crate::orderbook::spawn_exchange_price_aggregator(orderbook, rx, tx_purchase);

    #[cfg(feature = "csv")]
    let csv_handle = {
        use std::path::PathBuf;
        let path = args
            .csv_path
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if path.is_empty() {
            None
        } else {
            let run_label = format!(
                "{}-{}",
                args.perp_symbol,
                chrono::Utc::now().timestamp_millis()
            );
            Some(crate::metrics::csv::start_csv_writer(
                PathBuf::from(path),
                run_label,
            ))
        }
    };

    #[cfg(feature = "csv")]
    let csv_tx_opt = csv_handle.as_ref().map(|h| h.tx.clone());

    let purchase_handle = tokio::spawn(async move {
        #[cfg(feature = "csv")]
        let mut pm = PurchaseManager::new(rx_purchase, args, csv_tx_opt);
        #[cfg(not(feature = "csv"))]
        let mut pm = PurchaseManager::new(rx_purchase, args);
        pm.run_purchase_simulation().await;
    });

    // Wait for all tasks (they run indefinitely)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("ctrl-c: shutting down");
        }
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
        _ = bitget_handle => {
            // info!("Bitget task ended");
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

    // Best-effort cleanup on shutdown.
    #[cfg(feature = "csv")]
    if let Some(h) = csv_handle {
        drop(h.tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), h.join).await;
    }
}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn test_full_run() {}
}
