//! dYdX-specific purchase preflight (second venue when CEX feeds are disabled).

use crate::api::execution::ExecutorContext;

/// Second-venue checks when Hyperliquid is paired with dYdX (and optionally Binance with `cex`).
pub(crate) struct DydxPurchase;

impl DydxPurchase {
    pub(crate) fn ensure_second_venue(ctx: &ExecutorContext) -> Result<(), &'static str> {
        #[cfg(feature = "cex")]
        {
            if ctx.has_binance() || ctx.has_dydx() {
                Ok(())
            } else {
                Err(
                    "Missing second venue: add `--binance-api-key/--binance-api-secret` and/or `--dydx-private-key`",
                )
            }
        }
        #[cfg(not(feature = "cex"))]
        {
            if ctx.has_dydx() {
                Ok(())
            } else {
                Err("Missing `--dydx-private-key` (and use `--dydx-order-relay-url` for live dYdX POST); build with `--features cex` for Binance/Kraken")
            }
        }
    }
}

impl super::PurchaseVenueModule for DydxPurchase {
    fn preflight(ctx: &ExecutorContext) -> Result<(), &'static str> {
        Self::ensure_second_venue(ctx)
    }
}
