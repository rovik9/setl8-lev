//! M9c (handover/design-decisions.md 59): the verifier recomputes what the evaluator's own rules refuse. A market order that the
//! account cannot margin is `FillWithoutMargin`; one the evaluator would also refuse for another reason that the verifier can
//! reproduce (lock, a market that is not enabled or closed, the step and minimum, the notional cap, the ninth position) is
//! `FillNotAllowed`; a triggered pending order refused for the same reasons is expected NOT to fill. These tests run the whole
//! pipeline (the hand-written generator's journal, the archive, the intents) with an honest history, the nearest dishonest one
//! and a boundary pair; the unit tests with the other boundaries are in `src/walk.rs` (`refusal_tests`).
//!
//! Numbers: S = $100,000, class 0 (50x), mid 1.0, half-spread h = `TEST_HALF_SPREAD_E6` = 1 (decision 64: a market with 0 is
//! unpriced and takes no new exposure), no commission, so 1 EUR is 1e6 quantity units. The ask is ceil(mid x (1 + h)) =
//! 1_000_001_000_000 and the bid floor(mid x (1 - h)) = 999_999_000_000 (price e12); a long fills at the ask, a short at the
//! bid, a close takes the other side, and an open position is marked at the bid (long) or the ask (short). The margin is
//! `margin_micro(qty, fill price, 50)` and the notional cap is compared on qty x fill price. 5,000,000 EUR at the ask is
//! $5,000,005 of notional and $100,000.1 of margin, plus $10 of open loss at the bid: no whole-dollar account holds exactly
//! that (one 1,000-unit step is 20 micro-USD), so the tests that pin equity == margin scale the account and the quantity by
//! ten (50,000,000 EUR with a size of $1,000,101) as in the evaluator's `exec_edges` tests.
mod common;

use common::*;
use journal_types::JournalEvent as E;
use setl8_verify::FindingClass as C;

const UNITS: u64 = 1_000_000;
/// The half-spread of the test market (decision 64: 0 would make it unpriced and every opening Fill a `FillNotAllowed`).
const TEST_HALF_SPREAD_E6: u16 = 1;
/// The ask of mid 1.0 at `TEST_HALF_SPREAD_E6` (price e12); the bid is 999_999_000_000.
const ASK: u64 = 1_000_001_000_000;

fn flat_spec() -> Spec {
    let mut s = Spec::base();
    s.markets[0].spread_e6 = TEST_HALF_SPREAD_E6;
    // a price that stands still for 100 s must not close the market in the flip test
    s.markets[0].max_age_secs = 900;
    s
}

/// `flat_spec` with a size of `usd` whole dollars (the tier size and the opening balance).
fn sized(usd: i64) -> Spec {
    let mut s = flat_spec();
    s.size = usd * USD;
    s
}

/// The sizes at which a position of 50,000,000 EUR (5e13 units, ten times the 5 M of the old numbers) lands exactly on
/// equity == margin; one step (1,000 units) more is then short. All at mid 1.0, ask 1_000_001_000_000, bid 999_999_000_000.
/// Long: margin = 5e13 x 1_000_001_000_000 / (1e12 x 50) = 1_000_001_000_000 micro-USD; marked at the bid the open loss is
/// 5e13 x 2_000_000 / 1e12 = 100_000_000, so equity == margin needs 1_000_101_000_000 = $1,000,101.
const FULL_LONG_USD: i64 = 1_000_101;
/// Short: margin = 5e13 x 999_999_000_000 / 5e13 = 999_999_000_000; marked at the ask the open loss is again 100_000_000, so
/// $1,000,099 (1_000_099_000_000).
const FULL_SHORT_USD: i64 = 1_000_099;
/// Flip of a 40 M EUR long (entry ask) into a 50 M EUR short (entry bid) at an unchanged price: the closing slice sells at the
/// bid, 4e13 x (999_999_000_000 - 1_000_001_000_000) / 1e12 = -80_000_000 (a loss is credited whole), so
/// equity == margin needs 999_999_000_000 + 100_000_000 + 80_000_000 = 1_000_179_000_000 = $1,000,179.
const FULL_FLIP_USD: i64 = 1_000_179;

/// One sample at 14:00:00 and one market buy of `qty` units at that second.
fn buy(qty: u64) -> Bundle {
    buy_in(flat_spec(), qty)
}

fn buy_in(spec: Spec, qty: u64) -> Bundle {
    Gen::new(spec).run(&[
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
    // 50 M EUR at the ask: margin 1_000_001_000_000, open loss at the bid 100_000_000, balance $1,000,101 (see FULL_LONG_USD)
    let ok = buy_in(sized(FULL_LONG_USD), 50_000_000 * UNITS);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    assert_eq!(ok.final_state.b, 1_000_101_000_000);
    // one step more: margin ceil(1_000_001_000_000 + 1_000 x 1_000_001_000_000 / 5e13 = 20.00002) = 1_000_001_000_021, open loss
    // floor(-100_000_000.002) = -100_000_001, equity 1_000_000_999_999 < margin
    let over = buy_in(sized(FULL_LONG_USD), 50_000_000 * UNITS + 1_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
    // the finding names the Fill (the first event of the journal)
    let r = verify(&over);
    assert_eq!((r.findings[0].seq, r.findings[0].event_index), (Some(1), Some(0)));
    // a short order is no problem
    assert_eq!(classes_of(&buy(10 * UNITS)), Vec::<C>::new());
}

#[test]
fn a_flip_that_uses_exactly_all_the_margin_is_valid_and_one_step_more_is_not() {
    let flip = |spec: Spec, long: u64, second_mid: u64, sell: u64| {
        Gen::new(spec).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, long, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 100, mid: second_mid },
            // closes the long and opens a short of (sell - long)
            Script::Order { t: T_1400 + 100, intent: market_order(0, false, sell, T_1400 + 100) },
            Script::Heartbeat { t: T_1400 + 100 },
        ])
    };
    // price unchanged: the 40 M long bought at the ask is closed at the bid for -80_000_000 (not quick: 100 s, so a loss is
    // credited whole) and the short of 50 M EUR opened at the bid uses exactly the balance (see FULL_FLIP_USD)
    let ok = flip(sized(FULL_FLIP_USD), 40_000_000 * UNITS, eur(10_000), 90_000_000 * UNITS);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    // one step more: margin 999_999_000_000 + ceil(1_000 x 999_999_000_000 / 5e13 = 19.99998) = +20, open loss +1 (floor of
    // -100_000_000.002), so 21 micro-USD short
    let over = flip(sized(FULL_FLIP_USD), 40_000_000 * UNITS, eur(10_000), 90_000_000 * UNITS + 1_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
    // the judgment comes with the Close{Flip}, once: no second finding for the Fill that is its other half
    assert_eq!(verify(&over).findings.len(), 1);
    // at 1.0001 (S = $100,000 again; bid 1_000_098_999_900, ask 1_000_101_000_100) the 4 M long bought at the ask
    // 1_000_001_000_000 and closed at that bid has made 4e12 x 97_999_900 / 1e12 = 391_999_600 micro-USD (the slice is not quick:
    // 100 s), so B is 100_391_999_600. A short of r units opened at that bid, marked at that ask, needs margin
    // ceil(r x 1_000_098_999_900 / 5e13) plus an open loss of r x 2_000_200 / 1e12: r = 5_018_601_228_000 units needs
    // 100_381_961_381 and loses 10_038_207, so equity 100_381_961_393 (12 micro-USD to spare); the next step is 8 short.
    let ok = flip(flat_spec(), 4_000_000 * UNITS, eur(10_001), 4_000_000 * UNITS + 5_018_601_228_000);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "the credit of the closing slice counts: {:#?}", verify(&ok).findings);
    let over = flip(flat_spec(), 4_000_000 * UNITS, eur(10_001), 4_000_000 * UNITS + 5_018_601_229_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
}

#[test]
fn the_notional_cap_of_the_market_is_enforced_and_the_cap_itself_is_allowed() {
    let at_cap = buy(5_000 * UNITS);
    assert_eq!(classes_of(&at_cap), Vec::<C>::new());
    // the cap is compared on the notional at the FILL price (the ask): 5,000 EUR = 5e9 units x 1_000_001_000_000 / 1e12 =
    // 5_000_005_000 micro-USD, so a cap of exactly that allows it ...
    let r = with_rules(&at_cap, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000005000");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    // ... one micro-USD less (and the old cap of 5_000_000_000, the notional at the mid) does not
    let r = with_rules(&at_cap, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000004999");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    // 5,000.001 EUR (one step more) is 5_000_001_000 x 1_000_001_000_000 / 1e12 = 5_000_006_000 micro-USD, above it
    let above = buy(5_000 * UNITS + 1_000);
    let r = with_rules(&above, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000005000");
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
    // a limit buy of 10,000 EUR at the level ASK (the ask of mid 1.0) triggers when the ask falls to it (equality triggers), at
    // mid 1.0; the generator fills it (it has no cap)
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_010) },
        Script::Order { t: T_1400, intent: order(setl8_verify::intents::IntentKind::Limit, 0, 0, 10_000 * UNITS, ASK, 0, 0, 0, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_000) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_001) },
        Script::Heartbeat { t: T_1400 + 20 },
    ]);
    assert!(b.events().iter().any(|e| matches!(e, E::Fill { order_kind: journal_types::OrderKind::Limit, .. })), "the order fills");
    assert_eq!(classes_of(&b), Vec::<C>::new());
    // under a cap of $5,000 the evaluator refuses the order: its Fill is an execution that must not exist ...
    let r = with_rules(&b, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":5000000000");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
    // ... and under a cap of exactly its notional at the fill price, 1e10 units x 1_000_001_000_000 / 1e12 = 10_000_010_000, it
    // is fine, one micro-USD less is not
    let r = with_rules(&b, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":10000010000");
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    let r = with_rules(&b, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":10000009999");
    assert_eq!(r.classes(), vec![C::FillNotAllowed], "{:#?}", r.findings);
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
    // a limit buy at the level ASK (the ask of mid 1.0) that triggers at T+10 and T+30 (the ask is at its level then, above it
    // at T+20); the generator fills it at T+10 (it has no hold); the evaluator that held the account refused it and left it pending
    let b = Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_010) },
        Script::Order { t: T_1400, intent: order(setl8_verify::intents::IntentKind::Limit, 0, 0, 100 * UNITS, ASK, 0, 0, 0, T_1400) },
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
    // long 40 M EUR at the ask, then a market BUY of 10 M (exactly the rest of the margin of 50 M, see FULL_LONG_USD: the
    // merged position is 5e13 units at the ask, the same numbers as the first test) is fine, 10 M + one step is not
    let add = |more: u64| {
        Gen::new(sized(FULL_LONG_USD)).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, 40_000_000 * UNITS, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_000) },
            Script::Order { t: T_1400 + 10, intent: market_order(0, true, more, T_1400 + 10) },
            Script::Heartbeat { t: T_1400 + 10 },
        ])
    };
    assert_eq!(classes_of(&add(10_000_000 * UNITS)), Vec::<C>::new());
    assert_eq!(classes_of(&add(10_000_000 * UNITS + 1_000)), vec![C::FillWithoutMargin]);
    // a short of exactly the whole margin (50 M EUR at the bid, marked at the ask: FULL_SHORT_USD), and one step more (margin
    // +20, open loss +1: 21 micro-USD short)
    let short = |qty: u64| {
        Gen::new(sized(FULL_SHORT_USD)).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, false, qty, T_1400) },
            Script::Heartbeat { t: T_1400 },
        ])
    };
    assert_eq!(classes_of(&short(50_000_000 * UNITS)), Vec::<C>::new());
    assert_eq!(classes_of(&short(50_000_000 * UNITS + 1_000)), vec![C::FillWithoutMargin]);
}

#[test]
fn a_flip_is_judged_before_its_closing_slice_so_the_closing_commission_is_not_yet_paid() {
    // 0.01% commission (100 e6) of margin_micro(qty, price, 1). Long 4 M EUR at the ask (fee ceil(4_000_004_000_000 x 1e-4) =
    // 400_000_400, B = 99_599_999_600), then a market sell of 4 M + r units at the same mid. The evaluator checks the whole order
    // BEFORE it books the closing slice and its fee: the slice at the bid makes 4e12 x -2_000_000 / 1e12 = -8_000_000, so
    // equity = 99_591_999_600 - open loss of the short (r x 2e-6, marked at the ask) - the reserve for closing it (1e-4 of r at the
    // ask) - the fill commission (1e-4 of r at the bid) and the margin is r x 0.999999 / 50. At r = 4_929_813_790_000 units:
    // 99_591_999_600 - 9_859_628 - 492_981_872 - 492_980_887 = 98_596_177_213 against a margin of 98_596_177_204 (9 micro-USD
    // to spare); the next step is 12 short. After the closing fee (399_999_600) B would be lower and r would have to stay under
    // 4_910_013_770_000 (17 to spare, the next step 4 short): the verifier must use the first. The third case below sits between the two
    // limits.
    let mut spec = flat_spec();
    spec.commission[0] = 100;
    let flip = |r: u64| {
        Gen::new(spec.clone()).run(&[
            Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
            Script::Order { t: T_1400, intent: market_order(0, true, 4_000_000 * UNITS, T_1400) },
            Script::Sample { market: 0, t: T_1400 + 100, mid: eur(10_000) },
            Script::Order { t: T_1400 + 100, intent: market_order(0, false, 4_000_000 * UNITS + r, T_1400 + 100) },
            Script::Heartbeat { t: T_1400 + 100 },
        ])
    };
    let ok = flip(4_929_813_790_000);
    assert_eq!(classes_of(&ok), Vec::<C>::new(), "{:#?}", verify(&ok).findings);
    let over = flip(4_929_813_791_000);
    assert_eq!(classes_of(&over), vec![C::FillWithoutMargin], "{:#?}", verify(&over).findings);
    // 4,920,000 EUR is over the limit that would apply if the closing fee were paid first (4,910,013.77 EUR), so it passes only
    // if the verifier judges before the closing slice
    let between = flip(4_920_000 * UNITS);
    assert_eq!(classes_of(&between), Vec::<C>::new(), "{:#?}", verify(&between).findings);
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
