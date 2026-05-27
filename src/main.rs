mod api;
mod args;
mod calculation;
mod metrics;
mod orderbook;
mod purchase;
mod sizing;
mod telemetry;
mod util;

#[cfg(feature = "cex")]
use api::{BinanceClient, KrakenClient};
#[cfg(feature = "bitget")]
use api::BitgetClient;
use api::{CoinbaseClient, ExchangePrice, HyperliquidClient};
#[cfg(feature = "dydx")]
use api::DydxClient;

use std::time::Duration;

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
    // Sets up the global tracing subscriber. With `--features otel` this also installs
    // the OTLP exporter; the guard flushes pending spans on drop so we keep it until
    // after the shutdown path below.
    let _tracing_guard = crate::telemetry::subscriber::init_subscriber();

    // One always-on span at startup. Doubles as a smoke test that the export pipeline
    // is alive — `submit_cross_legs` only fires when an arb is detected, which may not
    // happen on short runs, but this span will land in Jaeger on every run.
    tracing::info_span!(
        "app_start",
        pair = %order_book_name,
        execute_live = args.execute_live,
        hyperliquid_network = ?args.hyperliquid_network,
    )
    .in_scope(|| {
        tracing::info!("starting low-latency arb engine");
    });

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
    let _binance_handle = tokio::spawn(async { std::future::pending::<()>().await });

    #[cfg(feature = "cex")]
    let kraken_handle = {
        let kraken_tx = tx.clone();
        tokio::spawn(async move {
            KrakenClient::new(kraken_tx).listen_btc_usdt().await;
        })
    };
    #[cfg(not(feature = "cex"))]
    let _kraken_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let coinbase_tx = tx.clone();
    let _coinbase_handle = tokio::spawn(async move {
        CoinbaseClient::new(coinbase_tx).listen_btc_usdt().await;
    });

    #[cfg(feature = "bitget")]
    let _bitget_handle = {
        let bitget_tx = tx.clone();
        tokio::spawn(async move {
            BitgetClient::new(bitget_tx).listen_btc_usdt().await;
        })
    };
    #[cfg(not(feature = "bitget"))]
    let bitget_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let hyperliquid_tx = tx.clone();
    let hyperliquid_network = args.hyperliquid_network.clone();
    let _hyperliquid_handle = tokio::spawn(async move {
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
    let _dydx_handle = tokio::spawn(async { std::future::pending::<()>().await });

    let (tx_purchase, rx_purchase) = tokio::sync::mpsc::channel(50);

    let orderbook = OrderBook::new(order_book_name);

    let telemetry = crate::telemetry::Telemetry::new();

    let mut aggregator_handle = crate::orderbook::spawn_exchange_price_aggregator(
        orderbook,
        telemetry.clone(),
        rx,
        tx_purchase,
    );

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

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let _telemetry_handle = crate::telemetry::spawn_reporter(
        telemetry.clone(),
        Duration::from_secs(5),
        shutdown_rx.clone(),
    );

    let run_secs_limit = args.run_seconds.filter(|&s| s > 0);

    let purchase_telemetry = telemetry.clone();
    let mut purchase_handle = tokio::spawn(async move {
        #[cfg(feature = "csv")]
        let mut pm = PurchaseManager::new(rx_purchase, args, purchase_telemetry, csv_tx_opt);
        #[cfg(not(feature = "csv"))]
        let mut pm = PurchaseManager::new(rx_purchase, args, purchase_telemetry);
        pm.run_purchase_simulation(shutdown_rx).await;
    });

    // Shutdown on Ctrl-C, aggregator exit, purchase exit (--max-routes / rx closed), or --run-seconds.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("ctrl-c: shutting down");
        }
        _ = &mut aggregator_handle => {
            eprintln!("aggregator task ended");
        }
        res = &mut purchase_handle => {
            match res {
                Ok(()) => eprintln!("purchase task ended"),
                Err(e) => eprintln!("purchase task join error: {e}"),
            }
        },
        _ = tokio::time::sleep(Duration::from_secs(run_secs_limit.unwrap())), if run_secs_limit.is_some() => {
            eprintln!("--run-seconds limit reached: shutting down");
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), purchase_handle).await;

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
