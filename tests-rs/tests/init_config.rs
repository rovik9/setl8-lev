mod common;

use anchor_lang::solana_program::{instruction::error::InstructionError, pubkey::Pubkey, system_program};
use common::*;
use setl8_lev_trading::{constants as c, errors::Setl8Error};
use litesvm::types::TransactionResult;
use setl8_lev_trading::instructions::InitConfigArgs;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

const TALLY_BYTES: [u8; 25] = {
    let mut b = [0u8; 25];
    let m = *b"SL8TALLY";
    let mut i = 0;
    while i < 8 {
        b[i] = m[i];
        i += 1;
    }
    b[8] = 1;
    b
};

#[test]
fn init_config_succeeds_and_state_matches_args() {
    let mut e = Env::new();
    let args = default_args();
    let ix = e.init_ix(&args);
    assert_ok(e.send_as_admins(&[ix]));

    let acc = e.svm.get_account(&config_pda()).unwrap();
    assert_eq!(acc.data.len(), 270);
    assert_eq!(acc.owner, PROGRAM_ID);
    let cfg = e.config();
    assert_eq!(cfg.admin_a, c::SL8_ADMIN);
    assert_eq!(cfg.admin_b, c::ROV_ADMIN);
    assert_eq!(cfg.admin_a, e.sl8.pubkey());
    assert_eq!(cfg.admin_b, e.rov.pubkey());
    assert_eq!(cfg.evaluator, args.evaluator);
    assert_eq!(cfg.vault_program, args.vault_program);
    assert_eq!(cfg.sl8_wallet, args.sl8_wallet);
    assert_eq!(cfg.current_ruleset, 0);
    assert_eq!(cfg.market_count, 0);
    assert_eq!(cfg.paused, 0);
    assert_eq!(cfg.bump, Pubkey::find_program_address(&[b"config"], &PROGRAM_ID).1);
    assert_eq!(cfg.pass_delay_secs, args.pass_delay_secs);
    assert_eq!(cfg.max_payout_per_window, args.max_payout_per_window);
    assert_eq!(cfg.window_secs, args.window_secs);
    assert_eq!(cfg.window_start, 0);
    assert_eq!(cfg.window_total, 0);
    assert_eq!(cfg.reserved, [0u8; 64]);
}

#[test]
fn init_config_creates_payout_tally_with_exact_bytes() {
    let mut e = Env::new();
    let ix = e.init_ix(&default_args());
    assert_ok(e.send_as_admins(&[ix]));

    let t = e.svm.get_account(&tally_pda()).unwrap();
    // b"SL8TALLY" | version 1 | requested_count u64 LE = 0 | requested_total u64 LE = 0
    let mut want = Vec::new();
    want.extend_from_slice(b"SL8TALLY");
    want.push(1);
    want.extend_from_slice(&0u64.to_le_bytes());
    want.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(t.data, want);
    assert_eq!(t.data, TALLY_BYTES);
    assert_eq!(t.data.len(), 25);
    assert_eq!(t.owner, PROGRAM_ID);
    assert_eq!(tally_pda(), Pubkey::find_program_address(&[b"payout_tally"], &PROGRAM_ID).0);
    assert_eq!(setl8_shared_interfaces::PayoutTally::parse(&t.data).unwrap(), Default::default());
}

#[test]
fn init_config_succeeds_when_tally_address_was_prefunded() {
    // Anyone can send lamports to the tally PDA before init; create_account would then fail.
    let mut e = Env::new();
    let griefer = Keypair::new();
    e.svm.airdrop(&griefer.pubkey(), 1_000_000_000).unwrap();
    let ix = anchor_lang::solana_program::system_instruction::transfer(&griefer.pubkey(), &tally_pda(), 1);
    assert_ok(e.send(&[ix], &[&griefer]));

    let ix = e.init_ix(&default_args());
    assert_ok(e.send_as_admins(&[ix]));
    let t = e.svm.get_account(&tally_pda()).unwrap();
    assert_eq!(t.data, TALLY_BYTES);
    assert_eq!(t.owner, PROGRAM_ID);
    assert!(t.lamports >= e.svm.minimum_balance_for_rent_exemption(25));
}

#[test]
fn init_config_fails_with_only_one_admin_signing() {
    for sl8_signs in [true, false] {
        let mut e = Env::new();
        let mut ix = e.init_ix(&default_args());
        // accounts: payer, sl8_admin, rov_admin, ...
        let missing = if sl8_signs { 2 } else { 1 };
        ix.accounts[missing].is_signer = false;
        let r = e.send_as_admins(&[ix]);
        assert_anchor_code(&r, 3010); // AccountNotSigner
        assert!(e.svm.get_account(&config_pda()).is_none());
        assert!(e.svm.get_account(&tally_pda()).is_none());
    }
}

#[test]
fn init_config_fails_when_a_required_signature_is_missing() {
    // Both admin accounts are flagged as signers but only one actually signed: the transaction is rejected.
    let mut e = Env::new();
    let ix = e.init_ix(&default_args());
    let sl8 = dup(&e.sl8);
    let r = e.send(&[ix], &[&sl8]);
    assert!(r.is_err());
    assert!(e.svm.get_account(&config_pda()).is_none());
}

#[test]
fn init_config_fails_with_two_random_signers() {
    let mut e = Env::new();
    let (a, b) = (Keypair::new(), Keypair::new());
    let ix = init_ix_with(e.payer.pubkey(), a.pubkey(), b.pubkey(), &default_args());
    let r = e.send(&[ix], &[&a, &b]);
    assert_err(&r, Setl8Error::Unauthorized);
    assert!(e.svm.get_account(&config_pda()).is_none());
}

#[test]
fn init_config_fails_when_a_third_key_replaces_an_admin() {
    let mut e = Env::new();
    let third = Keypair::new();
    let (sl8, rov) = (dup(&e.sl8), dup(&e.rov));
    // third key in the Rov slot
    let ix = init_ix_with(e.payer.pubkey(), sl8.pubkey(), third.pubkey(), &default_args());
    assert_err(&e.send(&[ix], &[&sl8, &third]), Setl8Error::Unauthorized);
    // third key in the SL8 slot
    let ix = init_ix_with(e.payer.pubkey(), third.pubkey(), rov.pubkey(), &default_args());
    assert_err(&e.send(&[ix], &[&third, &rov]), Setl8Error::Unauthorized);
    // the two admin slots swapped
    let ix = init_ix_with(e.payer.pubkey(), rov.pubkey(), sl8.pubkey(), &default_args());
    assert_err(&e.send(&[ix], &[&sl8, &rov]), Setl8Error::Unauthorized);
    assert!(e.svm.get_account(&config_pda()).is_none());
}

#[test]
fn second_init_config_fails_and_changes_nothing() {
    let mut e = Env::initialised();
    let cfg_before = e.svm.get_account(&config_pda()).unwrap();
    let tally_before = e.svm.get_account(&tally_pda()).unwrap();

    let mut args = default_args();
    args.evaluator = Pubkey::new_unique();
    args.max_payout_per_window = 1;
    let ix = e.init_ix(&args);
    let r = e.send_as_admins(&[ix]);
    // the system program refuses to create an account that already exists (SystemError::AccountAlreadyInUse = 0)
    match &r {
        Err(f) => assert_eq!(f.err, TransactionError::InstructionError(0, InstructionError::Custom(0))),
        Ok(_) => panic!("second init_config must fail"),
    }
    assert_eq!(e.svm.get_account(&config_pda()).unwrap(), cfg_before);
    assert_eq!(e.svm.get_account(&tally_pda()).unwrap(), tally_before);
}

fn init_with(mutate: impl FnOnce(&mut InitConfigArgs)) -> (Env, TransactionResult) {
    let mut e = Env::new();
    let mut args = default_args();
    mutate(&mut args);
    let ix = e.init_ix(&args);
    let r = e.send_as_admins(&[ix]);
    (e, r)
}

#[test]
fn init_config_rejects_delay_one_second_below_the_minimum() {
    let (e, r) = init_with(|a| a.pass_delay_secs = 86_399);
    assert_err(&r, Setl8Error::InvalidLimits);
    assert!(e.svm.get_account(&config_pda()).is_none());
}

#[test]
fn init_config_rejects_delay_one_second_above_the_maximum() {
    let (e, r) = init_with(|a| a.pass_delay_secs = 1_209_601);
    assert_err(&r, Setl8Error::InvalidLimits);
    assert!(e.svm.get_account(&config_pda()).is_none());
}

#[test]
fn init_config_accepts_the_delay_bounds() {
    for d in [86_400u32, 1_209_600] {
        let (e, r) = init_with(|a| a.pass_delay_secs = d);
        assert_ok(r);
        assert_eq!(e.config().pass_delay_secs, d);
    }
}

#[test]
fn init_config_rejects_zero_cap() {
    let (_, r) = init_with(|a| a.max_payout_per_window = 0);
    assert_err(&r, Setl8Error::InvalidLimits);
}

#[test]
fn init_config_rejects_zero_window() {
    let (_, r) = init_with(|a| a.window_secs = 0);
    assert_err(&r, Setl8Error::InvalidLimits);
}

#[test]
fn init_config_rejects_default_evaluator() {
    let (e, r) = init_with(|a| a.evaluator = system_program::ID);
    assert_err(&r, Setl8Error::Unauthorized);
    assert!(e.svm.get_account(&config_pda()).is_none());
}
