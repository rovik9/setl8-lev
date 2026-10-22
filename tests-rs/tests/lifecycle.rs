//! M7: report_fail, hold, release, violation, activity, close, and the post_journal static-floor hardening, against the
//! real vault harness. Every failure test asserts the exact error code and an unchanged world digest.
mod common;

use anchor_lang::AccountSerialize;
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::{acct_pda, vault_trader_state_pda};
use common::*;
use journal_types::FailReason;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E, events::*, state::TradingAccount};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

const CAP: u64 = 100_000_000_000;
const DELAY: i64 = 172_800;
const SR: u8 = 0; // StaticFloor
const PQ: u8 = 1; // PhaseQuick

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(172_800, CAP, 86_400);
    (w, t)
}

fn bytes_of(a: &TradingAccount) -> Vec<u8> {
    let mut d = Vec::new();
    a.try_serialize(&mut d).unwrap();
    d
}

fn assert_only(before: &TradingAccount, after: &TradingAccount, f: impl FnOnce(&mut TradingAccount)) {
    use anchor_lang::AccountDeserialize;
    let mut exp = TradingAccount::try_deserialize(&mut bytes_of(before).as_slice()).unwrap();
    f(&mut exp);
    assert_eq!(bytes_of(&exp), bytes_of(after));
}

/// `report_fail` of a `[pre..., Fail, Meta]` batch with the EUR market passed.
fn rf(w: &mut JEnv, t: i64, pre: Vec<journal_types::JournalEvent>, reason: FailReason, b_after: i64, flat: bool, arg: u8) -> litesvm::types::TransactionResult {
    let b = w.fail_batch(t, pre, reason, b_after, flat);
    w.report_fail(t + 1, &b, arg, &[MARKET_EUR])
}

/// A static-floor fail of the account as it is.
fn rf_floor(w: &mut JEnv, t: i64) -> litesvm::types::TransactionResult {
    let b = w.account().b;
    rf(w, t, vec![], FailReason::StaticFloor, b, true, SR)
}

fn at_floor(w: &mut JEnv) {
    w.edit_account(|a| a.b = FLOOR);
}

// ================================================================ report_fail: StaticFloor

#[test]
fn rf_static_floor_boundary_is_inclusive() {
    let (mut w, t) = base();
    w.edit_account(|a| a.b = FLOOR + 1);
    w.fails(E::FailStaticFloorNotReached, |w| rf_floor(w, t));
    w.edit_account(|a| a.b = FLOOR);
    w.ok(|w| rf_floor(w, t));
    assert_eq!(w.account().status, c::STATUS_FAILED);
}

#[test]
fn rf_latched_floor_is_096_s_and_an_unlatched_account_keeps_092_s() {
    let (mut w, t) = base();
    w.edit_account(|a| {
        a.latch = 1;
        a.b = LATCHED_FLOOR + 1;
    });
    w.fails(E::FailStaticFloorNotReached, |w| rf_floor(w, t));
    w.edit_account(|a| a.b = LATCHED_FLOOR);
    w.ok(|w| rf_floor(w, t));
    // not latched: 96,000 is far above the 92,000 floor
    let (mut w, t) = base();
    w.edit_account(|a| a.b = LATCHED_FLOOR);
    w.fails(E::FailStaticFloorNotReached, |w| rf_floor(w, t));
}

#[test]
fn rf_latch_reached_inside_the_batch_counts() {
    // +4,000 latches (b 104,000), then -8,000 lands on 96,000 == the latched floor
    let (mut w, t) = base();
    let pre = |second: i64| vec![close_long_pnl(t, t - 100, 4_000_000_000), close_long_pnl(t, t - 100, second)];
    w.fails(E::FailStaticFloorNotReached, |w| rf(w, t, pre(-7_999_999_999), FailReason::StaticFloor, LATCHED_FLOOR + 1, true, SR));
    w.ok(|w| rf(w, t, pre(-8_000_000_000), FailReason::StaticFloor, LATCHED_FLOOR, true, SR));
    let a = w.account();
    assert_eq!((a.b, a.latch, a.status, a.trades_closed), (LATCHED_FLOOR, 1, c::STATUS_FAILED, 2));
}

#[test]
fn rf_not_flat_wrong_reason_and_balance_mismatch_are_refused() {
    let (mut w, t) = base();
    at_floor(&mut w);
    w.fails(E::FailNotFlat, |w| rf(w, t, vec![], FailReason::StaticFloor, FLOOR, false, SR));
    w.fails(E::FailReasonMismatch, |w| rf(w, t, vec![], FailReason::StaticFloor, FLOOR, true, PQ));
    w.fails(E::FailReasonMismatch, |w| rf(w, t, vec![], FailReason::PhaseQuick, FLOOR, true, SR));
    for bad in [2u8, 3, 255] {
        w.fails(E::FailReasonInvalid, |w| rf(w, t, vec![], FailReason::StaticFloor, FLOOR, true, bad));
    }
    for d in [1i64, -1] {
        w.fails(E::FailBalanceMismatch, |w| rf(w, t, vec![], FailReason::StaticFloor, FLOOR + d, true, SR));
    }
}

#[test]
fn rf_batch_shape_fail_must_be_right_before_meta_and_unique() {
    let (mut w, t) = base();
    at_floor(&mut w);
    let f = |t: i64| fail_ev(t, FailReason::StaticFloor, FLOOR, 0);
    let go = |w: &mut JEnv, evs: Vec<journal_types::JournalEvent>| {
        let b = w.batch(evs, vec![p_eur(t)]);
        w.report_fail(t + 1, &b, SR, &[MARKET_EUR])
    };
    // a later event after the Fail
    w.fails(E::FailNotBeforeMeta, |w| go(w, vec![f(t), fee(MARKET_EUR, 0, t), meta(t, true)]));
    // two Fails
    w.fails(E::FailTwice, |w| go(w, vec![f(t), f(t), meta(t, true)]));
    // no Fail at all
    w.fails(E::FailMissing, |w| go(w, vec![fee(MARKET_EUR, 0, t), meta(t, true)]));
    // Meta missing / not last / Meta twice
    w.fails(E::MetaMissing, |w| go(w, vec![f(t)]));
    w.fails(E::MetaNotLast, |w| go(w, vec![meta(t, true), f(t)]));
    w.fails(E::MetaTwice, |w| go(w, vec![f(t), meta(t, true), meta(t, true)]));
    // the Fail is the only event before Meta: accepted
    w.ok(|w| go(w, vec![f(t), meta(t, true)]));
}

#[test]
fn rf_wrong_signer_and_inactive_accounts_are_refused() {
    let (mut w, t) = base();
    at_floor(&mut w);
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, FLOOR, true);
    let tr = dup(&w.trader);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.report_fail_ix(tr.pubkey(), b.to_bytes().unwrap(), SR, &[MARKET_EUR]);
        w.v.e.send(&[ix], &[&tr])
    });
    w.fails_anchor(3010, |w| {
        w.set_time(t + 1);
        let mut ix = w.report_fail_ix(dup(&w.v.evaluator).pubkey(), b.to_bytes().unwrap(), SR, &[MARKET_EUR]);
        ix.accounts[rf::EVALUATOR].is_signer = false;
        w.v.e.send(&[ix], &[])
    });
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        w.edit_account(|a| a.status = status);
        w.fails(E::AccountNotActive, |w| rf_floor(w, t));
    }
}

#[test]
fn rf_the_fail_event_needs_a_valid_proof() {
    let (mut w, t) = base();
    at_floor(&mut w);
    // no proof
    let b = w.batch(vec![fail_ev(t, FailReason::StaticFloor, FLOOR, 255), meta(t, true)], vec![]);
    w.fails(E::ProofMissing, |w| w.report_fail(t + 1, &b, SR, &[MARKET_EUR]));
    // a feed no passed market has
    let b = w.batch(vec![fail_ev(t, FailReason::StaticFloor, FLOOR, 0), meta(t, true)], vec![proof(MARKET_BTC, t, MID)]);
    w.fails(E::ProofWrongFeed, |w| w.report_fail(t + 1, &b, SR, &[MARKET_EUR]));
    // too old
    let b = w.batch(vec![fail_ev(t, FailReason::StaticFloor, FLOOR, 0), meta(t, true)], vec![p_eur(t - 61)]);
    w.fails(E::ProofTooOld, |w| w.report_fail(t + 1, &b, SR, &[MARKET_EUR]));
}

// ================================================================ report_fail: PhaseQuick

#[test]
fn rf_quick_tally_boundary_is_strict() {
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT);
    w.fails(E::FailQuickNotExceeded, |w| rf(w, t, vec![], FailReason::PhaseQuick, S100K, true, PQ));
    w.edit_account(|a| a.q_tally = QUICK_LIMIT + 1);
    w.ok(|w| rf(w, t, vec![], FailReason::PhaseQuick, S100K, true, PQ));
    assert_eq!(w.account().status, c::STATUS_FAILED);
}

#[test]
fn rf_quick_overshoot_inside_the_batch_forces_the_fail() {
    // a quick profit of 5,600.000002 is credited 2,800.000001 > 0.028 S
    let (mut w, t) = base();
    let over = |t: i64| close_long_pnl(t, t - 10, 5_600_000_002);
    let b_after = S100K + 2_800_000_001;
    // post_journal refuses the same close (I-8) ...
    let pj = simple(&w, t, vec![over(t)], true);
    w.post_rejected(t + 1, &pj, &[MARKET_EUR], E::QuickLimitExceeded);
    // ... the exact limit is fine there: 5,600.000001 is credited 2,800.000000
    let pj = simple(&w, t, vec![close_long_pnl(t, t - 10, 5_600_000_001)], true);
    w.post_ok(t + 1, &pj, &[MARKET_EUR]);
    let (mut w, t) = base();
    // nothing may sit between the overshoot and the Fail
    w.fails(E::QuickLimitExceeded, |w| rf(w, t, vec![over(t), fee(MARKET_EUR, 0, t)], FailReason::PhaseQuick, b_after, true, PQ));
    // a StaticFloor reason is refused while the quick limit is exceeded
    w.fails(E::QuickLimitNeedsPhaseQuick, |w| rf(w, t, vec![over(t)], FailReason::StaticFloor, b_after, true, SR));
    // the exact fail
    w.ok(|w| rf(w, t, vec![over(t)], FailReason::PhaseQuick, b_after, true, PQ));
    let a = w.account();
    assert_eq!((a.status, a.b, a.q_tally), (c::STATUS_FAILED, b_after, 2_800_000_001));
}

#[test]
fn rf_each_reason_needs_its_own_condition() {
    // only the quick limit exceeded: StaticFloor is not justified (and is refused for the quick overshoot first)
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = QUICK_LIMIT + 1);
    w.fails(E::QuickLimitNeedsPhaseQuick, |w| rf(w, t, vec![], FailReason::StaticFloor, S100K, true, SR));
    // only the floor reached: PhaseQuick is not justified
    let (mut w, t) = base();
    at_floor(&mut w);
    w.fails(E::FailQuickNotExceeded, |w| rf(w, t, vec![], FailReason::PhaseQuick, FLOOR, true, PQ));
    // both at once: the quick reason is the one that is accepted
    w.edit_account(|a| a.q_tally = QUICK_LIMIT + 1);
    w.fails(E::QuickLimitNeedsPhaseQuick, |w| rf(w, t, vec![], FailReason::StaticFloor, FLOOR, true, SR));
    w.ok(|w| rf(w, t, vec![], FailReason::PhaseQuick, FLOOR, true, PQ));
}

// ================================================================ report_fail: effects

#[test]
fn rf_effects_status_terminal_at_vault_wallet_events_and_chain() {
    let (mut w, t) = base();
    at_floor(&mut w);
    w.set_pending(c::PASS_REQUESTED, t - 50, 0);
    assert_eq!(w.wallet_profile().active_count, 1);
    let batch = w.fail_batch(t, vec![fee(MARKET_EUR, 0, t)], FailReason::StaticFloor, FLOOR, true);
    let head = w.expected_head(&batch).0;
    let m = w.ok(|w| w.report_fail(t + 5, &batch, SR, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending, a.flat, a.b, a.seq), (c::STATUS_FAILED, 0, 1, FLOOR, 3));
    assert_eq!(a.terminal_at(), t + 5, "terminal_at is the block time");
    assert_eq!(a.journal_hash, head);
    assert_eq!(a.last_batch_time, t);
    assert_eq!(w.wallet_profile().active_count, 0);
    assert_eq!(w.vault_status().status, 2, "vault: Failed");
    let ev = cpi_events::<AccountFailed>(&m);
    assert_eq!((ev.len(), ev[0].account, ev[0].reason, ev[0].b, ev[0].seq, ev[0].journal_hash), (1, w.acct, 0, FLOOR, 3, head));
    let logs = event_logs(&m);
    assert_eq!(logs.len(), 3);
    assert_eq!(chain_head_from_logs(&logs, &w.acct), a.journal_hash, "verify_chain over the emitted EventLogs");
    // the banned flag is NOT set by a floor fail (only fail_for_violation bans)
    assert_eq!(w.wallet_profile().banned, 0);
}

#[test]
fn rf_is_refused_in_state_2_allowed_in_state_1_and_a_failed_account_is_dead() {
    let (mut w, t) = base();
    at_floor(&mut w);
    w.set_pending(c::PASS_CONFIRMED, t - 50, t - 40);
    w.fails(E::PassPending, |w| rf_floor(w, t));
    w.set_pending(c::PASS_REQUESTED, t - 50, 0);
    w.ok(|w| rf_floor(w, t));
    // afterwards: no journal, no pass request, no second fail
    let pj = simple(&w, t + 10, vec![], true);
    w.post_rejected(t + 11, &pj, &[MARKET_EUR], E::AccountNotActive);
    w.fails(E::AccountNotActive, |w| w.request(t + 12));
    w.fails(E::AccountNotActive, |w| {
        let b = w.fail_batch(t + 13, vec![], FailReason::StaticFloor, FLOOR, true);
        w.report_fail(t + 14, &b, SR, &[MARKET_EUR])
    });
}

#[test]
fn rf_frees_a_slot_so_the_wallet_can_open_again_up_to_the_cap_of_five() {
    let (mut w, t) = base();
    let first = w.acct;
    for _ in 0..4 {
        w.open_another();
    }
    assert_eq!(w.wallet_profile().active_count, 5);
    let tr = dup(&w.trader);
    assert_err(&w.v.open(&tr, 8), E::WalletCapReached);
    w.acct = first;
    at_floor(&mut w);
    w.ok(|w| rf_floor(w, t));
    assert_eq!(w.wallet_profile().active_count, 4);
    assert_ok(w.v.open(&tr, 8));
    assert_eq!(w.wallet_profile().active_count, 5);
    assert_err(&w.v.open(&tr, 8), E::WalletCapReached);
}

#[test]
fn rf_works_while_the_sector_and_the_vault_product_are_paused() {
    let (mut w, t) = base();
    at_floor(&mut w);
    w.pause_sector(true);
    w.pause_vault_product();
    w.ok(|w| rf_floor(w, t));
    assert_eq!((w.account().status, w.vault_status().status), (c::STATUS_FAILED, 2));
}

#[test]
fn rf_on_a_held_account_keeps_the_hold_and_the_wallet_count() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    assert_eq!(w.wallet_profile().held_count, 1);
    at_floor(&mut w);
    w.ok(|w| rf_floor(w, t));
    let a = w.account();
    // P25c (decision 67 amendment): the hold STAYS on a Failed account, for the two admins' review
    assert_eq!((a.held, a.status), (1, c::STATUS_FAILED));
    assert_eq!(w.wallet_profile().held_count, 1);
}

// ================================================================ post_journal hardening (RESOLVE d)

/// `post_journal` of `[Close(pnl)]` + Meta; expects `want` (at instruction 1, digest unchanged) or success.
fn pj_close(w: &mut JEnv, t: i64, pnl: i64, flat: bool, want: Option<E>) {
    let b = simple(w, t, vec![close_long_pnl(t, t - 100, pnl)], flat);
    match want {
        Some(e) => w.post_rejected(t + 1, &b, &[MARKET_EUR], e),
        None => {
            w.post_ok(t + 1, &b, &[MARKET_EUR]);
        }
    }
}

#[test]
fn pj_a_flat_batch_at_or_below_the_static_floor_must_use_report_fail() {
    let (mut w, t) = base();
    // b would end exactly on 92,000: refused; below: refused; one micro above: accepted
    pj_close(&mut w, t, -8_000_000_000, true, Some(E::MustReportFail));
    pj_close(&mut w, t, -8_000_000_001, true, Some(E::MustReportFail));
    pj_close(&mut w, t, -7_999_999_999, true, None);
    assert_eq!(w.account().b, FLOOR + 1);
}

#[test]
fn pj_the_latched_floor_is_096_s() {
    let (mut w, t) = base();
    w.edit_account(|a| a.latch = 1);
    pj_close(&mut w, t, -4_000_000_000, true, Some(E::MustReportFail));
    pj_close(&mut w, t, -3_999_999_999, true, None);
    assert_eq!(w.account().b, LATCHED_FLOOR + 1);
    // unlatched: 96,000 is fine
    let (mut w, t) = base();
    pj_close(&mut w, t, -4_000_000_000, true, None);
    assert_eq!(w.account().b, LATCHED_FLOOR);
}

#[test]
fn pj_not_flat_may_end_below_the_floor_because_open_profit_can_keep_equity_up() {
    let (mut w, t) = base();
    let b = simple(&w, t, vec![close_long_pnl(t, t - 100, -8_000_000_000)], false);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, FLOOR);
    // the floor is also checked at the END of the batch: going down and coming back is fine
    let (mut w, t) = base();
    let b = simple(&w, t, vec![close_long_pnl(t, t - 100, -8_000_000_000), close_long_pnl(t, t - 100, 1_000_000_000)], true);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, FLOOR + 1_000_000_000);
}

// ================================================================ hold / release / violation

#[test]
fn hold_preconditions_each_violated_alone() {
    let (mut w, t) = base();
    let tr = dup(&w.trader);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t);
        let ix = w.hold_ix(tr.pubkey(), hash(1));
        w.v.e.send(&[ix], &[&tr])
    });
    w.fails(E::EvidenceMissing, |w| w.hold(t, no_hash()));
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        w.edit_account(|a| a.status = status);
        w.fails(E::AccountNotActive, |w| w.hold(t, hash(1)));
    }
    w.edit_account(|a| a.status = c::STATUS_ACTIVE);
    let before = w.account();
    let m = w.ok(|w| w.hold(t, hash(7)));
    assert_only(&before, &w.account(), |x| x.held = 1);
    assert_eq!(w.wallet_profile().held_count, 1);
    let ev = cpi_events::<AccountHeld>(&m);
    assert_eq!((ev.len(), ev[0].account, ev[0].trader, ev[0].evidence_hash), (1, w.acct, w.trader.pubkey(), hash(7)));
    w.fails(E::AlreadyHeld, |w| w.hold(t + 1, hash(8)));
    assert_eq!(w.wallet_profile().held_count, 1);
}

#[test]
fn hold_works_with_a_pending_pass_in_both_states_and_changes_nothing_else() {
    for state in [c::PASS_REQUESTED, c::PASS_CONFIRMED] {
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(state, t - 20, if state == c::PASS_CONFIRMED { t - 10 } else { 0 });
        let before = w.account();
        w.ok(|w| w.hold(t, hash(1)));
        assert_only(&before, &w.account(), |x| x.held = 1);
    }
}

#[test]
fn a_held_account_refuses_fills_and_passes_but_allows_closes_and_fees() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    let b = simple(&w, t + 1, vec![fill_long(t + 1, 0)], false);
    w.post_rejected(t + 2, &b, &[MARKET_EUR], E::AccountHeld);
    let b = simple(&w, t + 1, vec![fee(MARKET_EUR, 5, t + 1), close_long_pnl(t + 1, t - 100, 100)], true);
    w.post_ok(t + 2, &b, &[MARKET_EUR]);
    // pass flow (M6) with a real hold
    w.ready(2);
    w.fails(E::AccountHeld, |w| w.request(t + 3));
    w.set_pending(c::PASS_REQUESTED, t, 0);
    w.fails(E::AccountHeld, |w| w.confirm(t + 3));
}

#[test]
fn a_hold_stops_finalize_and_the_delay_clock_keeps_running_across_the_release() {
    let (mut w, t) = base();
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t - 10, t);
    let payer = w.new_payer();
    w.ok(|w| w.hold(t + 5, hash(1)));
    // the delay is over but the account is held
    w.fails(E::AccountHeld, |w| w.finalize(t + DELAY, &payer));
    // released well after the delay: finalize works at once, `pass_proposed_at` was not touched
    w.ok(|w| w.release_hold(t + DELAY + 100));
    assert_eq!(w.account().pass_proposed_at, t);
    w.ok(|w| w.finalize(t + DELAY + 101, &payer));
    assert_eq!(w.account().phase, 3);
}

#[test]
fn release_hold_needs_both_admins_and_a_held_account() {
    let (mut w, t) = base();
    w.fails(E::NotHeld, |w| w.release_hold(t));
    w.ok(|w| w.hold(t, hash(1)));
    for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e), Admins::neither(&w.v.e)] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 1);
            let ix = w.release_hold_ix(admins);
            w.v.e.send_admins(ix, admins)
        });
    }
    let before = w.account();
    let m = w.ok(|w| w.release_hold(t + 2));
    assert_only(&before, &w.account(), |x| x.held = 0);
    assert_eq!(w.wallet_profile().held_count, 0);
    assert_eq!(cpi_events::<HoldReleased>(&m).len(), 1);
    // a second release finds nothing held
    w.fails(E::NotHeld, |w| w.release_hold(t + 3));
}

#[test]
fn fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault() {
    let (mut w, t) = base();
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t - 10, t - 5);
    w.ok(|w| w.hold(t, hash(1)));
    // preconditions
    w.fails(E::EvidenceMissing, |w| w.violation(t + 1, no_hash()));
    for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e), Admins::neither(&w.v.e)] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 1);
            let ix = w.violation_ix(admins, hash(9));
            w.v.e.send_admins(ix, admins)
        });
    }
    let m = w.ok(|w| w.violation(t + 2, hash(9)));
    let a = w.account();
    assert_eq!((a.status, a.pass_pending, a.held), (c::STATUS_FAILED, c::PASS_VIOLATION, 0));
    assert!(a.violation_failed());
    assert_eq!(a.terminal_at(), t + 2);
    let wl = w.wallet_profile();
    assert_eq!((wl.banned, wl.active_count, wl.held_count), (1, 0, 0));
    assert_eq!(w.vault_status().status, 2, "vault: Failed");
    let ev = cpi_events::<ViolationFail>(&m);
    assert_eq!((ev.len(), ev[0].account, ev[0].trader, ev[0].evidence_hash), (1, w.acct, w.trader.pubkey(), hash(9)));
    assert_eq!(w.claim(1), None, "no payout");
    // the banned wallet cannot open
    let tr = dup(&w.trader);
    assert_err(&w.v.open(&tr, 8), E::WalletBanned);
    // unban restores opening but does not revive the account
    let admins = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tr.pubkey()), admins));
    assert_ok(w.v.open(&tr, 8));
    assert_eq!(w.account().status, c::STATUS_FAILED);
}

#[test]
fn fail_for_violation_needs_a_held_account_and_works_while_paused() {
    let (mut w, t) = base();
    w.fails(E::NotHeld, |w| w.violation(t, hash(1)));
    w.ok(|w| w.hold(t, hash(1)));
    w.pause_sector(true);
    w.pause_vault_product();
    w.ok(|w| w.violation(t + 1, hash(2)));
    assert_eq!(w.account().status, c::STATUS_FAILED);
    // a violation-failed account is Failed and no longer held: a second call is refused as NotHeld (P25c: a Failed account is a
    // valid target when it is still held, so the hold is what is missing; an Abandoned or Graduated one is AccountNotActive)
    w.fails(E::NotHeld, |w| w.violation(t + 2, hash(2)));
}

#[test]
fn held_count_bookkeeping_across_every_path_and_ban_wallet() {
    let (mut w, t) = base();
    let tr = dup(&w.trader);
    let first = w.acct;
    let second_id = w.open_another();
    assert_eq!(second_id, 1);
    let second = w.acct;
    // hold both
    w.acct = first;
    w.ok(|w| w.hold(t, hash(1)));
    w.acct = second;
    w.ok(|w| w.hold(t, hash(2)));
    assert_eq!(w.wallet_profile().held_count, 2);
    // ban_wallet is refused while a held account exists
    let admins = Admins::both(&w.v.e);
    let payer = w.v.e.payer.pubkey();
    let ban = |w: &mut JEnv| w.v.e.send_admins(ban_wallet_ix(payer, admins, tr.pubkey(), hash(5)), admins);
    w.fails(E::WalletHasHeldAccount, |w| ban(w));
    // release one: still refused
    w.acct = first;
    w.ok(|w| w.release_hold(t + 1));
    assert_eq!(w.wallet_profile().held_count, 1);
    w.fails(E::WalletHasHeldAccount, |w| ban(w));
    // violation on the other: count back to 0
    w.acct = second;
    w.ok(|w| w.violation(t + 2, hash(3)));
    let wl = w.wallet_profile();
    assert_eq!((wl.held_count, wl.banned), (0, 1));
    // now ban_wallet is allowed (idempotent on an already banned wallet)
    assert_ok(ban(&mut w));
    assert_eq!(w.wallet_profile().banned, 1);
    // an unrelated wallet with no profile can still be banned
    let stranger = Pubkey::new_unique();
    assert_ok(w.v.e.send_admins(ban_wallet_ix(payer, admins, stranger, hash(5)), admins));
}

#[test]
fn a_held_account_that_is_abandoned_releases_its_hold_count() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    assert_eq!(w.wallet_profile().held_count, 1);
    w.ok(|w| w.release_acct(w.t0 + 604_801));
    let a = w.account();
    assert_eq!((a.status, a.held), (c::STATUS_ABANDONED, 0));
    assert_eq!(w.wallet_profile().held_count, 0);
}

// ================================================================ touch_activity / release_account

#[test]
fn touch_boundary_is_86000_seconds_skipped_at_85999_pushed_at_86000() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    let v0 = w.vault_status().last_activity_timestamp;
    w.edit_account(|a| a.last_activity_pushed = now - 85_999);
    w.ok(|w| w.touch(now));
    assert_eq!(w.vault_status().last_activity_timestamp, v0, "skipped: no vault call");
    assert_eq!(w.account().last_activity_pushed, now - 85_999);
    w.edit_account(|a| a.last_activity_pushed = now - 86_000);
    w.ok(|w| w.touch(now));
    assert_eq!(w.vault_status().last_activity_timestamp, now, "pushed");
    assert_eq!(w.account().last_activity_pushed, now);
}

#[test]
fn release_account_has_the_same_boundary_and_a_skipped_call_is_a_harmless_success() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    let v0 = w.vault_status().last_activity_timestamp;
    w.edit_account(|a| a.last_activity_pushed = now - 85_999);
    let before = w.account();
    w.ok(|w| w.release_acct(now));
    assert_eq!(w.vault_status().last_activity_timestamp, v0);
    assert_only(&before, &w.account(), |_| {});
    w.edit_account(|a| a.last_activity_pushed = now - 86_000);
    w.ok(|w| w.release_acct(now));
    assert_eq!((w.vault_status().last_activity_timestamp, w.account().last_activity_pushed), (now, now));
}

#[test]
fn a_throttled_vault_answer_does_not_move_last_activity_pushed() {
    let (mut w, t) = base();
    // the vault saw activity at t0 (open); our stored push is old, so the CPI runs, and the vault says Throttled
    w.edit_account(|a| a.last_activity_pushed = t - 90_000);
    w.ok(|w| w.touch(t));
    assert_eq!(w.account().last_activity_pushed, t - 90_000);
    assert_eq!(w.vault_status().last_activity_timestamp, w.t0);
}

#[test]
fn activity_signers_evaluator_for_touch_trader_for_release() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    let (ev, tr, stranger) = (dup(&w.v.evaluator), dup(&w.trader), Keypair::new());
    for who in [&tr, &stranger] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(now);
            let ix = w.touch_ix(who.pubkey());
            w.v.e.send(&[ix], &[who])
        });
    }
    for who in [&ev, &stranger] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(now);
            let ix = w.release_acct_ix(who.pubkey());
            w.v.e.send(&[ix], &[who])
        });
    }
    w.fails_anchor(3010, |w| {
        w.set_time(now);
        let mut ix = w.release_acct_ix(tr.pubkey());
        ix.accounts[0].is_signer = false;
        w.v.e.send(&[ix], &[])
    });
    w.ok(|w| w.touch(now));
}

#[test]
fn abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass() {
    for via_touch in [true, false] {
        let (mut w, _) = base();
        w.set_pending(c::PASS_REQUESTED, w.t0, 0);
        // idle exactly 7 days is not stale (Recorded); one second more is
        let limit = w.t0 + 604_800;
        let m = w.ok(|w| if via_touch { w.touch(limit + 1) } else { w.release_acct(limit + 1) });
        let a = w.account();
        assert_eq!((a.status, a.pass_pending), (c::STATUS_ABANDONED, 0));
        assert_eq!(a.terminal_at(), limit + 1);
        assert_eq!(w.wallet_profile().active_count, 0);
        assert_eq!(w.vault_status().status, 3, "vault: Abandoned");
        assert_eq!(cpi_events::<AccountAbandoned>(&m).len(), 1);
    }
    let (mut w, _) = base();
    let limit = w.t0 + 604_800;
    w.ok(|w| w.touch(limit));
    assert_eq!((w.account().status, w.account().last_activity_pushed), (c::STATUS_ACTIVE, limit));
}

#[test]
fn a_challenge_already_abandoned_by_the_permissionless_vault_call_is_learned_locally() {
    for via_touch in [true, false] {
        let (mut w, _) = base();
        let t = w.t0 + 604_801;
        assert_ok(w.vault_mark_abandoned(t));
        assert_eq!(w.vault_status().status, 3);
        assert_eq!(w.account().status, c::STATUS_ACTIVE, "we do not know yet");
        w.ok(|w| if via_touch { w.touch(t + 1) } else { w.release_acct(t + 1) });
        assert_eq!(w.account().status, c::STATUS_ABANDONED);
        assert_eq!(w.wallet_profile().active_count, 0);
    }
}

#[test]
fn activity_on_terminal_or_non_active_accounts_is_refused() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        w.edit_account(|a| a.status = status);
        w.fails(E::AccountNotActive, |w| w.touch(now));
        w.fails(E::AccountNotActive, |w| w.release_acct(now));
    }
}

#[test]
fn activity_with_a_wrong_vault_trader_state_is_refused() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    let other = w.v.new_trader();
    assert_ok(w.v.open(&other, 8));
    let other_vts = vault_trader_state_pda(&other.pubkey(), 0);
    w.fails(E::InvalidVaultAccount, |w| {
        w.set_time(now);
        let ev = dup(&w.v.evaluator);
        let mut ix = w.touch_ix(ev.pubkey());
        ix.accounts[7].pubkey = other_vts;
        w.v.e.send(&[ix], &[&ev])
    });
    // a system account in the slot
    w.fails(E::InvalidVaultAccount, |w| {
        w.set_time(now);
        let tr = dup(&w.trader);
        let mut ix = w.release_acct_ix(tr.pubkey());
        ix.accounts[7].pubkey = Pubkey::new_unique();
        w.v.e.send(&[ix], &[&tr])
    });
}

// ================================================================ close_account

#[test]
fn close_is_refused_for_an_active_account_even_when_held() {
    let (mut w, t) = base();
    w.fails(E::AccountNotTerminal, |w| w.close(t + 10 * RETENTION));
    w.ok(|w| w.hold(t, hash(1)));
    w.fails(E::AccountNotTerminal, |w| w.close(t + 10 * RETENTION));
}

#[test]
fn close_boundary_is_terminal_at_plus_seven_days_for_every_terminal_status() {
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        let (mut w, t) = base();
        w.make_terminal(status, t);
        w.fails(E::CloseTooEarly, |w| w.close(t + RETENTION - 1));
        let before = w.v.e.svm.get_balance(&w.trader.pubkey()).unwrap();
        let rent = w.v.e.svm.get_account(&w.acct).unwrap().lamports;
        assert_eq!(rent, 2_331_600);
        w.ok(|w| w.close(t + RETENTION));
        // the whole rent goes to the trader (the fee payer is another account)
        assert_eq!(w.v.e.svm.get_balance(&w.trader.pubkey()).unwrap(), before + rent);
        assert!(w.v.e.svm.get_account(&w.acct).is_none_or(|a| a.lamports == 0 || a.data.is_empty()));
    }
}

#[test]
fn close_other_signer_refused_closing_twice_impossible_and_the_next_open_gets_the_next_id() {
    let (mut w, t) = base();
    w.make_terminal(c::STATUS_FAILED, t);
    let stranger = Keypair::new();
    w.v.e.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + RETENTION);
        let mut ix = w.close_ix(stranger.pubkey());
        ix.accounts[0].pubkey = stranger.pubkey();
        w.v.e.send(&[ix], &[&stranger])
    });
    let closed = w.acct;
    w.ok(|w| w.close(t + RETENTION));
    // twice: the account no longer exists
    w.set_time(t + RETENTION + 1);
    let tr = dup(&w.trader);
    let ix = w.close_ix(tr.pubkey());
    assert!(w.v.e.send(&[ix], &[&tr]).is_err());
    // the next account of the wallet is challenge 1, at another address; the closed address is never reused
    assert_eq!(w.wallet_profile().next_challenge_id, 1);
    let id = w.open_another();
    assert_eq!(id, 1);
    assert_ne!(w.acct, closed);
    assert_eq!(w.acct, acct_pda(&w.trader.pubkey(), 1));
    assert!(w.v.e.svm.get_account(&closed).is_none_or(|a| a.data.is_empty()));
}

// ================================================================ attacks (DESIGN 13.2)

#[test]
fn attack_the_evaluator_cannot_release_a_hold_or_fail_for_violation() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    let ev = dup(&w.v.evaluator);
    let other = Keypair::new();
    let admins = Admins::strangers(&ev, &other);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.release_hold_ix(admins);
        w.v.e.send(&[ix], &[&ev, &other])
    });
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.violation_ix(admins, hash(2));
        w.v.e.send(&[ix], &[&ev, &other])
    });
    // the evaluator cannot fail or ban either: the only evaluator fail is a justified report_fail
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
}

#[test]
fn attack_one_admin_alone_cannot_use_any_both_admin_instruction() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, hash(1)));
    let payer = w.v.e.payer.pubkey();
    let trader = w.trader.pubkey();
    for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e)] {
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 1);
            let ix = w.release_hold_ix(admins);
            w.v.e.send_admins(ix, admins)
        });
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 1);
            let ix = w.violation_ix(admins, hash(2));
            w.v.e.send_admins(ix, admins)
        });
        w.fails(E::Unauthorized, |w| w.v.e.send_admins(ban_wallet_ix(payer, admins, trader, hash(3)), admins));
    }
}

#[test]
fn attack_a_trader_cannot_report_fail_or_hold() {
    let (mut w, t) = base();
    at_floor(&mut w);
    let tr = dup(&w.trader);
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, FLOOR, true);
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.report_fail_ix(tr.pubkey(), b.to_bytes().unwrap(), SR, &[MARKET_EUR]);
        w.v.e.send(&[ix], &[&tr])
    });
    w.fails(E::Unauthorized, |w| {
        w.set_time(t + 1);
        let ix = w.hold_ix(tr.pubkey(), hash(1));
        w.v.e.send(&[ix], &[&tr])
    });
}

#[test]
fn attack_fake_wallet_and_vault_accounts_in_report_fail_and_fail_for_violation() {
    let (mut w, t) = base();
    at_floor(&mut w);
    w.ok(|w| w.hold(t, hash(1)));
    let other = w.v.new_trader();
    assert_ok(w.v.open(&other, 8));
    let o_wallet = wallet_pda(&other.pubkey());
    let o_vts = vault_trader_state_pda(&other.pubkey(), 0);
    let o_acct = acct_pda(&other.pubkey(), 0);
    let junk = Pubkey::new_unique();
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, FLOOR, true);
    // (index, replacement, expected anchor code or our error)
    let swaps: Vec<(usize, Pubkey, Option<E>)> = vec![
        (rf::WALLET, o_wallet, None),
        (rf::WALLET, junk, None),
        (rf::SECTOR_AUTHORITY, junk, None),
        (rf::VAULT_PROGRAM, junk, Some(E::InvalidVaultProgram)),
        (rf::REGISTRY, junk, None),
        (rf::VAULT_TRADER_STATE, o_vts, None),
        (rf::VAULT_TRADER_STATE, junk, None),
        (rf::TRADING_ACCOUNT, o_acct, None),
        (rf::RULESET, junk, None),
        (rf::CONFIG, junk, None),
    ];
    for (idx, key, want) in &swaps {
        let before = w.pass_digest();
        w.set_time(t + 1);
        let ev = dup(&w.v.evaluator);
        let mut ix = w.report_fail_ix(ev.pubkey(), b.to_bytes().unwrap(), SR, &[MARKET_EUR]);
        ix.accounts[*idx].pubkey = *key;
        let r = w.v.e.send(&[ix], &[&ev]);
        match want {
            Some(e) => assert_err(&r, *e),
            None => assert!(r.is_err(), "report_fail swap {idx} must fail"),
        }
        assert_eq!(w.pass_digest(), before, "report_fail swap {idx} changed the world");
    }
    let vswaps: Vec<(usize, Pubkey, Option<E>)> = vec![
        (fv::WALLET, o_wallet, None),
        (fv::SECTOR_AUTHORITY, junk, None),
        (fv::VAULT_PROGRAM, junk, Some(E::InvalidVaultProgram)),
        (fv::REGISTRY, junk, None),
        (fv::VAULT_TRADER_STATE, o_vts, None),
        (fv::TRADING_ACCOUNT, o_acct, None),
        (fv::CONFIG, junk, None),
    ];
    for (idx, key, want) in &vswaps {
        let before = w.pass_digest();
        w.set_time(t + 2);
        let a = Admins::both(&w.v.e);
        let mut ix = w.violation_ix(a, hash(4));
        ix.accounts[*idx].pubkey = *key;
        let r = w.v.e.send_admins(ix, a);
        match want {
            Some(e) => assert_err(&r, *e),
            None => assert!(r.is_err(), "fail_for_violation swap {idx} must fail"),
        }
        assert_eq!(w.pass_digest(), before, "fail_for_violation swap {idx} changed the world");
    }
    // the honest calls still work
    w.ok(|w| w.violation(t + 3, hash(4)));
}

// ================================================================ compute units

#[test]
fn compute_units_per_instruction() {
    let cu = |m: &litesvm::types::TransactionMetadata| m.compute_units_consumed;
    let (mut w, t) = base();
    let hold = cu(&w.ok(|w| w.hold(t, hash(1))));
    let release = cu(&w.ok(|w| w.release_hold(t + 1)));
    let touch_skip = cu(&w.ok(|w| w.touch(t + 2)));
    let now = w.t0 + 100_000;
    let touch = cu(&w.ok(|w| w.touch(now)));
    w.edit_account(|a| a.last_activity_pushed = now - 90_000);
    let release_acct = cu(&w.ok(|w| w.release_acct(now)));
    w.ok(|w| w.hold(now + 1, hash(1)));
    let violation = cu(&w.ok(|w| w.violation(now + 2, hash(2))));
    let (mut w2, t2) = base();
    at_floor(&mut w2);
    let report = cu(&w2.ok(|w| rf_floor(w, t2)));
    // heavy: 8 events with 3 proofs (6 closes/fees + Fail + Meta)
    let (mut w3, t3) = base();
    let pre = vec![
        close_long_pnl(t3, t3 - 100, 10),
        close_long_pnl(t3, t3 - 100, -10),
        close_long_pnl(t3, t3 - 100, 10),
        close_long_pnl(t3, t3 - 100, -10),
        fee(MARKET_EUR, 1, t3),
        fee(MARKET_EUR, 1, t3),
    ];
    w3.edit_account(|a| a.b = FLOOR + 2);
    let batch = w3.fail_batch(t3, pre, FailReason::StaticFloor, FLOOR, true);
    let r = w3.report_fail(t3 + 1, &batch, SR, &[MARKET_EUR]);
    let report8 = match &r {
        Ok(m) => cu(m),
        Err(f) => panic!("{:?}\n{}", f.err, f.meta.logs.join("\n")),
    };
    let (mut w4, t4) = base();
    w4.make_terminal(c::STATUS_FAILED, t4);
    let close = cu(&w4.ok(|w| w.close(t4 + RETENTION)));
    println!("CU report_fail(2 events) {report}, report_fail(8 events,3 proofs) {report8}, hold_account {hold}, release_hold {release}, fail_for_violation {violation}, touch_activity(skipped) {touch_skip}, touch_activity(vault call) {touch}, release_account(vault call) {release_acct}, close_account {close}");
    for c in [hold, release, violation, touch, release_acct, close, touch_skip] {
        assert!(c < 200_000);
    }
    assert!(report < 400_000 && report8 < 600_000);
}
