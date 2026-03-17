mod api;
mod calculation;
mod orderbook;
mod util;

use api::{BinanceClient, CoinbaseClient, ExchangePrice, HyperliquidClient, KrakenClient};
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

    // info!("Starting low-latency order book aggregator...");
    // info!("Monitoring BTC/USDT pair across multiple exchanges");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangePrice>(1000);

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

    let orderbook = OrderBook::new(order_book_name.to_string());
    let aggregator_handle = tokio::spawn(async move {
        while let Some(price) = rx.recv().await {
            match price {
                ExchangePrice::Binance {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    // Convert api::Side to pricelevel::Side
                    let orderbook_side = match side {
                        api::Side::Buy => pricelevel::Side::Buy,
                        api::Side::Sell => pricelevel::Side::Sell,
                    };
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Binance,
                        orderbook_side,
                        quantity,
                    );

                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Binance,
                        orderbook_side,
                        quantity,
                    );
                }
                ExchangePrice::Kraken {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    // Convert api::Side to pricelevel::Side
                    let orderbook_side = match side {
                        api::Side::Buy => pricelevel::Side::Buy,
                        api::Side::Sell => pricelevel::Side::Sell,
                    };
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Kraken,
                        orderbook_side,
                        quantity,
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Kraken,
                        orderbook_side,
                        quantity,
                    );
                }
                ExchangePrice::Coinbase {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    // Convert api::Side to pricelevel::Side
                    let orderbook_side = match side {
                        api::Side::Buy => pricelevel::Side::Buy,
                        api::Side::Sell => pricelevel::Side::Sell,
                    };
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Coinbase,
                        orderbook_side,
                        quantity,
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Coinbase,
                        orderbook_side,
                        quantity,
                    );
                }
                ExchangePrice::Hyperliquid {
                    price,
                    quantity,
                    exchange_timestamp: _,
                    received_at: _,
                    side,
                } => {
                    // Convert api::Side to pricelevel::Side
                    let orderbook_side = match side {
                        api::Side::Buy => pricelevel::Side::Buy,
                        api::Side::Sell => pricelevel::Side::Sell,
                    };
                    orderbook.check_for_immediate_purchase(
                        price,
                        orderbook::book::Exchange::Hyperliquid,
                        orderbook_side,
                        quantity,
                    );
                    orderbook.add_exchange_price_level(
                        price,
                        orderbook::book::Exchange::Hyperliquid,
                        orderbook_side,
                        quantity,
                    );
                }
            }
            // Here you could implement more complex aggregation logic
        }
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
