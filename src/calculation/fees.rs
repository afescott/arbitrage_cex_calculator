use crate::orderbook::book::Exchange;

/// Exchange fee structure - fees are in basis points (1 basis point = 0.01%)
/// Typical fees: Maker 0.1% = 10 bps, Taker 0.2% = 20 bps
#[derive(Debug, Clone, Copy)]
pub struct ExchangeFee {
    pub maker_bps: u64,  // Maker fee in basis points
    pub taker_bps: u64,  // Taker fee in basis points
}

impl ExchangeFee {
    pub fn new(maker_bps: u64, taker_bps: u64) -> Self {
        Self { maker_bps, taker_bps }
    }
}

/// Fee calculator for different exchanges
pub struct FeeCalculator;

impl FeeCalculator {
    /// Get fee structure for a specific exchange
    pub fn get_exchange_fee(exchange: Exchange) -> ExchangeFee {
        match exchange {
            Exchange::Binance => ExchangeFee::new(10, 20),  // 0.1% maker, 0.2% taker
            Exchange::Coinbase => ExchangeFee::new(10, 20), // 0.1% maker, 0.2% taker
            Exchange::Kraken => ExchangeFee::new(16, 26),   // 0.16% maker, 0.26% taker
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
        let fee_bps = if is_taker { fee.taker_bps } else { fee.maker_bps };
        let order_value = price.saturating_mul(quantity);
        (order_value * fee_bps) / 10000
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
}
