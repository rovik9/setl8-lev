//! M9c (handover/design-decisions.md 59): the verifier recomputes what the evaluator's own rules refuse. A market order that the
//! account cannot margin is `FillWithoutMargin`; one the evaluator would also refuse for another reason that the verifier can
//! reproduce (lock, a market that is not enabled or closed, the step and minimum, the notional cap, the ninth position) is
//! `FillNotAllowed`; a triggered pending order refused for the same reasons is expected NOT to fill. These tests run the whole
//! pipeline (the hand-written generator's journal, the archive, the intents) with an honest history, the nearest dishonest one
//! and a boundary pair; the unit tests with the other boundaries are in `src/walk.rs` (`refusal_tests`).
//!
//! Numbers: S = $100,000, class 0 (50x), price 1.0, half-spread 0, no commission, so 1 EUR is 1e6 quantity units and a
//! quantity in units is also its notional in micro-USD; 5,000,000 EUR needs exactly $100,000 of margin.
mod common;

use common::*;
use journal_types::JournalEvent as E;
use setl8_verify::FindingClass as C;

const UNITS: u64 = 1_000_000;

fn flat_spec() -> Spec {
    let mut s = Spec::base();
    s.markets[0].spread_e6 = 0;
    // a price that stands still for 100 s must not close the market in the flip test
    s.markets[0].max_age_secs = 900;
    s
}

/// One sample at 14:00:00 and one market buy of `qty` units at that second.
fn buy(qty: u64) -> Bundle {
    Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, qty, T_1400) },
        Script::Heartbeat { t: T_1400 },
    ])
}

fn classes_of(b: &Bundle) -> Vec<C> {
    verify(b).classes()
}

/// The same journal judged under rules with one textual change.
fn with_rules(b: &Bundle, from: &str, to: &str) -> setl8_verify::Report {
    let mut f = b.files();
    let rules = String::from_utf8(f.rules.clone()).unwrap();
    assert!(rules.contains(from), "{from} not in the rules");
    f.rules = rules.replacen(from, to, 1).into_bytes();
    // a journal edited to be the honest variant no longer matches the generator's final state
    f.snapshot = None;
    setl8_verify::verify_account(&f.inputs()).unwrap()
}

#[test]
fn a_market_order_that_uses_exactly_all_the_margin_is_valid_and_one_step_more_is_not() {
    let ok = buy(5_000_000 * UNITS);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    assert_eq!(ok.final_state.b, 100_000_000_000);
    let over = buy(5_000_000 * UNITS + 1_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
    // the finding names the Fill (the first event of the journal)
    let r = verify(&over);
    assert_eq!((r.findings[0].seq, r.findings[0].event_index), (Some(1), Some(0)));
    // a short order is no problem
    assert_eq!(classes_of(&buy(10 * UNITS)), Vec::<C>::new());
}

#[test]
fn a_flip_that_uses_exactly_all_the_margin_is_valid_and_one_step_more_is_not() {
    let flip = |second_mid: u64, sell: u64| {
        Gen::new(flat_spec()).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, 4_000_000 * UNITS, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 100, mid: second_mid },
            // closes the 4 M long and opens a short of (sell - 4 M)
            Script::Order { t: T_1400 + 100, intent: market_order(0, false, sell, T_1400 + 100) },
            Script::Heartbeat { t: T_1400 + 100 },
        ])
    };
    // price unchanged: the closing slice makes nothing, the short of 5 M EUR uses exactly the $100,000
    let ok = flip(eur(10_000), 9_000_000 * UNITS);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    let over = flip(eur(10_000), 9_000_000 * UNITS + 1_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
    // the judgment comes with the Close{Flip}, once: no second finding for the Fill that is its other half
    assert_eq!(verify(&over).findings.len(), 1);
    // at 1.0001 the closed 4 M long has made 4e12 x 0.0001 = 400,000,000 micro-USD (the slice is not quick: 100 s), so B is
    // 100,400,000,000. The short's margin is ceil(r x 1.0001 / 50): r = 5,019,498,050,000 units needs 100,399,999,997,
    // the next step 100,400,000,017.
    let ok = flip(eur(10_001), 4_000_000 * UNITS + 5_019_498_050_000);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "the credit of the closing slice counts: {:#?}", verify(&ok).findings);
    let over = flip(eur(10_001), 4_000_000 * UNITS + 5_019_498_051_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
}

#[test]
fn the_notional_cap_of_the_market_is_enforced_and_the_cap_itself_is_allowed() {
    let at_cap = buy(5_000 * UNITS);
    assert_eq!(classes_of(&at_cap), Vec::<C>::new());
    // cap $5,000: 5,000 EUR is exactly the cap, 5,000.001 EUR is above it
    let r = with_rules(&at_cap, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000000000");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    let above = buy(5_000 * UNITS + 1_000);
    let r = with_rules(&above, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000000000");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    assert!(r.findings[0].expected.contains("notional cap"), "{:?}", r.findings[0]);
}

#[test]
fn a_market_that_is_not_enabled_takes_no_new_exposure_but_a_close_is_fine() {
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, 100 * UNITS, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 100, mid: eur(10_001) },
        Script::Order { t: T_1400 + 100, intent: order(setl8_verify::intents::IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 100) },
        Script::Heartbeat { t: T_1400 + 100 },
    ]);
    assert_eq!(classes_of(&b), Vec::<C>::new());
    for status in [2u8, 0] {
        let r = with_rules(&b, "\"status\":1", &format!("\"status\":{status}"));
        // only the opening Fill is refused; the Close is not
        assert_eq!(r.classes(), vec![C::FillNotAllowed], "status {status}: {:#?}", r.findings);
        assert_eq!(r.findings.len(), 1);
        assert!(matches!(E::decode(&hexutil_bytes(&b, r.findings[0].seq.unwrap())).unwrap(), E::Fill { .. }));
    }
}

/// The bytes of the event with sequence number `seq` of the bundle.
fn hexutil_bytes(b: &Bundle, seq: u64) -> Vec<u8> {
    b.lines
        .iter()
        .find_map(|l| match l {
            GLine::Event { seq: s, ev, .. } if *s == seq => Some(ev.to_bytes()),
            _ => None,
        })
        .expect("event")
}

#[test]
fn a_market_order_in_a_closed_market_is_refused_and_a_market_that_was_open_is_not() {
    let b = buy(10 * UNITS);
    assert_eq!(classes_of(&b), Vec::<C>::new());
    // the same journal under sessions that end at 00:10 on Monday: 14:00 is outside them
    let r = with_rules(&b, "\"sessions\":[[0,10080]]", "\"sessions\":[[0,10]]");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    assert!(r.findings[0].expected.contains("closed"), "{:?}", r.findings[0]);
}

#[test]
fn a_quantity_off_the_step_or_below_the_minimum_is_refused() {
    // the generator's rules: step 1,000, minimum 1,000 units
    assert_eq!(classes_of(&buy(1_000)), Vec::<C>::new());
    let r = with_rules(&buy(1_000), "\"qty_min\":1000", "\"qty_min\":2000");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    let r = with_rules(&buy(1_500), "\"qty_step\":1000", "\"qty_step\":500");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    let r = with_rules(&buy(1_500), "\"qty_step\":1000", "\"qty_step\":1000");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
}

#[test]
fn a_triggered_pending_order_the_cap_refuses_must_not_fill_and_a_fill_is_the_dishonest_variant() {
    // a limit buy of 10,000 EUR at 1.0 triggers when the ask falls to it; the generator fills it (it has no cap)
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_010) },
        Script::Order { t: T_1400, intent: order(setl8_verify::intents::IntentKind::Limit, 0, 0, 10_000 * UNITS, eur(10_000), 0, 0, 0, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_000) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_001) },
        Script::Heartbeat { t: T_1400 + 20 },
    ]);
    assert!(b.events().iter().any(|e| matches!(e, E::Fill { order_kind: journal_types::OrderKind::Limit, .. })), "the order fills");
    assert_eq!(classes_of(&b), Vec::<C>::new());
    // under a cap of $5,000 the evaluator refuses the order: its Fill is an execution that must not exist ...
    let r = with_rules(&b, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000000000");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    // ... and under a cap of exactly $10,000 it is fine
    let r = with_rules(&b, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":10000000000");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    // the honest evaluator that refused it leaves no Fill and the verifier does not ask for one
    let mut honest = b.clone();
    let fill = honest.find(|e| matches!(e, E::Fill { .. }));
    // remove the Fill and its commission-less batch bookkeeping: the Meta commit table follows
    honest.remove_lines(&[fill]);
    let metas: Vec<usize> = honest.lines.iter().enumerate().filter(|(_, l)| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).map(|(i, _)| i).collect();
    for i in metas {
        if let E::Meta { flat, .. } = honest.ev_mut(i) {
            *flat = true;
        }
    }
    let r = with_rules(&honest, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000000000");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    // the same journal without a Fill under the ordinary rules is the existing TriggerMissed
    let r = verify(&honest);
    assert!(r.classes().contains(&C::TriggerMissed), "{:#?}", r.findings);
}

// ---------------------------------------------------------------- the hold (account_held / hold_released lines)

/// Inserts a raw log line at position `at` and moves the Meta commit table with it.
fn insert_line(b: &mut Bundle, at: usize, raw: String) {
    b.lines.insert(at, GLine::Raw(raw.trim_end().to_string()));
    for m in b.metas.iter_mut() {
        if m.line >= at {
            m.line += 1;
        }
    }
    b.renumber();
}

fn hold(t: i64, held: bool) -> String {
    setl8_verify::log::hold_line(&account_key(), t, held)
}

#[test]
fn a_market_fill_on_a_held_account_is_refused_from_the_second_of_the_hold_until_the_release() {
    // the honest evaluator refused this order: the journal has no Fill. The dishonest one executes it during the hold.
    let honest = buy(10 * UNITS);
    assert_eq!(classes_of(&honest), Vec::<C>::new());
    for (from, to, held) in [(T_1400, None, true), (T_1400 - 5, None, true), (T_1400 + 1, None, false), (T_1400 - 5, Some(T_1400), false), (T_1400 - 5, Some(T_1400 + 1), true)] {
        let mut b = honest.clone();
        // the hold lines come before the first event (line 0 is the opened line)
        if let Some(t) = to {
            insert_line(&mut b, 1, hold(t, false));
        }
        insert_line(&mut b, 1, hold(from, true));
        let r = verify_no_snapshot(&b);
        let want = if held { vec![C::FillNotAllowed] } else { Vec::<C>::new() };
        assert_eq!(r.classes(), want, "held from {from} to {to:?}: {:#?}", r.findings);
        if held {
            assert!(r.findings[0].expected.contains("held account"), "{:?}", r.findings[0]);
        }
    }
}

#[test]
fn a_triggered_pending_order_on_a_held_account_is_expected_not_to_fill() {
    // a limit buy that triggers at T+10 and T+30 (the ask is at its level then, above it at T+20); the generator fills it at
    // T+10 (it has no hold); the evaluator that held the account refused it and left it pending
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_010) },
        Script::Order { t: T_1400, intent: order(setl8_verify::intents::IntentKind::Limit, 0, 0, 100 * UNITS, eur(10_000), 0, 0, 0, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_000) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_001) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_000) },
        Script::Sample { market: 0, t: T_1400 + 40, mid: eur(10_001) },
        Script::Heartbeat { t: T_1400 + 40 },
    ]);
    assert_eq!(classes_of(&b), Vec::<C>::new());
    let fill = b.find(|e| matches!(e, E::Fill { .. }));
    // dishonest: the Fill exists although the log says the account was held at that second
    let mut held_fill = b.clone();
    insert_line(&mut held_fill, 1, hold(T_1400 + 5, true));
    let r = verify_no_snapshot(&held_fill);
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    // the honest journal: no Fill at all (flat Metas)
    let mut honest = b.clone();
    honest.remove_lines(&[fill]);
    for i in 0..honest.lines.len() {
        if let GLine::Event { ev: E::Meta { .. }, .. } = &honest.lines[i] {
            if let E::Meta { flat, .. } = honest.ev_mut(i) {
                *flat = true;
            }
        }
    }
    let missed = |b: &Bundle| -> Vec<String> { verify_no_snapshot(b).findings.iter().filter(|f| f.class == C::TriggerMissed).map(|f| f.expected.clone()).collect() };
    let at = |sec: i64| format!("second {sec}");
    // without the hold lines the order is a miss (the first one; later misses of the same state are not repeated) ...
    let m = missed(&honest);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains(&at(T_1400 + 10)), "{m:?}");
    // ... held from T+5 on, neither trigger is ...
    let mut held = honest.clone();
    insert_line(&mut held, 1, hold(T_1400 + 5, true));
    assert_eq!(verify_no_snapshot(&held).classes(), Vec::<C>::new());
    // ... held from T+5 to T+15 (released), only the trigger at T+30 is a miss ...
    let mut released = honest.clone();
    insert_line(&mut released, 1, hold(T_1400 + 15, false));
    insert_line(&mut released, 1, hold(T_1400 + 5, true));
    let m = missed(&released);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains(&at(T_1400 + 30)), "{m:?}");
    // ... and the boundary: a hold that starts at the second of the trigger holds it, one that starts a second later does not
    let mut from_10 = honest.clone();
    insert_line(&mut from_10, 1, hold(T_1400 + 10, true));
    assert_eq!(missed(&from_10), Vec::<String>::new());
    let mut from_11 = honest.clone();
    insert_line(&mut from_11, 1, hold(T_1400 + 11, true));
    let m = missed(&from_11);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains(&at(T_1400 + 10)), "{m:?}");
    // and a release at the second of a trigger lets it fill (the hold ends BEFORE that second)
    let mut released_at_30 = honest.clone();
    insert_line(&mut released_at_30, 1, hold(T_1400 + 30, false));
    insert_line(&mut released_at_30, 1, hold(T_1400 + 5, true));
    let m = missed(&released_at_30);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains(&at(T_1400 + 30)), "{m:?}");
}

#[test]
fn hold_lines_that_the_program_could_not_have_emitted_are_state_mismatches() {
    let b = buy(10 * UNITS);
    // a second hold on a held account
    let mut twice = b.clone();
    insert_line(&mut twice, 1, hold(T_1400 - 4, true));
    insert_line(&mut twice, 1, hold(T_1400 - 5, true));
    let r = verify_no_snapshot(&twice);
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
    // a release of an account that is not held
    let mut loose = b.clone();
    insert_line(&mut loose, 1, hold(T_1400 - 5, false));
    let r = verify_no_snapshot(&loose);
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
    // hold, release, hold again: the lines are fine (file order: held at T-5, released at T-4, held at T-3), and the account is
    // held at the second of the buy, which is the only finding
    let mut again = b.clone();
    insert_line(&mut again, 1, hold(T_1400 - 3, true));
    insert_line(&mut again, 1, hold(T_1400 - 4, false));
    insert_line(&mut again, 1, hold(T_1400 - 5, true));
    assert_eq!(verify_no_snapshot(&again).classes(), vec![C::FillNotAllowed]);
}

#[test]
fn the_side_of_a_fill_decides_whether_it_adds_or_reduces_and_a_short_is_judged_like_a_long() {
    // long 4 M EUR, then a market BUY of 1 M (exactly the rest of the margin) is fine, 1 M + one step is not
    let add = |more: u64| {
        Gen::new(flat_spec()).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, 4_000_000 * UNITS, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_000) },
            Script::Order { t: T_1400 + 10, intent: market_order(0, true, more, T_1400 + 10) },
            Script::Heartbeat { t: T_1400 + 10 },
        ])
    };
    assert_eq!(classes_of(&add(1_000_000 * UNITS)), Vec::<C>::new());
    assert_eq!(classes_of(&add(1_000_000 * UNITS + 1_000)), vec![C::FillWithoutMargin]);
    // a short of exactly the whole margin, and one step more
    let short = |qty: u64| {
        Gen::new(flat_spec()).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, false, qty, T_1400) },
            Script::Heartbeat { t: T_1400 },
        ])
    };
    assert_eq!(classes_of(&short(5_000_000 * UNITS)), Vec::<C>::new());
    assert_eq!(classes_of(&short(5_000_000 * UNITS + 1_000)), vec![C::FillWithoutMargin]);
}

#[test]
fn a_flip_is_judged_before_its_closing_slice_so_the_closing_commission_is_not_yet_paid() {
    // 0.01% commission. Long 4 M EUR at 1.0 (fee $400, B = 99,600), then a market sell of 4 M + N EUR at 1.0. The evaluator checks
    // the whole order BEFORE it books the closing slice and its fee: 99,600 - 2 x 0.0001 N - N / 50 >= 0, N up to 4,930,693 EUR.
    // After the closing fee B would be 99,200 and N would have to stay under 4,910,891: the verifier must use the first.
    let mut spec = flat_spec();
    spec.commission[0] = 100;
    let flip = |n: u64| {
        Gen::new(spec.clone()).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, 4_000_000 * UNITS, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 100, mid: eur(10_000) },
            Script::Order { t: T_1400 + 100, intent: market_order(0, false, (4_000_000 + n) * UNITS, T_1400 + 100) },
            Script::Heartbeat { t: T_1400 + 100 },
        ])
    };
    let ok = flip(4_920_000);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    let over = flip(4_931_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
}

#[test]
fn a_market_close_in_a_closed_market_is_refused_but_the_fill_before_it_is_not() {
    // sessions [0, 841) minutes of the week: 14:00:00 on Monday is minute 840 (open), 14:01:40 is minute 841 (closed)
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, 10 * UNITS, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 100, mid: eur(10_001) },
        Script::Order { t: T_1400 + 100, intent: order(setl8_verify::intents::IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 100) },
        Script::Heartbeat { t: T_1400 + 100 },
    ]);
    assert_eq!(classes_of(&b), Vec::<C>::new());
    let r = with_rules(&b, "\"sessions\":[[0,10080]]", "\"sessions\":[[0,841]]");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    assert_eq!(r.findings.len(), 1);
    assert!(r.findings[0].found.contains("market close"), "{:?}", r.findings[0]);
}

#[test]
fn a_pending_order_that_only_reduces_is_not_judged_as_a_market_order() {
    // a limit sell of 500 EUR + 500 units against a long of 1,000 EUR: the quantity of a pending order is validated when it is
    // placed, and it only reduces, so nothing is judged about it here even though 500.5 EUR is off the 1,000-unit step
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, 1_000 * UNITS, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_010) },
        Script::Order { t: T_1400 + 10, intent: order(setl8_verify::intents::IntentKind::Limit, 0, 1, 500 * UNITS + 500, eur(10_010), 0, 0, 0, T_1400 + 10) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_011) },
        Script::Heartbeat { t: T_1400 + 20 },
    ]);
    assert!(b.events().iter().any(|e| matches!(e, E::Close { reason: journal_types::CloseReason::Manual, intent_id: 2, .. })), "the limit order closes part of the position");
    assert_eq!(classes_of(&b), Vec::<C>::new(), "{:#?}", verify(&b).findings);
}
