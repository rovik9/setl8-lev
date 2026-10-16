//! M9b (handover/design-decisions.md 53, decisions D1 to D5): the honest histories of the five findings M10 reported verify,
//! and the nearest dishonest variants are still caught. The vectors in `vectors/m9b/` are the output of the REAL reference
//! evaluator (`crates/evaluator/tests/findings.rs` regenerates them byte for byte); each test edits the event list of one of
//! them (an event removed, changed or inserted, the chain recomputed) and says which finding class must appear.
mod common;

use std::path::{Path, PathBuf};

use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent as E, OrderKind};
use setl8_verify::log::{self, Line, OpenedRec};
use setl8_verify::{verify_account, FindingClass as C, Inputs, Report, Verdict};

#[derive(Clone)]
struct V {
    opened: OpenedRec,
    /// The events in file order with the proof bytes each one carries.
    events: Vec<(E, Vec<u8>)>,
    rules: Vec<u8>,
    archive: Vec<u8>,
    intents: Vec<u8>,
    snapshot: Vec<u8>,
}

fn dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/m9b").join(name)
}

fn load(name: &str) -> V {
    let read = |f: &str| std::fs::read(dir(name).join(f)).unwrap_or_else(|_| panic!("vector {name}/{f} is missing"));
    let text = String::from_utf8(read("events.jsonl")).unwrap();
    let mut opened = None;
    let mut events = Vec::new();
    for l in log::parse(&text).unwrap() {
        match l.line {
            Line::Opened(o) => opened = Some(o),
            Line::Event { event, proof, .. } => events.push((E::decode(&event).unwrap(), proof)),
            _ => {}
        }
    }
    V { opened: opened.unwrap(), events, rules: read("rules.json"), archive: read("archive.bin"), intents: read("intents.jsonl"), snapshot: read("snapshot.json") }
}

impl V {
    fn text(&self) -> String {
        let mut t = log::header_line();
        t.push_str(&log::opened_line(&self.opened));
        for (i, (e, p)) in self.events.iter().enumerate() {
            t.push_str(&log::event_line(&self.opened.account, i as u64 + 1, &e.to_bytes(), p, 1000 + i as u64));
        }
        t
    }

    fn run(&self, snapshot: bool) -> Report {
        let events = self.text();
        verify_account(&Inputs {
            events: events.as_bytes(),
            rules: &self.rules,
            archive: &self.archive,
            intents: &self.intents,
            snapshot: if snapshot { Some(&self.snapshot) } else { None },
            second_archive: None,
            as_of_seq: None,
        })
        .unwrap()
    }

    /// The report of the (possibly edited) history; no snapshot, the stored chain head no longer matches an edit.
    fn classes(&self) -> Vec<C> {
        self.run(false).classes()
    }

    fn edit(&self, f: impl FnOnce(&mut Vec<(E, Vec<u8>)>)) -> V {
        let mut v = self.clone();
        f(&mut v.events);
        v
    }

    fn pos(&self, f: impl Fn(&E) -> bool) -> usize {
        self.events.iter().position(|(e, _)| f(e)).expect("event")
    }
}

fn valid(name: &str) -> V {
    let v = load(name);
    let r = v.run(true);
    assert_eq!(r.verdict, Verdict::Valid, "{name}: {:#?}", r.findings);
    v
}

fn is_close(e: &E) -> bool {
    matches!(e, E::Close { .. })
}

fn is_limit_fill(e: &E) -> bool {
    matches!(e, E::Fill { order_kind: OrderKind::Limit, .. })
}

fn is_fail(e: &E) -> bool {
    matches!(e, E::Fail { .. })
}

// ----------------------------------------------------------------------------------------------------------- D1

#[test]
fn d1_a_pending_order_that_only_reduces_is_executed_by_its_close() {
    let v = valid("d1_pending_reduce");
    let k = v.pos(is_close);
    assert!(matches!(v.events[k].0, E::Close { reason: CloseReason::Manual, intent_id: 2, qty: 250_000_000_000, .. }));
    assert!(!v.events.iter().any(|(e, _)| is_limit_fill(e)));
}

#[test]
fn d1_a_pending_flip_is_a_close_and_a_fill() {
    let v = valid("d1_pending_flip");
    let k = v.pos(is_close);
    assert!(matches!(v.events[k].0, E::Close { reason: CloseReason::Flip, intent_id: 2, .. }));
    // the fill is the other half of the same execution, not a second one; a second Fill of the order is not
    let twice = v.edit(|evs| {
        let f = evs.iter().position(|(e, _)| is_limit_fill(e)).unwrap();
        let again = evs[f].clone();
        evs.insert(f + 1, again);
    });
    assert!(twice.classes().contains(&C::TriggerFalse));
}

#[test]
fn d1_still_caught_the_trigger_passes_and_nothing_executes_the_order() {
    let v = valid("d1_pending_reduce");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::TriggerMissed));
}

#[test]
fn d1_still_caught_a_second_execution_of_the_same_order() {
    let v = valid("d1_pending_reduce");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        let again = evs[k].clone();
        evs.insert(k + 1, again);
    });
    assert!(bad.classes().contains(&C::TriggerFalse));
}

#[test]
fn d1_still_caught_a_close_under_the_order_with_the_wrong_quantity_or_price() {
    let v = valid("d1_pending_reduce");
    let qty = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        if let E::Close { qty, .. } = &mut evs[k].0 {
            *qty += 1_000;
        }
    });
    let c = qty.classes();
    assert!(c.contains(&C::PnlMismatch) && c.contains(&C::ForgedTrade), "{c:?}");
    let price = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        if let E::Close { exit_e12, .. } = &mut evs[k].0 {
            *exit_e12 += 1_000_000_000;
        }
    });
    let c = price.classes();
    assert!(c.contains(&C::PriceMismatch) && c.contains(&C::PnlMismatch), "{c:?}");
}

#[test]
fn d1_still_caught_a_close_under_an_intent_that_is_no_order_of_the_trader() {
    // nonces of d1_untriggered: 1 market buy, 2 sell limit that is never reached, 3 set_sl_tp, 4 the trader's close
    let v = valid("d1_untriggered");
    let with_intent = |id: u32| {
        v.edit(|evs| {
            let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
            if let E::Close { intent_id, .. } = &mut evs[k].0 {
                *intent_id = id;
            }
        })
    };
    assert!(matches!(v.events[v.pos(is_close)].0, E::Close { intent_id: 4, .. }));
    assert!(with_intent(999).classes().contains(&C::ForgedTrade), "an intent nobody signed");
    assert!(with_intent(3).classes().contains(&C::ForgedTrade), "a set_sl_tp is not an order");
    assert!(with_intent(2).classes().contains(&C::TriggerFalse), "a pending order no sample triggers");
}

// ----------------------------------------------------------------------------------------------------------- D2

fn commission_fee_after(evs: &[(E, Vec<u8>)], k: usize) -> E {
    let E::Close { market, qty, exit_e12, time, .. } = evs[k].0 else { panic!("a Close") };
    let amount = engine::fee_ceil_micro(engine::margin_micro(qty, exit_e12, 1).unwrap(), 20).unwrap();
    E::Fee { kind: FeeKind::Commission, market, amount: u64::try_from(amount).unwrap(), time }
}

#[test]
fn d2_the_slice_that_overshoots_the_quick_limit_carries_no_commission_fee() {
    let v = valid("d2_quick_overshoot");
    let k = v.pos(is_close);
    assert!(matches!(v.events[k + 1].0, E::Fail { reason: FailReason::PhaseQuick, .. }), "the Fail follows the Close directly");
}

#[test]
fn d2_still_caught_a_fee_posted_after_the_overshoot() {
    let v = valid("d2_quick_overshoot");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        let fee = commission_fee_after(evs, k);
        evs.insert(k + 1, (fee, Vec::new()));
    });
    assert!(bad.classes().contains(&C::FeeMismatch));
}

#[test]
fn d2_still_caught_a_fee_missing_after_a_close_that_does_not_overshoot() {
    let v = valid("d2_plain_quick");
    let k = v.pos(is_close);
    assert!(matches!(v.events[k + 1].0, E::Fee { kind: FeeKind::Commission, .. }));
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        evs.remove(k + 1);
    });
    assert!(bad.classes().contains(&C::FeeMismatch));
}

#[test]
fn d2_still_caught_a_wrong_fee_after_a_close_that_does_not_overshoot() {
    let v = valid("d2_plain_quick");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        if let E::Fee { amount, .. } = &mut evs[k + 1].0 {
            *amount += 1;
        }
    });
    assert!(bad.classes().contains(&C::FeeMismatch));
}

#[test]
fn d2_still_caught_a_fail_after_a_slice_that_did_not_overshoot() {
    let v = valid("d2_plain_quick");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_close(e)).unwrap();
        let (E::Close { time, proof, .. }, p) = evs[k].clone() else { panic!() };
        evs.insert(k + 2, (E::Fail { time, reason: FailReason::PhaseQuick, b_after: 0, proof }, p));
    });
    assert!(bad.classes().contains(&C::FalseFail));
}

#[test]
fn d2_a_flip_whose_closing_slice_overshoots_ends_at_the_fail() {
    let v = valid("d2_flip_overshoot");
    assert!(!v.events.iter().any(|(e, _)| matches!(e, E::Fill { qty, .. } if *qty == 1_000_000_000_000)));
    let v = valid("d2_pending_flip_overshoot");
    let k = v.pos(is_close);
    assert!(matches!(v.events[k].0, E::Close { reason: CloseReason::Flip, intent_id: 2, .. }));
}

#[test]
fn d2_still_caught_a_flip_that_drops_its_remainder_without_an_overshoot() {
    let v = valid("d2_flip_plain");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| matches!(e, E::Fill { qty, .. } if *qty == 1_000_000_000_000)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::ForgedTrade));
}

#[test]
fn d2_a_pending_flip_that_overshoots_may_drop_its_remainder_but_a_flip_that_does_not_may_not() {
    // the pending flip of d1_pending_flip does not overshoot: dropping its Fill is a forged trade
    let v = valid("d1_pending_flip");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_limit_fill(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::ForgedTrade));
}

// ----------------------------------------------------------------------------------------------------------- D3

#[test]
fn d3_an_order_the_account_cannot_margin_expects_no_event() {
    let v = valid("d3_margin_refused");
    assert!(!v.events.iter().any(|(e, _)| is_limit_fill(e)));
}

#[test]
fn d3_the_same_order_with_enough_margin_fills() {
    let v = valid("d3_margin_ok");
    assert!(v.events.iter().any(|(e, _)| is_limit_fill(e)));
}

#[test]
fn d3_still_caught_an_order_with_enough_margin_that_has_no_fill() {
    let v = valid("d3_margin_ok");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_limit_fill(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::TriggerMissed));
}

#[test]
fn d3_still_caught_a_fill_of_an_order_the_account_could_not_margin() {
    // the Fill the evaluator did NOT post, posted: the limit order (nonce 2) at the second of its trigger (+20)
    let v = valid("d3_margin_refused");
    let ok = load("d3_margin_ok");
    let fill = ok.events.iter().find(|(e, _)| is_limit_fill(e)).unwrap().clone();
    let bad = v.edit(|evs| {
        let last = evs.len() - 1;
        evs.insert(last, fill.clone());
    });
    let c = bad.classes();
    assert!(c.contains(&C::PendingFillWithoutMargin), "{c:?}");
    // and the same Fill in the account that has the margin is not flagged for it
    assert!(!ok.classes().contains(&C::PendingFillWithoutMargin));
}

#[test]
fn d3_still_caught_the_rule_set_changing_the_margin_of_a_fill_that_fitted() {
    // the history of the account with enough margin, judged with leverage 30 instead of 50: the fill no longer fits
    let mut v = load("d3_margin_ok");
    v.rules = String::from_utf8(v.rules.clone()).unwrap().replace("\"leverage_x\":[50,", "\"leverage_x\":[30,").into_bytes();
    assert!(v.classes().contains(&C::PendingFillWithoutMargin));
}

#[test]
fn d3_an_order_refused_for_margin_fills_at_a_later_trigger() {
    let v = valid("d3_refused_then_filled");
    let k = v.pos(is_limit_fill);
    let E::Fill { time, .. } = v.events[k].0 else { panic!() };
    // the first trigger is at +20, the fill at +80
    assert_eq!(time, 1_791_813_600 + 80);
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_limit_fill(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::TriggerMissed));
}

// ----------------------------------------------------------------------------------------------------------- D4

#[test]
fn d4_a_breach_whose_closes_end_at_or_below_the_static_floor_is_a_fail() {
    let v = valid("d4_static_floor");
    assert!(!v.events.iter().any(|(e, _)| matches!(e, E::Breach { .. })));
    assert!(v.events.iter().any(|(e, _)| matches!(e, E::Fail { reason: FailReason::StaticFloor, .. })));
}

#[test]
fn d4_a_breach_whose_closes_overshoot_the_quick_limit_is_a_fail() {
    let v = valid("d4_quick_limit");
    assert!(!v.events.iter().any(|(e, _)| matches!(e, E::Breach { .. })));
    assert!(v.events.iter().any(|(e, _)| matches!(e, E::Fail { reason: FailReason::PhaseQuick, .. })));
}

#[test]
fn d4_a_plain_breach_stays_a_breach() {
    let v = valid("d4_plain_breach");
    assert!(v.events.iter().any(|(e, _)| matches!(e, E::Breach { .. })));
}

#[test]
fn d4_still_caught_a_breach_posted_where_the_closes_end_at_or_below_the_static_floor() {
    let v = valid("d4_static_floor");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_fail(e)).unwrap();
        let (E::Fail { time, b_after, proof, .. }, p) = evs[k].clone() else { panic!() };
        evs[k] = (E::Breach { time, l_level: 93_000_000_000, b_after, proof }, p);
    });
    let c = bad.classes();
    assert!(c.contains(&C::FalseBreach) && c.contains(&C::FloorMismatch), "{c:?}");
}

#[test]
fn d4_still_caught_a_breach_posted_where_the_closes_overshoot_the_quick_limit() {
    let v = valid("d4_quick_limit");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_fail(e)).unwrap();
        let (E::Fail { time, b_after, proof, .. }, p) = evs[k].clone() else { panic!() };
        evs[k] = (E::Breach { time, l_level: 105_700_000_000, b_after, proof }, p);
    });
    assert!(bad.classes().contains(&C::FalseBreach));
}

#[test]
fn d4_still_caught_a_static_floor_fail_where_the_closes_end_above_it() {
    let v = valid("d4_plain_breach");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| matches!(e, E::Breach { .. })).unwrap();
        let (E::Breach { time, b_after, proof, .. }, p) = evs[k].clone() else { panic!() };
        evs[k] = (E::Fail { time, reason: FailReason::StaticFloor, b_after, proof }, p);
    });
    assert!(bad.classes().contains(&C::FalseFail));
}

#[test]
fn d4_still_caught_an_omitted_breach_or_fail() {
    let v = valid("d4_plain_breach");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| matches!(e, E::Breach { .. })).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::MissedBreach));
    let v = valid("d4_static_floor");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_fail(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::MissedFail));
    let v = valid("d4_quick_limit");
    let bad = v.edit(|evs| {
        let k = evs.iter().position(|(e, _)| is_fail(e)).unwrap();
        evs.remove(k);
    });
    assert!(bad.classes().contains(&C::MissedFail));
}

// ----------------------------------------------------------------------------------------------------------- D5

use common::{pass_case, pass_case_in_phase, verify_no_snapshot, last_event_time, Bundle};

const PAY: u64 = 8_000_000_000;

fn finalized_phase_2(extra: &[&str]) -> Bundle {
    let mut b = pass_case(0);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, PAY, 108_000_000_000, seq, t);
    b.push_pass_confirmed(2, PAY, 108_000_000_000, seq, t + 172_800);
    b.push_raw(&format!("\"type\":\"pass_finalized\",\"phase\":2,\"to_phase\":3,\"amount\":{PAY},\"request_id\":1,\"tally_count\":1,\"tally_total\":{PAY},\"window_total\":{PAY},\"time\":{}", t + 172_800));
    for l in extra {
        b.push_raw(&l.replace("@T", &(t + 172_800).to_string()));
    }
    b
}

const ADVANCE_2_3: &str = "\"type\":\"phase_advanced\",\"from_phase\":2,\"to_phase\":3,\"time\":@T";

#[test]
fn d5_a_phase_advanced_right_after_its_pass_finalized_changes_nothing() {
    let only_finalized = verify_no_snapshot(&finalized_phase_2(&[]));
    assert_eq!(only_finalized.classes(), vec![], "{:#?}", only_finalized.findings);
    let both = verify_no_snapshot(&finalized_phase_2(&[ADVANCE_2_3]));
    assert_eq!(both.classes(), vec![], "{:#?}", both.findings);
    // the same account state: one phase change, not two
    assert_eq!((both.state.phase, both.state.b, both.per_phase.len()), (only_finalized.state.phase, only_finalized.state.b, only_finalized.per_phase.len()));
    assert_eq!(both.state.phase, 3);
}

#[test]
fn d5_still_caught_a_phase_advanced_to_another_phase() {
    let r = verify_no_snapshot(&finalized_phase_2(&["\"type\":\"phase_advanced\",\"from_phase\":2,\"to_phase\":4,\"time\":@T"]));
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
}

#[test]
fn d5_still_caught_a_duplicated_phase_advanced() {
    let r = verify_no_snapshot(&finalized_phase_2(&[ADVANCE_2_3, ADVANCE_2_3]));
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
}

#[test]
fn d5_still_caught_a_phase_advanced_that_does_not_directly_follow_the_pass_finalized() {
    let r = verify_no_snapshot(&finalized_phase_2(&["\"type\":\"pass_rejected\",\"phase\":3,\"reason\":1", ADVANCE_2_3]));
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
}

#[test]
fn d5_still_caught_a_phase_advanced_without_any_pass_line() {
    let mut b = pass_case(0);
    let t = last_event_time(&b);
    b.push_raw(&format!("\"type\":\"phase_advanced\",\"from_phase\":2,\"to_phase\":3,\"time\":{t}"));
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
}

#[test]
fn d5_the_phase_one_confirmation_is_a_pass_request_and_a_phase_advanced() {
    let mut b = pass_case_in_phase(0, 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(1, 0, 108_000_000_000, seq, t);
    b.push_raw(&format!("\"type\":\"phase_advanced\",\"from_phase\":1,\"to_phase\":2,\"time\":{}", t + 1));
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), vec![], "{:#?}", r.findings);
    assert_eq!(r.state.phase, 2);
    // a rejected request leaves nothing to advance
    let mut b = pass_case_in_phase(0, 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(1, 0, 108_000_000_000, seq, t);
    b.push_raw("\"type\":\"pass_rejected\",\"phase\":1,\"reason\":1");
    b.push_raw(&format!("\"type\":\"phase_advanced\",\"from_phase\":1,\"to_phase\":2,\"time\":{}", t + 1));
    assert!(verify_no_snapshot(&b).classes().contains(&C::StateMismatch));
    // a request of another phase does not either
    let mut b = pass_case_in_phase(0, 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(3, 0, 108_000_000_000, seq, t);
    b.push_raw(&format!("\"type\":\"phase_advanced\",\"from_phase\":1,\"to_phase\":2,\"time\":{}", t + 1));
    assert!(verify_no_snapshot(&b).classes().contains(&C::StateMismatch));
}
