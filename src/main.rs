mod api;
mod orderbook;
mod util;

use api::{BinanceClient, CoinbaseClient, ExchangePrice, KrakenClient};
use tracing::{info, Level};
use tracing_subscriber;

use crate::orderbook::book::OrderBook;

#[tokio::main]
async fn main() {
    run("BTC/USDT".to_string()).await;
}
async fn run(order_book_name: String) {
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
    let orderbook = OrderBook::new(order_book_name.to_string());
    let aggregator_handle = tokio::spawn(async move {
        while let Some(price) = rx.recv().await {
            match price {
                ExchangePrice::Binance {
                    price,
                    exchange_timestamp,
                    received_at,
                } => {
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Binance,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Binance,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                }
                ExchangePrice::Kraken {
                    price,
                    exchange_timestamp,
                    received_at,
                } => {
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Kraken,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Kraken,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                }
                ExchangePrice::Coinbase {
                    price,
                    exchange_timestamp,
                    received_at,
                } => {
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Coinbase,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Coinbase,
                        pricelevel::Side::Buy,
                        unimplemented!(),
                    );
                }
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

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn test_full_run() {}
}
