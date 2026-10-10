//! Netting: one net position per (account, market) and `apply_fill` (DESIGN 7.2).
use crate::consts::QUICK_SECS;
use crate::error::{EngineError, Result};
use crate::price::ceil_div_u128;
use crate::units::DEN;

/// Order side. A buy fill adds positive quantity, a sell fill negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// The net position of one market. `qty` is signed in 1e-6 base units (long > 0); a flat market has no `Position`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub qty: i64,
    /// Average entry, 1e-12 USD. Long: rounded up on merges; short: rounded down.
    pub avg_entry_e12: u64,
    /// Start of the quick-trade timer: set when the position opens and on every same-direction add.
    pub timer_start: i64,
    pub opened_at: i64,
    /// Index of the last 22:00 UTC rollover already charged (`cost::day_index`).
    pub fee_day: i64,
}

/// The closing slice of a fill (becomes a `Close` journal event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseSlice {
    pub qty_abs: u64,
    pub entry_e12: u64,
    pub exit_e12: u64,
    /// Realized P&L, micro-USD, floored.
    pub pnl: i64,
    /// `t - timer_start <= 45`.
    pub quick: bool,
    /// Balance credit: a loss in full; a profit in full, or floored to half when quick.
    pub credit: i64,
}

/// The exposure-increasing part of a fill (becomes a `Fill` journal event): an open, an add, or a flip remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenSlice {
    /// Signed: positive for a buy, negative for a sell.
    pub qty: i64,
    pub price_e12: u64,
}

/// Result of `apply_fill`. Fees are NOT included in `balance_delta`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillOutcome {
    pub position: Option<Position>,
    pub closed: Option<CloseSlice>,
    pub opened: Option<OpenSlice>,
    /// Equals the closing slice's `credit` (0 without one).
    pub balance_delta: i64,
    /// `floor(pnl / 2)` for a profitable quick slice, else 0 (losses never reduce the tally).
    pub quick_tally_delta: i64,
}

/// `floor(sign x qty_abs x (exit - entry) / DEN)` in micro-USD, rounding toward minus infinity. `long = false` flips the sign.
pub fn pnl_micro(long: bool, qty_abs: u64, entry_e12: u64, exit_e12: u64) -> Result<i64> {
    let diff = i128::from(exit_e12).checked_sub(i128::from(entry_e12)).ok_or(EngineError::Overflow)?;
    let signed_diff = if long { diff } else { diff.checked_neg().ok_or(EngineError::Overflow)? };
    let num = i128::from(qty_abs).checked_mul(signed_diff).ok_or(EngineError::Overflow)?;
    let q = num.checked_div_euclid(DEN).ok_or(EngineError::Overflow)?;
    i64::try_from(q).map_err(|_| EngineError::Overflow)
}

/// Open P&L at the marks: longs at `mark_bid`, shorts at `mark_ask`, floored. A zero-quantity position is `InvalidInput`.
pub fn unrealized_micro(pos: &Position, mark_bid_e12: u64, mark_ask_e12: u64) -> Result<i64> {
    if pos.qty == 0 {
        return Err(EngineError::InvalidInput);
    }
    let long = pos.qty.signum() == 1;
    let mark = if long { mark_bid_e12 } else { mark_ask_e12 };
    pnl_micro(long, pos.qty.unsigned_abs(), pos.avg_entry_e12, mark)
}

/// Applies a fill to the net position of a market (DESIGN 7.2). `t` is the fill time, `fee_day_now` the current
/// `cost::day_index(t)`, stored on positions that start here.
///
/// Errors: `InvalidQuantity` for `dq == 0` or `dq > i64::MAX`; `InvalidInput` for a zero price, a stored position with
/// zero quantity, or `t < timer_start` (time never runs backwards); `Overflow` for any arithmetic overflow, including a
/// merged quantity beyond `i64::MAX` in magnitude (a short may not reach `i64::MIN`).
pub fn apply_fill(
    pos: Option<Position>,
    side: Side,
    dq: u64,
    price_e12: u64,
    t: i64,
    fee_day_now: i64,
) -> Result<FillOutcome> {
    if dq == 0 {
        return Err(EngineError::InvalidQuantity);
    }
    if price_e12 == 0 {
        return Err(EngineError::InvalidInput);
    }
    let dq_pos = i64::try_from(dq).map_err(|_| EngineError::InvalidQuantity)?;
    let dq_signed = match side {
        Side::Buy => dq_pos,
        Side::Sell => dq_pos.checked_neg().ok_or(EngineError::Overflow)?,
    };

    let Some(p) = pos else {
        // 1. flat -> open
        return Ok(FillOutcome {
            position: Some(Position {
                qty: dq_signed,
                avg_entry_e12: price_e12,
                timer_start: t,
                opened_at: t,
                fee_day: fee_day_now,
            }),
            closed: None,
            opened: Some(OpenSlice { qty: dq_signed, price_e12 }),
            balance_delta: 0,
            quick_tally_delta: 0,
        });
    };
    if p.qty == 0 {
        return Err(EngineError::InvalidInput);
    }
    let age = t.checked_sub(p.timer_start).ok_or(EngineError::Overflow)?;
    if age < 0 {
        return Err(EngineError::InvalidInput);
    }

    let long = p.qty.signum() == 1;
    let q_abs = p.qty.unsigned_abs();

    if long == (side == Side::Buy) {
        // 2. same direction -> merge; long rounds the average up, short down; the timer restarts (decision 10)
        let num = u128::from(q_abs)
            .checked_mul(u128::from(p.avg_entry_e12))
            .and_then(|a| u128::from(dq).checked_mul(u128::from(price_e12)).and_then(|b| a.checked_add(b)))
            .ok_or(EngineError::Overflow)?;
        let den = u128::from(q_abs).checked_add(u128::from(dq)).ok_or(EngineError::Overflow)?;
        let avg = if long {
            ceil_div_u128(num, den)?
        } else {
            num.checked_div(den).ok_or(EngineError::Overflow)?
        };
        let avg_entry_e12 = u64::try_from(avg).map_err(|_| EngineError::Overflow)?;
        let qty = p.qty.checked_add(dq_signed).ok_or(EngineError::Overflow)?;
        if qty == i64::MIN {
            // keep |qty| <= i64::MAX so that longs and shorts are symmetric
            return Err(EngineError::Overflow);
        }
        return Ok(FillOutcome {
            position: Some(Position { qty, avg_entry_e12, timer_start: t, opened_at: p.opened_at, fee_day: p.fee_day }),
            closed: None,
            opened: Some(OpenSlice { qty: dq_signed, price_e12 }),
            balance_delta: 0,
            quick_tally_delta: 0,
        });
    }

    // 3. opposite direction: the closing slice is min(dq, |q|)
    let c = dq.min(q_abs);
    let pnl = pnl_micro(long, c, p.avg_entry_e12, price_e12)?;
    let quick = age <= QUICK_SECS;
    let credit = if quick && pnl > 0 { pnl.div_euclid(2) } else { pnl };
    // the tally takes the credited half-profit of a quick slice; a quick loss or break-even adds nothing
    let quick_tally_delta = if quick { credit.max(0) } else { 0 };
    let closed = CloseSlice { qty_abs: c, entry_e12: p.avg_entry_e12, exit_e12: price_e12, pnl, quick, credit };

    let (position, opened) = if dq < q_abs {
        // partial reduce: average and timer unchanged
        let qty = p.qty.checked_add(dq_signed).ok_or(EngineError::Overflow)?;
        (Some(Position { qty, ..p }), None)
    } else if dq == q_abs {
        // exactly flat
        (None, None)
    } else {
        // flip: the old position closes fully, the remainder opens the other way at `price` with a fresh timer
        let rest = dq.checked_sub(q_abs).ok_or(EngineError::Overflow)?;
        let rest_pos = i64::try_from(rest).map_err(|_| EngineError::Overflow)?;
        let rest_signed = if long { rest_pos.checked_neg().ok_or(EngineError::Overflow)? } else { rest_pos };
        (
            Some(Position {
                qty: rest_signed,
                avg_entry_e12: price_e12,
                timer_start: t,
                opened_at: t,
                fee_day: fee_day_now,
            }),
            Some(OpenSlice { qty: rest_signed, price_e12 }),
        )
    };
    Ok(FillOutcome { position, closed: Some(closed), opened, balance_delta: credit, quick_tally_delta })
}

/// Open profit of all positions: the sum of `unrealized_micro` (floored; longs at bid, shorts at ask) with checked
/// arithmetic. Each entry is `(position, bid, ask)` of its market at the sample.
pub fn open_profit_micro(positions: &[(Position, u64, u64)]) -> Result<i64> {
    let mut total: i64 = 0;
    for (pos, bid, ask) in positions {
        total = total.checked_add(unrealized_micro(pos, *bid, *ask)?).ok_or(EngineError::Overflow)?;
    }
    Ok(total)
}
