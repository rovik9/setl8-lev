//! Which samples trigger protective and pending orders (DESIGN 7.6 N-4, N-5, N-10, decisions 27 and 30).
//! A trigger is decided on the SAMPLE: the order fills at the sample's bid or ask, not at its own level (the trader
//! bears the gap; a limit keeps its price improvement, a stop keeps its slippage). Touching counts. A level of 0 means
//! "not set" and never triggers. Closed markets trigger nothing (frozen mark, DESIGN 3.4).
use crate::floors::SampleAction;
use crate::position::Side;

/// Which protective order fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectiveKind {
    StopLoss,
    TakeProfit,
}

/// A fired protective order and the price it closes at (the sample's bid for a long, ask for a short).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectiveTrigger {
    pub kind: ProtectiveKind,
    pub price_e12: u64,
}

/// Stop-loss / take-profit of a position (`position_side` `Buy` = long). Long: SL when `bid <= sl`, TP when `bid >= tp`.
/// Short: SL when `ask >= sl`, TP when `ask <= tp`. When one sample satisfies both, the stop-loss wins (N-5).
pub fn check_protective(
    position_side: Side,
    sl_e12: u64,
    tp_e12: u64,
    bid_e12: u64,
    ask_e12: u64,
    market_open: bool,
) -> Option<ProtectiveTrigger> {
    if !market_open {
        return None;
    }
    let (price, sl_hit, tp_hit) = match position_side {
        Side::Buy => (bid_e12, sl_e12 != 0 && bid_e12 <= sl_e12, tp_e12 != 0 && bid_e12 >= tp_e12),
        Side::Sell => (ask_e12, sl_e12 != 0 && ask_e12 >= sl_e12, tp_e12 != 0 && ask_e12 <= tp_e12),
    };
    if sl_hit {
        Some(ProtectiveTrigger { kind: ProtectiveKind::StopLoss, price_e12: price })
    } else if tp_hit {
        Some(ProtectiveTrigger { kind: ProtectiveKind::TakeProfit, price_e12: price })
    } else {
        None
    }
}

/// Kind of a pending entry order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingKind {
    Limit,
    Stop,
}

/// A pending order's fill price if this sample triggers it. `order_side` is the side of the order (`Buy` buys at the ask).
/// Buy limit: `ask <= limit`; sell limit: `bid >= limit`; buy stop: `ask >= stop`; sell stop: `bid <= stop`.
/// Nothing triggers while the market is closed or the account is locked (the lock blocks opening and adding).
pub fn check_pending(
    order_side: Side,
    kind: PendingKind,
    level_e12: u64,
    bid_e12: u64,
    ask_e12: u64,
    market_open: bool,
    locked: bool,
) -> Option<u64> {
    if !market_open || locked || level_e12 == 0 {
        return None;
    }
    let (price, hit) = match (order_side, kind) {
        (Side::Buy, PendingKind::Limit) => (ask_e12, ask_e12 <= level_e12),
        (Side::Sell, PendingKind::Limit) => (bid_e12, bid_e12 >= level_e12),
        (Side::Buy, PendingKind::Stop) => (ask_e12, ask_e12 >= level_e12),
        (Side::Sell, PendingKind::Stop) => (bid_e12, bid_e12 <= level_e12),
    };
    if hit {
        Some(price)
    } else {
        None
    }
}

/// Whether stop-loss / take-profit may still be processed after a floor decision: a fail or a breach closes everything
/// at the sample first (check order: static fail, breach, then SL/TP).
pub fn sl_tp_allowed_after(action: &SampleAction) -> bool {
    matches!(action, SampleAction::None | SampleAction::TightStart { .. } | SampleAction::FloorRaised { .. })
}
