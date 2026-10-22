//! M8: `reset_account` against the REAL vault fixture (core_vault.so, vault core commit 9a5c179, shared-interfaces v0.5.0),
//! through OUR instruction (tests-rs/tests/reset_price.rs proves the vault's side by calling `deposit_reset` directly).
//! Every failure test asserts the exact error and an unchanged world digest; the success tests assert the exact numbers.
mod common;

use anchor_lang::solana_program::pubkey::Pubkey;
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::*;
use journal_types::{genesis_hash, NativeSha256};
use setl8_lev_trading::{
    constants as c,
    errors::Setl8Error as E,
    events::{AccountReset, Opened},
    reset_price::reset_amount_micro,
    state::TradingAccount,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

const USD: u64 = 1_000_000;
const CAP: u64 = 100_000_000_000;
const DELAY: i64 = 86_400;
const WINDOW: u32 = 86_400;
const TIER_2_5K: u8 = 0;
const TIER_100K: u8 = 8;
const SIZE_100K: u64 = 100_000 * USD;
const SIZE_2_5K: u64 = 2_500 * USD;
/// The price of a reset by the phase the account failed in (index 0 = phase 1).
const PRICES_100K: [u64; 5] = [859_000_000, 987_850_000, 1_030_800_000, 1_095_225_000, 1_116_700_000];
const PRICES_2_5K: [u64; 5] = [30_000_000, 34_500_000, 36_000_000, 38_250_000, 39_000_000];
/// The 0.4% fee on top, rounded up: ceil(price x 40 / 10_000).
const FEES_100K: [u64; 5] = [3_436_000, 3_951_400, 4_123_200, 4_380_900, 4_466_800];
const FEES_2_5K: [u64; 5] = [120_000, 138_000, 144_000, 153_000, 156_000];
const VAULT_RESET_NOT_ALLOWED: u32 = 6011;

/// A world whose only account (challenge 0, tier `tier`) failed in `phase` through `report_fail`. Returns the clock after.
fn failed(phase: u8, tier: u8) -> (JEnv, i64) {
    let mut w = JEnv::with_tier(tier);
    let t = w.t0 + 1_000;
    w.set_limits(DELAY as u32, CAP, WINDOW);
    w.fail_in(phase, t);
    (w, t + 100)
}

fn fee_of(price: u64) -> u64 {
    u64::try_from((u128::from(price) * 40).div_ceil(10_000)).unwrap()
}

fn digest(w: &JEnv) -> [u8; 32] {
    w.pass_digest()
}

/// A call that must fail with exactly the vault's error (found in the logs) and change nothing.
fn fails_in_vault(w: &mut JEnv, needle: &str, f: impl FnOnce(&mut JEnv) -> litesvm::types::TransactionResult) {
    let before = digest(w);
    let r = f(w);
    let logs = match &r {
        Ok(_) => panic!("expected the vault to refuse with {needle}, but the call succeeded"),
        Err(e) => e.meta.logs.join("\n"),
    };
    assert!(logs.contains(needle), "expected {needle} in the logs:\n{logs}");
    assert_eq!(digest(w), before, "a rejected reset changed the world ({needle})");
}

fn genesis_of(acct: &Pubkey) -> [u8; 32] {
    genesis_hash(&NativeSha256, &acct.to_bytes())
}

/// The whole field list of a fresh account of `phase` (the body of DESIGN 5.1).
#[allow(clippy::too_many_arguments)]
fn assert_fresh(a: &TradingAccount, acct: &Pubkey, trader: &Pubkey, id: u64, size: u64, phase: u8, now: i64, next_request_id: u64) {
    let want = TradingAccount {
        trader: *trader,
        challenge_id: id,
        size_s: size,
        created_at: now,
        b: i64::try_from(size).unwrap(),
        q_tally: 0,
        lock_until: 0,
        last_batch_time: now,
        last_activity_pushed: now,
        next_request_id,
        seq: 0,
        journal_hash: genesis_of(acct),
        trades_closed: 0,
        ruleset_version: 1,
        status: c::STATUS_ACTIVE,
        phase,
        latch: 0,
        flat: 1,
        pass_pending: 0,
        vetoes: 0,
        bump: Pubkey::find_program_address(&[c::ACCT_SEED, trader.as_ref(), &id.to_le_bytes()], &PROGRAM_ID).1,
        tight_on: 0,
        held: 0,
        r: 0,
        g_peak: 0,
        l_level: 0,
        pass_requested_at: 0,
        pass_proposed_at: 0,
    };
    let (got, exp) = (bytes_of(a), bytes_of(&want));
    assert_eq!(got, exp, "the new account is not the fresh account of phase {phase} (bytes differ)");
}

fn bytes_of(a: &TradingAccount) -> Vec<u8> {
    use anchor_lang::AccountSerialize;
    let mut d = Vec::new();
    a.try_serialize(&mut d).unwrap();
    d
}

// ====================================================================== the price, all five phases, both tiers

fn five_phases(tier: u8, size: u64, prices: [u64; 5], fees: [u64; 5]) {
    for (i, (&price, &fee)) in prices.iter().zip(fees.iter()).enumerate() {
        let phase = u8::try_from(i + 1).unwrap();
        let (mut w, t) = failed(phase, tier);
        let tk = w.trader.pubkey();
        let tt = w.v.trader_token(&tk);
        let (bal0, pool0, sl80, fee0) = (w.v.token_balance(&tt), w.v.token_balance(&w.v.usdc_pool), w.v.token_balance(&w.v.vault_sl8_usdc), w.v.token_balance(&w.v.fee_usdc));
        // the vault copies the payouts of the failed record
        w.v.edit_vault_record(&tk, 0, None, Some(2));
        let wallet0 = w.wallet_profile();
        assert_eq!((wallet0.active_count, wallet0.next_challenge_id), (0, 1), "the failed account freed its slot");
        let prev0 = w.account();
        let prev_acct = w.acct;

        // the numbers the sector computes from the registry it reads
        let tiers = w.v.registry().challenge_sizes;
        assert_eq!(reset_amount_micro(&tiers, size, i, &RESET_LADDER_BPS).unwrap(), price);
        assert_eq!(fee_of(price), fee, "the 0.4% fee, rounded up");

        let m = w.reset_ok(t);

        // the trader pays the price plus the fee, nothing else
        assert_eq!(bal0 - w.v.token_balance(&tt), price + fee, "phase {phase}: price + fee");
        let (pool_part, sl8_part) = VaultEnv::vault_split(price);
        assert_eq!(w.v.token_balance(&w.v.usdc_pool), pool0 + pool_part, "the vault's pool share");
        assert_eq!(w.v.token_balance(&w.v.vault_sl8_usdc), sl80 + sl8_part, "the vault's SL8 share");
        assert_eq!(w.v.token_balance(&w.v.fee_usdc), fee0 + fee, "the 0.4% goes to the SL8 fee wallet");

        // the new local account: same phase, fresh state, payouts from the vault
        let a = w.account();
        assert_fresh(&a, &w.acct, &tk, 1, size, phase, t, 3);
        // the new vault record: size and payouts of the failed one
        let v1 = w.v.vault_trader_state(&tk, 1).expect("new vault record");
        assert_eq!((v1.account_size, v1.payout_count, v1.status, v1.trader_wallet, v1.product_program_id, v1.challenge_id), (size, 2, 0, tk, PROGRAM_ID, 1));
        // wallet: one live account again, the next id moved
        let wallet = w.wallet_profile();
        assert_eq!((wallet.active_count, wallet.next_challenge_id, wallet.banned, wallet.held_count), (1, 2, 0, 0));
        // the previous account: only the reset mark changed
        let prev = w.v.account(&prev_acct);
        assert_eq!(prev.reset_at(), t, "reset_at");
        let mut want_prev = prev0;
        want_prev.pass_requested_at = t;
        assert_eq!(bytes_of(&prev), bytes_of(&want_prev));
        // events: Opened (the same record as a purchase) then AccountReset
        let opened = cpi_events::<Opened>(&m);
        let resets = cpi_events::<AccountReset>(&m);
        assert_eq!((opened.len(), resets.len()), (1, 1));
        assert_eq!(
            (opened[0].account, opened[0].trader, opened[0].challenge_id, opened[0].size_s, opened[0].phase, opened[0].ruleset_version, opened[0].genesis_hash, opened[0].time),
            (w.acct, tk, 1, size, phase, 1, genesis_of(&w.acct), t)
        );
        assert_eq!(opened[0].tier_index, tier);
        let r = &resets[0];
        assert_eq!(
            (r.prev_account, r.account, r.trader, r.prev_challenge_id, r.challenge_id, r.phase, r.price, r.fee, r.payout_count, r.time),
            (prev_acct, w.acct, tk, 0, 1, phase, price, fee, 2, t)
        );
    }
}

#[test]
fn reset_charges_the_ladder_at_the_100k_tier_in_all_five_phases() {
    five_phases(TIER_100K, SIZE_100K, PRICES_100K, FEES_100K);
}

#[test]
fn reset_charges_the_ladder_at_the_2_5k_tier_in_all_five_phases() {
    // $30 for 2.5K is not proportional to $859 for 100K: only a tier-price formula gives these numbers
    five_phases(TIER_2_5K, SIZE_2_5K, PRICES_2_5K, FEES_2_5K);
}

#[test]
fn the_launch_ladder_numbers_are_the_ones_of_design_9_5() {
    // the arrays above ARE the DESIGN 9.5 table for the two tiers; recompute them independently of the helper
    for (cost, prices, fees) in [(859 * USD, PRICES_100K, FEES_100K), (30 * USD, PRICES_2_5K, FEES_2_5K)] {
        for (i, bps) in RESET_LADDER_BPS.iter().enumerate() {
            let p = u64::try_from((u128::from(cost) * u128::from(*bps)).div_ceil(10_000)).unwrap();
            assert_eq!(p, prices[i]);
            assert_eq!(fee_of(p), fees[i]);
        }
    }
}

// ====================================================================== same-phase resume, fresh state

#[test]
fn a_phase_3_failure_resets_into_phase_3_with_fresh_engine_state() {
    let (mut w, t) = failed(3, TIER_100K);
    let tk = w.trader.pubkey();
    // dirty everything the engine owns on the failed record: none of it may reach the new account
    w.edit_account(|a| {
        a.latch = 1;
        a.q_tally = 777_000;
        a.lock_until = t + 99_999;
        a.tight_on = 1;
        a.r = 1_000_000;
        a.g_peak = 2_000_000;
        a.l_level = 3_000_000;
        a.vetoes = 4;
        a.trades_closed = 9;
        a.seq = 12;
        a.journal_hash = [0xCD; 32];
        a.next_request_id = 7;
        a.flat = 0;
    });
    let m = w.reset_ok(t + 5);
    let a = w.account();
    // the vault says payout_count 0 for this record: the new account's next request id follows the VAULT, not the local record
    assert_fresh(&a, &w.acct, &tk, 1, SIZE_100K, 3, t + 5, 1);
    assert_eq!(cpi_events::<AccountReset>(&m)[0].payout_count, 0);
    // the new journal starts from its own genesis: sha256("SL8LEV" || new account)
    assert_ne!(a.journal_hash, genesis_of(&acct_pda(&tk, 0)));
    assert_eq!(a.journal_hash, genesis_of(&acct_pda(&tk, 1)));
}

#[test]
fn the_new_account_trades_passes_and_the_chain_starts_at_its_own_genesis() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    w.reset_ok(t);
    let t = t + 100;
    // a real journal batch on the NEW account: +8,000 closed profit = 1.08 S exactly
    let b = simple(&w, t, vec![close_long_pnl(t, t - 100, 8_000_000_000)], true);
    assert_eq!((b.first_seq, b.prev_hash), (1, genesis_of(&acct_pda(&tk, 1))));
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, S100K + 8_000_000_000);
    // the old, failed account takes nothing more
    let old = acct_pda(&tk, 0);
    let before = w.v.account(&old);
    w.ok(|w| w.request(t + 10));
    assert_eq!(w.account().pass_pending, 1);
    assert_eq!(bytes_of(&w.v.account(&old)), bytes_of(&before));
}

// ====================================================================== payouts carry over (the vault's payout_count)

/// The whole chain with REAL passes: phases 2, 3, 4 paid (3 payouts), fail in phase 5, reset, the new account earns ONLY the
/// remaining one (payout 4), the vault graduates it, and nothing more is possible.
#[test]
fn a_phase_5_reset_after_three_payouts_can_earn_only_the_remaining_one() {
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let payer = w.new_payer();
    let tk = w.trader.pubkey();
    let mut t = w.t0 + 1_000;
    t = w.advance_to(5, t, &payer, DELAY);
    // three payouts so far, claims 1 to 3 against the FIRST vault record
    let first_vts = vault_trader_state_pda(&tk, 0);
    assert_eq!(w.vault_status().payout_count, 3);
    assert_eq!(w.account().next_request_id, 4);
    assert_eq!(w.tally().requested_count, 3);
    for k in 1..=3 {
        assert_eq!(w.claim(k).unwrap().owed, PAYOUT_100K);
    }
    // fail in phase 5, reset
    w.fail_in(5, t);
    t += 100;
    w.reset_ok(t);
    // the vault carried the three payouts over; so does the new local account (next request id 4)
    let a = w.account();
    assert_eq!((a.phase, a.next_request_id), (5, 4));
    assert_eq!(w.vault_status().payout_count, 3);
    assert_eq!(w.v.vault_trader_state(&tk, 1).unwrap().payout_count, 3);
    // the new account passes phase 5: ONE payout, request id 4, against the NEW vault record
    t += 100;
    w.ready(5);
    w.ok(|w| w.request(t));
    w.ok(|w| w.confirm(t + 1));
    let fin_at = t + 1 + DELAY;
    let m = w.ok(|w| w.finalize(fin_at, &payer));
    let ev = cpi_events::<setl8_lev_trading::events::PassFinalized>(&m);
    assert_eq!((ev[0].phase, ev[0].to_phase, ev[0].amount, ev[0].request_id), (5, 0, PAYOUT_100K, 4));
    assert_eq!(w.claim(4).unwrap().owed, PAYOUT_100K);
    // graduated at the vault's max_payout_count 4: local and vault agree
    assert_eq!(w.account().status, c::STATUS_GRADUATED);
    assert_eq!((w.vault_status().status, w.vault_status().payout_count), (1, 4));
    // the tally equals the vault's claims: four payouts in the whole chain, in total, across both vault records
    assert_eq!((w.tally().requested_count, w.tally().requested_total), (4, 4 * PAYOUT_100K));
    assert_eq!(w.registry_totals(), (4, 4 * PAYOUT_100K));
    // nothing more can be earned: the graduated account takes no request, and the first record has no claim 4
    w.fails(E::AccountNotActive, |w| w.request(fin_at + 10));
    let old_claim = claim_pda(&first_vts, 4);
    assert!(w.v.e.svm.get_account(&old_claim).is_none() || w.v.e.svm.get_account(&old_claim).unwrap().data.is_empty());
}

#[test]
fn the_vaults_payout_count_not_the_local_record_decides_the_next_request_id() {
    let (mut w, t) = failed(4, TIER_100K);
    let tk = w.trader.pubkey();
    // local record says next_request_id 9; the vault record says 2 payouts -> 3
    w.edit_account(|a| a.next_request_id = 9);
    w.v.edit_vault_record(&tk, 0, None, Some(2));
    w.reset_ok(t);
    assert_eq!(w.account().next_request_id, 3);
    // and the other way round: local says 1 (a fresh phase-1 record), the vault says 3
    let (mut w2, t2) = failed(4, TIER_100K);
    let tk2 = w2.trader.pubkey();
    w2.v.edit_vault_record(&tk2, 0, None, Some(3));
    w2.reset_ok(t2);
    assert_eq!(w2.account().next_request_id, 4);
    assert_eq!(w2.v.vault_trader_state(&tk2, 1).unwrap().payout_count, 3);
}

// ====================================================================== every phase: fail, reset, trade, pass, finalize

/// Fails in phase k (after k - 2 real payouts for k >= 2), resets, then walks the NEW account through every remaining phase
/// with real passes. The vault pays exactly 4 payouts across the whole chain and graduates at the right one.
fn full_chain(k: u8) {
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let payer = w.new_payer();
    let tk = w.trader.pubkey();
    let mut t = w.t0 + 1_000;
    t = w.advance_to(k, t, &payer, DELAY);
    let paid_before = u64::from(k.saturating_sub(2));
    assert_eq!(w.vault_status().payout_count, paid_before);
    w.fail_in(k, t);
    t += 100;
    w.reset_ok(t);
    assert_eq!((w.account().phase, w.account().next_request_id), (k, paid_before + 1));
    let new_vts = vault_trader_state_pda(&tk, 1);
    assert_eq!(w.vts(), new_vts);
    t += 100;

    // trade in the new account: one real batch reaching the target
    let reach = |w: &mut JEnv, t: i64| {
        let b = simple(w, t, vec![close_long_pnl(t, t - 100, 8_000_000_000)], true);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(w.account().b, TARGET_100K);
    };
    reach(&mut w, t);
    t += 10;
    for phase in k..=5 {
        if phase > k {
            w.ready(phase);
        }
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        if phase >= 2 {
            let m = w.ok(|w| w.finalize(t + 1 + DELAY, &payer));
            let ev = cpi_events::<setl8_lev_trading::events::PassFinalized>(&m);
            // the payout number continues the chain: phase p is payout p - 1
            assert_eq!((ev[0].phase, ev[0].amount, ev[0].request_id), (phase, PAYOUT_100K, u64::from(phase) - 1));
            assert_eq!(w.claim(u64::from(phase) - 1).unwrap().owed, PAYOUT_100K);
            t += 1 + DELAY;
        }
        t += 100;
    }
    // graduated, 4 payouts in all, the vault and the local record agree
    assert_eq!(w.account().status, c::STATUS_GRADUATED);
    assert_eq!((w.vault_status().status, w.vault_status().payout_count), (1, 4));
    assert_eq!(w.tally().requested_count, 4);
    assert_eq!(w.tally().requested_total, 4 * PAYOUT_100K);
    assert_eq!(w.registry_totals(), (4, 4 * PAYOUT_100K));
    assert_eq!(w.wallet_profile().active_count, 0);
}

const TARGET_100K: i64 = 108_000_000_000;

#[test]
fn full_chain_fail_in_phase_1_reset_trade_pass_to_graduation() {
    full_chain(1);
}
#[test]
fn full_chain_fail_in_phase_2_reset_trade_pass_to_graduation() {
    full_chain(2);
}
#[test]
fn full_chain_fail_in_phase_3_reset_trade_pass_to_graduation() {
    full_chain(3);
}
#[test]
fn full_chain_fail_in_phase_4_reset_trade_pass_to_graduation() {
    full_chain(4);
}
#[test]
fn full_chain_fail_in_phase_5_reset_trade_pass_to_graduation() {
    full_chain(5);
}

#[test]
fn a_reset_account_that_fails_again_can_be_reset_again_and_the_payouts_still_count_once() {
    let mut w = JEnv::new();
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let payer = w.new_payer();
    let mut t = w.t0 + 1_000;
    t = w.advance_to(4, t, &payer, DELAY);
    assert_eq!(w.vault_status().payout_count, 2);
    for round in 0..3u64 {
        w.fail_in(4, t);
        t += 100;
        w.reset_ok(t);
        t += 100;
        assert_eq!(w.account().challenge_id, round + 1);
        assert_eq!((w.account().phase, w.account().next_request_id), (4, 3), "round {round}");
        assert_eq!(w.vault_status().payout_count, 2);
        assert_eq!(w.wallet_profile().active_count, 1);
    }
    // the reset price is paid every time
    assert_eq!(w.tally().requested_count, 2);
}

// ====================================================================== the price follows the registry as it is NOW

#[test]
fn a_tier_repriced_between_the_fail_and_the_reset_is_followed() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    let mut tiers = w.v.registry().challenge_sizes;
    for x in tiers.iter_mut().filter(|x| x.0 == SIZE_100K) {
        x.1 = 900 * USD;
    }
    assert_ok(w.v.vault_set_tiers(tiers));
    let bal0 = w.v.token_balance(&w.v.trader_token(&tk));
    let m = w.reset_ok(t);
    // ceil(900 x 1.15) = 1,035.00 and the fee ceil(1,035 x 0.004) = 4.14
    assert_eq!((cpi_events::<AccountReset>(&m)[0].price, cpi_events::<AccountReset>(&m)[0].fee), (1_035_000_000, 4_140_000));
    assert_eq!(bal0 - w.v.token_balance(&w.v.trader_token(&tk)), 1_035_000_000 + 4_140_000);
}

#[test]
fn a_tier_removed_between_the_fail_and_the_reset_blocks_it_before_any_cpi_and_returns_when_re_added() {
    let (mut w, t) = failed(2, TIER_100K);
    let tiers = w.v.registry().challenge_sizes;
    let without: Vec<(u64, u64)> = tiers.iter().copied().filter(|x| x.0 != SIZE_100K).collect();
    assert_ok(w.v.vault_set_tiers(without));
    w.fails(E::ResetTierInvalid, |w| w.reset(t));
    // the failed record is untouched and resettable once the tier is back (the deploy rule: never remove a tier while Failed records exist)
    assert_eq!(w.account().reset_at(), 0);
    assert_ok(w.v.vault_set_tiers(tiers));
    w.reset_ok(t);
    assert_eq!(w.account().phase, 2);
}

#[test]
fn duplicate_tier_sizes_are_refused_before_any_cpi_even_with_equal_costs() {
    let (mut w, t) = failed(3, TIER_100K);
    let mut tiers = w.v.registry().challenge_sizes;
    // equal-cost duplicate: the VAULT would accept it, the sector refuses on purpose
    tiers.push((SIZE_100K, 859 * USD));
    assert_ok(w.v.vault_set_tiers(tiers.clone()));
    w.fails(E::ResetTierInvalid, |w| w.reset(t));
    // different-cost duplicate: the sector says the same (the vault would say InvalidChallengeTier forever)
    tiers.pop();
    tiers.push((SIZE_100K, 900 * USD));
    assert_ok(w.v.vault_set_tiers(tiers));
    w.fails(E::ResetTierInvalid, |w| w.reset(t));
}

#[test]
fn a_ladder_shorter_than_the_failed_phase_is_refused_before_any_cpi() {
    let (mut w, t) = failed(5, TIER_100K);
    let tiers = w.v.registry().challenge_sizes;
    assert_ok(w.v.vault_set_config(tiers.clone(), vec![10_000, 11_500, 12_000, 12_750]));
    w.fails(E::ResetPhaseOutOfRange, |w| w.reset(t));
    // an empty ladder offers no resets at all
    assert_ok(w.v.vault_set_config(tiers, vec![]));
    w.fails(E::ResetPhaseOutOfRange, |w| w.reset(t));
}

#[test]
fn a_price_that_does_not_fit_u64_is_refused_before_any_cpi() {
    let (mut w, t) = failed(5, TIER_100K);
    let mut tiers = w.v.registry().challenge_sizes;
    for x in tiers.iter_mut().filter(|x| x.0 == SIZE_100K) {
        x.1 = u64::MAX;
    }
    // the vault takes such a tier at registration (it has no upper bound on the cost): the sector refuses the price
    assert_ok(w.v.vault_set_tiers(tiers));
    w.fails(E::ResetPriceOverflow, |w| w.reset(t));
}

#[test]
fn the_new_account_follows_the_current_rule_set_and_its_fee_rate() {
    // the failed account was bought under rule set 1; rule set 2 (fee 0.5%) is current when it is reset
    let (mut w, t) = failed(3, TIER_100K);
    let mut v2 = launch_ruleset_args();
    v2.version = 2;
    v2.fee_bps = 50;
    w.v.e.create_ruleset_ok(&v2);
    assert_eq!(w.account().ruleset_version, 1);
    let mut ra = w.reset_accts();
    ra.ruleset = ruleset_pda(2);
    let tk = w.trader.pubkey();
    let bal0 = w.v.token_balance(&w.v.trader_token(&tk));
    let m = w.ok(|w| w.send_reset(t, &ra, 0));
    w.acct = ra.trading_account;
    let fee = u64::try_from((u128::from(PRICES_100K[2]) * 50).div_ceil(10_000)).unwrap();
    assert_eq!(fee, 5_154_000, "0.5% of 1,030.80 USDC");
    assert_eq!(bal0 - w.v.token_balance(&w.v.trader_token(&tk)), PRICES_100K[2] + fee);
    assert_eq!(cpi_events::<AccountReset>(&m)[0].fee, fee);
    let a = w.account();
    assert_eq!((a.ruleset_version, a.phase), (2, 3));
    assert_eq!(cpi_events::<Opened>(&m)[0].ruleset_version, 2);
}

#[test]
fn the_vault_record_it_creates_must_carry_the_size_of_the_account_being_reset() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    // make the 50K tier cost the same as the 100K tier, and point the vault's failed record at the 50K size: the vault prices
    // the reset off ITS record (same amount), the new vault record gets the 50K size, the local account would be 100K
    let mut tiers = w.v.registry().challenge_sizes;
    for x in tiers.iter_mut().filter(|x| x.0 == 50_000 * USD) {
        x.1 = 859 * USD;
    }
    assert_ok(w.v.vault_set_tiers(tiers));
    w.v.edit_vault_record(&tk, 0, Some(50_000 * USD), None);
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    // the honest record resets
    w.v.edit_vault_record(&tk, 0, Some(SIZE_100K), None);
    w.reset_ok(t);
}

// ====================================================================== check order (each check alone, then in pairs)

#[test]
fn a_paused_sector_blocks_reset_and_unpausing_allows_it() {
    let (mut w, t) = failed(2, TIER_100K);
    w.pause_sector(true);
    w.fails(E::Paused, |w| w.reset(t));
    w.pause_sector(false);
    w.reset_ok(t);
}

#[test]
fn a_banned_wallet_cannot_reset_and_can_after_the_unban() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    let admins = Admins::both(&w.v.e);
    let ix = ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
    assert_ok(w.v.e.send_admins(ix, admins));
    w.fails(E::WalletBanned, |w| w.reset(t));
    let admins = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tk), admins));
    w.reset_ok(t);
}

#[test]
fn an_active_account_cannot_be_reset() {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.fails(E::ResetNotAllowed, |w| w.reset(t));
    // the vault record is Active too: it would refuse as well (6011), but the sector refuses first
    assert_eq!(w.vault_status().status, 0);
}

#[test]
fn a_held_account_cannot_be_reset() {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    assert_ok(w.hold(t, hash(7)));
    assert_eq!(w.account().held, 1);
    w.fails(E::ResetNotAllowed, |w| w.reset(t + 10));
}

#[test]
fn abandoned_and_graduated_accounts_cannot_be_reset() {
    for status in [c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.make_terminal(status, t);
        w.fails(E::ResetNotAllowed, |w| w.reset(t + 10));
    }
}

#[test]
fn a_previous_account_named_wrongly_is_refused() {
    let (mut w, t) = failed(2, TIER_100K);
    // the argument names another challenge than the account passed
    let ra = w.reset_accts();
    w.fails(E::ResetNotAllowed, |w| w.send_reset(t, &ra, 5));
    w.fails(E::ResetNotAllowed, |w| w.send_reset(t, &ra, 1));
}

#[test]
fn another_traders_failed_account_cannot_be_reset() {
    let (mut w, t) = failed(2, TIER_100K);
    let other = w.v.new_trader();
    assert_ok(w.v.open(&other, 8));
    // the other trader's account failed in phase 2 (we switch the fixture's trader/account to fail it, then back)
    let (mine, my_acct) = (dup(&w.trader), w.acct);
    w.trader = dup(&other);
    w.acct = acct_pda(&other.pubkey(), 0);
    w.fail_in(2, t);
    let other_acct = w.acct;
    w.trader = mine;
    w.acct = my_acct;

    // I name MY wallet and signature but THEIR failed account (and their vault record)
    let mut ra = w.reset_accts();
    ra.prev_trading_account = other_acct;
    ra.prev_vault_trader_state = vault_trader_state_pda(&other.pubkey(), 0);
    let prev_id = 0;
    w.fails(E::Unauthorized, |w| w.send_reset(t, &ra, prev_id));
    // nothing moved for the other trader either
    assert_eq!(w.v.account(&other_acct).reset_at(), 0);
}

#[test]
fn a_second_reset_of_one_failed_record_is_refused_locally_before_any_cpi_and_by_the_vault_as_backstop() {
    let (mut w, t) = failed(3, TIER_100K);
    let tk = w.trader.pubkey();
    let prev = w.acct;
    let ra = w.reset_accts();
    w.reset_ok(t);
    // the same failed record again: the sector's own mark refuses it
    let mut again = ra.clone();
    again.trading_account = acct_pda(&tk, 2);
    again.vault_trader_state = vault_trader_state_pda(&tk, 2);
    let before = digest(&w);
    assert_err(&w.send_reset(t + 1, &again, 0), E::AccountAlreadyReset);
    assert_eq!(digest(&w), before);
    // backstop: with the local mark wiped, the VAULT refuses (ResetNotAllowed 6011): once only
    let mut a = w.v.account(&prev);
    a.pass_requested_at = 0;
    let mut data = Vec::new();
    anchor_lang::AccountSerialize::try_serialize(&a, &mut data).unwrap();
    let mut acc = w.v.e.svm.get_account(&prev).unwrap();
    acc.data = data;
    w.v.e.svm.set_account(prev, acc).unwrap();
    let before = digest(&w);
    let r = w.send_reset(t + 2, &again, 0);
    assert_anchor_code(&r, VAULT_RESET_NOT_ALLOWED);
    assert_eq!(digest(&w), before);
}

#[test]
fn the_sixth_active_account_is_refused_and_the_fifth_is_allowed() {
    let (mut w, t) = failed(2, TIER_2_5K);
    let tr = dup(&w.trader);
    for _ in 0..4 {
        assert_ok(w.v.open(&tr, 0));
    }
    assert_eq!(w.wallet_profile().active_count, 4);
    // the failed account is not counted; the new one is the fifth: allowed
    let (mut w5, t5) = (w, t);
    w5.reset_ok(t5);
    assert_eq!(w5.wallet_profile().active_count, 5);

    let (mut w, t) = failed(2, TIER_2_5K);
    let tr = dup(&w.trader);
    for _ in 0..5 {
        assert_ok(w.v.open(&tr, 0));
    }
    assert_eq!(w.wallet_profile().active_count, 5);
    w.fails(E::WalletCapReached, |w| w.reset(t));
    // freeing one slot (an account of the wallet fails) lets the reset through
    w.acct = acct_pda(&tr.pubkey(), 1);
    w.fail_in(2, t);
    w.acct = acct_pda(&tr.pubkey(), 0);
    w.reset_ok(t + 200);
    assert_eq!(w.wallet_profile().active_count, 5);
}

#[test]
fn the_payer_must_be_the_trader_and_both_must_sign() {
    let (mut w, t) = failed(2, TIER_100K);
    let other = w.v.new_trader();
    let mut ra = w.reset_accts();
    ra.payer = other.pubkey();
    let prev_id = 0;
    w.set_time(t);
    let ix = ra.to_ix(prev_id);
    let (tr, ot) = (dup(&w.trader), dup(&other));
    let before = digest(&w);
    let r = w.v.e.send(&[ix], &[&tr, &ot]);
    assert_err(&r, E::Unauthorized);
    assert_eq!(digest(&w), before);
    // trader and payer are one key, so Solana merges their signer flags: the key is unsigned only if BOTH are cleared
    let mut ra = w.reset_accts();
    ra.trader_signs = false;
    ra.payer_signs = false;
    w.fails_anchor(3010, |w| w.send_reset(t, &ra, 0));
    // a payer that is not the trader and does not sign is an Anchor signer error (before the sector's own check)
    let mut ra = w.reset_accts();
    ra.payer = other.pubkey();
    ra.payer_signs = false;
    w.fails_anchor(3010, |w| w.send_reset(t, &ra, 0));
    // the trader unsigned while another key pays: the vault would need the trader's signature, the sector says Unauthorized first
    let mut ra = w.reset_accts();
    ra.payer = other.pubkey();
    ra.trader_signs = false;
    w.fails_anchor(3010, |w| w.send_reset(t, &ra, 0));
}

#[test]
fn a_wrong_vault_program_or_sector_authority_or_ruleset_is_refused() {
    let (mut w, t) = failed(2, TIER_100K);
    let mut ra = w.reset_accts();
    ra.vault_program = Pubkey::new_unique();
    w.fails(E::InvalidVaultProgram, |w| w.send_reset(t, &ra, 0));
    let mut ra = w.reset_accts();
    ra.sector_authority = Pubkey::new_unique();
    w.fails_anchor(2006, |w| w.send_reset(t, &ra, 0));
    // a stale rule set (config says 2, the call passes 1)
    let mut v2 = launch_ruleset_args();
    v2.version = 2;
    w.v.e.create_ruleset_ok(&v2);
    w.fails(E::RuleSetMissing, |w| w.reset(t));
}

#[test]
fn the_order_of_the_checks_is_pinned_pair_by_pair() {
    // paused beats banned
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    let admins = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [1; 32]), admins));
    w.pause_sector(true);
    w.fails(E::Paused, |w| w.reset(t));
    w.pause_sector(false);
    // banned beats a record that is not resettable (the previous account is also marked reset after a first reset below)
    let mut a = w.account();
    a.pass_requested_at = 5;
    let mut data = Vec::new();
    anchor_lang::AccountSerialize::try_serialize(&a, &mut data).unwrap();
    let mut acc = w.v.e.svm.get_account(&w.acct).unwrap();
    acc.data = data;
    w.v.e.svm.set_account(w.acct, acc).unwrap();
    w.fails(E::WalletBanned, |w| w.reset(t));
    let admins = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tk), admins));
    // already reset beats the cap and a bad registry
    w.edit_wallet_active(5);
    let mut ra = w.reset_accts();
    ra.product_registry = Pubkey::new_unique();
    w.fails(E::AccountAlreadyReset, |w| w.send_reset(t, &ra, 0));
    // an account that is not Failed is ResetNotAllowed before the reset mark is even looked at (a synthetic state: the mark is
    // only ever written on a Failed account)
    w.edit_account(|a| a.status = c::STATUS_ACTIVE);
    w.fails(E::ResetNotAllowed, |w| w.send_reset(t, &ra, 0));
    w.edit_account(|a| a.status = c::STATUS_FAILED);
    // cap beats a bad registry
    w.edit_account(|a| a.pass_requested_at = 0);
    w.fails(E::WalletCapReached, |w| w.send_reset(t, &ra, 0));
    // bad registry beats the price
    w.edit_wallet_active(0);
    w.fails(E::InvalidVaultAccount, |w| w.send_reset(t, &ra, 0));
}

impl JEnv {
    fn edit_wallet_active(&mut self, n: u8) {
        let tk = self.trader.pubkey();
        self.v.edit_wallet(&tk, |wlt| wlt.active_count = n);
    }
}

// ====================================================================== the registry is the vault-owned PDA of THIS product

#[test]
fn a_registry_that_is_not_the_vault_owned_pda_of_this_product_is_refused() {
    let (mut w, t) = failed(2, TIER_100K);
    let real = w.v.e.svm.get_account(&registry_pda()).unwrap();
    // 1. the vault's other accounts, a system account, an unrelated key
    let wrong = [w.v.vault_state, w.v.usdc_pool, Pubkey::new_unique(), w.v.e.payer.pubkey(), w.v.fee_usdc];
    for k in wrong {
        let mut ra = w.reset_accts();
        ra.product_registry = k;
        w.fails(E::InvalidVaultAccount, |w| w.send_reset(t, &ra, 0));
    }
    // 2. a perfect COPY of the registry (cheap tiers) at another address, owned by the vault: the address is wrong
    let fake = Pubkey::new_unique();
    let mut cheap = real.clone();
    // tier 8 cost 859 USDC -> 1 micro-USDC, in place: offset = 8 disc + 32 + 4 + 8 tiers x 16 + 8 size
    let off = 8 + 32 + 4 + 8 * 16 + 8;
    cheap.data[off..off + 8].copy_from_slice(&1u64.to_le_bytes());
    w.v.e.svm.set_account(fake, cheap).unwrap();
    let mut ra = w.reset_accts();
    ra.product_registry = fake;
    w.fails(E::InvalidVaultAccount, |w| w.send_reset(t, &ra, 0));
    // 3. the right address with another owner
    let mut moved = real.clone();
    moved.owner = Pubkey::new_unique();
    w.v.e.svm.set_account(registry_pda(), moved).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    // 4. the right address and owner but a bad header (discriminator) or another product id
    let mut bad_disc = real.clone();
    bad_disc.data[0] ^= 0xFF;
    w.v.e.svm.set_account(registry_pda(), bad_disc).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    let mut bad_prod = real.clone();
    bad_prod.data[8] ^= 0xFF;
    w.v.e.svm.set_account(registry_pda(), bad_prod).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    // 5. truncated
    let mut short = real.clone();
    short.data.truncate(40);
    w.v.e.svm.set_account(registry_pda(), short).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    // the genuine registry works again
    w.v.e.svm.set_account(registry_pda(), real).unwrap();
    w.reset_ok(t);
}

#[test]
fn a_previous_vault_record_that_is_not_the_vault_owned_pda_of_the_failed_challenge_is_refused() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    assert_ok(w.v.open(&dup(&w.trader), 8));
    // a record of ANOTHER challenge of the same trader, a random key, the new record's own address
    for k in [vault_trader_state_pda(&tk, 1), Pubkey::new_unique(), vault_trader_state_pda(&tk, 2)] {
        let mut ra = w.reset_accts();
        ra.prev_vault_trader_state = k;
        w.fails(E::InvalidVaultAccount, |w| w.send_reset(t, &ra, 0));
    }
    // the right address, owned by another program
    let real = w.v.e.svm.get_account(&vault_trader_state_pda(&tk, 0)).unwrap();
    let mut moved = real.clone();
    moved.owner = Pubkey::new_unique();
    w.v.e.svm.set_account(vault_trader_state_pda(&tk, 0), moved).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    // the right address, a record of another wallet
    let mut other = real.clone();
    other.data[8] ^= 1;
    w.v.e.svm.set_account(vault_trader_state_pda(&tk, 0), other).unwrap();
    w.fails(E::InvalidVaultAccount, |w| w.reset(t));
    w.v.e.svm.set_account(vault_trader_state_pda(&tk, 0), real).unwrap();
    // the reset targets challenge 2 now (the next id was taken by the extra open)
    let m = w.reset_ok(t);
    assert_eq!(cpi_events::<Opened>(&m)[0].challenge_id, 2);
}

// ====================================================================== the vault's own refusals roll everything back

#[test]
fn a_paused_vault_product_blocks_the_reset_and_changes_nothing() {
    let (mut w, t) = failed(2, TIER_100K);
    w.pause_vault_product();
    fails_in_vault(&mut w, "ProductNotActive", |w| w.reset(t));
    // the failed record can still be reset once the product runs again? (reactivation is the vault admins' business) -
    // here it only matters that nothing was marked
    assert_eq!(w.account().reset_at(), 0);
}

#[test]
fn a_payer_without_funds_cannot_reset_for_the_price_or_for_the_price_plus_the_fee() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    let tt = w.v.trader_token(&tk);
    let usdc = w.v.usdc;
    let (price, fee) = (PRICES_100K[1], FEES_100K[1]);
    // less than the price: the vault's transfer fails
    w.v.set_token_account(&tt, &usdc, &tk, price - 1);
    let before = digest(&w);
    let r = w.reset(t);
    assert!(r.is_err());
    assert_eq!(digest(&w), before);
    // the price but not the fee: the sector's fee transfer fails and the vault's payment is rolled back with it
    w.v.set_token_account(&tt, &usdc, &tk, price + fee - 1);
    let before = digest(&w);
    let r = w.reset(t);
    assert!(r.is_err());
    assert_eq!(digest(&w), before);
    assert_eq!(w.account().reset_at(), 0);
    // exactly price + fee works and leaves nothing
    w.v.set_token_account(&tt, &usdc, &tk, price + fee);
    w.reset_ok(t);
    assert_eq!(w.v.token_balance(&tt), 0);
}

#[test]
fn a_wrong_mint_or_token_account_is_refused() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    // the vault's other mint with the trader's USDC account: the sector's token account constraint
    let mut ra = w.reset_accts();
    ra.mint = w.v.usdt;
    w.fails(E::InvalidTokenAccount, |w| w.send_reset(t, &ra, 0));
    // another owner's token account
    let usdc = w.v.usdc;
    let foreign = w.v.new_token_account(&usdc, &Pubkey::new_unique(), TRADER_START);
    let mut ra = w.reset_accts();
    ra.trader_token_account = foreign;
    w.fails(E::InvalidTokenAccount, |w| w.send_reset(t, &ra, 0));
    // a mint the vault does not accept (with a matching token account of the trader)
    let fake_mint = Pubkey::new_unique();
    w.v.set_mint(&fake_mint);
    let fake_ta = w.v.new_token_account(&fake_mint, &tk, TRADER_START);
    let fee_wallet = w.v.fee_wallet;
    let fake_fee = w.v.new_token_account(&fake_mint, &fee_wallet, 0);
    let mut ra = w.reset_accts();
    ra.mint = fake_mint;
    ra.trader_token_account = fake_ta;
    ra.fee_token_account = fake_fee;
    fails_in_vault(&mut w, "InvalidMint", |w| w.send_reset(t, &ra, 0));
    // the USDT pool with the USDC mint: the vault checks the pool against the mint
    let mut ra = w.reset_accts();
    ra.pool_token_account = w.v.usdt_pool;
    let before = digest(&w);
    assert!(w.send_reset(t, &ra, 0).is_err());
    assert_eq!(digest(&w), before);
    // a fee account of another owner than config.sl8_wallet
    let mut ra = w.reset_accts();
    ra.fee_token_account = foreign;
    w.fails(E::InvalidTokenAccount, |w| w.send_reset(t, &ra, 0));
}

#[test]
fn a_reset_in_usdt_works_too() {
    let (mut w, t) = failed(2, TIER_100K);
    let tk = w.trader.pubkey();
    let (usdt, pool, ) = (w.v.usdt, w.v.usdt_pool);
    let ta = w.v.new_token_account(&usdt, &tk, TRADER_START);
    // the vault's SL8 destination and the fee wallet for USDT
    let sl8 = w.v.new_token_account(&usdt, &VAULT_SL8_ADMIN, 0);
    let fee_wallet = w.v.fee_wallet;
    let fee_ta = w.v.new_token_account(&usdt, &fee_wallet, 0);
    let mut ra = w.reset_accts();
    ra.mint = usdt;
    ra.trader_token_account = ta;
    ra.pool_token_account = pool;
    ra.vault_sl8_token_account = sl8;
    ra.fee_token_account = fee_ta;
    assert_ok(w.send_reset(t, &ra, 0));
    assert_eq!(w.v.token_balance(&ta), TRADER_START - PRICES_100K[1] - FEES_100K[1]);
    assert_eq!(w.v.token_balance(&fee_ta), FEES_100K[1]);
    let (pool_part, sl8_part) = VaultEnv::vault_split(PRICES_100K[1]);
    assert_eq!((w.v.token_balance(&pool), w.v.token_balance(&sl8)), (pool_part, sl8_part));
}

// ====================================================================== no free-reset path (OPEN_QUESTIONS #9)

#[test]
fn there_is_no_free_reset_path() {
    let (mut w, t) = failed(2, TIER_2_5K);
    // the instruction carries only the previous challenge id: no amount, no phase, no payout count
    let ra = w.reset_accts();
    let ix = ra.to_ix(0);
    assert_eq!(ix.data.len(), 8 + 8, "discriminator + prev_challenge_id");
    // no admin, evaluator or authority signs: the signer set is the trader (and the payer, the same key)
    let signers: std::collections::BTreeSet<Pubkey> = ix.accounts.iter().filter(|m| m.is_signer).map(|m| m.pubkey).collect();
    assert_eq!(signers, [w.trader.pubkey()].into_iter().collect());
    // every successful reset moves price + fee out of the trader's account
    let tt = w.v.trader_token(&w.trader.pubkey());
    let bal0 = w.v.token_balance(&tt);
    w.reset_ok(t);
    assert!(bal0 - w.v.token_balance(&tt) >= PRICES_2_5K[1] + FEES_2_5K[1]);
    // the evaluator cannot reset on the trader's behalf (the trader's signature is part of the vault CPI)
    let (mut w2, t2) = failed(2, TIER_2_5K);
    let mut ra = w2.reset_accts();
    ra.trader_signs = false;
    ra.payer_signs = false;
    w2.fails_anchor(3010, |w| w.send_reset(t2, &ra, 0));
}

// ====================================================================== wallet and vault bookkeeping

#[test]
fn a_violation_fail_bans_the_wallet_so_the_reset_is_refused() {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    assert_ok(w.hold(t, hash(3)));
    assert_ok(w.violation(t + 10, hash(3)));
    assert_eq!(w.account().status, c::STATUS_FAILED);
    w.fails(E::WalletBanned, |w| w.reset(t + 20));
}

#[test]
fn a_failed_account_with_the_hold_released_is_resettable_and_the_hold_count_returns_to_zero() {
    // P25c (decision 67 amendment): report_fail keeps the hold; the reset waits for the two admins (held_fail_review.rs has the rest)
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    assert_ok(w.hold(t, hash(3)));
    assert_eq!(w.wallet_profile().held_count, 1);
    w.fail_in(2, t + 10);
    assert_eq!((w.wallet_profile().held_count, w.account().held), (1, 1));
    w.fails(E::ReviewPending, |w| w.reset(t + 200));
    assert_ok(w.release_hold(t + 300));
    assert_eq!(w.wallet_profile().held_count, 0);
    w.reset_ok(t + 400);
    assert_eq!((w.wallet_profile().held_count, w.account().held), (0, 0));
}

#[test]
fn the_new_account_is_a_normal_account_close_account_works_and_the_old_one_closes_independently() {
    let (mut w, t) = failed(2, TIER_100K);
    let old = w.acct;
    w.reset_ok(t);
    let new = w.acct;
    // the old record closes after 7 days (the reset did not change terminal_at)
    w.acct = old;
    let terminal = w.account().terminal_at();
    assert_ok(w.close(terminal + RETENTION));
    assert!(w.v.e.svm.get_account(&old).is_none_or(|a| a.lamports == 0));
    w.acct = new;
    assert_eq!(w.account().status, c::STATUS_ACTIVE);
    // the next purchase takes the next id
    let tr = dup(&w.trader);
    assert_ok(w.v.open(&tr, 8));
    assert_eq!(w.wallet_profile().next_challenge_id, 3);
}

// ====================================================================== compute units

#[test]
fn compute_units_per_phase_and_tier() {
    // the PDA bump searches depend on the (random) keys of the trader, so every cell is eight fresh worlds: min / median / max
    let mut worst = 0;
    for tier in [TIER_100K, TIER_2_5K] {
        for phase in 1..=5u8 {
            let mut v = Vec::new();
            for _ in 0..8 {
                let (mut w, t) = failed(phase, tier);
                v.push(w.reset_ok(t).compute_units_consumed);
            }
            v.sort_unstable();
            println!("CU reset_account tier {tier} phase {phase}: min {} median {} max {}", v[0], v[4], v[7]);
            worst = worst.max(v[7]);
        }
    }
    println!("CU reset_account largest: {worst}");
    assert!(worst < 200_000, "default budget");
}

#[allow(dead_code)]
fn _unused(_: &Keypair) {}
