use crate::{
    api::execution::{submit_limit_order, ExecutorContext, LimitOrderRequest, OrderSide},
    args::Args,
    calculation::BuyExchangeSellExchange,
};

pub struct PurchaseManager {
    rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>,
    order: ExecutorContext,
}

impl PurchaseManager {
    pub fn new(rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>, args: Args) -> Self {
        let order = ExecutorContext::new(&args);
        Self { rx, order }
    }

    /// Minimal perps execution wiring (API spec focus).
    /// For each route: go long on `buy_exchange`, short on `sell_exchange`.
    pub async fn run_purchase_simulation(&mut self) {
        if !self.order.has_hyperliquid() {
            eprintln!("Missing `--hyperliquid-private-key`; skipping execution");
            return;
        }
        if !self.order.has_binance() {
            eprintln!("Missing `--binance-api-key/--binance-api-secret`; skipping execution");
            return;
        }

        while let Some(route) = self.rx.recv().await {
            // These keys are required to sign requests once you replace the stub executor.

            // Keep size tiny by default (0.01 BTC). Later: size from budget + book liquidity.
            let qty_sats: u64 = 1_000_000;

            // LONG leg (limit, post-only)
            let _ = submit_limit_order(
                &self.order,
                LimitOrderRequest {
                    exchange: route.buy_exchange,
                    symbol: "BTC",
                    side: OrderSide::Buy,
                    price_cents: route.buy_price,
                    qty_sats,
                    post_only: true,
                    reduce_only: false,
                },
            )
            .await;

            // SHORT leg (perps: SELL is the short entry)
            let _ = submit_limit_order(
                &self.order,
                LimitOrderRequest {
                    exchange: route.sell_exchange,
                    symbol: "BTC",
                    side: OrderSide::Sell,
                    price_cents: route.sell_price,
                    qty_sats,
                    post_only: true,
                    reduce_only: false,
                },
            )
            .await;
        }
    }
}
