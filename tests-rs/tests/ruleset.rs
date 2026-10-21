mod common;

use anchor_lang::solana_program::pubkey::Pubkey;
use common::*;
use setl8_lev_trading::{errors::Setl8Error, events::RuleSetCreated, instructions::RuleSetArgs};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn send_create(e: &mut Env, args: &RuleSetArgs) -> litesvm::types::TransactionResult {
    let admins = Admins::both(e);
    let ix = create_ruleset_ix(e.payer.pubkey(), admins, args);
    e.send_admins(ix, admins)
}

/// Fresh environment, `mutate` applied to the launch values, expects `err`, and nothing was written.
fn expect_rejected(label: &str, mutate: impl FnOnce(&mut RuleSetArgs), err: Setl8Error) {
    let mut e = Env::initialised();
    let mut args = launch_ruleset_args();
    mutate(&mut args);
    println!("case: {label}");
    let r = send_create(&mut e, &args);
    assert_err(&r, err);
    assert!(e.svm.get_account(&ruleset_pda(args.version)).is_none(), "{label}: account was created");
    assert_eq!(e.config().current_ruleset, 0, "{label}: current_ruleset moved");
}

fn expect_ok(label: &str, mutate: impl FnOnce(&mut RuleSetArgs)) {
    let mut e = Env::initialised();
    let mut args = launch_ruleset_args();
    mutate(&mut args);
    println!("case: {label}");
    assert_ok(send_create(&mut e, &args));
    assert_eq!(e.config().current_ruleset, 1);
}

#[test]
fn create_ruleset_succeeds_with_launch_values() {
    let mut e = Env::initialised();
    let args = launch_ruleset_args();
    let r = send_create(&mut e, &args);
    let meta = r.as_ref().expect("create_ruleset succeeds").clone();
    assert_eq!(common::pass::cpi_events::<RuleSetCreated>(&meta).len(), 1);
    assert_eq!(common::pass::cpi_events::<RuleSetCreated>(&meta)[0].version, 1);

    let acc = e.svm.get_account(&ruleset_pda(1)).unwrap();
    assert_eq!(acc.data.len(), 552);
    assert_eq!(acc.owner, PROGRAM_ID);
    let rs = e.ruleset(1);
    assert_eq!(rs.version, args.version);
    assert_eq!(rs.fee_bps, 40);
    assert_eq!(rs.tier_count, 10);
    assert_eq!(rs.ladder_len, 5);
    assert_eq!(rs.leverage_x, args.leverage_x);
    assert_eq!(rs.leverage_x, [50, 20, 20, 20, 10, 5, 5, 2]);
    assert_eq!(rs.holding_fee_e6, args.holding_fee_e6);
    assert_eq!(rs.holding_fee_e6, [100, 200, 400, 200, 400, 400, 1000, 1600]);
    assert_eq!(rs.commission_e6, [0; 8]);
    assert_eq!(rs.tier_size, args.tier_size);
    assert_eq!(rs.tier_size[0], 2_500_000_000);
    assert_eq!(rs.tier_size[9], 200_000_000_000);
    assert_eq!(rs.tier_cost, args.tier_cost);
    assert_eq!(rs.tier_cost[0], 30_000_000);
    assert_eq!(rs.tier_cost[9], 1_599_000_000);
    assert_eq!(rs.reset_ladder_bps, [10_000, 11_500, 12_000, 12_750, 13_000]);
    assert_eq!(rs.spread_e6, [0u16; 128]);
    assert_eq!(rs.bump, Pubkey::find_program_address(&[b"ruleset", &1u16.to_le_bytes()], &PROGRAM_ID).1);
    assert_eq!(rs.reserved, [0u8; 31]);
    assert_eq!(e.config().current_ruleset, 1);
}

#[test]
fn create_ruleset_fails_with_one_admin_only() {
    let mut e = Env::initialised();
    for admins in [Admins::only_sl8(&e), Admins::only_rov(&e), Admins::neither(&e)] {
        let ix = create_ruleset_ix(e.payer.pubkey(), admins, &launch_ruleset_args());
        assert_err(&e.send_admins(ix, admins), Setl8Error::Unauthorized);
    }
    assert!(e.svm.get_account(&ruleset_pda(1)).is_none());
    assert_eq!(e.config().current_ruleset, 0);
}

#[test]
fn create_ruleset_fails_for_non_admins() {
    let mut e = Env::initialised();
    let (x, y) = (Keypair::new(), Keypair::new());
    let ix = create_ruleset_ix(e.payer.pubkey(), Admins::strangers(&x, &y), &launch_ruleset_args());
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    assert!(e.svm.get_account(&ruleset_pda(1)).is_none());
}

#[test]
fn create_ruleset_rejects_version_zero() {
    expect_rejected("version 0", |a| a.version = 0, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_version_two_before_one_exists() {
    expect_rejected("version 2 first", |a| a.version = 2, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_version_one_twice() {
    let mut e = Env::initialised();
    let args = launch_ruleset_args();
    assert_ok(send_create(&mut e, &args));
    let before = e.svm.get_account(&ruleset_pda(1)).unwrap();
    let mut again = args.clone();
    again.fee_bps = 41; // a different body must not overwrite version 1
    let r = send_create(&mut e, &again);
    assert!(r.is_err(), "second create of version 1 must fail");
    assert_eq!(e.svm.get_account(&ruleset_pda(1)).unwrap(), before);
    assert_eq!(e.config().current_ruleset, 1);
}

#[test]
fn create_ruleset_rejects_bad_tier_and_ladder_counts() {
    expect_rejected("tier_count 0", |a| a.tier_count = 0, Setl8Error::InvalidTier);
    expect_rejected("tier_count 11", |a| a.tier_count = 11, Setl8Error::InvalidTier);
    expect_rejected("ladder_len 0", |a| a.ladder_len = 0, Setl8Error::InvalidTier);
    expect_rejected("ladder_len 6", |a| a.ladder_len = 6, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_non_increasing_tier_size() {
    expect_rejected("size equal", |a| a.tier_size[4] = a.tier_size[3], Setl8Error::InvalidTier);
    expect_rejected("size decreasing", |a| a.tier_size[4] = a.tier_size[3] - 1, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_non_increasing_tier_cost() {
    expect_rejected("cost equal", |a| a.tier_cost[4] = a.tier_cost[3], Setl8Error::InvalidTier);
    expect_rejected("cost decreasing", |a| a.tier_cost[4] = a.tier_cost[3] - 1, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_zero_tier_size_or_cost() {
    expect_rejected("size[0] = 0", |a| a.tier_size[0] = 0, Setl8Error::InvalidTier);
    expect_rejected("cost[0] = 0", |a| a.tier_cost[0] = 0, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_a_decreasing_reset_ladder() {
    expect_rejected("ladder decreasing", |a| a.reset_ladder_bps[3] = a.reset_ladder_bps[2] - 1, Setl8Error::InvalidTier);
    expect_rejected("ladder first zero", |a| a.reset_ladder_bps[0] = 0, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_non_zero_entries_beyond_the_counts() {
    expect_rejected(
        "tier_size beyond tier_count",
        |a| {
            a.tier_count = 9;
            a.tier_cost[9] = 0;
        },
        Setl8Error::InvalidTier,
    );
    expect_rejected(
        "tier_cost beyond tier_count",
        |a| {
            a.tier_count = 9;
            a.tier_size[9] = 0;
        },
        Setl8Error::InvalidTier,
    );
    expect_rejected("ladder beyond ladder_len", |a| a.ladder_len = 4, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_rejects_fee_above_ten_percent() {
    expect_rejected("fee 1001", |a| a.fee_bps = 1001, Setl8Error::InvalidTier);
    expect_ok("fee 1000", |a| a.fee_bps = 1000);
    expect_ok("fee 0", |a| a.fee_bps = 0);
}

#[test]
fn create_ruleset_rejects_zero_leverage() {
    expect_rejected("leverage[0] = 0", |a| a.leverage_x[0] = 0, Setl8Error::InvalidTier);
    expect_rejected("leverage[7] = 0", |a| a.leverage_x[7] = 0, Setl8Error::InvalidTier);
}

#[test]
fn create_ruleset_accepts_valid_variants() {
    expect_ok("one tier, one ladder step", |a| {
        a.tier_count = 1;
        a.ladder_len = 1;
        a.tier_size[1..].fill(0);
        a.tier_cost[1..].fill(0);
        a.reset_ladder_bps[1..].fill(0);
    });
    expect_ok("equal ladder steps", |a| a.reset_ladder_bps = [10_000; 5]);
    expect_ok("pending numbers non-zero", |a| {
        a.commission_e6 = [u32::MAX; 8];
        a.spread_e6 = [u16::MAX; 128];
        a.holding_fee_e6 = [0; 8];
    });
}

#[test]
fn version_two_can_follow_one_and_version_one_is_untouched() {
    let mut e = Env::initialised();
    let v1 = launch_ruleset_args();
    assert_ok(send_create(&mut e, &v1));
    let v1_bytes = e.svm.get_account(&ruleset_pda(1)).unwrap();

    let mut v2 = launch_ruleset_args();
    v2.version = 2;
    v2.fee_bps = 50;
    v2.leverage_x = [30, 10, 10, 10, 5, 3, 3, 1];
    assert_ok(send_create(&mut e, &v2));

    assert_eq!(e.svm.get_account(&ruleset_pda(1)).unwrap(), v1_bytes, "version 1 bytes changed");
    assert_eq!(e.ruleset(2).fee_bps, 50);
    assert_eq!(e.ruleset(2).leverage_x, [30, 10, 10, 10, 5, 3, 3, 1]);
    assert_eq!(e.config().current_ruleset, 2);

    // skipping to 4 is still refused
    let mut v4 = launch_ruleset_args();
    v4.version = 4;
    assert_err(&send_create(&mut e, &v4), Setl8Error::InvalidTier);
}
