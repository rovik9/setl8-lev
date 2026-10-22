//! P19 part A (decision 60): an account failed by `fail_for_violation` (both admins, DESIGN 2.9: "no refund, no reset, no payout")
//! can never be reset, not even after `unban_wallet`. The marker is `pass_pending == PASS_VIOLATION` (3), written by
//! `fail_for_violation` and by nothing else. Every refusal asserts the exact error and an unchanged world digest.
mod common;

use common::journal::*;
use common::vault::VAULT_ID;
use common::*;
use journal_types::FailReason;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E};
use solana_signer::Signer;

const CAP: u64 = 100_000_000_000;
const DELAY: i64 = 86_400;
const WINDOW: u32 = 86_400;
const SIZE_100K: i64 = 100_000_000_000;
const QUICK_LIMIT: i64 = 2_800_000_000;
const RETENTION: i64 = 7 * 86_400;
const PQ: u8 = 1;

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let t = w.t0 + 1_000;
    (w, t)
}

fn hash(n: u8) -> [u8; 32] {
    [n; 32]
}

/// A held account in `phase`, failed for a violation. Returns the clock after.
fn violated(phase: u8) -> (JEnv, i64) {
    let (mut w, t) = base();
    w.edit_account(|a| a.phase = phase);
    w.ok(|w| w.hold(t, hash(1)));
    w.ok(|w| w.violation(t + 1, hash(2)));
    (w, t + 100)
}

fn unban(w: &mut JEnv) {
    let admins = Admins::both(&w.v.e);
    let tk = w.trader.pubkey();
    assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tk), admins));
    assert_eq!(w.wallet_profile().banned, 0);
}

fn ban(w: &mut JEnv) {
    let admins = Admins::both(&w.v.e);
    let tk = w.trader.pubkey();
    let ix = ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
    assert_ok(w.v.e.send_admins(ix, admins));
    assert_eq!(w.wallet_profile().banned, 1);
}

#[test]
fn a_violation_fail_leaves_exactly_the_marker_value_3_in_pass_pending() {
    let (w, _) = violated(3);
    let a = w.account();
    assert_eq!((a.status, a.pass_pending, a.held, a.phase), (c::STATUS_FAILED, 3, 0, 3));
    assert_eq!(c::PASS_VIOLATION, 3);
    assert!(a.violation_failed());
    // the marker is not a reset, and the layout did not change
    assert_eq!(a.reset_at(), 0);
    assert_eq!(w.v.e.svm.get_account(&w.acct).unwrap().data.len(), 207);
    // the wallet is banned and the vault knows the challenge as failed
    assert_eq!(w.wallet_profile().banned, 1);
    assert_eq!(w.vault_status().status, 2, "vault: Failed");
}

#[test]
fn a_reset_of_a_violation_failed_account_is_refused_while_the_wallet_is_banned() {
    let (mut w, t) = violated(3);
    // the ban answers first (it is checked before the previous account); nothing changes either way
    w.fails(E::WalletBanned, |w| w.reset(t));
    assert!(w.account().violation_failed());
}

#[test]
fn a_reset_of_a_violation_failed_account_is_refused_by_the_marker_after_the_unban() {
    for phase in 1..=5u8 {
        let (mut w, t) = violated(phase);
        let tk = w.trader.pubkey();
        unban(&mut w);
        let bal = w.v.token_balance(&w.v.trader_token(&tk));
        let next_id = w.wallet_profile().next_challenge_id;
        w.fails(E::ViolationNoReset, |w| w.reset(t));
        // before any CPI: the vault program is never invoked (a check after the CPI would revert the same way, but the logs
        // would show the call), no vault record was created, no money moved, no id used
        let logs = w.reset(t).expect_err("refused").meta.logs.join("\n");
        assert!(!logs.contains(&VAULT_ID.to_string()), "the vault was called before the marker was checked:\n{logs}");
        assert_eq!(w.v.vault_trader_state(&tk, 1), None);
        assert_eq!(w.v.token_balance(&w.v.trader_token(&tk)), bal);
        assert_eq!(w.wallet_profile().next_challenge_id, next_id);
        // and again later: the refusal is permanent
        w.fails(E::ViolationNoReset, |w| w.reset(t + 10 * RETENTION));
        assert!(w.account().violation_failed());
    }
}

#[test]
fn the_marker_survives_ban_and_unban_cycles_and_a_veto_attempt() {
    let (mut w, t) = violated(2);
    unban(&mut w);
    ban(&mut w);
    unban(&mut w);
    // veto_pass works on any status, but 3 is not a pending pass: it is refused and the marker stays
    w.fails(E::NoPendingPass, |w| w.veto(t));
    assert_eq!((w.account().pass_pending, w.account().vetoes), (3, 0));
    w.fails(E::ViolationNoReset, |w| w.reset(t + 1));
}

#[test]
fn the_marker_value_on_an_active_account_is_not_a_pending_pass_for_any_pass_instruction() {
    let (mut w, t) = base();
    w.edit_account(|a| a.pass_pending = c::PASS_VIOLATION);
    w.fails(E::NoPendingPass, |w| w.veto(t));
    w.fails(E::NoPendingPass, |w| w.confirm(t));
    w.fails(E::PassAlreadyPending, |w| w.request(t));
    // veto still accepts the two real pending states
    for s in [c::PASS_REQUESTED, c::PASS_CONFIRMED] {
        w.edit_account(|a| a.pass_pending = s);
        w.ok(|w| w.veto(t + 5));
        assert_eq!((w.account().pass_pending, w.account().vetoes), (0, 1));
        w.edit_account(|a| a.vetoes = 0);
    }
}

#[test]
fn a_floor_fail_and_a_quick_fail_are_still_resettable_and_carry_no_marker() {
    // static floor
    for phase in [1u8, 3, 5] {
        let (mut w, t) = base();
        w.fail_in(phase, t);
        let a = w.account();
        assert_eq!((a.status, a.pass_pending), (c::STATUS_FAILED, 0), "floor fail in phase {phase}");
        assert!(!a.violation_failed());
        w.reset_ok(t + 100);
        assert_eq!((w.account().status, w.account().phase), (c::STATUS_ACTIVE, phase));
    }
    // phase quick limit
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT + 1);
    let b = w.fail_batch(t, vec![], FailReason::PhaseQuick, SIZE_100K, true);
    assert_ok(w.report_fail(t + 1, &b, PQ, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending), (c::STATUS_FAILED, 0));
    assert!(!a.violation_failed());
    w.reset_ok(t + 100);
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
}

#[test]
fn a_floor_fail_of_a_wallet_that_was_banned_and_unbanned_is_resettable() {
    // the ban of a wallet is not the marker: only fail_for_violation sets it
    let (mut w, t) = base();
    w.fail_in(2, t);
    ban(&mut w);
    unban(&mut w);
    w.reset_ok(t + 100);
    assert_eq!(w.account().phase, 2);
}

#[test]
fn a_report_fail_of_a_held_account_keeps_the_marker_unset() {
    // held, then the evaluator's own floor fail (report_fail works on a held account): not a violation. P25c: the hold stays, the
    // reset waits for the two admins; after release_hold it is an ordinary reset
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    w.edit_account(|a| a.b = SIZE_100K / 100 * 92);
    let b = w.fail_batch(t + 1, vec![], FailReason::StaticFloor, SIZE_100K / 100 * 92, true);
    assert_ok(w.report_fail(t + 2, &b, 0, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending, a.held), (c::STATUS_FAILED, 0, 1));
    assert!(!a.violation_failed());
    w.fails(E::ReviewPending, |w| w.reset(t + 100));
    w.ok(|w| w.release_hold(t + 101));
    w.reset_ok(t + 102);
}

#[test]
fn abandoned_and_graduated_accounts_carry_no_marker() {
    // abandoned through the vault's idle rule (touch_activity)
    let (mut w, _) = base();
    let limit = w.t0 + 604_800;
    w.ok(|w| w.touch(limit + 1));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending), (c::STATUS_ABANDONED, 0));
    assert!(!a.violation_failed());
    // graduated: the fourth payout
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let payer = w.new_payer();
    let t = w.advance_to(5, w.t0 + 1_000, &payer, DELAY);
    w.ready(5);
    w.ok(|w| w.request(t));
    w.ok(|w| w.confirm(t + 1));
    w.ok(|w| w.finalize(t + 1 + DELAY, &payer));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending), (c::STATUS_GRADUATED, 0));
    assert!(!a.violation_failed());
}

#[test]
fn a_pending_pass_of_a_held_account_is_cleared_by_the_violation_and_the_marker_replaces_it() {
    for pending in [c::PASS_REQUESTED, c::PASS_CONFIRMED] {
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(pending, t - 10, t - 5);
        w.ok(|w| w.hold(t, hash(1)));
        w.ok(|w| w.violation(t + 1, hash(2)));
        let a = w.account();
        assert_eq!((a.status, a.pass_pending, a.pass_requested_at), (c::STATUS_FAILED, 3, 0));
        assert_eq!(a.terminal_at(), t + 1);
        unban(&mut w);
        w.fails(E::ViolationNoReset, |w| w.reset(t + 100));
    }
}

#[test]
fn close_account_of_a_violation_failed_account_still_works_after_the_retention() {
    let (mut w, t) = violated(4);
    let terminal_at = w.account().terminal_at();
    assert_eq!(terminal_at, t - 99);
    w.fails(E::CloseTooEarly, |w| w.close(terminal_at + RETENTION - 1));
    let before = w.v.e.svm.get_balance(&w.trader.pubkey()).unwrap();
    let rent = w.v.e.svm.get_account(&w.acct).unwrap().lamports;
    w.ok(|w| w.close(terminal_at + RETENTION));
    assert_eq!(w.v.e.svm.get_balance(&w.trader.pubkey()).unwrap(), before + rent);
    assert!(w.v.e.svm.get_account(&w.acct).is_none_or(|a| a.lamports == 0 || a.data.is_empty()));
}

#[test]
fn a_wallet_with_a_violation_failed_account_can_open_and_reset_other_accounts_once_unbanned() {
    // the marker belongs to ONE failed record; another account of the same wallet that failed normally is still resettable
    let (mut w, t) = violated(2);
    unban(&mut w);
    let violation_acct = w.acct;
    let id = w.open_another();
    assert_eq!(id, 1);
    w.fail_in(3, t + 10);
    w.reset_ok(t + 200);
    assert_eq!(w.account().phase, 3);
    // the violation record is still refused
    w.acct = violation_acct;
    w.fails(E::ViolationNoReset, |w| w.reset(t + 300));
}
