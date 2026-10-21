//! Triage of the independent M12 review claims V6 and V10 (program / vault behaviour). These tests PIN facts the verdicts rely on;
//! none is a finding.
mod common;

use common::journal::*;
use common::*;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E};

const DELAY: i64 = 172_800;
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(DELAY as u32, CAP, WINDOW);
    (w, t)
}

fn pend2(w: &mut JEnv, at: i64) {
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, at - 10, at);
}

/// V6(a): a cap below one payout is accepted by `set_limits` (DESIGN 6.1: "cap > 0") and freezes confirmed passes; the trader has
/// no exit (`cancel_pass` is state 1 only), but the SAME two admins end it with a second `set_limits`; no veto, no loss of the pass.
#[test]
fn v6a_pin_a_tiny_cap_blocks_finalize_and_the_admins_undo_it_with_set_limits() {
    let (mut w, t) = base();
    pend2(&mut w, t);
    w.set_limits(DELAY as u32, 1, WINDOW);
    let payer = w.new_payer();
    assert_err(&w.finalize(t + DELAY, &payer), E::DailyPayoutCapReached);
    assert_err(&w.cancel(t + DELAY + 1), E::NoPendingPass);
    assert_eq!(w.account().pass_pending, c::PASS_CONFIRMED);
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let r = w.finalize(t + DELAY + 2, &payer);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(w.account().phase, 3);
}

/// V10: `finalize_pass` uses the CURRENT `pass_delay_secs` (DESIGN 6.4: `now >= pass_proposed_at + pass_delay_secs`; the doc
/// comment of `handle_finalize_pass` says so). A pass confirmed under a 14-day delay is finalizable after 1 day once both admins
/// lower the delay; `PassConfirmed.eligible_at` is then not what the program enforces.
#[test]
fn v10_pin_finalize_uses_the_delay_in_force_now_not_the_one_at_confirmation() {
    let (mut w, t) = base();
    w.set_limits(1_209_600, CAP, WINDOW);
    pend2(&mut w, t);
    let payer = w.new_payer();
    assert_err(&w.finalize(t + 86_400, &payer), E::PassDelayNotElapsed);
    w.set_limits(86_400, CAP, WINDOW);
    let r = w.finalize(t + 86_400, &payer);
    assert!(r.is_ok(), "{r:?}");
}
