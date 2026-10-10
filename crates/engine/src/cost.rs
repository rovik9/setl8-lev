//! Margin, fees, day index and order validation (DESIGN 7.3, 7.4, 9.2 to 9.4, 7.6).
use crate::error::{EngineError, Result};
use crate::position::Side;
use crate::price::ceil_div_u128;
use crate::units::DEN;

const E6: u128 = 1_000_000;
const BPS_DEN: u128 = 10_000;
/// Rollover time: the day index changes at 22:00 UTC.
const ROLLOVER_SECS: i64 = 22 * 3600;
const SECS_PER_DAY: i64 = 86_400;

fn den_u128() -> Result<u128> {
    u128::try_from(DEN).map_err(|_| EngineError::Overflow)
}

/// `margin = ceil(|q| x p / DEN / leverage)`: ONE ceiling over the whole expression. Zero leverage is `InvalidInput`.
pub fn margin_micro(qty_abs: u64, price_e12: u64, leverage_x: u16) -> Result<i64> {
    if leverage_x == 0 {
        return Err(EngineError::InvalidInput);
    }
    let num = u128::from(qty_abs).checked_mul(u128::from(price_e12)).ok_or(EngineError::Overflow)?;
    let den = den_u128()?.checked_mul(u128::from(leverage_x)).ok_or(EngineError::Overflow)?;
    i64::try_from(ceil_div_u128(num, den)?).map_err(|_| EngineError::Overflow)
}

/// `ceil(notional x rate / 1e6)` for a notional that is already in micro-USD (commission, swap-cost style fees).
/// A negative notional is `InvalidInput`.
pub fn fee_ceil_micro(notional_micro: i64, rate_e6: u32) -> Result<i64> {
    let n = u128::try_from(notional_micro).map_err(|_| EngineError::InvalidInput)?;
    let num = n.checked_mul(u128::from(rate_e6)).ok_or(EngineError::Overflow)?;
    i64::try_from(ceil_div_u128(num, E6)?).map_err(|_| EngineError::Overflow)
}

/// Holding fee for `nights` rollovers: `ceil(nights x rate_e6 x |q| x p / (DEN x 1e6))`, one ceiling over the exact
/// expression (the notional is not rounded first). The mark is the one in force when the fee is applied (DESIGN 9.3).
pub fn holding_fee_micro(qty_abs: u64, mark_price_e12: u64, rate_e6: u32, nights: u32) -> Result<i64> {
    let notional_e18 = u128::from(qty_abs).checked_mul(u128::from(mark_price_e12)).ok_or(EngineError::Overflow)?;
    let num = notional_e18
        .checked_mul(u128::from(rate_e6))
        .and_then(|n| n.checked_mul(u128::from(nights)))
        .ok_or(EngineError::Overflow)?;
    let den = den_u128()?.checked_mul(E6).ok_or(EngineError::Overflow)?;
    i64::try_from(ceil_div_u128(num, den)?).map_err(|_| EngineError::Overflow)
}

/// Index of the last 22:00 UTC rollover at `unix_secs`: `floor((t - 22 h) / 1 day)`; it changes at 22:00 UTC.
pub fn day_index(unix_secs: i64) -> i64 {
    let shifted = i128::from(unix_secs).saturating_sub(i128::from(ROLLOVER_SECS));
    // the quotient is at most about 1e14 in magnitude, so the conversion cannot fail
    i64::try_from(shifted.div_euclid(i128::from(SECS_PER_DAY))).unwrap_or(i64::MIN)
}

/// Number of rollovers to charge: `today_index - fee_day`. `today < fee_day` is `InvalidInput`.
pub fn nights_due(fee_day: i64, today_index: i64) -> Result<u32> {
    if today_index < fee_day {
        return Err(EngineError::InvalidInput);
    }
    let n = today_index.checked_sub(fee_day).ok_or(EngineError::Overflow)?;
    u32::try_from(n).map_err(|_| EngineError::Overflow)
}

/// The 0.4%-style fee on a tier price: `ceil(cost x bps / 10_000)` (DESIGN 9.2).
pub fn swap_fee_micro(tier_cost_micro: u64, fee_bps: u16) -> Result<u64> {
    let num = u128::from(tier_cost_micro).checked_mul(u128::from(fee_bps)).ok_or(EngineError::Overflow)?;
    u64::try_from(ceil_div_u128(num, BPS_DEN)?).map_err(|_| EngineError::Overflow)
}

/// N-1: the quantity must be non-zero, a multiple of `qty_step` and at least `qty_min`. A zero `qty_step` is a
/// misconfigured market and is `InvalidInput`.
pub fn validate_quantity(qty: u64, qty_step: u64, qty_min: u64) -> Result<()> {
    if qty_step == 0 {
        return Err(EngineError::InvalidInput);
    }
    let rem = qty.checked_rem(qty_step).ok_or(EngineError::Overflow)?;
    if qty == 0 || rem != 0 || qty < qty_min {
        return Err(EngineError::InvalidQuantity);
    }
    Ok(())
}

/// Which protective order is being validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopKind {
    StopLoss,
    TakeProfit,
}

/// N-4. `position_side` is the side of the position the order protects (`Buy` = long). A long closes at the bid, a
/// short at the ask. Valid levels (equality is VALID: touching counts as triggered, N-10):
/// long SL `level <= bid`, long TP `level >= bid`, short SL `level >= ask`, short TP `level <= ask`.
/// A level of 0 (the journal's "none") is `InvalidStopLevel`.
pub fn validate_stop_level(
    position_side: Side,
    kind: StopKind,
    level_e12: u64,
    bid_e12: u64,
    ask_e12: u64,
) -> Result<()> {
    if level_e12 == 0 {
        return Err(EngineError::InvalidStopLevel);
    }
    let ok = match (position_side, kind) {
        (Side::Buy, StopKind::StopLoss) => level_e12 <= bid_e12,
        (Side::Buy, StopKind::TakeProfit) => level_e12 >= bid_e12,
        (Side::Sell, StopKind::StopLoss) => level_e12 >= ask_e12,
        (Side::Sell, StopKind::TakeProfit) => level_e12 <= ask_e12,
    };
    if ok {
        Ok(())
    } else {
        Err(EngineError::InvalidStopLevel)
    }
}
