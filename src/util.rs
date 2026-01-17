/// Fast decimal string to cents (u64) parser for low-latency applications
/// Avoids f64 parsing overhead and floating-point arithmetic
/// 
/// Examples:
/// - "95245.75" -> 9524575 (cents)
/// - "100.00" -> 10000 (cents)
/// - "50.5" -> 5050 (cents)
/// - "100" -> 10000 (cents, assumes .00)
pub fn parse_price_cents(s: &str) -> Option<u64> {
    // Find decimal point
    match s.find('.') {
        Some(dot_pos) => {
            // Parse integer part (before decimal)
            let integer_part = s[..dot_pos].parse::<u64>().ok()?;
            
            // Parse fractional part (after decimal)
            let fractional_str = &s[dot_pos + 1..];
            
            // Handle up to 2 decimal places (cents)
            let fractional = match fractional_str.len() {
                0 => 0,
                1 => fractional_str.parse::<u64>().ok()? * 10,
                2 => fractional_str.parse::<u64>().ok()?,
                _ => {
                    // More than 2 decimal places - truncate to 2
                    fractional_str[..2].parse::<u64>().ok()?
                }
            };
            
            // Combine: integer_part * 100 + fractional
            Some(integer_part * 100 + fractional)
        }
        None => {
            // No decimal point - treat as whole dollars
            s.parse::<u64>().ok().map(|v| v * 100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_cents() {
        assert_eq!(parse_price_cents("95245.75"), Some(9524575));
        assert_eq!(parse_price_cents("100.00"), Some(10000));
        assert_eq!(parse_price_cents("50.5"), Some(5050));
        assert_eq!(parse_price_cents("0.01"), Some(1));
        assert_eq!(parse_price_cents("95245.75000000"), Some(9524575)); // Truncates extra decimals
        assert_eq!(parse_price_cents("100"), Some(10000)); // No decimal point - assumes .00
        assert_eq!(parse_price_cents("0"), Some(0));
    }
}
