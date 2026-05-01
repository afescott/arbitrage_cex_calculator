//! Purchase: perp limit-order execution (build → sign → send) and the arb purchase loop.
//!
//! Shared request/response types and [`OrderExecutor`] live here; Hyperliquid and dYdX live in
//! [`hl`] and [`dydx`]. Binance USDⓈ-M futures (HMAC) is in [`binance`] behind `feature = "cex"`.

#[cfg(feature = "cex")]
mod binance;
mod dydx;
mod hl;

use crate::{args::Args, calculation::BuyExchangeSellExchange, orderbook::book::Exchange, sizing};

/// Hyperliquid `s` field and dYdX planning `quantums`: decimal string from base qty × 1e8.
pub(crate) fn qty_sats_to_decimal_string(qty_sats: u64) -> String {
    let int = qty_sats / 100_000_000;
    let frac = qty_sats % 100_000_000;
    if frac == 0 {
        return int.to_string();
    }
    let mut s = format!("{}.{:08}", int, frac);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

#[derive(Debug, Clone, Copy)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct LimitOrderRequest {
    pub exchange: Exchange,
    /// Base asset (e.g. `BTC`, `SOL`); Binance futures mapping appends `USDT` when missing.
    pub symbol: String,
    pub side: OrderSide,
    pub price_cents: u64,
    /// Base quantity × 1e8 (used for sizing and venue quantity fields).
    pub qty_sats: u64,
    pub post_only: bool,
    pub reduce_only: bool,
}

#[derive(Debug, Clone)]
pub struct OrderAck {
    pub exchange: Exchange,
    pub client_order_id: String,
}

#[derive(Debug)]
pub enum ExecError {
    MissingCredentials(&'static str),
    UnsupportedExchange(Exchange),
    SendFailed(String),
}

#[derive(Debug, Clone)]
pub struct BuiltPayload {
    pub venue: Exchange,
    pub endpoint: &'static str,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SignedPayload {
    pub venue: Exchange,
    pub endpoint: &'static str,
    pub body: String,
    pub signature: String,
}

pub trait OrderExecutor {
    fn venue(&self) -> Exchange;
    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError>;
    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError>;
    fn send(
        &self,
        signed: SignedPayload,
    ) -> impl std::future::Future<Output = Result<OrderAck, ExecError>> + Send;
}

/// Execution context holding pre-built venue executors.
#[derive(Debug, Clone)]
pub struct ExecutorContext {
    hyperliquid: Option<hl::HyperliquidExecutor>,
    #[cfg(feature = "cex")]
    binance: Option<binance::BinanceFuturesExecutor>,
    dydx: Option<dydx::DydxExecutor>,
}

impl ExecutorContext {
    pub fn new(args: &Args) -> Self {
        let hyperliquid = args.hyperliquid_private_key.clone().map(|pk| {
            hl::HyperliquidExecutor::new(
                pk,
                args.hyperliquid_asset_id,
                args.perp_symbol.clone(),
                args.hyperliquid_network,
                args.hyperliquid_ioc_cross_bps,
            )
        });
        #[cfg(feature = "cex")]
        let binance = match (
            args.binance_api_key.clone(),
            args.binance_api_secret.clone(),
        ) {
            (Some(k), Some(s)) => Some(binance::BinanceFuturesExecutor::new(k, s)),
            _ => None,
        };
        // For relay-based execution, the Rust binary does NOT need the signing material.
        // We still create a Dydx executor if either a local key OR a relay URL is provided.
        let dydx = match (args.dydx_private_key.clone(), args.dydx_order_relay_url.clone()) {
            (Some(pk), relay) => Some(dydx::DydxExecutor::new(Some(pk), relay)),
            (None, Some(relay)) => Some(dydx::DydxExecutor::new(None, Some(relay))),
            (None, None) => None,
        };
        Self {
            hyperliquid,
            #[cfg(feature = "cex")]
            binance,
            dydx,
        }
    }

    pub fn has_hyperliquid(&self) -> bool {
        self.hyperliquid.is_some()
    }

    #[cfg(feature = "cex")]
    pub fn has_binance(&self) -> bool {
        self.binance.is_some()
    }

    #[cfg(not(feature = "cex"))]
    pub fn has_binance(&self) -> bool {
        false
    }

    pub fn has_dydx(&self) -> bool {
        self.dydx.is_some()
    }
}

pub async fn submit_limit_order(
    order: &ExecutorContext,
    req: LimitOrderRequest,
) -> Result<OrderAck, ExecError> {
    match req.exchange {
        Exchange::Hyperliquid => {
            let ex = order
                .hyperliquid
                .as_ref()
                .ok_or(ExecError::MissingCredentials("--hyperliquid-private-key"))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        #[cfg(feature = "cex")]
        Exchange::Binance => {
            let ex = order.binance.as_ref().ok_or(ExecError::MissingCredentials(
                "--binance-api-key/--binance-api-secret",
            ))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        #[cfg(not(feature = "cex"))]
        Exchange::Binance => Err(ExecError::UnsupportedExchange(Exchange::Binance)),
        Exchange::Dydx => {
            let ex = order
                .dydx
                .as_ref()
                .ok_or(ExecError::MissingCredentials("--dydx-private-key"))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        other => Err(ExecError::UnsupportedExchange(other)),
    }
}

/// Startup checks for a venue role before the purchase loop runs.
pub(crate) trait PurchaseVenueModule {
    fn preflight(ctx: &ExecutorContext) -> Result<(), &'static str>;
}

pub struct PurchaseManager {
    rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>,
    order: ExecutorContext,
    perp_symbol: String,
    notional_usd_per_leg: u64,
    execute_live: bool,
}

impl PurchaseManager {
    pub fn new(rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>, args: Args) -> Self {
        let notional_usd_per_leg = args.clamped_notional_usd_per_leg();
        let perp_symbol = args.perp_symbol.clone();
        let execute_live = args.execute_live;
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
            execute_live,
        }
    }

    fn run_venue_preflight(&self) -> Result<(), &'static str> {
        if !self.execute_live {
            // Dry run mode: allow running market data + detection without secrets.
            return Ok(());
        }
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

            if self.execute_live {
                self.submit_cross_legs(&route, qty_sats).await;
            } else {
                println!(
                    "DRY RUN (--execute-live=0): would LONG on {:?} and SHORT on {:?} (qty_e8={}, notional_usd_per_leg={})",
                    route.buy_exchange, route.sell_exchange, qty_sats, self.notional_usd_per_leg
                );
            }
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
        // IOC-style limit orders: submit the first leg; only attempt the second if the first
        // submission succeeded. (This does NOT guarantee fills; it guarantees we don't fire the
        // second leg when the first leg is rejected at submit-time.)
        let first = LimitOrderRequest {
            exchange: route.buy_exchange,
            symbol: sym.clone(),
            side: OrderSide::Buy,
            price_cents: route.buy_price,
            qty_sats,
            post_only: false,
            reduce_only: false,
        };
        let res_first = submit_limit_order(&self.order, first).await;
        println!("First leg submit result: {:?}", res_first);
        if res_first.is_err() {
            println!("Skipping second leg: first leg failed at submit-time.");
            return;
        }

        let second = LimitOrderRequest {
            exchange: route.sell_exchange,
            symbol: sym,
            side: OrderSide::Sell,
            price_cents: route.sell_price,
            qty_sats,
            post_only: false,
            reduce_only: false,
        };
        let res_second = submit_limit_order(&self.order, second).await;
        println!("Second leg submit result: {:?}", res_second);
    }
}
