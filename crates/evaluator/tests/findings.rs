//! Disagreements between the evaluator (which follows the DESIGN), the program and the public verifier, each reduced to a
//! minimal deterministic history. Every test here ASSERTS THE CURRENT DISAGREEMENT: when a component is fixed the test
//! fails and tells you to update handover/module-10.md. See "Findings" there.
mod common;
use common::*;
use evaluator::codec::IntentKind;
use evaluator::*;
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent};
use setl8_verify::FindingClass as F;

fn classes(w: &World, batches: &[Batch]) -> Vec<F> {
    verify(w, batches).classes()
}

fn drain(w: &mut World) -> Vec<Batch> {
    w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true))
}

/// FINDING 1. DESIGN 7.2 N-9: a pending order that fills in a market where a position exists nets normally. When it only
/// reduces the position, the result is a `Close` and no `Fill` event, so nothing in the journal says "this limit order
/// filled", and the verifier's archive walk (which expects a `Fill` of kind Limit/Stop for every triggered pending order)
/// reports `TriggerMissed` for an honest history.
#[test]
fn finding_1_a_pending_order_that_only_reduces_has_no_fill_event() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    let n = w.order(order(IntentKind::Limit, 0, 1, MEUR / 4, p4(10_050), 0, 0, 0, t)).unwrap(); // sell 250,000 at 1.0050 or better
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 120, p4(10_060)); // the limit triggers; it only reduces
    w.ev.tick(t + 130);
    let bs = drain(&mut w);
    let evs = events_of(&bs);
    assert_eq!(w.ev.stats().limit_fills, 1);
    let close = evs.iter().find_map(|e| if let JournalEvent::Close { qty, reason, intent_id, .. } = e { Some((*qty, *reason, *intent_id)) } else { None }).unwrap();
    assert_eq!(close, (MEUR / 4, CloseReason::Manual, n), "a Close under the limit order's intent, no Fill");
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: journal_types::OrderKind::Limit, .. })));
    assert!(classes(&w, &bs).contains(&F::TriggerMissed), "the verifier cannot see that the limit order was executed");
}

/// FINDING 2. DESIGN 4.4 I-8 allows nothing after a quick-tally overshoot but the `Fail`; the verifier (module 9, convention 4)
/// wants a commission `Fee` after every `Close`. With a non-zero commission the two cannot both be satisfied: the program
/// refuses `[Close, Fee, Fail]` (see tests-rs `finding_2`), so the evaluator omits the fee of the overshooting slice, and the
/// verifier reports `FeeMismatch` for the honest history.
#[test]
fn finding_2_commission_fee_of_a_quick_limit_fail() {
    let mut rules = base_rules(vec![{ let mut m = eur_market(0); m.half_spread_e6 = 0; m }]);
    rules.commission_e6[0] = 20;
    let mut w = World::new(rules, S100K, 2, T_1400 - HOUR);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 30, 1_005_602_000_000); // +5,602 raw in 30 s: credited 2,801 > 2,800
    w.order(close_order(0, 0, t + 30)).unwrap();
    w.sample(0, t + 40, 1_005_602_000_000);
    w.ev.tick(t + 41);
    let bs = drain(&mut w);
    let evs = events_of(&bs);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let commissions = evs.iter().filter(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Commission, .. })).count();
    assert_eq!(commissions, 1, "only the fill is charged; the overshooting close has no fee event");
    let k = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })).unwrap();
    assert!(matches!(evs[k - 1], JournalEvent::Close { .. }), "the Fail follows the Close directly");
    assert!(classes(&w, &bs).contains(&F::FeeMismatch));
}

/// FINDING 3. DESIGN 7.3 / decision 27: the margin of a pending order is checked when it fills. The verifier expects every
/// triggered pending order to fill (`TriggerMissed` otherwise), so a fill refused for margin shows up as a verifier mismatch.
#[test]
fn finding_3_a_pending_fill_refused_for_margin_is_a_trigger_missed() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, 4_900 * MEUR / 1000, t)).unwrap(); // $4.9M: margin $98,000 of $100,000
    let _ = w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_990), 0, 0, 0, t)).unwrap(); // accepted at arrival: margin is not reserved
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 20, p4(9_985)); // the limit triggers; $1M more needs $20,000 of margin
    w.ev.tick(t + 30);
    assert_eq!(w.ev.stats().refused_fills, 1);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(OrderReject::InsufficientMargin));
    let bs = drain(&mut w);
    assert!(classes(&w, &bs).contains(&F::TriggerMissed));
}

/// FINDING 4. A breach closes every position at the sample; a profitable QUICK slice is credited only half, so the balance
/// after the closes can be at or below the static floor although the equity that triggered the breach was above it. The
/// program then refuses the `Breach` (`BreachBelowStaticFloor`) and wants a `report_fail`; the evaluator posts
/// `Fail(StaticFloor)` instead, and the verifier, whose walk expects a Breach at that sample, reports `MissedBreach`.
#[test]
fn finding_4_a_breach_whose_closes_end_below_the_static_floor() {
    let mk = |i: u16, s: &str| { let mut m = common::market(i, 0, s, 0); m.max_age_secs = 3_600; m };
    let mut w = World::new(base_rules(vec![mk(0, "AAA"), mk(1, "BBB")]), S100K, 2, T_1400 - HOUR);
    let mut t = T_1400;
    // B = 92,500 (a normal -7,500 slice), static floor 92,000
    w.sample(0, t, p4(10_000));
    w.sample(1, t + 1, p4(10_000));
    w.order(market_order(0, true, MEUR, t + 1)).unwrap();
    t += 100;
    w.sample(0, t, p4(9_925));
    w.sample(1, t + 1, p4(10_000));
    w.order(close_order(0, 0, t + 1)).unwrap();
    t += 10;
    w.sample(0, t, p4(9_925));
    w.sample(1, t + 1, p4(10_000));
    w.ev.tick(t + 2);
    assert_eq!(w.ev.state().b, 92_500_000_000);
    t += 3;
    // two positions at 1.0. A moves to +1,500 (P = +1,500: tight mode, R = 92,500, L = 92,500 + 500 = 93,000) ...
    w.sample(0, t, p4(10_000));
    w.sample(1, t + 1, p4(10_000));
    w.order(market_order(0, true, MEUR, t + 1)).unwrap();
    w.order(market_order(1, true, MEUR, t + 1)).unwrap();
    w.sample(0, t + 10, p4(10_015));
    w.sample(1, t + 11, p4(10_000));
    // ... then B falls to -1,500: P = 0, E = 92,500 <= L, and E is above the static floor 92,000: the breach sample
    w.sample(1, t + 40, p4(9_985));
    w.ev.tick(t + 50);
    let bs = drain(&mut w);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::StaticFloor)), "stats {:?}", w.ev.stats());
    let evs = events_of(&bs);
    let b_after = evs.iter().find_map(|e| if let JournalEvent::Fail { b_after, .. } = e { Some(*b_after) } else { None }).unwrap();
    assert_eq!(b_after, 92_500_000_000 + 750_000_000 - 1_500_000_000, "A's quick profit is halved, B's loss is in full");
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })));
    assert!(classes(&w, &bs).contains(&F::MissedBreach));
}

/// FINDING 4, second form: the forced closes of a breach push the quick tally over its limit. Nothing but the `Fail` may follow
/// an overshoot, so the breach becomes `Fail(PhaseQuick)`; the verifier's walk still expects the Breach.
#[test]
fn finding_4_a_breach_whose_closes_overshoot_the_quick_tally() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 20, p4(11_070)); // +10,700: tight mode, G = 10,700, L = 96,000 + 9,700 = 105,700
    w.sample(0, t + 40, p4(10_570)); // +5,700: E = 105,700 <= L: breach; the quick slice credits 2,850 > 2,800
    w.ev.tick(t + 45);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)), "{:?}", w.ev.stats());
    let bs = drain(&mut w);
    let evs = events_of(&bs);
    let k = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })).unwrap();
    assert!(matches!(evs[k - 1], JournalEvent::Close { reason: CloseReason::Breach, quick: true, .. }), "{:?}", evs[k - 1]);
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })));
    assert!(classes(&w, &bs).contains(&F::MissedBreach));
}
