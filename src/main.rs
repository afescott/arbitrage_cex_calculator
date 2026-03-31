mod api;
mod args;
mod calculation;
mod orderbook;
mod util;

use api::{BinanceClient, CoinbaseClient, ExchangePrice, HyperliquidClient, KrakenClient};
use tracing::{info, Level};
use tracing_subscriber;

use crate::{args::Bias, calculation::FeeCalculator, orderbook::book::OrderBook};

#[tokio::main]
async fn main() {
    // Parse CLI credentials once at startup.
    // (Clients currently only use websockets for market data, so secrets aren't
    // consumed yet, but wiring this in makes later order execution work easier.)
    let args = crate::args::Args::from_env().unwrap_or_else(|e| {
        eprintln!("Argument error: {e}");
        std::process::exit(1);
    });
    // Avoid unused warnings without printing secrets.
    let _ = args;

    let pair = args.pair.unwrap_or_else(|| {
        eprintln!("Invalid trading pair args");
        std::process::exit(1);
    });

    run(pair, args.bias).await;
}
async fn run(order_book_name: String, bias: Bias) {
    // Initialize tracing for tokio-console compatibility
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    // info!("Starting low-latency order book aggregator...");
    // info!("Monitoring BTC/USDT pair across multiple exchanges");
    let (tx, rx) = tokio::sync::mpsc::channel::<ExchangePrice>(1000);

    // Spawn tasks for each exchange
    let binance_tx = tx.clone();
    let binance_handle = tokio::spawn(async move {
        BinanceClient::new(binance_tx).listen_btc_usdt().await;
    });

    let kraken_tx = tx.clone();
    let kraken_handle = tokio::spawn(async move {
        KrakenClient::new(kraken_tx).listen_btc_usdt().await;
    });

    let coinbase_tx = tx.clone();
    let coinbase_handle = tokio::spawn(async move {
        CoinbaseClient::new(coinbase_tx).listen_btc_usdt().await;
    });

    let hyperliquid_tx = tx.clone();
    let hyperliquid_handle = tokio::spawn(async move {
        HyperliquidClient::new(hyperliquid_tx)
            .listen_btc_usdt()
            .await;
    });

    let (tx_purchase, rx_purchase) = tokio::sync::mpsc::channel(50);

    let orderbook = OrderBook::new(order_book_name.to_string());
    let aggregator_handle =
        crate::orderbook::spawn_exchange_price_aggregator(orderbook, rx, tx_purchase);

    let purchase_handle = tokio::spawn(async move {
        let mut fee = FeeCalculator::new(rx_purchase);
        fee.run_purchase_simulation();
    });

    // Wait for all tasks (they run indefinitely)
    tokio::select! {
        _ = binance_handle => {
            // info!("Binance task ended");
        }
        _ = kraken_handle => {
            // info!("Kraken task ended");
        }
        _ = coinbase_handle => {
            // info!("Coinbase task ended");
        }
        _ = hyperliquid_handle => {
            // info!("Hyperliquid task ended");
        }
        _ = aggregator_handle => {
            // info!("Aggregator task ended");
        }
    }
}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn test_full_run() {}
}
