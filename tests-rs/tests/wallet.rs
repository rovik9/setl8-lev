mod common;

use anchor_lang::solana_program::pubkey::Pubkey;
use common::*;
use setl8_lev_trading::{
    errors::Setl8Error,
    events::{WalletBanned, WalletUnbanned},
};
use solana_keypair::Keypair;
use solana_signer::Signer;

const EVIDENCE: [u8; 32] = [0xE5; 32];

fn ban(e: &mut Env, admins: Admins, trader: Pubkey, evidence: [u8; 32]) -> litesvm::types::TransactionResult {
    let ix = ban_wallet_ix(e.payer.pubkey(), admins, trader, evidence);
    e.send_admins(ix, admins)
}

fn unban(e: &mut Env, admins: Admins, trader: Pubkey) -> litesvm::types::TransactionResult {
    let ix = unban_wallet_ix(admins, trader);
    e.send_admins(ix, admins)
}

#[test]
fn ban_wallet_creates_a_56_byte_profile_with_the_exact_layout() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    let logs = ban(&mut e, a, trader, EVIDENCE).expect("ban succeeds").logs;
    let ev = events_of::<WalletBanned>(&logs);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].trader, ev[0].evidence_hash), (trader, EVIDENCE));

    let acc = e.svm.get_account(&wallet_pda(&trader)).unwrap();
    assert_eq!(acc.owner, PROGRAM_ID);
    assert_eq!(acc.data.len(), 56);
    let bump = Pubkey::find_program_address(&[b"wallet", trader.as_ref()], &PROGRAM_ID).1;
    assert_eq!(&acc.data[8..40], trader.as_ref()); // trader
    assert_eq!(&acc.data[40..48], &0u64.to_le_bytes()); // next_challenge_id
    assert_eq!(acc.data[48], 0); // active_count
    assert_eq!(acc.data[49], bump); // bump
    assert_eq!(acc.data[50], 1); // banned
    assert_eq!(&acc.data[51..56], &[0u8; 5]); // pad
    let w = e.wallet(&trader);
    assert_eq!((w.trader, w.banned), (trader, 1));
}

#[test]
fn ban_wallet_requires_both_admins() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let (x, y) = (Keypair::new(), Keypair::new());
    for a in [Admins::only_sl8(&e), Admins::only_rov(&e), Admins::neither(&e)] {
        assert_err(&ban(&mut e, a, trader, EVIDENCE), Setl8Error::Unauthorized);
    }
    let ix = ban_wallet_ix(e.payer.pubkey(), Admins::strangers(&x, &y), trader, EVIDENCE);
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    assert!(e.svm.get_account(&wallet_pda(&trader)).is_none());
}

#[test]
fn ban_wallet_with_zero_evidence_fails() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_err(&ban(&mut e, a, trader, [0; 32]), Setl8Error::EvidenceMissing);
    assert!(e.svm.get_account(&wallet_pda(&trader)).is_none());
}

#[test]
fn ban_wallet_is_idempotent() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    let first = e.svm.get_account(&wallet_pda(&trader)).unwrap();
    let logs = ban(&mut e, a, trader, [0x77; 32]).expect("second ban succeeds").logs;
    assert_eq!(e.svm.get_account(&wallet_pda(&trader)).unwrap().data, first.data);
    assert_eq!(e.wallet(&trader).banned, 1);
    assert_eq!(events_of::<WalletBanned>(&logs)[0].evidence_hash, [0x77; 32]);
}

#[test]
fn ban_wallet_keeps_counters_of_an_existing_profile() {
    // a profile created by a later module may carry counters; banning must not reset them.
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    assert_ok(unban(&mut e, a, trader));
    let mut acc = e.svm.get_account(&wallet_pda(&trader)).unwrap();
    acc.data[40..48].copy_from_slice(&9u64.to_le_bytes());
    acc.data[48] = 3;
    e.svm.set_account(wallet_pda(&trader), acc).unwrap();
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    let w = e.wallet(&trader);
    assert_eq!((w.next_challenge_id, w.active_count, w.banned), (9, 3, 1));
}

#[test]
fn unban_wallet_clears_banned_and_emits_the_event() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    let logs = unban(&mut e, a, trader).expect("unban succeeds").logs;
    assert_eq!(events_of::<WalletUnbanned>(&logs)[0].trader, trader);
    assert_eq!(e.wallet(&trader).banned, 0);
    // the profile stays; it can be banned again
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    assert_eq!(e.wallet(&trader).banned, 1);
}

#[test]
fn unban_wallet_requires_both_admins() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_ok(ban(&mut e, a, trader, EVIDENCE));
    let (x, y) = (Keypair::new(), Keypair::new());
    for a in [Admins::only_sl8(&e), Admins::only_rov(&e), Admins::neither(&e)] {
        assert_err(&unban(&mut e, a, trader), Setl8Error::Unauthorized);
    }
    let ix = unban_wallet_ix(Admins::strangers(&x, &y), trader);
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    assert_eq!(e.wallet(&trader).banned, 1);
}

#[test]
fn unban_wallet_fails_when_the_wallet_has_no_profile() {
    let mut e = Env::initialised();
    let trader = Pubkey::new_unique();
    let a = Admins::both(&e);
    assert_anchor_code(&unban(&mut e, a, trader), ACCOUNT_NOT_INITIALIZED);
    assert!(e.svm.get_account(&wallet_pda(&trader)).is_none());
}
