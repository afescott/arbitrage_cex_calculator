//! Hyperliquid-specific purchase preflight and route helpers.

use crate::{api::execution::ExecutorContext, orderbook::book::Exchange};

/// Hyperliquid leg of the purchase flow (credentials, Kraken co-existence with CEX feature).
pub(crate) struct HyperliquidPurchase;

impl HyperliquidPurchase {
    pub(crate) fn ensure(ctx: &ExecutorContext) -> Result<(), &'static str> {
        if ctx.has_hyperliquid() {
            Ok(())
        } else {
            Err("Missing `--hyperliquid-private-key`")
        }
    }

    /// Whether this route touches Kraken limit-order execution (only meaningful with `cex`).
    pub(crate) fn route_hits_unsupported_kraken_legs(buy: Exchange, sell: Exchange) -> bool {
        #[cfg(feature = "cex")]
        {
            buy == Exchange::Kraken || sell == Exchange::Kraken
        }
        #[cfg(not(feature = "cex"))]
        {
            let _ = (buy, sell);
            false
        }
    }
}

impl super::PurchaseVenueModule for HyperliquidPurchase {
    fn preflight(ctx: &ExecutorContext) -> Result<(), &'static str> {
        Self::ensure(ctx)
    }
}
