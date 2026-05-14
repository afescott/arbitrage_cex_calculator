//! Arbitrage detection and route payloads.
#![allow(dead_code)]

use crate::orderbook::book::Exchange;
use std::time::Instant;

/// Represents a detected arbitrage opportunity
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    /// Exchange where we should buy
    pub buy_exchange: Exchange,
    /// Exchange where we should sell
    pub sell_exchange: Exchange,
    /// Price per unit to buy at (in cents)
    pub buy_price: u64,
    /// Price per unit to sell at (in cents)
    pub sell_price: u64,
    /// Net profit after fees: `(sell_price - buy_price)` per unit × `quantity`, minus fee terms
    /// (same units as `gross_profit_total`: cents × base_qty_e8).
    pub net_profit_total: u64,
    /// Gross profit before fees (same units as `net_profit_total`).
    pub gross_profit_total: u64,
    /// Quantity available for arbitrage
    pub quantity: u64,
    /// When this opportunity was detected
    pub detected_at: Instant,
    /// Age of the opportunity in milliseconds (for staleness checks)
    pub age_ms: u64,
}

impl ArbitrageOpportunity {
    pub fn new(
        buy_exchange: Exchange,
        sell_exchange: Exchange,
        buy_price: u64,
        sell_price: u64,
        quantity: u64,
        gross_profit: u64,
        net_profit: u64,
        detected_at: Instant,
    ) -> Self {
        let age_ms = detected_at.elapsed().as_millis() as u64;
        Self {
            buy_exchange,
            sell_exchange,
            buy_price,
            sell_price,
            net_profit_total: net_profit,
            gross_profit_total: gross_profit,
            quantity,
            detected_at,
            age_ms,
        }
    }

    /// Calculate profit percentage (as basis points)
    pub fn profit_bps(&self) -> u64 {
        if self.buy_price == 0 || self.quantity == 0 {
            return 0;
        }
        // Notional on buy leg in "cents × qty" units matches `net_profit_total` scaling.
        let denom = (self.buy_price as u128).saturating_mul(self.quantity as u128);
        if denom == 0 {
            return 0;
        }
        ((self.net_profit_total as u128) * 10_000 / denom) as u64
    }

    /// Check if opportunity is stale (older than threshold)
    pub fn is_stale(&self, max_age_ms: u64) -> bool {
        self.age_ms > max_age_ms
    }
}

/// Route snapshot for downstream fee / execution logic. Prices are in cents; `profit_expect` is basis points.
#[derive(Debug, Clone, Copy)]
pub struct BuyExchangeSellExchange {
    pub buy_price: u64,
    pub profit_expect: u64,
    pub sell_price: u64,
    pub sell_exchange: Exchange,
    pub buy_exchange: Exchange,
}

/// Arbitrage detection logic
pub struct ArbitrageDetector {
    /// Minimum profit threshold in cents
    min_profit_cents: u64,
    /// Minimum profit as percentage (basis points)
    min_profit_bps: u64,
    /// Maximum age for opportunities in milliseconds
    max_age_ms: u64,
}

impl ArbitrageDetector {
    pub fn new(min_profit_cents: u64, min_profit_bps: u64, max_age_ms: u64) -> Self {
        Self {
            min_profit_cents,
            min_profit_bps,
            max_age_ms,
        }
    }

    /// Default detector with reasonable thresholds
    pub fn default() -> Self {
        Self::new(
            1000, // $10 minimum profit
            10,   // 0.1% minimum profit percentage
            100,  // 100ms max age
        )
    }

    /// Check for arbitrage opportunity when we want to buy
    ///
    /// # Arguments
    /// * `our_bid_price` - Price we're willing to pay (in cents)
    /// * `our_exchange` - Exchange where we want to buy
    /// * `best_ask` - Best ask price and quantity available elsewhere (price, quantity, exchange)
    /// * `desired_quantity` - Quantity we want to trade
    ///
    /// # Returns
    /// `Some(ArbitrageOpportunity)` if profitable opportunity exists, `None` otherwise
    pub fn check_buy_opportunity(
        &self,
        our_bid_price: u64,
        our_exchange: Exchange,
        best_ask: Option<(u64, u64, Exchange)>,
        desired_quantity: u64,
    ) -> Option<ArbitrageOpportunity> {
        let (best_ask_price, best_ask_quantity, best_ask_exchange) = best_ask?;

        // Can't arbitrage on same exchange
        if best_ask_exchange == our_exchange {
            return None;
        }

        // Check liquidity - must have sufficient quantity available
        if best_ask_quantity < desired_quantity {
            return None;
        }

        // Check if we can buy cheaper elsewhere
        if best_ask_price >= our_bid_price {
            return None;
        }

        // Calculate gross profit
        let gross_profit = our_bid_price.saturating_sub(best_ask_price);
        let gross_profit_total = gross_profit.saturating_mul(desired_quantity);

        // Calculate fees
        let fees = crate::calculation::fees::FeeCalculator::calculate_total_fees(
            best_ask_price,
            our_bid_price,
            desired_quantity,
            best_ask_exchange,
            our_exchange,
        );

        // Calculate net profit
        let net_profit = gross_profit_total.saturating_sub(fees);

        // Check if profit meets thresholds
        if !self.is_profitable(net_profit, gross_profit_total, best_ask_price) {
            return None;
        }

        Some(ArbitrageOpportunity::new(
            best_ask_exchange,
            our_exchange,
            best_ask_price,
            our_bid_price,
            desired_quantity,
            gross_profit_total,
            net_profit,
            Instant::now(),
        ))
    }

    /// Check for arbitrage opportunity when we want to sell
    ///
    /// # Arguments
    /// * `our_ask_price` - Price we're willing to sell at (in cents)
    /// * `our_exchange` - Exchange where we want to sell
    /// * `best_bid` - Best bid price and quantity available elsewhere (price, quantity, exchange)
    /// * `desired_quantity` - Quantity we want to trade
    ///
    /// # Returns
    /// `Some(ArbitrageOpportunity)` if profitable opportunity exists, `None` otherwise
    pub fn check_sell_opportunity(
        &self,
        our_ask_price: u64,
        our_exchange: Exchange,
        best_bid: Option<(u64, u64, Exchange)>,
        desired_quantity: u64,
    ) -> Option<ArbitrageOpportunity> {
        let (best_bid_price, best_bid_quantity, best_bid_exchange) = best_bid?;

        // Can't arbitrage on same exchange
        if best_bid_exchange == our_exchange {
            return None;
        }

        // Check liquidity - must have sufficient quantity available
        if best_bid_quantity < desired_quantity {
            return None;
        }

        // Check if we can sell higher elsewhere
        if best_bid_price <= our_ask_price {
            return None;
        }

        // Calculate gross profit
        let gross_profit = best_bid_price.saturating_sub(our_ask_price);
        let gross_profit_total = gross_profit.saturating_mul(desired_quantity);

        // Calculate fees
        let fees = crate::calculation::fees::FeeCalculator::calculate_total_fees(
            our_ask_price,
            best_bid_price,
            desired_quantity,
            our_exchange,
            best_bid_exchange,
        );

        // Calculate net profit
        let net_profit = gross_profit_total.saturating_sub(fees);

        // Check if profit meets thresholds
        if !self.is_profitable(net_profit, gross_profit_total, our_ask_price) {
            return None;
        }

        Some(ArbitrageOpportunity::new(
            our_exchange,
            best_bid_exchange,
            our_ask_price,
            best_bid_price,
            desired_quantity,
            gross_profit_total,
            net_profit,
            Instant::now(),
        ))
    }

    /// Check if an opportunity is profitable based on thresholds
    fn is_profitable(&self, net_profit: u64, _gross_profit: u64, base_price: u64) -> bool {
        // Check minimum profit in cents
        if net_profit < self.min_profit_cents {
            return false;
        }

        // Check minimum profit percentage (basis points)
        if base_price > 0 {
            let profit_bps = (net_profit * 10000) / base_price;
            if profit_bps < self.min_profit_bps {
                return false;
            }
        }

        true
    }

    /// Check if opportunity is stale
    pub fn is_opportunity_stale(&self, opportunity: &ArbitrageOpportunity) -> bool {
        opportunity.is_stale(self.max_age_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::book::Exchange;

    #[test]
    fn test_buy_opportunity_profitable() {
        let detector = ArbitrageDetector::default();

        // We want to buy at $50,000, but can buy at $49,700 elsewhere (spread must clear ~40 bps taker/taker)
        let opportunity = detector.check_buy_opportunity(
            5_000_000, // $50,000 our bid
            Exchange::Binance,
            Some((4_970_000, 2_000_000, Exchange::Coinbase)), // $49,700 best ask, 0.02 BTC available
            1_000_000,                                        // 0.01 BTC (in smallest units)
        );

        assert!(opportunity.is_some());
        let opp = opportunity.unwrap();
        assert_eq!(opp.buy_exchange, Exchange::Coinbase);
        assert_eq!(opp.sell_exchange, Exchange::Binance);
        assert!(opp.net_profit_total > 0);
        assert!(opp.profit_bps() > 0);
    }

    #[test]
    fn test_buy_opportunity_not_profitable() {
        let detector = ArbitrageDetector::default();

        // Best ask is higher than our bid - no opportunity
        let opportunity = detector.check_buy_opportunity(
            5_000_000, // $50,000 our bid
            Exchange::Binance,
            Some((5_010_000, 2_000_000, Exchange::Coinbase)), // $50,100 best ask (higher!)
            1_000_000,
        );

        assert!(opportunity.is_none());
    }

    #[test]
    fn test_buy_opportunity_insufficient_liquidity() {
        let detector = ArbitrageDetector::default();

        // Good price but insufficient liquidity
        let opportunity = detector.check_buy_opportunity(
            5_000_000, // $50,000 our bid
            Exchange::Binance,
            Some((4_990_000, 500_000, Exchange::Coinbase)), // $49,900 best ask, but only 0.005 BTC available
            1_000_000,                                      // Need 0.01 BTC
        );

        // Should be filtered out due to insufficient liquidity
        assert!(opportunity.is_none());
    }

    #[test]
    fn test_sell_opportunity_profitable() {
        let detector = ArbitrageDetector::default();

        // We want to sell at $50,000, but can sell at $50,300 elsewhere (spread must clear fees)
        let opportunity = detector.check_sell_opportunity(
            5_000_000, // $50,000 our ask
            Exchange::Binance,
            Some((5_030_000, 2_000_000, Exchange::Coinbase)), // $50,300 best bid, 0.02 BTC available
            1_000_000,
        );

        assert!(opportunity.is_some());
        let opp = opportunity.unwrap();
        assert_eq!(opp.buy_exchange, Exchange::Binance);
        assert_eq!(opp.sell_exchange, Exchange::Coinbase);
        assert!(opp.net_profit_total > 0);
        assert!(opp.profit_bps() > 0);
    }

    #[test]
    fn test_same_exchange_no_opportunity() {
        let detector = ArbitrageDetector::default();

        // Same exchange - no arbitrage
        let opportunity = detector.check_buy_opportunity(
            5_000_000,
            Exchange::Binance,
            Some((4_990_000, 2_000_000, Exchange::Binance)), // Same exchange!
            1_000_000,
        );

        assert!(opportunity.is_none());
    }

    #[test]
    fn test_profit_threshold_filtering() {
        // Very strict detector - requires $100 profit
        let strict_detector = ArbitrageDetector::new(10_000, 100, 100);

        // Small profit opportunity - should be filtered out
        let opportunity = strict_detector.check_buy_opportunity(
            5_000_000,
            Exchange::Binance,
            Some((4_999_000, 2_000_000, Exchange::Coinbase)), // Only $10 difference
            1_000_000,
        );

        // Should be filtered out due to strict thresholds
        assert!(opportunity.is_none());
    }
}
