mod api;
mod orderbook;
mod util;

use api::{BinanceClient, CoinbaseClient, ExchangePrice, KrakenClient};
use tracing::{info, Level};
use tracing_subscriber;

use crate::orderbook::book::OrderBook;

#[tokio::main]
async fn main() {
    // Initialize tracing for tokio-console compatibility
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("Starting low-latency order book aggregator...");
    info!("Monitoring BTC/USDT pair across multiple exchanges");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangePrice>(1000);
    let (tx_exchange, rx_exchange) = tokio::sync::mpsc::channel::<ExchangePrice>(1000);

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

    /* let compare_price_handle = tokio::spawn(async move {
    });
        while let Some(price) = rx.recv().await {
            info!(
                "Received BTC/USDT price: {}, exchange timestamp: {:?}",
                price.price(),
                price.exchange_timestamp()
            );
        }
    }); */
    let orderbook = OrderBook::new("BTC/USDT".to_string());
    let aggregator_handle = tokio::spawn(async move {
        while let Some(price) = rx.recv().await {
            match price {
                ExchangePrice::Binance {
                    price,
                    exchange_timestamp,
                    received_at,
                } => {
                    /* orderbook.add_exchange_price_level(
                        "Binance".to_string(),
                        pricelevel::Side::Buy,
                        price,
                        1,
                    ); */
                }
                ExchangePrice::Kraken {
                    price,
                    exchange_timestamp,
                    received_at,
                } => todo!(),
                ExchangePrice::Coinbase {
                    price,
                    exchange_timestamp,
                    received_at,
                } => todo!(),
            }
            info!(
                "Aggregated BTC/USDT price: {}, exchange timestamp: {:?}",
                price,
                price.exchange_timestamp()
            );
            tx_exchange.send(price).await.unwrap();
            // Here you could implement more complex aggregation logic
        }
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
        _ = aggregator_handle => {
            info!("Aggregator task ended");
        }
    }
}
