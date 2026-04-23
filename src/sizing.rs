//! Per-leg notional → base-asset quantity in 1e8 fixed-point units (compat with existing executors).
//!
//! Venue-specific lot steps and minimum-notional rules differ; clamping here is conservative.
//! Re-check each exchange’s published `min trade size` / `tick` before live size.

/// Approximate minimum **USD notional** many perp books enforce (order may still reject if stricter).
pub fn conservative_min_notional_usd(_perp_symbol: &str) -> u64 {
    // Practical safety floor: Hyperliquid rejects orders below ~$10 notional.
    // Keeping this conservative avoids spamming venue APIs with guaranteed rejects.
    10
}

/// `price_cents`: quote (e.g. USD) cents per 1 base unit (matches orderbook route prices).
/// Returns base qty × 1e8 (e.g. satoshi-style for BTC), or `None` if inputs are unusable.
pub fn qty_base_e8_from_notional(price_cents: u64, notional_usd: u64) -> Option<u64> {
    if price_cents == 0 || notional_usd == 0 {
        return None;
    }
    let price_usd = price_cents as f64 / 100.0;
    let qty = notional_usd as f64 / price_usd;
    let e8 = (qty * 100_000_000.0).floor();
    if e8 < 1.0 {
        return None;
    }
    Some(e8.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qty_btc_roughly_matches_notional() {
        // $50k / BTC → 5000000 cents
        let cents = 5_000_000u64;
        let q = qty_base_e8_from_notional(cents, 50).expect("qty");
        //50 / 50000 = 0.001 BTC → 100_000 sats in 1e8 terms = 100_000?
        // 0.001 * 1e8 = 100_000
        assert_eq!(q, 100_000);
    }
}
