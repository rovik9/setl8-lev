//! Phase-reset price against the REAL vault fixture (core_vault.so, vault core commit 9a5c179, shared-interfaces v0.5.0):
//! the registered ladder and tier sizes, and the vault's `deposit_reset` price check (decision 51).
//!
//! `deposit_reset` is called on the vault DIRECTLY (our `reset_account` is M8): the `sector_authority` PDA is flagged as
//! a signer without a signature, which this environment allows because signature verification is off (the vault checks
//! only the PDA's key and the flag). The amounts come from the program crate's `reset_amount_micro` applied to the tier
//! list READ BACK from the vault's registry, which is exactly what M8 will do.
mod common;

use std::collections::BTreeSet;

use common::vault::*;
use common::*;
use setl8_lev_trading::reset_price::reset_amount_micro;
use solana_signer::Signer;

const LADDER: [u16; 5] = [10_000, 11_500, 12_000, 12_750, 13_000];
const USD: u64 = 1_000_000;
const VAULT_INVALID_CHALLENGE_TIER: u32 = 6006;
const VAULT_WRONG_AMOUNT: u32 = 6007;
const VAULT_INVALID_RESET_PHASE: u32 = 6012;
const VAULT_RESET_NOT_ALLOWED: u32 = 6011;
/// `TraderStatus` as stored by the vault: Active, Graduated, Failed, Abandoned.
const VAULT_ACTIVE: u8 = 0;
const VAULT_FAILED: u8 = 2;
/// Tier indexes of the launch rule set.
const TIER_2_5K: u8 = 0;
const TIER_100K: u8 = 8;
const PHASE_PRICES_100K: [u64; 5] = [859_000_000, 987_850_000, 1_030_800_000, 1_095_225_000, 1_116_700_000];
const PHASE_PRICES_2_5K: [u64; 5] = [30_000_000, 34_500_000, 36_000_000, 38_250_000, 39_000_000];

/// A trader with one OPEN account at `tier` (challenge 0) whose vault record is then flagged Failed.
fn failed_trader(v: &mut VaultEnv, tier: u8) -> solana_keypair::Keypair {
    let trader = v.new_trader();
    assert_ok(v.open(&trader, tier));
    assert_ok(v.flag_vault_failed(&trader.pubkey(), 0));
    assert_eq!(v.vault_trader_state(&trader.pubkey(), 0).unwrap().status, VAULT_FAILED);
    trader
}

fn tiers_of(v: &VaultEnv) -> Vec<(u64, u64)> {
    v.registry().challenge_sizes
}

// ------------------------------------------------------------------ registration

#[test]
fn the_vault_registry_holds_the_ladder_and_unique_tier_sizes() {
    let v = VaultEnv::new();
    let r = v.registry();
    assert_eq!(r.reset_price_bps, vec![10_000, 11_500, 12_000, 12_750, 13_000]);
    assert_eq!(r.max_payout_count, 4);
    assert_eq!(r.fee_split_bps, FEE_SPLIT_BPS);
    assert!(r.active);
    assert_eq!(r.product_program_id, PROGRAM_ID);
    assert_eq!(r.challenge_sizes.len(), 10);
    let sizes: BTreeSet<u64> = r.challenge_sizes.iter().map(|(s, _)| *s).collect();
    assert_eq!(sizes.len(), r.challenge_sizes.len(), "the registered tier sizes must be unique (decision 51)");
    // the registered tiers are the rule set's ten, in order
    let want: Vec<(u64, u64)> = (0..10).map(|i| (v.ruleset.tier_size[i], v.ruleset.tier_cost[i])).collect();
    assert_eq!(r.challenge_sizes, want);
}

#[test]
fn the_launch_rule_set_has_ten_unique_tier_sizes() {
    let rs = launch_ruleset_args();
    assert_eq!(rs.tier_count, 10);
    let sizes: BTreeSet<u64> = rs.tier_size[..usize::from(rs.tier_count)].iter().copied().collect();
    assert_eq!(sizes.len(), 10, "duplicate tier sizes make the vault refuse every reset of that size");
    assert_eq!(rs.reset_ladder_bps, LADDER);
}

// ------------------------------------------------------------------ deposit_reset, the real price check

fn five_phases(tier: u8, want: [u64; 5]) {
    for (phase, want_price) in want.iter().enumerate() {
        let mut v = VaultEnv::new();
        let trader = failed_trader(&mut v, tier);
        let tk = trader.pubkey();
        let size = v.ruleset.tier_size[usize::from(tier)];
        // the sector's number, from the tier list read back from the vault
        let price = reset_amount_micro(&tiers_of(&v), size, phase, &v.registry().reset_price_bps).unwrap();
        assert_eq!(price, *want_price, "phase index {phase}");
        // the vault copies payout_count and account_size from the failed record itself
        v.edit_vault_record(&tk, 0, None, Some(2));
        let (t0, pool0, sl80) = (v.token_balance(&v.trader_token(&tk)), v.token_balance(&v.usdc_pool), v.token_balance(&v.vault_sl8_usdc));

        assert_ok(v.vault_deposit_reset(&trader, 0, 7, price, u8::try_from(phase).unwrap()));

        let (pool_part, sl8_part) = VaultEnv::vault_split(price);
        assert_eq!(v.token_balance(&v.trader_token(&tk)), t0 - price, "trader pays exactly the price");
        assert_eq!(v.token_balance(&v.usdc_pool), pool0 + pool_part);
        assert_eq!(v.token_balance(&v.vault_sl8_usdc), sl80 + sl8_part);
        let new = v.vault_trader_state(&tk, 7).expect("new vault record");
        assert_eq!((new.status, new.account_size, new.payout_count), (VAULT_ACTIVE, size, 2), "size and payouts come from the failed record");
        // once only
        assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 8, price, u8::try_from(phase).unwrap()), VAULT_RESET_NOT_ALLOWED);
    }
}

#[test]
fn deposit_reset_charges_the_ladder_at_the_100k_tier_in_all_five_phases() {
    five_phases(TIER_100K, PHASE_PRICES_100K);
}

#[test]
fn deposit_reset_charges_the_ladder_at_the_2_5k_tier_in_all_five_phases() {
    // $30 for 2.5K: not proportional to the $859 of 100K, so only a tier-price formula can give these numbers
    five_phases(TIER_2_5K, PHASE_PRICES_2_5K);
}

#[test]
fn deposit_reset_with_any_other_amount_is_wrong_amount_and_changes_nothing() {
    let mut v = VaultEnv::new();
    let trader = failed_trader(&mut v, TIER_100K);
    let tk = trader.pubkey();
    let before = v.digest(&tk, &[]);
    for phase in 0u8..5 {
        let price = PHASE_PRICES_100K[usize::from(phase)];
        for amount in [price - 1, price + 1, 0, u64::MAX, PHASE_PRICES_100K[(usize::from(phase) + 1) % 5]] {
            assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, amount, phase), VAULT_WRONG_AMOUNT);
        }
    }
    // the old size-based number (1% of the size) is not accepted either
    assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, 1_000_000_000, 0), VAULT_WRONG_AMOUNT);
    assert_eq!(v.digest(&tk, &[]), before, "a rejected reset moves nothing");
}

#[test]
fn deposit_reset_for_a_size_that_is_not_a_tier_is_invalid_challenge_tier() {
    let mut v = VaultEnv::new();
    let trader = failed_trader(&mut v, TIER_100K);
    let tk = trader.pubkey();
    v.edit_vault_record(&tk, 0, Some(123 * USD), None);
    let before = v.digest(&tk, &[]);
    // no size-based fallback: not the tier cost, not 1% of the size, not anything
    for amount in [PHASE_PRICES_100K[0], 123 * USD / 100, 0] {
        assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, amount, 0), VAULT_INVALID_CHALLENGE_TIER);
    }
    assert_eq!(v.digest(&tk, &[]), before);
}

#[test]
fn deposit_reset_with_a_phase_beyond_the_table_is_invalid_reset_phase() {
    let mut v = VaultEnv::new();
    let trader = failed_trader(&mut v, TIER_100K);
    for phase in [5u8, 6, 255] {
        assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, PHASE_PRICES_100K[4], phase), VAULT_INVALID_RESET_PHASE);
    }
    // and the sector helper refuses the same index before any CPI
    assert!(reset_amount_micro(&tiers_of(&v), 100_000 * USD, 5, &v.registry().reset_price_bps).is_err());
}

// ------------------------------------------------------------------ the reset follows the CURRENT tier (decision 51)

#[test]
fn repricing_a_tier_changes_a_pending_reset_and_removing_it_blocks_it() {
    let mut v = VaultEnv::new();
    let trader = failed_trader(&mut v, TIER_100K);
    let tk = trader.pubkey();
    let size_100k = 100_000 * USD;
    let mut tiers = tiers_of(&v);

    // reprice 100K from $859 to $900 (2-of-2 update_product_config): the pending reset follows the new cost
    for t in tiers.iter_mut().filter(|t| t.0 == size_100k) {
        t.1 = 900 * USD;
    }
    assert_ok(v.vault_set_tiers(tiers.clone()));
    assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, PHASE_PRICES_100K[1], 1), VAULT_WRONG_AMOUNT);
    let repriced = reset_amount_micro(&tiers_of(&v), size_100k, 1, &LADDER).unwrap();
    assert_eq!(repriced, 1_035_000_000, "ceil(900 x 1.15)");

    // remove the tier: the reset is blocked, whatever is paid
    let without: Vec<(u64, u64)> = tiers.iter().copied().filter(|t| t.0 != size_100k).collect();
    assert_ok(v.vault_set_tiers(without));
    assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, repriced, 1), VAULT_INVALID_CHALLENGE_TIER);
    assert!(reset_amount_micro(&tiers_of(&v), size_100k, 1, &LADDER).is_err());

    // re-adding the tier unblocks it, at the cost registered now
    assert_ok(v.vault_set_tiers(tiers));
    assert_ok(v.vault_deposit_reset(&trader, 0, 7, repriced, 1));
    assert_eq!(v.vault_trader_state(&tk, 7).unwrap().account_size, size_100k);
}

#[test]
fn two_tiers_of_one_size_are_refused_by_the_vault_when_the_costs_differ_and_by_the_sector_always() {
    let mut v = VaultEnv::new();
    let trader = failed_trader(&mut v, TIER_100K);
    let size_100k = 100_000 * USD;

    // different costs: the vault says InvalidChallengeTier forever for that size (a reset can never be paid)
    let mut tiers = tiers_of(&v);
    tiers.push((size_100k, 900 * USD));
    assert_ok(v.vault_set_tiers(tiers.clone()));
    for amount in [PHASE_PRICES_100K[0], 900 * USD] {
        assert_anchor_code(&v.vault_deposit_reset(&trader, 0, 7, amount, 0), VAULT_INVALID_CHALLENGE_TIER);
    }
    assert!(reset_amount_micro(&tiers_of(&v), size_100k, 0, &LADDER).is_err());

    // equal costs: the vault accepts it, the sector helper is stricter on purpose
    tiers.pop();
    tiers.push((size_100k, 859 * USD));
    assert_ok(v.vault_set_tiers(tiers));
    assert!(reset_amount_micro(&tiers_of(&v), size_100k, 0, &LADDER).is_err(), "the sector refuses duplicate sizes");
    assert_ok(v.vault_deposit_reset(&trader, 0, 7, PHASE_PRICES_100K[0], 0));
}
