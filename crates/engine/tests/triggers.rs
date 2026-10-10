//! Order triggers (DESIGN 7.6 N-4, N-5, N-10; decisions 27, 30). Every case has a long and a short mirror.
use engine::*;

const BID: u64 = 1_000_000_000_000;
const ASK: u64 = 1_000_100_000_000;

fn prot(side: Side, sl: u64, tp: u64, bid: u64, ask: u64) -> Option<ProtectiveTrigger> {
    check_protective(side, sl, tp, bid, ask, true)
}

#[test]
fn long_stop_loss_triggers_at_or_below_the_level_touching_counts() {
    assert_eq!(prot(Side::Buy, BID, 0, BID, ASK), Some(ProtectiveTrigger { kind: ProtectiveKind::StopLoss, price_e12: BID })); // touch
    assert!(prot(Side::Buy, BID + 1, 0, BID, ASK).is_some());
    assert_eq!(prot(Side::Buy, BID - 1, 0, BID, ASK), None);
}

#[test]
fn short_stop_loss_triggers_at_or_above_the_level_touching_counts() {
    assert_eq!(prot(Side::Sell, ASK, 0, BID, ASK), Some(ProtectiveTrigger { kind: ProtectiveKind::StopLoss, price_e12: ASK }));
    assert!(prot(Side::Sell, ASK - 1, 0, BID, ASK).is_some());
    assert_eq!(prot(Side::Sell, ASK + 1, 0, BID, ASK), None);
}

#[test]
fn long_take_profit_triggers_at_or_above_the_level() {
    assert_eq!(prot(Side::Buy, 0, BID, BID, ASK), Some(ProtectiveTrigger { kind: ProtectiveKind::TakeProfit, price_e12: BID }));
    assert!(prot(Side::Buy, 0, BID - 1, BID, ASK).is_some());
    assert_eq!(prot(Side::Buy, 0, BID + 1, BID, ASK), None);
}

#[test]
fn short_take_profit_triggers_at_or_below_the_level() {
    assert_eq!(prot(Side::Sell, 0, ASK, BID, ASK), Some(ProtectiveTrigger { kind: ProtectiveKind::TakeProfit, price_e12: ASK }));
    assert!(prot(Side::Sell, 0, ASK + 1, BID, ASK).is_some());
    assert_eq!(prot(Side::Sell, 0, ASK - 1, BID, ASK), None);
}

#[test]
fn n5_stop_loss_beats_take_profit_on_one_sample() {
    // a gap sample can satisfy both for a (wrongly placed or crossed) pair: SL takes precedence
    let t = prot(Side::Buy, BID + 10, BID - 10, BID, ASK).unwrap();
    assert_eq!(t.kind, ProtectiveKind::StopLoss);
    let t = prot(Side::Sell, ASK - 10, ASK + 10, BID, ASK).unwrap();
    assert_eq!(t.kind, ProtectiveKind::StopLoss);
}

#[test]
fn a_gap_fills_the_stop_at_the_sample_price_not_the_stop_level() {
    // long SL 0.99, the sample gaps to bid 0.95: closes at 0.95
    let t = prot(Side::Buy, 990_000_000_000, 0, 950_000_000_000, 951_000_000_000).unwrap();
    assert_eq!(t.price_e12, 950_000_000_000);
    // short SL 1.01, the sample gaps to ask 1.05: closes at 1.05
    let t = prot(Side::Sell, 1_010_000_000_000, 0, 1_049_000_000_000, 1_050_000_000_000).unwrap();
    assert_eq!(t.price_e12, 1_050_000_000_000);
    // take profit also closes at the sample (price improvement kept)
    let t = prot(Side::Buy, 0, 1_010_000_000_000, 1_050_000_000_000, 1_051_000_000_000).unwrap();
    assert_eq!((t.kind, t.price_e12), (ProtectiveKind::TakeProfit, 1_050_000_000_000));
    let t = prot(Side::Sell, 0, 990_000_000_000, 949_000_000_000, 950_000_000_000).unwrap();
    assert_eq!((t.kind, t.price_e12), (ProtectiveKind::TakeProfit, 950_000_000_000));
}

#[test]
fn unset_levels_never_trigger() {
    assert_eq!(prot(Side::Buy, 0, 0, 1, u64::MAX), None);
    assert_eq!(prot(Side::Sell, 0, 0, 1, u64::MAX), None);
    assert_eq!(prot(Side::Buy, 0, 0, 0, 0), None);
    assert_eq!(prot(Side::Sell, 0, 0, 0, 0), None);
}

#[test]
fn nothing_triggers_while_the_market_is_closed() {
    assert_eq!(check_protective(Side::Buy, BID + 1, 0, BID, ASK, false), None);
    assert_eq!(check_protective(Side::Sell, 0, ASK + 1, BID, ASK, false), None);
}

// ---------------------------------------------------------------- pending orders

fn pend(side: Side, kind: PendingKind, level: u64, bid: u64, ask: u64) -> Option<u64> {
    check_pending(side, kind, level, bid, ask, true, false)
}

#[test]
fn buy_limit_fills_when_ask_is_at_or_below_the_limit_and_keeps_the_improvement() {
    assert_eq!(pend(Side::Buy, PendingKind::Limit, ASK, BID, ASK), Some(ASK)); // touch
    assert_eq!(pend(Side::Buy, PendingKind::Limit, ASK - 1, BID, ASK), None);
    // price improvement: limit 1.01, the sample's ask is 1.0 -> fills at 1.0
    assert_eq!(pend(Side::Buy, PendingKind::Limit, 1_010_000_000_000, BID, BID + 100_000_000), Some(BID + 100_000_000));
}

#[test]
fn sell_limit_fills_when_bid_is_at_or_above_the_limit_and_keeps_the_improvement() {
    assert_eq!(pend(Side::Sell, PendingKind::Limit, BID, BID, ASK), Some(BID));
    assert_eq!(pend(Side::Sell, PendingKind::Limit, BID + 1, BID, ASK), None);
    assert_eq!(pend(Side::Sell, PendingKind::Limit, 990_000_000_000, BID, ASK), Some(BID));
}

#[test]
fn buy_stop_fills_when_ask_touches_or_passes_the_stop_and_keeps_the_slippage() {
    assert_eq!(pend(Side::Buy, PendingKind::Stop, ASK, BID, ASK), Some(ASK)); // touch
    assert_eq!(pend(Side::Buy, PendingKind::Stop, ASK + 1, BID, ASK), None);
    // slippage kept: stop 1.0, the sample's ask is 1.05 -> fills at 1.05
    assert_eq!(pend(Side::Buy, PendingKind::Stop, 1_000_000_000_000, 1_049_000_000_000, 1_050_000_000_000), Some(1_050_000_000_000));
}

#[test]
fn sell_stop_fills_when_bid_touches_or_passes_the_stop_and_keeps_the_slippage() {
    assert_eq!(pend(Side::Sell, PendingKind::Stop, BID, BID, ASK), Some(BID));
    assert_eq!(pend(Side::Sell, PendingKind::Stop, BID - 1, BID, ASK), None);
    assert_eq!(pend(Side::Sell, PendingKind::Stop, 1_000_000_000_000, 950_000_000_000, 951_000_000_000), Some(950_000_000_000));
}

#[test]
fn pending_orders_do_not_trigger_when_the_market_is_closed_or_the_account_is_locked() {
    for (side, kind, level) in [
        (Side::Buy, PendingKind::Limit, ASK),
        (Side::Sell, PendingKind::Limit, BID),
        (Side::Buy, PendingKind::Stop, ASK),
        (Side::Sell, PendingKind::Stop, BID),
    ] {
        assert!(check_pending(side, kind, level, BID, ASK, true, false).is_some(), "{side:?} {kind:?} baseline");
        assert_eq!(check_pending(side, kind, level, BID, ASK, false, false), None, "{side:?} {kind:?} closed market");
        assert_eq!(check_pending(side, kind, level, BID, ASK, true, true), None, "{side:?} {kind:?} locked");
        assert_eq!(check_pending(side, kind, level, BID, ASK, false, true), None);
    }
}

#[test]
fn an_unset_pending_level_never_triggers() {
    for (side, kind) in [(Side::Buy, PendingKind::Limit), (Side::Sell, PendingKind::Limit), (Side::Buy, PendingKind::Stop), (Side::Sell, PendingKind::Stop)] {
        assert_eq!(pend(side, kind, 0, BID, ASK), None);
    }
}

#[test]
fn a_triggered_pending_order_nets_through_apply_fill_at_the_sample_price() {
    // N-9 together with triggers: a buy stop fills at the ask of the sample and merges into a long
    let pos = Position { qty: 1_000_000, avg_entry_e12: 1_000_000_000_000, timer_start: 0, opened_at: 0, fee_day: 0 };
    let price = pend(Side::Buy, PendingKind::Stop, 1_000_000_000_000, 1_049_000_000_000, 1_050_000_000_000).unwrap();
    let o = apply_fill(Some(pos), Side::Buy, 1_000_000, price, 10, 0).unwrap();
    assert_eq!(o.position.unwrap().avg_entry_e12, 1_025_000_000_000);
}
