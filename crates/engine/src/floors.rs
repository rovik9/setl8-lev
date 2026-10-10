//! Per-sample floor decisions (DESIGN 2.2, 2.3, 4.4 I-7 to I-10).
use crate::account::{
    effective_floor_micro, frac_exact, l_level_for, lock_end, static_floor_micro, tight_levels, AccountState, FailReason,
    Rules,
};
use crate::consts::TIGHT_START_BPS;
use crate::error::{EngineError, Result};

/// What one accepted price sample does to the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleAction {
    /// Nothing happens.
    None,
    /// `E <= F_s`: the account fails (close at the sample, report the fail).
    Fail(FailReason),
    /// Tight breach: close ALL positions at this sample's price and lock opening/adding until `lock_until` (now + 12 h).
    Breach { close_all: bool, lock_until: i64 },
    /// Tight mode starts.
    TightStart { r: i64, l_start: i64, g_peak: i64, l_level: i64 },
    /// A new high of `G`: L rises.
    FloorRaised { g_peak: i64, l_level: i64 },
}

/// The action and the account state after it. `Fail` keeps the state as it was; `Breach` returns the state with tight mode
/// off (levels reset), `flat = true` and the lock set; the closed slices still have to be credited with `apply_closed_slice`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleDecision {
    pub action: SampleAction,
    pub state: AccountState,
    /// `E = B + P - estimated closing commission` at this sample.
    pub equity_micro: i64,
}

/// Evaluates one sample. `p_open_micro` is the open profit of all positions (floored, longs at bid, shorts at ask),
/// `closing_commission_estimate_micro` the non-negative estimated commission of closing them (DESIGN 9.4).
///
/// Order (a) to (e) is fixed: static floor, then tight breach, then tight start, then ratchet, otherwise nothing.
/// A sample that satisfies both (a) and (b) is a `Fail`. Touching counts (`<=`). Tight mode starts only when
/// `p_open > 0.01 S` (strict). A locked account still runs all floors.
pub fn evaluate_sample(
    state: &AccountState,
    _rules: &Rules,
    p_open_micro: i64,
    closing_commission_estimate_micro: i64,
    now: i64,
) -> Result<SampleDecision> {
    if closing_commission_estimate_micro < 0 {
        return Err(EngineError::InvalidInput);
    }
    let equity = state
        .b
        .checked_add(p_open_micro)
        .and_then(|e| e.checked_sub(closing_commission_estimate_micro))
        .ok_or(EngineError::Overflow)?;
    let decide = |action: SampleAction, state: AccountState| SampleDecision { action, state, equity_micro: equity };

    // (a) static floor
    let f_s = static_floor_micro(state)?;
    if equity <= f_s {
        return Ok(decide(SampleAction::Fail(FailReason::StaticFloor), *state));
    }

    // (b) tight breach
    if state.tight_on && equity <= effective_floor_micro(state)? {
        let lock_until = lock_end(now)?;
        let next = AccountState {
            tight_on: false,
            r: 0,
            l_start: 0,
            g_peak: 0,
            l_level: 0,
            lock_until,
            flat: true,
            ..*state
        };
        return Ok(decide(SampleAction::Breach { close_all: true, lock_until }, next));
    }

    // (c) tight start
    if !state.tight_on && p_open_micro > frac_exact(state.size_micro, TIGHT_START_BPS)? {
        let r = state.b;
        let g_peak = equity.checked_sub(r).ok_or(EngineError::Overflow)?.max(0);
        let (l_start, l_level) = tight_levels(state.size_micro, f_s, r, g_peak)?;
        let next = AccountState { tight_on: true, r, l_start, g_peak, l_level, ..*state };
        return Ok(decide(SampleAction::TightStart { r, l_start, g_peak, l_level }, next));
    }

    // (d) ratchet: a new high of G = E - R raises L
    if state.tight_on {
        let g = equity.checked_sub(state.r).ok_or(EngineError::Overflow)?;
        if g > state.g_peak {
            let l_level = l_level_for(state.size_micro, state.l_start, g)?;
            let next = AccountState { g_peak: g, l_level, ..*state };
            return Ok(decide(SampleAction::FloorRaised { g_peak: g, l_level }, next));
        }
    }

    // (e) nothing
    Ok(decide(SampleAction::None, *state))
}
