//! setl8-lev-trading rules engine (DESIGN.md sections 2, 7, 9).
//!
//! Integer arithmetic only. Money is micro-USD (`i64`), prices are 1e-12 USD (`u64`),
//! quantities are 1e-6 base units (`i64`). Intermediate products use `u128`/`i128` with checked operations.
//! M3: prices, netting (`apply_fill`), costs and order validation. M4: account state, floors, tight mode, quick
//! tally, phase rules and order triggers.
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )
)]

pub mod account;
pub mod claims;
pub mod consts;
pub mod cost;
pub mod error;
pub mod floors;
pub mod position;
pub mod price;
pub mod triggers;
pub mod units;

pub use cost::{
    day_index, fee_ceil_micro, holding_fee_micro, margin_micro, nights_due, swap_fee_micro, validate_quantity,
    validate_stop_level, StopKind,
};
pub use account::{
    apply_closed_slice, can_increase_exposure, effective_floor_micro, end_tight_if_flat, frac_exact, next_phase,
    pass_check, payout_micro, reset_phase_state, static_floor_micro, AccountState, ClosedSliceDecision, FailReason,
    NextPhase, PassError, Rules,
};
pub use claims::{
    check_breach, check_close, check_floor_raised, check_tight_end, check_tight_start, close_price_e12, fill_price_e12,
    ClaimError, CloseClaim, CloseCredit,
};
pub use error::{EngineError, Result};
pub use floors::{evaluate_sample, SampleAction, SampleDecision};
pub use position::{
    apply_fill, open_profit_micro, pnl_micro, slice_credit, unrealized_micro, CloseSlice, FillOutcome, OpenSlice, Position, Side,
};
pub use price::{ask_from_mid, bid_from_mid, price_from_oracle};
pub use triggers::{
    check_pending, check_protective, sl_tp_allowed_after, PendingKind, ProtectiveKind, ProtectiveTrigger,
};

/// Version written into every journal `Meta` event so a verifier uses the right rules.
pub const ENGINE_VERSION: u16 = 1;
