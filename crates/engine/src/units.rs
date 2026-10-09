//! Units and the notional formula (DESIGN 7).

/// Notional micro-USD = qty (1e-6 base units) x price (1e-12 USD) / DEN.
pub const DEN: i128 = 1_000_000_000_000;

/// Closed-form notional in micro-USD, rounded toward zero. Callers apply the
/// rounding rule of DESIGN 7.4 for the quantity they are computing.
pub fn notional_micro(qty_abs: u64, price_e12: u64) -> i128 {
    (qty_abs as i128) * (price_e12 as i128) / DEN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DESIGN 4.9: 1,000,000 EUR at 1.0800324 is $1,080,032.40.
    #[test]
    fn worked_example_notional() {
        let qty = 1_000_000u64 * 1_000_000; // 1e6 EUR in 1e-6 units
        let price = 1_080_032_400_000u64; // 1.0800324 in 1e-12 units
        assert_eq!(notional_micro(qty, price), 1_080_032_400_000);
    }
}
