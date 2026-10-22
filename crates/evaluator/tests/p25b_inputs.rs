//! P25b (decision 68), evaluator side of the audit findings F-15, F-17, F-18, F-21: what the evaluator does with a hostile or skewed
//! input, written by hand from the decisions. The reference verifier (`assert_valid`) must agree with every honest history.
mod common;
use common::*;
use evaluator::codec::IntentKind;
use evaluator::{OrderReject, SampleError};
use journal_types::{CloseReason, JournalEvent};

const E18: i128 = 1_000_000_000_000_000_000;

fn ns(sec: i64) -> u64 {
    u64::try_from(sec).unwrap() * 1_000_000_000
}

// ------------------------------------------------------------------------------------------------ F-17: bad price samples

#[test]
fn a_sample_dated_more_than_five_seconds_after_its_receive_time_is_refused_and_counted() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    let now = T_1400 + 1;
    // exactly +5 s ahead of the receive time is allowed, +6 s is not
    assert_eq!(w.ev.on_sample(0, ns(now + 6), E18, ns(now), [0; 32]), Err(SampleError::FutureSample));
    assert_eq!((w.ev.stats().samples_future, w.ev.clock(), w.ev.sample_count()), (1, T_1400, 1), "refused samples never move the clock");
    assert_eq!(w.ev.on_sample(0, ns(now + 5), E18, ns(now), [0; 32]), Ok(()));
    assert_eq!((w.ev.clock(), w.ev.sample_count(), w.ev.stats().samples_future), (now + 5, 2, 1));
}

#[test]
fn a_sample_already_older_than_the_markets_max_age_when_received_is_refused_and_counted() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6); // max_age 3,600 s
    w.sample(0, T_1400, p4(10_000));
    let recv = T_1400 + 5_000;
    // 3,600 s old at receipt is fine, 3,601 s is stale
    assert_eq!(w.ev.on_sample(0, ns(recv - 3_601), E18, ns(recv), [0; 32]), Err(SampleError::StaleSample));
    assert_eq!(w.ev.stats().samples_stale, 1);
    assert_eq!(w.ev.on_sample(0, ns(recv - 3_600), E18, ns(recv), [0; 32]), Ok(()));
    assert_eq!(w.ev.stats().samples_stale, 1);
}

#[test]
fn a_refused_sample_leaves_the_market_time_alone_and_out_of_order_ones_are_counted() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400 + 10, p4(10_000));
    let before = w.ev.stats().samples_out_of_order;
    assert_eq!(w.ev.on_sample(0, ns(T_1400 + 9), E18, ns(T_1400 + 10), [0; 32]), Err(SampleError::OutOfOrder));
    assert_eq!(w.ev.on_sample(0, ns(T_1400 + 10) - 1, E18, ns(T_1400 + 10), [0; 32]), Err(SampleError::OutOfOrder));
    assert_eq!(w.ev.stats().samples_out_of_order, before + 2);
    // a poisoned timestamp first, then the honest feed: the honest one is recorded (E-02)
    let _ = w.ev.on_sample(0, ns(T_1400 + 11 + 365 * 86_400), E18, ns(T_1400 + 11), [0; 32]);
    assert_eq!(w.ev.on_sample(0, ns(T_1400 + 11), E18, ns(T_1400 + 11), [0; 32]), Ok(()));
}

// ------------------------------------------------------------------------------------------------ F-18: the order's time

#[test]
fn a_market_order_is_executed_in_the_evaluators_own_second_whatever_the_trader_signed() {
    for signed in [-5i64, 0, 3, 5] {
        let mut w = fx_world(TEST_HALF_SPREAD_E6);
        w.sample(0, T_1400, p4(10_000));
        w.order(market_order(0, true, MEUR, T_1400 + signed)).unwrap();
        w.sample(0, T_1400 + 20, p4(10_000));
        w.ev.tick(T_1400 + 21);
        let evs = events_of(&w.drain());
        let t = evs.iter().find_map(|e| if let JournalEvent::Fill { time, .. } = e { Some(*time) } else { None }).expect("a fill");
        assert_eq!(t, T_1400, "signed {signed:+} s from the evaluator's second");
    }
}

#[test]
fn a_market_order_trades_at_the_newest_sample_of_its_second_even_if_it_arrived_after_the_order() {
    // audit F-08: the verifier recomputes the price from the archive alone, so the evaluator cannot depend on who came first
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    let t = ns(T_1400) + 500_000_000;
    w.ev.on_sample(0, t, i128::from(p4(10_010)) * 1_000_000, t + 1_000_000, [0; 32]).unwrap(); // later in the SAME second
    w.sample(0, T_1400 + 20, p4(10_000));
    w.ev.tick(T_1400 + 21);
    let batches = w.drain();
    let ask = engine::ask_from_mid(p4(10_010), TEST_HALF_SPREAD_E6).unwrap();
    let price = events_of(&batches).iter().find_map(|e| if let JournalEvent::Fill { price_e12, .. } = e { Some(*price_e12) } else { None }).unwrap();
    assert_eq!(price, ask, "the fill is at the newest sample of the second");
    assert_valid(&w, &batches);
}

#[test]
fn a_cancel_dated_ahead_executes_at_its_signed_second_and_not_before() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    let limit = w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_990), 0, 0, 0, T_1400)).unwrap();
    w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, limit, T_1400 + 5)).unwrap();
    assert_eq!(w.ev.pending().len(), 1, "still pending until second T+5 closes");
    w.sample(0, T_1400 + 3, p4(9_980)); // triggers the limit (its ask is below the limit) before the cancel is effective
    w.sample(0, T_1400 + 10, p4(10_000));
    w.ev.tick(T_1400 + 11);
    let batches = w.drain();
    assert!(events_of(&batches).iter().any(|e| matches!(e, JournalEvent::Fill { .. })), "the limit filled before the cancel");
    assert_valid(&w, &batches);
}

// ------------------------------------------------------------------------------------------------ F-15 and F-21

/// Class 0 market, tight mode on; `second` is the pair of mids of two samples inside one second.
fn tight_world() -> World {
    let mut w = World::base();
    w.sample(0, T_1400, eur(10_800));
    w.order(market_order(0, true, 1_000_000_000_000, T_1400)).expect("order");
    w.sample(0, T_1400 + 30, eur(10_806));
    w.sample(0, T_1400 + 60, eur(10_813)); // tight mode starts
    w
}

fn two_in_one_second(w: &mut World, first: u64, second: u64) {
    let ts = ns(T_1400 + 90);
    w.ev.on_sample(0, ts, i128::from(eur(first)) * 1_000_000, ts + 1_000_000, [0; 32]).expect("s1");
    w.ev.on_sample(0, ts + 500_000_000, i128::from(eur(second)) * 1_000_000, ts + 501_000_000, [0; 32]).expect("s2");
    w.ev.tick(T_1400 + 91);
}

#[test]
fn a_static_fail_sample_wins_over_a_breach_sample_of_the_same_second_in_either_order() {
    // F-15 (decision 68): the more severe outcome wins. 1.0760 is a breach-shaped sample (E between the floors), 1.0720 a static fail.
    for (a, b) in [(10_760u64, 10_720u64), (10_720, 10_760)] {
        let mut w = tight_world();
        two_in_one_second(&mut w, a, b);
        let batches = w.drain();
        let evs = events_of(&batches);
        assert!(evs.iter().any(|e| matches!(e, JournalEvent::Fail { .. })), "{a}/{b}: {evs:?}");
        assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })), "{a}/{b}: no Breach event next to the Fail");
        assert_valid(&w, &batches);
    }
}

#[test]
fn a_lone_breach_sample_still_gives_a_breach() {
    let mut w = tight_world();
    two_in_one_second(&mut w, 10_760, 10_770); // both breach-shaped, none at the static floor
    let batches = w.drain();
    let evs = events_of(&batches);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })), "{evs:?}");
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fail { .. })));
    assert_valid(&w, &batches);
}

#[test]
fn the_forced_closes_use_the_marks_of_the_decision_sample_not_a_later_sample_of_the_second() {
    // F-21: sample 1 (1.0760) is the breach, sample 2 (1.0775) is a partial recovery in the same second; the closes are at sample 1's bid
    let mut w = tight_world();
    two_in_one_second(&mut w, 10_760, 10_775);
    let batches = w.drain();
    let evs = events_of(&batches);
    let want = engine::bid_from_mid(eur(10_760), 30).unwrap();
    let exit = evs.iter().find_map(|e| if let JournalEvent::Close { reason: CloseReason::Breach, exit_e12, .. } = e { Some(*exit_e12) } else { None }).expect("a breach close");
    assert_eq!(exit, want);
    assert_valid(&w, &batches);
}

#[test]
fn an_order_is_refused_for_the_documented_reasons_only() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 - 6)), Err(OrderReject::StaleOrder));
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 6)), Err(OrderReject::FutureOrder));
    assert!(w.order(market_order(0, true, MEUR, T_1400 - 5)).is_ok());
}

// ------------------------------------------------------------------------------------------------ F-16 (the feasible part)

/// Class 0 markets (half-spread 1e-6, commission 0), S = $100,000, phase 2, one market per position.
fn quick_world(markets: usize) -> World {
    let ms: Vec<_> = (0..markets as u16)
        .map(|i| {
            let mut m = market(i, 0, &format!("PAIR{i}"), TEST_HALF_SPREAD_E6);
            m.max_age_secs = 3_600;
            m
        })
        .collect();
    World::new(base_rules(ms), S100K, 2, T_1400 - HOUR)
}

/// Opens the positions `sizes` (EUR units of 1e-6), lets them age 20 s (quick), moves every price +0.6% and closes the LAST one by hand.
fn manual_close_over_the_limit(sizes: &[u64]) -> (World, Vec<evaluator::Batch>) {
    let mut w = quick_world(sizes.len());
    for m in 0..sizes.len() as u16 {
        w.sample(m, T_1400, p4(10_000));
    }
    for (m, q) in sizes.iter().enumerate() {
        w.order(market_order(m as u16, true, *q, T_1400)).unwrap();
    }
    for m in 0..sizes.len() as u16 {
        w.sample(m, T_1400 + 5, p4(10_000));
    }
    w.ev.tick(T_1400 + 6);
    for m in 0..sizes.len() as u16 {
        w.sample(m, T_1400 + 20, p4(10_060));
    }
    w.ev.tick(T_1400 + 21);
    w.order(close_order(sizes.len() as u16 - 1, 0, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    let batches = w.drain();
    (w, batches)
}

/// How many `Close` events stand at or after the first one that pushes the quick tally over the limit (the program takes one at most, as
/// the last close before the `Fail`; `report_fail` refuses every later one).
fn closes_from_the_first_overshoot(evs: &[JournalEvent]) -> usize {
    let mut st = engine::reset_phase_state(S100K);
    let rules = engine::Rules::default();
    let mut from = 0usize;
    for e in evs {
        if let JournalEvent::Close { side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, .. } = e {
            let c = engine::check_close(&engine::CloseClaim { long: *side == journal_types::Side::Long, qty: *qty, entry_e12: *entry_e12, exit_e12: *exit_e12, time: *time, timer_start: *timer_start, pnl: *pnl, quick: *quick, credited: *credited }).unwrap();
            let d = engine::apply_closed_slice(&st, &rules, c.credit, c.tally_delta).unwrap();
            st = d.state;
            if d.fail.is_some() || from > 0 {
                from += 1;
            }
        }
    }
    from
}

#[test]
fn when_a_postable_order_exists_the_forced_closes_follow_it_and_the_overshoot_is_last() {
    // two small positions (credits about 900 each) and the big one the trader closes (about 3,000 on its own, above the limit 2,800):
    // the two small ones fit under the limit (1,800), so "small ones first, the overshooting slice last, then the Fail" is postable
    let (w, batches) = manual_close_over_the_limit(&[MEUR * 3 / 10, MEUR * 3 / 10, MEUR]);
    assert!(matches!(w.ev.status(), evaluator::Status::Failed(engine::FailReason::PhaseQuick)), "{:?}", w.ev.status());
    let evs = events_of(&batches);
    assert_eq!(closes_from_the_first_overshoot(&evs), 1, "the overshooting close is the LAST close, and the Fail follows it");
    assert!(matches!(evs.iter().rev().find(|e| !matches!(e, JournalEvent::Meta { .. })), Some(JournalEvent::Fail { .. })));
    assert_eq!(w.ev.stats().unpostable, 0);
    assert_valid(&w, &batches);
}
