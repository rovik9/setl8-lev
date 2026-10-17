//! M8 (`reset_account`): the first journal state of a reset account is the fresh start of the phase it failed in, not phase 1.
//! The verifier takes that phase from the `opened` line (`Opened.phase` as the program emits it for a reset) and replays from
//! `engine::reset_phase_state(S)`; these tests pin that for every phase and keep the dishonest variants caught.
//! (Which phase a reset account SHOULD start in is the program's business: `reset_account` copies the failed account's phase,
//! and the account it came from is a different account with a different event log. The link between the two logs is the
//! `AccountReset` event, see handover/module-8.md; a single-account verification cannot see it.)
mod common;

use common::*;
use setl8_verify::FindingClass as C;

fn only(r: &setl8_verify::Report, c: C) {
    assert_eq!(r.classes(), vec![c], "{:#?}", r.findings);
}

/// A phase-`p` start: opened in phase `p`, a trade, and a pass at the target.
fn started_in(p: u8) -> Bundle {
    pass_case_in_phase(0, p)
}

#[test]
fn an_account_whose_first_state_is_a_phase_p_start_is_valid_for_every_phase() {
    for p in 1..=5u8 {
        let b = started_in(p);
        assert_eq!(b.spec.phase, p);
        // with the chain's account as the snapshot too: phase p, b = 1.08 S
        let r = verify(&b);
        assert_eq!(r.classes(), Vec::<C>::new(), "phase {p}: {:#?}", r.findings);
        assert_eq!(r.state.phase, p);
        assert_eq!(r.per_phase.len(), 1);
        assert_eq!((r.per_phase[0].phase, r.per_phase[0].valid), (p, true));
    }
}

#[test]
fn a_phase_p_start_begins_from_the_fresh_engine_state() {
    for p in 1..=5u8 {
        // no event at all: the replay state is the fresh state of the phase
        let b = meta_only_in(p);
        let r = verify_no_snapshot(&b);
        assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
        let s = &r.state;
        assert_eq!((s.phase, s.b, s.q_tally, s.latch, s.tight_on, s.flat, s.lock_until), (p, 100_000_000_000, 0, false, false, true, 0), "phase {p}");
    }
}

fn meta_only_in(p: u8) -> Bundle {
    let mut spec = Spec::base();
    spec.phase = p;
    Gen::new(spec).run(&[Script::Sample { market: 0, t: T_1400, mid: eur(10_800) }])
}

#[test]
fn the_pass_of_a_phase_p_start_is_judged_in_phase_p() {
    for p in 2..=5u8 {
        let base = started_in(p);
        let (seq, t) = (base.last_seq(), last_event_time(&base));
        let with = |phase: u8| {
            let mut x = base.clone();
            x.push_pass_requested(phase, 8_000_000_000, 108_000_000_000, seq, t);
            verify_no_snapshot(&x).per_pass[0].reasons.clone()
        };
        assert_eq!(with(p), Vec::<String>::new(), "phase {p}");
        // another phase than the one the account was opened in is a phase mismatch, above and below
        assert_eq!(with(p - 1), vec!["phase_mismatch"], "phase {p} claimed as {}", p - 1);
        if p < 5 {
            assert_eq!(with(p + 1), vec!["phase_mismatch"], "phase {p} claimed as {}", p + 1);
        }
    }
}

#[test]
fn a_phase_p_start_that_is_finalized_moves_on_to_p_plus_one_and_phase_5_graduates() {
    for p in 2..=5u8 {
        let mut b = started_in(p);
        let (seq, t) = (b.last_seq(), last_event_time(&b));
        b.push_pass_requested(p, 8_000_000_000, 108_000_000_000, seq, t);
        b.push_pass_confirmed(p, 8_000_000_000, 108_000_000_000, seq, t + 172_800);
        let to = if p == 5 { 0 } else { p + 1 };
        b.push_raw(&format!(
            "\"type\":\"pass_finalized\",\"phase\":{p},\"to_phase\":{to},\"amount\":8000000000,\"request_id\":{},\"tally_count\":1,\"tally_total\":8000000000,\"window_total\":8000000000,\"time\":{}",
            p - 1,
            t + 172_800
        ));
        let r = verify_no_snapshot(&b);
        assert_eq!(r.classes(), Vec::<C>::new(), "phase {p}: {:#?}", r.findings);
        if p < 5 {
            assert_eq!((r.state.phase, r.state.b), (p + 1, 100_000_000_000), "the next phase starts fresh");
        }
    }
}

#[test]
fn an_opened_phase_that_differs_from_the_chains_account_is_a_state_mismatch() {
    // the chain's account says phase 3 after the same history; the log's opened line says the account started in phase 4
    let honest = started_in(3);
    assert_eq!(verify(&honest).classes(), Vec::<C>::new());
    let mut lie = honest.clone();
    for l in lie.lines.iter_mut() {
        if let GLine::Opened(o) = l {
            o.phase = 4;
        }
    }
    let mut f = lie.files();
    f.snapshot = Some(honest.snapshot_json().into_bytes());
    let r = setl8_verify::verify_account(&f.inputs()).unwrap();
    only(&r, C::StateMismatch);
    assert!(r.findings[0].message.contains("phase"), "{}", r.findings[0].message);
}

#[test]
fn a_phase_advanced_from_a_phase_the_account_never_was_in_is_refused() {
    // the account started in phase 3: a "phase_advanced 2 -> 3" (as if it had been bought in phase 1) is not its history
    let mut b = started_in(3);
    let t = last_event_time(&b);
    b.push_raw(&format!("\"type\":\"phase_advanced\",\"from_phase\":2,\"to_phase\":3,\"time\":{t}"));
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::StateMismatch), "{:#?}", r.findings);
}

#[test]
fn a_phase_p_start_cannot_be_a_pass_of_phase_1_semantics() {
    // phase 1 pays nothing and advances at the confirmation; a phase 3 start with a phase-1 style request (amount 0) is wrong
    let mut b = started_in(3);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(3, 0, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.per_pass[0].reasons, vec!["amount_mismatch"], "{:?}", r.per_pass);
}
