//! Units and the notional formula (DESIGN 7).
use crate::error::{EngineError, Result};

/// Notional micro-USD = qty (1e-6 base units) x price (1e-12 USD) / DEN.
pub const DEN: i128 = 1_000_000_000_000;

/// Notional in micro-USD, rounded toward zero. Callers apply the rounding rule of DESIGN 7.4 for the quantity
/// they are computing. `Overflow` if the product does not fit `i128` (only possible near `u64::MAX` x `u64::MAX`).
pub fn notional_micro(qty_abs: u64, price_e12: u64) -> Result<i128> {
    i128::from(qty_abs)
        .checked_mul(i128::from(price_e12))
        .and_then(|n| n.checked_div(DEN))
        .ok_or(EngineError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DESIGN 4.9: 1,000,000 EUR at 1.0800324 is $1,080,032.40.
    #[test]
    fn worked_example_notional() {
        let qty = 1_000_000u64 * 1_000_000; // 1e6 EUR in 1e-6 units
        let price = 1_080_032_400_000u64; // 1.0800324 in 1e-12 units
        assert_eq!(notional_micro(qty, price), Ok(1_080_032_400_000));
    }

    #[test]
    fn notional_overflow_is_an_error_not_a_panic() {
        assert_eq!(notional_micro(u64::MAX, u64::MAX), Err(EngineError::Overflow));
        assert!(notional_micro(u64::MAX, 1).is_ok());
        assert_eq!(notional_micro(0, u64::MAX), Ok(0));
    }
}
