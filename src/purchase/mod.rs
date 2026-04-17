//! Purchase manager: consumes arb routes and submits perp legs via [`crate::api::execution`].
//!
//! Venue-specific preflight lives in [`hl`] and [`dydx`], unified by [`PurchaseVenueModule`].

mod dydx;
mod hl;

use crate::{
    api::execution::{submit_limit_order, ExecutorContext, LimitOrderRequest, OrderSide},
    args::Args,
    calculation::BuyExchangeSellExchange,
    sizing,
};

/// Startup checks for a venue role before the purchase loop runs.
pub(crate) trait PurchaseVenueModule {
    fn preflight(ctx: &ExecutorContext) -> Result<(), &'static str>;
}

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

    fn run_venue_preflight(&self) -> Result<(), &'static str> {
        hl::HyperliquidPurchase::preflight(&self.order)?;
        dydx::DydxPurchase::preflight(&self.order)?;
        Ok(())
    }

    /// Minimal perps execution wiring (API spec focus).
    /// For each route: go long on `buy_exchange`, short on `sell_exchange`.
    pub async fn run_purchase_simulation(&mut self) {
        if let Err(msg) = self.run_venue_preflight() {
            eprintln!("{msg}");
            return;
        }

        while let Some(route) = self.rx.recv().await {
            if !Self::sizing_ok(self.notional_usd_per_leg, &self.perp_symbol) {
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

            if hl::HyperliquidPurchase::route_hits_unsupported_kraken_legs(
                route.buy_exchange,
                route.sell_exchange,
            ) {
                println!("Kraken is not supported right now for the limit orders");
                continue;
            }

            self.submit_cross_legs(&route, qty_sats).await;
        }
    }

    fn sizing_ok(notional_usd_per_leg: u64, perp_symbol: &str) -> bool {
        let min_n = sizing::conservative_min_notional_usd(perp_symbol);
        if notional_usd_per_leg < min_n {
            eprintln!(
                "Skipping route: notional_usd_per_leg {} < conservative min {} (check venue min notional)",
                notional_usd_per_leg, min_n
            );
            return false;
        }
        true
    }

    async fn submit_cross_legs(&self, route: &BuyExchangeSellExchange, qty_sats: u64) {
        let sym = self.perp_symbol.clone();
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
