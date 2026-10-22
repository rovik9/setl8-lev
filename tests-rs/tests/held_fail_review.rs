//! P25c (decision 67 amendment, audit finding F-32): an account that `report_fail` fails WHILE HELD stays Failed + held until both
//! admins decide. Hand-written from the decision, not read off the program:
//!   1. `report_fail` on a held account sets status Failed exactly as before and does NOT clear the hold.
//!   2. `reset_account` on a Failed account that is still held is refused with `ReviewPending` (6110), before any CPI, bytes untouched.
//!   3. Both admins resolve it: `release_hold` (then it is an ordinary resettable failure) or `fail_for_violation` (then the marker
//!      refuses every reset with `ViolationNoReset`; the vault record is already Failed, so no second `flag_trader_failed`).
//!   4. Nothing else changes, except that `close_account` also refuses a Failed + held account (`ReviewPending`): closing it would
//!      delete the evidence and leave the wallet's `held_count` above zero for ever (`ban_wallet` would be refused for good).
mod common;

use anchor_lang::AccountSerialize;
use common::journal::*;
use common::lifecycle::*;
use common::pass::cpi_events;
use common::vault::*;
use common::*;
use journal_types::FailReason;
use setl8_lev_trading::{
    constants as c,
    errors::Setl8Error as E,
    events::{AccountFailed, HoldReleased, ViolationFail},
    state::TradingAccount,
};
use solana_signer::Signer;

const CAP: u64 = 100_000_000_000;
const DELAY: i64 = 86_400;
const WINDOW: u32 = 86_400;
const RETENTION: i64 = 7 * 86_400;

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let t = w.t0 + 1_000;
    (w, t)
}

fn bytes_of(a: &TradingAccount) -> Vec<u8> {
    let mut d = Vec::new();
    a.try_serialize(&mut d).unwrap();
    d
}

/// The static-floor fail batch at `t` (clock `t + 1`) on the current account, through the real `report_fail`.
fn fail_at(w: &mut JEnv, phase: u8, t: i64) -> litesvm::types::TransactionMetadata {
    let floor = i64::try_from(w.account().size_s / 100 * 92).unwrap();
    w.edit_account(|a| {
        a.phase = phase;
        a.b = floor;
    });
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, floor, true);
    w.ok(|w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]))
}

/// A held account in `phase`, failed by `report_fail` at `t + 10` (terminal_at = t + 11). Returns the world and the clock `t + 11`.
fn failed_while_held(phase: u8) -> (JEnv, i64) {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    fail_at(&mut w, phase, t + 10);
    (w, t + 11)
}

fn vault_bytes(w: &JEnv) -> Vec<u8> {
    w.v.e.svm.get_account(&w.vts()).expect("vault record").data
}

#[test]
fn report_fail_on_a_held_account_fails_it_and_keeps_the_hold() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    assert_eq!((w.account().held, w.wallet_profile().held_count, w.wallet_profile().active_count), (1, 1, 1));
    let m = fail_at(&mut w, 3, t + 10);
    let a = w.account();
    assert_eq!((a.status, a.held, a.pass_pending, a.flat, a.phase), (c::STATUS_FAILED, 1, 0, 1, 3), "Failed and STILL held");
    assert_eq!(a.terminal_at(), t + 11);
    assert_eq!(a.reset_at(), 0);
    let wp = w.wallet_profile();
    assert_eq!((wp.active_count, wp.held_count, wp.banned), (0, 1, 0), "the slot is freed, the hold still counts");
    // the vault record is Failed (the CPI ran once: a second flag_trader_failed would have reverted the whole transaction)
    assert_eq!(w.vault_status().status, 2);
    let ev = cpi_events::<AccountFailed>(&m);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].account, ev[0].reason), (w.acct, 0));
    // the same fail on an UNHELD twin differs in the held byte only
    let (mut twin, t2) = base();
    fail_at(&mut twin, 3, t2 + 10);
    let mut want = twin.account();
    assert_eq!(want.held, 0);
    want.held = 1;
    assert_eq!(bytes_of(&a), bytes_of(&want));
}

#[test]
fn an_unheld_account_is_still_failed_with_no_hold_at_all() {
    // control: the change touches only the held case
    let (mut w, t) = base();
    fail_at(&mut w, 2, t);
    let a = w.account();
    assert_eq!((a.status, a.held, a.pass_pending), (c::STATUS_FAILED, 0, 0));
    assert_eq!(w.wallet_profile().held_count, 0);
    w.reset_ok(t + 100);
}

#[test]
fn reset_of_a_failed_account_that_is_still_held_is_refused_with_review_pending() {
    for phase in 1..=5u8 {
        let (mut w, t) = failed_while_held(phase);
        let tk = w.trader.pubkey();
        let pre = bytes_of(&w.account());
        let wallet0 = w.wallet_profile();
        let bal = w.v.token_balance(&w.v.trader_token(&tk));
        let vault0 = vault_bytes(&w);
        w.fails(E::ReviewPending, |w| w.reset(t + 5));
        // before any CPI: the vault program is never invoked
        let logs = w.reset(t + 5).expect_err("refused").meta.logs.join("\n");
        assert!(!logs.contains(&VAULT_ID.to_string()), "phase {phase}: the vault was called before the hold was checked:\n{logs}");
        assert_eq!(bytes_of(&w.account()), pre, "phase {phase}: account bytes");
        let wallet1 = w.wallet_profile();
        assert_eq!(
            (wallet1.active_count, wallet1.next_challenge_id, wallet1.held_count, wallet1.banned),
            (wallet0.active_count, wallet0.next_challenge_id, wallet0.held_count, wallet0.banned)
        );
        assert_eq!(w.v.vault_trader_state(&tk, 1), None, "no new vault record");
        assert_eq!(vault_bytes(&w), vault0, "vault record bytes");
        assert_eq!(w.v.token_balance(&w.v.trader_token(&tk)), bal, "no money moved");
        // and for as long as the hold stays: a day, a month, a year later
        for later in [86_400, 30 * 86_400, 365 * 86_400] {
            w.fails(E::ReviewPending, |w| w.reset(t + later));
        }
    }
}

#[test]
fn reset_after_the_hold_is_released_is_an_ordinary_reset_at_the_ladder_price() {
    for phase in 1..=5u8 {
        let (mut w, t) = failed_while_held(phase);
        let tk = w.trader.pubkey();
        let prev_acct = w.acct;
        let m = w.ok(|w| w.release_hold(t + 5));
        assert_eq!(cpi_events::<HoldReleased>(&m).len(), 1);
        let released = w.account();
        assert_eq!((released.status, released.held, released.pass_pending), (c::STATUS_FAILED, 0, 0));
        assert_eq!(released.terminal_at(), t, "release does not move terminal_at");
        assert_eq!(w.wallet_profile().held_count, 0);
        // the price: the tier cost of this size x ladder[phase - 1], ceil; the 0.4% fee on top, ceil
        let size = released.size_s;
        let cost = w.v.registry().challenge_sizes.iter().find(|(s, _)| *s == size).expect("tier").1;
        let price = u64::try_from((u128::from(cost) * u128::from(RESET_LADDER_BPS[usize::from(phase) - 1])).div_ceil(10_000)).unwrap();
        let fee = u64::try_from((u128::from(price) * 40).div_ceil(10_000)).unwrap();
        let tt = w.v.trader_token(&tk);
        let bal = w.v.token_balance(&tt);
        w.reset_ok(t + 10);
        assert_eq!(bal - w.v.token_balance(&tt), price + fee, "phase {phase}: price + fee");
        let prev = w.v.account(&prev_acct);
        let mut want = released;
        want.pass_requested_at = t + 10; // reset_at
        assert_eq!(bytes_of(&prev), bytes_of(&want), "only the reset mark changed on the failed account");
        let new = w.account();
        assert_eq!((new.status, new.held, new.pass_pending, new.phase, new.b), (c::STATUS_ACTIVE, 0, 0, phase, i64::try_from(size).unwrap()));
    }
}

#[test]
fn release_hold_on_a_failed_held_account_works_once() {
    let (mut w, t) = failed_while_held(3);
    w.ok(|w| w.release_hold(t + 1));
    w.fails(E::NotHeld, |w| w.release_hold(t + 2));
}

#[test]
fn fail_for_violation_on_a_failed_held_account_writes_the_marker_and_no_second_vault_flag() {
    let (mut w, t) = failed_while_held(3);
    let tk = w.trader.pubkey();
    // a second live account of the same wallet: its slot must not be touched
    let failed_acct = w.acct;
    w.open_another();
    assert_eq!(w.wallet_profile().active_count, 1);
    w.acct = failed_acct;
    let pre = w.account();
    let vault0 = vault_bytes(&w);
    let m = w.ok(|w| w.violation(t + 5, hash(9)));
    let a = w.account();
    assert_eq!((a.status, a.held, a.pass_pending), (c::STATUS_FAILED, 0, c::PASS_VIOLATION));
    assert!(a.violation_failed());
    let mut want = pre;
    want.held = 0;
    want.pass_pending = c::PASS_VIOLATION;
    assert_eq!(bytes_of(&a), bytes_of(&want), "terminal_at and every other field are unchanged");
    let wp = w.wallet_profile();
    assert_eq!((wp.banned, wp.held_count, wp.active_count), (1, 0, 1), "banned; hold gone; the slot was freed by report_fail, not again");
    assert_eq!(vault_bytes(&w), vault0, "the vault record is untouched (already Failed)");
    assert_eq!(w.vault_status().status, 2);
    let logs = m.logs.join("\n");
    assert!(!logs.contains(&VAULT_ID.to_string()), "no vault call at all:\n{logs}");
    let ev = cpi_events::<ViolationFail>(&m);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].account, ev[0].trader, ev[0].evidence_hash), (failed_acct, tk, hash(9)));
    // never resettable: first the ban answers, then (after the unban) the marker, for ever
    w.fails(E::WalletBanned, |w| w.reset(t + 6));
    let admins = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tk), admins));
    for later in [6, RETENTION, 400 * 86_400] {
        w.fails(E::ViolationNoReset, |w| w.reset(t + later));
    }
}

#[test]
fn fail_for_violation_on_a_failed_account_that_is_not_held_is_refused() {
    let (mut w, t) = base();
    fail_at(&mut w, 3, t);
    w.fails(E::NotHeld, |w| w.violation(t + 5, hash(9)));
    // a violation-failed account (hold gone) cannot be violation-failed twice
    let (mut w, t) = failed_while_held(3);
    w.ok(|w| w.violation(t + 1, hash(9)));
    w.fails(E::NotHeld, |w| w.violation(t + 2, hash(9)));
}

#[test]
fn fail_for_violation_on_a_failed_held_account_needs_the_evidence_hash() {
    let (mut w, t) = failed_while_held(3);
    w.fails(E::EvidenceMissing, |w| w.violation(t + 1, no_hash()));
}

#[test]
fn a_failed_held_account_cannot_post_fill_close_or_fee() {
    let (mut w, t) = failed_while_held(3);
    let t = t + 10;
    let fl = fill(MARKET_EUR, true, MEUR, eur_ask(), t, 0);
    let b = w.batch(vec![fl, meta(t, false)], vec![p_eur(t)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::AccountNotActive);
    let c1 = close(MARKET_EUR, true, MEUR, eur_bid() + 5_000_000_000, eur_bid(), t, t - 100, 0);
    let b = w.batch(vec![c1, meta(t, true)], vec![p_eur(t)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::AccountNotActive);
    let b = w.batch(vec![fee(MARKET_EUR, 1_000_000_000, t), meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::AccountNotActive);
    // report_fail again: not Active
    let floor = w.account().b;
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, floor, true);
    let r = w.report_fail(t + 1, &b, 0, &[MARKET_EUR]);
    assert_err_at(&r, 0, E::AccountNotActive);
}

#[test]
fn close_account_refuses_a_failed_held_account_until_the_hold_is_resolved() {
    let (mut w, t) = failed_while_held(3);
    for later in [RETENTION, RETENTION + 1, 400 * 86_400] {
        w.fails(E::ReviewPending, |w| w.close(t + later));
    }
    // released: an ordinary failed account, closable 7 days after terminal_at (inclusive), not before
    w.ok(|w| w.release_hold(t + 100));
    w.fails(E::CloseTooEarly, |w| w.close(t + RETENTION - 1));
    w.ok(|w| w.close(t + RETENTION));
}

#[test]
fn close_account_still_closes_a_violation_failed_account_after_the_retention() {
    let (mut w, t) = failed_while_held(3);
    w.ok(|w| w.violation(t + 1, hash(9)));
    w.ok(|w| w.close(t + RETENTION));
}

#[test]
fn a_wallet_with_a_failed_held_account_cannot_be_banned_until_it_is_resolved() {
    let (mut w, t) = failed_while_held(3);
    let tk = w.trader.pubkey();
    let admins = Admins::both(&w.v.e);
    let ix = ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
    let r = w.v.e.send_admins(ix, admins);
    assert_err(&r, E::WalletHasHeldAccount);
    w.ok(|w| w.release_hold(t + 1));
    let ix = ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
    assert_ok(w.v.e.send_admins(ix, admins));
}

#[test]
fn a_failed_held_account_does_not_block_a_new_account_of_the_wallet() {
    // the slot was freed by report_fail; the hold does not count against the 5-account cap
    let (mut w, _t) = failed_while_held(3);
    let first = w.acct;
    for _ in 0..5 {
        w.open_another();
    }
    assert_eq!(w.wallet_profile().active_count, 5);
    assert_eq!(w.wallet_profile().held_count, 1);
    assert_eq!(w.v.account(&first).held, 1);
}
