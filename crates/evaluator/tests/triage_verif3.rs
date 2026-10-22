//! Triage of claims E-01, E-02 and E-03 of the M12 independent review (reference evaluator).
//! Each test asserts the CORRECT behaviour and is ignored until the finding is fixed.
mod common;
use common::*;
use evaluator::codec::IntentKind;
use journal_types::{JournalEvent, Side};

fn ns(sec: i64) -> u64 {
    u64::try_from(sec).unwrap() * 1_000_000_000
}

const ONE_E18: i128 = 1_000_000_000_000_000_000;

// FINDING E-01
// `accept_order` moves the clock to the order's `client_time` (up to clock + 5 s), and `on_sample` refuses every sample older than the
// clock. The bound is relative to the clock, not to real (sample) time, so six valid Limit/Cancel orders of the trader's own walk the
// clock 30 s ahead of the newest real sample and the next real samples are refused and not archived. Severity medium (the trader
// can skip his own floor, breach and SL checks for the skipped seconds; a second recorder shows MissedBreach). Needs a DESIGN change:
// a rule for an order's `client_time` against a time that the trader does not control (newest sample second or a caller-supplied now).
#[test]
fn e_01_a_trader_cannot_walk_the_clock_ahead_of_the_price_stream() {
    // P25b (decision 68, audit F-18): the trader's time never moves the evaluator's clock. Orders dated 5 s ahead are accepted ONCE and
    // do not walk it: the next one dated 10 s ahead is refused, and the real feed's next sample is recorded.
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    let last_limit = w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, T_1400 + 5)).expect("a limit dated 5 s ahead");
    assert_eq!(w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, last_limit, T_1400 + 10)), Err(evaluator::OrderReject::FutureOrder));
    assert!(w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, last_limit, T_1400 + 5)).is_ok());
    assert_eq!(w.ev.clock(), T_1400, "no order moved the clock");
    // real time is still about T_1400 + 1: the feed's next sample must be recorded
    let r = w.ev.on_sample_mid(0, T_1400 + 1, p4(10_000));
    assert!(r.is_ok(), "a real sample one second after the last one was refused: {r:?}");
    assert_eq!(w.ev.sample_count(), 2);
    assert_eq!(w.ev.clock(), T_1400 + 1);
}

// FINDING E-02
// `on_sample` never compares `ts_ns` with `recv_ns`. One sample dated far in the future moves the clock there, so every honest sample
// afterwards is `OutOfOrder` (clock) and `MarketTimeBackwards` (per market), for ever. Severity medium (liveness and grief for every
// account fed the poisoned stream, needs a bad upstream timestamp). Plain implementation error: refuse `ts_ns` more than a small
// tolerance past `recv_ns` (recv_ns is already an argument), no signature or rule change.
#[test]
fn e_02_a_future_dated_sample_does_not_poison_the_stream() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    let now = T_1400 + 1;
    // received now, but stamped one year ahead
    let _ = w.ev.on_sample(0, ns(now + 365 * 86_400), ONE_E18, ns(now), [0; 32]);
    let honest = w.ev.on_sample(0, ns(now), ONE_E18, ns(now), [0; 32]);
    assert!(honest.is_ok(), "the honest sample after the poisoned one was refused: {honest:?}");
}

// FINDING E-02 (consequence): the clock jump charges a holding fee for every night up to the poisoned second.
#[test]
fn e_02_a_future_dated_sample_does_not_queue_a_year_of_holding_fees() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), 1);
    let before = w.ev.queued_events();
    let now = T_1400 + 7;
    let _ = w.ev.on_sample(0, ns(now + 100 * 86_400), ONE_E18, ns(now), [0; 32]);
    w.ev.tick(now + 1);
    let queued = w.ev.queued_events() - before;
    assert!(queued < 5, "{queued} events were queued by one future-dated sample (fees for each of ~100 nights)");
}

// ---------------------------------------------------------------------------------------------------- E-03

/// Class 0 markets (half-spread 1e-6, commission 0), S = $100,000, phase 2.
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

// FINDING E-03
// A manual close that would push the quick tally over 0.028 S makes the evaluator close every other position first (reason Fail),
// ascending by tally, and only the LAST close of the sequence is expected to overshoot. When the first of the others already overshoots
// (here the credits of the three 0.5 M EUR positions are 1,499 / 2,999 / 4,498 against the limit 2,800) the queue holds
// Close(over), Close, Close, Close, Fail. `post_journal` rejects the first (QuickLimitExceeded), `report_fail` accepts the first but
// rejects every later Close (post_journal.rs:253-257), and a batch that ends after the first is not flat (FailNotFlat). The
// head of the queue is unpostable for ever (`stats.unpostable` counts it; exec_edges.rs:217 pins the count as 1). A breach or
// static fail reaches the same code through `force_close_all`. Severity high (an honest account's journal is stuck: it can neither
// continue nor be failed). Needs a DESIGN change: the program must accept several forced Close events after an overshoot (only
// Close with reason Fail/Breach, then the Fail), or the evaluator must have another way to end the account.
#[test]
#[ignore = "FINDING E-03: a forced close of 3+ young profitable positions near the quick limit emits a Close sequence neither post_journal nor report_fail accepts | P25b STOP (F-16): the evaluator already orders the closes so that an overshoot is last whenever any order allows it; THIS set cannot be ordered (the two smallest credits already exceed the limit), so no sequence is postable under the program rules I-8/I-11 without a program or DESIGN change"]
fn e_03_forced_closes_over_the_quick_limit_are_a_sequence_the_program_accepts() {
    let mut w = quick_world(4);
    for m in 0..4u16 {
        w.sample(m, T_1400, p4(10_000));
    }
    for (m, q) in [MEUR / 2, MEUR / 2, MEUR / 2, MEUR].iter().enumerate() {
        w.order(market_order(m as u16, true, *q, T_1400)).unwrap();
    }
    for m in 0..4u16 {
        w.sample(m, T_1400 + 5, p4(10_000));
    }
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), 4);
    for m in 0..4u16 {
        w.sample(m, T_1400 + 20, p4(10_060));
    }
    w.ev.tick(T_1400 + 21);
    w.order(close_order(3, 0, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    assert!(matches!(w.ev.status(), evaluator::Status::Failed(engine::FailReason::PhaseQuick)), "{:?}", w.ev.status());
    let evs = events_of(&w.drain());

    // replay the Close events through the program's own engine calls (post_journal.rs:293-299) and count the closes from the first
    // overshoot on: the program takes at most that one (as the last close before the Fail, in report_fail mode)
    let mut st = engine::reset_phase_state(S100K);
    let rules = engine::Rules::default();
    let mut from_first_overshoot = 0usize;
    for e in &evs {
        if let JournalEvent::Close { side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, .. } = e {
            let c = engine::check_close(&engine::CloseClaim {
                long: *side == Side::Long,
                qty: *qty,
                entry_e12: *entry_e12,
                exit_e12: *exit_e12,
                time: *time,
                timer_start: *timer_start,
                pnl: *pnl,
                quick: *quick,
                credited: *credited,
            })
            .expect("a well-formed close");
            let d = engine::apply_closed_slice(&st, &rules, c.credit, c.tally_delta).expect("slice");
            st = d.state;
            if d.fail.is_some() || from_first_overshoot > 0 {
                from_first_overshoot += 1;
            }
        }
    }
    assert!(from_first_overshoot <= 1, "{from_first_overshoot} Close events at or after the first overshoot of the quick limit: neither post_journal nor report_fail accepts them");
    assert_eq!(w.ev.stats().unpostable, 0);
}
