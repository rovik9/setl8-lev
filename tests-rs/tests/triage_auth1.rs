//! Triage of the M12 authorization review claims AUTH-01 .. AUTH-08 (see the notes at each test).
//! AUTH-01, 02, 03, 05, 08: accepted design limits, no test. AUTH-04: pinning test (refuted). AUTH-06, AUTH-07: findings.
mod common;

use common::journal::*;
use common::lifecycle::*;
use common::vault::*;
use common::*;
use journal_types::FailReason;
use setl8_lev_trading::constants as c;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

const CAP: u64 = 100_000_000_000;
const DELAY: i64 = 172_800;

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(DELAY as u32, CAP, 86_400);
    (w, t)
}

/// AUTH-04 (REFUTED for `vault_state`): finalize_pass takes `vault_state` as an unchecked account. With the real vault, any
/// other account in that slot (nonexistent key, the product registry, our own config) is refused and nothing changes; the
/// other vault slots (registry, record, claim) are pinned by vault_boundary.rs `c_*`.
#[test]
fn auth04_finalize_pass_with_a_substituted_vault_state_is_refused_and_changes_nothing() {
    let (mut w, t) = base();
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    let payer = w.new_payer();
    w.set_time(t + DELAY);
    let before = w.pass_digest();
    let p = dup(&payer);
    let good = w.finalize_accts(payer.pubkey());
    for (name, sub) in [("nonexistent key", Pubkey::new_unique()), ("product registry", registry_pda()), ("our config", config_pda())] {
        let mut f = good.clone();
        f.vault_state = sub;
        let r = w.v.e.send(&[f.to_ix()], &[&p]);
        assert!(r.is_err(), "finalize_pass accepted `{name}` as vault_state");
        assert_eq!(w.pass_digest(), before, "`{name}` changed the world");
    }
    // control: the unchanged call pays
    assert!(w.v.e.send(&[good.to_ix()], &[&p]).is_ok());
}

/// FINDING AUTH-06: `init_config` takes `vault_program` unvalidated, and no instruction can change it later (nor `sl8_wallet`
/// nor the admin pair), so a typo bricks every vault-touching instruction until a 2-of-2 upgrade. The correct behaviour here
/// is the cheapest part of the fix: refuse obviously wrong values (the SPL Token program is not a vault; the System program is the default key and already refused). A complete fix is a
/// both-admin setter or a validating account (design change).
#[test]
#[ignore = "FINDING AUTH-06: init_config accepts any non-default vault_program/sl8_wallet and nothing can change them afterwards"]
fn auth06_init_config_refuses_the_token_program_as_vault_program() {
    // FINDING AUTH-06
    let mut e = Env::new();
    let mut args = default_args();
    args.vault_program = anchor_spl::token::ID; // the SPL Token program: an executable program, plainly not the vault
    let ix = e.init_ix(&args);
    let r = e.send_as_admins(&[ix]);
    assert!(r.is_err(), "init_config stored the SPL Token program as the vault program");
}

/// AUTH-07 (audit finding F-32, fixed by P25c, decision 67 amendment): `report_fail` works on a HELD account (a held account is
/// not frozen for losses). It used to clear the hold, so the account became an ordinary resettable failure and `fail_for_violation`
/// (Active and held) could never be applied. Now the account stays Failed + held: `reset_account` is refused with `ReviewPending`
/// until both admins decide, and `fail_for_violation` is accepted on the Failed + held account and leaves the marker.
/// ADAPTED from the original assertions: the original expected `held == 0` after the fail and a refused reset only; the correct
/// behaviour chosen by the owner keeps the hold (the review is not lost) and lets the admins apply the violation afterwards.
#[test]
fn auth07_an_account_failed_while_held_cannot_be_reset_before_the_review() {
    let (mut w, t) = base();
    w.ok(|w| w.hold(t, [1u8; 32]));
    w.edit_account(|a| a.b = FLOOR);
    let b = w.account().b;
    let batch = w.fail_batch(t + 10, vec![], FailReason::StaticFloor, b, true);
    w.ok(|w| w.report_fail(t + 11, &batch, 0, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.held), (c::STATUS_FAILED, 1), "the hold is still there after the fail");
    // the trader cannot reset before the review
    w.fails(setl8_lev_trading::errors::Setl8Error::ReviewPending, |w| w.reset(t + 20));
    // the admins can still apply the violation: the account then carries the marker and is never resettable
    w.ok(|w| w.violation(t + 12, [2u8; 32]));
    let a = w.account();
    assert_eq!((a.status, a.held, a.pass_pending), (c::STATUS_FAILED, 0, c::PASS_VIOLATION));
    assert!(w.reset(t + 30).is_err(), "reset of a violation-failed account must stay refused");
}
