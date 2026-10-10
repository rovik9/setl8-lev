//! Account state and phase rules (DESIGN 2.1 to 2.5, 2.7, 2.8). Everything is in micro-USD; every threshold is
//! a number of basis points of the account size S (`consts`), and S is a whole number of dollars so
//! `S x bps / 10_000` is exact (an inexact size is `InvalidInput`).
use crate::consts::{
    BPS, LATCHED_FLOOR_BPS, LATCH_TRIGGER_BPS, LOCK_SECS, PAYOUT_BPS, QUICK_LIMIT_BPS, STATIC_FLOOR_BPS, TARGET_BPS,
    TIGHT_OFFSET_BPS, RATCHET_SLACK_BPS,
};
use crate::error::{EngineError, Result};

/// Why a phase failed (the engine's own copy; the journal encodes it as a byte).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    /// Equity touched the static floor.
    StaticFloor,
    /// The quick tally passed the limit.
    PhaseQuick,
}

/// The per-account numbers the rules act on (plain data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountState {
    /// S, the account size in micro-USD (a multiple of 1,000,000).
    pub size_micro: i64,
    /// B, the closed balance.
    pub b: i64,
    /// The static floor moved to 0.96 S (never clears within a phase).
    pub latch: bool,
    pub tight_on: bool,
    /// B at the moment tight mode began (0 when off).
    pub r: i64,
    /// `min(F_s + 0.04 S, R)` at the start (0 when off).
    pub l_start: i64,
    /// Highest `G = E - R` seen since tight mode began (0 when off).
    pub g_peak: i64,
    /// The tight floor L (0 when off).
    pub l_level: i64,
    /// The quick tally Q: credited half-profits of profitable quick slices.
    pub q_tally: i64,
    /// Unix seconds until which opening or adding is blocked (0 = no lock).
    pub lock_until: i64,
    /// All positions closed, as last asserted by the evaluator.
    pub flat: bool,
}

impl AccountState {
    /// A fresh phase: B = S, no latch, tight mode off, Q = 0, no lock, flat.
    pub fn new_phase(size_micro: i64) -> Self {
        AccountState {
            size_micro,
            b: size_micro,
            latch: false,
            tight_on: false,
            r: 0,
            l_start: 0,
            g_peak: 0,
            l_level: 0,
            q_tally: 0,
            lock_until: 0,
            flat: true,
        }
    }
}

/// The two assumptions the owner has not yet confirmed, as switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// A-RULE-1. `true` (default): L is a frozen ratchet; a latch in the middle of tight mode does not touch it and the
    /// effective floor is `max(F_s, L)`. `false`: when the latch flips during tight mode, `l_start` is recomputed as
    /// `min(F_s_new + 0.04 S, R)` and `l_level = l_start + max(0, g_peak - 0.01 S)` (R and `g_peak` are kept).
    pub frozen_ratchet: bool,
    /// A-RULE-2. `true` (default): the phase fails when `Q > 0.028 S`. `false`: when `Q >= 0.028 S`.
    pub quick_strict: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { frozen_ratchet: true, quick_strict: true }
    }
}

/// `size x bps / 10_000`, exact. A size that makes the division inexact (not whole dollars) or a non-positive size is `InvalidInput`.
pub fn frac_exact(size_micro: i64, bps: i64) -> Result<i64> {
    if size_micro <= 0 {
        return Err(EngineError::InvalidInput);
    }
    let n = i128::from(size_micro).checked_mul(i128::from(bps)).ok_or(EngineError::Overflow)?;
    let d = i128::from(BPS);
    if n.checked_rem(d).ok_or(EngineError::Overflow)? != 0 {
        return Err(EngineError::InvalidInput);
    }
    i64::try_from(n.checked_div(d).ok_or(EngineError::Overflow)?).map_err(|_| EngineError::Overflow)
}

/// F_s: 0.92 S, or 0.96 S once latched.
pub fn static_floor_micro(state: &AccountState) -> Result<i64> {
    frac_exact(state.size_micro, if state.latch { LATCHED_FLOOR_BPS } else { STATIC_FLOOR_BPS })
}

/// The floor that breaches tight mode: `max(F_s, L)` while tight mode is on (A-RULE-1), else F_s.
pub fn effective_floor_micro(state: &AccountState) -> Result<i64> {
    let f_s = static_floor_micro(state)?;
    Ok(if state.tight_on { f_s.max(state.l_level) } else { f_s })
}

/// `(l_start, l_level)` for a tight mode: `l_start = min(F_s + 0.04 S, R)`, `l_level = l_start + max(0, g_peak - 0.01 S)`.
pub(crate) fn tight_levels(size_micro: i64, static_floor: i64, r: i64, g_peak: i64) -> Result<(i64, i64)> {
    let cap = static_floor.checked_add(frac_exact(size_micro, TIGHT_OFFSET_BPS)?).ok_or(EngineError::Overflow)?;
    let l_start = cap.min(r);
    let l_level = l_level_for(size_micro, l_start, g_peak)?;
    Ok((l_start, l_level))
}

/// `l_start + max(0, g - 0.01 S)`.
pub(crate) fn l_level_for(size_micro: i64, l_start: i64, g: i64) -> Result<i64> {
    let slack = frac_exact(size_micro, RATCHET_SLACK_BPS)?;
    let extra = g.checked_sub(slack).ok_or(EngineError::Overflow)?.max(0);
    l_start.checked_add(extra).ok_or(EngineError::Overflow)
}

/// Opening or adding is blocked while locked (`now < lock_until`). Floors still run; closing is always allowed.
pub fn can_increase_exposure(state: &AccountState, now: i64) -> bool {
    now >= state.lock_until
}

/// The result of crediting a closed slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosedSliceDecision {
    pub state: AccountState,
    /// `Some(PhaseQuick)` when the quick limit is exceeded at this slice (A-RULE-2).
    pub fail: Option<FailReason>,
}

/// Credits a closed slice: `b += credit`, `q_tally += quick_tally_delta`, latch when `b >= 1.04 S`, and the quick-limit
/// check at this moment (`Q > 0.028 S`, or `>=` with `quick_strict = false`). The delta comes from `FillOutcome` and is
/// never negative (a quick loss never reduces Q): a negative delta is `InvalidInput`. With `frozen_ratchet = false`, a latch
/// that flips during tight mode recomputes `l_start` and `l_level` (see `Rules`).
pub fn apply_closed_slice(
    state: &AccountState,
    rules: &Rules,
    credit_micro: i64,
    quick_tally_delta_micro: i64,
) -> Result<ClosedSliceDecision> {
    if quick_tally_delta_micro < 0 {
        return Err(EngineError::InvalidInput);
    }
    let mut next = *state;
    next.b = state.b.checked_add(credit_micro).ok_or(EngineError::Overflow)?;
    next.q_tally = state.q_tally.checked_add(quick_tally_delta_micro).ok_or(EngineError::Overflow)?;

    if !next.latch && next.b >= frac_exact(state.size_micro, LATCH_TRIGGER_BPS)? {
        next.latch = true;
        if next.tight_on && !rules.frozen_ratchet {
            let f_new = frac_exact(state.size_micro, LATCHED_FLOOR_BPS)?;
            let (l_start, l_level) = tight_levels(state.size_micro, f_new, next.r, next.g_peak)?;
            next.l_start = l_start;
            next.l_level = l_level;
        }
    }

    let limit = frac_exact(state.size_micro, QUICK_LIMIT_BPS)?;
    let over = if rules.quick_strict { next.q_tally > limit } else { next.q_tally >= limit };
    Ok(ClosedSliceDecision { state: next, fail: if over { Some(FailReason::PhaseQuick) } else { None } })
}

/// Records the evaluator's `flat` and ends tight mode when flat (mode levels reset to 0). Tight mode does not end while a position is open.
pub fn end_tight_if_flat(state: &AccountState, flat: bool) -> AccountState {
    let mut next = *state;
    next.flat = flat;
    if flat && state.tight_on {
        next.tight_on = false;
        next.r = 0;
        next.l_start = 0;
        next.g_peak = 0;
        next.l_level = 0;
    }
    next
}

/// Why a pass is refused (DESIGN 2.5, program errors `NotFlat`, `TargetNotReached`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassError {
    NotFlat,
    TargetNotReached,
    PhaseFailed,
}

/// `B >= 1.08 S` AND flat AND the phase is not failed. Checked in the order failed, flat, target.
pub fn pass_check(state: &AccountState, flat: bool, phase_failed: bool) -> core::result::Result<(), PassError> {
    if phase_failed {
        return Err(PassError::PhaseFailed);
    }
    if !flat {
        return Err(PassError::NotFlat);
    }
    // an unusable size cannot reach any target
    match frac_exact(state.size_micro, TARGET_BPS) {
        Ok(target) if state.b >= target => Ok(()),
        _ => Err(PassError::TargetNotReached),
    }
}

/// The payout of a pass in phases 2 to 5: exactly 0.08 S.
pub fn payout_micro(size_micro: i64) -> Result<i64> {
    frac_exact(size_micro, PAYOUT_BPS)
}

/// What follows a passed phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextPhase {
    /// The next phase starts fresh; `payout` is false only for 1 -> 2.
    Next { phase: u8, payout: bool },
    /// After phase 5 (with its payout).
    Graduated,
}

impl NextPhase {
    /// Whether this pass pays 0.08 S.
    pub fn pays(&self) -> bool {
        match self {
            NextPhase::Next { payout, .. } => *payout,
            NextPhase::Graduated => true,
        }
    }
}

/// 1 -> 2 (no payout), 2..=4 -> next (payout), 5 -> Graduated (payout). Any other phase is `InvalidInput`.
pub fn next_phase(phase: u8) -> Result<NextPhase> {
    match phase {
        1 => Ok(NextPhase::Next { phase: 2, payout: false }),
        2 => Ok(NextPhase::Next { phase: 3, payout: true }),
        3 => Ok(NextPhase::Next { phase: 4, payout: true }),
        4 => Ok(NextPhase::Next { phase: 5, payout: true }),
        5 => Ok(NextPhase::Graduated),
        _ => Err(EngineError::InvalidInput),
    }
}

/// A fresh phase state (B = S, F_s = 0.92 S, latch cleared, tight mode off, Q = 0, no lock).
pub fn reset_phase_state(size_micro: i64) -> AccountState {
    AccountState::new_phase(size_micro)
}

/// Lock end for a breach at `now`: `now + 12 h`.
pub(crate) fn lock_end(now: i64) -> Result<i64> {
    now.checked_add(LOCK_SECS).ok_or(EngineError::Overflow)
}
