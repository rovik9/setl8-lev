//! M12 item 3: ACCOUNT CONFUSION AND ACCOUNT HYGIENE, for every instruction that takes accounts.
//!
//! Method.
//! * `scenarios()` builds, for each of the 25 instructions of the program, a world in which the legitimate call succeeds, and the
//!   instruction from the REAL builders of `tests-rs/tests/common` (`accounts::X { .. }.to_account_metas`, `OpenAccounts`,
//!   `ResetAccts`, `FinalizeAccts`, `JEnv::*_ix`). Each account slot of each instruction gets a hand-written type `Ty` (what the
//!   Anchor struct and the handler demand of that slot, read from `programs/setl8-lev-trading/src/instructions/*.rs`).
//! * The generic mutations (`Mu`) are applied one at a time to a fresh fork of that world: wrong owner, lookalike at another address
//!   (same owner / foreign owner), closed, revived-empty, missing, below rent, wrong discriminator, truncated data, flipped stored bump,
//!   non-canonical bump address, writable <-> read-only, signer <-> non-signer, the key of another slot copied in (duplicate), two
//!   slots swapped, a fake program id in a program slot, accounts dropped from the end, an extra account appended.
//! * The expectation of every row is computed by `expect()` from the slot type and the mutation ONLY (a hand-written table of what Anchor
//!   0.32 does, see the comments there), never from running the program. A refusal must carry the exact error and leave every account
//!   byte-identical (config, tally, wallets, markets, rule sets, trading accounts, vault registry/state/records/claims, token accounts,
//!   lamports of every key involved; the fee payer of the transaction is the only account left out). A success must be a case listed
//!   as harmless with its reason.
//! * Dedicated tests at the end: re-initialisation (open_account on an existing account, init_config twice, create_* twice), reuse of a
//!   closed address inside one transaction, the revival attempt, account of another trader, wrong mint / wrong token program.
mod common;

use std::collections::BTreeMap;
use std::rc::Rc;

use anchor_lang::solana_program::instruction::{error::InstructionError, AccountMeta, Instruction};
use anchor_lang::solana_program::{pubkey::Pubkey, system_program};
use anchor_lang::{AccountDeserialize, AccountSerialize, ToAccountMetas};
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::{
    both, config_pda, create_market_args, create_market_ix, create_ruleset_ix, default_args, dup, init_ix_with, launch_ruleset_args, market_params,
    market_pda, ruleset_pda, set_evaluator_ix, set_limits_ix, set_paused_ix, tally_pda, unban_wallet_ix, update_market_ix, wallet_pda, Admins, Env,
    PROGRAM_ID,
};
use litesvm::types::TransactionResult;
use setl8_lev_trading::{
    accounts, constants as c,
    errors::Setl8Error as E,
    instruction,
    instructions::UpdateMarketArgs,
    state::{Market, RuleSet, SectorConfig, TradingAccount, WalletProfile},
};
use sha2::{Digest, Sha256};
use solana_account::Account as RawAccount;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

// ====================================================================================================== slot types

/// What an account SLOT demands (Anchor struct + handler), written by hand from the instruction sources. `w` is the
/// `#[account(mut)]` flag of OUR struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ty {
    /// `#[account(mut)] Signer`
    SigW,
    /// `Signer`
    Sig,
    /// `Signer` + `address = <compiled-in admin key> @ Unauthorized` (init_config)
    SigPin,
    /// `Account<SectorConfig>` seeds `["config"]`, stored bump
    Cfg(bool),
    /// `Account<TradingAccount>` seeds from its own trader / challenge id, stored bump; `tc` = slot of the trader signer when
    /// `constraint = trading_account.trader == trader.key() @ Unauthorized` exists
    Ta { w: bool, tc: Option<u8> },
    /// `Account<WalletProfile>` seeds `["wallet", trader]`, stored bump, `mut`
    Wal,
    /// `Account<RuleSet>` seeds from the trading account's version, stored bump, `constraint version == ..`
    RsSeed,
    /// `Account<RuleSet>` without seeds; the handler checks version and bump address (open_account, reset_account)
    RsFree,
    /// `Account<Market>` seeds `["market", id]`, stored bump, `mut`
    Mkt,
    /// `UncheckedAccount` compared with the stored admin key in the handler
    Adm,
    /// `UncheckedAccount` seeds `["setl8_sector_authority"]`, canonical bump
    SecAuth,
    /// `UncheckedAccount` `address = config.vault_program @ InvalidVaultProgram`
    VProg,
    /// vault `ProductRegistry` (unchecked here; the vault checks owner, discriminator, seeds). `bool` = our `mut`
    VReg(bool),
    /// vault `ProductRegistry` that OUR handler also decodes before the CPI (reset_account: `vault_view::check_registry`)
    VRegRd,
    /// vault `TraderState` of an existing challenge (unchecked here, `mut`; the vault checks)
    Vts,
    /// the same, but our handler reads its status byte before the CPI (touch_activity, release_account: `vault_status`)
    VtsRd,
    /// vault `TraderState` created by the vault in this instruction (open_account, reset_account)
    VtsNew,
    /// vault `TraderState` of the FAILED challenge in reset_account: owner and PDA checked in our handler (InvalidVaultAccount)
    VtsPrev,
    /// vault `VaultState`. `bool` = our `mut`
    VState(bool),
    /// the new claim PDA of finalize_pass (created by the vault)
    Claim,
    /// the vault pool token account (unchecked here, `mut`)
    Pool,
    /// the vault SL8 destination token account (unchecked here, `mut`)
    Sl8,
    /// `Account<TokenAccount>` + `constraint owner == trader && mint == mint @ InvalidTokenAccount`, `mut` (the buyer's source)
    TraderTok,
    /// `Account<TokenAccount>` + `constraint owner == config.sl8_wallet && mint == mint @ InvalidTokenAccount`, `mut` (the fee destination)
    FeeTok,
    /// `Account<Mint>`
    Mint,
    /// `Program<Token>`
    Tok,
    /// `Program<System>`
    Sys,
    /// `event_cpi` `event_authority` (seeds `["__event_authority"]`)
    EvAuth,
    /// `event_cpi` `program` (no constraint at all)
    SelfProg,
    /// payout tally: unchecked, `mut`, seeds `["payout_tally"]`, `owner = crate::ID` (finalize_pass)
    Tally,
    /// `init` accounts: config / payout tally (init_config), rule set, market, trading account; `init_if_needed` wallet
    InitCfg,
    InitTally,
    InitRs,
    InitMkt,
    InitWal,
    InitTa,
    /// a `Market` passed as a remaining account of post_journal / report_fail
    Rem,
}

// ====================================================================================================== world

enum World {
    /// Vault world with one account of one trader (`JEnv`).
    J(Box<JEnv>),
    /// Bare program world, no config (init_config).
    P(Box<Env>),
}

fn fork_env(e: &Env) -> Env {
    Env { svm: e.svm.clone(), sl8: dup(&e.sl8), rov: dup(&e.rov), payer: dup(&e.payer), last_tx: e.last_tx }
}

fn fork_j(w: &JEnv) -> JEnv {
    let v = &w.v;
    let nv = VaultEnv {
        e: fork_env(&v.e),
        usdc: v.usdc,
        usdt: v.usdt,
        vault_state: v.vault_state,
        usdc_pool: v.usdc_pool,
        usdt_pool: v.usdt_pool,
        vault_sl8_usdc: v.vault_sl8_usdc,
        fee_wallet: v.fee_wallet,
        fee_usdc: v.fee_usdc,
        ruleset: v.ruleset.clone(),
        evaluator: dup(&v.evaluator),
        tracked: v.tracked.clone(),
        traders: v.traders.clone(),
    };
    JEnv { v: nv, trader: dup(&w.trader), acct: w.acct, t0: w.t0 }
}

impl World {
    fn fork(&self) -> World {
        match self {
            World::J(w) => World::J(Box::new(fork_j(w))),
            World::P(e) => World::P(Box::new(fork_env(e))),
        }
    }
    fn env(&mut self) -> &mut Env {
        match self {
            World::J(w) => &mut w.v.e,
            World::P(e) => e,
        }
    }
    fn envr(&self) -> &Env {
        match self {
            World::J(w) => &w.v.e,
            World::P(e) => e,
        }
    }
    fn j(&self) -> &JEnv {
        match self {
            World::J(w) => w,
            World::P(_) => panic!("not a vault world"),
        }
    }
}

/// One instruction in a world where it succeeds.
struct Scn {
    name: &'static str,
    world: World,
    /// Instructions before the one under test (compute budget). The index of the one under test is `pre.len()`.
    pre: Vec<Instruction>,
    ix: Instruction,
    tys: Vec<Ty>,
    /// Keypairs that sign (the transaction signs for the required ones; the world has signature verification off).
    keys: Vec<Keypair>,
    /// The success condition of the unmutated call.
    post: Rc<dyn Fn(&World) -> bool>,
    /// A second trader's valid account (and wallet), for the "another trader's account" row; `None` when not applicable.
    relayer: Keypair,
}

impl Scn {
    fn fork(&self) -> Scn {
        Scn {
            name: self.name,
            world: self.world.fork(),
            pre: self.pre.clone(),
            ix: self.ix.clone(),
            tys: self.tys.clone(),
            keys: self.keys.iter().map(dup).collect(),
            post: self.post.clone(),
            relayer: dup(&self.relayer),
        }
    }
    fn n(&self) -> usize {
        self.ix.accounts.len()
    }
}

fn budget_ix() -> Instruction {
    Instruction {
        program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(),
        accounts: vec![],
        data: [vec![2u8], 600_000u32.to_le_bytes().to_vec()].concat(),
    }
}

const DELAY: u32 = 172_800;
const WINDOW: u32 = 86_400;
const CAP: u64 = 100_000_000_000;

fn ev_auth() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

fn mk(name: &'static str, mut w: JEnv, t: i64, pre: Vec<Instruction>, ix: Instruction, tys: Vec<Ty>, post: Rc<dyn Fn(&World) -> bool>) -> Scn {
    assert_eq!(ix.accounts.len(), tys.len(), "{name}: slot list and account list differ");
    w.set_time(t);
    let relayer = Keypair::new();
    w.v.e.svm.airdrop(&relayer.pubkey(), 10_000_000_000).unwrap();
    let keys = vec![dup(&w.trader), dup(&w.v.evaluator), dup(&w.v.e.sl8), dup(&w.v.e.rov)];
    Scn { name, world: World::J(Box::new(w)), pre, ix, tys, keys, post, relayer }
}

/// The 25 scenarios, in the order of the `#[program]` block.
fn scenarios() -> Vec<Scn> {
    scenarios_for("")
}

/// `only` = "" builds all 25 scenarios, a name builds just that one (each world costs about 150 ms).
fn scenarios_for(only: &str) -> Vec<Scn> {
    let sel = |n: &str| only.is_empty() || only == n;
    use Ty::*;
    let ta_w = |tc: Option<u8>| Ta { w: true, tc };
    let admin3 = vec![Cfg(true), Adm, Adm, EvAuth, SelfProg];
    let mut out: Vec<Scn> = Vec::new();

    // ---- init_config: a world without a config
    {
        // signature verification OFF (like the vault world): a signer flag without a signature must reach the program, which is
        // the thing under test; real signatures are exercised by who_may_call.rs
        if sel("init_config") {
        let e = Env::new_with(false);
        let ix = e.init_ix(&default_args());
        let relayer = Keypair::new();
        let mut e = e;
        e.svm.airdrop(&relayer.pubkey(), 10_000_000_000).unwrap();
        let keys = vec![dup(&e.sl8), dup(&e.rov)];
        out.push(Scn {
            name: "init_config",
            world: World::P(Box::new(e)),
            pre: vec![],
            ix,
            tys: vec![SigW, SigPin, SigPin, InitCfg, InitTally, Sys],
            keys,
            post: Rc::new(|w| w.envr().svm.get_account(&config_pda()).is_some_and(|a| a.owner == PROGRAM_ID && !a.data.is_empty())),
            relayer,
        });
        }
    }

    // ---- the three one-shot admin instructions
    {
        if sel("set_evaluator") {
        let w = JEnv::new();
        let new_key = Pubkey::new_from_array([0x42; 32]);
        let ix = set_evaluator_ix(both(&w.v.e), new_key);
        let t = w.t0 + 1_000;
        out.push(mk("set_evaluator", w, t, vec![], ix, admin3.clone(), Rc::new(move |w| w.envr().config().evaluator == new_key)));
        }
        if sel("set_limits") {
        let w = JEnv::new();
        let ix = set_limits_ix(both(&w.v.e), 200_000, 6_000_000_000, 3_600);
        let t = w.t0 + 1_000;
        out.push(mk("set_limits", w, t, vec![], ix, admin3.clone(), Rc::new(|w| w.envr().config().max_payout_per_window == 6_000_000_000)));
        }
        if sel("set_paused") {
        let w = JEnv::new();
        let ix = set_paused_ix(both(&w.v.e), true);
        let t = w.t0 + 1_000;
        out.push(mk("set_paused", w, t, vec![], ix, admin3.clone(), Rc::new(|w| w.envr().config().paused == 1)));
        }
    }
    // ---- create_ruleset / create_market / update_market
    {
        if sel("create_ruleset") {
        let w = JEnv::new();
        let mut args = launch_ruleset_args();
        args.version = 2;
        let ix = create_ruleset_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), &args);
        let t = w.t0 + 1_000;
        out.push(mk("create_ruleset", w, t, vec![], ix, vec![SigW, Cfg(true), Adm, Adm, InitRs, Sys, EvAuth, SelfProg], Rc::new(|w| w.envr().config().current_ruleset == 2)));
        }
        if sel("create_market") {
        let w = JEnv::new();
        let id = w.v.e.config().market_count;
        let ix = create_market_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), &create_market_args(id, 0, 55));
        let t = w.t0 + 1_000;
        out.push(mk("create_market", w, t, vec![], ix, vec![SigW, Cfg(true), Adm, Adm, InitMkt, Sys, EvAuth, SelfProg], Rc::new(move |w| w.envr().config().market_count == id + 1)));
        }
        if sel("update_market") {
        let w = JEnv::new();
        let ix = update_market_ix(Admins::both(&w.v.e), &UpdateMarketArgs { market_id: 0, params: market_params(77) });
        let t = w.t0 + 1_000;
        out.push(mk("update_market", w, t, vec![], ix, vec![Cfg(false), Adm, Adm, Mkt, EvAuth, SelfProg], Rc::new(|w| w.envr().market(0).feed_id == [77; 32])));
        }
    }
    // ---- wallet bans
    {
        if sel("ban_wallet") {
        let w = JEnv::new();
        let victim = Keypair::new().pubkey();
        let ix = common::ban_wallet_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), victim, hash(5));
        let t = w.t0 + 1_000;
        out.push(mk("ban_wallet", w, t, vec![], ix, vec![SigW, Cfg(false), Adm, Adm, InitWal, Sys, EvAuth, SelfProg], Rc::new(move |w| w.envr().wallet(&victim).banned == 1)));
        }
        if sel("unban_wallet") {
        let mut w = JEnv::new();
        let victim = w.trader.pubkey();
        let ban = common::ban_wallet_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), victim, hash(5));
        let a = Admins::both(&w.v.e);
        common::assert_ok(w.v.e.send_admins(ban, a));
        let ix = unban_wallet_ix(Admins::both(&w.v.e), victim);
        let t = w.t0 + 1_000;
        out.push(mk("unban_wallet", w, t, vec![], ix, vec![Cfg(false), Adm, Adm, Wal, EvAuth, SelfProg], Rc::new(move |w| w.envr().wallet(&victim).banned == 0)));
        }
    }
    // ---- open_account (the trader opens his SECOND account) and reset_account
    {
        if sel("open_account") {
        let w = JEnv::new();
        let oa = w.v.open_accounts(&w.trader.pubkey());
        let acct = oa.trading_account;
        let ix = oa.to_ix(0);
        let t = w.t0 + 1_000;
        out.push(mk(
            "open_account",
            w,
            t,
            vec![],
            ix,
            vec![SigW, SigW, Cfg(false), RsFree, InitWal, InitTa, SecAuth, VProg, VReg(true), VtsNew, VState(false), TraderTok, Mint, Pool, Sl8, FeeTok, Tok, Sys, EvAuth, SelfProg],
            Rc::new(move |w| w.envr().svm.get_account(&acct).is_some_and(|a| a.owner == PROGRAM_ID && !a.data.is_empty())),
        ));
        }
        if sel("reset_account") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.fail_in(1, t);
        let prev_id = w.account().challenge_id;
        let ra = w.reset_accts();
        let (new_acct, prev_acct) = (ra.trading_account, ra.prev_trading_account);
        let ix = ra.to_ix(prev_id);
        out.push(mk(
            "reset_account",
            w,
            t + 100,
            vec![],
            ix,
            vec![SigW, SigW, Cfg(false), RsFree, Wal, ta_w(Some(0)), InitTa, SecAuth, VProg, VRegRd, VtsNew, VtsPrev, VState(false), TraderTok, Mint, Pool, Sl8, FeeTok, Tok, Sys, EvAuth, SelfProg],
            Rc::new(move |w| w.envr().svm.get_account(&new_acct).is_some_and(|a| a.owner == PROGRAM_ID && !a.data.is_empty()) && w.j().v.account(&prev_acct).reset_at() != 0),
        ));
        }
    }
    // ---- post_journal
    {
        if sel("post_journal") {
        let w = JEnv::new();
        let t = w.t0 + 1_000;
        let batch = simple(&w, t, vec![], true);
        let mut metas = accounts::PostJournal {
            evaluator: w.v.evaluator.pubkey(),
            trading_account: w.acct,
            config: config_pda(),
            ruleset: ruleset_pda(w.account().ruleset_version),
            event_authority: ev_auth(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(JEnv::market_metas(&[MARKET_EUR]));
        let ix = Instruction { program_id: PROGRAM_ID, accounts: metas, data: anchor_lang::InstructionData::data(&instruction::PostJournal { batch_bytes: batch.to_bytes().unwrap() }) };
        out.push(mk(
            "post_journal",
            w,
            t,
            vec![budget_ix()],
            ix,
            vec![SigW, ta_w(None), Cfg(false), RsSeed, EvAuth, SelfProg, Rem],
            Rc::new(|w| w.j().account().seq == 1),
        ));
        }
    }
    // ---- pass flow
    {
        if sel("request_advance") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.ready(2);
        let ix = w.request_ix();
        out.push(mk("request_advance", w, t, vec![], ix, vec![Sig, ta_w(Some(0)), Wal, Cfg(false), SecAuth, VProg, VReg(false), Vts, EvAuth, SelfProg], Rc::new(|w| w.j().account().pass_pending == c::PASS_REQUESTED)));
        }
        if sel("cancel_pass") {
        let mut w = JEnv::new();
        let t = w.t0 + 10_000;
        w.ready(2);
        w.set_pending(c::PASS_REQUESTED, t - 4_000, 0);
        let ix = w.cancel_ix();
        out.push(mk("cancel_pass", w, t, vec![], ix, vec![Sig, ta_w(Some(0)), EvAuth, SelfProg], Rc::new(|w| w.j().account().pass_pending == c::PASS_NONE)));
        }
        if sel("confirm_pass") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.ready(2);
        w.set_pending(c::PASS_REQUESTED, t - 100, 0);
        let ix = w.confirm_ix(w.v.evaluator.pubkey());
        out.push(mk("confirm_pass", w, t, vec![], ix, vec![Sig, ta_w(None), Cfg(false), EvAuth, SelfProg], Rc::new(|w| w.j().account().pass_pending == c::PASS_CONFIRMED)));
        }
        if sel("reject_pass") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.ready(2);
        w.set_pending(c::PASS_REQUESTED, t - 100, 0);
        let ix = w.reject_ix(w.v.evaluator.pubkey(), c::REJECT_OTHER);
        out.push(mk("reject_pass", w, t, vec![], ix, vec![Sig, ta_w(None), Cfg(false), EvAuth, SelfProg], Rc::new(|w| w.j().account().pass_pending == c::PASS_NONE)));
        }
        if sel("veto_pass") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.ready(2);
        w.set_pending(c::PASS_CONFIRMED, t - 200, t - 100);
        let ix = w.veto_ix(Admins::both(&w.v.e));
        out.push(mk("veto_pass", w, t, vec![], ix, vec![Cfg(false), Adm, Adm, ta_w(None), EvAuth, SelfProg], Rc::new(|w| w.j().account().vetoes == 1)));
        }
        if sel("finalize_pass") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.set_limits(DELAY, CAP, WINDOW);
        w.ready(2);
        w.set_pending(c::PASS_CONFIRMED, t - 200, t);
        let payer = w.v.e.payer.pubkey();
        let ix = w.finalize_accts(payer).to_ix();
        out.push(mk(
            "finalize_pass",
            w,
            t + i64::from(DELAY),
            vec![],
            ix,
            vec![SigW, ta_w(None), Cfg(true), Tally, Wal, SecAuth, VProg, VReg(true), Vts, VState(true), Claim, Sys, EvAuth, SelfProg],
            Rc::new(|w| w.j().account().phase == 3 && w.j().tally().requested_count == 1),
        ));
        }
    }
    // ---- report_fail
    {
        if sel("report_fail") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.edit_account(|a| a.b = FLOOR);
        let batch = w.fail_batch(t, vec![], journal_types::FailReason::StaticFloor, FLOOR, true);
        let ix = w.report_fail_ix(w.v.evaluator.pubkey(), batch.to_bytes().unwrap(), 0, &[MARKET_EUR]);
        out.push(mk(
            "report_fail",
            w,
            t + 1,
            vec![],
            ix,
            vec![SigW, ta_w(None), Cfg(false), RsSeed, Wal, SecAuth, VProg, VReg(false), Vts, EvAuth, SelfProg, Rem],
            Rc::new(|w| w.j().account().status == c::STATUS_FAILED),
        ));
        }
    }
    // ---- hold / release_hold / fail_for_violation
    {
        if sel("hold_account") {
        let w = JEnv::new();
        let t = w.t0 + 1_000;
        let ix = w.hold_ix(w.v.evaluator.pubkey(), hash(1));
        out.push(mk("hold_account", w, t, vec![], ix, vec![Sig, ta_w(None), Wal, Cfg(false), EvAuth, SelfProg], Rc::new(|w| w.j().account().held == 1)));
        }
        if sel("release_hold") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        common::assert_ok(w.hold(t, hash(1)));
        let ix = w.release_hold_ix(Admins::both(&w.v.e));
        out.push(mk("release_hold", w, t + 1, vec![], ix, vec![Cfg(false), Adm, Adm, ta_w(None), Wal, EvAuth, SelfProg], Rc::new(|w| w.j().account().held == 0)));
        }
        if sel("fail_for_violation") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        common::assert_ok(w.hold(t, hash(1)));
        let ix = w.violation_ix(Admins::both(&w.v.e), hash(9));
        out.push(mk("fail_for_violation", w, t + 1, vec![], ix, vec![Cfg(false), Adm, Adm, ta_w(None), Wal, SecAuth, VProg, VReg(false), Vts, EvAuth, SelfProg], Rc::new(|w| w.j().account().status == c::STATUS_FAILED)));
        }
    }
    // ---- activity, close
    {
        if sel("touch_activity") {
        let w = JEnv::new();
        let t = w.t0 + 90_000;
        let ix = w.touch_ix(w.v.evaluator.pubkey());
        out.push(mk("touch_activity", w, t, vec![], ix, vec![Sig, ta_w(None), Wal, Cfg(false), SecAuth, VProg, VReg(false), VtsRd, EvAuth, SelfProg], Rc::new(move |w| w.j().account().last_activity_pushed == t)));
        }
        if sel("release_account") {
        let w = JEnv::new();
        let t = w.t0 + 90_000;
        let ix = w.release_acct_ix(w.trader.pubkey());
        out.push(mk("release_account", w, t, vec![], ix, vec![Sig, ta_w(Some(0)), Wal, Cfg(false), SecAuth, VProg, VReg(false), VtsRd, EvAuth, SelfProg], Rc::new(move |w| w.j().account().last_activity_pushed == t)));
        }
        if sel("close_account") {
        let mut w = JEnv::new();
        let t = w.t0 + 1_000;
        w.make_terminal(c::STATUS_FAILED, t);
        let ix = w.close_ix(w.trader.pubkey());
        let acct = w.acct;
        out.push(mk("close_account", w, t + RETENTION, vec![], ix, vec![SigW, ta_w(Some(0))], Rc::new(move |w| w.envr().svm.get_account(&acct).is_none_or(|a| a.lamports == 0 || a.data.is_empty()))));
        }
    }
    out
}

// ====================================================================================================== sending and digests

fn digest_keys(s: &Scn, ix: &Instruction) -> Vec<Pubkey> {
    let mut extra: Vec<Pubkey> = ix.accounts.iter().map(|m| m.pubkey).collect();
    extra.extend(s.ix.accounts.iter().map(|m| m.pubkey));
    for k in &s.keys {
        extra.push(k.pubkey());
    }
    extra.push(sector_authority_pda());
    extra.push(ev_auth());
    for m in 0..8u16 {
        extra.push(market_pda(m));
    }
    if let World::J(w) = &s.world {
        let tr = w.trader.pubkey();
        for id in 0..4u64 {
            let vts = vault_trader_state_pda(&tr, id);
            for rid in 1..=6u64 {
                extra.push(claim_pda(&vts, rid));
            }
        }
        extra.push(w.v.usdt_pool);
    }
    extra.sort();
    extra.dedup();
    extra
}

fn digest_of(s: &Scn, ix: &Instruction) -> [u8; 32] {
    let extra: Vec<Pubkey> = digest_keys(s, ix).into_iter().filter(|k| *k != s.relayer.pubkey()).collect();
    match &s.world {
        World::J(w) => w.v.digest(&w.trader.pubkey(), &extra),
        World::P(e) => {
            let mut addrs = extra;
            addrs.push(config_pda());
            addrs.push(tally_pda());
            addrs.sort();
            addrs.dedup();
            let mut h = Sha256::new();
            for a in addrs {
                h.update(a.as_ref());
                match e.svm.get_account(&a) {
                    Some(acc) if acc.lamports > 0 => {
                        h.update([1u8]);
                        h.update(acc.lamports.to_le_bytes());
                        h.update(acc.owner.as_ref());
                        h.update(&acc.data);
                    }
                    _ => h.update([0u8]),
                }
            }
            h.finalize().into()
        }
    }
}

/// Outcome of one row.
#[derive(Debug)]
struct Out {
    result: TransactionResult,
    before: [u8; 32],
    after: [u8; 32],
    post_ok: bool,
}

/// Sends `pre + ix` with the relayer as fee payer (never a slot of any instruction), at the clock already set in the world.
fn run(s: &mut Scn, ix: Instruction) -> Out {
    let before = digest_of(s, &ix);
    let mut ixs = s.pre.clone();
    ixs.push(ix.clone());
    let relayer = dup(&s.relayer);
    let keys: Vec<Keypair> = s.keys.iter().map(dup).collect();
    let e = s.world.env();
    let bh = e.svm.latest_blockhash();
    let msg = solana_message::Message::new_with_blockhash(&ixs, Some(&relayer.pubkey()), &bh);
    let mut tx = solana_transaction::Transaction::new_unsigned(msg);
    let n = tx.message.header.num_required_signatures as usize;
    let required: Vec<Pubkey> = tx.message.account_keys[..n].to_vec();
    let mut sigs: Vec<&Keypair> = vec![&relayer];
    let mut seen = vec![relayer.pubkey()];
    for k in &keys {
        if required.contains(&k.pubkey()) && !seen.contains(&k.pubkey()) {
            seen.push(k.pubkey());
            sigs.push(k);
        }
    }
    tx.partial_sign(sigs.as_slice(), bh);
    let result = e.svm.send_transaction(tx);
    e.svm.expire_blockhash();
    let after = digest_of(s, &ix);
    let post_ok = result.is_ok() && (s.post)(&s.world);
    Out { result, before, after, post_ok }
}

/// The unmutated call must succeed (and meet its postcondition) in every scenario; a broken scenario would make every row vacuous.
fn legit(s: &Scn) -> Out {
    let mut f = s.fork();
    let ix = f.ix.clone();
    run(&mut f, ix)
}

// ====================================================================================================== mutations

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mu {
    /// same address, same data, owner = a foreign program
    Foreign(usize),
    /// new address, same owner and data (a lookalike of the right size and discriminator, owned by this program / the vault)
    LookSame(usize),
    /// new address, foreign owner, same data (a lookalike of the right size and discriminator, owned by someone else)
    LookForeign(usize),
    /// same address, 0 lamports, no data, system owner (closed)
    Closed(usize),
    /// same address, funded, system owner, no data (the closed address revived with lamports)
    Funded(usize),
    /// a fresh address nobody ever created
    Missing(usize),
    /// lamports = rent exemption - 1
    BelowRent(usize),
    /// first discriminator byte flipped
    BadDisc(usize),
    /// data cut to 4 bytes
    Short(usize),
    /// the stored `bump` field flipped (PDAs with a stored bump)
    BumpFlip(usize),
    /// a lookalike at the NON-canonical bump address of the same seeds, stored bump = that bump
    NonCanon(usize),
    /// is_writable := false
    Ro(usize),
    /// is_writable := true
    Rw(usize),
    /// is_signer := false
    NoSign(usize),
    /// is_signer := true
    Sign(usize),
    /// slot i takes the key of slot j (a duplicate in two slots)
    Dup(usize, usize),
    /// slots i and j trade keys (flags stay with the position)
    Swap(usize, usize),
    /// a program slot gets a random non-executable key
    FakeProg(usize),
    /// a program slot gets another EXECUTABLE program (the vault)
    OtherExec(usize),
    /// only the first k accounts are passed
    Trunc(usize),
    /// one more read-only account is appended
    Extra,
    /// the trading account slot takes ANOTHER trader's valid account
    OtherTa(usize),
}

const FOREIGN: Pubkey = Pubkey::new_from_array([0xF0; 32]);

/// Replaces `acc`'s stored bump through the typed state.
fn flip_bump(data: &mut Vec<u8>, ty: Ty) -> bool {
    macro_rules! flip {
        ($t:ty) => {{
            let mut v = match <$t>::try_deserialize(&mut data.as_slice()) {
                Ok(v) => v,
                Err(_) => return false,
            };
            v.bump ^= 1;
            let mut out = Vec::new();
            v.try_serialize(&mut out).unwrap();
            *data = out;
            true
        }};
    }
    match ty {
        Ty::Cfg(_) => flip!(SectorConfig),
        Ty::Ta { .. } => flip!(TradingAccount),
        Ty::Wal => flip!(WalletProfile),
        Ty::RsSeed | Ty::RsFree => flip!(RuleSet),
        Ty::Mkt | Ty::Rem => flip!(Market),
        _ => false,
    }
}

/// A non-canonical but valid (off-curve) PDA for `seeds`: the largest valid bump is canonical, any smaller valid one is not.
fn non_canonical(seeds: &[&[u8]]) -> Option<(Pubkey, u8)> {
    let (_, canon) = Pubkey::find_program_address(seeds, &PROGRAM_ID);
    for b in (0..canon).rev() {
        let mut s: Vec<&[u8]> = seeds.to_vec();
        let bb = [b];
        s.push(&bb);
        if let Ok(k) = Pubkey::create_program_address(&s, &PROGRAM_ID) {
            return Some((k, b));
        }
    }
    None
}

/// Seeds (without bump) of the PDA that sits in a slot of type `ty` in this scenario, read from the account's own data.
fn seeds_of(ty: Ty, data: &[u8]) -> Option<Vec<Vec<u8>>> {
    match ty {
        Ty::Cfg(_) => Some(vec![c::CONFIG_SEED.to_vec()]),
        Ty::Ta { .. } => {
            let a = TradingAccount::try_deserialize(&mut &data[..]).ok()?;
            Some(vec![c::ACCT_SEED.to_vec(), a.trader.as_ref().to_vec(), a.challenge_id.to_le_bytes().to_vec()])
        }
        Ty::Wal => {
            let a = WalletProfile::try_deserialize(&mut &data[..]).ok()?;
            Some(vec![c::WALLET_SEED.to_vec(), a.trader.as_ref().to_vec()])
        }
        Ty::RsSeed | Ty::RsFree => {
            let a = RuleSet::try_deserialize(&mut &data[..]).ok()?;
            Some(vec![c::RULESET_SEED.to_vec(), a.version.to_le_bytes().to_vec()])
        }
        _ => None,
    }
}

fn set_bump(data: &mut Vec<u8>, ty: Ty, bump: u8) {
    macro_rules! setb {
        ($t:ty) => {{
            let mut v = <$t>::try_deserialize(&mut data.as_slice()).unwrap();
            v.bump = bump;
            let mut out = Vec::new();
            v.try_serialize(&mut out).unwrap();
            *data = out;
        }};
    }
    match ty {
        Ty::Cfg(_) => setb!(SectorConfig),
        Ty::Ta { .. } => setb!(TradingAccount),
        Ty::Wal => setb!(WalletProfile),
        Ty::RsSeed | Ty::RsFree => setb!(RuleSet),
        _ => unreachable!(),
    }
}

/// Can mutation `mu` be applied to this scenario at all (the slot has an account, the type has a bump, ...)?
fn applicable(s: &Scn, mu: Mu) -> bool {
    let n = s.n();
    let acct_at = |i: usize| s.world.envr().svm.get_account(&s.ix.accounts[i].pubkey);
    let ty_at = |i: usize| s.tys.get(i).copied();
    match mu {
        Mu::BadDisc(i) => {
            i < n && acct_at(i).is_some_and(|a| !a.data.is_empty() && !a.executable) && !matches!(ty_at(i), Some(Ty::SigW | Ty::Sig | Ty::SigPin | Ty::Mint | Ty::TraderTok | Ty::FeeTok | Ty::Pool | Ty::Sl8))
        }
        Mu::Foreign(i) | Mu::LookSame(i) | Mu::LookForeign(i) | Mu::BelowRent(i) | Mu::Short(i) => {
            i < n && acct_at(i).is_some_and(|a| !a.data.is_empty() && !a.executable) && !matches!(ty_at(i), Some(Ty::SigW | Ty::Sig | Ty::SigPin))
        }
        Mu::Closed(i) | Mu::Funded(i) => {
            i < n && acct_at(i).is_some_and(|a| !a.executable) && !matches!(ty_at(i), Some(Ty::SelfProg | Ty::Sig | Ty::SigW))
        }
        Mu::Missing(i) => i < n && !matches!(ty_at(i), Some(Ty::Sys | Ty::Tok | Ty::SelfProg | Ty::Sig | Ty::SigW)),
        Mu::BumpFlip(i) => i < n && ty_at(i).is_some() && acct_at(i).is_some_and(|a| { let mut d = a.data.clone(); flip_bump(&mut d, ty_at(i).unwrap()) }),
        Mu::NonCanon(i) => {
            i < n && ty_at(i).is_some() && acct_at(i).is_some_and(|a| seeds_of(ty_at(i).unwrap(), &a.data).is_some_and(|sd| {
                let r: Vec<&[u8]> = sd.iter().map(|v| v.as_slice()).collect();
                non_canonical(&r).is_some()
            })) || (i < n && ty_at(i) == Some(Ty::SecAuth) && non_canonical(&[c::SECTOR_AUTHORITY_SEED]).is_some())
        }
        Mu::Ro(i) => i < n && s.ix.accounts[i].is_writable,
        Mu::Rw(i) => i < n && !s.ix.accounts[i].is_writable,
        Mu::NoSign(i) => i < n && s.ix.accounts[i].is_signer,
        Mu::Sign(i) => i < n && !s.ix.accounts[i].is_signer,
        Mu::Dup(i, j) => i < n && j < n && i != j && s.ix.accounts[i].pubkey != s.ix.accounts[j].pubkey,
        Mu::Swap(i, j) => i < j && j < n && s.ix.accounts[i].pubkey != s.ix.accounts[j].pubkey,
        Mu::FakeProg(i) | Mu::OtherExec(i) => i < n && matches!(ty_at(i), Some(Ty::Sys | Ty::Tok)),
        Mu::Trunc(k) => k < n,
        Mu::Extra => true,
        Mu::OtherTa(i) => i < n && matches!(ty_at(i), Some(Ty::Ta { .. })),
    }
}

/// Applies `mu` to the scenario (world and instruction). Returns a human note.
fn apply(s: &mut Scn, mu: Mu) {
    let rent = |s: &Scn, len: usize| s.world.envr().svm.minimum_balance_for_rent_exemption(len);
    match mu {
        Mu::Foreign(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            a.owner = FOREIGN;
            s.world.env().svm.set_account(k, a).unwrap();
        }
        Mu::LookSame(i) | Mu::LookForeign(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            if matches!(mu, Mu::LookForeign(_)) {
                a.owner = FOREIGN;
            }
            let x = Pubkey::new_unique();
            s.world.env().svm.set_account(x, a).unwrap();
            s.ix.accounts[i].pubkey = x;
        }
        Mu::Closed(i) => {
            let k = s.ix.accounts[i].pubkey;
            s.world.env().svm.set_account(k, RawAccount::default()).unwrap();
        }
        Mu::Funded(i) => {
            let k = s.ix.accounts[i].pubkey;
            let l = rent(s, 0).max(1_000_000);
            s.world.env().svm.set_account(k, RawAccount { lamports: l, data: vec![], owner: system_program::ID, executable: false, rent_epoch: 0 }).unwrap();
        }
        Mu::Missing(i) => {
            s.ix.accounts[i].pubkey = Pubkey::new_unique();
        }
        Mu::BelowRent(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            a.lamports = rent(s, a.data.len()) - 1;
            s.world.env().svm.set_account(k, a).unwrap();
        }
        Mu::BadDisc(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            a.data[0] ^= 1;
            s.world.env().svm.set_account(k, a).unwrap();
        }
        Mu::Short(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            a.data.truncate(4);
            s.world.env().svm.set_account(k, a).unwrap();
        }
        Mu::BumpFlip(i) => {
            let k = s.ix.accounts[i].pubkey;
            let mut a = s.world.envr().svm.get_account(&k).unwrap();
            assert!(flip_bump(&mut a.data, s.tys[i]));
            s.world.env().svm.set_account(k, a).unwrap();
        }
        Mu::NonCanon(i) => {
            let k = s.ix.accounts[i].pubkey;
            if s.tys[i] == Ty::SecAuth {
                let (x, _) = non_canonical(&[c::SECTOR_AUTHORITY_SEED]).unwrap();
                s.ix.accounts[i].pubkey = x;
            } else {
                let mut a = s.world.envr().svm.get_account(&k).unwrap();
                let sd = seeds_of(s.tys[i], &a.data).unwrap();
                let r: Vec<&[u8]> = sd.iter().map(|v| v.as_slice()).collect();
                let (x, b) = non_canonical(&r).unwrap();
                set_bump(&mut a.data, s.tys[i], b);
                s.world.env().svm.set_account(x, a).unwrap();
                s.ix.accounts[i].pubkey = x;
            }
        }
        // a flag belongs to the ACCOUNT in a transaction (the message merges the flags of every slot with the same key), so the
        // flag is changed in every slot that holds the same key
        Mu::Ro(i) | Mu::Rw(i) | Mu::NoSign(i) | Mu::Sign(i) => {
            let key = s.ix.accounts[i].pubkey;
            for m in s.ix.accounts.iter_mut().filter(|m| m.pubkey == key) {
                match mu {
                    Mu::Ro(_) => m.is_writable = false,
                    Mu::Rw(_) => m.is_writable = true,
                    Mu::NoSign(_) => m.is_signer = false,
                    _ => m.is_signer = true,
                }
            }
        }
        Mu::Dup(i, j) => {
            let kj = s.ix.accounts[j].pubkey;
            s.ix.accounts[i].pubkey = kj;
        }
        Mu::Swap(i, j) => {
            let (ki, kj) = (s.ix.accounts[i].pubkey, s.ix.accounts[j].pubkey);
            s.ix.accounts[i].pubkey = kj;
            s.ix.accounts[j].pubkey = ki;
        }
        Mu::FakeProg(i) => s.ix.accounts[i].pubkey = Pubkey::new_unique(),
        Mu::OtherExec(i) => s.ix.accounts[i].pubkey = VAULT_ID,
        Mu::Trunc(k) => s.ix.accounts.truncate(k),
        Mu::Extra => s.ix.accounts.push(AccountMeta::new_readonly(Pubkey::new_unique(), false)),
        Mu::OtherTa(i) => {
            // a second trader with his own wallet and live account; his account takes the slot
            let w = match &mut s.world {
                World::J(w) => w,
                World::P(_) => panic!("no vault world"),
            };
            let t2 = w.v.new_trader();
            common::assert_ok(w.v.open(&t2, 0));
            s.ix.accounts[i].pubkey = acct_pda(&t2.pubkey(), 0);
        }
    }
}

// ====================================================================================================== the model (what Anchor 0.32 and the vault do)

/// The error of a row: `Ok` or exactly this `InstructionError`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Exp {
    Ok,
    Err(InstructionError),
}

fn cu(code: u32) -> Exp {
    Exp::Err(InstructionError::Custom(code))
}
fn pe(e: E) -> Exp {
    cu(u32::from(e))
}

// Anchor framework codes (anchor-lang 0.32 `ErrorCode`)
const A_CONSTRAINT_MUT: u32 = 2000;
const A_CONSTRAINT_OWNER: u32 = 2004;
const A_CONSTRAINT_RENT_EXEMPT: u32 = 2005;
const A_CONSTRAINT_SEEDS: u32 = 2006;
const A_DISC_NOT_FOUND: u32 = 3001;
const A_DISC_MISMATCH: u32 = 3002;
const A_NOT_ENOUGH_KEYS: u32 = 3005;
const A_WRONG_OWNER: u32 = 3007;
const A_INVALID_PROGRAM_ID: u32 = 3008;
const A_NOT_SIGNER: u32 = 3010;
const A_NOT_INITIALIZED: u32 = 3012;
// the vault's own codes (6000 + index in `VaultError`)
const V_INVALID_TOKEN_ACCOUNT: u32 = 6021;

/// A failure with its position in the order Anchor and the handlers evaluate: (phase, sub-phase, slot).
/// phase 0: deserialisation of every slot in order. phase 1: constraints (sub 0: the `init` fields first, sub 1: all others in field
/// order). phase 2: the handler (sub = order inside the handler). phase 3: the CPI into the vault / token program.
type Hit = ((u8, u8, usize), Exp);

fn anchor_disc(name: &str) -> [u8; 8] {
    let h = Sha256::digest(format!("account:{name}").as_bytes());
    let mut d = [0u8; 8];
    d.copy_from_slice(&h[..8]);
    d
}

/// `Account<T>::try_from` for an account whose owner is `owner` and whose discriminator is `disc`.
fn anchor_deser(a: &Option<RawAccount>, owner: &Pubkey, disc: &[u8; 8]) -> Option<Exp> {
    match a {
        None => Some(cu(A_NOT_INITIALIZED)),
        Some(a) if a.owner == system_program::ID && a.lamports == 0 => Some(cu(A_NOT_INITIALIZED)),
        Some(a) if a.owner != *owner => Some(cu(A_WRONG_OWNER)),
        Some(a) if a.data.len() < 8 => Some(cu(A_DISC_NOT_FOUND)),
        Some(a) if a.data[..8] != disc[..] => Some(cu(A_DISC_MISMATCH)),
        Some(_) => None,
    }
}

/// `Account<Mint>` / `Account<TokenAccount>` of anchor-spl: owner is the token program, the SPL `unpack` needs the exact length.
fn spl_deser(a: &Option<RawAccount>, len: usize) -> Option<Exp> {
    use anchor_spl::token::spl_token;
    match a {
        None => Some(cu(A_NOT_INITIALIZED)),
        Some(a) if a.owner == system_program::ID && a.lamports == 0 => Some(cu(A_NOT_INITIALIZED)),
        Some(a) if a.owner != spl_token::ID => Some(cu(A_WRONG_OWNER)),
        Some(a) if a.data.len() != len => Some(Exp::Err(InstructionError::InvalidAccountData)),
        Some(_) => None,
    }
}

fn spl_token_fields(a: &RawAccount) -> (Pubkey, Pubkey) {
    // SPL token account layout: mint 0..32, owner 32..64
    (Pubkey::new_from_array(a.data[..32].try_into().unwrap()), Pubkey::new_from_array(a.data[32..64].try_into().unwrap()))
}

/// The instruction as the runtime hands it to the program, plus the legitimate scenario for reference.
struct Mx<'a> {
    o: &'a Scn,
    m: &'a Scn,
    keys: Vec<Pubkey>,
    acc: Vec<Option<RawAccount>>,
    sig: Vec<bool>,
    wr: Vec<bool>,
    cfg: Option<SectorConfig>,
}

impl<'a> Mx<'a> {
    fn new(o: &'a Scn, m: &'a Scn) -> Mx<'a> {
        let keys: Vec<Pubkey> = m.ix.accounts.iter().map(|a| a.pubkey).collect();
        let e = m.world.envr();
        let acc: Vec<Option<RawAccount>> = keys.iter().map(|k| e.svm.get_account(k).filter(|a| a.lamports > 0)).collect();
        // the message merges the flags of every slot that holds the same key; program ids are never writable
        let sig: Vec<bool> = keys.iter().map(|k| m.ix.accounts.iter().any(|a| a.pubkey == *k && a.is_signer)).collect();
        let wr: Vec<bool> = keys
            .iter()
            .map(|k| m.ix.accounts.iter().any(|a| a.pubkey == *k && a.is_writable) && *k != PROGRAM_ID)
            .collect();
        let cfg = o.world.envr().svm.get_account(&config_pda()).and_then(|a| SectorConfig::try_deserialize(&mut a.data.as_slice()).ok());
        Mx { o, m, keys, acc, sig, wr, cfg }
    }

    fn kx(&self, i: usize) -> Pubkey {
        self.keys.get(i).copied().unwrap_or_default()
    }
    /// What the vault sees in slot `i` when it runs (after this program's own `init` of the new trading account has already created
    /// the account at its address): a key that equals the new trading account's key is a program-owned TradingAccount by then.
    fn vault_view(&self, i: usize) -> Option<RawAccount> {
        let created = self.m.tys.iter().enumerate().any(|(j, t)| *t == Ty::InitTa && j != i && j < self.keys.len() && self.keys[j] == self.keys[i]);
        if created && self.acc[i].is_none() {
            let mut data = anchor_disc("TradingAccount").to_vec();
            data.extend_from_slice(&[0u8; 64]);
            return Some(RawAccount { lamports: 1, data, owner: PROGRAM_ID, executable: false, rent_epoch: 0 });
        }
        self.acc[i].clone()
    }
    fn legit(&self, i: usize) -> Pubkey {
        self.o.ix.accounts[i].pubkey
    }
    fn first(&self, f: impl Fn(Ty) -> bool) -> Option<usize> {
        self.m.tys.iter().position(|t| f(*t))
    }
    fn all(&self, f: impl Fn(Ty) -> bool) -> Vec<usize> {
        self.m.tys.iter().enumerate().filter(|(_, t)| f(**t)).map(|(i, _)| i).collect()
    }
    fn ta_data(&self) -> Option<TradingAccount> {
        let i = self.first(|t| matches!(t, Ty::Ta { .. }))?;
        let a = self.acc[i].as_ref()?;
        TradingAccount::try_deserialize(&mut a.data.as_slice()).ok()
    }
    /// The trader whose wallet / account address the seeds name.
    fn trader_for_seeds(&self) -> Pubkey {
        match self.m.name {
            // the trader slot itself is in the seeds
            "open_account" | "reset_account" => self.kx(0),
            "unban_wallet" => self.o.world.j().trader.pubkey(),
            _ => self.ta_data().map(|t| t.trader).unwrap_or_else(|| self.o.world.j().trader.pubkey()),
        }
    }
    fn pda_ok(&self, seeds: &[&[u8]], bump: u8, key: &Pubkey) -> bool {
        let b = [bump];
        let mut s: Vec<&[u8]> = seeds.to_vec();
        s.push(&b);
        Pubkey::create_program_address(&s, &PROGRAM_ID).is_ok_and(|k| k == *key)
    }
    /// Admin-slot rule of the scenario: (indexes of the two admin slots, both needed?)
    fn admin_rule(&self) -> Option<(usize, usize, bool)> {
        let adm = self.all(|t| t == Ty::Adm);
        if adm.len() != 2 {
            return None;
        }
        let both = !matches!(self.m.name, "create_market" | "update_market" | "set_paused");
        Some((adm[0], adm[1], both))
    }
    /// The error of an `init` CPI (system `create_account` paid by the payer slot) for the account in slot `init_i`.
    fn init_cpi(&self, payer: usize, init_i: usize) -> Option<Exp> {
        if payer >= self.keys.len() {
            // the payer slot was cut off: the 3005 of that slot comes first
            return None;
        }
        if !self.wr[payer] || !self.wr[init_i] {
            return Some(Exp::Err(InstructionError::PrivilegeEscalation));
        }
        if self.keys[payer] == self.keys[init_i] {
            // the same account as source and destination of create_account
            return Some(Exp::Err(InstructionError::InvalidArgument));
        }
        match &self.acc[payer] {
            None => Some(cu(1)),
            Some(a) if !a.data.is_empty() || a.owner != system_program::ID || a.executable => Some(Exp::Err(InstructionError::InvalidArgument)),
            Some(_) => None,
        }
    }
    fn payer_slot(&self) -> Option<usize> {
        match self.m.name {
            "init_config" | "create_ruleset" | "create_market" | "ban_wallet" => Some(0),
            "open_account" | "reset_account" => Some(1),
            _ => None,
        }
    }

    fn hits(&self) -> Vec<Hit> {
        let mut out: Vec<Hit> = Vec::new();
        let nt = self.m.tys.len();
        let n = self.keys.len();
        for i in 0..nt {
            if i >= n {
                if self.m.tys[i] != Ty::Rem {
                    out.push(((0, 0, i), cu(A_NOT_ENOUGH_KEYS)));
                }
                continue;
            }
            self.slot_hits(i, &mut out);
        }
        // remaining accounts that are not typed slots (an appended extra account): post_journal and report_fail read them as markets
        for i in nt..n {
            if matches!(self.m.name, "post_journal" | "report_fail") {
                out.push(((2, 3, i), pe(E::MarketUnknown)));
            }
        }
        // finalize_pass: the vault creates the claim account paid by the payer slot (system create_account inside the vault)
        if self.m.name == "finalize_pass" && n > 0 {
            if let Some(claim) = self.first(|t| t == Ty::Claim).filter(|c| *c < n) {
                if let Some(e) = self.init_cpi(0, claim) {
                    out.push(((3, 9, 0), e));
                }
            }
        }
        // report_fail: the Fail event's proof names a market feed, and the market account is not there
        if self.m.name == "report_fail" {
            if let Some(r) = self.first(|t| t == Ty::Rem) {
                if r >= n {
                    out.push(((2, 4, r), pe(E::ProofWrongFeed)));
                }
            }
        }
        self.handler_hits(&mut out);
        out
    }

    fn handler_hits(&self, out: &mut Vec<Hit>) {
        let Some(cfg) = &self.cfg else { return };
        let n = self.keys.len();
        // evaluator instructions: slot 0 must be config.evaluator
        if matches!(self.m.name, "post_journal" | "report_fail" | "confirm_pass" | "reject_pass" | "hold_account" | "touch_activity") && n > 0 && self.kx(0) != cfg.evaluator {
            out.push(((2, 0, 0), pe(E::Unauthorized)));
        }
        // buyer instructions: payer must be the trader
        if matches!(self.m.name, "open_account" | "reset_account") && n > 1 && self.kx(0) != self.keys[1] {
            out.push(((2, 0, 1), pe(E::Unauthorized)));
        }
        // admin instructions
        if let Some((a, b, both)) = self.admin_rule() {
            if a < n && b < n {
                let a_ok = self.sig[a] && self.keys[a] == cfg.admin_a;
                let b_ok = self.sig[b] && self.keys[b] == cfg.admin_b;
                let ok = if both { a_ok && b_ok } else { a_ok || b_ok };
                if !ok {
                    out.push(((2, 0, a), pe(E::Unauthorized)));
                }
            }
        }
        // another trader's account in the trading account slot: the handler's own state check (evaluator instructions without a wallet slot)
        if let Some(i) = self.first(|t| matches!(t, Ty::Ta { .. })) {
            let legit_id = self.o.world.envr().svm.get_account(&self.legit(i)).and_then(|a| TradingAccount::try_deserialize(&mut a.data.as_slice()).ok()).map(|t| (t.trader, t.challenge_id));
            let here = self.acc.get(i).and_then(|a| a.as_ref()).and_then(|a| TradingAccount::try_deserialize(&mut a.data.as_slice()).ok()).map(|t| (t.trader, t.challenge_id));
            if i < n && here.is_some() && here != legit_id {
                match self.m.name {
                    "post_journal" => out.push(((2, 2, i), pe(E::BadPrevHash))),
                    "confirm_pass" | "reject_pass" | "veto_pass" => out.push(((2, 2, i), pe(E::NoPendingPass))),
                    _ => {}
                }
            }
        }
    }

    fn slot_hits(&self, i: usize, out: &mut Vec<Hit>) {
        use anchor_spl::token::spl_token;
        let ty = self.m.tys[i];
        let key = self.keys[i];
        let a = &self.acc[i];
        let legit = self.legit(i);
        let mut push = |rank: (u8, u8, usize), e: Exp| out.push((rank, e));
        let mutchk = |wr: bool, push: &mut dyn FnMut((u8, u8, usize), Exp)| {
            if !wr {
                push((1, 1, i), cu(A_CONSTRAINT_MUT));
            }
        };
        let vault_deser = |disc: &[u8; 8], push: &mut dyn FnMut((u8, u8, usize), Exp)| -> bool {
            if let Some(e) = anchor_deser(&self.vault_view(i), &VAULT_ID, disc) {
                push((3, 0, i), e);
                return false;
            }
            true
        };
        let vault_seeds = |push: &mut dyn FnMut((u8, u8, usize), Exp)| {
            if key != legit {
                push((3, 1, i), cu(A_CONSTRAINT_SEEDS));
            }
        };
        match ty {
            Ty::SigW | Ty::Sig => {
                if !self.sig[i] {
                    push((0, 0, i), cu(A_NOT_SIGNER));
                }
                if ty == Ty::SigW {
                    mutchk(self.wr[i], &mut push);
                }
            }
            Ty::SigPin => {
                if !self.sig[i] {
                    push((0, 0, i), cu(A_NOT_SIGNER));
                }
                if key != legit {
                    push((1, 1, i), pe(E::Unauthorized));
                }
            }
            Ty::Cfg(_) | Ty::Ta { .. } | Ty::Wal | Ty::RsSeed | Ty::Mkt => {
                let w = match ty {
                    Ty::Cfg(w) | Ty::Ta { w, .. } => w,
                    Ty::Wal | Ty::Mkt => true,
                    _ => false,
                };
                let disc = match ty {
                    Ty::Cfg(_) => anchor_disc("SectorConfig"),
                    Ty::Ta { .. } => anchor_disc("TradingAccount"),
                    Ty::Wal => anchor_disc("WalletProfile"),
                    Ty::RsSeed => anchor_disc("RuleSet"),
                    _ => anchor_disc("Market"),
                };
                if let Some(e) = anchor_deser(a, &PROGRAM_ID, &disc) {
                    push((0, 0, i), e);
                    return;
                }
                let data = &a.as_ref().unwrap().data;
                let seeds_ok = match ty {
                    Ty::Cfg(_) => SectorConfig::try_deserialize(&mut data.as_slice()).is_ok_and(|v| self.pda_ok(&[c::CONFIG_SEED], v.bump, &key)),
                    Ty::Ta { .. } => TradingAccount::try_deserialize(&mut data.as_slice())
                        .is_ok_and(|v| self.pda_ok(&[c::ACCT_SEED, v.trader.as_ref(), &v.challenge_id.to_le_bytes()], v.bump, &key)),
                    Ty::Wal => {
                        let tr = self.trader_for_seeds();
                        WalletProfile::try_deserialize(&mut data.as_slice()).is_ok_and(|v| self.pda_ok(&[c::WALLET_SEED, tr.as_ref()], v.bump, &key))
                    }
                    Ty::RsSeed => {
                        let ver = self.ta_data().map_or(1, |t| t.ruleset_version);
                        RuleSet::try_deserialize(&mut data.as_slice()).is_ok_and(|v| self.pda_ok(&[c::RULESET_SEED, &ver.to_le_bytes()], v.bump, &key))
                    }
                    _ => {
                        let id = Market::try_deserialize(&mut self.o.world.envr().svm.get_account(&legit).unwrap().data.as_slice()).unwrap().market_id;
                        Market::try_deserialize(&mut data.as_slice()).is_ok_and(|v| self.pda_ok(&[c::MARKET_SEED, &id.to_le_bytes()], v.bump, &key))
                    }
                };
                if !seeds_ok {
                    push((1, 1, i), cu(A_CONSTRAINT_SEEDS));
                    return;
                }
                if w {
                    mutchk(self.wr[i], &mut push);
                }
                if let Ty::Ta { tc: Some(t), .. } = ty {
                    let tr = TradingAccount::try_deserialize(&mut data.as_slice()).map(|v| v.trader).unwrap_or_default();
                    if tr != self.kx(usize::from(t)) {
                        push((1, 1, i), pe(E::Unauthorized));
                    }
                }
                if ty == Ty::RsSeed {
                    let ver = self.ta_data().map_or(1, |t| t.ruleset_version);
                    if RuleSet::try_deserialize(&mut data.as_slice()).is_ok_and(|v| v.version != ver) {
                        push((1, 1, i), pe(E::RuleSetMissing));
                    }
                }
            }
            Ty::RsFree => {
                if let Some(e) = anchor_deser(a, &PROGRAM_ID, &anchor_disc("RuleSet")) {
                    push((0, 0, i), e);
                    return;
                }
                let rs = RuleSet::try_deserialize(&mut a.as_ref().unwrap().data.as_slice()).unwrap();
                let cur = self.cfg.as_ref().map_or(0, |c| c.current_ruleset);
                if !(cur != 0 && rs.version == cur && self.pda_ok(&[c::RULESET_SEED, &rs.version.to_le_bytes()], rs.bump, &key)) {
                    push((2, 1, i), pe(E::RuleSetMissing));
                }
            }
            Ty::Adm | Ty::SelfProg => {}
            Ty::Rem => {
                let bad = match a {
                    Some(acc) if acc.owner == PROGRAM_ID => match Market::try_deserialize(&mut acc.data.as_slice()) {
                        Ok(m) => !self.pda_ok(&[c::MARKET_SEED, &m.market_id.to_le_bytes()], m.bump, &key),
                        Err(_) => true,
                    },
                    _ => true,
                };
                if bad {
                    push((2, 3, i), pe(E::MarketUnknown));
                }
            }
            Ty::SecAuth => {
                if key != legit {
                    push((1, 1, i), cu(A_CONSTRAINT_SEEDS));
                }
            }
            Ty::EvAuth => {
                if key != ev_auth() {
                    push((1, 1, i), cu(A_CONSTRAINT_SEEDS));
                }
            }
            Ty::VProg => {
                if self.cfg.as_ref().is_some_and(|c| c.vault_program != key) {
                    push((1, 1, i), pe(E::InvalidVaultProgram));
                }
            }
            Ty::Sys | Ty::Tok => {
                let want = if ty == Ty::Sys { system_program::ID } else { spl_token::ID };
                if key != want {
                    push((0, 0, i), cu(A_INVALID_PROGRAM_ID));
                }
            }
            Ty::VReg(_) | Ty::VRegRd => {
                if matches!(ty, Ty::VReg(true)) || ty == Ty::VRegRd {
                    mutchk(self.wr[i], &mut push);
                }
                if ty == Ty::VRegRd {
                    let good = key == legit
                        && a.as_ref().is_some_and(|x| x.owner == VAULT_ID && x.data.len() >= 44 && x.data[..8] == anchor_disc("ProductRegistry")[..] && x.data[8..40] == PROGRAM_ID.to_bytes()[..]);
                    if !good {
                        push((2, 1, i), pe(E::InvalidVaultAccount));
                    }
                }
                if vault_deser(&anchor_disc("ProductRegistry"), &mut push) {
                    vault_seeds(&mut push);
                }
            }
            Ty::Vts | Ty::VtsRd | Ty::VtsPrev => {
                mutchk(self.wr[i], &mut push);
                if ty == Ty::VtsRd {
                    // status byte at offset 96 is read before the CPI
                    let good = key == legit && a.as_ref().is_some_and(|x| x.owner == VAULT_ID && x.data.len() > c::VAULT_STATUS_OFFSET);
                    if !good {
                        push((2, 1, i), pe(E::InvalidVaultAccount));
                    }
                }
                if ty == Ty::VtsPrev {
                    let good = key == legit
                        && a.as_ref().is_some_and(|x| x.owner == VAULT_ID && x.data.len() >= 97 && x.data[..8] == anchor_disc("TraderState")[..]);
                    if !good {
                        push((2, 2, i), pe(E::InvalidVaultAccount));
                    }
                }
                if vault_deser(&anchor_disc("TraderState"), &mut push) {
                    vault_seeds(&mut push);
                }
            }
            Ty::VtsNew => {
                mutchk(self.wr[i], &mut push);
                // created by the vault's `init`: the address must be the seeds' address
                vault_seeds(&mut push);
            }
            Ty::VState(w) => {
                if w {
                    mutchk(self.wr[i], &mut push);
                }
                if vault_deser(&anchor_disc("VaultState"), &mut push) {
                    vault_seeds(&mut push);
                }
            }
            Ty::Claim => {
                mutchk(self.wr[i], &mut push);
                vault_seeds(&mut push);
            }
            Ty::Tally => {
                if key != legit {
                    push((1, 1, i), cu(A_CONSTRAINT_SEEDS));
                    return;
                }
                mutchk(self.wr[i], &mut push);
                if a.as_ref().is_none_or(|x| x.owner != PROGRAM_ID) {
                    push((1, 1, i), cu(A_CONSTRAINT_OWNER));
                } else if a.as_ref().is_some_and(|x| setl8_shared_interfaces::PayoutTally::parse(&x.data).is_err()) {
                    // the layout is owned by setl8-shared-interfaces (an independent crate): it decides whether the bytes are a tally
                    push((2, 5, i), pe(E::PayoutTallyInvalid));
                }
            }
            Ty::Pool | Ty::Sl8 => {
                mutchk(self.wr[i], &mut push);
                if let Some(e) = spl_deser(&self.vault_view(i), 165) {
                    push((3, 0, i), e);
                    return;
                }
                if ty == Ty::Pool {
                    if key != legit {
                        push((3, 1, i), cu(V_INVALID_TOKEN_ACCOUNT));
                    }
                } else {
                    let mint_i = self.first(|t| t == Ty::Mint).unwrap_or(0);
                    let (mint, owner) = spl_token_fields(a.as_ref().unwrap());
                    if owner != VAULT_SL8_ADMIN || mint != self.kx(mint_i) {
                        push((3, 1, i), cu(V_INVALID_TOKEN_ACCOUNT));
                    }
                }
            }
            Ty::Mint => {
                if let Some(e) = spl_deser(a, 82) {
                    push((0, 0, i), e);
                }
            }
            Ty::TraderTok | Ty::FeeTok => {
                if let Some(e) = spl_deser(a, 165) {
                    push((0, 0, i), e);
                    return;
                }
                mutchk(self.wr[i], &mut push);
                let mint_i = self.first(|t| t == Ty::Mint).unwrap_or(0);
                let (mint, owner) = spl_token_fields(a.as_ref().unwrap());
                let want_owner = if ty == Ty::TraderTok { self.kx(0) } else { self.cfg.as_ref().map(|c| c.sl8_wallet).unwrap_or_default() };
                if !(owner == want_owner && mint == self.kx(mint_i)) {
                    push((1, 1, i), pe(E::InvalidTokenAccount));
                }
            }
            Ty::InitCfg | Ty::InitRs | Ty::InitMkt => {
                // `init`: the address must be the seeds' address (the scenario's own key), then the system CPI
                if key != legit {
                    push((1, 0, i), cu(A_CONSTRAINT_SEEDS));
                } else if let Some(e) = self.payer_slot().and_then(|p| self.init_cpi(p, i)) {
                    push((1, 0, i), e);
                }
            }
            Ty::InitTally => {
                if key != legit {
                    push((1, 1, i), cu(A_CONSTRAINT_SEEDS));
                    return;
                }
                mutchk(self.wr[i], &mut push);
            }
            Ty::InitTa => {
                // seeds [acct, trader slot, wallet.next_challenge_id]
                let wal_i = self.first(|t| matches!(t, Ty::Wal | Ty::InitWal));
                let next_id = wal_i
                    .and_then(|w| self.acc[w].as_ref())
                    .filter(|x| x.owner == PROGRAM_ID && x.data.len() > 8)
                    .and_then(|x| WalletProfile::try_deserialize(&mut x.data.as_slice()).ok())
                    .map_or(0, |w| w.next_challenge_id);
                let want = acct_pda(&self.kx(0), next_id);
                if key != want {
                    push((1, 0, i), cu(A_CONSTRAINT_SEEDS));
                } else if let Some(e) = self.payer_slot().and_then(|p| self.init_cpi(p, i)) {
                    push((1, 0, i), e);
                }
            }
            Ty::InitWal => {
                let trader = if self.m.name == "ban_wallet" { self.o.world.j().trader.pubkey() } else { self.kx(0) };
                let victim = if self.m.name == "ban_wallet" { self.o.ix.accounts[i].pubkey } else { wallet_pda(&trader) };
                let want = if self.m.name == "ban_wallet" { victim } else { wallet_pda(&trader) };
                let fresh = a.as_ref().is_none_or(|x| x.owner == system_program::ID);
                if fresh {
                    // init path: a new, zeroed profile at the seeds' address
                    if key != want {
                        push((1, 0, i), cu(A_CONSTRAINT_SEEDS));
                    } else if let Some(e) = self.payer_slot().and_then(|p| self.init_cpi(p, i)) {
                        push((1, 0, i), e);
                    }
                } else {
                    // existing profile: seeds first, then Account<WalletProfile> deserialisation, rent exemption, mut
                    if key != want {
                        push((1, 0, i), cu(A_CONSTRAINT_SEEDS));
                        return;
                    }
                    if let Some(e) = anchor_deser(a, &PROGRAM_ID, &anchor_disc("WalletProfile")) {
                        push((1, 0, i), e);
                        return;
                    }
                    let x = a.as_ref().unwrap();
                    if x.lamports < self.m.world.envr().svm.minimum_balance_for_rent_exemption(x.data.len()) {
                        push((1, 0, i), cu(A_CONSTRAINT_RENT_EXEMPT));
                        return;
                    }
                    if !self.wr[i] {
                        push((1, 0, i), cu(A_CONSTRAINT_MUT));
                    }
                }
            }
        }
    }
}

/// The expected result of the mutated scenario `m` (the legitimate one is `o`).
fn expect(o: &Scn, m: &Scn) -> Exp {
    let mx = Mx::new(o, m);
    let mut hits = mx.hits();
    hits.sort_by_key(|h| h.0);
    match hits.into_iter().next() {
        Some((_, e)) => e,
        None => Exp::Ok,
    }
}

// ====================================================================================================== the row generator

fn index_of(s: &Scn, ty_pred: impl Fn(Ty) -> bool) -> Option<usize> {
    s.tys.iter().position(|t| ty_pred(*t))
}

/// Every mutation that applies to the scenario (all pairs for Dup, adjacent pairs for Swap).
fn all_mutations(s: &Scn, full_pairs: bool) -> Vec<Mu> {
    let n = s.n();
    let mut v = Vec::new();
    for i in 0..n {
        for mu in [
            Mu::Foreign(i), Mu::LookSame(i), Mu::LookForeign(i), Mu::Closed(i), Mu::Funded(i), Mu::Missing(i), Mu::BelowRent(i), Mu::BadDisc(i), Mu::Short(i),
            Mu::BumpFlip(i), Mu::NonCanon(i), Mu::Ro(i), Mu::Rw(i), Mu::NoSign(i), Mu::Sign(i), Mu::FakeProg(i), Mu::OtherExec(i), Mu::OtherTa(i),
        ] {
            v.push(mu);
        }
        for j in 0..n {
            if i != j && (full_pairs || j == 0 || j + 1 == i || j == i + 1 || Some(j) == index_of(s, |t| matches!(t, Ty::Ta { .. })) || Some(j) == index_of(s, |t| matches!(t, Ty::Cfg(_)))) {
                v.push(Mu::Dup(i, j));
            }
        }
        if i + 1 < n {
            v.push(Mu::Swap(i, i + 1));
        }
    }
    for k in 0..n {
        v.push(Mu::Trunc(k));
    }
    v.push(Mu::Extra);
    v.into_iter().filter(|m| applicable(s, *m)).collect()
}

fn code_of(o: &Out) -> String {
    match &o.result {
        Ok(_) => if o.post_ok { "OK".into() } else { "OK-but-postcondition-false".into() },
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, InstructionError::Custom(c)) => format!("C{c}@{i}"),
            TransactionError::InstructionError(i, e) => format!("IE:{e:?}@{i}"),
            other => format!("TX:{other:?}"),
        },
    }
}

#[test]
fn probe_print() {
    let Ok(which) = std::env::var("HYG_PROBE") else { return };
    let t0 = std::time::Instant::now();
    for s in scenarios() {
        if which != "all" && which != s.name {
            continue;
        }
        let l = legit(&s);
        assert!(l.result.is_ok() && l.post_ok, "{}: the legitimate call must succeed: {:?}", s.name, l.result.as_ref().err().map(|f| (&f.err, f.meta.logs.join("\n"))));
        let muts = all_mutations(&s, true);
        let tt = std::time::Instant::now();
        for mu in &muts {
            let mut f = s.fork();
            apply(&mut f, *mu);
            let ix = f.ix.clone();
            let o = run(&mut f, ix);
            let dirty = if o.result.is_err() && o.before != o.after { " DIRTY" } else { "" };
            let slot = |i: usize| format!("{}:{:?}", i, s.tys[i]);
            let desc = match mu {
                Mu::Dup(i, j) => format!("Dup {} <- {}", slot(*i), slot(*j)),
                Mu::Swap(i, j) => format!("Swap {} <-> {}", slot(*i), slot(*j)),
                Mu::Foreign(i) | Mu::LookSame(i) | Mu::LookForeign(i) | Mu::Closed(i) | Mu::Funded(i) | Mu::Missing(i) | Mu::BelowRent(i) | Mu::BadDisc(i) | Mu::Short(i) | Mu::BumpFlip(i) | Mu::NonCanon(i) | Mu::Ro(i) | Mu::Rw(i) | Mu::NoSign(i) | Mu::Sign(i) | Mu::FakeProg(i) | Mu::OtherExec(i) | Mu::OtherTa(i) => {
                    format!("{} {}", format!("{mu:?}").split('(').next().unwrap(), slot(*i))
                }
                other => format!("{other:?}"),
            };
            println!("ROW {} | {} | {}{}", s.name, desc, code_of(&o), dirty);
        }
        eprintln!("{}: {} rows in {:?}", s.name, muts.len(), tt.elapsed());
    }
    eprintln!("total {:?}", t0.elapsed());
}


// ====================================================================================================== the table runner

fn actual_of(o: &Out, idx: usize) -> Result<Exp, String> {
    match &o.result {
        Ok(_) => Ok(Exp::Ok),
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, e) if usize::from(*i) == idx => Ok(Exp::Err(e.clone())),
            other => Err(format!("{other:?}")),
        },
    }
}

fn mu_text(s: &Scn, mu: Mu) -> String {
    let slot = |i: usize| format!("{}:{:?}", i, s.tys.get(i));
    match mu {
        Mu::Dup(i, j) => format!("Dup {} <- {}", slot(i), slot(j)),
        Mu::Swap(i, j) => format!("Swap {} <-> {}", slot(i), slot(j)),
        Mu::Trunc(k) => format!("Trunc({k})"),
        Mu::Extra => "Extra".to_string(),
        Mu::Foreign(i) | Mu::LookSame(i) | Mu::LookForeign(i) | Mu::Closed(i) | Mu::Funded(i) | Mu::Missing(i) | Mu::BelowRent(i) | Mu::BadDisc(i) | Mu::Short(i) | Mu::BumpFlip(i) | Mu::NonCanon(i) | Mu::Ro(i) | Mu::Rw(i) | Mu::NoSign(i) | Mu::Sign(i) | Mu::FakeProg(i) | Mu::OtherExec(i) | Mu::OtherTa(i) => {
            format!("{} {}", format!("{mu:?}").split('(').next().unwrap(), slot(i))
        }
    }
}

/// One checked row.
struct Row {
    ix: &'static str,
    mu: Mu,
    text: String,
    want: Exp,
    got: Result<Exp, String>,
    dirty: bool,
    post_ok: bool,
    /// For a row that is accepted: why that is acceptable (None = nobody wrote a reason down = a problem).
    reason: Option<&'static str>,
    /// For an accepted row: the account in the mutated slot is byte-identical afterwards when the program does not write it.
    untouched_ok: bool,
}

impl Row {
    fn problem(&self) -> Option<String> {
        match &self.got {
            Err(e) => Some(format!("tx-level failure {e}")),
            Ok(g) if *g != self.want => Some(format!("expected {:?}, got {:?}", self.want, g)),
            Ok(Exp::Err(_)) if self.dirty => Some("the refusal CHANGED account bytes".to_string()),
            Ok(Exp::Ok) if self.reason.is_none() => Some("accepted, and no reason is written down for accepting it".to_string()),
            Ok(Exp::Ok) if !self.untouched_ok => Some("accepted, and the account in the mutated slot was changed although the reason says the program does not write it".to_string()),
            Ok(_) => None,
        }
    }
}

/// Why a mutation that the model says is ACCEPTED is acceptable. Every accepted row needs one of these.
fn ok_reason(s: &Scn, mu: Mu) -> Option<&'static str> {
    let ty = |i: usize| s.tys.get(i).copied();
    match mu {
        Mu::Rw(_) => Some("FLAG: a read-only slot passed writable. Anchor does not reject it and the program never writes the account, so nothing changes"),
        Mu::Sign(_) => Some("FLAG: an extra signer flag on a slot that does not need one is ignored"),
        Mu::BelowRent(_) => Some("RENT: the runtime lets a rent-paying account stay as it is when its size does not change and its lamports do not fall; accounts are created rent-exempt and only their owner can debit them, so the state is unreachable"),
        Mu::NonCanon(_) => Some("BUMP: a lookalike at a non-canonical bump address passes the stored-bump seeds check, but no party can create a program-owned account at that off-curve address (this program creates only canonical ones, see stored_bumps_are_canonical), so it is unreachable"),
        Mu::Extra => Some("EXTRA: a trailing account that no slot reads is ignored by Anchor"),
        Mu::Trunc(_) => Some("MARKET: post_journal needs the market account only for events that name it; a Meta-only batch does not"),
        Mu::Closed(i) | Mu::Funded(i) | Mu::Foreign(i) | Mu::LookForeign(i) | Mu::Short(i) | Mu::BadDisc(i) | Mu::Missing(i) | Mu::LookSame(i) => match ty(i) {
            Some(Ty::Adm) => Some("ADMIN: the admin slots are not deserialised, only their key and signer flag are compared"),
            Some(Ty::SigPin) => Some("ADMIN KEY: the compiled-in admin slot is `Signer` + an address check; the balance of the account is irrelevant"),
            Some(Ty::TraderTok | Ty::FeeTok | Ty::Sl8) if matches!(mu, Mu::LookSame(_)) => Some("TOKEN: a token account is not a PDA; any account with the right owner and mint is accepted by design"),
            _ => None,
        },
        Mu::NoSign(i) | Mu::Dup(i, _) | Mu::Swap(i, _) => match ty(i) {
            Some(Ty::SigW) if matches!(mu, Mu::Dup(_, j) if matches!(ty(j), Some(Ty::Adm | Ty::SigPin))) => {
                Some("PAYER: any funded signer may pay the rent of the new accounts; here the payer slot holds the key of an admin that signs anyway")
            }
            Some(Ty::Adm) => Some("ADMIN: one admin signature is enough for this instruction (DESIGN 6.1, 14)"),
            Some(Ty::SelfProg) => Some("EVENT: the event_cpi `program` slot has no constraint and is never read"),
            _ => None,
        },
        _ => None,
    }
}

/// Runs every mutation of one scenario against the model.
fn check_scenario(s: &Scn, mus: &[Mu]) -> Vec<Row> {
    let idx = s.pre.len();
    let mut rows = Vec::new();
    for mu in mus {
        let mut f = s.fork();
        apply(&mut f, *mu);
        let want = expect(s, &f);
        let ix = f.ix.clone();
        let acct_before: std::collections::BTreeMap<Pubkey, (u64, Vec<u8>, Pubkey)> =
            f.ix.accounts.iter().filter_map(|m| f.world.envr().svm.get_account(&m.pubkey).map(|a| (m.pubkey, (a.lamports, a.data, a.owner)))).collect();
        let o = run(&mut f, ix);
        let got = actual_of(&o, idx);
        let dirty = o.result.is_err() && o.before != o.after;
        let reason = if want == Exp::Ok { ok_reason(s, *mu) } else { None };
        // an accepted flag-only mutation on a slot the program does not write must not change the account it touched
        let untouched_ok = match mu {
            Mu::Rw(i) | Mu::Sign(i) | Mu::BelowRent(i) if !s.ix.accounts[*i].is_writable => {
                let k = f.ix.accounts[*i].pubkey;
                let now = f.world.envr().svm.get_account(&k).map(|a| (a.lamports, a.data.clone(), a.owner));
                acct_before.get(&k).cloned() == now
            }
            _ => true,
        };
        rows.push(Row { ix: s.name, mu: *mu, text: mu_text(s, *mu), want, got, dirty, post_ok: o.post_ok, reason, untouched_ok });
    }
    rows
}

#[test]
fn model_vs_program_probe() {
    let Ok(which) = std::env::var("HYG_MODEL") else { return };
    let full = std::env::var("HYG_FULL").is_ok();
    for s in scenarios() {
        if which != "all" && which != s.name {
            continue;
        }
        let l = legit(&s);
        assert!(l.result.is_ok() && l.post_ok, "{}: the legitimate call must succeed", s.name);
        let rows = check_scenario(&s, &all_mutations(&s, full));
        let bad: Vec<&Row> = rows.iter().filter(|r| r.problem().is_some()).collect();
        eprintln!("{}: {} rows, {} disagree", s.name, rows.len(), bad.len());
        for r in bad {
            println!("MISMATCH {} | {} | {}", s.name, r.text, r.problem().unwrap());
        }
    }
}


// ====================================================================================================== per-instruction tables

const NAMES: [&str; 25] = [
    "init_config", "set_evaluator", "set_limits", "set_paused", "create_ruleset", "create_market", "update_market", "ban_wallet", "unban_wallet", "open_account",
    "reset_account", "post_journal", "request_advance", "cancel_pass", "confirm_pass", "reject_pass", "veto_pass", "finalize_pass", "report_fail", "hold_account",
    "release_hold", "fail_for_violation", "touch_activity", "release_account", "close_account",
];

fn full_pairs() -> bool {
    std::env::var("HYG_FULL").is_ok()
}

/// Builds the scenario `name`, proves the legitimate call works, runs every mutation against the model.
fn table(name: &str) -> (Scn, Vec<Row>) {
    let s = scenarios_for(name).pop().unwrap_or_else(|| panic!("no scenario {name}"));
    let l = legit(&s);
    assert!(l.result.is_ok() && l.post_ok, "{name}: the legitimate call must succeed first: {:?}", l.result.as_ref().err().map(|f| (&f.err, f.meta.logs.join("\n"))));
    let mus = all_mutations(&s, full_pairs());
    let rows = check_scenario(&s, &mus);
    (s, rows)
}

fn assert_table(name: &str) {
    let (_s, rows) = table(name);
    assert!(rows.len() > 20, "{name}: only {} rows", rows.len());
    let bad: Vec<String> = rows.iter().filter_map(|r| r.problem().map(|p| format!("{} | {} | {p}", r.ix, r.text))).collect();
    assert!(bad.is_empty(), "{} of {} rows disagree with the hand-written model:\n{}", bad.len(), rows.len(), bad.join("\n"));
}

macro_rules! table_tests {
    ($($fname:ident => $name:expr),* $(,)?) => {
        $( #[test] fn $fname() { assert_table($name); } )*
    };
}

table_tests! {
    table_init_config => "init_config",
    table_set_evaluator => "set_evaluator",
    table_set_limits => "set_limits",
    table_set_paused => "set_paused",
    table_create_ruleset => "create_ruleset",
    table_create_market => "create_market",
    table_update_market => "update_market",
    table_ban_wallet => "ban_wallet",
    table_unban_wallet => "unban_wallet",
    table_open_account => "open_account",
    table_reset_account => "reset_account",
    table_post_journal => "post_journal",
    table_request_advance => "request_advance",
    table_cancel_pass => "cancel_pass",
    table_confirm_pass => "confirm_pass",
    table_reject_pass => "reject_pass",
    table_veto_pass => "veto_pass",
    table_finalize_pass => "finalize_pass",
    table_report_fail => "report_fail",
    table_hold_account => "hold_account",
    table_release_hold => "release_hold",
    table_fail_for_violation => "fail_for_violation",
    table_touch_activity => "touch_activity",
    table_release_account => "release_account",
    table_close_account => "close_account",
}


// ====================================================================================================== dedicated tests

/// Digest of everything an instruction of this program can touch, in a vault world, for the dedicated tests. The fee payer of the
/// transaction (`e.payer`) is the only account left out.
fn jdigest(w: &JEnv, ixs: &[Instruction]) -> [u8; 32] {
    let fp = w.v.e.payer.pubkey();
    let mut extra: Vec<Pubkey> = ixs.iter().flat_map(|i| i.accounts.iter().map(|m| m.pubkey)).collect();
    extra.retain(|k| *k != fp);
    extra.push(sector_authority_pda());
    extra.push(ev_auth());
    for m in 0..8u16 {
        extra.push(market_pda(m));
    }
    let tr = w.trader.pubkey();
    for id in 0..4u64 {
        let vts = vault_trader_state_pda(&tr, id);
        for rid in 1..=6u64 {
            extra.push(claim_pda(&vts, rid));
        }
    }
    extra.push(w.v.usdt_pool);
    extra.push(w.v.evaluator.pubkey());
    extra.push(w.v.e.sl8.pubkey());
    extra.push(w.v.e.rov.pubkey());
    extra.sort();
    extra.dedup();
    w.v.digest(&tr, &extra)
}

fn send_keys(w: &mut JEnv, ixs: &[Instruction], more: &[&Keypair]) -> TransactionResult {
    let ks = [dup(&w.trader), dup(&w.v.evaluator), dup(&w.v.e.sl8), dup(&w.v.e.rov)];
    let mut refs: Vec<&Keypair> = ks.iter().collect();
    refs.extend(more.iter().copied());
    w.v.e.send(ixs, &refs)
}

fn got_at(r: &TransactionResult, idx: usize) -> Result<Exp, String> {
    match r {
        Ok(_) => Ok(Exp::Ok),
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, e) if usize::from(*i) == idx => Ok(Exp::Err(e.clone())),
            other => Err(format!("{other:?}\n{}", f.meta.logs.join("\n"))),
        },
    }
}

/// The transaction fails with exactly `want` at instruction `idx` and every account is byte-identical afterwards.
fn refused(w: &mut JEnv, ixs: &[Instruction], more: &[&Keypair], idx: usize, want: Exp, what: &str) {
    let before = jdigest(w, ixs);
    let r = send_keys(w, ixs, more);
    assert_eq!(got_at(&r, idx), Ok(want), "{what}");
    assert_eq!(jdigest(w, ixs), before, "{what}: the refusal changed account bytes");
}

fn ok_tx(w: &mut JEnv, ixs: &[Instruction], more: &[&Keypair], what: &str) {
    let r = send_keys(w, ixs, more);
    if let Err(f) = &r {
        panic!("{what}: expected success, got {:?}\n{}", f.err, f.meta.logs.join("\n"));
    }
}

fn post_journal_ix(w: &JEnv, signer: Pubkey, bytes: Vec<u8>, markets: &[u16]) -> Instruction {
    let mut metas = accounts::PostJournal {
        evaluator: signer,
        trading_account: w.acct,
        config: config_pda(),
        ruleset: ruleset_pda(w.account().ruleset_version),
        event_authority: ev_auth(),
        program: PROGRAM_ID,
    }
    .to_account_metas(None);
    metas.extend(JEnv::market_metas(markets));
    Instruction { program_id: PROGRAM_ID, accounts: metas, data: anchor_lang::InstructionData::data(&instruction::PostJournal { batch_bytes: bytes }) }
}

// ---------------------------------------------------------------------------------------------------- re-initialisation

/// RE-INIT 1. The wallet profile says `next_challenge_id = 0` (state edited, as if the profile had been wiped), so the account address
/// the seeds name is account 0 which EXISTS. `open_account` must not overwrite it. BLOCKED by the system program (`create_account` /
/// `allocate` on an address that is already in use, Custom(0)); account 0, its vault record and the wallet stay byte-identical.
#[test]
fn reinit_open_account_cannot_overwrite_an_existing_trading_account() {
    let mut w = JEnv::new();
    let tr = w.trader.pubkey();
    let t = w.t0 + 1_000;
    w.v.edit_wallet(&tr, |p| p.next_challenge_id = 0);
    let oa = w.v.open_accounts(&tr);
    assert_eq!(oa.trading_account, w.acct, "the seeds now name the existing account");
    let ix = oa.to_ix(0);
    w.set_time(t);
    let existing = w.v.e.svm.get_account(&w.acct).unwrap();
    assert!(!existing.data.is_empty());
    refused(&mut w, &[ix], &[], 0, cu(0), "open_account on an address that already holds a trading account");
    assert_eq!(w.v.e.svm.get_account(&w.acct).unwrap().data, existing.data, "the existing account was rewritten");
}

/// RE-INIT 2. The wallet is intact (`next = 1`) but the caller names the EXISTING account 0 in the new-account slot: the seeds name
/// account 1, so Anchor refuses with ConstraintSeeds (2006) before anything is created.
#[test]
fn reinit_open_account_naming_the_existing_account_is_a_seeds_error() {
    let mut w = JEnv::new();
    let tr = w.trader.pubkey();
    let t = w.t0 + 1_000;
    let mut oa = w.v.open_accounts(&tr);
    oa.trading_account = w.acct;
    let ix = oa.to_ix(0);
    w.set_time(t);
    refused(&mut w, &[ix], &[], 0, cu(A_CONSTRAINT_SEEDS), "open_account naming account 0 while the wallet says 1");
}

/// RE-INIT 3. `init_config` a second time, with both real admins signing and different parameters: the config PDA exists, so the
/// `init` constraint fails (system: address already in use, Custom(0)); the config and the tally keep every byte.
#[test]
fn reinit_init_config_twice_is_refused_and_changes_nothing() {
    let mut w = JEnv::new();
    let mut args = default_args();
    args.evaluator = Pubkey::new_from_array([0x99; 32]);
    args.max_payout_per_window = 1;
    let ix = init_ix_with(w.v.e.payer.pubkey(), w.v.e.sl8.pubkey(), w.v.e.rov.pubkey(), &args);
    w.set_time(w.t0 + 1_000);
    let cfg_before = w.v.e.svm.get_account(&config_pda()).unwrap();
    refused(&mut w, &[ix], &[], 0, cu(0), "init_config on an existing config");
    assert_eq!(w.v.e.svm.get_account(&config_pda()).unwrap().data, cfg_before.data);
}

/// RE-INIT 4. `create_ruleset` for version 1 and `create_market` for market 0 a second time (both admins): `init` on an address in
/// use, Custom(0), nothing changes (the immutable rule set cannot be replaced, a market only through update_market).
#[test]
fn reinit_create_ruleset_and_create_market_twice_are_refused() {
    let mut w = JEnv::new();
    w.set_time(w.t0 + 1_000);
    let payer = w.v.e.payer.pubkey();
    let a = Admins::both(&w.v.e);
    let mut args = launch_ruleset_args();
    args.fee_bps = 1; // a different content for the same version
    let ix = create_ruleset_ix(payer, a, &args);
    refused(&mut w, &[ix], &[], 0, cu(0), "create_ruleset version 1 again");
    let mut m = create_market_args(0, 0, 99);
    m.params.max_age_secs = 1;
    let ix = create_market_ix(payer, a, &m);
    refused(&mut w, &[ix], &[], 0, cu(0), "create_market id 0 again");
}

/// RE-INIT 5 (LIMITED). `ban_wallet` uses `init_if_needed` on a wallet that already exists. It must set `banned` and nothing else:
/// `next_challenge_id` and `active_count` survive (a wipe would let the trader's next account reuse an address and a challenge id).
#[test]
fn ban_wallet_on_an_existing_wallet_keeps_its_counters() {
    let mut w = JEnv::new();
    let tr = w.trader.pubkey();
    w.set_time(w.t0 + 1_000);
    let before = w.wallet_profile();
    assert_eq!((before.next_challenge_id, before.active_count, before.banned), (1, 1, 0));
    let a = Admins::both(&w.v.e);
    let ix = common::ban_wallet_ix(w.v.e.payer.pubkey(), a, tr, hash(5));
    common::assert_ok(w.v.e.send_admins(ix, a));
    let after = w.wallet_profile();
    assert_eq!((after.next_challenge_id, after.active_count, after.banned, after.trader, after.bump), (1, 1, 1, before.trader, before.bump));
}

// ---------------------------------------------------------------------------------------------------- closed addresses

/// Every instruction that names the trading account, built against the CURRENT account (`w.acct`), for the closed-address tests.
fn ta_instructions(w: &JEnv, payer: Pubkey) -> Vec<(&'static str, Instruction)> {
    let ev = w.v.evaluator.pubkey();
    let tr = w.trader.pubkey();
    let adm = Admins::both(&w.v.e);
    let ra = w.reset_accts();
    vec![
        ("request_advance", w.request_ix()),
        ("cancel_pass", w.cancel_ix()),
        ("confirm_pass", w.confirm_ix(ev)),
        ("reject_pass", w.reject_ix(ev, c::REJECT_OTHER)),
        ("veto_pass", w.veto_ix(adm)),
        ("finalize_pass", w.finalize_accts(payer).to_ix()),
        ("report_fail", w.report_fail_ix(ev, vec![], 0, &[MARKET_EUR])),
        ("post_journal", post_journal_ix(w, ev, vec![], &[MARKET_EUR])),
        ("hold_account", w.hold_ix(ev, hash(1))),
        ("release_hold", w.release_hold_ix(adm)),
        ("fail_for_violation", w.violation_ix(adm, hash(9))),
        ("touch_activity", w.touch_ix(ev)),
        ("release_account", w.release_acct_ix(tr)),
        ("close_account", w.close_ix(tr)),
        ("reset_account", ra.to_ix(w.account().challenge_id)),
    ]
}

fn closable_world() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.make_terminal(c::STATUS_FAILED, t);
    (w, t + RETENTION)
}

/// REUSE 1. In ONE transaction `close_account` empties the trading account and a second instruction names the same address. Inside
/// the transaction the account is already system-owned with 0 lamports: every one of the 15 instructions that take the account
/// fails with AccountNotInitialized (3012) at instruction 1, and since the transaction is atomic the account is NOT closed afterwards.
#[test]
fn closed_address_reused_in_the_same_transaction_is_not_initialized_for_every_instruction() {
    let (probe, _) = closable_world();
    let names: Vec<&'static str> = ta_instructions(&probe, Pubkey::new_unique()).iter().map(|(n, _)| *n).collect();
    assert_eq!(names.len(), 15);
    for (k, name) in names.iter().enumerate() {
        let (mut w, t) = closable_world();
        let payer = w.new_payer();
        let ixs = ta_instructions(&w, payer.pubkey());
        let second = ixs[k].1.clone();
        let close = w.close_ix(w.trader.pubkey());
        w.set_time(t);
        let acct = w.acct;
        refused(&mut w, &[close, second], &[&payer], 1, cu(A_NOT_INITIALIZED), &format!("close_account then {name} in one transaction"));
        assert!(w.v.e.svm.get_account(&acct).is_some_and(|a| a.lamports > 0 && !a.data.is_empty()), "{name}: the atomic failure must leave the account open");
    }
}

/// REUSE 2 (revival). close_account, then a plain transfer of lamports to the dead address (so the runtime would keep it), then the
/// second instruction: the address is system-owned, funded, empty: WrongOwner (3007) at instruction 2 for every instruction.
/// Then the same without the second instruction (the revival succeeds as a plain system account) and the instruction in a later
/// transaction: still 3007; the revived address is never a trading account again.
#[test]
fn revived_closed_address_is_a_system_account_for_every_instruction() {
    let (probe, _) = closable_world();
    let n = ta_instructions(&probe, Pubkey::new_unique()).len();
    for k in 0..n {
        let (mut w, t) = closable_world();
        let payer = w.new_payer();
        let ixs = ta_instructions(&w, payer.pubkey());
        let (name, second) = (ixs[k].0, ixs[k].1.clone());
        let close = w.close_ix(w.trader.pubkey());
        let acct = w.acct;
        let fund = anchor_lang::solana_program::system_instruction::transfer(&payer.pubkey(), &acct, 5_000_000);
        w.set_time(t);
        refused(&mut w, &[close.clone(), fund.clone(), second.clone()], &[&payer], 2, cu(A_WRONG_OWNER), &format!("close, fund, {name} in one transaction"));
        // the revival alone succeeds ...
        ok_tx(&mut w, &[close, fund], &[&payer], "close + fund");
        let a = w.v.e.svm.get_account(&acct).expect("revived account exists");
        assert_eq!((a.owner, a.data.len()), (system_program::ID, 0));
        // ... and the instruction in a later transaction still sees a system account
        refused(&mut w, &[second], &[&payer], 0, cu(A_WRONG_OWNER), &format!("{name} on the revived address"));
    }
}

/// REUSE 3. The address of a closed account is never created again: the wallet's `next_challenge_id` only increases, so a later
/// `open_account` creates account 1, and naming the closed address 0 in the new-account slot is a seeds error (2006), also in
/// the same transaction as the close.
#[test]
fn a_closed_account_address_is_never_recreated_by_open_account() {
    let (mut w, t) = closable_world();
    let tr = w.trader.pubkey();
    let acct0 = w.acct;
    let close = w.close_ix(tr);
    w.set_time(t);
    // same transaction: close, then open naming the closed address
    let mut oa = w.v.open_accounts(&tr);
    oa.trading_account = acct0;
    refused(&mut w, &[close.clone(), oa.to_ix(0)], &[], 1, cu(A_CONSTRAINT_SEEDS), "close_account then open_account naming the closed address");
    // close for real, then name it again in a later transaction
    ok_tx(&mut w, &[close], &[], "close_account");
    assert!(w.v.e.svm.get_account(&acct0).is_none_or(|a| a.lamports == 0));
    let mut oa = w.v.open_accounts(&tr);
    oa.trading_account = acct0;
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(A_CONSTRAINT_SEEDS), "open_account naming the closed address");
    // a correct open creates account 1 and leaves address 0 empty
    let ttr = dup(&w.trader);
    common::assert_ok(w.v.open(&ttr, 8));
    assert!(w.v.e.svm.get_account(&acct_pda(&tr, 1)).is_some_and(|a| a.owner == PROGRAM_ID));
    assert!(w.v.e.svm.get_account(&acct0).is_none_or(|a| a.lamports == 0), "the closed address was reused");
}

/// PRE-FUNDED ADDRESSES (DoS). Anyone can send lamports to an address that a later `open_account` / `init_config` will create (the
/// address is predictable). Anchor's `init` takes the transfer / allocate / assign path then, so the creation still works.
#[test]
fn a_prefunded_future_address_does_not_block_account_creation() {
    // open_account: the trading account, the vault record and the wallet profile of a NEW trader are all pre-funded
    let mut w = JEnv::new();
    let t2 = w.v.new_trader();
    let k = t2.pubkey();
    for addr in [acct_pda(&k, 0), vault_trader_state_pda(&k, 0), wallet_pda(&k)] {
        w.v.e.svm.set_account(addr, RawAccount { lamports: 1_234_567, data: vec![], owner: system_program::ID, executable: false, rent_epoch: 0 }).unwrap();
    }
    w.set_time(w.t0 + 1_000);
    common::assert_ok(w.v.open(&t2, 0));
    let a = w.v.e.svm.get_account(&acct_pda(&k, 0)).unwrap();
    assert_eq!(a.owner, PROGRAM_ID);
    assert!(!a.data.is_empty());
    assert!(w.v.e.svm.get_account(&wallet_pda(&k)).is_some_and(|a| a.owner == PROGRAM_ID));
    assert!(w.v.vault_trader_state(&k, 0).is_some());
    // init_config: the config and the payout tally pre-funded
    let mut e = Env::new_with(false);
    for addr in [config_pda(), tally_pda()] {
        e.svm.set_account(addr, RawAccount { lamports: 777_777, data: vec![], owner: system_program::ID, executable: false, rent_epoch: 0 }).unwrap();
    }
    let ix = e.init_ix(&default_args());
    common::assert_ok(e.send_as_admins(&[ix]));
    assert_eq!(e.svm.get_account(&config_pda()).unwrap().owner, PROGRAM_ID);
    assert_eq!(e.svm.get_account(&tally_pda()).unwrap().data.len(), 25);
}

// ---------------------------------------------------------------------------------------------------- sysvar and canonical bumps

/// SYSVAR FROM A FAKE ACCOUNT. No instruction has a sysvar slot: the program reads the Clock and the Rent through syscalls. So a fake
/// clock account (right size and layout, a unix time far in the future) placed where a sysvar might be expected, as an extra account
/// of finalize_pass and cancel_pass, changes nothing: the delay is still not over (PassDelayNotElapsed) and the cancel is still too early.
#[test]
fn a_fake_clock_account_changes_nothing() {
    use anchor_lang::solana_program::clock::Clock;
    let fake_clock = |unix: i64| {
        let c = Clock { slot: 1, epoch_start_timestamp: 0, epoch: 1, leader_schedule_epoch: 1, unix_timestamp: unix };
        RawAccount { lamports: 1_000_000, data: bincode::serialize(&c).unwrap(), owner: system_program::ID, executable: false, rent_epoch: 0 }
    };
    // finalize_pass one second before the delay is over
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(DELAY, CAP, WINDOW);
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t - 200, t);
    let payer = w.new_payer();
    let mut ix = w.finalize_accts(payer.pubkey()).to_ix();
    let fake = Pubkey::new_unique();
    w.v.e.svm.set_account(fake, fake_clock(t + i64::from(DELAY) + 10_000)).unwrap();
    ix.accounts.push(AccountMeta::new_readonly(fake, false));
    ix.accounts.push(AccountMeta::new_readonly(anchor_lang::solana_program::sysvar::clock::ID, false));
    w.set_time(t + i64::from(DELAY) - 1);
    refused(&mut w, &[ix], &[&payer], 0, pe(E::PassDelayNotElapsed), "finalize_pass with a fake clock account");
    // cancel_pass 1 second too early
    let mut w = JEnv::new();
    let t = w.t0 + 10_000;
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t - 3_599, 0);
    let mut ix = w.cancel_ix();
    let fake = Pubkey::new_unique();
    w.v.e.svm.set_account(fake, fake_clock(t + 100_000)).unwrap();
    ix.accounts.push(AccountMeta::new_readonly(fake, false));
    w.set_time(t);
    refused(&mut w, &[ix], &[], 0, pe(E::CancelTooEarly), "cancel_pass with a fake clock account");
}

/// The creation path uses the CANONICAL bump for every PDA it creates, which is why a lookalike at a non-canonical bump (accepted by
/// the stored-bump seeds check, see the table rows with reason BUMP) cannot exist: nobody else can create a program-owned account at
/// an off-curve address.
#[test]
fn stored_bumps_are_canonical() {
    let mut w = JEnv::new();
    let tr = w.trader.pubkey();
    let cfg = w.v.e.config();
    assert_eq!(cfg.bump, Pubkey::find_program_address(&[c::CONFIG_SEED], &PROGRAM_ID).1);
    let ta = w.account();
    assert_eq!(ta.bump, Pubkey::find_program_address(&[c::ACCT_SEED, tr.as_ref(), &0u64.to_le_bytes()], &PROGRAM_ID).1);
    let wal = w.wallet_profile();
    assert_eq!(wal.bump, Pubkey::find_program_address(&[c::WALLET_SEED, tr.as_ref()], &PROGRAM_ID).1);
    assert_eq!(w.v.e.ruleset(1).bump, Pubkey::find_program_address(&[c::RULESET_SEED, &1u16.to_le_bytes()], &PROGRAM_ID).1);
    for id in 0..5u16 {
        assert_eq!(w.v.e.market(id).bump, Pubkey::find_program_address(&[c::MARKET_SEED, &id.to_le_bytes()], &PROGRAM_ID).1);
    }
    // an account opened later, and one made by reset_account, also store the canonical bump
    let ttr = dup(&w.trader);
    common::assert_ok(w.v.open(&ttr, 8));
    let second = w.v.account(&acct_pda(&tr, 1));
    assert_eq!(second.bump, Pubkey::find_program_address(&[c::ACCT_SEED, tr.as_ref(), &1u64.to_le_bytes()], &PROGRAM_ID).1);
    // the table's non-canonical lookalikes really are at a DIFFERENT address than the canonical one
    let (nc, b) = non_canonical(&[c::CONFIG_SEED]).expect("a non-canonical bump exists for the config seeds");
    assert_ne!(nc, config_pda());
    assert!(b < cfg.bump);
}


// ---------------------------------------------------------------------------------------------------- money slots, mints, token program

type Redirect = Box<dyn Fn(&mut [Pubkey; 5])>;
type Case = (&'static str, Redirect, Exp);

/// A world with a second trader (the victim) holding 100,000 USDC, and the attacker (`w.trader`) about to buy.
fn buyer_world() -> (JEnv, Keypair, Pubkey) {
    let mut w = JEnv::new();
    let victim = w.v.new_trader();
    let victim_token = w.v.trader_token(&victim.pubkey());
    w.set_time(w.t0 + 1_000);
    (w, victim, victim_token)
}

fn failed_world() -> JEnv {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.fail_in(1, t);
    w.set_time(t + 100);
    w
}

/// Every token slot that decides where money goes or whence it comes, in open_account and reset_account.
/// * `trader_token_account` = somebody else's token account (the signer is not its owner): InvalidTokenAccount (6040), ours.
/// * `fee_token_account` = the buyer's own account (the 0.4% fee would stay with the buyer): 6040, ours.
/// * `pool_token_account` / `vault_sl8_token_account` = the buyer's own account, or the two swapped: the vault's InvalidTokenAccount (6021).
///
/// BLOCKED in every case; no token balance and no other account moves (the digest covers every tracked token account).
#[test]
fn money_slots_cannot_be_redirected_in_open_and_reset() {
    for flow in ["open_account", "reset_account"] {
        let (mut w, _victim, victim_token) = if flow == "open_account" { buyer_world() } else { (failed_world(), Keypair::new(), Pubkey::default()) };
        let tr = w.trader.pubkey();
        let victim_token = if flow == "reset_account" { w.v.new_token_account(&w.v.usdc.clone(), &Pubkey::new_unique(), 50_000_000_000) } else { victim_token };
        let own = w.v.trader_token(&tr);
        let build = |w: &JEnv, f: &dyn Fn(&mut [Pubkey; 5])| -> Instruction {
            // [trader_token, fee_token, pool, vault_sl8, mint]
            let mut v = [w.v.trader_token(&tr), w.v.fee_usdc, w.v.usdc_pool, w.v.vault_sl8_usdc, w.v.usdc];
            f(&mut v);
            if flow == "open_account" {
                let mut oa = w.v.open_accounts(&tr);
                (oa.trader_token_account, oa.fee_token_account, oa.pool_token_account, oa.vault_sl8_token_account, oa.mint) = (v[0], v[1], v[2], v[3], v[4]);
                oa.to_ix(0)
            } else {
                let mut ra = w.reset_accts();
                (ra.trader_token_account, ra.fee_token_account, ra.pool_token_account, ra.vault_sl8_token_account, ra.mint) = (v[0], v[1], v[2], v[3], v[4]);
                ra.to_ix(w.account().challenge_id)
            }
        };
        let cases: Vec<Case> = vec![
            ("trader_token_account is another person's", Box::new(move |v| v[0] = victim_token), pe(E::InvalidTokenAccount)),
            ("fee_token_account is the buyer's own", Box::new(move |v| v[1] = own), pe(E::InvalidTokenAccount)),
            ("pool_token_account is the buyer's own", Box::new(move |v| v[2] = own), cu(V_INVALID_TOKEN_ACCOUNT)),
            ("vault_sl8_token_account is the buyer's own", Box::new(move |v| v[3] = own), cu(V_INVALID_TOKEN_ACCOUNT)),
            ("pool and vault_sl8 swapped", Box::new(|v| v.swap(2, 3)), cu(V_INVALID_TOKEN_ACCOUNT)),
            ("fee and vault_sl8 swapped", Box::new(|v| v.swap(1, 3)), pe(E::InvalidTokenAccount)),
        ];
        for (what, f, want) in cases {
            let ix = build(&w, &*f);
            refused(&mut w, &[ix], &[], 0, want, &format!("{flow}: {what}"));
        }
        // sanity: the unmodified call succeeds in this very world
        let ix = build(&w, &|_| {});
        ok_tx(&mut w, &[ix], &[], &format!("{flow}: the correct call"));
    }
}

/// WRONG MINT and WRONG TOKEN PROGRAM in open_account.
/// * mint = USDT (a mint the vault accepts) while the token accounts are USDC: 6040 (ours, the token accounts name another mint).
/// * mint and the buyer's and the fee account are USDT but the pool is the USDC pool: the vault's 6021.
/// * a fake mint of the right layout with matching token accounts: the vault's InvalidMint (6017) after our fee transfer ran; the
///   atomic failure puts the fee back.
/// * token program = Token-2022 or the system program: InvalidProgramId (3008).
#[test]
fn wrong_mint_and_wrong_token_program_are_refused_in_open_account() {
    let (mut w, _victim, _vt) = buyer_world();
    let tr = w.trader.pubkey();
    let (usdt, fee_wallet) = (w.v.usdt, w.v.fee_wallet);
    let base = w.v.open_accounts(&tr);

    let mut oa = base.clone();
    oa.mint = usdt;
    refused(&mut w, &[oa.to_ix(0)], &[], 0, pe(E::InvalidTokenAccount), "mint = USDT, token accounts USDC");

    let usdt_trader = w.v.new_token_account(&usdt, &tr, TRADER_START);
    let usdt_fee = w.v.new_token_account(&usdt, &fee_wallet, 0);
    let usdt_sl8 = w.v.new_token_account(&usdt, &VAULT_SL8_ADMIN, 0);
    let mut oa = base.clone();
    (oa.mint, oa.trader_token_account, oa.fee_token_account, oa.vault_sl8_token_account) = (usdt, usdt_trader, usdt_fee, usdt_sl8);
    // pool stays the USDC pool
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(V_INVALID_TOKEN_ACCOUNT), "mint = USDT but the USDC pool");

    let fake = Pubkey::new_unique();
    w.v.set_mint(&fake);
    let fake_trader = w.v.new_token_account(&fake, &tr, TRADER_START);
    let fake_fee = w.v.new_token_account(&fake, &fee_wallet, 0);
    let mut oa = base.clone();
    (oa.mint, oa.trader_token_account, oa.fee_token_account) = (fake, fake_trader, fake_fee);
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(6017), "a fake mint that the vault does not know (VaultError::InvalidMint)");
    assert_eq!(w.v.token_balance(&fake_fee), 0, "the fee transfer must be rolled back");

    let mut oa = base.clone();
    oa.token_program = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb".parse().unwrap();
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(A_INVALID_PROGRAM_ID), "Token-2022 in the token program slot");
    let mut oa = base.clone();
    oa.token_program = system_program::ID;
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(A_INVALID_PROGRAM_ID), "the system program in the token program slot");
    let mut oa = base.clone();
    oa.system_program = anchor_spl::token::spl_token::ID;
    refused(&mut w, &[oa.to_ix(0)], &[], 0, cu(A_INVALID_PROGRAM_ID), "the token program in the system program slot");
}

/// LIMITED. USDT is the vault's second mint: a buyer who uses USDT in EVERY slot (mint, own account, fee account, the vault's USDT
/// pool, a USDT account of the vault's SL8 wallet) is accepted. Pinned so that a change of the mint policy breaks this test.
#[test]
fn usdt_in_every_slot_is_a_valid_alternative_and_moves_only_usdt() {
    let (mut w, _victim, _vt) = buyer_world();
    let tr = w.trader.pubkey();
    let (usdc, usdt, fee_wallet) = (w.v.usdc, w.v.usdt, w.v.fee_wallet);
    let usdt_trader = w.v.new_token_account(&usdt, &tr, TRADER_START);
    let usdt_fee = w.v.new_token_account(&usdt, &fee_wallet, 0);
    let usdt_sl8 = w.v.new_token_account(&usdt, &VAULT_SL8_ADMIN, 0);
    let usdc_before = (w.v.token_balance(&w.v.trader_token(&tr)), w.v.token_balance(&w.v.usdc_pool), w.v.token_balance(&w.v.fee_usdc));
    let mut oa = w.v.open_accounts(&tr);
    (oa.mint, oa.trader_token_account, oa.fee_token_account, oa.vault_sl8_token_account, oa.pool_token_account) = (usdt, usdt_trader, usdt_fee, usdt_sl8, w.v.usdt_pool);
    ok_tx(&mut w, &[oa.to_ix(0)], &[], "open_account with USDT everywhere");
    assert!(w.v.token_balance(&usdt_trader) < TRADER_START);
    assert!(w.v.token_balance(&w.v.usdt_pool) > 0 && w.v.token_balance(&usdt_fee) > 0);
    assert_eq!(usdc_before, (w.v.token_balance(&w.v.trader_token(&tr)), w.v.token_balance(&w.v.usdc_pool), w.v.token_balance(&w.v.fee_usdc)), "no USDC may move");
    let _ = usdc;
}

/// MISSING / EXTRA REMAINING ACCOUNTS. post_journal and report_fail take the markets as remaining accounts and cap them at 8
/// (BatchTooLarge). 8 are accepted (the same market repeated), 9 are refused, state unchanged.
#[test]
fn more_than_eight_remaining_market_accounts_are_refused() {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    let ev = w.v.evaluator.pubkey();
    let batch = simple(&w, t, vec![], true);
    let bytes = batch.to_bytes().unwrap();
    w.set_time(t);
    let nine = post_journal_ix(&w, ev, bytes.clone(), &[MARKET_EUR; 9]);
    let eight = post_journal_ix(&w, ev, bytes, &[MARKET_EUR; 8]);
    refused(&mut w, &[budget_ix(), nine], &[], 1, pe(E::BatchTooLarge), "post_journal with 9 remaining accounts");
    ok_tx(&mut w, &[budget_ix(), eight], &[], "post_journal with 8 remaining accounts");
    assert_eq!(w.account().seq, 1);
    // report_fail
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.edit_account(|a| a.b = FLOOR);
    let b = w.fail_batch(t, vec![], journal_types::FailReason::StaticFloor, FLOOR, true);
    let ix = w.report_fail_ix(w.v.evaluator.pubkey(), b.to_bytes().unwrap(), 0, &[MARKET_EUR; 9]);
    w.set_time(t + 1);
    refused(&mut w, &[ix], &[], 0, pe(E::BatchTooLarge), "report_fail with 9 remaining accounts");
}


// ====================================================================================================== the harness cannot pass vacuously

use std::collections::BTreeSet;

const LIB_RS: &str = include_str!("../../programs/setl8-lev-trading/src/lib.rs");

/// The names of the `pub fn`s inside the `#[program]` module of `src` (same parser as who_may_call.rs).
fn program_instructions(src: &str) -> Vec<String> {
    let start = src.find("#[program]").expect("no #[program] block");
    let open = start + src[start..].find('{').expect("program module has no body");
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut end = bytes.len();
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut names = Vec::new();
    let mut rest = &src[open..end];
    while let Some(i) = rest.find("pub fn ") {
        let tail = &rest[i + "pub fn ".len()..];
        names.push(tail.chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect());
        rest = tail;
    }
    names
}

/// A new instruction without a scenario (and so without account-hygiene rows) fails here.
#[test]
fn every_instruction_of_the_program_has_a_scenario_and_vice_versa() {
    let program: BTreeSet<String> = program_instructions(LIB_RS).into_iter().collect();
    let ours: BTreeSet<String> = NAMES.iter().map(|n| n.to_string()).collect();
    assert_eq!(program.len(), 25);
    assert_eq!(program.difference(&ours).collect::<Vec<_>>(), Vec::<&String>::new(), "instructions without a scenario");
    assert_eq!(ours.difference(&program).collect::<Vec<_>>(), Vec::<&String>::new(), "scenarios without an instruction");
    // the scenario builder knows every name
    let built: BTreeSet<&str> = scenarios().iter().map(|s| s.name).collect();
    assert_eq!(built.len(), 25);
    for n in NAMES {
        assert!(built.contains(n), "scenarios() does not build {n}");
    }
    // and the parser itself notices a new instruction
    let with_new = LIB_RS.replacen("    pub fn close_account(", "    pub fn drain(ctx: Context<CloseAccount>) -> Result<()> {\n        Ok(())\n    }\n\n    pub fn close_account(", 1);
    assert_ne!(with_new, LIB_RS);
    assert!(program_instructions(&with_new).contains(&"drain".to_string()));
}

/// Every slot type of the model is used by at least one scenario (a type nobody uses would be untested model code), and every
/// scenario's slot list is as long as its account list (checked in `mk`).
#[test]
fn every_slot_type_is_used_by_a_scenario() {
    use Ty::*;
    let all = [
        SigW, Sig, SigPin, Cfg(true), Cfg(false), Ta { w: true, tc: Some(0) }, Ta { w: true, tc: None }, Wal, RsSeed, RsFree, Mkt, Adm, SecAuth, VProg, VReg(true), VReg(false),
        VRegRd, Vts, VtsRd, VtsNew, VtsPrev, VState(true), VState(false), Claim, Pool, Sl8, TraderTok, FeeTok, Mint, Tok, Sys, EvAuth, SelfProg, Tally, InitCfg, InitTally,
        InitRs, InitMkt, InitWal, InitTa, Rem,
    ];
    let used: BTreeSet<Ty> = scenarios().iter().flat_map(|s| s.tys.iter().copied()).collect();
    let missing: Vec<&Ty> = all.iter().filter(|t| !used.contains(t)).collect();
    assert!(missing.is_empty(), "slot types no scenario uses: {missing:?}");
    assert_eq!(used.len(), all.len(), "a scenario uses a slot type that this list does not know");
}

/// The table runner must be able to FAIL: a wrong expectation, a refusal that changed a byte and an acceptance without a reason each
/// produce a problem; a correct row produces none.
#[test]
fn the_table_runner_reports_wrong_expectations_dirty_refusals_and_unexplained_acceptances() {
    let s = scenarios_for("set_evaluator").pop().unwrap();
    // Cfg slot wrapped in a foreign owner: refused with WrongOwner (3007)
    let rows = check_scenario(&s, &[Mu::Foreign(0), Mu::Rw(0), Mu::Missing(1)]);
    let (foreign, rw, missing) = (&rows[0], &rows[1], &rows[2]);
    assert!(foreign.problem().is_none() && foreign.want == cu(A_WRONG_OWNER), "{:?}", foreign.problem());
    assert!(missing.problem().is_none() && missing.want == pe(E::Unauthorized), "{:?}", missing.problem());
    assert!(rw.problem().is_none() && rw.want == Exp::Ok && rw.reason.is_some());
    let mk_row = |r: &Row| Row { ix: r.ix, mu: r.mu, text: r.text.clone(), want: r.want.clone(), got: r.got.clone(), dirty: r.dirty, post_ok: r.post_ok, reason: r.reason, untouched_ok: r.untouched_ok };
    let mut wrong = mk_row(foreign);
    wrong.want = cu(A_NOT_INITIALIZED);
    assert!(wrong.problem().unwrap().contains("expected"));
    let mut dirty = mk_row(foreign);
    dirty.dirty = true;
    assert!(dirty.problem().unwrap().contains("CHANGED"));
    let mut no_reason = mk_row(rw);
    no_reason.reason = None;
    assert!(no_reason.problem().unwrap().contains("no reason"));
    let mut touched = mk_row(rw);
    touched.untouched_ok = false;
    assert!(touched.problem().unwrap().contains("was changed"));
    let mut tx_level = mk_row(foreign);
    tx_level.got = Err("SignatureFailure".into());
    assert!(tx_level.problem().unwrap().contains("tx-level"));
    // the model is not trivially "Ok": over the whole scenario most rows are refusals
    let rows = check_scenario(&s, &all_mutations(&s, false));
    let refused = rows.iter().filter(|r| r.want != Exp::Ok).count();
    assert!(refused * 2 > rows.len(), "{refused} of {} rows are refusals", rows.len());
}

/// Row counts of the two modes (default: donors of a Dup are slot 0, the trading account, the config and the two neighbours;
/// `HYG_FULL=1`: every slot). Pinned, so a change of the mutation set is a deliberate edit. `HYG_COUNT=1` prints the per-instruction table.
#[test]
fn row_counts_are_pinned() {
    let mut rows_default = 0usize;
    let mut rows_full = 0usize;
    let mut lines = Vec::new();
    for s in scenarios() {
        let (d, f) = (all_mutations(&s, false).len(), all_mutations(&s, true).len());
        rows_default += d;
        rows_full += f;
        lines.push(format!("| `{}` | {} | {} | {} |", s.name, s.n(), d, f));
    }
    if std::env::var("HYG_COUNT").is_ok() {
        println!("| instruction | accounts | rows (default) | rows (HYG_FULL=1) |\n|---|---|---|---|");
        for l in &lines {
            println!("{l}");
        }
        println!("total default {rows_default}, full {rows_full}");
    }
    assert_eq!((rows_default, rows_full), (ROWS_DEFAULT, ROWS_FULL));
}

const ROWS_DEFAULT: usize = 2553; // was 2411 before nine instructions took the two event_cpi accounts (F-27)
const ROWS_FULL: usize = 3891; // was 3644


/// `HYG_REPORT=1 cargo test --offline --test account_hygiene -- --nocapture report_summary` prints the numbers quoted in
/// handover/audit/03-account-hygiene.md (rows by expected result, accepted rows by reason). Without the variable it only checks that
/// the summary can be built from one small scenario.
#[test]
fn report_summary() {
    let all = std::env::var("HYG_REPORT").is_ok();
    let names: Vec<&str> = if all { NAMES.to_vec() } else { vec!["cancel_pass"] };
    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_reason: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut rows_n, mut refused_n, mut ok_n) = (0usize, 0usize, 0usize);
    for n in names {
        let (_s, rows) = table(n);
        for r in &rows {
            rows_n += 1;
            match &r.want {
                Exp::Ok => {
                    ok_n += 1;
                    *by_reason.entry(r.reason.unwrap_or("NONE")).or_default() += 1;
                }
                Exp::Err(e) => {
                    refused_n += 1;
                    *by_code.entry(format!("{e:?}")).or_default() += 1;
                }
            }
        }
    }
    if all {
        println!("REPORT rows {rows_n} refused {refused_n} accepted {ok_n}");
        for (k, v) in &by_code {
            println!("REPORT refused-with {k}: {v}");
        }
        for (k, v) in &by_reason {
            println!("REPORT accepted-because {}: {v}", k.split(':').next().unwrap());
        }
    }
    assert_eq!(rows_n, refused_n + ok_n);
}


// ---------------------------------------------------------------------------------------------------- valid accounts in the wrong slot

/// CROSS-BINDING. The accounts below are all VALID (right owner, discriminator and layout, rent-exempt, really created by the program
/// or the vault), only they belong to somebody else or to another challenge: the vault record of the trader's OTHER challenge, the
/// wallet profile of ANOTHER trader, the claim address of the NEXT request. Each binding is checked by seeds that are rebuilt from the
/// trading account's own fields, so every one is refused: ours (ConstraintSeeds 2006 for the wallet), the vault's (2006 for the
/// record and the claim), or our own handler (InvalidVaultAccount for touch_activity / release_account, which read the record first).
/// 13 refusals over 6 instructions; state unchanged.
#[test]
fn valid_accounts_of_another_challenge_or_trader_in_the_wrong_slot_are_refused() {
    let names = ["request_advance", "touch_activity", "release_account", "report_fail", "finalize_pass", "fail_for_violation"];
    let mut refusals = 0;
    for name in names {
        for case in ["vault record of the other challenge", "wallet of another trader", "claim of the next request"] {
            let s = scenarios_for(name).pop().unwrap();
            let mut f = s.fork();
            let (other_vts, other_wallet) = {
                let w = match &mut f.world {
                    World::J(w) => w,
                    World::P(_) => unreachable!(),
                };
                let a = dup(&w.trader);
                common::assert_ok(w.v.open(&a, 8)); // the trader's SECOND account: a second vault record
                let b = w.v.new_trader();
                common::assert_ok(w.v.open(&b, 0)); // another trader with a wallet profile
                (vault_trader_state_pda(&a.pubkey(), 1), wallet_pda(&b.pubkey()))
            };
            let pos = |pred: &dyn Fn(Ty) -> bool| f.tys.iter().position(|t| pred(*t));
            let (idx, key, want) = match case {
                "vault record of the other challenge" => {
                    let i = pos(&|t| matches!(t, Ty::Vts | Ty::VtsRd)).unwrap();
                    let want = if f.tys[i] == Ty::VtsRd { pe(E::InvalidVaultAccount) } else { cu(A_CONSTRAINT_SEEDS) };
                    (i, other_vts, want)
                }
                "wallet of another trader" => (pos(&|t| t == Ty::Wal).unwrap(), other_wallet, cu(A_CONSTRAINT_SEEDS)),
                _ => {
                    let Some(i) = pos(&|t| t == Ty::Claim) else { continue };
                    let claim = f.ix.accounts[i].pubkey;
                    // the claim PDA of request id + 1 under the same vault record
                    let vts = f.ix.accounts[pos(&|t| t == Ty::Vts).unwrap()].pubkey;
                    let next = claim_pda(&vts, 2);
                    assert_ne!(claim, next);
                    (i, next, cu(A_CONSTRAINT_SEEDS))
                }
            };
            f.ix.accounts[idx].pubkey = key;
            let ix = f.ix.clone();
            let o = run(&mut f, ix);
            assert_eq!(actual_of(&o, f.pre.len()), Ok(want), "{name}: {case}");
            assert_eq!(o.before, o.after, "{name}: {case}: the refusal changed account bytes");
            refusals += 1;
        }
    }
    assert_eq!(refusals, 13);
}


/// A TRADING ACCOUNT OF ANOTHER SECTOR. A real second program (the vault, which is loaded and executable) "owns" a byte-copy of the
/// trading account: right size, right discriminator, right content. In every one of the 15 instructions that take the account, both
/// when the copy sits at the real address and at another address: `AccountOwnedByWrongProgram` (3007), nothing changes. (The generic
/// table uses an unloaded foreign owner; this is the same check against a program that exists.)
#[test]
fn a_trading_account_owned_by_another_sector_program_is_refused_by_all_fifteen_instructions() {
    let ta_names = [
        "request_advance", "cancel_pass", "confirm_pass", "reject_pass", "veto_pass", "finalize_pass", "report_fail", "post_journal", "hold_account", "release_hold",
        "fail_for_violation", "touch_activity", "release_account", "close_account", "reset_account",
    ];
    let mut rows = 0;
    for name in ta_names {
        for at_other_address in [false, true] {
            let s = scenarios_for(name).pop().unwrap();
            let mut f = s.fork();
            let i = f.tys.iter().position(|t| matches!(t, Ty::Ta { .. })).unwrap();
            let key = f.ix.accounts[i].pubkey;
            let mut acc = f.world.envr().svm.get_account(&key).unwrap();
            assert!(acc.data.len() > 8 && acc.owner == PROGRAM_ID);
            acc.owner = VAULT_ID;
            if at_other_address {
                let x = Pubkey::new_unique();
                f.world.env().svm.set_account(x, acc).unwrap();
                f.ix.accounts[i].pubkey = x;
            } else {
                f.world.env().svm.set_account(key, acc).unwrap();
            }
            let ix = f.ix.clone();
            let o = run(&mut f, ix);
            assert_eq!(actual_of(&o, f.pre.len()), Ok(cu(A_WRONG_OWNER)), "{name} (other address: {at_other_address})");
            assert_eq!(o.before, o.after, "{name}: the refusal changed account bytes");
            rows += 1;
        }
    }
    assert_eq!(rows, 30);
}
