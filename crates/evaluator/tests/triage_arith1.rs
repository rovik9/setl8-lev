//! Triage of claims arith-03 and arith-06 of the M12 independent review (evaluator behaviour).
//! Each test asserts the CORRECT behaviour and is ignored until the finding is fixed.
mod common;
use common::*;
use journal_types::{CloseReason, JournalEvent};

fn world(markets: usize) -> World {
    let ms: Vec<_> = (0..markets as u16)
        .map(|i| {
            let mut m = market(i, 0, &format!("PAIR{i}"), TEST_HALF_SPREAD_E6);
            m.max_age_secs = 3_600;
            m
        })
        .collect();
    World::new(base_rules(ms), S100K, 2, T_1400 - HOUR)
}

/// arith-03 (audit F-16, FIXED by P26 / decision 69, ADAPTED): after the close that pushes the quick tally over 0.028 S the program
/// accepts only `Close` events with reason `Fail` (the forced close-all of the remaining positions) and then the `Fail`. The original
/// asserted "the very next event is the Fail"; the decision lets the remaining positions close first, with reason Fail.
#[test]
fn arith_03_no_close_follows_the_close_that_overshoots_the_quick_limit() {
    // 3 positions of 0.5 M EUR (+0.6%: quick credit 1,499 each) and a 1 M EUR one (+0.6%); closing the big one by hand overshoots,
    // so the three others are force-closed first (same set-up as exec_edges::forced_close(3)).
    let others = 3usize;
    let mut qtys = vec![MEUR / 2; others];
    qtys.push(MEUR);
    let mut w = world(others + 1);
    for m in 0..=others as u16 {
        w.sample(m, T_1400, p4(10_000));
    }
    for (m, q) in qtys.iter().enumerate() {
        w.order(market_order(m as u16, true, *q, T_1400)).unwrap();
    }
    for m in 0..=others as u16 {
        w.sample(m, T_1400 + 5, p4(10_000));
    }
    w.ev.tick(T_1400 + 6);
    for m in 0..=others as u16 {
        w.sample(m, T_1400 + 20, p4(10_060));
    }
    w.ev.tick(T_1400 + 21);
    w.order(close_order(others as u16, 0, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    let evs = events_of(&w.drain());
    let limit: i64 = 28 * S100K / 1000; // 0.028 S
    let mut tally = 0i64;
    let mut over_at = None;
    for (k, e) in evs.iter().enumerate() {
        if let JournalEvent::Close { quick, pnl, credited, .. } = e {
            if *quick && *pnl > 0 {
                tally += *credited;
            }
            if tally > limit && over_at.is_none() {
                over_at = Some(k);
            }
        }
    }
    let k = over_at.expect("the tally must cross the limit in this scenario");
    // after the overshooting close: forced closes (reason Fail) only, then the Fail
    let mut i = k + 1;
    while matches!(evs.get(i), Some(JournalEvent::Close { reason: CloseReason::Fail, .. })) {
        i += 1;
    }
    assert!(matches!(evs.get(i), Some(JournalEvent::Fail { .. })), "after the overshooting close only Close{{Fail}} events and then the Fail may follow, got {:?}", evs.get(i));
    let closes = evs.iter().filter(|e| matches!(e, JournalEvent::Close { .. })).count();
    assert_eq!(closes, others + 1, "every position is closed");
}

/// FINDING arith-06: DESIGN 2.2 says a breach closes everything "at THAT sample's price". Two samples of one market in one second:
/// the first touches the tight floor, the second recovers. The closes (and the Breach) must use the first one.
#[test]
fn arith_06_breach_close_uses_the_breach_sample_not_the_last_of_the_second() {
    let mut w = world(1);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 4 * MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), 1);
    w.sample(0, T_1400 + 20, p4(10_003)); // P > 1% S: tight mode starts, L_start = 96,000
    w.sample(0, T_1400 + 21, p4(10_010)); // G peak about 3,992: L about 98,992
    // second +22: j1 touches L (E about 95,992 <= L, above F_s 92,000), j2 recovers to the peak (no new raise)
    let ns = |s: i64, off: u64| u64::try_from(s).unwrap() * 1_000_000_000 + off;
    let q = |p: u64| i128::from(p) * 1_000_000;
    w.ev.on_sample(0, ns(T_1400 + 22, 0), q(p4(9_990)), ns(T_1400 + 22, 1_000_000), [0; 32]).unwrap();
    w.ev.on_sample(0, ns(T_1400 + 22, 500_000_000), q(p4(10_010)), ns(T_1400 + 22, 501_000_000), [0; 32]).unwrap();
    w.ev.tick(T_1400 + 24);
    let evs = events_of(&w.drain());
    let breach_bid = engine::bid_from_mid(p4(9_990), TEST_HALF_SPREAD_E6).unwrap();
    let close = evs
        .iter()
        .find_map(|e| if let JournalEvent::Close { reason: CloseReason::Breach, exit_e12, .. } = e { Some(*exit_e12) } else { None })
        .expect("a Breach close");
    assert_eq!(close, breach_bid, "the forced close must be at the bid of the breach sample");
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })), "the breach must be posted (with its 12 h lock), not a TightEnd");
}
