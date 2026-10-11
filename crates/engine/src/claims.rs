//! Claim checks used by the program's `post_journal` (DESIGN 4.4 I-5 to I-10): pure functions that take the evaluator's
//! claims, recompute them with the engine's own formulas and constants, and return the new `AccountState` or the exact
//! reason for the refusal. Nothing here has a threshold of its own.
use crate::account::{frac_exact, l_level_for, lock_end, static_floor_micro, tight_levels, AccountState};
use crate::consts::{QUICK_SECS, TIGHT_START_BPS};
use crate::error::EngineError;
use crate::position::{pnl_micro, slice_credit};
use crate::price::{ask_from_mid, bid_from_mid};

/// Why a claim was refused. One variant per reason so a caller can report each precisely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimError {
    /// An engine function failed (overflow and the like).
    Engine(EngineError),
    ZeroQuantity,
    ZeroPrice,
    /// `timer_start` is after the close time.
    TimerAfterTime,
    PnlMismatch,
    QuickMismatch,
    CreditMismatch,
    TightAlreadyOn,
    TightNotOn,
    /// `p_open` is not strictly above 0.01 S.
    TightStartPOpen,
    /// `g_peak` is negative or above `p_open` (G = E - R cannot exceed the open profit).
    TightStartGPeak,
    /// `r` differs from the closed balance.
    TightStartR,
    /// `l_start` differs from `min(F_s + 0.04 S, r)`.
    TightStartLStart,
    /// `l_level` differs from `l_start + max(0, g_peak - 0.01 S)`.
    TightStartLLevel,
    /// The new `g_peak` is not above the stored one.
    FloorNotRaised,
    FloorLLevel,
    BreachLLevel,
    /// `b_after` differs from the closed balance at that point of the batch.
    BreachBalance,
    /// `b_after` is above the tight floor.
    BreachNotBelowFloor,
    /// `b_after` is at or below the static floor: that is a fail, not a breach.
    BreachBelowStaticFloor,
}

impl From<EngineError> for ClaimError {
    fn from(e: EngineError) -> Self {
        ClaimError::Engine(e)
    }
}

type Result<T> = core::result::Result<T, ClaimError>;

/// Price a FILL executes at from the proof's mid (DESIGN 7.1, 9.1): a long (buy) pays the ask `ceil(mid x (1e6 + h) / 1e6)`,
/// a short (sell) gets the bid `floor(mid x (1e6 - h) / 1e6)`; `h` is the rule set's `spread_e6` of the market.
pub fn fill_price_e12(mid_e12: u64, half_spread_e6: u32, long: bool) -> Result<u64> {
    Ok(if long { ask_from_mid(mid_e12, half_spread_e6)? } else { bid_from_mid(mid_e12, half_spread_e6)? })
}

/// Price a CLOSE executes at: closing a long sells at the bid, closing a short buys at the ask. Breach and fail
/// closes follow the same rule.
pub fn close_price_e12(mid_e12: u64, half_spread_e6: u32, long: bool) -> Result<u64> {
    fill_price_e12(mid_e12, half_spread_e6, !long)
}

/// A `Close` event's claims (I-5).
#[derive(Clone, Copy, Debug)]
pub struct CloseClaim {
    /// Side of the position being closed (true = long).
    pub long: bool,
    pub qty: u64,
    pub entry_e12: u64,
    pub exit_e12: u64,
    pub time: i64,
    pub timer_start: i64,
    pub pnl: i64,
    pub quick: bool,
    pub credited: i64,
}

/// What a verified close adds to the account: the balance credit and the quick-tally delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseCredit {
    pub credit: i64,
    pub tally_delta: i64,
}

/// I-5: `pnl == floor(sign x qty x (exit - entry) / DEN)`, `quick == (time - timer_start <= 45)` and `credited` per DESIGN 7.4.
pub fn check_close(c: &CloseClaim) -> Result<CloseCredit> {
    if c.qty == 0 {
        return Err(ClaimError::ZeroQuantity);
    }
    if c.entry_e12 == 0 || c.exit_e12 == 0 {
        return Err(ClaimError::ZeroPrice);
    }
    if c.timer_start > c.time {
        return Err(ClaimError::TimerAfterTime);
    }
    if pnl_micro(c.long, c.qty, c.entry_e12, c.exit_e12)? != c.pnl {
        return Err(ClaimError::PnlMismatch);
    }
    let age = c.time.checked_sub(c.timer_start).ok_or(EngineError::Overflow)?;
    if (age <= QUICK_SECS) != c.quick {
        return Err(ClaimError::QuickMismatch);
    }
    let (credit, tally_delta) = slice_credit(c.pnl, c.quick);
    if credit != c.credited {
        return Err(ClaimError::CreditMismatch);
    }
    Ok(CloseCredit { credit, tally_delta })
}

/// I-9 `TightStart`: tight mode off before; `p_open > 0.01 S` (strict); `0 <= g_peak <= p_open`; `r == b`;
/// `l_start == min(F_s + 0.04 S, r)`; `l_level == l_start + max(0, g_peak - 0.01 S)`. Returns the state with tight mode on.
pub fn check_tight_start(
    st: &AccountState,
    p_open: i64,
    r: i64,
    l_start: i64,
    g_peak: i64,
    l_level: i64,
) -> Result<AccountState> {
    if st.tight_on {
        return Err(ClaimError::TightAlreadyOn);
    }
    if p_open <= frac_exact(st.size_micro, TIGHT_START_BPS)? {
        return Err(ClaimError::TightStartPOpen);
    }
    if g_peak < 0 || g_peak > p_open {
        return Err(ClaimError::TightStartGPeak);
    }
    if r != st.b {
        return Err(ClaimError::TightStartR);
    }
    let (want_start, _) = tight_levels(st.size_micro, static_floor_micro(st)?, r, g_peak)?;
    if l_start != want_start {
        return Err(ClaimError::TightStartLStart);
    }
    if l_level != l_level_for(st.size_micro, l_start, g_peak)? {
        return Err(ClaimError::TightStartLLevel);
    }
    Ok(AccountState { tight_on: true, r, l_start, g_peak, l_level, ..*st })
}

/// I-9 `FloorRaised`: tight mode on; the new `g_peak` strictly above the stored one; `l_level` by the formula (L only rises).
pub fn check_floor_raised(st: &AccountState, g_peak: i64, l_level: i64) -> Result<AccountState> {
    if !st.tight_on {
        return Err(ClaimError::TightNotOn);
    }
    if g_peak <= st.g_peak {
        return Err(ClaimError::FloorNotRaised);
    }
    if l_level != l_level_for(st.size_micro, st.l_start, g_peak)? {
        return Err(ClaimError::FloorLLevel);
    }
    Ok(AccountState { g_peak, l_level, ..*st })
}

/// I-9 `TightEnd`: tight mode on. (The caller checks that the batch ends flat.) Returns the state with the mode ended.
pub fn check_tight_end(st: &AccountState) -> Result<AccountState> {
    if !st.tight_on {
        return Err(ClaimError::TightNotOn);
    }
    Ok(crate::account::end_tight_if_flat(st, true))
}

/// I-10 `Breach` at event time `time`: tight mode on; `l_level` equals the stored L; `b_after` equals the closed balance
/// (the closing slices are already applied); `b_after <= l_level`; and above the static floor (otherwise it is a fail).
/// Returns the state with tight mode off, `flat`, and `lock_until = time + 12 h`.
pub fn check_breach(st: &AccountState, time: i64, l_level: i64, b_after: i64) -> Result<AccountState> {
    if !st.tight_on {
        return Err(ClaimError::TightNotOn);
    }
    if l_level != st.l_level {
        return Err(ClaimError::BreachLLevel);
    }
    if b_after != st.b {
        return Err(ClaimError::BreachBalance);
    }
    if b_after > l_level {
        return Err(ClaimError::BreachNotBelowFloor);
    }
    if b_after <= static_floor_micro(st)? {
        return Err(ClaimError::BreachBelowStaticFloor);
    }
    Ok(AccountState { tight_on: false, r: 0, l_start: 0, g_peak: 0, l_level: 0, lock_until: lock_end(time)?, flat: true, ..*st })
}
