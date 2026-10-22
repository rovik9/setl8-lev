//! P26 (decision 69, audit finding F-16): once a batch pushes the quick tally above 0.028 S, the only events that may still stand before
//! the final `Fail{PhaseQuick}` are `Close` events with reason `Fail` (the forced close-all of the remaining positions). Hand-written from
//! the decision; every refusal asserts the exact error and an unchanged world.
//!
//! Numbers: S = $100,000, limit 0.028 S = 2,800,000,000 micro-USD. A `Close` built by `quick(t, c)` is a quick profitable close credited
//! exactly `c` (pnl 2c, rounded down by half). The program does not track positions, so every close here is in the EUR market and uses
//! the one proof of the batch (a batch has at most 8 events and 3 distinct proofs).
mod common;

use common::journal::*;
use common::lifecycle::*;
use journal_types::{CloseReason, FailReason, JournalEvent};
use setl8_lev_trading::{constants as c, errors::Setl8Error as E};

const PQ: u8 = 1; // PhaseQuick
const SR: u8 = 0; // StaticFloor
const CREDIT: i64 = 1_499_000_000; // the E-03 credit of a 0.5 M EUR position at +0.6 %

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(172_800, 100_000_000_000, 86_400);
    (w, t)
}

/// A quick profitable close of a long, credited exactly `credit` (pnl = 2 x credit), reason Manual, intent 1.
fn quick(t: i64, credit: i64) -> JournalEvent {
    close_long_pnl(t, t - 10, 2 * credit)
}

/// The same close as a forced close of the close-all: reason Fail, no intent.
fn forced(ev: JournalEvent) -> JournalEvent {
    match ev {
        JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, proof, .. } => {
            JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason: CloseReason::Fail, intent_id: 0, proof }
        }
        other => other,
    }
}

fn with_reason(ev: JournalEvent, r: CloseReason) -> JournalEvent {
    match ev {
        JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, intent_id, proof, .. } => {
            JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason: r, intent_id, proof }
        }
        other => other,
    }
}

fn rf(w: &mut JEnv, t: i64, pre: Vec<JournalEvent>, reason: FailReason, b_after: i64, flat: bool, arg: u8) -> litesvm::types::TransactionResult {
    let b = w.fail_batch(t, pre, reason, b_after, flat);
    w.report_fail(t + 1, &b, arg, &[MARKET_EUR])
}

fn total(credits: &[i64]) -> i64 {
    S100K + credits.iter().sum::<i64>()
}

// ---------------------------------------------------------------------------------------------------- accepted

#[test]
fn the_e03_story_is_one_report_fail_batch_four_closes_the_fail_and_the_meta() {
    // 3 x 0.5 M EUR (credit 1,499 each) and the 1 M EUR one (2,999): ascending, the tally is 1,499 / 2,998 (over) / 4,497 / 7,496
    let (mut w, t) = base();
    let credits = [CREDIT, CREDIT, CREDIT, 2 * CREDIT + 1_000_000];
    let evs = vec![quick(t, credits[0]), quick(t, credits[1]), forced(quick(t, credits[2])), forced(quick(t, credits[3]))];
    w.ok(|w| rf(w, t, evs, FailReason::PhaseQuick, total(&credits), true, PQ));
    let a = w.account();
    assert_eq!((a.status, a.b, a.q_tally, a.flat), (c::STATUS_FAILED, total(&credits), credits.iter().sum::<i64>(), 1));
}

#[test]
fn the_crossing_close_can_come_first_and_every_other_close_after_it_is_a_forced_one() {
    let (mut w, t) = base();
    let credits = [2_900_000_000i64, 100, 200];
    let evs = vec![quick(t, credits[0]), forced(quick(t, credits[1])), forced(quick(t, credits[2]))];
    w.ok(|w| rf(w, t, evs, FailReason::PhaseQuick, total(&credits), true, PQ));
    assert_eq!(w.account().status, c::STATUS_FAILED);
}

#[test]
fn two_positions_the_second_close_crosses_and_one_forced_close_follows() {
    let (mut w, t) = base();
    let credits = [1_500_000_000i64, 1_500_000_000, 700_000_000];
    let evs = vec![quick(t, credits[0]), quick(t, credits[1]), forced(quick(t, credits[2]))];
    w.ok(|w| rf(w, t, evs, FailReason::PhaseQuick, total(&credits), true, PQ));
    assert_eq!(w.account().q_tally, credits.iter().sum::<i64>());
}

#[test]
fn eight_positions_the_first_two_are_posted_the_other_six_close_in_the_failing_batch() {
    // eight closes of 1,000 each: the tally is 1,000 / 2,000 (two posted by post_journal) then 3,000 crosses the limit 2,800: the failing
    // batch holds the crossing close, five forced closes, the Fail and the Meta = 8 events, the most a batch can have
    let (mut w, t) = base();
    let each = 1_000_000_000i64;
    let first = simple(&w, t, vec![quick(t, each), quick(t, each)], false);
    w.post_ok(t + 1, &first, &[MARKET_EUR]);
    assert_eq!(w.account().q_tally, 2 * each);
    let mut evs = vec![quick(t, each)];
    for _ in 0..5 {
        evs.push(forced(quick(t, each)));
    }
    assert_eq!(evs.len(), 6);
    w.ok(|w| rf(w, t, evs, FailReason::PhaseQuick, S100K + 8 * each, true, PQ));
    let a = w.account();
    assert_eq!((a.status, a.q_tally, a.b), (c::STATUS_FAILED, 8 * each, S100K + 8 * each));
}

#[test]
fn the_batch_limits_still_hold_a_ninth_event_is_not_a_batch() {
    // seven closes + Fail + Meta = 9 events: the eight-event cap of the format is unchanged (the encoder refuses it; the evaluator closes
    // everything it can before the crossing in earlier batches and counts a group that does not fit as unpostable)
    let (w, t) = base();
    let mut evs = vec![quick(t, 1_000_000_000)];
    for _ in 0..6 {
        evs.push(forced(quick(t, 1_000_000_000)));
    }
    evs.push(JournalEvent::Fail { time: t, reason: FailReason::PhaseQuick, b_after: S100K, proof: 0 });
    evs.push(meta(t, true));
    assert_eq!(evs.len(), 9);
    let b = journal_types::JournalBatch { first_seq: w.account().seq + 1, prev_hash: w.account().journal_hash, events: evs, proofs: vec![p_eur(t)] };
    assert!(b.to_bytes().is_err(), "nine events do not encode");
    assert_eq!(journal_types::MAX_EVENTS_PER_BATCH, 8);
    assert_eq!(journal_types::MAX_PROOFS_PER_BATCH, 3);
}

// ---------------------------------------------------------------------------------------------------- refused

#[test]
fn a_fill_after_the_overshoot_is_refused() {
    let (mut w, t) = base();
    let evs = vec![quick(t, 2_900_000_000), fill_long(t, 0), forced(quick(t, 1))];
    w.fails(E::QuickLimitExceeded, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[2_900_000_000, 1]), true, PQ));
}

#[test]
fn a_close_with_another_reason_after_the_overshoot_is_refused_for_every_reason() {
    for r in [CloseReason::Manual, CloseReason::StopLoss, CloseReason::TakeProfit, CloseReason::Flip, CloseReason::Breach] {
        let (mut w, t) = base();
        let evs = vec![quick(t, 2_900_000_000), with_reason(quick(t, 1), r)];
        w.fails(E::QuickLimitExceeded, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[2_900_000_000, 1]), true, PQ));
    }
}

#[test]
fn a_fee_or_a_floor_event_after_the_overshoot_is_refused() {
    let (mut w, t) = base();
    let evs = vec![quick(t, 2_900_000_000), fee(MARKET_EUR, 0, t)];
    w.fails(E::QuickLimitExceeded, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[2_900_000_000]), true, PQ));
    let evs = vec![quick(t, 2_900_000_000), forced(quick(t, 1)), fee(MARKET_EUR, 0, t)];
    w.fails(E::QuickLimitExceeded, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[2_900_000_000, 1]), true, PQ));
}

#[test]
fn forced_closes_without_any_overshoot_do_not_justify_a_quick_fail() {
    // Close{Fail} events are allowed anywhere in a report_fail batch (a static-floor fail closes everything with them), but the Fail
    // PhaseQuick still needs the tally above the limit at its end
    let (mut w, t) = base();
    let evs = vec![forced(quick(t, 1_000_000_000)), forced(quick(t, 1_000_000_000))];
    w.fails(E::FailQuickNotExceeded, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[1_000_000_000, 1_000_000_000]), true, PQ));
}

#[test]
fn a_static_floor_fail_cannot_stand_after_an_overshoot() {
    let (mut w, t) = base();
    let evs = vec![quick(t, 2_900_000_000), forced(quick(t, 1))];
    w.fails(E::QuickLimitNeedsPhaseQuick, |w| rf(w, t, evs, FailReason::StaticFloor, total(&[2_900_000_000, 1]), true, SR));
}

#[test]
fn closes_after_the_overshoot_that_leave_a_position_open_are_not_flat() {
    let (mut w, t) = base();
    let evs = vec![quick(t, 2_900_000_000), forced(quick(t, 1))];
    w.fails(E::FailNotFlat, |w| rf(w, t, evs, FailReason::PhaseQuick, total(&[2_900_000_000, 1]), false, PQ));
}

#[test]
fn the_fail_balance_must_be_the_balance_after_all_the_closes() {
    let (mut w, t) = base();
    let credits = [2_900_000_000i64, 7, 9];
    for wrong in [total(&credits) - 1, total(&credits) + 1, total(&credits[..1])] {
        let evs = vec![quick(t, credits[0]), forced(quick(t, credits[1])), forced(quick(t, credits[2]))];
        w.fails(E::FailBalanceMismatch, |w| rf(w, t, evs, FailReason::PhaseQuick, wrong, true, PQ));
    }
}

#[test]
fn only_a_meta_may_follow_the_fail() {
    let (mut w, t) = base();
    let mut evs = vec![quick(t, 2_900_000_000), forced(quick(t, 1))];
    let b_after = total(&[2_900_000_000, 1]);
    evs.push(JournalEvent::Fail { time: t, reason: FailReason::PhaseQuick, b_after, proof: 0 });
    evs.push(forced(quick(t, 1)));
    evs.push(meta(t, true));
    let b = w.batch(evs, vec![p_eur(t)]);
    w.fails(E::FailNotBeforeMeta, |w| w.report_fail(t + 1, &b, PQ, &[MARKET_EUR]));
}

#[test]
fn post_journal_still_refuses_the_crossing_close_and_every_forced_close_after_it() {
    let (mut w, t) = base();
    let pj = simple(&w, t, vec![quick(t, 2_900_000_000)], false);
    w.post_rejected(t + 1, &pj, &[MARKET_EUR], E::QuickLimitExceeded);
    let pj = simple(&w, t, vec![quick(t, 1_000_000_000), quick(t, 1_000_000_000), forced(quick(t, 1_000_000_000))], false);
    w.post_rejected(t + 1, &pj, &[MARKET_EUR], E::QuickLimitExceeded);
    // a forced close under the limit is still a plain close for post_journal
    let (mut w, t) = base();
    let pj = simple(&w, t, vec![forced(quick(t, 1_000_000_000))], false);
    w.post_ok(t + 1, &pj, &[MARKET_EUR]);
}

// ---------------------------------------------------------------------------------------------------- the boundary

#[test]
fn a_tally_exactly_at_the_limit_is_not_an_overshoot_one_micro_usd_above_is() {
    // q = 0.028 S - 1,000,000; a close crediting 1,000,000 leaves q EXACTLY at the limit: the next close is under the plain rules
    // (any reason), and it is the one that crosses (credit 1). With q one micro-USD higher the same second close is refused.
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT - 1_000_000);
    let at_limit = quick(t, 1_000_000);
    let crossing = quick(t, 1); // Manual, intent 1: only allowed when the state before it is NOT over
    w.ok(|w| rf(w, t, vec![at_limit, crossing], FailReason::PhaseQuick, total(&[1_000_000, 1]), true, PQ));
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT - 1_000_000 + 1);
    let one_above = quick(t, 1_000_000);
    let crossing = quick(t, 1);
    w.fails(E::QuickLimitExceeded, |w| rf(w, t, vec![one_above, crossing], FailReason::PhaseQuick, total(&[1_000_000, 1]), true, PQ));
    // and the same second close as a forced close is fine
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT - 1_000_000 + 1);
    let one_above = quick(t, 1_000_000);
    let forced_close = forced(quick(t, 1));
    w.ok(|w| rf(w, t, vec![one_above, forced_close], FailReason::PhaseQuick, total(&[1_000_000, 1]), true, PQ));
}
