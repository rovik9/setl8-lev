//! M6: the pass flow against the real vault harness. Every failure test asserts the exact error code and an unchanged
//! world digest (accounts, vault records, payout claims, tally, config, token balances).
mod common;

use anchor_lang::AccountSerialize;
use common::journal::*;
use common::pass::*;
use common::vault::{acct_pda, registry_pda, vault_trader_state_pda, VAULT_ID};
use common::*;
use setl8_lev_trading::{
    constants as c,
    errors::Setl8Error as E,
    events::*,
    state::TradingAccount,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

const DELAY: i64 = 172_800;
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;
const TARGET: i64 = 108_000_000_000;

/// Fresh world, base time (account time + 1,000 s), limits: 2-day delay, $100k cap, 24 h window.
fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(172_800, CAP, WINDOW);
    (w, t)
}

fn bytes_of(a: &TradingAccount) -> Vec<u8> {
    let mut d = Vec::new();
    a.try_serialize(&mut d).unwrap();
    d
}

/// The account after `f(before)` must equal the current one, byte for byte.
fn assert_only(before: &TradingAccount, after: &TradingAccount, f: impl FnOnce(&mut TradingAccount)) {
    let mut exp = TradingAccount::try_deserialize_unchecked_clone(before);
    f(&mut exp);
    assert_eq!(bytes_of(&exp), bytes_of(after));
}

trait CloneAcct {
    fn try_deserialize_unchecked_clone(a: &TradingAccount) -> TradingAccount;
}
impl CloneAcct for TradingAccount {
    fn try_deserialize_unchecked_clone(a: &TradingAccount) -> TradingAccount {
        use anchor_lang::AccountDeserialize;
        TradingAccount::try_deserialize(&mut bytes_of(a).as_slice()).unwrap()
    }
}

/// State 1 (requested at `at`) in `phase`, directly written.
fn pend1(w: &mut JEnv, phase: u8, at: i64) {
    w.ready(phase);
    w.set_pending(c::PASS_REQUESTED, at, 0);
}

/// State 2 (confirmed at `at`) in `phase`, directly written.
fn pend2(w: &mut JEnv, phase: u8, at: i64) {
    w.ready(phase);
    w.set_pending(c::PASS_CONFIRMED, at - 10, at);
}

fn vault_log_has(r: &litesvm::types::TransactionResult, needle: &str) -> bool {
    match r {
        Ok(_) => false,
        Err(f) => f.meta.logs.iter().any(|l| l.contains(needle)),
    }
}

// ================================================================ the state machine (DESIGN 10.3): every edge

#[test]
fn edge_request_advance_0_to_1() {
    let (mut w, t) = base();
    w.ready(2);
    let before = w.account();
    let m = w.ok(|w| w.request(t));
    let a = w.account();
    assert_eq!((a.pass_pending, a.pass_requested_at), (c::PASS_REQUESTED, t));
    assert_only(&before, &a, |x| {
        x.pass_pending = 1;
        x.pass_requested_at = t;
    });
    assert_eq!(cpi_events::<PassRequested>(&m).len(), 1);
}

#[test]
fn edge_reject_pass_1_to_0() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.ok(|w| w.reject(t + 5, c::REJECT_NOT_FLAT));
    assert_eq!(w.account().pass_pending, 0);
}

#[test]
fn edge_cancel_pass_1_to_0() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.ok(|w| w.cancel(t + 3_600));
    assert_eq!(w.account().pass_pending, 0);
}

#[test]
fn edge_confirm_pass_1_to_2_in_phases_2_to_5() {
    for phase in 2..=5u8 {
        let (mut w, t) = base();
        pend1(&mut w, phase, t);
        w.ok(|w| w.confirm(t + 7));
        let a = w.account();
        assert_eq!((a.pass_pending, a.pass_proposed_at, a.phase, a.b), (c::PASS_CONFIRMED, t + 7, phase, TARGET), "phase {phase}");
    }
}

#[test]
fn edge_confirm_pass_in_phase_1_goes_to_0_and_starts_phase_2() {
    let (mut w, t) = base();
    pend1(&mut w, 1, t);
    let m = w.ok(|w| w.confirm(t + 7));
    let a = w.account();
    assert_eq!((a.pass_pending, a.phase, a.b), (0, 2, S100K));
    let ev = cpi_events::<PhaseAdvanced>(&m);
    assert_eq!((ev.len(), ev[0].from_phase, ev[0].to_phase, ev[0].time), (1, 1, 2, t + 7));
    assert!(cpi_events::<PassConfirmed>(&m).is_empty());
    assert_eq!(w.claim(1), None, "phase 1 moves no money");
}

#[test]
fn edge_veto_pass_from_state_1() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.ok(|w| w.veto(t + 5));
    assert_eq!(w.account().pass_pending, 0);
}

#[test]
fn edge_veto_pass_from_state_2() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    w.ok(|w| w.veto(t + 5));
    assert_eq!(w.account().pass_pending, 0);
}

#[test]
fn edge_finalize_pass_2_to_0_starts_the_next_phase() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    let m = w.ok(|w| w.finalize(t + DELAY, &payer));
    let a = w.account();
    assert_eq!((a.pass_pending, a.phase, a.status), (0, 3, c::STATUS_ACTIVE));
    let ev = cpi_events::<PassFinalized>(&m);
    assert_eq!((ev.len(), ev[0].phase, ev[0].to_phase, ev[0].amount, ev[0].request_id), (1, 2, 3, PAYOUT_100K, 1));
}

// ================================================================ the state machine: every illegal edge

#[test]
fn illegal_request_advance_from_states_1_and_2() {
    for state in [c::PASS_REQUESTED, c::PASS_CONFIRMED] {
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(state, t - 5, t - 4);
        w.fails(E::PassAlreadyPending, |w| w.request(t));
    }
}

#[test]
fn illegal_cancel_pass_from_states_0_and_2() {
    let (mut w, t) = base();
    w.ready(2);
    w.fails(E::NoPendingPass, |w| w.cancel(t + 99_999));
    pend2(&mut w, 2, t);
    w.fails(E::NoPendingPass, |w| w.cancel(t + 99_999));
}

#[test]
fn illegal_confirm_pass_from_states_0_and_2() {
    let (mut w, t) = base();
    w.ready(2);
    w.fails(E::NoPendingPass, |w| w.confirm(t));
    pend2(&mut w, 2, t);
    w.fails(E::NoPendingPass, |w| w.confirm(t));
}

#[test]
fn illegal_reject_pass_from_states_0_and_2() {
    let (mut w, t) = base();
    w.ready(2);
    w.fails(E::NoPendingPass, |w| w.reject(t, c::REJECT_OTHER));
    pend2(&mut w, 2, t);
    w.fails(E::NoPendingPass, |w| w.reject(t, c::REJECT_OTHER));
}

#[test]
fn illegal_veto_pass_from_state_0() {
    let (mut w, t) = base();
    w.ready(2);
    w.fails(E::NoPendingPass, |w| w.veto(t));
}

#[test]
fn illegal_finalize_pass_from_states_0_and_1() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.ready(2);
    w.fails(E::NoPendingPass, |w| w.finalize(t + 9 * DELAY, &payer));
    pend1(&mut w, 2, t);
    w.fails(E::NoPendingPass, |w| w.finalize(t + 9 * DELAY, &payer));
}

// ================================================================ request_advance

#[test]
fn request_not_active_is_refused() {
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        let (mut w, t) = base();
        w.ready(2);
        w.edit_account(|a| a.status = status);
        w.fails(E::AccountNotActive, |w| w.request(t));
    }
}

#[test]
fn request_held_is_refused() {
    let (mut w, t) = base();
    w.ready(2);
    w.edit_account(|a| a.held = 1);
    w.fails(E::AccountHeld, |w| w.request(t));
}

#[test]
fn request_paused_sector_is_refused() {
    let (mut w, t) = base();
    w.ready(2);
    w.pause_sector(true);
    w.fails(E::Paused, |w| w.request(t));
    w.pause_sector(false);
    w.ok(|w| w.request(t));
}

#[test]
fn request_not_flat_is_refused() {
    let (mut w, t) = base();
    w.ready(2);
    w.edit_account(|a| a.flat = 0);
    w.fails(E::NotFlat, |w| w.request(t));
}

#[test]
fn request_tight_mode_on_is_refused() {
    let (mut w, t) = base();
    w.ready(2);
    w.edit_account(|a| a.tight_on = 1);
    w.fails(E::TightModeOn, |w| w.request(t));
}

#[test]
fn request_target_boundary_is_1_08_s_to_the_micro() {
    let (mut w, t) = base();
    w.ready(2);
    w.edit_account(|a| a.b = TARGET - 1);
    w.fails(E::TargetNotReached, |w| w.request(t));
    w.edit_account(|a| a.b = TARGET);
    w.ok(|w| w.request(t));
    assert_eq!(w.account().pass_pending, 1);
}

#[test]
fn request_phase_marked_failed_is_refused() {
    let (mut w, t) = base();
    w.ready(2);
    // quick tally above 0.028 S (2,800 on a 100k account): failed; at the limit it is not
    w.edit_account(|a| a.q_tally = 2_800_000_001);
    w.fails(E::PhaseFailed, |w| w.request(t));
    w.edit_account(|a| a.q_tally = 2_800_000_000);
    w.ok(|w| w.request(t));
}

#[test]
fn request_wrong_signer_and_missing_signature() {
    let (mut w, t) = base();
    w.ready(2);
    let stranger = Keypair::new();
    w.fails(E::Unauthorized, |w| {
        w.set_time(t);
        let mut ix = w.request_ix();
        ix.accounts[0].pubkey = stranger.pubkey();
        w.v.e.send(&[ix], &[&stranger])
    });
    w.fails_anchor(3010, |w| {
        w.set_time(t);
        let mut ix = w.request_ix();
        ix.accounts[0].is_signer = false;
        w.v.e.send(&[ix], &[])
    });
}

#[test]
fn request_wrong_vault_program_and_wrong_sector_authority() {
    let (mut w, t) = base();
    w.ready(2);
    w.fails(E::InvalidVaultProgram, |w| {
        w.set_time(t);
        let mut ix = w.request_ix();
        ix.accounts[5].pubkey = solana_pubkey::Pubkey::new_unique();
        w.v.e.send(&[ix], &[])
    });
    w.fails_anchor(2006, |w| {
        w.set_time(t);
        let mut ix = w.request_ix();
        ix.accounts[4].pubkey = solana_pubkey::Pubkey::new_unique();
        w.v.e.send(&[ix], &[])
    });
}

#[test]
fn request_amount_in_the_event_is_zero_in_phase_1_and_eight_percent_after() {
    for (phase, amount) in [(1u8, 0u64), (2, PAYOUT_100K), (3, PAYOUT_100K), (4, PAYOUT_100K), (5, PAYOUT_100K)] {
        let (mut w, t) = base();
        w.ready(phase);
        w.edit_account(|a| a.seq = 7);
        let m = w.ok(|w| w.request(t));
        let ev = cpi_events::<PassRequested>(&m);
        assert_eq!(ev.len(), 1);
        let a = w.account();
        assert_eq!(
            (ev[0].account, ev[0].phase, ev[0].amount, ev[0].b, ev[0].seq, ev[0].journal_hash, ev[0].time),
            (w.acct, phase, amount, TARGET, 7, a.journal_hash, t)
        );
    }
}

#[test]
fn request_records_activity_in_the_vault_and_remembers_it() {
    let (mut w, t) = base();
    w.ready(2);
    // inside the vault's throttle window nothing changes on the vault and `last_activity_pushed` stays
    let v0 = w.vault_status().last_activity_timestamp;
    let pushed0 = w.account().last_activity_pushed;
    w.ok(|w| w.request(t));
    assert_eq!(w.vault_status().last_activity_timestamp, v0, "throttled");
    assert_eq!(w.account().last_activity_pushed, pushed0);
    // a day later it is Recorded: the vault clock and ours move
    let (mut w, _) = base();
    w.ready(2);
    let t2 = w.t0 + 86_401;
    w.ok(|w| w.request(t2));
    assert_eq!(w.vault_status().last_activity_timestamp, t2);
    assert_eq!(w.account().last_activity_pushed, t2);
}

#[test]
fn request_still_works_while_the_vault_product_is_paused() {
    let (mut w, t) = base();
    w.ready(2);
    w.pause_vault_product();
    w.ok(|w| w.request(t));
    assert_eq!(w.account().pass_pending, 1);
}

#[test]
fn request_abandoned_by_the_vault_marks_the_account_and_frees_the_slot() {
    let (mut w, _) = base();
    w.ready(2);
    assert_eq!(w.wallet_profile().active_count, 1);
    // idle exactly 7 days is not stale; one second more is
    let limit = w.t0 + 604_800;
    w.ok(|w| w.request(limit));
    assert_eq!(w.account().pass_pending, 1);
    let (mut w, _) = base();
    w.ready(2);
    let tally0 = w.tally_bytes();
    let cfg0 = w.v.e.config();
    let m = w.ok(|w| w.request(limit + 1));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending), (c::STATUS_ABANDONED, 0));
    assert_eq!(w.wallet_profile().active_count, 0);
    assert_eq!(w.vault_status().status, 3, "vault: Abandoned");
    assert_eq!(w.tally_bytes(), tally0);
    assert_eq!((w.v.e.config().window_start, w.v.e.config().window_total), (cfg0.window_start, cfg0.window_total));
    assert_eq!(cpi_events::<AccountAbandoned>(&m).len(), 1);
    assert!(cpi_events::<PassRequested>(&m).is_empty());
}

// ================================================================ cancel_pass

#[test]
fn cancel_boundary_is_3600_seconds() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.fails(E::CancelTooEarly, |w| w.cancel(t + 3_599));
    let ms = w.ok(|w| w.cancel(t + 3_600));
    assert_eq!(w.account().pass_pending, 0);
    let ev = events_of::<PassCancelled>(&ms.logs);
    assert_eq!((ev.len(), ev[0].phase, ev[0].time), (1, 2, t + 3_600));
}

#[test]
fn cancel_by_another_signer_is_refused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    let stranger = Keypair::new();
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 4_000);
        let mut ix = w.cancel_ix();
        ix.accounts[0].pubkey = stranger.pubkey();
        w.v.e.send(&[ix], &[&stranger])
    });
    // the evaluator cannot cancel either
    let ev = dup(&w.v.evaluator);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 4_000);
        let mut ix = w.cancel_ix();
        ix.accounts[0].pubkey = ev.pubkey();
        w.v.e.send(&[ix], &[&ev])
    });
}

#[test]
fn cancel_leaves_everything_else_untouched() {
    let (mut w, t) = base();
    pend1(&mut w, 3, t);
    let before = w.account();
    w.ok(|w| w.cancel(t + 3_600));
    assert_only(&before, &w.account(), |x| x.pass_pending = 0);
}

// ================================================================ confirm_pass

#[test]
fn confirm_wrong_signer_is_refused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    let tr = dup(&w.trader);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.confirm_ix(tr.pubkey());
        w.v.e.send(&[ix], &[&tr])
    });
    // both admins together cannot confirm either
    let (a, b) = (dup(&w.v.e.sl8), dup(&w.v.e.rov));
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.confirm_ix(a.pubkey());
        w.v.e.send(&[ix], &[&a, &b])
    });
    w.fails_anchor(3010, |w| {
        w.set_time(t + 1);
        let mut ix = w.confirm_ix(dup(&w.v.evaluator).pubkey());
        ix.accounts[0].is_signer = false;
        w.v.e.send(&[ix], &[])
    });
}

#[test]
fn confirm_preconditions_each_violated_alone() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.edit_account(|a| a.held = 1);
    w.fails(E::AccountHeld, |w| w.confirm(t + 1));
    w.edit_account(|a| a.held = 0);
    w.edit_account(|a| a.flat = 0);
    w.fails(E::NotFlat, |w| w.confirm(t + 1));
    w.edit_account(|a| a.flat = 1);
    w.edit_account(|a| a.b = TARGET - 1);
    w.fails(E::TargetNotReached, |w| w.confirm(t + 1));
    w.edit_account(|a| a.b = TARGET);
    w.edit_account(|a| a.q_tally = 2_800_000_001);
    w.fails(E::PhaseFailed, |w| w.confirm(t + 1));
    w.edit_account(|a| a.q_tally = 0);
    w.edit_account(|a| a.status = c::STATUS_FAILED);
    w.fails(E::AccountNotActive, |w| w.confirm(t + 1));
    w.edit_account(|a| a.status = c::STATUS_ACTIVE);
    w.ok(|w| w.confirm(t + 1));
}

#[test]
fn confirm_works_while_the_sector_is_paused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.pause_sector(true);
    w.ok(|w| w.confirm(t + 1));
    assert_eq!(w.account().pass_pending, 2);
}

#[test]
fn confirm_phase_1_starts_a_fresh_phase_2_with_the_exact_reset_list() {
    let (mut w, t) = base();
    pend1(&mut w, 1, t);
    // dirty everything a phase carries
    w.edit_account(|a| {
        a.latch = 1;
        a.q_tally = 1_234;
        a.lock_until = t + 50_000;
        a.trades_closed = 9;
        a.seq = 31;
        a.journal_hash = [0x5A; 32];
        a.last_batch_time = t - 3;
        a.vetoes = 2;
        a.next_request_id = 1;
        a.last_activity_pushed = t - 9;
        a.r = 7;
        a.g_peak = 8;
        a.l_level = 9;
    });
    let before = w.account();
    w.ok(|w| w.confirm(t + 1));
    let a = w.account();
    // reset: b = S, q = 0, latch 0, tight off, r/g_peak/l_level 0, flat 1, phase 2, nothing pending, pass_* cleared
    assert_eq!((a.b, a.q_tally, a.latch, a.tight_on, a.r, a.g_peak, a.l_level, a.flat), (S100K, 0, 0, 0, 0, 0, 0, 1));
    assert_eq!((a.phase, a.pass_pending, a.pass_requested_at, a.pass_proposed_at), (2, 0, 0, 0));
    // kept: an active lock, the chain (seq, hash, last_batch_time), counters (trades_closed, vetoes), request id, activity
    assert_eq!((a.lock_until, a.trades_closed, a.seq, a.journal_hash, a.last_batch_time), (t + 50_000, 9, 31, [0x5A; 32], t - 3));
    assert_eq!((a.vetoes, a.next_request_id, a.last_activity_pushed, a.status, a.held), (2, 1, t - 9, c::STATUS_ACTIVE, 0));
    assert_only(&before, &a, |x| {
        x.b = S100K;
        x.q_tally = 0;
        x.latch = 0;
        x.r = 0;
        x.g_peak = 0;
        x.l_level = 0;
        x.phase = 2;
        x.pass_pending = 0;
        x.pass_requested_at = 0;
        x.pass_proposed_at = 0;
    });
}

#[test]
fn confirm_phases_2_to_5_event_and_eligibility() {
    let (mut w, t) = base();
    pend1(&mut w, 4, t);
    w.edit_account(|a| a.seq = 5);
    let m = w.ok(|w| w.confirm(t + 11));
    let ev = cpi_events::<PassConfirmed>(&m);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].phase, ev[0].amount, ev[0].b, ev[0].seq, ev[0].eligible_at), (4, PAYOUT_100K, TARGET, 5, t + 11 + DELAY));
    assert_eq!(w.account().pass_proposed_at, t + 11);
    assert!(cpi_events::<PhaseAdvanced>(&m).is_empty());
}

// ================================================================ reject_pass

#[test]
fn reject_each_reason_is_accepted_and_emitted() {
    for reason in [c::REJECT_NOT_FLAT, c::REJECT_JOURNAL_BEHIND, c::REJECT_TARGET_NOT_REACHED, c::REJECT_OTHER] {
        let (mut w, t) = base();
        pend1(&mut w, 3, t);
        w.edit_account(|a| {
            a.latch = 1;
            a.q_tally = 77;
            a.tight_on = 0;
        });
        let before = w.account();
        let m = w.ok(|w| w.reject(t + 1, reason));
        let ev = cpi_events::<PassRejected>(&m);
        assert_eq!((ev.len(), ev[0].reason, ev[0].phase, ev[0].account), (1, reason, 3, w.acct));
        // phase, b, q, latch, tight state untouched: only `pass_pending` differs
        assert_only(&before, &w.account(), |x| x.pass_pending = 0);
    }
}

#[test]
fn reject_bad_reason_codes_are_refused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    for reason in [4u8, 5, 100, 255] {
        w.fails(E::RejectReasonInvalid, |w| w.reject(t + 1, reason));
    }
}

#[test]
fn reject_wrong_signer_is_refused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    let tr = dup(&w.trader);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.reject_ix(tr.pubkey(), 0);
        w.v.e.send(&[ix], &[&tr])
    });
}

// ================================================================ veto_pass

#[test]
fn veto_single_admin_is_refused() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e), Admins::neither(&w.v.e)] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 1);
            let ix = w.veto_ix(admins);
            w.v.e.send_admins(ix, admins)
        });
    }
    // strangers in the admin slots
    let (x, y) = (Keypair::new(), Keypair::new());
    let admins = Admins::strangers(&x, &y);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.veto_ix(admins);
        w.v.e.send(&[ix], &[&x, &y])
    });
    // the swapped order of the real admins is refused too (positional)
    let e = &w.v.e;
    let swapped = Admins { a: e.rov.pubkey(), b: e.sl8.pubkey(), a_signs: true, b_signs: true };
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.veto_ix(swapped);
        let (a, b) = (dup(&w.v.e.sl8), dup(&w.v.e.rov));
        w.v.e.send(&[ix], &[&a, &b])
    });
}

#[test]
fn veto_counts_and_changes_nothing_else_in_both_pending_states() {
    for state in [c::PASS_REQUESTED, c::PASS_CONFIRMED] {
        let (mut w, t) = base();
        w.ready(3);
        w.edit_account(|a| {
            a.latch = 1;
            a.q_tally = 5;
            a.lock_until = 12;
        });
        w.set_pending(state, t - 5, if state == c::PASS_CONFIRMED { t - 4 } else { 0 });
        let before = w.account();
        let m = w.ok(|w| w.veto(t));
        assert_only(&before, &w.account(), |x| {
            x.pass_pending = 0;
            x.vetoes = 1;
        });
        let ev = cpi_events::<PassVetoed>(&m);
        let e = &w.v.e;
        assert_eq!((ev.len(), ev[0].phase, ev[0].admin_a, ev[0].admin_b), (1, 3, e.sl8.pubkey(), e.rov.pubkey()));
        assert_eq!(w.claim(1), None);
        assert_eq!(w.tally().requested_count, 0);
    }
}

#[test]
fn veto_counter_saturates_at_255() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    w.edit_account(|a| a.vetoes = 254);
    w.ok(|w| w.veto(t));
    assert_eq!(w.account().vetoes, 255);
    w.set_pending(c::PASS_REQUESTED, t, 0);
    w.ok(|w| w.veto(t + 1));
    assert_eq!(w.account().vetoes, 255, "saturates, never wraps");
}

#[test]
fn a_vetoed_trader_can_request_again() {
    let (mut w, t) = base();
    w.ready(2);
    w.ok(|w| w.request(t));
    w.ok(|w| w.veto(t + 1));
    assert_eq!((w.account().pass_pending, w.account().b), (0, TARGET));
    w.ok(|w| w.request(t + 2));
    assert_eq!(w.account().pass_pending, 1);
}

// ================================================================ finalize_pass: delay, payer, hold, pauses

#[test]
fn finalize_delay_boundary_uses_the_current_config_value() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + DELAY - 1, &payer));
    w.ok(|w| w.finalize(t + DELAY, &payer));
    assert_eq!(w.account().phase, 3);
}

#[test]
fn finalize_delay_clamped_minimum_is_one_day_and_a_changed_delay_moves_the_date() {
    let (mut w, t) = base();
    // the minimum (1 day) is accepted by set_limits and honoured; below it set_limits itself refuses
    let ix = set_limits_ix(both(&w.v.e), 86_399, CAP, WINDOW);
    assert_err(&w.v.e.send_as_admins(&[ix]), E::InvalidLimits);
    w.set_limits(86_400, CAP, WINDOW);
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + 86_399, &payer));
    // an admin raising the delay mid-wait moves the real date out (documented consequence)
    w.set_limits(3 * 86_400, CAP, WINDOW);
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + 86_400, &payer));
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + 3 * 86_400 - 1, &payer));
    // lowering it brings the date in
    w.set_limits(86_400, CAP, WINDOW);
    w.ok(|w| w.finalize(t + 86_400, &payer));
    assert_eq!(w.account().phase, 3);
}

#[test]
fn finalize_anyone_can_pay() {
    // an unrelated payer, the trader, and the evaluator each work as the payer
    for who in 0..3 {
        let (mut w, t) = base();
        pend2(&mut w, 2, t);
        let payer = match who {
            0 => w.new_payer(),
            1 => dup(&w.trader),
            _ => dup(&w.v.evaluator),
        };
        let lamports_before = w.v.e.svm.get_balance(&payer.pubkey()).unwrap();
        w.ok(|w| w.finalize(t + DELAY, &payer));
        assert_eq!(w.account().phase, 3);
        assert!(w.v.e.svm.get_balance(&payer.pubkey()).unwrap() < lamports_before, "the payer pays the claim rent");
    }
}

#[test]
fn finalize_held_is_refused_until_release_and_the_delay_clock_keeps_running() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.edit_account(|a| a.held = 1);
    w.fails(E::AccountHeld, |w| w.finalize(t + DELAY, &payer));
    w.fails(E::AccountHeld, |w| w.finalize(t + 3 * DELAY, &payer));
    // release_hold (M7) is not built: the helper writes the account; the long-elapsed delay does not restart
    w.edit_account(|a| a.held = 0);
    w.ok(|w| w.finalize(t + 3 * DELAY, &payer));
    assert_eq!(w.account().phase, 3);
}

#[test]
fn finalize_paused_sector_is_refused() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.pause_sector(true);
    w.fails(E::Paused, |w| w.finalize(t + DELAY, &payer));
    w.pause_sector(false);
    w.ok(|w| w.finalize(t + DELAY, &payer));
}

#[test]
fn finalize_with_the_vault_product_paused_rolls_everything_back() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.pause_vault_product();
    let tally0 = w.tally_bytes();
    let cfg0 = w.v.e.config();
    let before = w.pass_digest();
    let r = w.finalize(t + DELAY, &payer);
    assert!(vault_log_has(&r, "ProductNotActive"), "the vault refuses: {:?}", r.as_ref().err().map(|f| f.meta.logs.clone()));
    assert_eq!(w.pass_digest(), before);
    assert_eq!(w.tally_bytes(), tally0);
    assert_eq!((w.v.e.config().window_start, w.v.e.config().window_total), (cfg0.window_start, cfg0.window_total));
    assert_eq!((w.account().pass_pending, w.account().phase, w.account().next_request_id), (2, 2, 1));
}

#[test]
fn finalize_rechecks_the_pass_condition_and_the_phase() {
    let (mut w, t) = base();
    pend2(&mut w, 2, t);
    let payer = w.new_payer();
    w.edit_account(|a| a.b = TARGET - 1);
    w.fails(E::TargetNotReached, |w| w.finalize(t + DELAY, &payer));
    w.edit_account(|a| {
        a.b = TARGET;
        a.flat = 0;
    });
    w.fails(E::NotFlat, |w| w.finalize(t + DELAY, &payer));
    w.edit_account(|a| {
        a.flat = 1;
        a.q_tally = 2_800_000_001;
    });
    w.fails(E::PhaseFailed, |w| w.finalize(t + DELAY, &payer));
    // a phase-1 account can never be in state 2: no amount, no payout
    w.edit_account(|a| {
        a.q_tally = 0;
        a.phase = 1;
    });
    w.fails(E::WrongPhase, |w| w.finalize(t + DELAY, &payer));
    w.edit_account(|a| {
        a.phase = 2;
        a.status = c::STATUS_FAILED;
    });
    w.fails(E::AccountNotActive, |w| w.finalize(t + DELAY, &payer));
}

// ================================================================ daily cap (DESIGN 10.6)

/// An account in state 2 at `at`, and the payer.
fn confirmed(w: &mut JEnv, phase: u8, at: i64) -> Keypair {
    pend2(w, phase, at);
    w.new_payer()
}

#[test]
fn cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    w.set_limits(172_800, PAYOUT_100K - 1, WINDOW);
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(t + DELAY, &payer));
    w.set_limits(172_800, PAYOUT_100K, WINDOW);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    let cfg = w.v.e.config();
    assert_eq!((cfg.window_total, cfg.window_start), (PAYOUT_100K, t + DELAY));
}

#[test]
fn cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second() {
    let (mut w, t) = base();
    // cap = one payout; a long window so two finalizes fit inside it; delay 1 day
    w.set_limits(86_400, PAYOUT_100K, 400_000);
    let payer = confirmed(&mut w, 2, t);
    let t1 = t + 86_400;
    w.ok(|w| w.finalize(t1, &payer));
    let start = w.v.e.config().window_start;
    assert_eq!(start, t1);
    // the next phase passes at once
    let t2 = t1 + 10;
    w.ready(3);
    w.set_pending(c::PASS_CONFIRMED, t2 - 10, t2);
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(t2 + 86_400, &payer));
    // one second before the window ends: still the old window
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(start + 400_000 - 1, &payer));
    // exactly at window_start + window_secs the window rolls
    w.ok(|w| w.finalize(start + 400_000, &payer));
    let cfg = w.v.e.config();
    assert_eq!((cfg.window_start, cfg.window_total), (start + 400_000, PAYOUT_100K));
}

#[test]
fn cap_allows_up_to_twice_the_cap_across_a_window_boundary() {
    let (mut w, t) = base();
    w.set_limits(86_400, PAYOUT_100K, 400_000);
    let payer = confirmed(&mut w, 2, t);
    let t1 = t + 86_400;
    w.ok(|w| w.finalize(t1, &payer));
    let end = w.v.e.config().window_start + 400_000;
    // second payout one second before the boundary is refused; one at the boundary is paid: 2 x cap within 1 s of
    // the boundary is possible by design (fixed windows)
    w.ready(3);
    w.set_pending(c::PASS_CONFIRMED, end - 100_100, end - 100_000);
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(end - 1, &payer));
    w.ok(|w| w.finalize(end, &payer));
    assert_eq!(w.tally().requested_total, 2 * PAYOUT_100K);
    assert_eq!(w.claim(1).unwrap().owed + w.claim(2).unwrap().owed, 2 * PAYOUT_100K);
}

#[test]
fn cap_smaller_than_one_payout_blocks_every_finalize() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    w.set_limits(172_800, 1, WINDOW);
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(t + DELAY, &payer));
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(t + 30 * DELAY, &payer));
}

#[test]
fn cap_window_total_does_not_change_when_nothing_is_paid() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    // a failed finalize (delay) leaves the rolled-over window untouched
    let before = w.v.e.config();
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + 3, &payer));
    let after = w.v.e.config();
    assert_eq!((after.window_start, after.window_total), (before.window_start, before.window_total));
}

// ================================================================ PayoutTally

#[test]
fn tally_layout_is_the_25_bytes_of_the_contract() {
    let (mut w, t) = base();
    assert_eq!(w.tally_bytes().len(), 25);
    assert_eq!(&w.tally_bytes()[..9], b"SL8TALLY\x01");
    let payer = confirmed(&mut w, 2, t);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    let d = w.tally_bytes();
    assert_eq!(d.len(), 25);
    assert_eq!(&d[..8], b"SL8TALLY");
    assert_eq!(d[8], 1);
    assert_eq!(u64::from_le_bytes(d[9..17].try_into().unwrap()), 1);
    assert_eq!(u64::from_le_bytes(d[17..25].try_into().unwrap()), PAYOUT_100K);
}

#[test]
fn tally_only_goes_up_and_equals_what_the_vault_queued() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let mut at = t;
    let mut last = (0u64, 0u64);
    for k in 1..=4u64 {
        let phase = u8::try_from(k + 1).unwrap();
        w.ready(phase);
        w.set_pending(c::PASS_CONFIRMED, at, at);
        at += DELAY;
        w.ok(|w| w.finalize(at, &payer));
        let tl = w.tally();
        assert!(tl.requested_count > last.0 && tl.requested_total > last.1, "strictly increasing");
        assert_eq!((tl.requested_count, tl.requested_total), (k, k * PAYOUT_100K));
        last = (tl.requested_count, tl.requested_total);
        // the vault's own books agree with the tally
        assert_eq!(w.registry_totals(), (k, k * PAYOUT_100K));
        assert_eq!(w.claim(k).unwrap().owed, PAYOUT_100K);
        at += 1;
    }
}

#[test]
fn tally_is_not_touched_by_anything_but_a_paid_finalize() {
    let (mut w, t) = base();
    let t0 = w.tally_bytes();
    w.ready(2);
    w.ok(|w| w.request(t));
    w.ok(|w| w.confirm(t + 1));
    w.ok(|w| w.veto(t + 2));
    w.ok(|w| w.request(t + 3));
    w.ok(|w| w.reject(t + 4, 3));
    w.ok(|w| w.request(t + 5));
    w.ok(|w| w.cancel(t + 5 + 3_600));
    assert_eq!(w.tally_bytes(), t0);
}

#[test]
fn tally_overflow_is_refused_and_changes_nothing() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    let mut data = vec![0u8; 25];
    si_write(&mut data, u64::MAX, 0);
    w.v.set_raw(&tally_pda(), data, PROGRAM_ID);
    w.fails(E::MathOverflow, |w| w.finalize(t + DELAY, &payer));
    let mut data = vec![0u8; 25];
    si_write(&mut data, 3, u64::MAX - (PAYOUT_100K - 1));
    w.v.set_raw(&tally_pda(), data, PROGRAM_ID);
    w.fails(E::MathOverflow, |w| w.finalize(t + DELAY, &payer));
    // exactly fitting is fine
    let mut data = vec![0u8; 25];
    si_write(&mut data, 3, u64::MAX - PAYOUT_100K);
    w.v.set_raw(&tally_pda(), data, PROGRAM_ID);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    assert_eq!((w.tally().requested_count, w.tally().requested_total), (4, u64::MAX));
}

fn si_write(buf: &mut [u8], count: u64, total: u64) {
    setl8_shared_interfaces::PayoutTally { requested_count: count, requested_total: total }.write_into(buf).unwrap();
}

#[test]
fn tally_account_that_does_not_parse_is_refused() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    let mut data = vec![0u8; 25];
    si_write(&mut data, 0, 0);
    data[0] = b'X';
    w.v.set_raw(&tally_pda(), data, PROGRAM_ID);
    w.fails(E::PayoutTallyInvalid, |w| w.finalize(t + DELAY, &payer));
    w.v.set_raw(&tally_pda(), vec![0u8; 24], PROGRAM_ID);
    w.fails(E::PayoutTallyInvalid, |w| w.finalize(t + DELAY, &payer));
}

#[test]
fn tally_wrong_owner_or_address_is_refused() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    let mut data = vec![0u8; 25];
    si_write(&mut data, 0, 0);
    // right bytes, wrong owner
    w.v.set_raw(&tally_pda(), data.clone(), solana_pubkey::Pubkey::new_unique());
    w.fails_anchor(2004, |w| w.finalize(t + DELAY, &payer)); // ConstraintOwner
    w.v.set_raw(&tally_pda(), data.clone(), PROGRAM_ID);
    // another account in the tally slot
    let other = solana_pubkey::Pubkey::new_unique();
    w.v.set_raw(&other, data, PROGRAM_ID);
    w.fails_anchor(2006, |w| {
        w.set_time(t + DELAY);
        let mut f = w.finalize_accts(payer.pubkey());
        f.payout_tally = other;
        let p = dup(&payer);
        w.v.e.send(&[f.to_ix()], &[&p])
    });
}

// ================================================================ return data and Abandoned in finalize

#[test]
fn finalize_abandoned_by_the_vault_changes_neither_tally_nor_window() {
    let (mut w, t) = base();
    // a delay longer than the vault's 7 days idle limit: the vault flips the challenge on the payout request
    w.set_limits(691_200, CAP, WINDOW);
    let payer = confirmed(&mut w, 2, t);
    let tally0 = w.tally_bytes();
    let cfg0 = w.v.e.config();
    let regs0 = w.registry_totals();
    let m = w.ok(|w| w.finalize(t + 691_200, &payer));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending, a.phase, a.next_request_id), (c::STATUS_ABANDONED, 0, 2, 1));
    assert_eq!(w.wallet_profile().active_count, 0);
    assert_eq!(w.vault_status().status, 3);
    assert_eq!(w.tally_bytes(), tally0);
    let cfg = w.v.e.config();
    assert_eq!((cfg.window_start, cfg.window_total), (cfg0.window_start, cfg0.window_total));
    assert_eq!(w.registry_totals(), regs0);
    assert_eq!(w.claim(1), None, "no claim for an abandoned challenge");
    assert_eq!(cpi_events::<AccountAbandoned>(&m).len(), 1);
    assert!(cpi_events::<PassFinalized>(&m).is_empty());
}

// ================================================================ item 8: post_journal and the pass state

#[test]
fn a_confirmed_pass_freezes_post_journal_and_a_requested_one_does_not() {
    let (mut w, t) = base();
    w.set_limits(172_800, CAP, WINDOW);
    w.ready(2);
    w.edit_account(|a| a.b = S100K); // so a +8,000 close reaches the target through the real journal
    let b = simple(&w, t, vec![close_long_pnl(t, t - 100, 8_000_000_000)], true);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, TARGET);
    w.ok(|w| w.request(t + 2));
    // state 1: the evaluator may still post (to catch the chain up)
    let b = simple(&w, t + 3, vec![fee(MARKET_EUR, 0, t + 3)], true);
    w.post_ok(t + 4, &b, &[MARKET_EUR]);
    w.ok(|w| w.confirm(t + 5));
    // state 2: frozen
    let b = simple(&w, t + 6, vec![fee(MARKET_EUR, 0, t + 6)], true);
    w.post_rejected(t + 7, &b, &[MARKET_EUR], E::PassPending);
    // a veto thaws it
    w.ok(|w| w.veto(t + 8));
    let b = simple(&w, t + 9, vec![fee(MARKET_EUR, 0, t + 9)], true);
    w.post_ok(t + 10, &b, &[MARKET_EUR]);
}

// ================================================================ finalize: the next phase and graduation

#[test]
fn finalize_next_phase_is_fresh_with_the_same_field_list_as_confirm() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 3, t);
    w.edit_account(|a| {
        a.latch = 1;
        a.q_tally = 1_234;
        a.lock_until = t + 90_000;
        a.trades_closed = 9;
        a.seq = 31;
        a.journal_hash = [0x5A; 32];
        a.last_batch_time = t - 3;
        a.vetoes = 2;
        a.next_request_id = 2;
        a.last_activity_pushed = t - 9;
    });
    // vault side: payout 1 already happened for this challenge
    // (so the vault's expected id for the next request is 2); emulate by finalizing phase 2 first
    let (mut w, t) = base();
    let payer2 = confirmed(&mut w, 2, t);
    w.ok(|w| w.finalize(t + DELAY, &payer2));
    let _ = payer;
    w.ready(3);
    w.set_pending(c::PASS_CONFIRMED, t + DELAY + 10, t + DELAY + 20);
    w.edit_account(|a| {
        a.latch = 1;
        a.q_tally = 1_234;
        a.lock_until = t + 900_000;
        a.trades_closed = 9;
        a.seq = 31;
        a.journal_hash = [0x5A; 32];
        a.last_batch_time = t - 3;
        a.vetoes = 2;
        a.last_activity_pushed = t - 9;
    });
    let before = w.account();
    w.ok(|w| w.finalize(t + 2 * DELAY + 20, &payer2));
    let a = w.account();
    assert_eq!((a.b, a.q_tally, a.latch, a.tight_on, a.r, a.g_peak, a.l_level, a.flat), (S100K, 0, 0, 0, 0, 0, 0, 1));
    assert_eq!((a.phase, a.pass_pending, a.pass_requested_at, a.pass_proposed_at, a.status), (4, 0, 0, 0, c::STATUS_ACTIVE));
    assert_eq!((a.lock_until, a.trades_closed, a.seq, a.journal_hash, a.last_batch_time), (t + 900_000, 9, 31, [0x5A; 32], t - 3));
    assert_eq!((a.vetoes, a.last_activity_pushed, a.next_request_id), (2, t - 9, before.next_request_id + 1));
}

/// One account through phases 1 to 5 with REAL closes posted through `post_journal` (exact pnl), checking the vault claims,
/// the tally, the wallet slot and the vault's own state after every phase.
#[test]
fn full_journey_phases_1_to_5_then_graduation_and_a_new_account() {
    let (mut w, t0) = base();
    w.set_limits(86_400, CAP, WINDOW);
    let payer = w.new_payer();
    let mut t = t0;
    // phase 1: close +8,000 -> b == 1.08 S exactly; request; confirm -> fresh phase 2, no claim
    let reach = |w: &mut JEnv, t: i64| {
        let b = simple(w, t, vec![close_long_pnl(t, t - 100, 8_000_000_000)], true);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(w.account().b, TARGET);
    };
    reach(&mut w, t);
    w.ok(|w| w.request(t + 10));
    w.ok(|w| w.confirm(t + 20));
    let a = w.account();
    assert_eq!((a.phase, a.b, a.latch, a.next_request_id), (2, S100K, 0, 1));
    assert_eq!((w.claim(1), w.tally().requested_count, w.wallet_profile().active_count, w.vault_status().payout_count), (None, 0, 1, 0));
    t += 30;

    for phase in 2..=5u8 {
        let k = u64::from(phase) - 1; // payouts after this phase
        reach(&mut w, t);
        let m = w.ok(|w| w.request(t + 10));
        assert_eq!(cpi_events::<PassRequested>(&m)[0].amount, PAYOUT_100K);
        w.ok(|w| w.confirm(t + 20));
        assert_eq!(w.account().pass_pending, 2, "frozen for the delay");
        // trading is frozen during the delay
        let frozen = simple(&w, t + 30, vec![fee(MARKET_EUR, 0, t + 30)], true);
        w.post_rejected(t + 31, &frozen, &[MARKET_EUR], E::PassPending);
        let fin_at = t + 20 + 86_400;
        w.fails(E::PassDelayNotElapsed, |w| w.finalize(fin_at - 1, &payer));
        let m = w.ok(|w| w.finalize(fin_at, &payer));
        let ev = cpi_events::<PassFinalized>(&m);
        assert_eq!((ev[0].amount, ev[0].request_id, ev[0].phase), (PAYOUT_100K, k, phase));
        // vault claim, tally and counters after this phase
        assert_eq!(w.claim(k).unwrap().owed, PAYOUT_100K, "claim {k}");
        assert_eq!(w.claim(k).unwrap().trader_wallet, w.trader.pubkey());
        assert_eq!(w.claim(k + 1), None);
        let tl = w.tally();
        assert_eq!((tl.requested_count, tl.requested_total), (k, k * PAYOUT_100K));
        assert_eq!(w.registry_totals(), (k, k * PAYOUT_100K));
        assert_eq!(w.vault_status().payout_count, k);
        let a = w.account();
        assert_eq!(a.next_request_id, k + 1);
        if phase < 5 {
            assert_eq!((a.phase, a.status, a.b, a.latch, a.q_tally, a.pass_pending), (phase + 1, c::STATUS_ACTIVE, S100K, 0, 0, 0));
            assert_eq!(w.wallet_profile().active_count, 1);
            assert_eq!(w.vault_status().status, 0, "vault: Active");
        } else {
            // graduation: local and vault agree
            assert_eq!((a.status, a.pass_pending), (c::STATUS_GRADUATED, 0));
            assert_eq!(a.phase, 5);
            assert_eq!(w.wallet_profile().active_count, 0);
            assert_eq!(w.vault_status().status, 1, "vault: Graduated");
            assert_eq!(w.vault_status().payout_count, 4);
            assert_eq!(ev[0].to_phase, 0);
        }
        t = fin_at + 30;
    }
    assert_eq!(w.tally().requested_total, 4 * PAYOUT_100K);

    // the graduated account accepts nothing more
    w.fails(E::AccountNotActive, |w| w.request(t));
    w.fails(E::AccountNotActive, |w| w.confirm(t));
    // the wallet slot is free: a new account opens
    let tr = dup(&w.trader);
    assert_ok(w.v.open(&tr, 8));
    assert_eq!(w.wallet_profile().active_count, 1);
    assert_eq!(w.wallet_profile().next_challenge_id, 2);
}

#[test]
fn the_vaults_fifth_payout_is_not_reachable() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let mut at = t;
    for phase in 2..=5u8 {
        w.ready(phase);
        w.set_pending(c::PASS_CONFIRMED, at, at);
        at += DELAY;
        w.ok(|w| w.finalize(at, &payer));
        at += 1;
    }
    assert_eq!(w.account().status, c::STATUS_GRADUATED);
    // even with the local account forced back to a confirmed pass, the vault (Graduated) refuses a fifth claim and the
    // whole transaction reverts
    w.edit_account(|a| {
        a.status = c::STATUS_ACTIVE;
        a.phase = 5;
        a.pass_pending = c::PASS_CONFIRMED;
        a.pass_proposed_at = at - DELAY;
        a.b = TARGET;
        a.flat = 1;
        a.next_request_id = 5;
    });
    let before = w.pass_digest();
    let r = w.finalize(at, &payer);
    assert!(vault_log_has(&r, "InvalidTraderStatus"), "{:?}", r.err().map(|f| f.meta.logs));
    assert_eq!(w.pass_digest(), before);
    assert_eq!(w.tally().requested_count, 4);
    assert_eq!(w.claim(5), None);
}

#[test]
fn a_wrong_request_id_is_refused_by_the_vault() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    w.edit_account(|a| a.next_request_id = 2);
    let before = w.pass_digest();
    let r = w.finalize(t + DELAY, &payer);
    assert!(vault_log_has(&r, "RequestIdMismatch"));
    assert_eq!(w.pass_digest(), before);
}

// ================================================================ attacks (DESIGN 13.2, pass path)

#[test]
fn attack_no_instruction_takes_an_amount_and_the_evaluator_has_no_payout_path() {
    // finalize_pass has no arguments at all: its data is the 8-byte discriminator
    let (mut w, t) = base();
    let payer = w.new_payer();
    let ix = w.finalize_accts(payer.pubkey()).to_ix();
    assert_eq!(ix.data.len(), 8);
    for ix in [w.request_ix(), w.cancel_ix(), w.confirm_ix(payer.pubkey())] {
        assert_eq!(ix.data.len(), 8);
    }
    // the evaluator cannot request an advance (not the trader) ...
    let ev = dup(&w.v.evaluator);
    w.ready(2);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t);
        let mut ix = w.request_ix();
        ix.accounts[0].pubkey = ev.pubkey();
        w.v.e.send(&[ix], &[&ev])
    });
    // ... and as a payer it can only finalize what the delay and the cap allow, for the formula amount
    pend2(&mut w, 2, t);
    w.fails(E::PassDelayNotElapsed, |w| w.finalize(t + 1, &ev));
    w.ok(|w| w.finalize(t + DELAY, &ev));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
    // a direct vault call with the evaluator in the sector_authority slot is refused by the vault
    let (mut w, t) = base();
    let vts = w.vts();
    let ix = setl8_shared_interfaces::request_payout(
        VAULT_ID,
        ev.pubkey(),
        registry_pda(),
        &[
            anchor_lang::solana_program::instruction::AccountMeta::new(vts, false),
            anchor_lang::solana_program::instruction::AccountMeta::new(w.v.vault_state, false),
            anchor_lang::solana_program::instruction::AccountMeta::new(claim_pda(&vts, 1), false),
            anchor_lang::solana_program::instruction::AccountMeta::new(ev.pubkey(), true),
            anchor_lang::solana_program::instruction::AccountMeta::new_readonly(anchor_lang::solana_program::system_program::ID, false),
        ],
        setl8_shared_interfaces::RequestPayoutArgs {
            trader_wallet: w.trader.pubkey(),
            amount: PAYOUT_100K,
            product_program_id: PROGRAM_ID,
            challenge_id: 0,
            proposed_request_id: 1,
        },
    );
    w.set_time(t);
    let r = w.v.e.send(&[ix], &[&ev]);
    assert!(vault_log_has(&r, "Unauthorized"), "{:?}", r.err().map(|f| f.meta.logs));
    assert_eq!(w.claim(1), None);
}

#[test]
fn attack_the_admin_pair_cannot_confirm_or_finalize_through_veto() {
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    let before = w.account();
    // veto only ever lowers the state; it never reaches state 2 and never pays
    w.ok(|w| w.veto(t + 1));
    assert_eq!(w.account().pass_pending, 0);
    assert_eq!(w.account().phase, before.phase);
    assert_eq!(w.claim(1), None);
    assert_eq!(w.tally().requested_count, 0);
    // from state 0 a veto does nothing at all
    w.fails(E::NoPendingPass, |w| w.veto(t + 2));
}

#[test]
fn attack_a_trader_cancelling_against_the_evaluators_decision() {
    // the evaluator rejects first: the late cancel finds nothing pending
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.fails(E::CancelTooEarly, |w| w.cancel(t + 100));
    w.ok(|w| w.reject(t + 200, c::REJECT_OTHER));
    w.fails(E::NoPendingPass, |w| w.cancel(t + 4_000));
    // the trader cancels after the hour: the late confirm and reject find nothing pending
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.ok(|w| w.cancel(t + 3_600));
    w.fails(E::NoPendingPass, |w| w.confirm(t + 3_601));
    w.fails(E::NoPendingPass, |w| w.reject(t + 3_601, 0));
    // the evaluator confirms first: the trader can no longer cancel the confirmed pass
    let (mut w, t) = base();
    pend1(&mut w, 2, t);
    w.ok(|w| w.confirm(t + 10));
    w.fails(E::NoPendingPass, |w| w.cancel(t + 4_000));
}

#[test]
fn attack_double_finalize() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    w.fails(E::NoPendingPass, |w| w.finalize(t + DELAY, &payer));
    w.fails(E::NoPendingPass, |w| w.finalize(t + DELAY + 1, &payer));
    assert_eq!(w.tally().requested_count, 1);
    // even with the pending state forged back, the vault's own id check stops a second claim for the same id
    w.edit_account(|a| {
        a.phase = 2;
        a.pass_pending = c::PASS_CONFIRMED;
        a.pass_proposed_at = t;
        a.next_request_id = 1;
        a.b = TARGET;
    });
    let before = w.pass_digest();
    let r = w.finalize(t + DELAY + 2, &payer);
    assert!(vault_log_has(&r, "RequestIdMismatch") || vault_log_has(&r, "already in use"), "{:?}", r.err().map(|f| f.meta.logs));
    assert_eq!(w.pass_digest(), before);
}

#[test]
fn attack_finalize_with_another_traders_accounts() {
    let (mut w, t) = base();
    let payer = confirmed(&mut w, 2, t);
    // a second trader with an account of its own
    let other = w.v.new_trader();
    assert_ok(w.v.open(&other, 8));
    let other_vts = vault_trader_state_pda(&other.pubkey(), 0);
    let other_acct = acct_pda(&other.pubkey(), 0);
    let at = t + DELAY;
    let run = |w: &mut JEnv, f: &dyn Fn(&mut FinalizeAccts)| {
        w.set_time(at);
        let mut a = w.finalize_accts(payer.pubkey());
        f(&mut a);
        let p = dup(&payer);
        w.v.e.send(&[a.to_ix()], &[&p])
    };
    // each swap must fail without changing anything (vault seeds / our seeds / owner constraints)
    type Swap = Box<dyn Fn(&mut FinalizeAccts)>;
    let cases: Vec<(&str, Swap)> = vec![
        ("other trader_state", Box::new(move |a| a.vault_trader_state = other_vts)),
        ("other payout_claim", Box::new(move |a| a.payout_claim = claim_pda(&other_vts, 1))),
        ("claim of a wrong id", Box::new(|a| a.payout_claim = claim_pda(&a.vault_trader_state, 2))),
        ("other wallet", Box::new(move |a| a.wallet = wallet_pda(&solana_pubkey::Pubkey::new_unique()))),
        ("wrong sector authority", Box::new(|a| a.sector_authority = solana_pubkey::Pubkey::new_unique())),
        ("wrong registry", Box::new(|a| a.product_registry = solana_pubkey::Pubkey::new_unique())),
        ("wrong vault state", Box::new(|a| a.vault_state = solana_pubkey::Pubkey::new_unique())),
        ("trader state in the claim slot", Box::new(|a| a.payout_claim = a.vault_trader_state)),
        ("vault state and trader state swapped", Box::new(|a| std::mem::swap(&mut a.vault_state, &mut a.vault_trader_state))),
        ("config swapped for the tally", Box::new(|a| a.config = tally_pda())),
    ];
    for (name, f) in cases {
        let before = w.pass_digest();
        let r = run(&mut w, &*f);
        assert!(r.is_err(), "{name} must fail");
        assert_eq!(w.pass_digest(), before, "{name} changed the world");
    }
    // a wrong vault program id is its own error
    w.fails(E::InvalidVaultProgram, |w| run(w, &|a| a.vault_program = solana_pubkey::Pubkey::new_unique()));
    // another trader's TRADING account in the first slot: the wallet, vault records and claim are not its own
    w.fails_anchor(2006, |w| run(w, &move |a| a.trading_account = other_acct));
    // the real call still works afterwards
    w.ok(|w| run(w, &|_| {}));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
}

// ================================================================ compute units

#[test]
fn compute_units_per_instruction() {
    let (mut w, t) = base();
    w.ready(2);
    let payer = w.new_payer();
    let t_req = w.t0 + 86_401; // vault Recorded path
    let cu = |m: &litesvm::types::TransactionMetadata| m.compute_units_consumed;
    let request = cu(&w.ok(|w| w.request(t_req)));
    let confirm = cu(&w.ok(|w| w.confirm(t_req + 1)));
    let veto = cu(&w.ok(|w| w.veto(t_req + 2)));
    w.ok(|w| w.request(t_req + 3));
    let reject = cu(&w.ok(|w| w.reject(t_req + 4, 0)));
    w.ok(|w| w.request(t_req + 5));
    let cancel = cu(&w.ok(|w| w.cancel(t_req + 5 + 3_600)));
    w.ok(|w| w.request(t_req + 4_000));
    w.ok(|w| w.confirm(t_req + 4_001));
    let finalize = cu(&w.ok(|w| w.finalize(t_req + 4_001 + DELAY, &payer)));
    // phase 1 confirm (starts phase 2)
    pend1(&mut w, 1, t);
    let confirm1 = cu(&w.ok(|w| w.confirm(t_req + 10 * DELAY)));
    println!("CU request_advance {request}, confirm_pass(2..5) {confirm}, confirm_pass(1) {confirm1}, reject_pass {reject}, cancel_pass {cancel}, veto_pass {veto}, finalize_pass {finalize}");
    for c in [request, confirm, confirm1, reject, cancel, veto, finalize] {
        assert!(c < 200_000, "default budget");
    }
}
