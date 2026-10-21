//! Triage of the M12 review claims V13 and V14 (verifier side). Tests marked `#[ignore = "FINDING ..."]` assert the CORRECT
//! behaviour and fail today; run them with `-- --ignored`.
mod common;

use common::*;
use journal_types::JournalEvent as E;
use setl8_verify::FindingClass as C;

const UNITS: u64 = 1_000_000;
const TEST_HALF_SPREAD_E6: u16 = 1;

fn flat_spec() -> Spec {
    let mut s = Spec::base();
    s.markets[0].spread_e6 = TEST_HALF_SPREAD_E6;
    s.markets[0].max_age_secs = 900;
    s
}

/// One sample at 14:00:00, one market buy at that second, one heartbeat Meta.
fn buy(qty: u64) -> Bundle {
    Gen::new(flat_spec()).run(&[
        Script::Sample { market: 0, t: T_1400, mid: eur(10_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, qty, T_1400) },
        Script::Heartbeat { t: T_1400 },
    ])
}

/// Inserts a raw log line at position `at` and moves the Meta commit table with it (as in `m9c.rs`).
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

// FINDING V13
/// The program refuses an exposure-increasing Fill only when `held == 1` at POST time. Chain order of the log is: hold, release,
/// then the evaluator's Fill (dated one second before the release's block time, which is legal: `ev.time >= last_batch_time` and
/// `<= now + 2`). The verifier judges the Fill by event time against `[hold, release)` and says FillNotAllowed on a history the
/// program accepted. The hold/release lines carry only the indexer's block time (no `at_seq`, DESIGN 8.1 promises one).
#[test]
#[ignore = "FINDING V13: verifier judges Fills by event time against hold-line block times, not by chain order; false MISMATCH on a legal history"]
fn a_fill_posted_after_the_release_but_dated_before_its_block_time_is_not_a_fill_on_a_held_account() {
    let honest = buy(10 * UNITS);
    let mut b = honest.clone();
    // chain order: opened, account_held (block time T-5), hold_released (block time T+1), then the Fill batch
    insert_line(&mut b, 1, hold(T_1400 + 1, false));
    insert_line(&mut b, 1, hold(T_1400 - 5, true));
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), Vec::<C>::new(), "the program accepts this order of events; {:#?}", r.findings);
}

/// Control: the same history with the hold line AFTER the Fill in file order is Valid (the verifier does follow file order for
/// holds that start later), so the V13 false positive is limited to a release whose block time is later than the Fill's date.
#[test]
fn control_a_hold_line_after_the_fill_does_not_condemn_the_fill() {
    let honest = buy(10 * UNITS);
    assert_eq!(verify_no_snapshot(&honest).classes(), Vec::<C>::new());
    let mut b = honest.clone();
    let fill_line = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Fill { .. }, .. })).unwrap();
    insert_line(&mut b, fill_line + 1, hold(T_1400, true));
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
}

// FINDING V14
/// The program checks `t >= previous event time` for EVERY event of a batch including `Meta` (post_journal.rs:399-402) and stores
/// the Meta time as `last_batch_time`. The verifier exempts Meta from the backwards check, so a Meta dated before the Fill that
/// precedes it in the same batch is "Valid" here, `TimeBackwards` on chain (trust-matrix row 15 says Both).
#[test]
#[ignore = "FINDING V14: verifier does not flag a Meta dated before the previous event (program: TimeBackwards)"]
fn a_meta_dated_before_the_previous_event_is_time_backwards() {
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { time, .. } = b.ev_mut(m) {
        *time = T_1400 - 5;
    }
    let r = verify_no_snapshot(&b);
    assert!(r.findings.iter().any(|f| f.class == C::StateMismatch && f.message.contains("backwards")), "no backwards finding: {:#?}", r.findings);
}

/// Second half: the Meta time is the program's `last_batch_time`, so the next batch's first event must be >= it. The verifier
/// keeps `last_time` from events only: a Close dated before the previous batch's Meta passes the monotonic clock.
#[test]
#[ignore = "FINDING V14: verifier does not update its clock from Meta, so an event dated before an earlier Meta is not flagged"]
fn an_event_dated_before_the_meta_of_the_previous_batch_is_time_backwards() {
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { time, .. } = b.ev_mut(m) {
        *time = T_1400 + 700; // the Close of the next batch is dated T_1400 + 600
    }
    let r = verify_no_snapshot(&b);
    assert!(r.findings.iter().any(|f| f.message.contains("backwards")), "no backwards finding: {:#?}", r.findings);
}
