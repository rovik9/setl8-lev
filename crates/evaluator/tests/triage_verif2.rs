//! Triage of claim V-18 of the M12 independent review (evaluator against verifier).
//! Each test asserts the CORRECT behaviour and is ignored until the finding is fixed.
mod common;
use common::*;

/// Two samples of one market inside the same second: the first has E between the static floor and the tight floor
/// (the shape of a Breach), the second has E at or under the static floor.
fn two_samples_in_one_second() -> (World, Vec<evaluator::Batch>) {
    let mut w = World::base();
    w.sample(0, T_1400, eur(10_800));
    w.order(market_order(0, true, 1_000_000_000_000, T_1400)).expect("order");
    w.sample(0, T_1400 + 30, eur(10_806));
    w.sample(0, T_1400 + 60, eur(10_813)); // tight mode starts (l_level about 96,235)
    let ts = u64::try_from(T_1400 + 90).unwrap() * 1_000_000_000;
    // sample 1: mid 1.0760 -> E about 95,937 (static floor 92,000 < E <= tight floor)
    w.ev.on_sample(0, ts, i128::from(eur(10_760)) * 1_000_000, ts + 1_000_000, [0; 32]).expect("s1");
    // sample 2 (500 ms later, same second): mid 1.0720 -> E about 91,936 (at or under the static floor)
    w.ev.on_sample(0, ts + 500_000_000, i128::from(eur(10_720)) * 1_000_000, ts + 501_000_000, [0; 32]).expect("s2");
    w.ev.tick(T_1400 + 91);
    let batches = w.ev.drain_batches(&evaluator::BatchPolicy::MAX.with_heartbeat(true));
    (w, batches)
}

// FINDING V-18
#[test]
#[ignore = "FINDING V-18: a breach-shaped sample and a static-fail sample in one second: the evaluator posts Close..Fail, the verifier keeps a MissedBreach expectation (honest account reads MISMATCH)"]
fn v_18_breach_and_static_fail_samples_in_one_second_verify_valid() {
    let (w, batches) = two_samples_in_one_second();
    let evs = events_of(&batches);
    // the evaluator's output is what the claim says: closes and a StaticFloor fail, no Breach event
    assert!(evs.iter().any(|e| matches!(e, journal_types::JournalEvent::Fail { .. })), "{evs:?}");
    assert_valid(&w, &batches);
}
