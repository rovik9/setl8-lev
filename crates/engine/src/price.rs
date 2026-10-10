//! Execution prices (DESIGN 7.1, 9.1) and oracle conversion.
use crate::error::{EngineError, Result};

/// Half-spread unit: e6 of the price (30 = 0.003%).
const E6: u128 = 1_000_000;

/// Divides rounding up. `d` must be non-zero.
pub(crate) fn ceil_div_u128(n: u128, d: u128) -> Result<u128> {
    let q = n.checked_div(d).ok_or(EngineError::Overflow)?;
    let r = n.checked_rem(d).ok_or(EngineError::Overflow)?;
    if r == 0 {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(EngineError::Overflow)
    }
}

/// Buy price: `ceil(mid x (1e6 + h) / 1e6)` with `h` the half-spread in e6.
pub fn ask_from_mid(mid_e12: u64, half_spread_e6: u32) -> Result<u64> {
    let factor = E6.checked_add(u128::from(half_spread_e6)).ok_or(EngineError::Overflow)?;
    let num = u128::from(mid_e12).checked_mul(factor).ok_or(EngineError::Overflow)?;
    u64::try_from(ceil_div_u128(num, E6)?).map_err(|_| EngineError::Overflow)
}

/// Sell price: `floor(mid x (1e6 - h) / 1e6)`. A half-spread above 100% (`h > 1e6`) is `InvalidInput`.
pub fn bid_from_mid(mid_e12: u64, half_spread_e6: u32) -> Result<u64> {
    let h = u128::from(half_spread_e6);
    if h > E6 {
        return Err(EngineError::InvalidInput);
    }
    let factor = E6.checked_sub(h).ok_or(EngineError::Overflow)?;
    let num = u128::from(mid_e12).checked_mul(factor).ok_or(EngineError::Overflow)?;
    let q = num.checked_div(E6).ok_or(EngineError::Overflow)?;
    u64::try_from(q).map_err(|_| EngineError::Overflow)
}

/// Oracle value with 18 decimals to the engine's 12-decimal price: `floor(value / 1e6)`. Zero is returned as 0
/// (callers reject a zero price: `apply_fill` does, and the program's proof check does).
pub fn price_from_oracle(value_e18: u128) -> Result<u64> {
    let q = value_e18.checked_div(E6).ok_or(EngineError::Overflow)?;
    u64::try_from(q).map_err(|_| EngineError::Overflow)
}
