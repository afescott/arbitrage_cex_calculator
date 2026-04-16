use crate::{
    api::execution::{submit_limit_order, ExecutorContext, LimitOrderRequest, OrderSide},
    args::Args,
    calculation::BuyExchangeSellExchange,
    sizing,
};

pub struct PurchaseManager {
    rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>,
    order: ExecutorContext,
    perp_symbol: String,
    notional_usd_per_leg: u64,
}

impl PurchaseManager {
    pub fn new(rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>, args: Args) -> Self {
        let notional_usd_per_leg = args.clamped_notional_usd_per_leg();
        let perp_symbol = args.perp_symbol.clone();
        let order = ExecutorContext::new(&args);
        eprintln!(
            "Purchase sizing: perp={} notional_usd_per_leg={} (budget={}, cap uses max_margin_leverage={})",
            perp_symbol,
            notional_usd_per_leg,
            args.budget,
            args.max_margin_leverage_assumption
        );
        Self {
            rx,
            order,
            perp_symbol,
            notional_usd_per_leg,
        }
    }

    /// Minimal perps execution wiring (API spec focus).
    /// For each route: go long on `buy_exchange`, short on `sell_exchange`.
    pub async fn run_purchase_simulation(&mut self) {
        if !self.order.has_hyperliquid() {
            eprintln!("Missing `--hyperliquid-private-key`; skipping execution");
            return;
        }
        #[cfg(feature = "cex")]
        if !self.order.has_binance() && !self.order.has_dydx() {
            eprintln!(
                "Missing second venue credentials: add `--binance-api-key/--binance-api-secret` and/or `--dydx-private-key`"
            );
            return;
        }
        #[cfg(not(feature = "cex"))]
        if !self.order.has_dydx() {
            eprintln!("Missing `--dydx-private-key` (build with `--features cex` for Binance/Kraken feeds and Binance execution)");
            return;
        }

        while let Some(route) = self.rx.recv().await {
            let min_n = sizing::conservative_min_notional_usd(&self.perp_symbol);
            if self.notional_usd_per_leg < min_n {
                eprintln!(
                    "Skipping route: notional_usd_per_leg {} < conservative min {} (check venue min notional)",
                    self.notional_usd_per_leg, min_n
                );
                continue;
            }

            let Some(qty_sats) =
                sizing::qty_base_e8_from_notional(route.buy_price, self.notional_usd_per_leg)
            else {
                eprintln!(
                    "Skipping route: could not size qty (buy_price_cents={}, notional={})",
                    route.buy_price, self.notional_usd_per_leg
                );
                continue;
            };

            println!(
                "Buying on {:?} at {} cents, selling on {:?} at {:?} cents",
                route.buy_exchange, route.buy_price, route.sell_exchange, route.sell_price
            );

            #[cfg(feature = "cex")]
            let skip_kraken_route = route.buy_exchange == crate::orderbook::book::Exchange::Kraken
                || route.sell_exchange == crate::orderbook::book::Exchange::Kraken;
            #[cfg(not(feature = "cex"))]
            let skip_kraken_route = false;

            if skip_kraken_route {
                println!("Kraken is not supported right now for the limit orders");
            } else {
                let sym = self.perp_symbol.clone();
                // LONG leg (limit, post-only)
                let res_buy = submit_limit_order(
                    &self.order,
                    LimitOrderRequest {
                        exchange: route.buy_exchange,
                        symbol: sym.clone(),
                        side: OrderSide::Buy,
                        price_cents: route.buy_price,
                        qty_sats,
                        post_only: true,
                        reduce_only: false,
                    },
                )
                .await;

                // SHORT leg (perps: SELL is the short entry)
                let res_short = submit_limit_order(
                    &self.order,
                    LimitOrderRequest {
                        exchange: route.sell_exchange,
                        symbol: sym,
                        side: OrderSide::Sell,
                        price_cents: route.sell_price,
                        qty_sats,
                        post_only: true,
                        reduce_only: false,
                    },
                )
                .await;

                println!("Buy order result: {:?}", res_buy);
                println!();
                println!("Short order result: {:?}", res_short);
            }
        }
    }
}
