//! Triage of the independent M12 review claims AUTH-09 .. AUTH-16 (program / vault behaviour).
//! Tests marked `#[ignore = "FINDING ..."]` assert the CORRECT behaviour and fail today. The others pin facts the verdicts rely on.
mod common;

use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::{acct_pda, vault_trader_state_pda};
use common::*;
use journal_types::FailReason;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E, events::PassCancelled};
use solana_signer::Signer;

const DELAY: i64 = 172_800;
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;
const WEEK: i64 = 604_800;
const MAX_DELAY: u32 = 518_400; // 6 days since decision 67

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

// ------------------------------------------------------------------------------------------------ AUTH-09

// FINDING AUTH-09 / F-30 (FIXED, decision 67: the delay range is 1 to 6 days)
#[test]
fn auth09_set_limits_must_refuse_a_delay_beyond_the_vault_inactivity_window() {
    let mut w = JEnv::new();
    for delay in [MAX_DELAY + 1, WEEK as u32 - 1, WEEK as u32, WEEK as u32 + 1, 1_209_600] {
        let ix = set_limits_ix(both(&w.v.e), delay, CAP, WINDOW);
        let r = w.v.e.send_as_admins(&[ix]);
        assert_err(&r, E::InvalidLimits);
    }
}

/// Since decision 67 the maximum delay (6 days) is paid with NO refresh of the vault activity clock: the record is idle for at most
/// 6 days + the time between the last activity and the confirmation, under the vault's 7 days.
#[test]
fn auth09_the_maximum_delay_is_paid_without_any_refresh() {
    let (mut w, t) = base();
    w.set_limits(MAX_DELAY, CAP, WINDOW);
    pend2(&mut w, t);
    let payer = w.new_payer();
    w.ok(|w| w.finalize(t + i64::from(MAX_DELAY), &payer));
    assert!(w.claim(1).is_some(), "the 0.08 S claim is queued");
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
}

/// The mitigation the claim names: the trader's own `release_account` while the pass is pending keeps the vault clock alive.
#[test]
fn auth09_pin_release_account_works_while_a_pass_is_confirmed() {
    let (mut w, t) = base();
    w.set_limits(MAX_DELAY, CAP, WINDOW);
    pend2(&mut w, t);
    w.ok(|w| w.release_acct(t + WEEK - 1_000));
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
    assert_eq!(w.vault_status().last_activity_timestamp, t + WEEK - 1_000);
}

// ------------------------------------------------------------------------------------------------ AUTH-10

/// A lamport sent to the vault TraderState PDA of the NEXT challenge must not block `open_account` (real vault build).
#[test]
fn auth10_prefunded_vault_trader_state_does_not_block_open_account() {
    let (mut w, _) = base();
    let tr = dup(&w.trader);
    let next = w.v.wallet_opt(&tr.pubkey()).unwrap().next_challenge_id;
    let vts = vault_trader_state_pda(&tr.pubkey(), next);
    w.v.e.svm.airdrop(&vts, 1).unwrap();
    let r = w.v.open(&tr, 8);
    assert!(r.is_ok(), "open_account failed on a pre-funded vault TraderState: {r:?}");
}

/// Same for this program's own TradingAccount PDA (Anchor `init`).
#[test]
fn auth10_prefunded_trading_account_does_not_block_open_account() {
    let (mut w, _) = base();
    let tr = dup(&w.trader);
    let next = w.v.wallet_opt(&tr.pubkey()).unwrap().next_challenge_id;
    w.v.e.svm.airdrop(&acct_pda(&tr.pubkey(), next), 1).unwrap();
    let r = w.v.open(&tr, 8);
    assert!(r.is_ok(), "open_account failed on a pre-funded TradingAccount: {r:?}");
}

/// A lamport sent to the next PayoutClaim PDA must not block `finalize_pass`.
#[test]
fn auth10_prefunded_payout_claim_does_not_block_finalize_pass() {
    let (mut w, t) = base();
    pend2(&mut w, t);
    let id = w.account().next_request_id;
    let vts = w.vts();
    w.v.e.svm.airdrop(&claim_pda(&vts, id), 1).unwrap();
    let payer = w.new_payer();
    let r = w.finalize(t + DELAY, &payer);
    assert!(r.is_ok(), "finalize_pass failed on a pre-funded PayoutClaim: {r:?}");
}

// ------------------------------------------------------------------------------------------------ AUTH-12

/// Pins the facts: with the vault record Abandoned (permissionless call) and the local account Active and floor-breached,
/// `report_fail` reverts, but `touch_activity` (evaluator) or `release_account` (trader) resolve the state to Abandoned.
#[test]
fn auth12_pin_report_fail_reverts_on_a_vault_abandoned_record_and_touch_resolves_it() {
    let (mut w, _) = base();
    let t = w.t0 + WEEK + 1;
    w.edit_account(|a| a.b = FLOOR);
    assert_ok(w.vault_mark_abandoned(t));
    let b = w.account().b;
    let batch = w.fail_batch(t + 5, vec![], FailReason::StaticFloor, b, true);
    let r = w.report_fail(t + 6, &batch, 0, &[MARKET_EUR]);
    assert!(r.is_err(), "report_fail on a vault-Abandoned record reverts");
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
    w.ok(|w| w.touch(t + 10));
    assert_eq!(w.account().status, c::STATUS_ABANDONED);
    assert_eq!(w.wallet_profile().active_count, 0);
}

/// Same for a held account: the violation fail reverts while the vault record is Abandoned; push_activity resolves it.
#[test]
fn auth12_pin_fail_for_violation_reverts_on_a_vault_abandoned_record_and_release_resolves_it() {
    let (mut w, _) = base();
    let t = w.t0 + WEEK + 1;
    w.ok(|w| w.hold(w.t0 + 10, hash(1)));
    assert_ok(w.vault_mark_abandoned(t));
    assert!(w.violation(t + 5, hash(2)).is_err());
    w.ok(|w| w.release_acct(t + 10));
    let a = w.account();
    assert_eq!(a.status, c::STATUS_ABANDONED);
    assert_eq!(a.held, 0, "the hold count is released with the abandonment");
    assert_eq!(w.wallet_profile().held_count, 0);
}

/// A confirmed pass whose vault record is Abandoned: `finalize_pass` reverts, `release_account` then abandons locally.
#[test]
fn auth12_pin_finalize_reverts_on_a_vault_abandoned_record_and_release_resolves_it() {
    let (mut w, t) = base();
    pend2(&mut w, t);
    let ts = w.t0 + WEEK + 1;
    assert_ok(w.vault_mark_abandoned(ts));
    let payer = w.new_payer();
    assert!(w.finalize(ts + DELAY, &payer).is_err());
    assert!(w.claim(1).is_none());
    w.ok(|w| w.release_acct(ts + DELAY + 1));
    assert_eq!(w.account().status, c::STATUS_ABANDONED);
}

// ------------------------------------------------------------------------------------------------ AUTH-13

// FINDING AUTH-13 (fixed by F-27, decision 67: every event is emit_cpi!)
#[test]
fn auth13_cancel_pass_emits_its_event_as_a_cpi_event() {
    let (mut w, t) = base();
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t, 0);
    let m = w.ok(|w| w.cancel(t + 3_600));
    assert_eq!(cpi_events::<PassCancelled>(&m).len(), 1);
}

// ------------------------------------------------------------------------------------------------ AUTH-14

// FINDING AUTH-14
#[test]
#[ignore = "FINDING AUTH-14: vault_status reads byte 96 of any vault-owned PDA-address account without checking the TraderState discriminator, so a layout shift marks live accounts Abandoned"]
fn auth14_release_account_must_not_trust_a_status_byte_of_a_foreign_layout() {
    let (mut w, _) = base();
    let vts = w.vts();
    let mut acc = w.v.e.svm.get_account(&vts).unwrap();
    acc.data[0..8].copy_from_slice(&[7u8; 8]); // not a TraderState any more (a different or upgraded layout)
    acc.data[c::VAULT_STATUS_OFFSET] = 3;
    w.v.e.svm.set_account(vts, acc).unwrap();
    let t = w.t0 + 100_000;
    let r = w.release_acct(t);
    assert!(r.is_err(), "a wrong-discriminator vault account must be refused");
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
}
