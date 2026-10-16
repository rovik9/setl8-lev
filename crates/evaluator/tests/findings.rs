//! M9b: the disagreements M10 found between the evaluator (which follows the DESIGN), the program and the public verifier
//! (handover/module-10.md "Findings" 1 to 5) are RESOLVED by decisions D1 to D5 (handover/design-decisions.md 53). Each
//! scenario below is the minimal history of its finding; the first test of a pair asserts that the verifier now says VALID,
//! the others edit the honest history into its nearest dishonest variants and assert that the verifier still catches them.
mod common;
use common::*;
use evaluator::codec::IntentKind;
use evaluator::*;
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent, OrderKind, Side};
use setl8_verify::FindingClass as F;

fn drain(w: &mut World) -> Vec<Batch> {
    w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true))
}

/// The report of the edited history (no snapshot: the stored chain head would no longer match).
fn report(w: &World, batches: &[Batch]) -> setl8_verify::Report {
    let f = files(w, batches);
    setl8_verify::verify_account(&setl8_verify::Inputs {
        events: f.events.as_bytes(),
        rules: f.rules.as_bytes(),
        archive: &f.archive,
        intents: &f.intents,
        snapshot: None,
        second_archive: None,
        as_of_seq: None,
    })
    .expect("inputs parse")
}

fn classes(w: &World, batches: &[Batch]) -> Vec<F> {
    report(w, batches).classes()
}

/// A copy of the batches with `f` applied to the event list of each batch.
fn edited(batches: &[Batch], f: impl Fn(&mut Vec<JournalEvent>)) -> Vec<Batch> {
    batches
        .iter()
        .map(|b| {
            let mut b = b.clone();
            f(&mut b.batch.events);
            b
        })
        .collect()
}

fn find_close(evs: &[JournalEvent]) -> Option<usize> {
    evs.iter().position(|e| matches!(e, JournalEvent::Close { .. }))
}

// ------------------------------------------------------------------------------------------------- D1

/// Finding 1 / D1. DESIGN 7.2 N-9: a pending order that fills in a market where a position exists nets normally. When it
/// only reduces the position, the result is a `Close` under the order's nonce and no `Fill`.
fn d1_world() -> (World, Vec<Batch>, u32) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    let n = w.order(order(IntentKind::Limit, 0, 1, MEUR / 4, p4(10_050), 0, 0, 0, t)).unwrap(); // sell 250,000 at 1.0050 or better
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 120, p4(10_060)); // the limit triggers; it only reduces
    w.ev.tick(t + 130);
    let bs = drain(&mut w);
    (w, bs, n)
}

#[test]
fn finding_1_a_pending_order_that_only_reduces_is_executed_by_a_close_and_verifies() {
    let (w, bs, n) = d1_world();
    let evs = events_of(&bs);
    assert_eq!(w.ev.stats().limit_fills, 1);
    let close = evs.iter().find_map(|e| if let JournalEvent::Close { qty, reason, intent_id, .. } = e { Some((*qty, *reason, *intent_id)) } else { None }).unwrap();
    assert_eq!(close, (MEUR / 4, CloseReason::Manual, n), "a Close under the limit order's intent, no Fill");
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. })));
    assert_valid(&w, &bs);
}

#[test]
fn d1_still_caught_the_trigger_passes_and_nothing_executes_the_order() {
    let (w, bs, _) = d1_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        evs.remove(k);
    });
    assert!(classes(&w, &bad).contains(&F::TriggerMissed));
}

#[test]
fn d1_still_caught_a_close_under_the_order_with_the_wrong_quantity_or_price() {
    let (w, bs, _) = d1_world();
    let qty = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        if let JournalEvent::Close { qty, .. } = &mut evs[k] {
            *qty += 1_000;
        }
    });
    let c = classes(&w, &qty);
    assert!(c.contains(&F::PnlMismatch) && c.contains(&F::ForgedTrade), "{c:?}");
    let price = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        if let JournalEvent::Close { exit_e12, .. } = &mut evs[k] {
            *exit_e12 += 1_000_000_000; // a tenth of a pip
        }
    });
    let c = classes(&w, &price);
    assert!(c.contains(&F::PriceMismatch) && c.contains(&F::PnlMismatch), "{c:?}");
}

/// A long 1 M EUR, a sell limit of 250,000 at 1.0500 that is never reached (nonce 2), a `set_sl_tp` (nonce 3) and a trader close of
/// half the position (nonce 4).
fn d1_untriggered_world() -> (World, Vec<Batch>, [u32; 3]) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    let pending = w.order(order(IntentKind::Limit, 0, 1, MEUR / 4, p4(10_500), 0, 0, 0, t)).unwrap(); // never reached
    w.sample(0, t + 10, p4(10_000));
    w.ev.tick(t + 11);
    let sltp = w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(9_000), 0, 0, t + 11)).unwrap();
    let close_nonce = w.order(close_order(0, MEUR / 2, t + 12)).unwrap();
    w.sample(0, t + 20, p4(10_001));
    w.ev.tick(t + 30);
    let bs = drain(&mut w);
    (w, bs, [pending, sltp, close_nonce])
}

#[test]
fn d1_still_caught_a_close_under_an_intent_that_is_no_order_of_the_trader() {
    let (w, bs, [pending, sltp, close_nonce]) = d1_untriggered_world();
    assert_valid(&w, &bs);
    let with_intent = |id: u32| {
        edited(&bs, |evs| {
            let Some(k) = find_close(evs) else { return };
            if let JournalEvent::Close { intent_id, .. } = &mut evs[k] {
                *intent_id = id;
            }
        })
    };
    // an intent id nobody signed, and the nonce of a set_sl_tp (not an order)
    assert!(classes(&w, &with_intent(999)).contains(&F::ForgedTrade));
    assert!(classes(&w, &with_intent(sltp)).contains(&F::ForgedTrade));
    // the nonce of a pending order that no sample of that second triggers: nothing executed it
    assert!(classes(&w, &with_intent(pending)).contains(&F::TriggerFalse));
    assert_ne!(close_nonce, pending);
}

// ------------------------------------------------------------------------------------------------- D2

fn commission_world() -> World {
    let mut rules = base_rules(vec![{
        let mut m = eur_market(0);
        m.half_spread_e6 = 0;
        m
    }]);
    rules.commission_e6[0] = 20;
    World::new(rules, S100K, 2, T_1400 - HOUR)
}

/// Finding 2 / D2. DESIGN 4.4 I-8 allows nothing after a quick-tally overshoot but the `Fail`: the overshooting slice has no
/// commission `Fee`.
fn d2_overshoot_world() -> (World, Vec<Batch>) {
    let mut w = commission_world();
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 30, 1_005_602_000_000); // +5,602 raw in 30 s: credited 2,801 > 2,800
    w.order(close_order(0, 0, t + 30)).unwrap();
    w.sample(0, t + 40, 1_005_602_000_000);
    w.ev.tick(t + 41);
    let bs = drain(&mut w);
    (w, bs)
}

/// The same history one tick short of the limit: a quick close of +1,000 (credit 500), commission charged.
fn d2_plain_quick_world() -> (World, Vec<Batch>) {
    let mut w = commission_world();
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 30, p4(10_010));
    w.order(close_order(0, 0, t + 30)).unwrap();
    w.sample(0, t + 40, p4(10_010));
    w.ev.tick(t + 41);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn finding_2_the_slice_that_overshoots_the_quick_limit_has_no_commission_fee_and_verifies() {
    let (w, bs) = d2_overshoot_world();
    let evs = events_of(&bs);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let commissions = evs.iter().filter(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Commission, .. })).count();
    assert_eq!(commissions, 1, "only the fill is charged; the overshooting close has no fee event");
    let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })) else { return };
    assert!(matches!(evs[k - 1], JournalEvent::Close { .. }), "the Fail follows the Close directly");
    assert_valid(&w, &bs);
}

#[test]
fn d2_still_caught_a_fee_missing_after_a_close_that_does_not_overshoot() {
    let (w, bs) = d2_plain_quick_world();
    assert_valid(&w, &bs);
    let bad = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        assert!(matches!(evs[k + 1], JournalEvent::Fee { kind: FeeKind::Commission, .. }));
        evs.remove(k + 1);
    });
    assert!(classes(&w, &bad).contains(&F::FeeMismatch));
}

#[test]
fn d2_still_caught_a_fee_posted_after_the_overshoot() {
    let (w, bs) = d2_overshoot_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        let JournalEvent::Close { market, qty, exit_e12, time, .. } = evs[k] else { unreachable!() };
        // ceil(20 ppm x notional) of the slice, as the verifier would charge it after a slice that did not overshoot
        let amount = engine::fee_ceil_micro(engine::margin_micro(qty, exit_e12, 1).unwrap(), 20).unwrap();
        evs.insert(k + 1, JournalEvent::Fee { kind: FeeKind::Commission, market, amount: u64::try_from(amount).unwrap(), time });
    });
    assert!(classes(&w, &bad).contains(&F::FeeMismatch));
}

#[test]
fn d2_still_caught_a_fail_after_a_slice_that_did_not_overshoot() {
    let (w, bs) = d2_plain_quick_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = find_close(evs) else { return };
        let JournalEvent::Close { time, proof, .. } = evs[k] else { unreachable!() };
        evs.insert(k + 2, JournalEvent::Fail { time, reason: FailReason::PhaseQuick, b_after: 0, proof });
    });
    assert!(classes(&w, &bad).contains(&F::FalseFail));
}

/// A flip whose CLOSING slice overshoots: the account fails before the opening remainder; the trades under the intent end at the Fail.
fn d2_flip_world(profit_pips: u64) -> (World, Vec<Batch>) {
    let mut w = world_for_flip();
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, 2 * MEUR, t)).unwrap();
    w.sample(0, t + 5, p4(10_000));
    w.sample(0, t + 20, p4(10_000 + profit_pips));
    w.ev.tick(t + 21);
    w.order(market_order(0, false, 3 * MEUR, t + 21)).unwrap(); // sell 3 M: closes 2 M, would open 1 M short
    w.ev.tick(t + 22);
    let bs = drain(&mut w);
    (w, bs)
}

fn world_for_flip() -> World {
    let mut m = eur_market(0);
    m.half_spread_e6 = 0;
    m.max_age_secs = 3_600;
    World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR)
}

#[test]
fn d2_a_flip_whose_closing_slice_overshoots_ends_at_the_fail_and_verifies() {
    // 2 M EUR, +0.30% in 20 s: profit $6,000, quick credit $3,000 over 0.028 S = $2,800
    let (w, bs) = d2_flip_world(30);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let evs = events_of(&bs);
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fill { qty, .. } if *qty == MEUR)), "the short is void");
    assert_valid(&w, &bs);
}

#[test]
fn d2_still_caught_a_flip_that_drops_its_remainder_without_an_overshoot() {
    // +0.20%: credit $2,000, no overshoot: the flip is Close(Flip) + Fill
    let (w, bs) = d2_flip_world(20);
    assert_valid(&w, &bs);
    let evs = events_of(&bs);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Fill { qty, .. } if *qty == MEUR)));
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fill { qty, .. } if *qty == MEUR)) else { return };
        evs.remove(k);
    });
    assert!(classes(&w, &bad).contains(&F::ForgedTrade));
}

// ------------------------------------------------------------------------------------------------- D3

/// Finding 3 / D3. `first` EUR bought at 1.0000; a limit buy of 1 M at 0.9990 triggers at 0.9985.
fn d3_world(first: u64) -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, first, t)).unwrap();
    w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_990), 0, 0, 0, t)).unwrap(); // accepted at arrival: margin is not reserved
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 20, p4(9_985)); // the limit triggers
    w.ev.tick(t + 30);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn finding_3_a_pending_fill_refused_for_margin_expects_no_event_and_verifies() {
    // $4.9M: margin $98,000 of $100,000; the 1 M more needs $20,000
    let (w, bs) = d3_world(4_900 * MEUR / 1000);
    assert_eq!(w.ev.stats().refused_fills, 1);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(OrderReject::InsufficientMargin));
    assert!(!events_of(&bs).iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. })));
    assert_valid(&w, &bs);
}

#[test]
fn d3_the_same_order_with_enough_margin_fills_and_verifies() {
    // $3.0M: margin $60,000; after the fill $80,000 against an equity of about $95,500
    let (w, bs) = d3_world(3_000 * MEUR / 1000);
    assert_eq!(w.ev.stats().refused_fills, 0);
    assert!(events_of(&bs).iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. })));
    assert_valid(&w, &bs);
}

#[test]
fn d3_still_caught_an_order_with_enough_margin_that_has_no_fill() {
    let (w, bs) = d3_world(3_000 * MEUR / 1000);
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. })) else { return };
        evs.remove(k);
    });
    assert!(classes(&w, &bad).contains(&F::TriggerMissed));
}

#[test]
fn d3_still_caught_a_fill_the_account_could_not_margin() {
    // the history of the account with enough margin, judged under a rule set whose leverage is 30 instead of 50: the same
    // fill now needs $80,000 + ... against an equity that does not cover it
    let (w, bs) = d3_world(3_000 * MEUR / 1000);
    let mut f = files(&w, &bs);
    f.rules = f.rules.replace("\"leverage_x\":[50,", "\"leverage_x\":[30,");
    let rep = setl8_verify::verify_account(&setl8_verify::Inputs {
        events: f.events.as_bytes(),
        rules: f.rules.as_bytes(),
        archive: &f.archive,
        intents: &f.intents,
        snapshot: None,
        second_archive: None,
        as_of_seq: None,
    })
    .unwrap();
    assert!(rep.classes().contains(&F::PendingFillWithoutMargin), "{}", describe(&rep));
}

fn d3_later_fill_world() -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, 4_900 * MEUR / 1000, t)).unwrap();
    w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_990), 0, 0, 0, t)).unwrap();
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 20, p4(9_985)); // triggers, refused: stays pending
    w.sample(0, t + 30, p4(10_000));
    w.ev.tick(t + 31);
    w.order(close_order(0, 0, t + 32)).unwrap(); // the trader closes the big position: margin is free again
    w.sample(0, t + 40, p4(10_000));
    w.sample(0, t + 80, p4(9_985)); // triggers again: now there is margin
    w.ev.tick(t + 90);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn d3_an_order_refused_for_margin_fills_at_a_later_trigger_once_the_margin_is_there() {
    let (w, bs) = d3_later_fill_world();
    assert_eq!(w.ev.stats().refused_fills, 1);
    assert_eq!(w.ev.stats().limit_fills, 1);
    let fill_time = events_of(&bs).iter().find_map(|e| if let JournalEvent::Fill { order_kind: OrderKind::Limit, time, .. } = e { Some(*time) } else { None });
    assert_eq!(fill_time, Some(T_1400 + 80));
    assert_valid(&w, &bs);
}

// ------------------------------------------------------------------------------------------------- D4

/// Finding 4 / D4. A breach closes every position at the sample; a profitable QUICK slice is credited only half, so the
/// balance after the closes can be at or below the static floor although the equity that triggered the breach was above it.
fn d4_static_world() -> (World, Vec<Batch>) {
    let mk = |i: u16, s: &str| {
        let mut m = common::market(i, 0, s, 0);
        m.max_age_secs = 3_600;
        m
    };
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
    (w, bs)
}

#[test]
fn finding_4_a_breach_whose_closes_end_below_the_static_floor_is_a_fail_and_verifies() {
    let (w, bs) = d4_static_world();
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::StaticFloor)), "stats {:?}", w.ev.stats());
    let evs = events_of(&bs);
    let b_after = evs.iter().find_map(|e| if let JournalEvent::Fail { b_after, .. } = e { Some(*b_after) } else { None }).unwrap();
    assert_eq!(b_after, 92_500_000_000 + 750_000_000 - 1_500_000_000, "A's quick profit is halved, B's loss is in full");
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })));
    assert_valid(&w, &bs);
}

/// Long 1 M EUR for 100 s, +20 pips: tight mode (G = 2,000, L = 96,000 + 1,000 = 97,000); down to -35 pips: E = 96,500 <= L, the
/// breach; the closes (a loss, in full) leave B = 96,500, above the static floor 92,000.
fn d4_plain_breach_world() -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 100, p4(10_020));
    w.sample(0, t + 110, p4(9_965));
    w.ev.tick(t + 120);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn d4_a_plain_breach_stays_a_breach_and_verifies() {
    let (w, bs) = d4_plain_breach_world();
    let evs = events_of(&bs);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })), "{evs:?}");
    assert_valid(&w, &bs);
}

#[test]
fn d4_still_caught_a_breach_posted_where_the_closes_end_at_or_below_the_static_floor() {
    let (w, bs) = d4_static_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fail { .. })) else { return };
        let JournalEvent::Fail { time, b_after, proof, .. } = evs[k] else { unreachable!() };
        evs[k] = JournalEvent::Breach { time, l_level: 93_000_000_000, b_after, proof };
    });
    let c = classes(&w, &bad);
    assert!(c.contains(&F::FalseBreach) && c.contains(&F::FloorMismatch), "{c:?}");
}

#[test]
fn d4_still_caught_a_static_floor_fail_where_the_closes_end_above_it() {
    let (w, bs) = d4_plain_breach_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Breach { .. })) else { return };
        let JournalEvent::Breach { time, b_after, proof, .. } = evs[k] else { unreachable!() };
        evs[k] = JournalEvent::Fail { time, reason: FailReason::StaticFloor, b_after, proof };
    });
    assert!(classes(&w, &bad).contains(&F::FalseFail));
}

#[test]
fn d4_still_caught_an_omitted_breach_or_fail() {
    let (w, bs) = d4_plain_breach_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Breach { .. })) else { return };
        evs.remove(k);
    });
    assert!(classes(&w, &bad).contains(&F::MissedBreach));
    // where the closes end at or below the static floor the omitted event is the Fail
    let (w, bs) = d4_static_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fail { .. })) else { return };
        evs.remove(k);
    });
    assert!(classes(&w, &bad).contains(&F::MissedFail));
}

/// Finding 4, second form / D4: the forced closes of a breach push the quick tally over its limit. Nothing but the `Fail`
/// may follow an overshoot, so the breach becomes `Fail(PhaseQuick)`.
fn d4_quick_world() -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 20, p4(11_070)); // +10,700: tight mode, G = 10,700, L = 96,000 + 9,700 = 105,700
    w.sample(0, t + 40, p4(10_570)); // +5,700: E = 105,700 <= L: breach; the quick slice credits 2,850 > 2,800
    w.ev.tick(t + 45);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn finding_4_a_breach_whose_closes_overshoot_the_quick_tally_is_a_fail_and_verifies() {
    let (w, bs) = d4_quick_world();
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)), "{:?}", w.ev.stats());
    let evs = events_of(&bs);
    let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })) else { return };
    assert!(matches!(evs[k - 1], JournalEvent::Close { reason: CloseReason::Breach, quick: true, .. }), "{:?}", evs[k - 1]);
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. })));
    assert_valid(&w, &bs);
}

#[test]
fn d4_still_caught_a_breach_posted_where_the_closes_overshoot_the_quick_limit() {
    let (w, bs) = d4_quick_world();
    let bad = edited(&bs, |evs| {
        let Some(k) = evs.iter().position(|e| matches!(e, JournalEvent::Fail { .. })) else { return };
        let JournalEvent::Fail { time, b_after, proof, .. } = evs[k] else { unreachable!() };
        evs[k] = JournalEvent::Breach { time, l_level: 105_700_000_000, b_after, proof };
    });
    let c = classes(&w, &bad);
    assert!(c.contains(&F::FalseBreach), "{c:?}");
}

#[allow(dead_code)]
fn unused(_: Side) {}

/// A pending order larger than the position it meets: `Close(Flip)` under its nonce and a `Fill(Limit)` for the remainder.
fn d1_pending_flip_world() -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR / 2, t)).unwrap();
    w.order(order(IntentKind::Limit, 0, 1, MEUR, p4(10_050), 0, 0, 0, t)).unwrap(); // sell 1 M at 1.0050 or better
    w.sample(0, t + 10, p4(10_000));
    w.sample(0, t + 120, p4(10_060));
    w.ev.tick(t + 130);
    let bs = drain(&mut w);
    (w, bs)
}

/// The same, with a closing slice that overshoots the quick limit: `Close(Flip)` under the order's nonce, then the `Fail`.
fn d2_pending_flip_overshoot_world() -> (World, Vec<Batch>) {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, 2 * MEUR, t)).unwrap();
    w.order(order(IntentKind::Limit, 0, 1, 3 * MEUR, p4(10_020), 0, 0, 0, t)).unwrap(); // sell 3 M at 1.0020 or better
    w.sample(0, t + 5, p4(10_000));
    w.sample(0, t + 20, p4(10_030)); // +6,000 on the position: credit 3,000 > 2,800
    w.ev.tick(t + 30);
    let bs = drain(&mut w);
    (w, bs)
}

#[test]
fn a_pending_flip_is_a_flip_close_and_a_fill_and_verifies() {
    let (w, bs) = d1_pending_flip_world();
    let evs = events_of(&bs);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Flip, intent_id: 2, .. })));
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, intent_id: 2, .. })));
    assert_valid(&w, &bs);
}

#[test]
fn a_pending_flip_whose_closing_slice_overshoots_ends_at_the_fail_and_verifies() {
    let (w, bs) = d2_pending_flip_overshoot_world();
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let evs = events_of(&bs);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Flip, intent_id: 2, .. })));
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. })));
    assert_valid(&w, &bs);
}

// ------------------------------------------------------------------------------------------------- vectors for the verifier crate

/// The honest histories above as files, for the verifier's own tests (`crates/verifier/tests/m9b.rs` edits them into the
/// dishonest variants). `SL8_WRITE_VECTORS=1 cargo test -p setl8-lev-evaluator --test findings` rewrites them.
fn m9b_vectors() -> Vec<(&'static str, World, Vec<Batch>)> {
    let mut v = Vec::new();
    let (w, bs, _) = d1_world();
    v.push(("d1_pending_reduce", w, bs));
    let (w, bs, _) = d1_untriggered_world();
    v.push(("d1_untriggered", w, bs));
    let (w, bs) = d2_overshoot_world();
    v.push(("d2_quick_overshoot", w, bs));
    let (w, bs) = d2_plain_quick_world();
    v.push(("d2_plain_quick", w, bs));
    let (w, bs) = d2_flip_world(30);
    v.push(("d2_flip_overshoot", w, bs));
    let (w, bs) = d2_flip_world(20);
    v.push(("d2_flip_plain", w, bs));
    let (w, bs) = d1_pending_flip_world();
    v.push(("d1_pending_flip", w, bs));
    let (w, bs) = d2_pending_flip_overshoot_world();
    v.push(("d2_pending_flip_overshoot", w, bs));
    let (w, bs) = d3_world(4_900 * MEUR / 1000);
    v.push(("d3_margin_refused", w, bs));
    let (w, bs) = d3_world(3_000 * MEUR / 1000);
    v.push(("d3_margin_ok", w, bs));
    let (w, bs) = d3_later_fill_world();
    v.push(("d3_refused_then_filled", w, bs));
    let (w, bs) = d4_static_world();
    v.push(("d4_static_floor", w, bs));
    let (w, bs) = d4_plain_breach_world();
    v.push(("d4_plain_breach", w, bs));
    let (w, bs) = d4_quick_world();
    v.push(("d4_quick_limit", w, bs));
    v
}

#[test]
fn the_m9b_vectors_are_regenerated_byte_for_byte() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../verifier/tests/vectors/m9b");
    let write = std::env::var("SL8_WRITE_VECTORS").is_ok();
    for (name, w, bs) in m9b_vectors() {
        assert_valid(&w, &bs);
        let f = files(&w, &bs);
        for (file, bytes) in [("events.jsonl", f.events.as_bytes()), ("rules.json", f.rules.as_bytes()), ("archive.bin", &f.archive[..]), ("intents.jsonl", &f.intents[..]), ("snapshot.json", f.snapshot.as_bytes())] {
            let path = root.join(name).join(file);
            if write {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, bytes).unwrap();
            } else {
                let on_disk = std::fs::read(&path).unwrap_or_else(|_| panic!("{} is missing (SL8_WRITE_VECTORS=1 writes it)", path.display()));
                assert!(on_disk == bytes, "{} differs from what the evaluator produces", path.display());
            }
        }
    }
}
