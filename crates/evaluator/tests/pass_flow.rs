//! The evaluator's side of the pass flow (DESIGN 4.10): what it answers to `request_advance`, and the phase start.
mod common;
use common::*;
use evaluator::*;

/// A zero-spread world whose account closed a +8.5% trade (non-quick) and is flat with B = 1.085 S.
fn winner() -> (World, i64) {
    let mut w = fx_world(0);
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
    assert_eq!(w.ev.state().b, 108_500_000_000);
    let v = view(&w, &bs);
    assert_eq!(w.ev.request_pass(t, &v, ReplayCheck::Valid), PassDecision::Confirm);
    assert!(w.ev.is_frozen(), "the account is frozen from the request on");
    assert_eq!(w.order(market_order(0, true, MEUR, t + 1)), Err(OrderReject::AccountFrozen));
    // the pass is confirmed; phase 2 -> 3 starts fresh at the finalization and unfreezes
    w.ev.phase_started(3);
    assert!(!w.ev.is_frozen());
    let st = w.ev.state();
    assert_eq!((w.ev.phase(), st.b, st.q_tally, st.latch, st.tight_on, st.flat), (3, S100K, 0, false, false, true));
    assert!(w.order(market_order(0, true, MEUR, t + 1)).is_ok());
}

#[test]
fn a_new_phase_keeps_the_breach_lock() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    for (dt, p) in [(30, 10_006), (60, 10_012), (90, 10_030), (150, 9_980)] {
        w.sample(0, t + dt, p4(p));
    }
    w.ev.tick(t + 151);
    let lock = w.ev.state().lock_until;
    assert!(lock > 0);
    w.ev.phase_started(3);
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
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &ChainView { seq: good.seq - 1, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::JournalBehind));
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &ChainView { journal_hash: [7; 32], ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::JournalBehind));
    // NotFlat: the chain says so
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &ChainView { flat: false, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat));
    // Other: a pending order, a balance that differs, another phase, a watchdog mismatch
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &ChainView { b: good.b + 1, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &ChainView { phase: 3, ..good }, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Mismatch), PassDecision::Reject(PassReject::Other));
    w.ev.pass_resolved();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Skipped), PassDecision::Confirm);
    w.ev.pass_resolved();
    w.order(order(evaluator::codec::IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, t)).unwrap();
    assert_eq!(w.ev.request_pass(t, &good, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other));
    // the reject codes are the program's
    assert_eq!([PassReject::NotFlat as u8, PassReject::JournalBehind as u8, PassReject::TargetNotReached as u8, PassReject::Other as u8], [0, 1, 2, 3]);
}

#[test]
fn not_flat_and_target() {
    // an open position (events posted): NotFlat
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 10, p4(10_000));
    w.ev.tick(t + 11);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let v = view(&w, &bs);
    assert!(!v.flat);
    assert_eq!(w.ev.request_pass(t + 11, &v, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat));
    // the target: 1.08 S exactly passes, one micro less does not
    for (profit, ok) in [(8_000_000_000i64, true), (7_999_999_999, false)] {
        let mut w = fx_world(0);
        let entry = 1_000_000_000_000u64;
        w.sample(0, t, entry);
        w.order(market_order(0, true, MEUR, t)).unwrap();
        w.sample(0, t + 100, entry + u64::try_from(profit).unwrap());
        w.order(close_order(0, 0, t + 100)).unwrap();
        w.sample(0, t + 110, entry + u64::try_from(profit).unwrap());
        w.ev.tick(t + 111);
        let bs = w.ev.drain_batches(&BatchPolicy::MAX);
        let v = view(&w, &bs);
        let d = w.ev.request_pass(t + 111, &v, ReplayCheck::Valid);
        assert_eq!(d, if ok { PassDecision::Confirm } else { PassDecision::Reject(PassReject::TargetNotReached) }, "profit {profit}");
    }
}

#[test]
fn a_failed_phase_or_ended_account_is_rejected() {
    // a quick-limit fail
    let mut w = fx_world(0);
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
    let mut w = fx_world(0);
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
