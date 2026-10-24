//! What the evaluator is configured with: the rule set the account was bought under, its markets, and the account itself.
//! All of it is plain data a service layer reads from the chain (`RuleSet`, `Market`, `Opened`).

/// Number of market classes (DESIGN 3.1).
pub const CLASSES: usize = 8;
/// DESIGN A-CAP-1: pending orders per account.
pub const MAX_PENDING: usize = 10;
/// DESIGN A-CAP-1: open positions per account.
pub const MAX_POSITIONS: usize = 8;
/// Market status values (Market.status).
pub const STATUS_DISABLED: u8 = 0;
pub const STATUS_ENABLED: u8 = 1;
pub const STATUS_REDUCE_ONLY: u8 = 2;

/// One market of the account's world (a `Market` account plus the rule set's half-spread for it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketCfg {
    pub id: u16,
    /// Class 0..=7: index into the per-class arrays of [`RulesCfg`].
    pub class: u8,
    /// PriceProof source of this market's feed (2 GMX tickers, 3 GMX signed, 4 Hyperliquid, 255 mock).
    pub source: u8,
    /// `sha256("SL8FEED" ++ source ++ symbol)`, the market's current feed id.
    pub feed_id: [u8; 32],
    /// A proof may be this many seconds older than the event it justifies; also the staleness limit of the market.
    pub max_age_secs: u16,
    /// 0 disabled, 1 enabled, 2 reduce-only.
    pub status: u8,
    /// Quantity step and minimum, 1e-6 base units.
    pub qty_step: u64,
    pub qty_min: u64,
    /// Per-position notional cap in micro-USD.
    pub max_notional_micro: u64,
    /// Up to six weekly windows `(start_minute, end_minute)`, UTC minutes of the week.
    pub sessions: Vec<(u16, u16)>,
    /// Half-spread in e6 of the mid (the rule set's `spread_e6[id]`).
    pub half_spread_e6: u32,
}

/// The rule set snapshot of the account (DESIGN 5.4) plus its markets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesCfg {
    pub ruleset_version: u16,
    /// Leverage per class (1:x).
    pub leverage_x: [u16; CLASSES],
    /// Holding fee per night per class, e6 of the notional.
    pub holding_fee_e6: [u32; CLASSES],
    /// Commission per class, e6 of the notional, charged at every fill and every closing slice.
    pub commission_e6: [u32; CLASSES],
    pub markets: Vec<MarketCfg>,
}

impl RulesCfg {
    pub fn market(&self, id: u16) -> Option<&MarketCfg> {
        self.markets.iter().find(|m| m.id == id)
    }

    pub fn leverage(&self, class: u8) -> Option<u16> {
        self.leverage_x.get(usize::from(class)).copied()
    }

    pub fn holding(&self, class: u8) -> u32 {
        self.holding_fee_e6.get(usize::from(class)).copied().unwrap_or(0)
    }

    pub fn commission(&self, class: u8) -> u32 {
        self.commission_e6.get(usize::from(class)).copied().unwrap_or(0)
    }
}

/// The account being evaluated (an `Opened` event).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountParams {
    /// The `TradingAccount` address: the hash chain starts at `sha256("SL8LEV" ++ account)` and intents are signed for it.
    pub account: [u8; 32],
    /// The trader wallet: intents must be signed by it.
    pub trader: [u8; 32],
    pub challenge_id: u64,
    /// S in micro-USD (a whole number of dollars).
    pub size_micro: i64,
    /// 1 to 5.
    pub phase: u8,
    pub tier_index: u8,
    /// Unix seconds of `open_account`.
    pub opened_time: i64,
}

/// Why `Evaluator::new` refused its configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// `size_micro` is not a positive whole number of dollars (the thresholds must be exact).
    BadSize,
    /// Phase outside 1..=5.
    BadPhase,
    /// A market with class above 7, more than six sessions, a zero step or a zero `max_age_secs`.
    BadMarket(u16),
    /// Two markets with the same id.
    DuplicateMarket(u16),
    /// A class with leverage 0.
    BadLeverage,
}

/// Why `on_sample` refused a sample (the sample is NOT recorded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleError {
    UnknownMarket,
    /// The value is not a positive price.
    BadValue,
    /// The source second is earlier than the evaluator's clock (the newest accepted sample second or the last `tick`): the stream is in non-decreasing second order.
    OutOfOrder,
    /// The market's own timestamps went backwards.
    MarketTimeBackwards,
    /// A sample dated after the account ended.
    AccountEnded,
    /// The sample's source (`PriceProof.source`) is not the market's.
    WrongSource,
    /// The source second is more than `MAX_SAMPLE_FUTURE_SECS` ahead of the evaluator's own receive time (audit F-17): refused, so one
    /// bad timestamp cannot move the clock or the market's time.
    FutureSample,
    /// The sample is already older than the market's `max_age_secs` when it is received (audit F-17).
    StaleSample,
}

/// Why an order was refused. A refused order leaves no journal event and (for `Limit`/`Stop`/`Cancel`/`SetSlTp`) no line in the intents file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderReject {
    /// The injected signature check failed.
    BadSignature,
    /// Nonce 0 (reserved) or already used.
    BadNonce,
    /// The 48 order bytes do not decode.
    BadOrder,
    UnknownMarket,
    /// `client_time` is before the second the evaluator is at (a market or close order: more than `MAX_ORDER_SKEW_SECS` before it).
    StaleOrder,
    /// `client_time` is more than `MAX_ORDER_SKEW_SECS` ahead of the evaluator's second.
    FutureOrder,
    /// Failed, abandoned, graduated or ended.
    AccountEnded,
    /// A pass is requested or confirmed: the account is frozen for trading.
    AccountFrozen,
    /// The account is held for review (DESIGN 2.9): no exposure-increasing order.
    AccountHeld,
    /// Opening or adding during the 12 h lock after a breach.
    AccountLocked,
    /// Market disabled, or reduce-only and the order would increase exposure.
    MarketNotTradable,
    /// The market's half-spread in the account's rule set is 0 (a market the rule set does not price, decision 64): no
    /// order that opens or adds exposure. Rule sets are immutable, so this never changes for the same account.
    MarketUnpriced,
    /// Outside its session windows or no price change within `max_age_secs` (decision 49).
    MarketClosed,
    /// No price for the market, or the newest one is older than `max_age_secs`.
    NoFreshPrice,
    /// Zero, not a multiple of the step, below the minimum, or larger than the position for a close.
    InvalidQuantity,
    /// SL/TP/limit/stop level on the wrong side of the market, or a limit/stop with level 0.
    InvalidLevel,
    /// The position after the order would exceed the market's notional cap.
    NotionalCap,
    /// Free equity does not cover the margin (DESIGN 7.3).
    InsufficientMargin,
    /// `MAX_PENDING` pending orders already.
    TooManyPending,
    /// `MAX_POSITIONS` positions already.
    TooManyPositions,
    /// A close or SL/TP for a market without a position.
    NoPosition,
    /// A cancel for an order that is not pending.
    NoSuchOrder,
    /// A flip whose remainder is not a valid fill quantity.
    InvalidRemainder,
    /// An engine function refused the numbers.
    Engine,
}

/// Why a pass request is not confirmed. The numbers are the `reject_pass` reason codes of the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassReject {
    NotFlat = 0,
    JournalBehind = 1,
    TargetNotReached = 2,
    Other = 3,
}

/// Why a lifecycle call was refused (`phase_started`, `pass_resolved`, `set_held`, `end_externally`). A refused call changes NOTHING:
/// no field, no counter, no queue. The host maps it to a refused lifecycle input; it never means the chain event did not happen, only that
/// this evaluator is not in the state in which that event is possible (the host's ordering or its own state is wrong).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    /// The evaluator faulted or was ended outside the journal: it accepts no more lifecycle input (except `end_externally`).
    Ended,
    /// No pass is pending: `phase_started` and `pass_resolved` follow a `request_pass`, which freezes the account.
    NotFrozen,
    /// The journal ended with a `Fail` (`phase_started`, `pass_resolved`, `set_held(true)`).
    NotActive,
    /// A position is open.
    NotFlat,
    /// A limit or stop order is still pending.
    PendingOrders,
    /// Journal events are still queued (`drain_batches` first, and post them).
    QueuedEvents,
    /// An accepted order has not executed yet (it sits in its second's bucket): it would execute after the lifecycle change.
    OrdersQueued,
    /// `phase_started` was called with the wrong phase: `have` is the phase the host passed, `want` the phase the evaluator expects
    /// after its current one (2 after 1 ... 5 after 4; 0 after 5, the account graduates: call `end_externally`).
    WrongPhase { have: u8, want: u8 },
    /// The `ChainView` of the fresh phase disagrees with the evaluator: `what` is `seq`, `journal_hash`, `b` (a fresh phase has B = S),
    /// `flat` (a fresh phase is flat) or `phase`.
    ChainMismatch { what: &'static str },
    /// `set_held(true)` on a held account (the verifier's `StateMismatch` for a second `account_held` line).
    AlreadyHeld,
    /// `set_held(false)` on an account that is not held.
    NotHeld,
    /// `end_externally` on an account that was ended outside the journal already.
    AlreadyEnded,
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Ended => write!(f, "the evaluator faulted or was ended outside the journal"),
            LifecycleError::NotFrozen => write!(f, "no pass is pending (the account is not frozen)"),
            LifecycleError::NotActive => write!(f, "the account is not active (its journal ended with a Fail)"),
            LifecycleError::NotFlat => write!(f, "a position is open"),
            LifecycleError::PendingOrders => write!(f, "a pending order is still live"),
            LifecycleError::QueuedEvents => write!(f, "journal events are still queued"),
            LifecycleError::OrdersQueued => write!(f, "an accepted order has not executed yet"),
            LifecycleError::WrongPhase { have, want } => write!(f, "phase {have} passed, phase {want} expected (0 = graduation)"),
            LifecycleError::ChainMismatch { what } => write!(f, "the chain view disagrees with the evaluator: {what}"),
            LifecycleError::AlreadyHeld => write!(f, "the account is already held"),
            LifecycleError::NotHeld => write!(f, "the account is not held"),
            LifecycleError::AlreadyEnded => write!(f, "the account was already ended outside the journal"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Internal failures (a bug or an impossible configuration), never a trader error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalError {
    Overflow,
    Engine(engine::EngineError),
    Invariant(&'static str),
}

impl From<engine::EngineError> for EvalError {
    fn from(e: engine::EngineError) -> Self {
        EvalError::Engine(e)
    }
}

/// How far the trader's signed `client_time` may be from the evaluator's own second when the order arrives (audit F-18, decision 68):
/// a market or close order is executed in the evaluator's second, its signed time only has to be within this many seconds of it (either
/// side); a pending order, a cancel and a set_sl_tp keep their signed time (the verifier uses it) and may be dated this far AHEAD.
/// The verifier's `ORDER_WINDOW_SECS` is the same number. An order never moves the evaluator's clock.
pub const MAX_ORDER_SKEW_SECS: i64 = 5;

/// How far ahead of the evaluator's own receive time a price sample's source second may be (audit F-17).
pub const MAX_SAMPLE_FUTURE_SECS: i64 = 5;

/// Whether `qty x price` (1e-6 units x 1e-12 USD) is above `cap_micro` micro-USD, compared on the exact product (audit F-35, DESIGN 7.4:
/// rounding is against the trader, so a notional a fraction of a micro-USD over the cap is over it). The verifier has the same function
/// (`setl8_verify::walk::notional_over_cap`); they must stay equal.
pub fn notional_over_cap(qty_abs: u64, price_e12: u64, cap_micro: u64) -> bool {
    u128::from(qty_abs) * u128::from(price_e12) > u128::from(cap_micro) * 1_000_000_000_000u128
}

#[cfg(test)]
mod notional_tests {
    use super::notional_over_cap;

    #[test]
    fn the_notional_cap_is_compared_on_the_exact_product() {
        // qty 1,000 units x price 1.0 = exactly 1,000 micro-USD
        assert!(!notional_over_cap(1_000, 1_000_000_000_000, 1_000), "equal to the cap is allowed");
        assert!(notional_over_cap(1_000, 1_000_000_000_001, 1_000), "one e12 unit of price more is above it (1000.000001 micro-USD)");
        assert!(!notional_over_cap(1_000, 999_999_999_999, 1_000));
        assert!(notional_over_cap(1_000, 1_000_000_000_000, 999), "a cap one below");
        // no overflow at the extremes
        assert!(notional_over_cap(u64::MAX, u64::MAX, u64::MAX));
        assert!(!notional_over_cap(0, u64::MAX, 0));
        assert!(notional_over_cap(1, 1, 0), "any positive notional, however small, is above a cap of 0");
    }
}
