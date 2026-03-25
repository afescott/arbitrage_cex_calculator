use crate::orderbook::book::Exchange;

/// Exchange fee structure - fees are in basis points (1 basis point = 0.01%)
/// Typical fees: Maker 0.1% = 10 bps, Taker 0.2% = 20 bps
#[derive(Debug, Clone, Copy)]
pub struct ExchangeFee {
    pub maker_bps: u64, // Maker fee in basis points
    pub taker_bps: u64, // Taker fee in basis points
}

impl ExchangeFee {
    pub fn new(maker_bps: u64, taker_bps: u64) -> Self {
        Self {
            maker_bps,
            taker_bps,
        }
    }
}

/// A compact estimate for a single cross-exchange round trip.
/// Assumes one buy leg and one sell leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePnlEstimate {
    /// Trade size in cents (e.g. $10.00 => 1000)
    pub notional_cents: u64,
    /// Gross spread used for simulation (in basis points)
    pub gross_spread_bps: u64,
    /// Round-trip taker fees in cents
    pub fees_cents: u64,
    /// Gross PnL before fees in cents
    pub gross_pnl_cents: u64,
    /// Net PnL after fees in cents
    pub net_pnl_cents: i64,
}

pub struct PurchaseOption {
    exchange_type_price: (Exchange, u64),
    others: [Exchange; 2],
}

/// Fee calculator for different exchanges
///
pub struct FeeCalculator {
    rx: tokio::sync::mpsc::Receiver<PurchaseOption>,
}

impl FeeCalculator {
    /// Get fee structure for a specific exchange
    pub fn get_exchange_fee(exchange: Exchange) -> ExchangeFee {
        match exchange {
            Exchange::Binance => ExchangeFee::new(10, 20), // 0.1% maker, 0.2% taker
            Exchange::Coinbase => ExchangeFee::new(10, 20), // 0.1% maker, 0.2% taker
            Exchange::Kraken => ExchangeFee::new(16, 26),  // 0.16% maker, 0.26% taker
            Exchange::Hyperliquid => ExchangeFee::new(2, 2), // 0.02% maker/taker (very low fees!)
        }
    }

    /// Calculate total fees for a buy and sell order
    /// For arbitrage, we typically use taker fees on both sides (market orders)
    ///
    /// # Arguments
    /// * `buy_price` - Price per unit for buy order (in cents)
    /// * `sell_price` - Price per unit for sell order (in cents)
    /// * `quantity` - Quantity to trade
    /// * `buy_exchange` - Exchange where we buy
    /// * `sell_exchange` - Exchange where we sell
    ///
    /// # Returns
    /// Total fees in cents
    pub fn calculate_total_fees(
        buy_price: u64,
        sell_price: u64,
        quantity: u64,
        buy_exchange: Exchange,
        sell_exchange: Exchange,
    ) -> u64 {
        let buy_fee = Self::get_exchange_fee(buy_exchange);
        let sell_fee = Self::get_exchange_fee(sell_exchange);

        // Calculate fees: (price * quantity * fee_bps) / 10000
        let buy_order_value = buy_price.saturating_mul(quantity);
        let sell_order_value = sell_price.saturating_mul(quantity);

        let buy_fee_amount = (buy_order_value * buy_fee.taker_bps) / 10000;
        let sell_fee_amount = (sell_order_value * sell_fee.taker_bps) / 10000;

        buy_fee_amount + sell_fee_amount
    }

    /// Calculate fees for a single order
    pub fn calculate_order_fee(
        price: u64,
        quantity: u64,
        exchange: Exchange,
        is_taker: bool,
    ) -> u64 {
        let fee = Self::get_exchange_fee(exchange);
        let fee_bps = if is_taker {
            fee.taker_bps
        } else {
            fee.maker_bps
        };
        let order_value = price.saturating_mul(quantity);
        (order_value * fee_bps) / 10000
    }

    /// Total taker fee rate for a two-leg cross-exchange arbitrage trade.
    /// Returns basis points.
    pub fn round_trip_taker_bps(buy_exchange: Exchange, sell_exchange: Exchange) -> u64 {
        let buy_taker = Self::get_exchange_fee(buy_exchange).taker_bps;
        let sell_taker = Self::get_exchange_fee(sell_exchange).taker_bps;
        buy_taker + sell_taker
    }

    /// Minimum gross spread (in basis points) required to break even on fees
    /// for a pure taker/taker round trip between exchanges.
    pub fn min_break_even_spread_bps(buy_exchange: Exchange, sell_exchange: Exchange) -> u64 {
        Self::round_trip_taker_bps(buy_exchange, sell_exchange)
    }

    /// Simulate net PnL for a round-trip arbitrage on a fixed notional.
    ///
    /// `gross_spread_bps` is the raw cross-exchange price difference in basis points.
    /// Positive net PnL means the opportunity clears fees.
    pub fn estimate_round_trip_pnl(
        notional_cents: u64,
        gross_spread_bps: u64,
        buy_exchange: Exchange,
        sell_exchange: Exchange,
    ) -> RoutePnlEstimate {
        let round_trip_fee_bps = Self::round_trip_taker_bps(buy_exchange, sell_exchange);
        let fees_cents = (notional_cents.saturating_mul(round_trip_fee_bps)) / 10000;
        let gross_pnl_cents = (notional_cents.saturating_mul(gross_spread_bps)) / 10000;
        let net_pnl_cents = gross_pnl_cents as i64 - fees_cents as i64;

        RoutePnlEstimate {
            notional_cents,
            gross_spread_bps,
            fees_cents,
            gross_pnl_cents,
            net_pnl_cents,
        }
    }

    /// Temporary placeholder used by `main.rs` while order execution is under development.
    pub async fn run_purchase_simulation() {
        // Keep the task alive to match the current "spawn and select!" pattern.
        // Replace with real simulation/execution wiring later.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::book::Exchange;

    #[test]
    fn test_fee_calculation() {
        // Buy 1 BTC at $50,000, sell at $50,100
        let buy_price = 5_000_000; // $50,000 in cents
        let sell_price = 5_010_000; // $50,100 in cents
        let quantity = 100_000_000; // 1 BTC in satoshis (or smallest unit)

        let fees = FeeCalculator::calculate_total_fees(
            buy_price,
            sell_price,
            quantity,
            Exchange::Binance,
            Exchange::Coinbase,
        );

        // Buy fee: 5,000,000 * 100,000,000 * 20 / 10000 = 1,000,000,000 cents = $10,000
        // Sell fee: 5,010,000 * 100,000,000 * 20 / 10000 = 1,002,000,000 cents = $10,020
        // Total: $20,020
        // Note: This seems high because quantity is in smallest units
        // In practice, you'd normalize quantity to BTC units
    }

    #[test]
    fn test_exchange_fee_differences() {
        let binance_fee = FeeCalculator::get_exchange_fee(Exchange::Binance);
        let kraken_fee = FeeCalculator::get_exchange_fee(Exchange::Kraken);

        assert_eq!(binance_fee.taker_bps, 20);
        assert_eq!(kraken_fee.taker_bps, 26);
        assert!(kraken_fee.taker_bps > binance_fee.taker_bps);
    }

    #[test]
    fn test_min_break_even_spread_bps_for_routes() {
        // Binance taker (20) + Kraken taker (26) = 46 bps => 0.46%
        assert_eq!(
            FeeCalculator::min_break_even_spread_bps(Exchange::Binance, Exchange::Kraken),
            46
        );

        // Binance taker (20) + Hyperliquid taker (2) = 22 bps => 0.22%
        assert_eq!(
            FeeCalculator::min_break_even_spread_bps(Exchange::Binance, Exchange::Hyperliquid),
            22
        );
    }

    #[test]
    fn test_ten_dollar_route_estimate() {
        // $10 notional with Binance <-> Kraken route
        let estimate = FeeCalculator::estimate_round_trip_pnl(
            1_000, // $10.00 in cents
            50,    // 0.50% gross spread
            Exchange::Binance,
            Exchange::Kraken,
        );

        // Gross PnL: $10 * 0.50% = $0.05
        assert_eq!(estimate.gross_pnl_cents, 5);
        // Fees: $10 * 0.46% = $0.046 => 4 cents (integer truncation)
        assert_eq!(estimate.fees_cents, 4);
        assert_eq!(estimate.net_pnl_cents, 1);
    }
}
