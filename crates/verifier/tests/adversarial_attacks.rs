//! M12 item 1, verifier side: DESIGN 13.2 attacks whose control is the public verifier (SELF-audit by the builder).
//! Each test builds an honest journal with the test-only evaluator in `common`, checks that the verifier calls it VALID, then
//! changes exactly what a dishonest evaluator would change and requires a named finding class. The program-side half is in
//! `tests-rs/tests/adversarial.rs` (what the program lets through).
//!
//! * A-1: a breach posted late with an old sample (caught), a proof older than the window (caught), and FINDING ATK-1 (an old
//!   sample of the window on a market close is NOT caught, ignored test).
//! * A-9: a `timer_start` that does not follow from the fills (the timer-restart evasion).
//! * A-14: a stale or false `Meta.flat` with a pass requested while a position is open.
//! * A-18: a fabricated loss (a Fee no rule calls for; a Close of a position nobody opened) used to fail an honest account.
mod common;
use common::*;
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent as E};
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

fn proof_of_line(b: &Bundle, line: usize) -> journal_types::PriceProof {
    match &b.lines[line] {
        GLine::Event { proof: Some(p), .. } => p.clone(),
        _ => panic!("no proof"),
    }
}

fn set_proof(b: &mut Bundle, line: usize, p: journal_types::PriceProof) {
    match &mut b.lines[line] {
        GLine::Event { proof, .. } => *proof = Some(p),
        _ => panic!("not an event"),
    }
}

/// 200,000 EUR bought at 14:00:00 (mid 1.0800); the price spikes to 1.0830 at +30 s and is back at 1.0800 at +60 s, where the
/// honest evaluator closes at the bid of 1.0800.
fn spike_then_close() -> Bundle {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_830) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_800) },
        Script::Order { t: T_1400 + 60, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 60) },
    ];
    Gen::new(spec).run(&script)
}

/// The Close of `spike_then_close` re-priced with the 30 s old spike sample (a genuine archive record, inside the 60 s window).
fn close_with_the_old_spike(honest: &Bundle) -> Bundle {
    let mut b = honest.clone();
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    let old = b.samples.iter().position(|s| s.ts_sec() == T_1400 + 30).expect("the spike sample");
    let old_proof = proof_of(&b.spec, &b.samples[old]);
    let h = u32::from(b.spec.market(0).spread_e6);
    let exit = engine::bid_from_mid(b.samples[old].mid_e12().unwrap(), h).unwrap();
    set_proof(&mut b, ci, old_proof);
    if let E::Close { exit_e12, pnl, credited, entry_e12, qty, quick, .. } = b.ev_mut(ci) {
        *exit_e12 = exit;
        *pnl = engine::pnl_micro(true, *qty, *entry_e12, exit).unwrap();
        *quick = false;
        *credited = *pnl;
        assert!(*pnl > 0, "the replayed price is a profit, the honest close is a loss by the spread");
    }
    b
}

#[test]
fn a01_the_old_sample_close_is_what_the_program_accepts_and_the_honest_journal_is_valid() {
    // the honest journal is VALID (control for the finding below)
    let honest = spike_then_close();
    let r = verify(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
    // the tampered close is internally consistent: price from the proof, pnl from the price, proof a real sample inside 60 s
    let b = close_with_the_old_spike(&honest);
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    let p = proof_of_line(&b, ci);
    assert_eq!(journal_types::proof_time_secs(&p), T_1400 + 30);
    if let E::Close { time, .. } = b.events().iter().find(|e| matches!(e, E::Close { .. })).unwrap() {
        assert_eq!(*time - T_1400 - 30, 30, "30 s old at the event time, the market's window is 60 s");
    }
}

// FINDING ATK-1
#[test]
#[ignore = "FINDING ATK-1: the verifier calls VALID a market close priced with an older sample of the max_age window (DESIGN 13.2 A-1 says it compares with the archive)"]
fn a01_finding_atk1_a_market_close_priced_with_an_older_favourable_sample_must_be_flagged() {
    let b = close_with_the_old_spike(&spike_then_close());
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "an old but genuine sample of the last 60 s was accepted for a close at +60 s: {:#?}", r.findings);
}

/// DESIGN 13.2 A-1, the case the verifier DOES catch: the breach is posted late (30 s) with the old sample. The program accepts it
/// (the sample is inside the window); the archive walk finds the breach at its own second and none at the posted one. (The same row
/// is `row_030_...` in tests-rs/tests/trust_matrix.rs; this is the standalone, shorter form.)
#[test]
fn a01_late_breach_with_an_old_sample() {
    let honest = breach_then_recovery();
    let r = verify(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
    let mut b = honest.clone();
    let bi = b.find(|e| matches!(e, E::Breach { .. }));
    // the batch of the Breach: its Close, Fee and Breach events move 30 s later, and so does the Meta that ends the batch; every
    // later event is kept in order
    let shift = 30;
    let from = (0..bi).rev().find(|i| matches!(&b.lines[*i], GLine::Event { ev: E::Meta { .. }, .. })).map_or(0, |i| i + 1);
    for i in from..b.lines.len() {
        let mut is_meta = false;
        if let GLine::Event { ev, .. } = &mut b.lines[i] {
            match ev {
                E::Close { time, .. } | E::Fee { time, .. } | E::Breach { time, .. } => *time += shift,
                E::Meta { time, .. } => {
                    *time += shift;
                    is_meta = true;
                }
                _ => {}
            }
        }
        if is_meta {
            break;
        }
    }
    let r = verify_no_snapshot(&b);
    println!("A-1 late breach, verifier classes: {:?}", r.classes());
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::MissedBreach) || r.classes().contains(&C::FalseBreach), "{:#?}", r.findings);
}

#[test]
fn a01_the_same_event_with_a_proof_older_than_the_window_is_a_proof_age_finding() {
    let mut b = spike_then_close();
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    let mut p = proof_of_line(&b, ci);
    p.timestamp_ns -= 61_000_000_000;
    set_proof(&mut b, ci, p);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::ProofAge), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ A-9

/// 1,000 units at 14:00:00, 200,000 EUR added at +44 s, everything closed at +50 s at a profit: the timer is the big add (6 s),
/// so the close is a quick slice.
fn tiny_then_big_then_close() -> Bundle {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 1_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 44, mid: eur(10_800) },
        Script::Order { t: T_1400 + 44, intent: market_order(0, true, 200_000_000_000, T_1400 + 44) },
        Script::Sample { market: 0, t: T_1400 + 50, mid: eur(10_830) },
        Script::Order { t: T_1400 + 50, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 50) },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn a09_a_timer_start_that_ignores_the_restart_on_the_add_is_a_finding() {
    let honest = tiny_then_big_then_close();
    let ci = honest.find(|e| matches!(e, E::Close { .. }));
    if let GLine::Event { ev: E::Close { timer_start, quick, .. }, .. } = &honest.lines[ci] {
        assert_eq!((*timer_start, *quick), (T_1400 + 44, true), "honest: the add restarted the timer");
    }
    let r = verify(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);

    // dishonest: timer_start = the first order (50 s old at the close), not quick, full credit: the PROGRAM accepts this
    let mut b = honest.clone();
    if let E::Close { timer_start, quick, credited, pnl, .. } = b.ev_mut(ci) {
        *timer_start = T_1400;
        *quick = false;
        *credited = *pnl;
    }
    let r = verify_no_snapshot(&b);
    println!("A-9 verifier classes: {:?}", r.classes());
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::QuickMismatch), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ A-14

#[test]
fn a14_a_pass_requested_with_an_open_position_and_a_flat_meta_that_lies_is_invalid() {
    // 1,000,000 EUR bought and held; b is 100,000 (no target either)
    let spec = Spec::base();
    let mut b = eur_run(spec, &[eur(10_801), eur(10_802)]);
    assert!(!b.final_state.flat);
    // the evaluator (stale or lying) writes the last Meta as flat
    let last_meta = b.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    if let GLine::Event { ev: E::Meta { flat, .. }, .. } = &mut b.lines[last_meta] {
        assert!(!*flat);
        *flat = true;
    }
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&b);
    println!("A-14 verifier classes: {:?}", r.classes());
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::StateMismatch), "Meta.flat disagrees with the replayed position: {:#?}", r.findings);
    assert!(r.classes().contains(&C::PassInvalid), "{:#?}", r.findings);
    assert!(r.per_pass[0].reasons.contains(&"not_flat".to_string()), "{:?}", r.per_pass);
}

// ------------------------------------------------------------------------------------------------ A-18

/// An account with one sample (14:00:00, mid 1.0800) and one heartbeat batch, to which the test appends a failing batch
/// `[loss event, Fail, Meta(flat)]` at 14:00:10 with the sample as the Fail's proof: exactly the shape `report_fail` accepts on chain.
fn fail_after(loss: impl FnOnce(&Bundle, i64) -> E) -> Bundle {
    let spec = Spec::base();
    let mut b = Gen::new(spec).run(&[Script::Sample { market: 0, t: T_1400, mid: eur(10_800) }, Script::Heartbeat { t: T_1400 }]);
    let r = verify_no_snapshot(&b);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "the base account is honest: {:#?}", r.findings);
    let t = T_1400 + 10;
    let seq = b.last_seq();
    let ev = loss(&b, t);
    let p = proof_of(&b.spec, &b.samples[0]);
    let floor = 92_000_000_000;
    b.lines.push(GLine::Event { seq: seq + 1, ev: ev.clone(), proof: if matches!(ev, E::Close { .. }) { Some(p.clone()) } else { None } });
    b.lines.push(GLine::Event { seq: seq + 2, ev: E::Fail { time: t, reason: FailReason::StaticFloor, b_after: floor, proof: 0 }, proof: Some(p) });
    let meta = E::Meta { time: t, flat: true, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: engine::ENGINE_VERSION };
    b.lines.push(GLine::Event { seq: seq + 3, ev: meta, proof: None });
    b.metas.push(MetaCommit { line: b.lines.len() - 1, n_samples: b.samples.len(), n_intent_lines: b.intents.len() });
    b.refresh_commitments();
    b
}

#[test]
fn a18_a_fee_no_rule_calls_for_used_to_fail_an_honest_account_is_a_fee_mismatch() {
    let b = fail_after(|_, t| E::Fee { kind: FeeKind::Commission, market: 0, amount: 8_000_000_000, time: t });
    let r = verify_no_snapshot(&b);
    println!("A-18 (fee) verifier classes: {:?}", r.classes());
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::FeeMismatch), "{:#?}", r.findings);
}

#[test]
fn a18_a_close_of_a_position_nobody_opened_used_to_fail_an_honest_account_is_a_position_mismatch() {
    let b = fail_after(|b, t| {
        let exit = engine::bid_from_mid(b.samples[0].mid_e12().unwrap(), 30).unwrap();
        let qty = 1_000_000_000_000u64; // 1M EUR: at this quantity pnl (micro-USD) = exit - entry (e12 units)
        let entry = exit + 8_000_000_000; // an invented entry price 0.008 above the exit: a loss of exactly 8,000 USD
        let pnl = engine::pnl_micro(true, qty, entry, exit).unwrap();
        E::Close { market: 0, side: journal_types::Side::Long, qty, entry_e12: entry, exit_e12: exit, time: t, timer_start: t - 100, pnl, quick: false, credited: pnl, reason: CloseReason::Manual, intent_id: 0, proof: 0 }
    });
    let r = verify_no_snapshot(&b);
    println!("A-18 (close) verifier classes: {:?}", r.classes());
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.classes().contains(&C::PositionMismatch), "{:#?}", r.findings);
    assert!(r.classes().contains(&C::ForgedTrade), "a manual close with no signed intent behind it: {:#?}", r.findings);
}
