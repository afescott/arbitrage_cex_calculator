//! Purchase: perp limit-order execution (build → sign → send) and the arb purchase loop.
//!
//! Shared request/response types and [`OrderExecutor`] live here; Hyperliquid and dYdX live in
//! [`hl`] and [`dydx`]. Binance USDⓈ-M futures (HMAC) is in [`binance`] behind `feature = "cex"`.

#[cfg(feature = "cex")]
mod binance;
#[cfg(feature = "bitget")]
mod bitget;
#[cfg(feature = "dydx")]
mod dydx;
mod hl;

use crate::{args::Args, calculation::BuyExchangeSellExchange, orderbook::book::Exchange, sizing};
#[cfg(feature = "csv")]
use crate::metrics::csv::{CsvEvent, CsvOrderAttempt, CsvRouteOutcome};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Market order (Bitget: `orderType=market`; price ignored).
    pub market: bool,
    /// Hedge retry: cross the book more aggressively on Hyperliquid IOC.
    pub aggressive_hedge: bool,
}

#[derive(Debug, Clone)]
pub struct OrderAck {
    pub exchange: Exchange,
    pub client_order_id: String,
    /// Filled base quantity × 1e8, if the venue reports an immediate fill amount.
    ///
    /// - `Some(0)`: explicitly known to be unfilled at submit-time
    /// - `Some(n>0)`: known immediate filled size (may be partial)
    /// - `None`: venue/transport did not provide fill information
    pub filled_qty_e8: Option<u64>,
    /// Venue order id, when available.
    pub venue_order_id: Option<u64>,
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
    #[cfg(feature = "bitget")]
    bitget: Option<bitget::BitgetExecutor>,
    #[cfg(feature = "dydx")]
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
        #[cfg(feature = "bitget")]
        let bitget = match (
            args.bitget_api_key.clone(),
            args.bitget_api_secret.clone(),
            args.bitget_passphrase.clone(),
        ) {
            (Some(k), Some(s), Some(p)) => Some(bitget::BitgetExecutor::new(k, s, p)),
            _ => None,
        };
        // For relay-based execution, the Rust binary does NOT need the signing material.
        // We still create a Dydx executor if either a local key OR a relay URL is provided.
        #[cfg(feature = "dydx")]
        let dydx = match (args.dydx_private_key.clone(), args.dydx_order_relay_url.clone()) {
            (Some(pk), relay) => Some(dydx::DydxExecutor::new(Some(pk), relay)),
            (None, Some(relay)) => Some(dydx::DydxExecutor::new(None, Some(relay))),
            (None, None) => None,
        };
        Self {
            hyperliquid,
            #[cfg(feature = "cex")]
            binance,
            #[cfg(feature = "bitget")]
            bitget,
            #[cfg(feature = "dydx")]
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
        #[cfg(feature = "dydx")]
        {
            self.dydx.is_some()
        }
        #[cfg(not(feature = "dydx"))]
        {
            false
        }
    }

    #[cfg(feature = "bitget")]
    pub fn has_bitget(&self) -> bool {
        self.bitget.is_some()
    }

    #[cfg(not(feature = "bitget"))]
    pub fn has_bitget(&self) -> bool {
        false
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
        #[cfg(feature = "bitget")]
        Exchange::Bitget => {
            let ex = order
                .bitget
                .as_ref()
                .ok_or(ExecError::MissingCredentials(
                    "--bitget-api-key/--bitget-api-secret/--bitget-passphrase",
                ))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        #[cfg(not(feature = "bitget"))]
        Exchange::Bitget => Err(ExecError::UnsupportedExchange(Exchange::Bitget)),
        #[cfg(feature = "dydx")]
        Exchange::Dydx => {
            let ex = order
                .dydx
                .as_ref()
                .ok_or(ExecError::MissingCredentials("--dydx-private-key"))?;
            let built = ex.build_payload(&req)?;
            let signed = ex.sign(built)?;
            ex.send(signed).await
        }
        #[cfg(not(feature = "dydx"))]
        Exchange::Dydx => Err(ExecError::UnsupportedExchange(Exchange::Dydx)),
        other => Err(ExecError::UnsupportedExchange(other)),
    }
}

/// Startup checks for a venue role before the purchase loop runs.
pub(crate) trait PurchaseVenueModule {
    fn preflight(ctx: &ExecutorContext) -> Result<(), &'static str>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct HlPositionLedger {
    /// Signed net base qty × 1e8: positive = long, negative = short.
    pub net_qty_e8: i64,
}

impl HlPositionLedger {
    pub fn apply_fill(&mut self, side: OrderSide, closing: bool, filled_e8: u64) {
        if filled_e8 == 0 {
            return;
        }
        let delta = match (side, closing) {
            (OrderSide::Buy, false) => filled_e8 as i64,
            (OrderSide::Sell, false) => -(filled_e8 as i64),
            (OrderSide::Buy, true) => filled_e8 as i64,
            (OrderSide::Sell, true) => -(filled_e8 as i64),
        };
        self.net_qty_e8 += delta;
    }
}

#[cfg(feature = "bitget")]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BitgetPositionLedger {
    pub long_qty_e8: u64,
    pub short_qty_e8: u64,
}

#[cfg(feature = "bitget")]
impl BitgetPositionLedger {
    pub fn has_long(&self) -> bool {
        self.long_qty_e8 > 0
    }

    pub fn has_short(&self) -> bool {
        self.short_qty_e8 > 0
    }

    pub fn apply_fill(&mut self, side: OrderSide, closing: bool, filled_e8: u64) {
        if filled_e8 == 0 {
            return;
        }
        if closing {
            match side {
                OrderSide::Buy => self.short_qty_e8 = self.short_qty_e8.saturating_sub(filled_e8),
                OrderSide::Sell => self.long_qty_e8 = self.long_qty_e8.saturating_sub(filled_e8),
            }
        } else {
            match side {
                OrderSide::Buy => self.long_qty_e8 = self.long_qty_e8.saturating_add(filled_e8),
                OrderSide::Sell => self.short_qty_e8 = self.short_qty_e8.saturating_add(filled_e8),
            }
        }
    }
}

pub struct PurchaseManager {
    rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>,
    order: ExecutorContext,
    perp_symbol: String,
    notional_usd_per_leg: u64,
    execute_live: bool,
    hl_ledger: HlPositionLedger,
    #[cfg(feature = "bitget")]
    bitget_ledger: BitgetPositionLedger,
    #[cfg(feature = "csv")]
    csv_tx: Option<tokio::sync::mpsc::Sender<CsvEvent>>,
}

impl PurchaseManager {
    #[cfg(feature = "csv")]
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<BuyExchangeSellExchange>,
        args: Args,
        csv_tx: Option<tokio::sync::mpsc::Sender<CsvEvent>>,
    ) -> Self {
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
            hl_ledger: HlPositionLedger::default(),
            #[cfg(feature = "bitget")]
            bitget_ledger: BitgetPositionLedger::default(),
            csv_tx,
        }
    }

    #[cfg(not(feature = "csv"))]
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
            hl_ledger: HlPositionLedger::default(),
            #[cfg(feature = "bitget")]
            bitget_ledger: BitgetPositionLedger::default(),
        }
    }

    fn run_venue_preflight(&self) -> Result<(), &'static str> {
        if !self.execute_live {
            // Dry run mode: allow running market data + detection without secrets.
            return Ok(());
        }
        hl::HyperliquidPurchase::preflight(&self.order)?;
        #[cfg(feature = "dydx")]
        dydx::DydxPurchase::preflight(&self.order)?;
        Ok(())
    }

    /// Minimal perps execution wiring (API spec focus).
    /// For each route: go long on `buy_exchange`, short on `sell_exchange`.
    pub async fn run_purchase_simulation(
        &mut self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        if let Err(msg) = self.run_venue_preflight() {
            eprintln!("{msg}");
            return;
        }

        let mut live_trading_enabled = true;

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        self.flatten_all_positions().await;
                        return;
                    }
                }
                route = self.rx.recv() => {
                    let Some(route) = route else {
                        self.flatten_all_positions().await;
                        return;
                    };
                    if !Self::sizing_ok(self.notional_usd_per_leg, &self.perp_symbol) {
                        continue;
                    }

                    if self.execute_live && !live_trading_enabled {
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
                        if let Err(msg) = self.submit_cross_legs(&route, qty_sats).await {
                            eprintln!("{msg}");
                            eprintln!(
                                "Live trading disabled; still draining routes so market-data feeds stay unblocked."
                            );
                            live_trading_enabled = false;
                            continue;
                        }
                    } else {
                        println!(
                            "DRY RUN (--execute-live=0): would LONG on {:?} and SHORT on {:?} (qty_e8={}, notional_usd_per_leg={})",
                            route.buy_exchange, route.sell_exchange, qty_sats, self.notional_usd_per_leg
                        );
                    }
                }
            }
        }
    }

    #[cfg(feature = "bitget")]
    async fn flatten_bitget_positions(&mut self) {
        if !self.execute_live || !self.order.has_bitget() {
            return;
        }
        let sym = self.perp_symbol.clone();
        if self.bitget_ledger.has_short() {
            let qty = self.bitget_ledger.short_qty_e8;
            eprintln!("Bitget flatten: market close short qty_e8={qty}");
            let req = LimitOrderRequest {
                exchange: Exchange::Bitget,
                symbol: sym.clone(),
                side: OrderSide::Buy,
                price_cents: 0,
                qty_sats: qty,
                post_only: false,
                reduce_only: true,
                market: true,
                aggressive_hedge: false,
            };
            if let Ok(ack) = self.submit_bitget_order(req).await {
                if let Some(filled) = ack.filled_qty_e8 {
                    self.bitget_ledger.apply_fill(OrderSide::Buy, true, filled);
                }
            }
        }
        if self.bitget_ledger.has_long() {
            let qty = self.bitget_ledger.long_qty_e8;
            eprintln!("Bitget flatten: market close long qty_e8={qty}");
            let req = LimitOrderRequest {
                exchange: Exchange::Bitget,
                symbol: sym,
                side: OrderSide::Sell,
                price_cents: 0,
                qty_sats: qty,
                post_only: false,
                reduce_only: true,
                market: true,
                aggressive_hedge: false,
            };
            if let Ok(ack) = self.submit_bitget_order(req).await {
                if let Some(filled) = ack.filled_qty_e8 {
                    self.bitget_ledger.apply_fill(OrderSide::Sell, true, filled);
                }
            }
        }
    }

    #[cfg(not(feature = "bitget"))]
    async fn flatten_bitget_positions(&mut self) {}

    async fn flatten_hyperliquid_positions(&mut self) {
        if !self.execute_live || !self.order.has_hyperliquid() {
            return;
        }
        let sym = self.perp_symbol.clone();
        let net = self.hl_ledger.net_qty_e8;
        if net > 0 {
            let qty = net as u64;
            eprintln!("Hyperliquid flatten: IOC sell reduce qty_e8={qty}");
            let req = LimitOrderRequest {
                exchange: Exchange::Hyperliquid,
                symbol: sym.clone(),
                side: OrderSide::Sell,
                price_cents: 0,
                qty_sats: qty,
                post_only: false,
                reduce_only: true,
                market: false,
                aggressive_hedge: true,
            };
            let _ = self.submit_hyperliquid_order(req).await;
        } else if net < 0 {
            let qty = (-net) as u64;
            eprintln!("Hyperliquid flatten: IOC buy reduce qty_e8={qty}");
            let req = LimitOrderRequest {
                exchange: Exchange::Hyperliquid,
                symbol: sym,
                side: OrderSide::Buy,
                price_cents: 0,
                qty_sats: qty,
                post_only: false,
                reduce_only: true,
                market: false,
                aggressive_hedge: true,
            };
            let _ = self.submit_hyperliquid_order(req).await;
        }
    }

    async fn flatten_all_positions(&mut self) {
        self.flatten_bitget_positions().await;
        self.flatten_hyperliquid_positions().await;
    }

    async fn submit_hyperliquid_order(
        &mut self,
        req: LimitOrderRequest,
    ) -> Result<OrderAck, ExecError> {
        let closing = req.reduce_only;
        let side = req.side;
        let ack = submit_limit_order(&self.order, req).await?;
        if let Some(filled) = ack.filled_qty_e8 {
            self.hl_ledger.apply_fill(side, closing, filled);
        }
        Ok(ack)
    }

    #[cfg(feature = "bitget")]
    async fn submit_bitget_order(
        &mut self,
        req: LimitOrderRequest,
    ) -> Result<OrderAck, ExecError> {
        if !req.reduce_only {
            if req.side == OrderSide::Buy && self.bitget_ledger.has_short() {
                let qty = self.bitget_ledger.short_qty_e8;
                eprintln!("Bitget: closing short qty_e8={qty} before open long");
                let close = LimitOrderRequest {
                    exchange: Exchange::Bitget,
                    symbol: req.symbol.clone(),
                    side: OrderSide::Buy,
                    price_cents: 0,
                    qty_sats: qty,
                    post_only: false,
                    reduce_only: true,
                    market: true,
                    aggressive_hedge: false,
                };
                let ack = submit_limit_order(&self.order, close).await?;
                if let Some(filled) = ack.filled_qty_e8 {
                    self.bitget_ledger.apply_fill(OrderSide::Buy, true, filled);
                }
            } else if req.side == OrderSide::Sell && self.bitget_ledger.has_long() {
                let qty = self.bitget_ledger.long_qty_e8;
                eprintln!("Bitget: closing long qty_e8={qty} before open short");
                let close = LimitOrderRequest {
                    exchange: Exchange::Bitget,
                    symbol: req.symbol.clone(),
                    side: OrderSide::Sell,
                    price_cents: 0,
                    qty_sats: qty,
                    post_only: false,
                    reduce_only: true,
                    market: true,
                    aggressive_hedge: false,
                };
                let ack = submit_limit_order(&self.order, close).await?;
                if let Some(filled) = ack.filled_qty_e8 {
                    self.bitget_ledger.apply_fill(OrderSide::Sell, true, filled);
                }
            }
        }

        let closing = req.reduce_only;
        let side = req.side;
        let ack = submit_limit_order(&self.order, req).await?;
        if let Some(filled) = ack.filled_qty_e8 {
            self.bitget_ledger.apply_fill(side, closing, filled);
        }
        Ok(ack)
    }

    #[cfg(feature = "bitget")]
    async fn submit_order(
        &mut self,
        req: LimitOrderRequest,
    ) -> Result<OrderAck, ExecError> {
        match req.exchange {
            Exchange::Bitget if self.execute_live => self.submit_bitget_order(req).await,
            Exchange::Hyperliquid if self.execute_live => self.submit_hyperliquid_order(req).await,
            _ => submit_limit_order(&self.order, req).await,
        }
    }

    #[cfg(not(feature = "bitget"))]
    async fn submit_order(
        &mut self,
        req: LimitOrderRequest,
    ) -> Result<OrderAck, ExecError> {
        if req.exchange == Exchange::Hyperliquid && self.execute_live {
            self.submit_hyperliquid_order(req).await
        } else {
            submit_limit_order(&self.order, req).await
        }
    }

    fn sizing_ok(notional_usd_per_leg: u64, perp_symbol: &str) -> bool {
        let _ = (notional_usd_per_leg, perp_symbol);
        // No global min-notional gate here. Venue-specific checks still apply at execution time
        // (e.g., Hyperliquid enforces a ~$10 min notional and the executor already bumps size).
        true
    }

    async fn submit_cross_legs(
        &mut self,
        route: &BuyExchangeSellExchange,
        qty_sats: u64,
    ) -> Result<(), String> {
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
            market: false,
            aggressive_hedge: false,
        };
        let res_first = self.submit_order(first).await;
        println!("First leg submit result: {:?}", res_first);
        #[cfg(feature = "csv")]
        match res_first.as_ref() {
            Ok(ack) => {
                self.emit_order_attempt(
                    "first",
                    route.buy_exchange,
                    OrderSide::Buy,
                    route.buy_price,
                    qty_sats,
                    ack.filled_qty_e8,
                    false,
                    false,
                    Ok(()),
                    Some(route.profit_expect),
                )
                .await;
            }
            Err(e) => {
                self.emit_order_attempt(
                    "first",
                    route.buy_exchange,
                    OrderSide::Buy,
                    route.buy_price,
                    qty_sats,
                    None,
                    false,
                    false,
                    Err(format!("{e:?}")),
                    Some(route.profit_expect),
                )
                .await;
            }
        };
        let first_ack = match res_first {
            Ok(a) => a,
            Err(_) => {
                println!("Skipping second leg: first leg failed at submit-time.");
                #[cfg(feature = "csv")]
                self.emit_route_outcome(route, qty_sats, false, false, 0, 0, false, false)
                    .await;
                return Ok(());
            }
        };

        // Hedge safety: only place leg2 when we can confirm leg1 actually filled.
        // For Hyperliquid IOC, `filled_qty_e8` is derived from the immediate response.
        // For other venues, fill info is currently unknown; we prefer skipping leg2
        // over risking an unhedged position.
        let filled_qty = match first_ack.filled_qty_e8 {
            Some(0) => {
                println!("Skipping second leg: first leg submit OK but filled_qty_e8=0 (no fill).");
                #[cfg(feature = "csv")]
                self.emit_route_outcome(route, qty_sats, true, false, 0, 0, false, false)
                    .await;
                return Ok(());
            }
            Some(q) => q,
            None => {
                if route.sell_exchange == Exchange::Hyperliquid {
                    println!(
                        "WARNING: first leg submit OK but fill is unknown; still placing Hyperliquid hedge leg (qty_e8={}).",
                        qty_sats
                    );
                    qty_sats
                } else {
                    println!("Skipping second leg: first leg submit OK but fill is unknown (no fill tracking for this venue yet).");
                    #[cfg(feature = "csv")]
                    self.emit_route_outcome(route, qty_sats, true, false, 0, 0, false, false)
                    .await;
                    return Ok(());
                }
            }
        };
        if filled_qty != qty_sats {
            println!(
                "First leg filled partially: requested_qty_e8={} filled_qty_e8={}",
                qty_sats, filled_qty
            );
        }

        let mut hedge_retry = false;
        let mut res_second = self
            .place_hedge_leg(route, &sym, filled_qty, route.sell_price, false)
            .await;
        println!("Second leg submit result: {:?}", res_second);

        if let Some(retry_qty) = Self::hedge_retry_qty(filled_qty, &res_second) {
            hedge_retry = true;
            eprintln!(
                "Hedge incomplete on {:?}: retry once qty_e8={} (market/aggressive IOC)",
                route.sell_exchange, retry_qty
            );
            let res_retry = self
                .place_hedge_leg(route, &sym, retry_qty, route.sell_price, true)
                .await;
            println!("Hedge retry result: {:?}", res_retry);
            res_second = Self::merge_hedge_attempts(&res_second, &res_retry);
        }

        let leg2_filled_e8 = res_second
            .as_ref()
            .ok()
            .and_then(|a| a.filled_qty_e8)
            .unwrap_or(0);
        let hedge_complete = res_second.is_ok()
            && matches!(
                res_second.as_ref().ok().and_then(|a| a.filled_qty_e8),
                Some(h) if h >= filled_qty
            );

        #[cfg(feature = "csv")]
        match res_second.as_ref() {
            Ok(ack) => {
                self.emit_order_attempt(
                    if hedge_retry { "second_retry" } else { "second" },
                    route.sell_exchange,
                    OrderSide::Sell,
                    route.sell_price,
                    filled_qty,
                    ack.filled_qty_e8,
                    false,
                    false,
                    Ok(()),
                    Some(route.profit_expect),
                )
                .await;
            }
            Err(e) => {
                self.emit_order_attempt(
                    if hedge_retry { "second_retry" } else { "second" },
                    route.sell_exchange,
                    OrderSide::Sell,
                    route.sell_price,
                    filled_qty,
                    None,
                    false,
                    false,
                    Err(format!("{e:?}")),
                    Some(route.profit_expect),
                )
                .await;
            }
        };
        #[cfg(feature = "csv")]
        self.emit_route_outcome(
            route,
            filled_qty,
            true,
            res_second.is_ok(),
            filled_qty,
            leg2_filled_e8,
            hedge_retry,
            hedge_complete,
        )
        .await;

        if filled_qty > 0 {
            if let Err(e) = &res_second {
                return Err(format!(
                    "HALT: first leg filled_qty_e8={} on {:?} but hedge leg submit failed ({e:?}); refusing further orders.",
                    filled_qty, route.buy_exchange
                ));
            }
            if let Ok(ack) = &res_second {
                if let Some(reason) = Self::hedge_fill_mismatch(filled_qty, route, ack) {
                    return Err(reason);
                }
            }
        }
        Ok(())
    }

    async fn place_hedge_leg(
        &mut self,
        route: &BuyExchangeSellExchange,
        sym: &str,
        qty_sats: u64,
        price_cents: u64,
        aggressive: bool,
    ) -> Result<OrderAck, ExecError> {
        let use_market = aggressive && route.sell_exchange == Exchange::Bitget;
        let req = LimitOrderRequest {
            exchange: route.sell_exchange,
            symbol: sym.to_string(),
            side: OrderSide::Sell,
            price_cents,
            qty_sats,
            post_only: false,
            reduce_only: false,
            market: use_market,
            aggressive_hedge: aggressive && route.sell_exchange == Exchange::Hyperliquid,
        };
        self.submit_order(req).await
    }

    /// Remaining hedge size to retry: submit error, zero fill, partial, or unknown fill.
    fn hedge_retry_qty(leg1_filled: u64, res: &Result<OrderAck, ExecError>) -> Option<u64> {
        match res {
            Err(_) => Some(leg1_filled),
            Ok(ack) => match ack.filled_qty_e8 {
                Some(h) if h >= leg1_filled => None,
                Some(h) => Some(leg1_filled.saturating_sub(h)),
                None => Some(leg1_filled),
            },
        }
    }

    fn merge_hedge_attempts(
        first: &Result<OrderAck, ExecError>,
        second: &Result<OrderAck, ExecError>,
    ) -> Result<OrderAck, ExecError> {
        let f1 = first
            .as_ref()
            .ok()
            .and_then(|a| a.filled_qty_e8)
            .unwrap_or(0);
        match second {
            Ok(a2) => {
                let f2 = a2.filled_qty_e8.unwrap_or(0);
                let total = f1.saturating_add(f2);
                let filled_qty_e8 = match (
                    first.as_ref().ok().and_then(|a| a.filled_qty_e8),
                    a2.filled_qty_e8,
                ) {
                    (None, None) => None,
                    _ => Some(total),
                };
                Ok(OrderAck {
                    exchange: a2.exchange,
                    client_order_id: a2.client_order_id.clone(),
                    filled_qty_e8,
                    venue_order_id: a2.venue_order_id,
                })
            }
            Err(_) => match first {
                Ok(a) => Ok(a.clone()),
                Err(e) => Err(ExecError::SendFailed(format!("{e:?}"))),
            },
        }
    }

    /// Halt when leg1 filled but leg2 did not fully hedge (submit OK but zero/partial/unknown fill).
    fn hedge_fill_mismatch(
        leg1_filled: u64,
        route: &BuyExchangeSellExchange,
        leg2_ack: &OrderAck,
    ) -> Option<String> {
        match leg2_ack.filled_qty_e8 {
            Some(0) => Some(format!(
                "HALT: first leg filled_qty_e8={} on {:?}, hedge filled_qty_e8=0 on {:?}; refusing further orders.",
                leg1_filled, route.buy_exchange, route.sell_exchange
            )),
            Some(hedge_filled) if hedge_filled < leg1_filled => Some(format!(
                "HALT: first leg filled_qty_e8={} on {:?}, hedge filled_qty_e8={} on {:?}; refusing further orders.",
                leg1_filled, route.buy_exchange, hedge_filled, route.sell_exchange
            )),
            None => Some(format!(
                "HALT: first leg filled_qty_e8={} on {:?}, hedge fill unknown on {:?}; refusing further orders.",
                leg1_filled, route.buy_exchange, route.sell_exchange
            )),
            _ => None,
        }
    }
}

#[cfg(feature = "csv")]
impl PurchaseManager {
    async fn emit_order_attempt(
        &self,
        leg: &'static str,
        exchange: Exchange,
        side: OrderSide,
        price_cents: u64,
        qty_e8: u64,
        filled_qty_e8: Option<u64>,
        post_only: bool,
        reduce_only: bool,
        result: Result<(), String>,
        profit_expect_bps: Option<u64>,
    ) {
        let Some(tx) = self.csv_tx.as_ref() else {
            return;
        };
        let (ok, err) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        let ev = CsvOrderAttempt {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            leg,
            exchange,
            side,
            price_cents,
            qty_e8,
            filled_qty_e8,
            post_only,
            reduce_only,
            ok,
            err,
            profit_expect_bps,
        };
        // Best-effort: if metrics channel is full, drop the record rather than stalling execution.
        let _ = tx.try_send(CsvEvent::OrderAttempt(ev));
    }

    async fn emit_route_outcome(
        &self,
        route: &BuyExchangeSellExchange,
        qty_e8: u64,
        first_ok: bool,
        second_ok: bool,
        leg1_filled_e8: u64,
        leg2_filled_e8: u64,
        hedge_retry: bool,
        hedge_complete: bool,
    ) {
        let Some(tx) = self.csv_tx.as_ref() else {
            return;
        };
        let ev = CsvRouteOutcome {
            buy_exchange: route.buy_exchange,
            sell_exchange: route.sell_exchange,
            buy_price_cents: route.buy_price,
            sell_price_cents: route.sell_price,
            qty_e8,
            profit_expect_bps: route.profit_expect,
            first_ok,
            second_ok,
            leg1_filled_e8,
            leg2_filled_e8,
            hedge_retry,
            hedge_complete,
        };
        let _ = tx.try_send(CsvEvent::RouteOutcome(ev));
    }
}
