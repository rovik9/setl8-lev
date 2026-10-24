//! The evaluator's side of the pass flow (DESIGN 4.10): what it answers to `request_advance`, and the phase start.
mod common;
use common::*;
use evaluator::*;

/// B after `winner()`: the long fills at the ask of mid 1.0 (`ASK_AT_ONE`), the close sells at the bid of mid 1.0085,
/// `floor(1_008_500_000_000 x (1e6 - 1) / 1e6) = 1_008_500_000_000 - 1_008_500 = 1_008_498_991_500`; qty = 1 MEUR makes the
/// pnl in micro-USD the price difference: 1_008_498_991_500 - 1_000_001_000_000 = 8_497_991_500 (no commission, no rollover).
const WINNER_B: i64 = S100K + 8_497_991_500;

/// A world (the smallest priced spread) whose account closed a +8.5% mid trade (non-quick) and is flat with B = `WINNER_B`.
fn winner() -> (World, i64) {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 100, p4(10_085));
    w.order(close_order(0, 0, t + 100)).unwrap();
    w.sample(0, t + 110, p4(10_085));
    w.ev.tick(t + 111);
    (w, t + 111)
}

fn view(w: &World, batches: &[Batch]) -> ChainView {
    let _ = batches;
    ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: w.ev.state().b, flat: w.ev.positions().is_empty(), phase: w.ev.phase() }
}

#[test]
fn confirm_when_everything_agrees() {
    let (mut w, t) = winner();
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    assert_eq!(w.ev.state().b, WINNER_B);
    let v = view(&w, &bs);
    assert_eq!(w.ev.request_pass(t, &v, ReplayCheck::Valid), PassDecision::Confirm);
    assert!(w.ev.is_frozen(), "the account is frozen from the request on");
    assert_eq!(w.order(market_order(0, true, MEUR, t + 1)), Err(OrderReject::AccountFrozen));
    // the pass is confirmed; phase 2 -> 3 starts fresh at the finalization and unfreezes
    let after = ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: S100K, flat: true, phase: 3 };
    assert_eq!(w.ev.phase_started(3, &after), Ok(()));
    assert!(!w.ev.is_frozen());
    let st = w.ev.state();
    assert_eq!((w.ev.phase(), st.b, st.q_tally, st.latch, st.tight_on, st.flat), (3, S100K, 0, false, false, true));
    assert!(w.order(market_order(0, true, MEUR, t + 1)).is_ok());
}

#[test]
fn a_new_phase_keeps_the_breach_lock() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    // fills at ASK_AT_ONE; marks at the bid floor(mid x 0.999999): P(10_012) = 1_001_198_998_800 - ASK_AT_ONE = 1_197_998_800 > 0.01 S
    // starts tight mode (R = 100_000_000_000, L_start = 96_000_000_000); the peak at 10_030 is P = 1_002_998_997_000 - ASK_AT_ONE =
    // 2_997_997_000, so L = L_start + P - 0.01 S = 97_997_997_000. The breach needs equity <= L, i.e. P <= -2_002_003_000, i.e. a bid
    // of 997_998_997_000: the mid 997_999_995_000 (x 0.999999 = 997_998_997_000.005, floored). (At mid 0.998 the loss is only
    // 2_001_998_000: the spread costs 5_000 more than the old exact -2_000_000_000 against L = 98_000_000_000, so it would not breach.)
    for (dt, mid) in [(30, p4(10_006)), (60, p4(10_012)), (90, p4(10_030)), (150, 997_999_995_000)] {
        w.sample(0, t + dt, mid);
    }
    w.ev.tick(t + 151);
    let lock = w.ev.state().lock_until;
    assert!(lock > 0);
    // the account pressed the button after the breach (the pass is refused: the target is not reached, but the account is frozen), then the
    // journal is posted and the chain moves to phase 3
    let _ = w.ev.drain_batches(&BatchPolicy::MAX);
    let v = ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: w.ev.state().b, flat: true, phase: 2 };
    assert!(matches!(w.ev.request_pass(t + 151, &v, ReplayCheck::Skipped), PassDecision::Reject(_)));
    assert_eq!(w.ev.phase_started(3, &ChainView { b: S100K, phase: 3, ..v }), Ok(()));
    assert_eq!(w.ev.state().lock_until, lock);
    assert_eq!(w.ev.state().b, S100K);
}

#[test]
fn every_rejection() {
    // JournalBehind: events still queued
    let (mut w, t) = winner();
    let v = ChainView { seq: 0, journal_hash: [0; 32], b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(t, &v, ReplayCheck::Valid), PassDecision::Reject(PassReject::JournalBehind));
    // ... or the chain is at another sequence / hash than the evaluator
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let good = view(&w, &bs);
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &ChainView { seq: good.seq - 1, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::JournalBehind));
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &ChainView { journal_hash: [7; 32], ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::JournalBehind));
    // NotFlat: the chain says so
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &ChainView { flat: false, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat));
    // Other: a pending order, a balance that differs, another phase, a watchdog mismatch
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &ChainView { b: good.b + 1, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &ChainView { phase: 3, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Mismatch), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved().unwrap();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Skipped), PassDecision::Confirm);
    w.ev.pass_resolved().unwrap();
    w.order(order(evaluator::codec::IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, t)).unwrap();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    // the reject codes are the program's
    assert_eq!([PassReject::NotFlat as u8, PassReject::JournalBehind as u8, PassReject::TargetNotReached as u8, PassReject::Other as u8], [0, 1, 2, 3]);
}

#[test]
fn not_flat_and_target() {
    // an open position (events posted): NotFlat
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 10, p4(10_000));
    w.ev.tick(t + 11);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let v = view(&w, &bs);
    assert!(!v.flat);
    assert_eq!(w.ev.request_pass(t + 11, &v, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat));
    // the target: 1.08 S exactly passes, one micro less does not. The entry mid 1.0 fills at ASK_AT_ONE; the exit takes the bid
    // floor(exit x 0.999999). A bid of ASK_AT_ONE + 8_000_000_000 = 1_008_001_000_000 needs the mid 1_008_002_008_003
    // (x 0.999999 = 1_008_001_000_000.99, floored); the mid one below gives 1_008_000_999_999.99, floored: one micro less.
    for (exit, profit, ok) in [(1_008_002_008_003u64, 8_000_000_000i64, true), (1_008_002_008_002, 7_999_999_999, false)] {
        let mut w = fx_world(TEST_HALF_SPREAD_E6);
        let entry = 1_000_000_000_000u64;
        w.sample(0, t, entry);
        w.order(market_order(0, true, MEUR, t)).unwrap();
        w.sample(0, t + 100, exit);
        w.order(close_order(0, 0, t + 100)).unwrap();
        w.sample(0, t + 110, exit);
        w.ev.tick(t + 111);
        assert_eq!(w.ev.state().b, S100K + profit, "profit {profit}");
        let bs = w.ev.drain_batches(&BatchPolicy::MAX);
        let v = view(&w, &bs);
        let d = w.ev.request_pass(t + 111, &v, ReplayCheck::Valid);
        assert_eq!(d, if ok { PassDecision::Confirm } else { PassDecision::Reject(PassReject::TargetNotReached) }, "profit {profit}");
    }
}

#[test]
fn a_failed_phase_or_ended_account_is_rejected() {
    // a quick-limit fail
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 30, p4(10_060));
    w.order(close_order(0, 0, t + 30)).unwrap();
    w.sample(0, t + 40, p4(10_060));
    w.ev.tick(t + 41);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let v = view(&w, &bs);
    assert_eq!(w.ev.request_pass(t + 41, &v, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
}

#[test]
fn request_pass_closes_the_seconds_before_now() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 10, p4(10_000));
    // the order has not executed yet (second t+10 is the open one, t closed already): it has; the request at t+20 closes t+10
    let before = w.ev.processed_through();
    let v = ChainView { seq: 0, journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    let _ = w.ev.request_pass(t + 20, &v, ReplayCheck::Valid);
    assert!(w.ev.processed_through() > before);
    assert_eq!(w.ev.clock(), t + 20);
}
