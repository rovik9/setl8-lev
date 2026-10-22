//! P25b (decision 68), audit findings F-08 and F-12: the PRICE-SAMPLE RULE (DESIGN 4.5). A market Fill/Close is priced with the NEWEST
//! archive sample of its market at or before the event second; a triggered pending order and a stop-loss/take-profit close with the
//! trigger sample; a Breach/Fail close with the mark the decision used. Any other sample, even a genuine one inside the market's
//! `max_age_secs` window and even one that is better for the trader, is the finding `PriceSampleMismatch` (older or newer).
//! Every expectation below is written by hand from that rule.
mod common;
use common::*;
use journal_types::{JournalEvent as E, OrderKind};
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

fn set_proof(b: &mut Bundle, line: usize, p: journal_types::PriceProof) {
    match &mut b.lines[line] {
        GLine::Event { proof, .. } => *proof = Some(p),
        _ => panic!("not an event"),
    }
}

/// Re-prices the Close or Fill at `line` with the archive sample at second `sec` (the first such sample in the stream, or the
/// `nth`), keeping the bundle internally consistent (price from the proof, pnl from the price).
fn reprice_with(b: &mut Bundle, line: usize, sec: i64, nth: usize) {
    let idx: Vec<usize> = b.samples.iter().enumerate().filter(|(_, s)| s.ts_sec() == sec).map(|(i, _)| i).collect();
    let si = idx[nth];
    let proof = proof_of(&b.spec, &b.samples[si]);
    let mid = b.samples[si].mid_e12().unwrap();
    let h = u32::from(b.spec.market(0).spread_e6);
    set_proof(b, line, proof);
    match b.ev_mut(line) {
        E::Close { exit_e12, pnl, credited, entry_e12, qty, quick, side, .. } => {
            let long = *side == journal_types::Side::Long;
            *exit_e12 = engine::close_price_e12(mid, h, long).unwrap();
            *pnl = engine::pnl_micro(long, *qty, *entry_e12, *exit_e12).unwrap();
            *quick = false;
            *credited = *pnl;
        }
        E::Fill { price_e12, side, .. } => *price_e12 = engine::fill_price_e12(mid, h, *side == journal_types::Side::Long).unwrap(),
        _ => panic!("not a trade"),
    }
}

/// 200,000 EUR bought at +0 (mid 1.0800); the close is a trader close at +60. `mids` are the mids at +30 and +60; `extra` samples
/// are appended AFTER the close order (so they are in the archive but newer than the event second or in the same second).
fn buy_then_close(mid30: u64, mid60: u64, same_second_second_sample: Option<u64>, after: Option<(i64, u64)>) -> Bundle {
    let spec = Spec::base();
    let mut script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(mid30) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(mid60) },
    ];
    if let Some(m) = same_second_second_sample {
        script.push(Script::Sample { market: 0, t: T_1400 + 60, mid: eur(m) });
    }
    script.push(Script::Order { t: T_1400 + 60, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 60) });
    if let Some((t, m)) = after {
        script.push(Script::Sample { market: 0, t, mid: eur(m) });
    }
    script.push(Script::Heartbeat { t: T_1400 + 61 });
    Gen::new(spec).run(&script)
}

fn mismatches(b: &Bundle) -> Vec<String> {
    let r = verify_no_snapshot(b);
    r.findings.iter().filter(|f| f.class == C::PriceSampleMismatch).map(|f| f.message.clone()).collect()
}

fn valid(b: &Bundle) {
    let r = verify_no_snapshot(b);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

#[test]
fn the_honest_close_at_the_newest_sample_is_valid() {
    valid(&buy_then_close(10_830, 10_800, None, None));
}

#[test]
fn a_market_close_priced_with_an_older_favourable_sample_is_a_price_sample_mismatch() {
    // the spike at +30 s is better for the trader (bid 1.0830 against 1.0800 at +60 s); a genuine record inside the 60 s window
    let mut b = buy_then_close(10_830, 10_800, None, None);
    valid(&b);
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    reprice_with(&mut b, ci, T_1400 + 30, 0);
    let m = mismatches(&b);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("OLDER"), "{}", m[0]);
    assert_eq!(verify_no_snapshot(&b).verdict, Verdict::Mismatch);
}

#[test]
fn the_unfavourable_twin_is_flagged_as_well() {
    // the dip at +30 s is WORSE for the trader; the same rule, the same class (the old trust-matrix NEITHER row)
    let mut b = buy_then_close(10_770, 10_800, None, None);
    valid(&b);
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    reprice_with(&mut b, ci, T_1400 + 30, 0);
    assert_eq!(mismatches(&b).len(), 1);
}

#[test]
fn the_newest_sample_of_the_event_second_is_the_one_not_the_first_of_that_second() {
    // two samples in second +60: the evaluator closes at the LAST of them
    let mut b = buy_then_close(10_830, 10_800, Some(10_805), None);
    valid(&b);
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    reprice_with(&mut b, ci, T_1400 + 60, 0); // the first one of the second
    let m = mismatches(&b);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("OLDER"), "{}", m[0]);
}

#[test]
fn a_sample_after_the_event_second_is_newer_than_the_rule_allows() {
    // a genuine sample one second AFTER the close (the shared proof check tolerates +2 s): the rule says no
    let mut b = buy_then_close(10_830, 10_800, None, Some((T_1400 + 61, 10_790)));
    valid(&b);
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    reprice_with(&mut b, ci, T_1400 + 61, 0);
    let m = mismatches(&b);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("NEWER"), "{}", m[0]);
}

#[test]
fn a_market_fill_is_held_to_the_same_rule() {
    // buy at +60 with the sample of +30 instead of the newest
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_810) },
        Script::Order { t: T_1400 + 60, intent: market_order(0, true, 200_000_000_000, T_1400 + 60) },
        Script::Heartbeat { t: T_1400 + 61 },
    ];
    let mut b = Gen::new(spec).run(&script);
    valid(&b);
    let fi = b.find(|e| matches!(e, E::Fill { .. }));
    reprice_with(&mut b, fi, T_1400 + 30, 0);
    assert_eq!(mismatches(&b).len(), 1);
}

#[test]
fn a_triggered_limit_order_is_filled_at_the_trigger_sample_not_at_an_older_one() {
    // limit buy at 1.0795: the +30 s sample (1.0798) does not trigger it, the +60 s sample (1.0790) does
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_798) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_810) },
        Script::Heartbeat { t: T_1400 + 90 },
    ];
    let mut b = Gen::new(spec).run(&script);
    valid(&b);
    let fi = b.find(|e| matches!(e, E::Fill { order_kind: OrderKind::Limit, .. }));
    reprice_with(&mut b, fi, T_1400 + 30, 0); // the older, non-triggering sample
    let m = mismatches(&b);
    assert!(m.iter().any(|x| x.contains("OLDER")), "{m:?}");
}

#[test]
fn the_forced_closes_of_a_breach_are_priced_with_the_marks_of_the_decision_sample() {
    // breach_then_recovery: tamper the Close{Breach} to the genuine sample one step before the breach sample (inside the 60 s window)
    let honest = breach_then_recovery();
    valid(&honest);
    let mut b = honest.clone();
    let ci = b.find(|e| matches!(e, E::Close { reason: journal_types::CloseReason::Breach, .. }));
    let tb = if let E::Close { time, .. } = b.ev_mut(ci) { *time } else { unreachable!() };
    let before = b.samples.iter().map(|s| s.ts_sec()).filter(|t| *t < tb).max().expect("an earlier sample");
    assert!(tb - before <= 60, "inside the market window: {tb} {before}");
    reprice_with(&mut b, ci, before, 0);
    let m = mismatches(&b);
    assert!(m.iter().any(|x| x.contains("OLDER")), "{m:?}\n{:#?}", verify_no_snapshot(&b).findings);
}
