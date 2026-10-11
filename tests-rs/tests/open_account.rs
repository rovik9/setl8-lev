//! `open_account` against the REAL vault (fixture core_vault.so) in one LiteSVM. See tests/common/vault.rs.
mod common;

use anchor_lang::solana_program::pubkey::Pubkey;
use common::vault::*;
use common::*;
use setl8_lev_trading::errors::Setl8Error;
use setl8_shared_interfaces as si;
use sha2::{Digest, Sha256};
use solana_keypair::Keypair;
use solana_signer::Signer;

const T0: i64 = common::T0;
/// DESIGN 9.2: the 0.4% fee in micro-USD for the ten launch tiers (rounded UP).
const FEE_TABLE: [u64; 10] = [120_000, 220_000, 396_000, 580_000, 756_000, 940_000, 1_956_000, 2_756_000, 3_436_000, 6_396_000];

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn ok(r: litesvm::types::TransactionResult) -> litesvm::types::TransactionMetadata {
    match r {
        Ok(m) => m,
        Err(f) => panic!("expected success, got {:?}\n{}", f.err, f.meta.logs.join("\n")),
    }
}

fn assert_failed(r: &litesvm::types::TransactionResult, what: &str) {
    assert!(r.is_err(), "{what}: expected the transaction to fail");
}

// ---------------------------------------------------------------- happy path

#[test]
fn open_account_happy_path_for_every_tier_index() {
    for tier in 0u8..=9 {
        let mut v = VaultEnv::new();
        let trader = v.new_trader();
        let tk = trader.pubkey();
        let ta = v.trader_token(&tk);
        let i = usize::from(tier);
        let (cost, size) = (v.ruleset.tier_cost[i], v.ruleset.tier_size[i]);
        let fee = v.expected_fee(i);
        assert_eq!(fee, FEE_TABLE[i], "tier {tier} fee table");
        let (pool0, vsl8_0, fee0) = (v.token_balance(&v.usdc_pool), v.token_balance(&v.vault_sl8_usdc), v.token_balance(&v.fee_usdc));

        let meta = ok(v.open(&trader, tier));

        // the account, field by field
        let addr = acct_pda(&tk, 0);
        let a = v.account(&addr);
        let raw = v.e.svm.get_account(&addr).unwrap();
        assert_eq!(raw.data.len(), 207);
        assert_eq!(raw.lamports, 2_331_600, "rent for 207 bytes");
        assert_eq!(a.trader, tk);
        assert_eq!((a.challenge_id, a.size_s), (0, size));
        assert_eq!((a.created_at, a.last_batch_time, a.last_activity_pushed), (T0, T0, T0));
        assert_eq!((a.b, a.q_tally, a.lock_until), (i64::try_from(size).unwrap(), 0, 0));
        assert_eq!((a.next_request_id, a.seq, a.trades_closed), (1, 0, 0));
        assert_eq!(a.journal_hash, sha(&[b"SL8LEV", addr.as_ref()]), "h_0 = sha256(\"SL8LEV\" || account pubkey)");
        assert_eq!(a.ruleset_version, 1);
        assert_eq!((a.status, a.phase, a.latch, a.flat), (0, 1, 0, 1));
        assert_eq!((a.pass_pending, a.vetoes, a.tight_on, a.held), (0, 0, 0, 0));
        assert_eq!(a.bump, Pubkey::find_program_address(&[b"acct", tk.as_ref(), &0u64.to_le_bytes()], &PROGRAM_ID).1);
        assert_eq!((a.r, a.g_peak, a.l_level, a.pass_requested_at, a.pass_proposed_at), (0, 0, 0, 0, 0));

        // money: price to the vault (split by the vault's formula), the 0.4% fee to the fee wallet
        assert_eq!(v.token_balance(&ta), TRADER_START - cost - fee, "tier {tier} trader debit");
        let (pool, sl8) = VaultEnv::vault_split(cost);
        assert_eq!(v.token_balance(&v.usdc_pool) - pool0, pool);
        assert_eq!(v.token_balance(&v.vault_sl8_usdc) - vsl8_0, sl8);
        assert_eq!(v.token_balance(&v.fee_usdc) - fee0, fee);

        // the vault's record
        let ts = v.vault_trader_state(&tk, 0).expect("vault TraderState exists");
        assert_eq!(ts.trader_wallet, tk);
        assert_eq!(ts.product_program_id, PROGRAM_ID);
        assert_eq!((ts.challenge_id, ts.account_size, ts.payout_count, ts.status), (0, size, 0, 0));
        assert_eq!(ts.last_activity_timestamp, T0);

        // the wallet profile
        let w = v.wallet_opt(&tk).unwrap();
        assert_eq!((w.trader, w.next_challenge_id, w.active_count, w.banned), (tk, 1, 1, 0));

        // the Opened event (inner instruction, emit_cpi!)
        let ev = opened_events(&meta);
        assert_eq!(ev.len(), 1);
        let e = &ev[0];
        assert_eq!((e.account, e.trader, e.challenge_id, e.size_s), (addr, tk, 0, size));
        assert_eq!((e.tier_index, e.phase, e.ruleset_version, e.time), (tier, 1, 1, T0));
        assert_eq!(e.genesis_hash, a.journal_hash);
    }
}

#[test]
fn open_account_also_works_with_the_vaults_other_mint() {
    let mut v = VaultEnv::new();
    let t = Keypair::new();
    v.e.svm.airdrop(&t.pubkey(), 10_000_000_000).unwrap();
    let usdt = v.usdt;
    let ta = v.new_token_account(&usdt, &t.pubkey(), TRADER_START);
    let fee_ta = v.new_token_account(&usdt, &v.fee_wallet.clone(), 0);
    let vsl8 = v.new_token_account(&usdt, &VAULT_SL8_ADMIN, 0);
    v.traders.insert(t.pubkey(), ta);
    let mut oa = v.open_accounts(&t.pubkey());
    oa.mint = usdt;
    oa.pool_token_account = v.usdt_pool;
    oa.vault_sl8_token_account = vsl8;
    oa.fee_token_account = fee_ta;
    ok(v.send_open(&oa, 0, &t));
    assert_eq!(v.token_balance(&fee_ta), 120_000);
    assert_eq!(v.token_balance(&ta), TRADER_START - 30_000_000 - 120_000);
}

#[test]
fn the_fee_account_and_the_vault_sl8_account_may_be_the_same_account() {
    let mut v = VaultEnv::with(Opts { fee_wallet_is_vault_sl8: true, ..Opts::default() });
    assert_eq!(v.fee_usdc, v.vault_sl8_usdc);
    let t = v.new_trader();
    ok(v.open(&t, 2));
    let (cost, fee) = (99_000_000u64, 396_000u64);
    let (_, sl8) = VaultEnv::vault_split(cost);
    assert_eq!(v.token_balance(&v.vault_sl8_usdc), sl8 + fee);
}

// ---------------------------------------------------------------- fee rounding

#[test]
fn fee_table_of_design_9_2_for_all_ten_tiers() {
    for (i, want) in FEE_TABLE.iter().enumerate() {
        let mut v = VaultEnv::new();
        let t = v.new_trader();
        let before = v.token_balance(&v.fee_usdc);
        ok(v.open(&t, u8::try_from(i).unwrap()));
        assert_eq!(v.token_balance(&v.fee_usdc) - before, *want, "tier {i}");
    }
}

#[test]
fn the_fee_is_rounded_up_not_down_and_not_to_nearest() {
    // fee_bps = 1: cost 12_340_001 x 1 / 10_000 = 1234.0001 -> ceil 1235 (floor 1234, round-to-nearest 1234)
    let mut args = launch_ruleset_args();
    args.fee_bps = 1;
    args.tier_cost[0] = 12_340_001;
    let mut v = VaultEnv::with(Opts { ruleset: args, ..Opts::default() });
    let t = v.new_trader();
    ok(v.open(&t, 0));
    assert_eq!(v.token_balance(&v.fee_usdc), 1235);
    assert_eq!(v.token_balance(&v.trader_token(&t.pubkey())), TRADER_START - 12_340_001 - 1235);

    // an exact quotient is not rounded: 10_000_000 x 1 / 10_000 = 1000
    let mut args = launch_ruleset_args();
    args.fee_bps = 1;
    args.tier_cost[0] = 10_000_000;
    let mut v = VaultEnv::with(Opts { ruleset: args, ..Opts::default() });
    let t = v.new_trader();
    ok(v.open(&t, 0));
    assert_eq!(v.token_balance(&v.fee_usdc), 1000);

    // just above the exact quotient: 10_000_001 -> 1000.0001 -> 1001
    let mut args = launch_ruleset_args();
    args.fee_bps = 1;
    args.tier_cost[0] = 10_000_001;
    let mut v = VaultEnv::with(Opts { ruleset: args, ..Opts::default() });
    let t = v.new_trader();
    ok(v.open(&t, 0));
    assert_eq!(v.token_balance(&v.fee_usdc), 1001);

    // fee_bps = 0: no fee transfer at all
    let mut args = launch_ruleset_args();
    args.fee_bps = 0;
    let mut v = VaultEnv::with(Opts { ruleset: args, ..Opts::default() });
    let t = v.new_trader();
    ok(v.open(&t, 0));
    assert_eq!(v.token_balance(&v.fee_usdc), 0);
    assert_eq!(v.token_balance(&v.trader_token(&t.pubkey())), TRADER_START - 30_000_000);
}

// ---------------------------------------------------------------- ids and the cap

#[test]
fn challenge_ids_count_up_and_a_replayed_account_address_fails() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    ok(v.open(&t, 0));
    ok(v.open(&t, 1));
    assert_eq!(v.account(&acct_pda(&tk, 0)).challenge_id, 0);
    assert_eq!(v.account(&acct_pda(&tk, 1)).challenge_id, 1);
    assert_ne!(acct_pda(&tk, 0), acct_pda(&tk, 1));
    assert_eq!(v.account(&acct_pda(&tk, 1)).size_s, 5_000_000_000);
    let w = v.wallet_opt(&tk).unwrap();
    assert_eq!((w.next_challenge_id, w.active_count), (2, 2));
    assert_eq!(v.vault_trader_state(&tk, 1).unwrap().challenge_id, 1);

    // replay: try to create challenge 0's account (and vault record) again
    let before = v.digest(&tk, &[]);
    let mut oa = v.open_accounts(&tk);
    oa.trading_account = acct_pda(&tk, 0);
    oa.vault_trader_state = vault_trader_state_pda(&tk, 0);
    let r = v.send_open(&oa, 0, &t);
    assert_failed(&r, "replayed account address");
    assert_eq!(v.digest(&tk, &[]), before);
    // two traders get independent id sequences
    let u = v.new_trader();
    ok(v.open(&u, 0));
    assert_eq!(v.account(&acct_pda(&u.pubkey(), 0)).challenge_id, 0);
}

#[test]
fn five_accounts_open_and_the_sixth_fails_without_changing_anything() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    for k in 0..5u8 {
        ok(v.open(&t, k));
    }
    assert_eq!(v.wallet_opt(&t.pubkey()).unwrap().active_count, 5);
    let before = v.digest(&t.pubkey(), &[]);
    let r = v.open(&t, 0);
    assert_err(&r, Setl8Error::WalletCapReached);
    assert_eq!(v.digest(&t.pubkey(), &[]), before);
    assert_eq!(v.wallet_opt(&t.pubkey()).unwrap().next_challenge_id, 5);
}

#[test]
fn cap_boundary_four_to_five_works_and_five_fails() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    ok(v.open(&t, 0));
    // pretend four accounts are active (failing and re-opening is M7): 4 -> 5 is allowed
    v.edit_wallet(&t.pubkey(), |w| w.active_count = 4);
    ok(v.open(&t, 0));
    assert_eq!(v.wallet_opt(&t.pubkey()).unwrap().active_count, 5);
    for n in [5u8, 6, 255] {
        v.edit_wallet(&t.pubkey(), |w| w.active_count = n);
        let before = v.digest(&t.pubkey(), &[]);
        assert_err(&v.open(&t, 0), Setl8Error::WalletCapReached);
        assert_eq!(v.digest(&t.pubkey(), &[]), before, "active_count {n}");
    }
    // a freed slot (4) opens again
    v.edit_wallet(&t.pubkey(), |w| w.active_count = 4);
    ok(v.open(&t, 0));
}

// ---------------------------------------------------------------- rejections (each leaves everything unchanged)

/// Runs `open` with `setup` on a fresh env, expects failure and an unchanged world.
fn rejected(
    label: &str,
    setup: impl FnOnce(&mut VaultEnv, &Keypair, &mut OpenAccounts, &mut u8, &mut Vec<Pubkey>),
    check: impl FnOnce(&litesvm::types::TransactionResult),
) {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let mut oa = v.open_accounts(&t.pubkey());
    let (mut tier, mut extra) = (0u8, Vec::new());
    setup(&mut v, &t, &mut oa, &mut tier, &mut extra);
    let before = v.digest(&t.pubkey(), &extra);
    let r = v.send_open(&oa, tier, &t);
    println!("case: {label}");
    check(&r);
    assert!(r.is_err(), "{label}: must fail");
    assert_eq!(v.digest(&t.pubkey(), &extra), before, "{label}: the world changed");
    assert!(v.wallet_opt(&t.pubkey()).is_none() || v.wallet_opt(&t.pubkey()).unwrap().next_challenge_id == 0, "{label}");
    assert!(v.e.svm.get_account(&acct_pda(&t.pubkey(), 0)).is_none(), "{label}: account was created");
}

#[test]
fn rejected_when_the_sector_is_paused() {
    rejected(
        "paused",
        |v, _, _, _, _| {
            let ix = set_paused_ix(both(&v.e), true);
            assert_ok(v.e.send_as_admins(&[ix]));
        },
        |r| assert_err(r, Setl8Error::Paused),
    );
}

#[test]
fn rejected_when_tier_index_is_not_below_tier_count() {
    for bad in [10u8, 11, 255] {
        rejected("tier", move |_, _, _, tier, _| *tier = bad, |r| assert_err(r, Setl8Error::InvalidTier));
    }
}

#[test]
fn rejected_when_tier_index_is_not_below_a_smaller_tier_count() {
    // a rule set with 3 tiers: index 3 is refused although the arrays are 10 wide
    let mut args = launch_ruleset_args();
    args.tier_count = 3;
    for i in 3..10 {
        args.tier_size[i] = 0;
        args.tier_cost[i] = 0;
    }
    let mut v = VaultEnv::with(Opts { ruleset: args, ..Opts::default() });
    let t = v.new_trader();
    let before = v.digest(&t.pubkey(), &[]);
    assert_err(&v.open(&t, 3), Setl8Error::InvalidTier);
    assert_eq!(v.digest(&t.pubkey(), &[]), before);
    ok(v.open(&t, 2));
}

#[test]
fn rejected_for_a_banned_wallet_and_allowed_again_after_unban() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    let admins = Admins::both(&v.e);
    let ix = ban_wallet_ix(v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
    assert_ok(v.e.send_admins(ix, admins));
    let before = v.digest(&tk, &[]);
    assert_err(&v.open(&t, 0), Setl8Error::WalletBanned);
    assert_eq!(v.digest(&tk, &[]), before);

    let ix = unban_wallet_ix(admins, tk);
    assert_ok(v.e.send_admins(ix, admins));
    ok(v.open(&t, 0));
    let w = v.wallet_opt(&tk).unwrap();
    assert_eq!((w.banned, w.active_count, w.next_challenge_id), (0, 1, 1));
}

#[test]
fn ban_wallet_on_a_wallet_without_a_profile_is_unchanged_by_this_module() {
    // M1b: ban_wallet creates the profile with next_challenge_id 0 and active_count 0; open_account continues from there
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    assert!(v.wallet_opt(&tk).is_none());
    let admins = Admins::both(&v.e);
    let ix = ban_wallet_ix(v.e.payer.pubkey(), admins, tk, [1; 32]);
    assert_ok(v.e.send_admins(ix, admins));
    let w = v.wallet_opt(&tk).unwrap();
    assert_eq!((w.trader, w.next_challenge_id, w.active_count, w.banned), (tk, 0, 0, 1));
    assert_eq!(v.e.svm.get_account(&wallet_pda(&tk)).unwrap().data.len(), 56);
    let ix = unban_wallet_ix(admins, tk);
    assert_ok(v.e.send_admins(ix, admins));
    ok(v.open(&t, 0));
    assert_eq!(v.account(&acct_pda(&tk, 0)).challenge_id, 0, "the first account of a pre-banned wallet is challenge 0");
}


#[test]
fn rejected_with_a_stale_rule_set_version() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    let mut v2 = launch_ruleset_args();
    v2.version = 2;
    v.e.create_ruleset_ok(&v2);
    assert_eq!(v.e.config().current_ruleset, 2);
    // passing version 1 while the config says 2
    let before = v.digest(&tk, &[]);
    let oa = v.open_accounts(&tk); // defaults to rule set 1
    assert_err(&v.send_open(&oa, 0, &t), Setl8Error::RuleSetMissing);
    assert_eq!(v.digest(&tk, &[]), before);
    // the current version works and the account records it
    let mut oa = v.open_accounts(&tk);
    oa.ruleset = ruleset_pda(2);
    ok(v.send_open(&oa, 0, &t));
    assert_eq!(v.account(&acct_pda(&tk, 0)).ruleset_version, 2);
    // a rule set account that is not a rule set at all
    let mut oa = v.open_accounts(&tk);
    oa.ruleset = config_pda();
    assert_failed(&v.send_open(&oa, 0, &t), "config passed as rule set");
    // a version that does not exist yet
    let mut oa = v.open_accounts(&tk);
    oa.ruleset = ruleset_pda(3);
    assert_failed(&v.send_open(&oa, 0, &t), "version 3 does not exist");
}

#[test]
fn rejected_with_a_mint_the_vault_does_not_accept() {
    rejected(
        "fake mint",
        |v, t, oa, _, extra| {
            let fake = Pubkey::new_unique();
            v.set_mint(&fake);
            let ta = v.new_token_account(&fake, &t.pubkey(), TRADER_START);
            let fee = v.new_token_account(&fake, &v.fee_wallet.clone(), 0);
            let pool = v.new_token_account(&fake, &v.vault_state.clone(), 0);
            let vsl8 = v.new_token_account(&fake, &VAULT_SL8_ADMIN, 0);
            extra.extend([fake, ta, fee, pool, vsl8]);
            oa.mint = fake;
            oa.trader_token_account = ta;
            oa.fee_token_account = fee;
            oa.pool_token_account = pool;
            oa.vault_sl8_token_account = vsl8;
        },
        |_| {},
    );
}

#[test]
fn rejected_when_a_token_account_has_the_wrong_owner_or_mint() {
    // trader token account owned by someone else
    rejected(
        "trader token account of another owner",
        |v, _, oa, _, extra| {
            let usdc = v.usdc;
            let other = v.new_token_account(&usdc, &Pubkey::new_unique(), TRADER_START);
            extra.push(other);
            oa.trader_token_account = other;
        },
        |r| assert_err(r, Setl8Error::InvalidTokenAccount),
    );
    // trader token account of the other mint
    rejected(
        "trader token account with a different mint",
        |v, t, oa, _, extra| {
            let usdt = v.usdt;
            let other = v.new_token_account(&usdt, &t.pubkey(), TRADER_START);
            extra.push(other);
            oa.trader_token_account = other;
        },
        |r| assert_err(r, Setl8Error::InvalidTokenAccount),
    );
    // the fee (SL8 wallet) token account owned by a stranger
    rejected(
        "fee token account of a wrong owner",
        |v, _, oa, _, extra| {
            let usdc = v.usdc;
            let other = v.new_token_account(&usdc, &Pubkey::new_unique(), 0);
            extra.push(other);
            oa.fee_token_account = other;
        },
        |r| assert_err(r, Setl8Error::InvalidTokenAccount),
    );
    // the fee account of the right owner but another mint
    rejected(
        "fee token account with a wrong mint",
        |v, _, oa, _, extra| {
            let usdt = v.usdt;
            let other = v.new_token_account(&usdt, &v.fee_wallet.clone(), 0);
            extra.push(other);
            oa.fee_token_account = other;
        },
        |r| assert_err(r, Setl8Error::InvalidTokenAccount),
    );
    // the VAULT's SL8 account of a wrong owner: the fee already moved, the vault refuses, everything reverts
    rejected(
        "vault SL8 token account of a wrong owner",
        |v, _, oa, _, extra| {
            let usdc = v.usdc;
            let other = v.new_token_account(&usdc, &Pubkey::new_unique(), 0);
            extra.push(other);
            oa.vault_sl8_token_account = other;
        },
        |_| {},
    );
    // the vault's pool account replaced by a stranger's
    rejected(
        "vault pool replaced",
        |v, _, oa, _, extra| {
            let usdc = v.usdc;
            let other = v.new_token_account(&usdc, &Pubkey::new_unique(), 0);
            extra.push(other);
            oa.pool_token_account = other;
        },
        |_| {},
    );
}

#[test]
fn rejected_without_the_trader_signature() {
    // trader and payer are the same key, so Solana merges their signer flags: the key is unsigned only if BOTH are cleared
    rejected(
        "trader and payer flags cleared",
        |_, _, oa, _, _| {
            oa.trader_signs = false;
            oa.payer_signs = false;
        },
        |r| assert_anchor_code(r, 3010),
    );
}

#[test]
fn rejected_when_the_payer_is_not_the_trader() {
    rejected(
        "payer is another funded signer",
        |v, _, oa, _, extra| {
            let other = Keypair::new();
            v.e.svm.airdrop(&other.pubkey(), 10_000_000_000).unwrap();
            extra.push(other.pubkey());
            oa.payer = other.pubkey();
            // the other signer does not need a private key here: sigverify is off and the flag is set
        },
        |r| assert_err(r, Setl8Error::Unauthorized),
    );
}

#[test]
fn rejected_with_a_wrong_sector_authority_or_vault_program() {
    rejected("sector_authority is a random key", |_, _, oa, _, _| oa.sector_authority = Pubkey::new_unique(), |r| assert_anchor_code(r, 2006));
    rejected("sector_authority is the config", |_, _, oa, _, _| oa.sector_authority = config_pda(), |r| assert_anchor_code(r, 2006));
    rejected("vault program is a random key", |_, _, oa, _, _| oa.vault_program = Pubkey::new_unique(), |r| assert_err(r, Setl8Error::InvalidVaultProgram));
    rejected("vault program is the system program", |_, _, oa, _, _| oa.vault_program = anchor_lang::solana_program::system_program::ID, |r| assert_err(r, Setl8Error::InvalidVaultProgram));
}

#[test]
fn rejected_when_the_vault_accounts_do_not_belong_together() {
    rejected("another product registry", |_, _, oa, _, _| oa.product_registry = Pubkey::new_unique(), |_| {});
    rejected("vault state replaced", |_, _, oa, _, _| oa.vault_state = Pubkey::new_unique(), |_| {});
    rejected("vault trader_state of another challenge", |_, t, oa, _, _| oa.vault_trader_state = vault_trader_state_pda(&t.pubkey(), 7), |_| {});
}

#[test]
fn rejected_when_the_balance_cannot_cover_cost_plus_fee() {
    // cost + fee - 1: the fee transfer alone would succeed, the vault payment would not: nothing may stay
    let (cost, fee) = (30_000_000u64, 120_000u64);
    for balance in [cost + fee - 1, cost, fee, 0] {
        let mut v = VaultEnv::new();
        let t = v.new_trader_with(balance);
        let tk = t.pubkey();
        let before = v.digest(&tk, &[]);
        let r = v.open(&t, 0);
        assert_failed(&r, "insufficient balance");
        assert_eq!(v.digest(&tk, &[]), before, "balance {balance}");
        assert_eq!(v.token_balance(&v.fee_usdc), 0, "the fee must not stay behind (balance {balance})");
        assert!(v.wallet_opt(&tk).is_none());
    }
    // exactly cost + fee works and empties the account
    let mut v = VaultEnv::new();
    let t = v.new_trader_with(cost + fee);
    ok(v.open(&t, 0));
    assert_eq!(v.token_balance(&v.trader_token(&t.pubkey())), 0);
}

// ---------------------------------------------------------------- atomicity

#[test]
fn a_failing_vault_cpi_rolls_back_the_fee_the_profile_and_the_account() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    // the vault's own pause_product: deposit_fee then fails with ProductNotActive
    let pause = si::pause_product(VAULT_ID, VAULT_SL8_ADMIN, VAULT_ROV_ADMIN, registry_pda(), &[], si::PauseProductArgs { product_program_id: PROGRAM_ID });
    v.send_plain(pause).expect("pause product at the vault");
    let before = v.digest(&tk, &[]);
    let fee_before = v.token_balance(&v.fee_usdc);
    let r = v.open(&t, 0);
    assert_failed(&r, "paused product");
    if let Err(f) = &r {
        assert!(f.meta.logs.iter().any(|l| l.contains("DepositFee")), "the vault CPI was reached:\n{}", f.meta.logs.join("\n"));
        assert!(f.meta.logs.iter().any(|l| l.contains("TransferChecked")), "the fee transfer had run before the CPI failed");
    }
    assert_eq!(v.digest(&tk, &[]), before);
    assert_eq!(v.token_balance(&v.fee_usdc), fee_before);
    assert!(v.wallet_opt(&tk).is_none(), "no profile left behind");
    assert!(v.e.svm.get_account(&acct_pda(&tk, 0)).is_none(), "no account left behind");
    assert!(v.vault_trader_state(&tk, 0).is_none());
    // control: after the vault reactivates the product the same call succeeds
    let react = si::reactivate_product(VAULT_ID, VAULT_SL8_ADMIN, VAULT_ROV_ADMIN, registry_pda(), &[], si::ReactivateProductArgs { product_program_id: PROGRAM_ID });
    v.send_plain(react).expect("reactivate");
    ok(v.open(&t, 0));
}

// ---------------------------------------------------------------- compute units

#[test]
fn compute_units_of_open_account_are_recorded() {
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let first = ok(v.open(&t, 0)); // creates the wallet profile
    let second = ok(v.open(&t, 9)); // profile exists
    println!("CU open_account (first account, wallet profile created): {}", first.compute_units_consumed);
    println!("CU open_account (second account): {}", second.compute_units_consumed);
    assert!(first.compute_units_consumed > 0 && second.compute_units_consumed > 0);
}
