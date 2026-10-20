//! M12 item 7: the VAULT BOUNDARY and the money bound R-1, through the REAL vault fixture (tests-rs/fixtures/core_vault.so,
//! sha256 42c62670..., never rebuilt).
//!
//! What lives where (read this first):
//! * The VAULT enforces: the `sector_authority` key is the PDA of the registered product (error `Unauthorized`) and that
//!   account is a `Signer`; record seeds (trader, product, challenge); record status; `payout_count < max_payout_count`;
//!   `proposed_request_id == payout_count + 1`; the tier price on `deposit_fee`/`deposit_reset`. The vault does NOT check the
//!   payout AMOUNT against the account size (only `amount > 0` and a global claims ceiling): the 0.08 S formula is enforced by
//!   OUR program only (`pass_amount`), and it is reachable only through `finalize_pass`.
//! * ENVIRONMENT CAVEAT. The harness runs LiteSVM with signature verification OFF (the vault's public test admin keys cannot
//!   sign). So here an account can be FLAGGED as a signer without a signature. The vault's own checks (`Signer` flag + key
//!   equals the derived PDA) are tested for real by the tests below. What these tests cannot show is the Solana RUNTIME rule
//!   that a program can only sign (`invoke_signed`) for PDAs derived from ITS OWN id; see `a3_*` for what is shown instead.
//! * Classification words used in the comments: BLOCKED, LIMITED, OFF-CHAIN, POLICY (see handover/audit/07-vault-boundary-r1.md).
//!
//! Measured numbers of R-1 are printed by `cargo test --offline --test vault_boundary -- --nocapture r1_`.
mod common;

use anchor_lang::solana_program::{
    instruction::{error::InstructionError, AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use anchor_spl::token::spl_token;
use common::journal::*;
use common::pass::*;
use common::vault::*;
use common::*;
use litesvm::types::TransactionResult;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E, events::*};
use setl8_shared_interfaces as si;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

const DELAY: i64 = 172_800;
/// Launch values (handover/deploy-checklist.md, decision 37): cap 100,000 USDC per 24 h window, 2-day delay.
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;
const TARGET: i64 = 108_000_000_000;

// vault error codes: 6000 + index in `VaultError` (../setl8-vault/programs/core-vault/src/errors.rs)
const V_UNAUTHORIZED: u32 = 6000;
const V_PRODUCT_NOT_ACTIVE: u32 = 6001;
const V_INVALID_TRADER_STATUS: u32 = 6005;
const V_INVALID_TIER: u32 = 6006;
const V_WRONG_AMOUNT: u32 = 6007;
const V_PAYOUT_CAP_REACHED: u32 = 6008;
const V_REQUEST_ID_MISMATCH: u32 = 6009;
const V_RESET_NOT_ALLOWED: u32 = 6011;
const V_INVALID_RESET_PHASE: u32 = 6012;
const V_ZERO_AMOUNT: u32 = 6015;
const V_TRADER_WALLET_MISMATCH: u32 = 6022;
// Anchor framework codes
const ANCHOR_CONSTRAINT_SEEDS: u32 = 2006;
const ANCHOR_ACCOUNT_NOT_SIGNER: u32 = 3010;
const ANCHOR_ACCOUNT_NOT_INITIALIZED: u32 = 3012;

/// $100k account, launch rule set, markets, limits = launch values (2-day delay, $100k cap, 24 h window).
fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(DELAY as u32, CAP, WINDOW);
    (w, t)
}

/// Exact custom error `code` on instruction 0 and the name `needle` in the logs (the vault's or Anchor's error name).
fn assert_code(r: &TransactionResult, code: u32, needle: &str) {
    match r {
        Ok(m) => panic!("expected error {code} ({needle}) but the transaction succeeded\n{}", m.logs.join("\n")),
        Err(f) => {
            match &f.err {
                TransactionError::InstructionError(0, InstructionError::Custom(got)) => {
                    assert_eq!(*got, code, "wrong error code, want {code} ({needle}); logs:\n{}", f.meta.logs.join("\n"));
                }
                other => panic!("want Custom({code}) at instruction 0, got {other:?}\n{}", f.meta.logs.join("\n")),
            }
            assert!(f.meta.logs.iter().any(|l| l.contains(needle)), "log lacks {needle}:\n{}", f.meta.logs.join("\n"));
        }
    }
}

/// A digest of everything the vault instructions here can change, for the audited trader and `others`.
fn world(w: &JEnv, others: &[Pubkey]) -> Vec<[u8; 32]> {
    let mut v = vec![w.pass_digest()];
    for o in others {
        v.push(w.v.digest(o, &[]));
    }
    v
}

fn bytes_of_account(w: &JEnv, k: &Pubkey) -> Option<Vec<u8>> {
    w.v.e.svm.get_account(k).map(|a| a.data)
}

// ====================================================================================================================
// Direct vault instruction builders. The five sector instructions with an explicit `authority` (and whether it is flagged
// as a signer), so a test can put anything in the authority slot and in every seed argument.
// ====================================================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vix {
    DepositFee,
    DepositReset,
    RecordActivity,
    FlagFailed,
    RequestPayout,
}

/// Everything a vault sector call is made of. `Default`-like constructor `Call::ok` gives a CORRECT call on challenge 0 of
/// `trader`; tests then override single fields.
#[derive(Clone)]
struct Call {
    authority: Pubkey,
    authority_signs: bool,
    registry: Pubkey,
    /// `product_program_id` argument
    product: Pubkey,
    /// `trader_wallet` argument
    trader_arg: Pubkey,
    /// `challenge_id` argument (`prev_challenge_id` for a reset)
    challenge_arg: u64,
    /// the `trader_state` account passed (the PDA of the record the call is about)
    state: Pubkey,
    /// who signs as `trader` for fee/reset, and whose token account pays
    payer_trader: Pubkey,
    amount: u64,
    account_size: u64,
    new_challenge: u64,
    new_state: Pubkey,
    reset_phase: u8,
    proposed_request_id: u64,
    /// override of the payout claim address (`request_payout`)
    claim: Option<Pubkey>,
    /// override of the trader's token account (`deposit_fee`, `deposit_reset`)
    token: Option<Pubkey>,
}

impl Call {
    /// A correct call of `kind` about `trader`'s record `challenge`.
    fn ok(kind: Vix, trader: &Pubkey, challenge: u64) -> Call {
        let state = vault_trader_state_pda(trader, challenge);
        let (amount, new_challenge) = match kind {
            Vix::DepositFee => (859_000_000, challenge),
            Vix::DepositReset => (859_000_000, challenge + 1), // reset price index 0 = 100% of the tier price
            Vix::RequestPayout => (PAYOUT_100K, challenge),
            _ => (0, challenge),
        };
        Call {
            authority: sector_authority_pda(),
            authority_signs: true,
            registry: registry_pda(),
            product: PROGRAM_ID,
            trader_arg: *trader,
            challenge_arg: challenge,
            state,
            payer_trader: *trader,
            amount,
            account_size: 100_000_000_000,
            new_challenge,
            new_state: vault_trader_state_pda(trader, new_challenge),
            reset_phase: 0,
            proposed_request_id: 1,
            claim: None,
            token: None,
        }
    }

    fn ix(&self, kind: Vix, w: &JEnv) -> Instruction {
        let vts = &self.state;
        let tt = |t: &Pubkey| w.v.traders.get(t).copied().unwrap_or_else(|| w.v.trader_token(&w.trader.pubkey()));
        let (a, r) = (self.authority, self.registry);
        let mut ix = match kind {
            Vix::FlagFailed => si::flag_trader_failed(
                VAULT_ID,
                a,
                r,
                &[AccountMeta::new(*vts, false)],
                si::FlagTraderFailedArgs { trader_wallet: self.trader_arg, product_program_id: self.product, challenge_id: self.challenge_arg },
            ),
            Vix::RecordActivity => si::record_activity(
                VAULT_ID,
                a,
                r,
                &[AccountMeta::new(*vts, false)],
                si::RecordActivityArgs { trader_wallet: self.trader_arg, product_program_id: self.product, challenge_id: self.challenge_arg },
            ),
            Vix::RequestPayout => si::request_payout(
                VAULT_ID,
                a,
                r,
                &[
                    AccountMeta::new(*vts, false),
                    AccountMeta::new(w.v.vault_state, false),
                    AccountMeta::new(self.claim.unwrap_or_else(|| claim_pda(vts, self.proposed_request_id)), false),
                    AccountMeta::new(w.v.e.payer.pubkey(), true),
                    AccountMeta::new_readonly(system_program::ID, false),
                ],
                si::RequestPayoutArgs {
                    trader_wallet: self.trader_arg,
                    amount: self.amount,
                    product_program_id: self.product,
                    challenge_id: self.challenge_arg,
                    proposed_request_id: self.proposed_request_id,
                },
            ),
            Vix::DepositFee => si::deposit_fee(
                VAULT_ID,
                a,
                r,
                &[
                    AccountMeta::new(*vts, false),
                    AccountMeta::new(self.payer_trader, true),
                    AccountMeta::new_readonly(system_program::ID, false),
                    AccountMeta::new_readonly(w.v.vault_state, false),
                    AccountMeta::new_readonly(self.payer_trader, true),
                    AccountMeta::new(self.token.unwrap_or_else(|| tt(&self.payer_trader)), false),
                    AccountMeta::new_readonly(w.v.usdc, false),
                    AccountMeta::new(w.v.usdc_pool, false),
                    AccountMeta::new(w.v.vault_sl8_usdc, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
                si::DepositFeeArgs {
                    amount: self.amount,
                    product_program_id: self.product,
                    challenge_id: self.challenge_arg,
                    trader_wallet: self.trader_arg,
                    account_size: self.account_size,
                },
            ),
            Vix::DepositReset => si::deposit_reset(
                VAULT_ID,
                a,
                r,
                &[
                    AccountMeta::new(*vts, false),
                    AccountMeta::new(self.new_state, false),
                    AccountMeta::new(self.payer_trader, true),
                    AccountMeta::new_readonly(system_program::ID, false),
                    AccountMeta::new_readonly(w.v.vault_state, false),
                    AccountMeta::new_readonly(self.payer_trader, true),
                    AccountMeta::new(self.token.unwrap_or_else(|| tt(&self.payer_trader)), false),
                    AccountMeta::new_readonly(w.v.usdc, false),
                    AccountMeta::new(w.v.usdc_pool, false),
                    AccountMeta::new(w.v.vault_sl8_usdc, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
                si::DepositResetArgs {
                    amount: self.amount,
                    trader_wallet: self.trader_arg,
                    product_program_id: self.product,
                    prev_challenge_id: self.challenge_arg,
                    new_challenge_id: self.new_challenge,
                    reset_phase: self.reset_phase,
                },
            ),
        };
        ix.accounts[0].is_signer = self.authority_signs;
        ix
    }
}

/// Sends the call; the trader (if it is a known keypair) signs for real, the authority only by flag (sigverify is off).
fn send_call(w: &mut JEnv, kind: Vix, call: &Call, trader_kp: Option<&Keypair>) -> TransactionResult {
    let ix = call.ix(kind, w);
    match trader_kp {
        Some(k) => {
            let k = dup(k);
            w.v.e.send(&[ix], &[&k])
        }
        None => w.v.send_plain(ix),
    }
}

fn kp_of(w: &JEnv, kind: Vix) -> Option<Keypair> {
    matches!(kind, Vix::DepositFee | Vix::DepositReset).then(|| dup(&w.trader))
}

/// A world where every one of the five calls is VALID for the audited trader: record 0 exists and is Active (record 1 is the
/// target of a fee or a reset), and for the reset test the record 0 is Failed. Returns the world after `report_fail`.
fn world_for(kind: Vix) -> (JEnv, i64, Call) {
    let (mut w, t) = base();
    let tk = w.trader.pubkey();
    let call = match kind {
        Vix::DepositFee => {
            // a fresh trader buys challenge 0: the call is about a record that does not exist yet
            let fresh = w.v.new_trader();
            let mut c0 = Call::ok(Vix::DepositFee, &fresh.pubkey(), 0);
            c0.payer_trader = fresh.pubkey();
            // keep the fresh trader reachable for `kp_of`
            w.trader = fresh;
            c0
        }
        Vix::DepositReset => {
            w.fail_in(2, t);
            Call::ok(Vix::DepositReset, &tk, 0)
        }
        other => Call::ok(other, &tk, 0),
    };
    (w, t, call)
}

// ====================================================================================================================
// (a) A-3: impersonating this program to the vault
// ====================================================================================================================

/// DESIGN A-3: "Impersonate this program to the vault. Blocked by the vault: `sector_authority` must be a PDA signed by this
/// program id."
///
/// BLOCKED, in two halves.
///  1. This file (all five vault sector instructions): the authority slot holds anything but the PDA of our program id, or
///     the PDA flagged as a non-signer: the vault refuses with its exact error and nothing changes. A correct call at the
///     end of each test (control) succeeds, so the refusals are for the authority and not for a broken call.
///  2. NOT shown here (OFF-CHAIN = Solana runtime): a CPI from ANOTHER program cannot sign for OUR PDA, because
///     `invoke_signed` derives the signer address from the CALLER's program id; the harness has sigverify off and no second
///     program, so only half 1 can be executed. See `a3_real_cpi_from_another_program` for the fixture status.
fn authority_variants(w: &JEnv, trader: &Pubkey) -> Vec<(&'static str, Pubkey, bool, u32, &'static str)> {
    let other_program = Pubkey::new_unique();
    vec![
        ("our PDA, not flagged as a signer", sector_authority_pda(), false, ANCHOR_ACCOUNT_NOT_SIGNER, "AccountNotSigner"),
        ("random key flagged as signer", Pubkey::new_unique(), true, V_UNAUTHORIZED, "Unauthorized"),
        ("sector PDA derived under another program id", si::derive_sector_authority(&other_program).0, true, V_UNAUTHORIZED, "Unauthorized"),
        ("sector PDA derived under the VAULT id", si::derive_sector_authority(&VAULT_ID).0, true, V_UNAUTHORIZED, "Unauthorized"),
        (
            "our program id, right program, wrong seed",
            Pubkey::find_program_address(&[b"sector_authority"], &PROGRAM_ID).0,
            true,
            V_UNAUTHORIZED,
            "Unauthorized",
        ),
        ("our config PDA flagged as signer", config_pda(), true, V_UNAUTHORIZED, "Unauthorized"),
        ("the trader's own wallet as signer", *trader, true, V_UNAUTHORIZED, "Unauthorized"),
        ("the vault registry account itself", registry_pda(), true, V_UNAUTHORIZED, "Unauthorized"),
        ("the sl8 admin of the vault as signer", VAULT_SL8_ADMIN, true, V_UNAUTHORIZED, "Unauthorized"),
        ("the evaluator key as signer", w.v.evaluator.pubkey(), true, V_UNAUTHORIZED, "Unauthorized"),
    ]
}

fn a3_for(kind: Vix) {
    let (mut w, _t, call) = world_for(kind);
    let signer = kp_of(&w, kind);
    let trader = call.trader_arg;
    let before = world(&w, &[trader]);
    let before_state = bytes_of_account(&w, &call.state);
    let before_reg = bytes_of_account(&w, &registry_pda());
    for (name, key, signs, code, needle) in authority_variants(&w, &trader) {
        let mut c2 = call.clone();
        c2.authority = key;
        c2.authority_signs = signs;
        let r = send_call(&mut w, kind, &c2, signer.as_ref());
        assert_code(&r, code, needle);
        assert_eq!(world(&w, &[trader]), before, "{kind:?} with authority `{name}` changed the world");
        assert_eq!(bytes_of_account(&w, &call.state), before_state);
        assert_eq!(bytes_of_account(&w, &registry_pda()), before_reg);
    }
    // control: the same call with the real PDA flagged as signer is accepted (what our program's invoke_signed provides)
    let r = send_call(&mut w, kind, &call, signer.as_ref());
    assert!(r.is_ok(), "{kind:?} control call failed: {:?}", r.err().map(|f| (f.err, f.meta.logs)));
    assert_ne!(world(&w, &[trader]), before, "{kind:?} control call changed nothing");
}

#[test]
fn a3_deposit_fee_authority_slot() {
    a3_for(Vix::DepositFee);
}
#[test]
fn a3_deposit_reset_authority_slot() {
    a3_for(Vix::DepositReset);
}
#[test]
fn a3_record_activity_authority_slot() {
    // record_activity is throttled inside 86,000 s of the last activity but still returns Ok; so the control is Ok and the
    // refusals above must still hold for every variant (they run before the throttle)
    let (mut w, t, call) = world_for(Vix::RecordActivity);
    w.set_time(t + 90_000);
    let trader = call.trader_arg;
    let before = world(&w, &[trader]);
    for (name, key, signs, code, needle) in authority_variants(&w, &trader) {
        let mut c2 = call.clone();
        c2.authority = key;
        c2.authority_signs = signs;
        let r = send_call(&mut w, Vix::RecordActivity, &c2, None);
        assert_code(&r, code, needle);
        assert_eq!(world(&w, &[trader]), before, "record_activity with authority `{name}` changed the world");
    }
    let last = |w: &JEnv| w.v.vault_trader_state(&trader, 0).unwrap().last_activity_timestamp;
    let l0 = last(&w);
    assert!(send_call(&mut w, Vix::RecordActivity, &call, None).is_ok());
    assert!(last(&w) > l0, "the control call must refresh the activity clock");
}
#[test]
fn a3_flag_trader_failed_authority_slot() {
    a3_for(Vix::FlagFailed);
}
#[test]
fn a3_request_payout_authority_slot() {
    a3_for(Vix::RequestPayout);
}


// ====================================================================================================================
// Driving a pass chain with a COMPROMISED evaluator
// ====================================================================================================================

/// Hand-written payout table: 0.08 x S per tier of the launch rule set, in micro-USD. (S in USD: 2,500 / 5,000 / 10,000 /
/// 15,000 / 20,000 / 25,000 / 50,000 / 75,000 / 100,000 / 200,000.)
const TIER_SIZE_USD: [u64; 10] = [2_500, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
const TIER_PAYOUT_USD: [u64; 10] = [200, 400, 800, 1_200, 1_600, 2_000, 4_000, 6_000, 8_000, 16_000];

fn usd_u(d: u64) -> u64 {
    d * 1_000_000
}

/// The FAKE history a compromised evaluator needs: one consistent journal batch that books +0.08 S of closed profit in a
/// single close (no fill ever happened; the program does not track positions, only the books) and reports flat. The chain is
/// consistent (right sequence, right previous hash, proof within `max_age_secs`), so `post_journal` accepts it: this is the
/// "as far as the program allows" part of R-1. Asserts that `b` is now exactly 1.08 S.
fn fake_to_target(w: &mut JEnv, t: i64) {
    let s = i64::try_from(w.account().size_s).unwrap();
    let pnl = s / 100 * 8;
    let b = simple(w, t, vec![close_long_pnl(t, t - 100, pnl)], true);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, s + pnl, "the fabricated batch must bring b to 1.08 S exactly");
}

/// What one chain of the compromised evaluator got out of the vault.
#[derive(Debug, Default, Clone)]
struct Chain {
    /// `(phase, claim owed, finalize time)` for every payout the vault queued, in order.
    paid: Vec<(u8, u64, i64)>,
    /// clock after the last step
    end: i64,
}

impl Chain {
    fn total(&self) -> u64 {
        self.paid.iter().map(|p| p.1).sum()
    }
}

/// Drives the CURRENT account (`w.acct`) from phase 1 to graduation with fabricated passes and NO veto: fabricate, request
/// (trader), confirm (evaluator), wait exactly the delay, finalize (anyone). Stops when the program or the vault refuses.
fn run_chain_no_veto(w: &mut JEnv, mut t: i64, payer: &Keypair) -> Chain {
    let mut out = Chain::default();
    // phase 1: no money, the confirmation starts phase 2 at once
    fake_to_target(w, t);
    w.ok(|w| w.request(t + 10));
    w.ok(|w| w.confirm(t + 20));
    assert_eq!(w.account().phase, 2);
    t += 30;
    for phase in 2..=5u8 {
        fake_to_target(w, t);
        w.ok(|w| w.request(t + 10));
        w.ok(|w| w.confirm(t + 20));
        let fin = t + 20 + DELAY;
        w.fails(E::PassDelayNotElapsed, |w| w.finalize(fin - 1, payer));
        w.ok(|w| w.finalize(fin, payer));
        let k = w.account().next_request_id - 1;
        let owed = w.claim(k).expect("the vault queued a claim").owed;
        out.paid.push((phase, owed, fin));
        t = fin + 30;
    }
    out.end = t;
    out
}

// ====================================================================================================================
// (b) the payout amount is the fixed formula 0.08 S, phases 2 to 5 only, once per phase, for every tier
// ====================================================================================================================

/// DESIGN 2.5 / 10.4 "Fixed formula: `amount = 0.08 S`, once per phase 2 to 5 ... No instruction takes an amount from any
/// caller." BLOCKED by construction (`pass_amount`), shown end to end for EVERY tier through the real vault: request amount,
/// claim owed, event amount, tally, registry total and vault payout_count all equal the hand-written table, phase 1 pays
/// nothing, and the chain total is exactly 0.32 S.
#[test]
fn b_payout_is_exactly_0_08_s_in_phases_2_to_5_for_every_tier() {
    for tier in 0..10u8 {
        let mut w = JEnv::with_tier(tier);
        let t0 = w.t0 + 1_000;
        w.set_limits(DELAY as u32, CAP, WINDOW);
        let (s_usd, exp) = (TIER_SIZE_USD[usize::from(tier)], usd_u(TIER_PAYOUT_USD[usize::from(tier)]));
        assert_eq!(w.account().size_s, usd_u(s_usd), "tier {tier} size");
        assert_eq!(exp * 100, usd_u(s_usd) * 8, "the table is 0.08 S");
        let payer = w.new_payer();
        let mut t = t0;
        // phase 1: evaluation, no money anywhere
        fake_to_target(&mut w, t);
        let m = w.ok(|w| w.request(t + 10));
        assert_eq!(cpi_events::<PassRequested>(&m)[0].amount, 0, "tier {tier}: phase 1 event amount");
        w.ok(|w| w.confirm(t + 20));
        assert_eq!((w.claim(1), w.tally().requested_count, w.tally().requested_total, w.registry_totals(), w.vault_status().payout_count), (None, 0, 0, (0, 0), 0), "tier {tier}: phase 1 moved money");
        t += 30;
        for phase in 2..=5u8 {
            let k = u64::from(phase) - 1;
            fake_to_target(&mut w, t);
            let m = w.ok(|w| w.request(t + 10));
            assert_eq!(cpi_events::<PassRequested>(&m)[0].amount, exp, "tier {tier} phase {phase}: PassRequested.amount");
            w.ok(|w| w.confirm(t + 20));
            let fin = t + 20 + DELAY;
            let m = w.ok(|w| w.finalize(fin, &payer));
            assert_eq!(cpi_events::<PassFinalized>(&m)[0].amount, exp, "tier {tier} phase {phase}: PassFinalized.amount");
            assert_eq!(w.claim(k).unwrap().owed, exp, "tier {tier} phase {phase}: vault claim");
            assert_eq!(w.claim(k + 1), None);
            assert_eq!((w.tally().requested_count, w.tally().requested_total), (k, k * exp));
            assert_eq!(w.registry_totals(), (k, k * exp));
            assert_eq!(w.vault_status().payout_count, k);
            // the payout of a phase already paid: the phase moved on, and the same call cannot pay again
            if phase < 5 {
                assert_eq!(w.account().phase, phase + 1);
                w.fails(E::NoPendingPass, |w| w.finalize(fin + 5, &payer));
                w.fails(E::TargetNotReached, |w| w.request(fin + 10));
            }
            t = fin + 30;
        }
        // chain total = 0.32 S exactly, graduated on both sides
        assert_eq!(w.tally().requested_total * 100, usd_u(s_usd) * 32, "tier {tier}: chain total is 32% of S");
        assert_eq!((w.account().status, w.vault_status().status, w.vault_status().payout_count), (c::STATUS_GRADUATED, 1, 4));
    }
}

/// The vault side of "a payout for a phase already paid is refused": a direct `request_payout` that repeats an already used
/// request id (or skips one) is refused with `RequestIdMismatch` and nothing changes; amount 0 with `ZeroAmount`. BLOCKED by
/// the vault. The authority is the real PDA flagged as signer (the harness stand-in for our program's `invoke_signed`).
#[test]
fn b_vault_refuses_a_repeated_or_skipped_request_id_and_a_zero_amount() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    w.ok(|w| w.finalize(t + DELAY, &payer)); // payout 1 paid by our program
    let tk = w.trader.pubkey();
    let before = world(&w, &[]);
    // repeat id 1 (already used): the record is at payout_count 1, expects 2
    for id in [0u64, 1, 3, 4, u64::MAX] {
        let mut c1 = Call::ok(Vix::RequestPayout, &tk, 0);
        c1.proposed_request_id = id;
        // the claim PDA of `id` may not exist yet; the vault checks the seeds of the passed address, which `Call` derives from `id`
        let r = send_call(&mut w, Vix::RequestPayout, &c1, None);
        assert_code(&r, V_REQUEST_ID_MISMATCH, "RequestIdMismatch");
        assert_eq!(world(&w, &[]), before, "id {id} changed the world");
    }
    let mut c1 = Call::ok(Vix::RequestPayout, &tk, 0);
    c1.proposed_request_id = 2;
    c1.amount = 0;
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c1, None), V_ZERO_AMOUNT, "ZeroAmount");
    assert_eq!(world(&w, &[]), before);
}

/// LIMITED (a fact about the vault, not a bug of ours): the vault does not compare a payout `amount` with the account size.
/// It checks `amount > 0` and the global claims ceiling (2,500,000 USDC). So the 0.08 S rule rests entirely on A-3: nobody but
/// our program can sign for the sector PDA. This test pins exactly what the vault still accepts and where it stops.
#[test]
fn b_vault_accepts_any_amount_up_to_the_global_ceiling_so_the_formula_rests_on_the_authority() {
    let (mut w, _t) = base();
    let tk = w.trader.pubkey();
    // ceiling 2,500,000 USDC = 2_500_000_000_000 micro; one above is refused with ClaimsCeilingExceeded (6043)
    let mut c1 = Call::ok(Vix::RequestPayout, &tk, 0);
    c1.amount = 2_500_000_000_001;
    let before = world(&w, &[]);
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c1, None), 6043, "ClaimsCeilingExceeded");
    assert_eq!(world(&w, &[]), before);
    c1.amount = u64::MAX;
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c1, None), 6043, "ClaimsCeilingExceeded");
    // exactly the ceiling is accepted for a $100k account (25x the account size): nothing in the vault ties amount to S
    c1.amount = 2_500_000_000_000;
    assert!(send_call(&mut w, Vix::RequestPayout, &c1, None).is_ok());
    assert_eq!(w.claim(1).unwrap().owed, 2_500_000_000_000);
    assert_eq!(w.registry_totals(), (1, 2_500_000_000_000));
}

// ====================================================================================================================
// (d) a FAILED record cannot be paid
// ====================================================================================================================

/// DESIGN 4.7 / 10.1: a failed account is terminal. Three layers, each pinned:
///  1. local: after `report_fail` a confirmed pass cannot exist (state machine) and `finalize_pass` says `AccountNotActive`;
///  2. vault: even if the local account is forced back to Active + confirmed (a desync that no instruction can create), the
///     vault record is Failed and `request_payout` is refused with `InvalidTraderStatus`, rolled back, no claim;
///  3. direct vault call on a Failed record (A-3 half 1 stand-in): same `InvalidTraderStatus`.
#[test]
fn d_a_failed_record_cannot_be_paid_locally_or_at_the_vault() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.fail_in(3, t);
    let tk = w.trader.pubkey();
    assert_eq!(w.vault_status().status, 2, "vault record Failed");
    // 1. local refusals
    w.fails(E::AccountNotActive, |w| w.finalize(t + 10 * DELAY, &payer));
    w.fails(E::AccountNotActive, |w| w.request(t + 100));
    // 2. force the local account back to a confirmed pass; the vault still refuses
    w.edit_account(|a| {
        a.status = c::STATUS_ACTIVE;
        a.phase = 3;
        a.b = TARGET;
        a.flat = 1;
        a.pass_pending = c::PASS_CONFIRMED;
        a.pass_proposed_at = t;
        a.next_request_id = 1;
    });
    let before = w.pass_digest();
    let r = w.finalize(t + DELAY, &payer);
    assert_code(&r, V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
    assert_eq!(w.pass_digest(), before);
    assert_eq!((w.claim(1), w.tally().requested_count, w.registry_totals()), (None, 0, (0, 0)));
    // 3. direct vault call
    let c1 = Call::ok(Vix::RequestPayout, &tk, 0);
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c1, None), V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
    assert_eq!(w.pass_digest(), before);
}

/// A failed record cannot be flagged twice, cannot record activity, and its reset is allowed exactly once; the old record
/// never becomes payable again after the reset (the new record is a different PDA with the same `payout_count`).
#[test]
fn d_failed_record_terminal_and_reset_once() {
    let (mut w, t) = base();
    w.fail_in(2, t);
    let tk = w.trader.pubkey();
    let before = world(&w, &[]);
    for kind in [Vix::FlagFailed, Vix::RecordActivity, Vix::RequestPayout] {
        let c1 = Call::ok(kind, &tk, 0);
        assert_code(&send_call(&mut w, kind, &c1, None), V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
        assert_eq!(world(&w, &[]), before, "{kind:?} on a Failed record changed the world");
    }
    // reset 1: accepted through our program; reset 2 of the same record: the vault says ResetNotAllowed
    let prev = w.acct;
    w.reset_ok(t + 100);
    let second = Call::ok(Vix::DepositReset, &tk, 0);
    let mut second = second;
    second.new_challenge = 5;
    second.new_state = vault_trader_state_pda(&tk, 5);
    let kp = dup(&w.trader);
    let before = world(&w, &[]);
    assert_code(&send_call(&mut w, Vix::DepositReset, &second, Some(&kp)), V_RESET_NOT_ALLOWED, "ResetNotAllowed");
    assert_eq!(world(&w, &[]), before);
    assert_ne!(prev, w.acct);
    // the old record stays Failed and unpayable
    let old = Call::ok(Vix::RequestPayout, &tk, 0);
    assert_code(&send_call(&mut w, Vix::RequestPayout, &old, None), V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
}

// ====================================================================================================================
// (e) max_payout_count
// ====================================================================================================================

/// The vault's `max_payout_count` is 4 (registered). Boundary through the real flow AND at the vault:
///  * `payout_count` 3 -> the 4th payout is accepted and the record becomes Graduated;
///  * `payout_count` 4 on an ACTIVE record (forced) -> a fifth is refused with `PayoutCapReached` (6008);
///  * a Graduated record -> `InvalidTraderStatus` (6005), also through our `finalize_pass`.
#[test]
fn e_max_payout_count_is_four_and_a_fifth_is_refused() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    assert_eq!(w.v.registry().max_payout_count, 4);
    let tk = w.trader.pubkey();
    // forced record at payout_count 4 but still Active: the cap check itself
    w.v.edit_vault_record(&tk, 0, None, Some(4));
    let mut c5 = Call::ok(Vix::RequestPayout, &tk, 0);
    c5.proposed_request_id = 5;
    let before = world(&w, &[]);
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c5, None), V_PAYOUT_CAP_REACHED, "PayoutCapReached");
    assert_eq!(world(&w, &[]), before);
    // through our program: account at phase 5, confirmed, next_request_id 5
    w.edit_account(|a| {
        a.phase = 5;
        a.b = TARGET;
        a.flat = 1;
        a.pass_pending = c::PASS_CONFIRMED;
        a.pass_proposed_at = t;
        a.next_request_id = 5;
    });
    let before = w.pass_digest();
    assert_code(&w.finalize(t + DELAY, &payer), V_PAYOUT_CAP_REACHED, "PayoutCapReached");
    assert_eq!(w.pass_digest(), before);
    // payout_count 3: the fourth is accepted and graduates the record at the vault
    w.v.edit_vault_record(&tk, 0, None, Some(3));
    w.edit_account(|a| a.next_request_id = 4);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    assert_eq!((w.vault_status().payout_count, w.vault_status().status, w.account().status), (4, 1, c::STATUS_GRADUATED));
    assert_eq!(w.claim(4).unwrap().owed, PAYOUT_100K);
    // the fifth on the graduated record
    let mut c5 = Call::ok(Vix::RequestPayout, &tk, 0);
    c5.proposed_request_id = 5;
    let before = w.pass_digest();
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c5, None), V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
    assert_eq!(w.pass_digest(), before);
}

/// A reset does NOT restart the payout allowance: the vault copies `payout_count` (and `account_size`) into the new record.
/// Two payouts, a fail, a reset, then exactly two more payouts graduate the chain: 4 x 0.08 S over TWO records.
#[test]
fn e_a_reset_carries_payout_count_so_the_chain_still_pays_four_times() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let t = w.advance_to(4, t, &payer, DELAY);
    let tk = w.trader.pubkey();
    assert_eq!(w.vault_status().payout_count, 2);
    w.fail_in(4, t);
    w.reset_ok(t + 500);
    let rec = w.v.vault_trader_state(&tk, w.account().challenge_id).unwrap();
    assert_eq!((rec.payout_count, rec.status, rec.account_size), (2, 0, 100_000_000_000), "reset copies the count");
    assert_eq!(w.account().next_request_id, 3, "local request id continues the vault count");
    let mut t2 = t + 1_000;
    for phase in 4..=5u8 {
        fake_to_target(&mut w, t2);
        w.ok(|w| w.request(t2 + 10));
        w.ok(|w| w.confirm(t2 + 20));
        w.ok(|w| w.finalize(t2 + 20 + DELAY, &payer));
        t2 += 20 + DELAY + 30;
        let _ = phase;
    }
    assert_eq!(w.account().status, c::STATUS_GRADUATED);
    assert_eq!(w.tally().requested_count, 4);
    assert_eq!(w.tally().requested_total, 4 * PAYOUT_100K);
    // no fifth, no matter the record
    for id in [0u64, 1] {
        let mut c5 = Call::ok(Vix::RequestPayout, &tk, id);
        c5.proposed_request_id = 5;
        let r = send_call(&mut w, Vix::RequestPayout, &c5, None);
        assert_code(&r, V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
    }
}


// ====================================================================================================================
// (c) mismatched trader / sector / record
// ====================================================================================================================

/// Two traders (A = `w.trader`, B) with one $100k account each, plus a SECOND registered product X (a random program id)
/// in the same vault, so "another sector" is a real registry in the real vault.
struct Two {
    w: JEnv,
    t: i64,
    a: Pubkey,
    b: Keypair,
    x: Pubkey,
    x_registry: Pubkey,
}

/// Registers `product` as a second product of the real vault (both vault admins flagged: sigverify is off) with the $100k
/// tier at the launch price; returns its registry address.
fn register_other_product(w: &mut JEnv, product: &Pubkey) -> Pubkey {
    let registry = Pubkey::find_program_address(&[b"product_registry", product.as_ref()], &VAULT_ID).0;
    let reg = si::register_product(
        VAULT_ID,
        VAULT_SL8_ADMIN,
        VAULT_ROV_ADMIN,
        registry,
        &[AccountMeta::new_readonly(system_program::ID, false)],
        si::RegisterProductArgs {
            product_program_id: *product,
            fee_split_bps: FEE_SPLIT_BPS,
            challenge_sizes: vec![si::ChallengeSize { size: 100_000_000_000, cost: 859_000_000 }],
            max_payout_count: 4,
            reset_price_bps: RESET_LADDER_BPS.to_vec(),
        },
    );
    assert_ok(w.v.send_plain(reg));
    registry
}

fn two() -> Two {
    let (mut w, t) = base();
    let b = w.v.new_trader();
    assert_ok(w.v.open(&b, 8));
    let x = Pubkey::new_unique();
    let x_registry = register_other_product(&mut w, &x);
    let a = w.trader.pubkey();
    Two { w, t, a, b, x, x_registry }
}

fn vts_of(trader: &Pubkey, challenge: u64) -> Pubkey {
    vault_trader_state_pda(trader, challenge)
}

/// Record-bound calls (flag, activity, payout) with a mismatch in the trader, the challenge or the product: every one is
/// refused by the vault's SEED constraints (Anchor 2006) or by the account loader (3012 when the passed address holds no
/// record), and nothing changes. BLOCKED (vault).
#[test]
fn c_record_bound_calls_refuse_a_mismatched_trader_challenge_or_product() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let others = [b];
    for kind in [Vix::FlagFailed, Vix::RecordActivity, Vix::RequestPayout] {
        let before = world(&z.w, &others);
        let regs = (bytes_of_account(&z.w, &registry_pda()), bytes_of_account(&z.w, &z.x_registry));
        let mut cases: Vec<(&str, Call, u32, &str)> = Vec::new();
        // trader argument B, record of A
        let mut c1 = Call::ok(kind, &a, 0);
        c1.trader_arg = b;
        cases.push(("trader arg B, record A", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // trader argument A, record of B
        let mut c1 = Call::ok(kind, &a, 0);
        c1.state = vts_of(&b, 0);
        cases.push(("trader arg A, record B", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // challenge argument 1, record 0
        let mut c1 = Call::ok(kind, &a, 0);
        c1.challenge_arg = 1;
        cases.push(("challenge arg 1, record 0", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // record 1 does not exist
        let mut c1 = Call::ok(kind, &a, 1);
        c1.challenge_arg = 1;
        cases.push(("record 1 does not exist", c1, ANCHOR_ACCOUNT_NOT_INITIALIZED, "AccountNotInitialized"));
        // product argument X with OUR registry: the registry seeds use the argument
        let mut c1 = Call::ok(kind, &a, 0);
        c1.product = z.x;
        cases.push(("product arg X, our registry", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // product argument ours with X's registry
        let mut c1 = Call::ok(kind, &a, 0);
        c1.registry = z.x_registry;
        cases.push(("product arg ours, registry X", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // X's registry and X argument, our PDA flagged, but OUR record: the record seeds use the argument X
        let mut c1 = Call::ok(kind, &a, 0);
        c1.registry = z.x_registry;
        c1.product = z.x;
        cases.push(("product X + registry X, record of ours", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        // X's registry and X argument and X's record address (no record there)
        let mut c1 = Call::ok(kind, &a, 0);
        c1.registry = z.x_registry;
        c1.product = z.x;
        c1.state = Pubkey::find_program_address(&[b"trader_state", z.x.as_ref(), a.as_ref(), &0u64.to_le_bytes()], &VAULT_ID).0;
        cases.push(("product X, record address under X (empty)", c1, ANCHOR_ACCOUNT_NOT_INITIALIZED, "AccountNotInitialized"));
        if kind == Vix::RequestPayout {
            let mut c1 = Call::ok(kind, &a, 0);
            c1.claim = Some(claim_pda(&vts_of(&b, 0), 1));
            cases.push(("claim address of B's record", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
            let mut c1 = Call::ok(kind, &a, 0);
            c1.claim = Some(claim_pda(&vts_of(&a, 0), 2));
            cases.push(("claim address of the wrong request id", c1, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds"));
        }
        for (name, call, code, needle) in cases {
            let r = send_call(&mut z.w, kind, &call, None);
            assert_code(&r, code, needle);
            assert_eq!(world(&z.w, &others), before, "{kind:?} `{name}` changed the world");
            assert_eq!((bytes_of_account(&z.w, &registry_pda()), bytes_of_account(&z.w, &z.x_registry)), regs, "{kind:?} `{name}` changed a registry");
        }
    }
}

/// The other sector's own authority is useless against our records and ours against theirs: X's PDA with X's registry cannot
/// touch OUR record (seeds), and OUR PDA with X's registry cannot touch X's record (`Unauthorized`: the registry names X, the
/// authority must be X's PDA). The X record is created through X's own authority (flagged as signer: sigverify is off).
#[test]
fn c_another_sectors_authority_cannot_touch_our_record_and_ours_cannot_touch_theirs() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let x_auth = si::derive_sector_authority(&z.x).0;
    // X records a purchase of A under X (deposit_fee as X)
    let mut fee = Call::ok(Vix::DepositFee, &a, 0);
    fee.authority = x_auth;
    fee.registry = z.x_registry;
    fee.product = z.x;
    fee.state = Pubkey::find_program_address(&[b"trader_state", z.x.as_ref(), a.as_ref(), &0u64.to_le_bytes()], &VAULT_ID).0;
    let kp = dup(&z.w.trader);
    assert!(send_call(&mut z.w, Vix::DepositFee, &fee, Some(&kp)).is_ok(), "X's own fee call");
    let x_state = fee.state;
    let before = world(&z.w, &[b]);
    let x_bytes = bytes_of_account(&z.w, &x_state);
    for kind in [Vix::FlagFailed, Vix::RecordActivity, Vix::RequestPayout] {
        // our authority on X's record
        let mut c1 = Call::ok(kind, &a, 0);
        c1.registry = z.x_registry;
        c1.product = z.x;
        c1.state = x_state;
        assert_code(&send_call(&mut z.w, kind, &c1, None), V_UNAUTHORIZED, "Unauthorized");
        // X's authority on OUR record (X registry, X product argument: the record address is under OUR product)
        let mut c2 = Call::ok(kind, &a, 0);
        c2.authority = x_auth;
        c2.registry = z.x_registry;
        c2.product = z.x;
        assert_code(&send_call(&mut z.w, kind, &c2, None), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
        // X's authority with OUR registry and our product argument
        let mut c3 = Call::ok(kind, &a, 0);
        c3.authority = x_auth;
        assert_code(&send_call(&mut z.w, kind, &c3, None), V_UNAUTHORIZED, "Unauthorized");
        assert_eq!(world(&z.w, &[b]), before, "{kind:?} cross-sector call changed our world");
        assert_eq!(bytes_of_account(&z.w, &x_state), x_bytes, "{kind:?} cross-sector call changed X's record");
    }
}

/// `deposit_fee` with mismatches: trader signer vs `trader_wallet` argument, price, size, someone else's token account, and an
/// already used challenge id (the record exists). BLOCKED (vault): exact errors, nothing moves (token balances in the digest).
#[test]
fn c_deposit_fee_refuses_mismatched_trader_price_size_token_and_a_reused_challenge() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let kp_a = dup(&z.w.trader);
    let kp_b = dup(&z.b);
    let before = world(&z.w, &[b]);
    let new_for = |who: &Pubkey, challenge: u64| {
        let mut c1 = Call::ok(Vix::DepositFee, who, challenge);
        c1.state = vts_of(who, challenge);
        c1
    };
    // signer A, trader_wallet argument B (state PDA of B)
    let mut c1 = new_for(&b, 5);
    c1.payer_trader = a;
    let r = send_call(&mut z.w, Vix::DepositFee, &c1, Some(&kp_a));
    assert_code(&r, V_TRADER_WALLET_MISMATCH, "TraderWalletMismatch");
    // price one micro-USD short, one over, zero
    for amt in [858_999_999u64, 859_000_001, 0, u64::MAX] {
        let mut c1 = new_for(&a, 5);
        c1.amount = amt;
        assert_code(&send_call(&mut z.w, Vix::DepositFee, &c1, Some(&kp_a)), V_INVALID_TIER, "InvalidChallengeTier");
    }
    // size not registered, or the price of another size
    for size in [100_000_000_001u64, 50_000_000_000, 0] {
        let mut c1 = new_for(&a, 5);
        c1.account_size = size;
        assert_code(&send_call(&mut z.w, Vix::DepositFee, &c1, Some(&kp_a)), V_INVALID_TIER, "InvalidChallengeTier");
    }
    // B's token account pays for A
    let mut c1 = new_for(&a, 5);
    c1.token = Some(z.w.v.trader_token(&b));
    assert_code(&send_call(&mut z.w, Vix::DepositFee, &c1, Some(&kp_a)), 6021, "InvalidTokenAccount");
    // the record of challenge 0 exists: `init` refuses (system program: account already in use)
    let c1 = new_for(&a, 0);
    let r = send_call(&mut z.w, Vix::DepositFee, &c1, Some(&kp_a));
    match &r {
        Err(f) => {
            assert!(matches!(&f.err, TransactionError::InstructionError(0, InstructionError::Custom(0))), "{:?}", f.err);
            assert!(f.meta.logs.iter().any(|l| l.contains("already in use")), "{}", f.meta.logs.join("\n"));
        }
        Ok(_) => panic!("a second purchase of challenge 0 was accepted"),
    }
    // B tries to overwrite A's record by buying A's challenge id 0 for B's wallet: the PDA is B's own, nothing of A is touched
    assert_eq!(world(&z.w, &[b]), before, "a refused fee call changed the world");
    let _ = kp_b;
}

/// `deposit_reset` with mismatches. BLOCKED (vault): exact errors, nothing moves. The positive control is in `d_*`.
#[test]
fn c_deposit_reset_refuses_active_records_wrong_price_phase_trader_and_reused_ids() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let kp_a = dup(&z.w.trader);
    // A's record 0 is ACTIVE: a reset is refused
    let before = world(&z.w, &[b]);
    let c1 = Call::ok(Vix::DepositReset, &a, 0);
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), V_RESET_NOT_ALLOWED, "ResetNotAllowed");
    assert_eq!(world(&z.w, &[b]), before);
    // fail A's record (through the real report_fail) and B's too
    z.w.fail_in(2, z.t);
    let before = world(&z.w, &[b]);
    let mk = || Call::ok(Vix::DepositReset, &a, 0);
    // price: one micro short, one over, the price of another ladder step, zero
    for amt in [858_999_999u64, 859_000_001, 0, 987_850_000] {
        let mut c1 = mk();
        c1.amount = amt;
        assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), V_WRONG_AMOUNT, "WrongAmount");
    }
    // phase index 1 is 115%: the 100% price is wrong there, and index 5 is outside the ladder
    let mut c1 = mk();
    c1.reset_phase = 1;
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), V_WRONG_AMOUNT, "WrongAmount");
    let mut c1 = mk();
    c1.reset_phase = 5;
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), V_INVALID_RESET_PHASE, "InvalidResetPhase");
    let mut c1 = mk();
    c1.reset_phase = 255;
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), V_INVALID_RESET_PHASE, "InvalidResetPhase");
    // trader signer B, trader_wallet argument A
    let mut c1 = mk();
    c1.payer_trader = b;
    let kp_b = dup(&z.b);
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_b)), V_TRADER_WALLET_MISMATCH, "TraderWalletMismatch");
    // B's (Active) record as the previous record under A's name
    let mut c1 = mk();
    c1.state = vts_of(&b, 0);
    assert_code(&send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a)), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    // the new record is the previous one (record 0): `init` refuses, in use
    let mut c1 = mk();
    c1.new_challenge = 0;
    c1.new_state = vts_of(&a, 0);
    let r = send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a));
    match &r {
        Err(f) => assert!(matches!(&f.err, TransactionError::InstructionError(0, InstructionError::Custom(0))), "{:?}", f.err),
        Ok(_) => panic!("a reset onto the previous record was accepted"),
    }
    // the new record is B's record 0 address with A's seeds mismatch
    let mut c1 = mk();
    c1.new_state = vts_of(&b, 0);
    let r = send_call(&mut z.w, Vix::DepositReset, &c1, Some(&kp_a));
    assert_code(&r, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    assert_eq!(world(&z.w, &[b]), before, "a refused reset call changed the world");
}

/// The same record mismatch through OUR program: A's instruction with B's vault record in the vault-record slot. Our program
/// derives the vault arguments from A's trading account, so the vault's seed check refuses (2006). One row per instruction
/// that talks to the vault about a record. BLOCKED (vault, behind our program); the local accounts are byte-identical after.
#[test]
fn c_our_instructions_with_another_traders_vault_record_are_refused_and_change_nothing() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let b_vts = vts_of(&b, 0);
    let a_vts = vts_of(&a, 0);
    let t = z.t;
    let swap = |mut ix: Instruction| {
        for m in ix.accounts.iter_mut() {
            if m.pubkey == a_vts {
                m.pubkey = b_vts;
            }
        }
        ix
    };
    // a confirmed pass so finalize reaches the vault; activity older than a day so touch/release reach it too
    z.w.ready(2);
    z.w.set_pending(c::PASS_CONFIRMED, t, t);
    let payer = z.w.new_payer();
    let others = [b];
    // finalize_pass
    z.w.set_time(t + DELAY);
    let ix = swap(z.w.finalize_accts(payer.pubkey()).to_ix());
    let before = world(&z.w, &others);
    let p = dup(&payer);
    let r = z.w.v.e.send(&[ix], &[&p]);
    assert_code(&r, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    assert_eq!(world(&z.w, &others), before, "finalize_pass with B's record changed the world");
    // touch_activity (evaluator) after 90,000 s of silence: our own check (`InvalidVaultAccount`) comes first
    let own = (u32::from(E::InvalidVaultAccount), "InvalidVaultAccount");
    z.w.set_time(t + 90_000);
    let ev = dup(&z.w.v.evaluator);
    let ix = swap(z.w.touch_ix(ev.pubkey()));
    let r = z.w.v.e.send(&[ix], &[&ev]);
    assert_code(&r, own.0, own.1);
    assert_eq!(world(&z.w, &others), before, "touch_activity with B's record changed the world");
    // release_account (trader)
    let tr = dup(&z.w.trader);
    let ix = swap(z.w.release_acct_ix(tr.pubkey()));
    let r = z.w.v.e.send(&[ix], &[&tr]);
    assert_code(&r, own.0, own.1);
    assert_eq!(world(&z.w, &others), before, "release_account with B's record changed the world");
    // request_advance (needs state 0 and the target)
    z.w.ready(2);
    z.w.set_pending(c::PASS_NONE, 0, 0);
    let before = world(&z.w, &others);
    let ix = swap(z.w.request_ix());
    let r = z.w.v.e.send(&[ix], &[&tr]);
    // request_advance has no local check of the record: the vault's seed constraint refuses
    assert_code(&r, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    assert_eq!(world(&z.w, &others), before, "request_advance with B's record changed the world");
    // report_fail (evaluator): a valid failing batch, B's record in the vault slot
    let floor = i64::try_from(z.w.account().size_s / 100 * 92).unwrap();
    z.w.edit_account(|a| a.b = floor);
    let tf = t + 200_000;
    let fb = z.w.fail_batch(tf, vec![], journal_types::FailReason::StaticFloor, floor, true);
    z.w.set_time(tf + 1);
    let ix = swap(z.w.report_fail_ix(ev.pubkey(), fb.to_bytes().unwrap(), 0, &[MARKET_EUR]));
    let before = world(&z.w, &others);
    let r = z.w.v.e.send(&[ix], &[&ev]);
    // report_fail validates the batch and updates the local account first, then the vault refuses: the whole transaction
    // rolls back, so the local account is NOT failed (checked below by the digest and the status)
    assert_code(&r, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    assert_eq!(world(&z.w, &others), before, "report_fail with B's record changed the world");
    assert_eq!(z.w.account().status, c::STATUS_ACTIVE);
    // control: the same failing batch with the right record is accepted and flags the vault record
    z.w.ok(|w| w.report_fail(tf + 1, &fb, 0, &[MARKET_EUR]));
    assert_eq!(z.w.vault_status().status, 2);
}

/// finalize_pass with the other vault accounts wrong: another product's registry, a claim address of another request id,
/// B's wallet profile. Exact error, world unchanged. BLOCKED (vault seeds, or our own seeds for the wallet).
#[test]
fn c_finalize_pass_with_a_foreign_registry_claim_or_wallet_is_refused_and_changes_nothing() {
    let mut z = two();
    let (a, b) = (z.a, z.b.pubkey());
    let t = z.t;
    z.w.ready(2);
    z.w.set_pending(c::PASS_CONFIRMED, t, t);
    let payer = z.w.new_payer();
    z.w.set_time(t + DELAY);
    let others = [b];
    let before = world(&z.w, &others);
    let p = dup(&payer);
    let good = z.w.finalize_accts(payer.pubkey());
    let mut f1 = good.clone();
    f1.product_registry = z.x_registry;
    assert_code(&z.w.v.e.send(&[f1.to_ix()], &[&p]), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    let mut f2 = good.clone();
    f2.payout_claim = claim_pda(&vts_of(&a, 0), 2);
    assert_code(&z.w.v.e.send(&[f2.to_ix()], &[&p]), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    let mut f3 = good.clone();
    f3.payout_claim = claim_pda(&vts_of(&b, 0), 1);
    assert_code(&z.w.v.e.send(&[f3.to_ix()], &[&p]), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    let mut f4 = good.clone();
    f4.wallet = common::wallet_pda(&b);
    assert_code(&z.w.v.e.send(&[f4.to_ix()], &[&p]), ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    assert_eq!(world(&z.w, &others), before, "a refused finalize changed the world");
    // control: the unchanged call pays
    assert!(z.w.v.e.send(&[good.to_ix()], &[&p]).is_ok());
    assert_eq!(z.w.claim(1).unwrap().owed, PAYOUT_100K);
}

// ---- (a) half 2: a REAL CPI from another program ---------------------------------------------------------------------

/// The test-only helper program (tests-rs/fixtures/attacker_cpi, built with `cargo build-sbf --offline`; its source is next to
/// it). It CPIs into the vault signing with the seeds `["setl8_sector_authority", bump]` under ITS OWN program id.
fn attacker_so() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/attacker_cpi.so");
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}). It is a git-ignored build product (like core_vault.so). Build it offline with: \
             cd tests-rs/fixtures/attacker_cpi && cargo build-sbf --offline --sbf-out-dir \"$TMPDIR/attacker_out\" && \
             cp \"$TMPDIR/attacker_out/attacker_cpi.so\" .. (the out dir also gets a throw-away *-keypair.json: do not copy or commit it)",
            p.display()
        )
    })
}
const ATTACKER_ID: Pubkey = Pubkey::new_from_array([0xA7; 32]);

/// Wraps a vault instruction into a call of the helper program. The helper marks the account in the authority slot as a
/// signer for the vault and signs with ITS OWN seeds. `outer_signs_authority` is what the outer transaction says about that
/// account (it must stay false: with signature verification off a flagged signer would be honoured, which proves nothing).
fn via_attacker(inner: &Instruction) -> Instruction {
    let mut accounts = vec![AccountMeta::new_readonly(VAULT_ID, false)];
    accounts.extend(inner.accounts.iter().cloned());
    accounts[1].is_signer = false;
    Instruction { program_id: ATTACKER_ID, accounts, data: inner.data.clone() }
}

/// DESIGN A-3, the real thing: "Impersonate this program to the vault. Blocked by the vault: `sector_authority` must be a PDA
/// signed by this program id." A second program (loaded into the same LiteSVM next to the vault) tries what an attacker program
/// would try, with OUR seed string:
///  1. it names OUR sector PDA as the authority and signs with its own seeds: the Solana runtime refuses the CPI (the
///     authority is not a signer of the caller, and the helper's seeds derive another address);
///  2. it names ITS OWN PDA (same seed string, other program id) as the authority against OUR registry and OUR record: the
///     runtime accepts the signature, the vault refuses with `Unauthorized` (6000);
///  3. CONTROL: the helper registered as its own product can use ITS PDA against ITS registry, so the CPI path itself works
///     (this is what our program does for itself).
///
/// BLOCKED in all three; nothing of ours changes. (Signature verification is off in this harness; the helper's outer
/// instruction never flags the authority as a signer, so the runtime rule is what is tested.)
#[test]
fn a3_real_cpi_from_another_program_cannot_act_as_this_program() {
    let (mut w, _t) = base();
    w.v.e.svm.add_program(ATTACKER_ID, &attacker_so()).expect("load the helper program");
    let tk = w.trader.pubkey();
    let ours = sector_authority_pda();
    let theirs = Pubkey::find_program_address(&[b"setl8_sector_authority"], &ATTACKER_ID).0;
    assert_ne!(ours, theirs, "the same seed under another program id is another address");
    assert_eq!(theirs, si::derive_sector_authority(&ATTACKER_ID).0);
    let before = world(&w, &[]);
    let (ours_state, ours_registry, ours_tally) = (bytes_of_account(&w, &vault_trader_state_pda(&tk, 0)), bytes_of_account(&w, &registry_pda()), bytes_of_account(&w, &tally_pda()));
    for kind in [Vix::FlagFailed, Vix::RecordActivity, Vix::RequestPayout] {
        // 1. our PDA as the authority, not a signer outside
        let c1 = Call::ok(kind, &tk, 0);
        let inner = c1.ix(kind, &w);
        let r = w.v.send_plain(via_attacker(&inner));
        match &r {
            Err(f) => {
                assert!(
                    matches!(&f.err, TransactionError::InstructionError(0, InstructionError::PrivilegeEscalation)),
                    "{kind:?}: want a runtime signer refusal, got {:?}\n{}",
                    f.err,
                    f.meta.logs.join("\n")
                );
                assert!(!f.meta.logs.iter().any(|l| l.contains("FlagTraderFailed") || l.contains("RecordActivity") || l.contains("RequestPayout")), "the vault handler must not run");
            }
            Ok(_) => panic!("{kind:?}: a foreign program signed for our sector PDA"),
        }
        assert_eq!(world(&w, &[]), before);
        // 2. its own PDA as the authority against OUR registry and record
        let mut c2 = Call::ok(kind, &tk, 0);
        c2.authority = theirs;
        let inner = c2.ix(kind, &w);
        let r = w.v.send_plain(via_attacker(&inner));
        assert_code(&r, V_UNAUTHORIZED, "Unauthorized");
        assert_eq!(world(&w, &[]), before);
    }
    // 3. control: the helper as a registered product acts with its own PDA on its own registry (deposit_fee)
    let reg = register_other_product(&mut w, &ATTACKER_ID);
    let fresh = w.v.new_trader();
    let mut cf = Call::ok(Vix::DepositFee, &fresh.pubkey(), 0);
    cf.authority = theirs;
    cf.registry = reg;
    cf.product = ATTACKER_ID;
    cf.state = Pubkey::find_program_address(&[b"trader_state", ATTACKER_ID.as_ref(), fresh.pubkey().as_ref(), &0u64.to_le_bytes()], &VAULT_ID).0;
    let ix = via_attacker(&cf.ix(Vix::DepositFee, &w));
    let f = dup(&fresh);
    let r = w.v.e.send(&[ix], &[&f]);
    assert!(r.is_ok(), "control: the helper's own CPI must work: {:?}", r.err().map(|e| (e.err, e.meta.logs)));
    assert!(w.v.e.svm.get_account(&cf.state).is_some_and(|a| a.owner == VAULT_ID), "the helper product has its record");
    // and the helper product's authority cannot touch OUR record even with its registry: the seeds use the argument
    let mut c3 = Call::ok(Vix::FlagFailed, &tk, 0);
    c3.authority = theirs;
    c3.registry = reg;
    c3.product = ATTACKER_ID;
    let r = w.v.send_plain(via_attacker(&c3.ix(Vix::FlagFailed, &w)));
    assert_code(&r, ANCHOR_CONSTRAINT_SEEDS, "ConstraintSeeds");
    // our record, our registry and our tally are exactly as before the control purchase (which moved tokens of another product)
    assert_eq!(bytes_of_account(&w, &vault_trader_state_pda(&tk, 0)), ours_state);
    assert_eq!(bytes_of_account(&w, &registry_pda()), ours_registry);
    assert_eq!(bytes_of_account(&w, &tally_pda()), ours_tally);
}

// ====================================================================================================================
// (f) R-1: the money a COMPROMISED evaluator can get out, measured
// ====================================================================================================================
//
// DESIGN 13.1 R-1: "Money out is bounded by the 2-day delay + veto, the daily cap and the vault (32% of size per record
// chain)". The tests below drive the REAL program and the REAL vault with a dishonest evaluator that holds the evaluator key
// and fabricates consistent journal batches. They print the measured numbers (run with --nocapture) and assert them.

fn pct(part: u64, whole: u64) -> String {
    format!("{:.4}%", part as f64 * 100.0 / whole as f64)
}

/// R-1, no veto, ONE record chain, $100k tier, launch parameters. The evaluator fabricates a pass in every phase 1..5; the
/// trader requests, the evaluator confirms, the delay passes, anyone finalizes. Result: the vault queues exactly 4 claims of
/// 0.08 S = 32% of S, and every further step is refused. BLOCKED beyond that (program + vault); the 32% is the measured bound.
#[test]
fn r1_compromised_evaluator_without_veto_extracts_exactly_32_percent_of_size_per_chain() {
    let (mut w, t) = base();
    let tk = w.trader.pubkey();
    let s = w.account().size_s;
    let payer = w.new_payer();
    // what the attacker's wallet paid for the chain: the tier price (deposit_fee) + the 0.4% fee
    let spent = TRADER_START - w.v.token_balance(&w.v.trader_token(&tk));
    assert_eq!(spent, 859_000_000 + w.v.expected_fee(8));
    let chain = run_chain_no_veto(&mut w, t, &payer);
    let total = chain.total();
    let claims: u64 = (1..=4).map(|k| w.claim(k).unwrap().owed).sum();
    println!("R-1 one chain, $100k tier, no veto:");
    println!("  payouts (phase, micro-USD): {:?}", chain.paid.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>());
    println!("  chain total paid by the vault = {total} micro-USD = {} of S (DESIGN 13.1 R-1 says 32%)", pct(total, s));
    println!("  tally.requested_total = {}, registry total_requested_amount = {}", w.tally().requested_total, w.registry_totals().1);
    println!("  attacker outlay for the chain (tier price + 0.4% fee) = {spent} micro-USD = {} of S; net = {} micro-USD", pct(spent, s), total - spent);
    assert_eq!(chain.paid.iter().map(|p| p.0).collect::<Vec<_>>(), vec![2, 3, 4, 5]);
    // pacing: two payouts of one chain are never closer than the delay, so one chain yields at most 1 payout per 2 days
    for pair in chain.paid.windows(2) {
        assert!(pair[1].2 - pair[0].2 >= DELAY, "payouts of one chain closer than the delay");
    }
    println!("  first payout {} s after the start, last {} s after the start (delay {DELAY} s)", chain.paid[0].2 - t, chain.paid[3].2 - t);
    assert_eq!(total, 32_000_000_000);
    assert_eq!(total * 100, s * 32, "R-1: 32% of size per record chain, to the micro-USD");
    assert_eq!((claims, w.tally().requested_total, w.registry_totals()), (total, total, (4, total)));
    assert_eq!((w.tally().requested_count, w.vault_status().payout_count, w.vault_status().status), (4, 4, 1));
    assert_eq!(w.account().status, c::STATUS_GRADUATED);
    // nothing more can be pulled from this chain: locally ...
    w.fails(E::AccountNotActive, |w| w.request(chain.end));
    w.fails(E::AccountNotActive, |w| w.confirm(chain.end));
    w.fails(E::AccountNotActive, |w| w.finalize(chain.end, &payer));
    // ... or at the vault (a fifth claim, with the authority flag simulating our program)
    let mut c5 = Call::ok(Vix::RequestPayout, &tk, 0);
    c5.proposed_request_id = 5;
    let before = w.pass_digest();
    assert_code(&send_call(&mut w, Vix::RequestPayout, &c5, None), V_INVALID_TRADER_STATUS, "InvalidTraderStatus");
    assert_eq!(w.pass_digest(), before);
    // and a fail + reset cannot extend the chain (the old record is Graduated, not Failed): reset refused
    let mut rs = Call::ok(Vix::DepositReset, &tk, 0);
    rs.new_challenge = 9;
    rs.new_state = vault_trader_state_pda(&tk, 9);
    let kp = dup(&w.trader);
    assert_code(&send_call(&mut w, Vix::DepositReset, &rs, Some(&kp)), V_RESET_NOT_ALLOWED, "ResetNotAllowed");
    assert_eq!(w.pass_digest(), before);
}

/// R-1 per tier: the chain bound is 0.32 S for EVERY tier (hand table), through fabricated passes and the real vault; the
/// largest single chain is the $200k tier: $64,000. Prints the table.
#[test]
fn r1_chain_bound_is_32_percent_for_every_tier() {
    let mut grand = 0u64;
    println!("R-1 per tier (micro-USD): tier, S, 4 payouts, chain total, % of S, attacker outlay (price + 0.4% fee)");
    for tier in 0..10u8 {
        let mut w = JEnv::with_tier(tier);
        w.set_limits(DELAY as u32, CAP, WINDOW);
        let t = w.t0 + 1_000;
        let tk = w.trader.pubkey();
        let s = w.account().size_s;
        let payer = w.new_payer();
        let outlay = TRADER_START - w.v.token_balance(&w.v.trader_token(&tk));
        let chain = run_chain_no_veto(&mut w, t, &payer);
        let total = chain.total();
        println!("  {tier}  S={s}  {:?}  total={total}  {}  outlay={outlay}", chain.paid.iter().map(|p| p.1).collect::<Vec<_>>(), pct(total, s));
        assert_eq!(total * 100, s * 32, "tier {tier}");
        assert_eq!(chain.paid.len(), 4);
        grand += total;
    }
    // sum of one chain of each tier: 0.32 x (2,500 + 5,000 + 10,000 + 15,000 + 20,000 + 25,000 + 50,000 + 75,000 + 100,000 + 200,000)
    assert_eq!(grand, 32 * (2_500 + 5_000 + 10_000 + 15_000 + 20_000 + 25_000 + 50_000 + 75_000 + 100_000 + 200_000) * 10_000);
}

/// R-1 WITH the veto used on every pass: zero is paid. POLICY (the control is two admin signatures inside the delay; the
/// visible trace is the `vetoes` counter and the `PassVetoed` event). The compromised evaluator re-fabricates and the trader
/// re-requests six times, and the veto lands at four different moments: right after the request (state 1), right after the
/// confirmation, one second before the delay ends, and exactly at the second the delay ends. Nothing is ever paid, the daily
/// window is never touched (vetoed passes do not consume the cap), and a late finalize says `NoPendingPass`.
#[test]
fn r1_with_the_veto_on_every_pass_nothing_is_paid() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let mut t = t;
    // phase 1 needs no money: let it through
    fake_to_target(&mut w, t);
    w.ok(|w| w.request(t + 10));
    w.ok(|w| w.confirm(t + 20));
    assert_eq!(w.account().phase, 2);
    t += 30;
    fake_to_target(&mut w, t);
    let cfg0 = w.v.e.config();
    for round in 0..6u8 {
        w.ok(|w| w.request(t + 10));
        let m;
        let vetoed_at;
        match round % 4 {
            0 => {
                // veto in state 1 (requested)
                vetoed_at = t + 15;
                m = w.ok(|w| w.veto(vetoed_at));
            }
            1 => {
                w.ok(|w| w.confirm(t + 20));
                vetoed_at = t + 25;
                m = w.ok(|w| w.veto(vetoed_at));
            }
            2 => {
                w.ok(|w| w.confirm(t + 20));
                vetoed_at = t + 20 + DELAY - 1;
                w.fails(E::PassDelayNotElapsed, |w| w.finalize(vetoed_at, &payer));
                m = w.ok(|w| w.veto(vetoed_at));
            }
            _ => {
                w.ok(|w| w.confirm(t + 20));
                vetoed_at = t + 20 + DELAY; // finalize would be allowed in this second; the veto is first in the block
                m = w.ok(|w| w.veto(vetoed_at));
            }
        }
        assert_eq!(cpi_events::<PassVetoed>(&m).len(), 1, "PassVetoed is emitted");
        let a = w.account();
        assert_eq!((a.pass_pending, u64::from(a.vetoes), a.phase, a.b), (0, u64::from(round) + 1, 2, TARGET), "round {round}");
        // nothing paid anywhere
        assert_eq!((w.claim(1), w.tally().requested_count, w.tally().requested_total, w.registry_totals(), w.vault_status().payout_count), (None, 0, 0, (0, 0), 0));
        let cfg = w.v.e.config();
        assert_eq!((cfg.window_total, cfg.window_start), (cfg0.window_total, cfg0.window_start), "vetoed passes do not consume the cap");
        w.fails(E::NoPendingPass, |w| w.finalize(vetoed_at + 10 + DELAY, &payer));
        t = vetoed_at + 30;
    }
    println!("R-1 with the veto on every pass: 6 fabricated passes, 6 vetoes, paid = 0 micro-USD (0.0000% of S), vetoes counter = {}", w.account().vetoes);
    assert_eq!(w.account().vetoes, 6);
    assert_eq!(w.vault_status().payout_count, 0);
}

/// R-6 (both admins unreachable) for scale: if nobody vetoes, the SAME chain pays out; the loss is the chain bound and the cap.
/// LIMITED: shown in `r1_compromised_evaluator_without_veto...`; here only that a late veto (after the finalize) is refused
/// because the money is already queued: `NoPendingPass`, and the claim stays.
#[test]
fn r1_a_veto_after_the_finalize_is_too_late_and_changes_nothing() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    let before = w.pass_digest();
    w.fails(E::NoPendingPass, |w| w.veto(t + DELAY + 1));
    assert_eq!(w.pass_digest(), before);
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
}

/// Opens challenges for `trader` until it holds `n` accounts of `tier` (the first one may exist already).
struct Acc {
    trader: Keypair,
    challenge: u64,
    phase: u8,
    confirmed_at: Option<i64>,
    done: bool,
    paid: u64,
}

fn sw(w: &mut JEnv, a: &Acc) {
    w.trader = dup(&a.trader);
    w.acct = common::vault::acct_pda(&a.trader.pubkey(), a.challenge);
}

/// R-1 at SECTOR level with the launch parameters (delay 2 days, cap $100,000 per 24 h window): N = 14 accounts of the $100k
/// tier (three wallets, the 5-account wallet cap respected) run by the compromised evaluator with fabricated passes, no
/// veto, finalizing as often as the program allows (one attempt per account per "tick" of 88,400 s, a little over a window,
/// oldest request first). Every account is taken through its whole chain. Asserts: total claims = N x 0.32 S exactly; no
/// window ever holds more than the cap; the cap really refuses finalizes (the first tick pays 12 of 14); the tally equals the
/// claims. Prints the timeline.
#[test]
fn r1_many_accounts_are_bounded_by_the_daily_cap_and_by_the_chain_bound() {
    let (mut w, _t) = base();
    let tick: i64 = 88_400;
    let payer = w.new_payer();
    let tr0 = dup(&w.trader);
    let mut accs: Vec<Acc> = vec![Acc { trader: dup(&tr0), challenge: 0, phase: 1, confirmed_at: None, done: false, paid: 0 }];
    for n in [5usize, 5, 4] {
        let tr = if accs.len() == 1 { dup(&tr0) } else { w.v.new_trader() };
        let have = if accs.len() == 1 { 1 } else { 0 };
        for k in have..n {
            assert_ok(w.v.open(&tr, 8));
            accs.push(Acc { trader: dup(&tr), challenge: k as u64, phase: 1, confirmed_at: None, done: false, paid: 0 });
        }
        if have == 0 && n != 0 {
            // first account of a new wallet was opened by the loop above when have == 0 and k == 0
        }
    }
    assert_eq!(accs.len(), 14);
    let n = accs.len() as u64;
    let start = w.t0 + 100_000;
    let (mut max_tick, mut max_window, mut refusals) = (0u64, 0u64, 0u32);
    let mut paid_total = 0u64;
    println!("R-1 sector level: {n} accounts of $100k, cap {CAP} per {WINDOW} s window, delay {DELAY} s, tick {tick} s");
    for d in 0..60i64 {
        let td = start + d * tick;
        let mut cl = td;
        let (mut paid_now, mut refused_now) = (0u64, 0u32);
        // 1. finalize attempts, oldest request first
        let mut order: Vec<usize> = (0..accs.len()).filter(|i| accs[*i].confirmed_at.is_some() && !accs[*i].done).collect();
        order.sort_by_key(|i| (accs[*i].confirmed_at, *i));
        for i in order {
            if cl < accs[i].confirmed_at.unwrap() + DELAY {
                continue;
            }
            sw(&mut w, &accs[i]);
            match w.finalize(cl, &payer) {
                Ok(_) => {
                    let k = w.account().next_request_id - 1;
                    let owed = w.claim(k).unwrap().owed;
                    assert_eq!(owed, PAYOUT_100K);
                    accs[i].paid += owed;
                    paid_now += owed;
                    accs[i].phase += 1;
                    accs[i].confirmed_at = None;
                    if accs[i].phase > 5 {
                        accs[i].done = true;
                    }
                    let cfg = w.v.e.config();
                    max_window = max_window.max(cfg.window_total);
                    assert!(cfg.window_total <= CAP, "a window holds more than the cap");
                }
                Err(f) => {
                    assert!(matches!(&f.err, TransactionError::InstructionError(0, InstructionError::Custom(code)) if *code == u32::from(E::DailyPayoutCapReached)), "{:?}\n{}", f.err, f.meta.logs.join("\n"));
                    refused_now += 1;
                }
            }
            cl += 10;
        }
        // 2. fabricate, request, confirm for every account that waits for its next pass
        for acc in accs.iter_mut() {
            while !acc.done && acc.confirmed_at.is_none() {
                sw(&mut w, acc);
                fake_to_target(&mut w, cl);
                w.ok(|w| w.request(cl + 4));
                w.ok(|w| w.confirm(cl + 6));
                if acc.phase == 1 {
                    acc.phase = 2; // the confirmation of phase 1 starts phase 2 at once
                } else {
                    acc.confirmed_at = Some(cl + 6);
                }
                cl += 10;
            }
        }
        max_tick = max_tick.max(paid_now);
        refusals += refused_now;
        paid_total += paid_now;
        println!("  tick {d:2}: paid {paid_now:>12}  refused-by-cap {refused_now}  window_total {:>12}  cumulative {paid_total}", w.v.e.config().window_total);
        if accs.iter().all(|a| a.done) {
            break;
        }
    }
    assert!(accs.iter().all(|a| a.done), "every account finished its chain");
    let claims: u64 = accs.iter().map(|a| a.paid).sum();
    assert_eq!(claims, n * 32_000_000_000, "N x 0.32 S");
    assert_eq!((w.tally().requested_total, w.tally().requested_count, w.registry_totals()), (claims, n * 4, (n * 4, claims)));
    assert!(max_tick <= CAP && max_window <= CAP);
    assert_eq!(max_tick, 96_000_000_000, "12 payouts of $8,000 fit in one window, the 13th does not");
    assert!(refusals > 0, "the cap must have refused at least one finalize");
    println!("  N x 0.32 S = {claims} paid in total; max paid in one tick = {max_tick} ({} of the cap); cap refusals = {refusals}", pct(max_tick, CAP));
}

fn ready_s(w: &mut JEnv, phase: u8) {
    w.edit_account(|a| {
        a.phase = phase;
        a.b = i64::try_from(a.size_s / 100 * 108).unwrap();
        a.flat = 1;
        a.tight_on = 0;
        a.q_tally = 0;
    });
}

/// R-1 cap arithmetic with a MIX of tiers (the evaluator picks the biggest payouts first): in one window the vault is asked for
/// 6 x $16,000 ($200k tier) + 1 x $16,000 (refused: $112,000 > cap) + 1 x $4,000 ($50k tier) = EXACTLY the cap, $100,000. The
/// window rolls at exactly `window_start + 86,400` (one second earlier is refused). Over 4 consecutive windows the evaluator
/// gets 4 x cap = $400,000 in 3 x 86,400 s: the real sector bound for a period of D days is cap x (D + 1), independent of the
/// number of accounts, because the windows are fixed and a burst can sit at both ends. LIMITED: pinned to the micro-USD.
/// The accounts are put into the confirmed state by a state edit (the fabricated-journal route is r1_compromised... and
/// r1_many_accounts...); finalize_pass and the vault are the real ones.
#[test]
fn r1_window_bound_is_exactly_the_cap_and_a_burst_at_each_end_gives_cap_times_days_plus_one() {
    let mut w = JEnv::with_tier(9);
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let payer = w.new_payer();
    let tr0 = dup(&w.trader);
    let mut accs: Vec<Acc> = vec![Acc { trader: dup(&tr0), challenge: 0, phase: 2, confirmed_at: None, done: false, paid: 0 }];
    for _ in 1..5 {
        assert_ok(w.v.open(&tr0, 9));
        let id = w.v.wallet_opt(&tr0.pubkey()).unwrap().next_challenge_id - 1;
        accs.push(Acc { trader: dup(&tr0), challenge: id, phase: 2, confirmed_at: None, done: false, paid: 0 });
    }
    let tr1 = w.v.new_trader();
    for tier in [9u8, 9, 6] {
        assert_ok(w.v.open(&tr1, tier));
        let id = w.v.wallet_opt(&tr1.pubkey()).unwrap().next_challenge_id - 1;
        accs.push(Acc { trader: dup(&tr1), challenge: id, phase: 2, confirmed_at: None, done: false, paid: 0 });
    }
    assert_eq!(accs.len(), 8);
    let big = [0usize, 1, 2, 3, 4, 5, 6];
    let small = 7usize;
    let w0 = w.t0 + 500_000;
    println!("R-1 window arithmetic: cap {CAP}, window {WINDOW} s; 7 x $200k accounts (payout 16,000) + 1 x $50k account (payout 4,000)");
    let mut total = 0u64;
    for k in 0..4i64 {
        let wk = w0 + k * i64::from(WINDOW);
        let phase = u8::try_from(2 + k).unwrap();
        for acc in accs.iter() {
            if acc.phase == phase {
                sw(&mut w, acc);
                ready_s(&mut w, phase);
                w.set_pending(c::PASS_CONFIRMED, wk - 300_000, wk - 299_990);
            }
        }
        // the 7th big account is confirmed too but never gets paid: it is the refused one every window
        let mut paid_this = 0u64;
        for (n, &a) in big.iter().enumerate() {
            sw(&mut w, &accs[a]);
            if a == 6 {
                // not paid in window 0, and it stays in phase 2: it only ever fights for the cap
                if k > 0 {
                    ready_s(&mut w, 2);
                    w.set_pending(c::PASS_CONFIRMED, wk - 300_000, wk - 299_990);
                }
                w.fails(E::DailyPayoutCapReached, |w| w.finalize(wk + 10, &payer));
                continue;
            }
            // first payout of the window exactly at the window second
            let at = wk + i64::try_from(n).unwrap();
            let m = w.ok(|w| w.finalize(at, &payer));
            let ev = cpi_events::<PassFinalized>(&m);
            assert_eq!(ev[0].amount, 16_000_000_000);
            paid_this += ev[0].amount;
            accs[a].phase += 1;
            if n == 0 {
                assert_eq!(w.v.e.config().window_start, wk);
            }
        }
        // the 7th big one is refused ($96,000 + $16,000 > cap); the small one still fits: window total == cap EXACTLY
        sw(&mut w, &accs[small]);
        let m = w.ok(|w| w.finalize(wk + 20, &payer));
        let ev = cpi_events::<PassFinalized>(&m);
        assert_eq!(ev[0].amount, 4_000_000_000);
        accs[small].phase += 1;
        paid_this += ev[0].amount;
        assert_eq!(paid_this, CAP, "window {k}: the evaluator reaches the cap exactly");
        assert_eq!(w.v.e.config().window_total, CAP);
        // anything more in this window is refused, down to the last second
        sw(&mut w, &accs[6]);
        w.fails(E::DailyPayoutCapReached, |w| w.finalize(wk + i64::from(WINDOW) - 1, &payer));
        total += paid_this;
        println!("  window {k} starts at +{} s: paid {paid_this} (= cap), cumulative {total} after {} s", k * i64::from(WINDOW), k * i64::from(WINDOW));
    }
    // the first payout of window k is at w0 + k x 86,400: the window rolled exactly there
    assert_eq!(total, 4 * CAP);
    assert_eq!(w.tally().requested_total, 4 * CAP);
    println!("  R-1 sector bound with launch parameters: {} micro-USD in 3 x {WINDOW} s = cap x (D + 1) for D = 3 days; per full day it is the cap ($100,000)", total);
}

/// R-1: the SIZE OF THE LIE does not change the money. LIMITED: the program does not bound the profit a batch may claim (a
/// consistent fake close of +$999,000 on a $100k account is accepted: b = 11 S), but the payout of that pass is still exactly
/// 0.08 S, because the amount is a function of the tier only (`pass_amount`). So the harm of a compromised evaluator is the
/// number of passes (chain bound, cap, veto), not the size of the invented profit.
#[test]
fn r1_the_size_of_the_invented_profit_does_not_change_the_payout() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let t = w.advance_to(2, t, &payer, DELAY); // phase 1 -> 2 (no money)
    let pnl = 999_000_000_000i64;
    let b = simple(&w, t, vec![close_long_pnl(t, t - 100, pnl)], true);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, S100K + pnl, "the program accepted b = 11 S");
    w.ok(|w| w.request(t + 10));
    let m = w.ok(|w| w.confirm(t + 20));
    let _ = m;
    w.ok(|w| w.finalize(t + 20 + DELAY, &payer));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K, "still exactly 0.08 S of the TIER");
    assert_eq!(w.tally().requested_total, PAYOUT_100K);
    assert_eq!(w.account().b, S100K, "the next phase starts fresh at S, the invented profit is forfeited");
}

/// R-1 / DESIGN 10.6, admin configuration: `set_limits` accepts any non-zero window, also 1 second, and any cap, so the daily cap
/// is only as strong as the two admins' settings. LIMITED (2-of-2 admin power, not a code error): with a 1-second window and a cap of
/// one payout, two payouts one second apart are both accepted. The deploy checklist must pin cap = $100,000 and window = 86,400.
#[test]
fn r1_the_cap_is_only_as_strong_as_the_window_the_admins_set() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.set_limits(DELAY as u32, PAYOUT_100K, 1);
    let cfg = w.v.e.config();
    assert_eq!((cfg.max_payout_per_window, cfg.window_secs), (PAYOUT_100K, 1));
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    w.ok(|w| w.finalize(t + DELAY, &payer));
    w.ready(3);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    // one second later the 1-second window has rolled: a second payout of the same size is accepted
    w.ok(|w| w.finalize(t + DELAY + 1, &payer));
    assert_eq!(w.tally().requested_total, 2 * PAYOUT_100K, "two payouts in two seconds with a cap of one");
    // a zero window or a zero cap is refused
    let ix = set_limits_ix(both(&w.v.e), DELAY as u32, PAYOUT_100K, 0);
    assert!(w.v.e.send_as_admins(&[ix]).is_err());
}

/// DESIGN 10.8, the vault side: while the vault pauses the product, `deposit_fee`, `deposit_reset` and `request_payout` are
/// refused with `ProductNotActive` (6001) and change nothing; `flag_trader_failed` and `record_activity` still work ("a breach
/// is a breach"). BLOCKED / allowed exactly as written; the pause is a second brake on every payout.
#[test]
fn f_vault_pause_stops_money_calls_and_not_the_failure_flag() {
    let (mut w, _t) = base();
    let tk = w.trader.pubkey();
    let fresh = w.v.new_trader();
    w.pause_vault_product();
    let before = world(&w, &[fresh.pubkey()]);
    let kp = dup(&fresh);
    let fee = Call::ok(Vix::DepositFee, &fresh.pubkey(), 0);
    assert_code(&send_call(&mut w, Vix::DepositFee, &fee, Some(&kp)), V_PRODUCT_NOT_ACTIVE, "ProductNotActive");
    let pay = Call::ok(Vix::RequestPayout, &tk, 0);
    assert_code(&send_call(&mut w, Vix::RequestPayout, &pay, None), V_PRODUCT_NOT_ACTIVE, "ProductNotActive");
    assert_eq!(world(&w, &[fresh.pubkey()]), before);
    // record_activity and flag_trader_failed go through
    let act = Call::ok(Vix::RecordActivity, &tk, 0);
    assert!(send_call(&mut w, Vix::RecordActivity, &act, None).is_ok());
    let flag = Call::ok(Vix::FlagFailed, &tk, 0);
    assert!(send_call(&mut w, Vix::FlagFailed, &flag, None).is_ok());
    assert_eq!(w.vault_status().status, 2);
    // a reset of the failed record is a money call: refused while paused, state unchanged
    let reset = Call::ok(Vix::DepositReset, &tk, 0);
    let kp_t = dup(&w.trader);
    let before = world(&w, &[]);
    assert_code(&send_call(&mut w, Vix::DepositReset, &reset, Some(&kp_t)), V_PRODUCT_NOT_ACTIVE, "ProductNotActive");
    assert_eq!(world(&w, &[]), before);
}

/// Griefing at the vault boundary: anyone can send lamports to the address of a future vault record or payout claim before
/// the honest call. The honest trader must not be frozen by that. BLOCKED (vault creates PDAs "safe against pre-funding"): a
/// finalize onto a pre-funded `PayoutClaim` address and an `open_account` / `reset_account` onto a pre-funded
/// `TraderState` address still succeed.
#[test]
fn g_pre_funded_claim_and_record_addresses_do_not_freeze_an_honest_trader() {
    let (mut w, t) = base();
    let payer = w.new_payer();
    let tk = w.trader.pubkey();
    // the claim of request 1
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t, t);
    let claim = claim_pda(&w.vts(), 1);
    w.v.e.svm.airdrop(&claim, 1).unwrap();
    assert_eq!(w.v.e.svm.get_account(&claim).map(|a| (a.lamports, a.owner)), Some((1, system_program::ID)));
    w.ok(|w| w.finalize(t + DELAY, &payer));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
    // the vault record of the NEXT purchase (challenge 1) and of the reset record after a fail
    let rec1 = vault_trader_state_pda(&tk, 1);
    w.v.e.svm.airdrop(&rec1, 1).unwrap();
    let kp = dup(&w.trader);
    assert_ok(w.v.open(&kp, 8)); // challenge 1 (account 0 is still active: wallet cap is 5)
    assert_eq!(w.v.vault_trader_state(&tk, 1).unwrap().status, 0);
    w.acct = common::vault::acct_pda(&tk, 1);
    w.fail_in(2, t + 10 * DELAY);
    let rec2 = vault_trader_state_pda(&tk, 2);
    w.v.e.svm.airdrop(&rec2, 1).unwrap();
    w.reset_ok(t + 10 * DELAY + 100);
    assert_eq!(w.v.vault_trader_state(&tk, 2).unwrap().status, 0);
}

/// A confirmed pass, nothing else touching the account since it was opened (the vault's activity clock is the open time), with
/// the pass delay set to `delay` seconds. Returns the world, the payer and the finalize time (= confirm time + delay).
fn long_delay_world(delay: i64) -> (JEnv, Keypair, i64) {
    let (mut w, t) = base();
    let payer = w.new_payer();
    w.set_limits(u32::try_from(delay).unwrap(), CAP, WINDOW);
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t + 5, 0);
    w.ok(|w| w.confirm(t + 20));
    (w, payer, t + 20 + delay)
}

// FINDING VLT-1
/// DESIGN 14 lets both admins set `pass_delay_secs` anywhere in 1 to 14 days, and DESIGN 10.1 / the vault abandon a record that is
/// idle for more than 7 days (`INACTIVITY_LIMIT_SECS` = 604,800). A pass confirmed with a delay above about 7 days is therefore
/// finalized against a record the vault already considers idle: `request_payout` flips it to Abandoned, returns Ok(Abandoned),
/// and `finalize_pass` marks the account Abandoned: the honest trader loses the account and the 0.08 S, silently, although every
/// step was done by the book. Nothing in the frozen account refreshes the activity clock (`confirm_pass` does not call the
/// vault; the account is frozen, so no orders reach the evaluator; the evaluator's `touch_activity` follows orders only).
/// Expected by the DESIGN: a pass that was valid and confirmed is paid after ANY delay inside the allowed range.
/// Needs a DESIGN change (clamp the maximum delay below the vault's idle limit, or make `confirm_pass` refresh the activity
/// clock = new vault CPI accounts in `confirm_pass`).
#[test]
#[ignore = "FINDING VLT-1: pass_delay_secs may be 8 days but the vault abandons a record idle for 7 days, so an honest confirmed pass is lost at finalize"]
fn h_a_confirmed_pass_is_paid_after_any_allowed_delay_even_longer_than_the_vault_idle_limit() {
    let (mut w, payer, fin) = long_delay_world(8 * 86_400);
    let r = w.finalize(fin, &payer);
    assert!(r.is_ok(), "{:?}", r.err().map(|f| f.err));
    assert_eq!(w.account().status, c::STATUS_ACTIVE, "the honest account must not be Abandoned by a valid pass");
    assert_eq!(w.claim(1).map(|c| c.owed), Some(PAYOUT_100K), "the honest 0.08 S must be queued");
}

/// The mitigation that exists today and the control: with the default 2-day delay the same flow pays; with an 8-day delay the
/// pass is paid ONLY if somebody refreshes the vault's activity clock inside the delay (`release_account` by the trader, or
/// `touch_activity` by the evaluator after the 86,000 s throttle), both of which are allowed in state 2. LIMITED: pinned.
#[test]
fn h_long_delay_is_survivable_only_if_the_activity_clock_is_refreshed_inside_the_delay() {
    // control: 2 days
    let (mut w, payer, fin) = long_delay_world(DELAY);
    w.ok(|w| w.finalize(fin, &payer));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
    // 8 days, trader refreshes on day 4
    let (mut w, payer, fin) = long_delay_world(8 * 86_400);
    let day4 = w.t0 + 4 * 86_400;
    w.ok(|w| w.release_acct(day4));
    w.ok(|w| w.finalize(fin, &payer));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
    // 8 days, the evaluator refreshes on day 5
    let (mut w, payer, fin) = long_delay_world(8 * 86_400);
    let day5 = w.t0 + 5 * 86_400;
    w.ok(|w| w.touch(day5));
    w.ok(|w| w.finalize(fin, &payer));
    assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
}
