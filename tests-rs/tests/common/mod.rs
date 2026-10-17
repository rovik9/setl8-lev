//! Shared harness for the setl8-lev-trading LiteSVM suite.
//!
//! Loads the LOCALNET (public test admin keys) build from `../target/test-deploy/`. Signature
//! verification is ON, so a signer that did not sign makes the transaction fail like on a real cluster.
#![allow(dead_code)]

pub mod evaluator;
pub mod journal;
pub mod lifecycle;
pub mod pass;
pub mod vault;

use std::path::PathBuf;

use anchor_lang::solana_program::{
    clock::Clock,
    instruction::{error::InstructionError, AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use litesvm::{types::TransactionResult, LiteSVM};
use setl8_lev_trading::{
    accounts, constants as c,
    errors::Setl8Error,
    instruction,
    instructions::{CreateMarketArgs, InitConfigArgs, MarketParams, RuleSetArgs, UpdateMarketArgs},
    state::{Market, RuleSet, SectorConfig, Session, WalletProfile},
};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

pub const T0: i64 = 1_700_000_000;
pub const PROGRAM_ID: Pubkey = setl8_lev_trading::ID;

/// Seeds of the PUBLIC TEST admin keys (the same bytes as in `constants/admin.rs`).
pub const SL8_TEST_SEED: [u8; 32] = [0x11; 32];
pub const ROV_TEST_SEED: [u8; 32] = [0x22; 32];

pub fn sl8_test_admin() -> Keypair {
    Keypair::new_from_array(SL8_TEST_SEED)
}
pub fn rov_test_admin() -> Keypair {
    Keypair::new_from_array(ROV_TEST_SEED)
}

pub fn so_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/test-deploy/setl8_lev_trading.so")
}

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[c::CONFIG_SEED], &PROGRAM_ID).0
}
pub fn tally_pda() -> Pubkey {
    setl8_shared_interfaces::derive_payout_tally(&PROGRAM_ID).0
}

pub fn dup(k: &Keypair) -> Keypair {
    Keypair::try_from(k.to_bytes().as_slice()).unwrap()
}

/// Only `target/test-deploy/` (localnet, public test admin keys) is ever loaded. A real-key build would
/// make `init_config` fail with `Unauthorized` in every test. (A byte scan for the keys in the `.so` does
/// not work: the compiler inlines the 32 bytes as immediates, they are not a contiguous blob.)
pub struct Env {
    pub svm: LiteSVM,
    pub sl8: Keypair,
    pub rov: Keypair,
    pub payer: Keypair,
    /// Shape of the last transaction `send` built (M11 budget measurements): serialized bytes, distinct accounts, signatures.
    pub last_tx: TxShape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxShape {
    pub bytes: usize,
    pub accounts: usize,
    pub signatures: usize,
}

impl Env {
    pub fn new() -> Self {
        Self::new_with(true)
    }

    /// `sigverify = false` lets a test flag an account as a signer without a signature (the vault harness uses it
    /// for the vault's public test admins, whose key files are never read).
    pub fn new_with(sigverify: bool) -> Self {
        let so = so_path();
        let mut svm = LiteSVM::new().with_sigverify(sigverify);
        svm.add_program_from_file(PROGRAM_ID, &so).unwrap_or_else(|e| panic!("cannot load {} ({e:?})", so.display()));
        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = T0;
        svm.set_sysvar(&clock);
        let (sl8, rov, payer) = (sl8_test_admin(), rov_test_admin(), Keypair::new());
        assert_eq!(sl8.pubkey(), c::SL8_ADMIN, "test seed / constant drift");
        assert_eq!(rov.pubkey(), c::ROV_ADMIN, "test seed / constant drift");
        for k in [&sl8, &rov, &payer] {
            svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        }
        Self { svm, sl8, rov, payer, last_tx: TxShape::default() }
    }

    /// Environment with `init_config` already run with `default_args()` by the two test admins.
    pub fn initialised() -> Self {
        let mut e = Self::new();
        let ix = e.init_ix(&default_args());
        e.send_as_admins(&[ix]).expect("init_config must succeed");
        e
    }

    /// Fee payer = `payer`; `extra` sign only if the message requires them.
    pub fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> TransactionResult {
        let bh = self.svm.latest_blockhash();
        let payer = dup(&self.payer);
        let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &bh);
        let mut tx = Transaction::new_unsigned(msg);
        let n = tx.message.header.num_required_signatures as usize;
        let required: Vec<Pubkey> = tx.message.account_keys[..n].to_vec();
        let mut sigs: Vec<&Keypair> = vec![&payer];
        sigs.extend(extra.iter().copied().filter(|k| required.contains(&k.pubkey())));
        tx.partial_sign(sigs.as_slice(), bh);
        self.last_tx = TxShape { bytes: bincode::serialize(&tx).map(|b| b.len()).unwrap_or(0), accounts: tx.message.account_keys.len(), signatures: n };
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        r
    }

    /// Sets the cluster clock (unix seconds).
    pub fn set_clock(&mut self, t: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = t;
        self.svm.set_sysvar(&clock);
    }

    /// Both test admins sign.
    pub fn send_as_admins(&mut self, ixs: &[Instruction]) -> TransactionResult {
        let (a, b) = (dup(&self.sl8), dup(&self.rov));
        self.send(ixs, &[&a, &b])
    }

    pub fn init_ix(&self, args: &InitConfigArgs) -> Instruction {
        init_ix_with(self.payer.pubkey(), self.sl8.pubkey(), self.rov.pubkey(), args)
    }

    pub fn config(&self) -> SectorConfig {
        let acc = self.svm.get_account(&config_pda()).expect("config account exists");
        SectorConfig::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }
}

pub fn default_args() -> InitConfigArgs {
    InitConfigArgs {
        evaluator: Pubkey::new_from_array([7; 32]),
        vault_program: Pubkey::new_from_array([8; 32]),
        sl8_wallet: Pubkey::new_from_array([9; 32]),
        pass_delay_secs: c::DEFAULT_PASS_DELAY_SECS,
        max_payout_per_window: 5_000_000_000,
        window_secs: c::DEFAULT_WINDOW_SECS,
    }
}

pub fn init_ix_with(payer: Pubkey, sl8_admin: Pubkey, rov_admin: Pubkey, args: &InitConfigArgs) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::InitConfig {
            payer,
            sl8_admin,
            rov_admin,
            config: config_pda(),
            payout_tally: tally_pda(),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: instruction::InitConfig { args: args.clone() }.data(),
    }
}

/// Admin-action accounts with explicit keys and signer flags (to model "only one admin signed").
pub fn admin_metas(admin_a: (Pubkey, bool), admin_b: (Pubkey, bool)) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(config_pda(), false),
        AccountMeta { pubkey: admin_a.0, is_signer: admin_a.1, is_writable: false },
        AccountMeta { pubkey: admin_b.0, is_signer: admin_b.1, is_writable: false },
    ]
}

pub fn set_evaluator_ix(metas: Vec<AccountMeta>, new_key: Pubkey) -> Instruction {
    Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::SetEvaluator { new_key }.data() }
}

pub fn set_limits_ix(metas: Vec<AccountMeta>, delay: u32, cap: u64, window: u32) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: metas,
        data: instruction::SetLimits { pass_delay_secs: delay, max_payout_per_window: cap, window_secs: window }.data(),
    }
}

pub fn set_paused_ix(metas: Vec<AccountMeta>, paused: bool) -> Instruction {
    Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::SetPaused { paused }.data() }
}

/// Both stored admins, both flagged as signers.
pub fn both(e: &Env) -> Vec<AccountMeta> {
    admin_metas((e.sl8.pubkey(), true), (e.rov.pubkey(), true))
}
/// Only SL8 signs (Rov present but not a signer).
pub fn only_sl8(e: &Env) -> Vec<AccountMeta> {
    admin_metas((e.sl8.pubkey(), true), (e.rov.pubkey(), false))
}
/// Only Rov signs (SL8 present but not a signer).
pub fn only_rov(e: &Env) -> Vec<AccountMeta> {
    admin_metas((e.sl8.pubkey(), false), (e.rov.pubkey(), true))
}

fn instruction_error(r: &TransactionResult) -> &InstructionError {
    match r {
        Ok(m) => panic!("expected failure but the transaction succeeded\nlogs:\n{}", m.logs.join("\n")),
        Err(f) => match &f.err {
            TransactionError::InstructionError(0, e) => e,
            other => panic!("expected an instruction-0 error, got {other:?}\nlogs:\n{}", f.meta.logs.join("\n")),
        },
    }
}

/// Exact program error on instruction 0.
pub fn assert_err(r: &TransactionResult, want: Setl8Error) {
    let want_code = u32::from(want);
    match instruction_error(r) {
        InstructionError::Custom(code) => assert_eq!(*code, want_code, "wrong error code, want {want_code}"),
        other => panic!("expected Custom({want_code}), got {other:?}"),
    }
}

/// Anchor framework error code on instruction 0 (e.g. 3010 AccountNotSigner).
pub fn assert_anchor_code(r: &TransactionResult, want: u32) {
    match instruction_error(r) {
        InstructionError::Custom(code) => assert_eq!(*code, want, "wrong anchor error code"),
        other => panic!("expected Custom({want}), got {other:?}"),
    }
}

pub fn assert_ok(r: TransactionResult) {
    if let Err(f) = r {
        panic!("expected success, got {:?}\nlogs:\n{}", f.err, f.meta.logs.join("\n"));
    }
}


// ------------------------------------------------------------------ M1b helpers

pub fn ruleset_pda(version: u16) -> Pubkey {
    Pubkey::find_program_address(&[c::RULESET_SEED, &version.to_le_bytes()], &PROGRAM_ID).0
}
pub fn market_pda(id: u16) -> Pubkey {
    Pubkey::find_program_address(&[c::MARKET_SEED, &id.to_le_bytes()], &PROGRAM_ID).0
}
pub fn wallet_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[c::WALLET_SEED, trader.as_ref()], &PROGRAM_ID).0
}

/// Who is named in the admin slots of an admin instruction and who really signs.
#[derive(Clone, Copy)]
pub struct Admins {
    pub a: Pubkey,
    pub b: Pubkey,
    pub a_signs: bool,
    pub b_signs: bool,
}

impl Admins {
    pub fn both(e: &Env) -> Self {
        Self { a: e.sl8.pubkey(), b: e.rov.pubkey(), a_signs: true, b_signs: true }
    }
    pub fn only_sl8(e: &Env) -> Self {
        Self { b_signs: false, ..Self::both(e) }
    }
    pub fn only_rov(e: &Env) -> Self {
        Self { a_signs: false, ..Self::both(e) }
    }
    pub fn neither(e: &Env) -> Self {
        Self { a_signs: false, b_signs: false, ..Self::both(e) }
    }
    pub fn strangers(x: &Keypair, y: &Keypair) -> Self {
        Self { a: x.pubkey(), b: y.pubkey(), a_signs: true, b_signs: true }
    }
    fn patch(&self, mut ix: Instruction) -> Instruction {
        for m in ix.accounts.iter_mut() {
            if m.pubkey == self.a {
                m.is_signer = self.a_signs;
            } else if m.pubkey == self.b {
                m.is_signer = self.b_signs;
            }
        }
        ix
    }
}

pub fn create_ruleset_ix(payer: Pubkey, admins: Admins, args: &RuleSetArgs) -> Instruction {
    admins.patch(Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::CreateRuleset {
            payer,
            config: config_pda(),
            admin_a: admins.a,
            admin_b: admins.b,
            ruleset: ruleset_pda(args.version),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: instruction::CreateRuleset { args: args.clone() }.data(),
    })
}

pub fn create_market_ix(payer: Pubkey, admins: Admins, args: &CreateMarketArgs) -> Instruction {
    admins.patch(Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::CreateMarket {
            payer,
            config: config_pda(),
            admin_a: admins.a,
            admin_b: admins.b,
            market: market_pda(args.market_id),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: instruction::CreateMarket { args: args.clone() }.data(),
    })
}

pub fn update_market_ix(admins: Admins, args: &UpdateMarketArgs) -> Instruction {
    admins.patch(Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::UpdateMarket {
            config: config_pda(),
            admin_a: admins.a,
            admin_b: admins.b,
            market: market_pda(args.market_id),
        }
        .to_account_metas(None),
        data: instruction::UpdateMarket { args: args.clone() }.data(),
    })
}

pub fn ban_wallet_ix(payer: Pubkey, admins: Admins, trader: Pubkey, evidence_hash: [u8; 32]) -> Instruction {
    admins.patch(Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::BanWallet {
            payer,
            config: config_pda(),
            admin_a: admins.a,
            admin_b: admins.b,
            wallet: wallet_pda(&trader),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: instruction::BanWallet { trader, evidence_hash }.data(),
    })
}

pub fn unban_wallet_ix(admins: Admins, trader: Pubkey) -> Instruction {
    admins.patch(Instruction {
        program_id: PROGRAM_ID,
        accounts: accounts::UnbanWallet {
            config: config_pda(),
            admin_a: admins.a,
            admin_b: admins.b,
            wallet: wallet_pda(&trader),
        }
        .to_account_metas(None),
        data: instruction::UnbanWallet { trader }.data(),
    })
}

/// DESIGN 5.4 launch values (decisions 13, 15, 17, 26). Commission and spread are PENDING numbers: zeros.
pub fn launch_ruleset_args() -> RuleSetArgs {
    const USD: u64 = 1_000_000;
    let mut tier_size = [0u64; 10];
    let mut tier_cost = [0u64; 10];
    let sizes = [2_500u64, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
    let costs = [30u64, 55, 99, 145, 189, 235, 489, 689, 859, 1_599];
    for i in 0..10 {
        tier_size[i] = sizes[i] * USD;
        tier_cost[i] = costs[i] * USD;
    }
    RuleSetArgs {
        version: 1,
        fee_bps: 40,
        tier_count: 10,
        ladder_len: 5,
        leverage_x: [50, 20, 20, 20, 10, 5, 5, 2],
        holding_fee_e6: [100, 200, 400, 200, 400, 400, 1000, 1600],
        commission_e6: [0; 8],
        tier_size,
        tier_cost,
        reset_ladder_bps: [10_000, 11_500, 12_000, 12_750, 13_000],
        spread_e6: [0; 128],
    }
}

/// A valid market parameter set; `feed` makes the feed id distinguishable.
pub fn market_params(feed: u8) -> MarketParams {
    let mut sessions = [Session::default(); 6];
    sessions[0] = Session { start_min: 0, end_min: 7_200 };
    sessions[1] = Session { start_min: 7_500, end_min: 10_080 };
    MarketParams {
        status: 1,
        feed_id: [feed; 32],
        max_age_secs: 60,
        qty_step: 1_000,
        qty_min: 10_000,
        max_notional_usd: 500_000_000_000,
        session_count: 2,
        sessions,
        quote_conv_market: c::NO_QUOTE_CONV,
    }
}

pub fn create_market_args(id: u16, class: u8, feed: u8) -> CreateMarketArgs {
    CreateMarketArgs { market_id: id, class, params: market_params(feed) }
}

impl Env {
    pub fn ruleset(&self, version: u16) -> RuleSet {
        let acc = self.svm.get_account(&ruleset_pda(version)).expect("rule set exists");
        RuleSet::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }
    pub fn market(&self, id: u16) -> Market {
        let acc = self.svm.get_account(&market_pda(id)).expect("market exists");
        Market::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }
    pub fn wallet(&self, trader: &Pubkey) -> WalletProfile {
        let acc = self.svm.get_account(&wallet_pda(trader)).expect("wallet profile exists");
        WalletProfile::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }

    /// Sends with the signers `admins` asks for (plus the payer).
    pub fn send_admins(&mut self, ix: Instruction, admins: Admins) -> TransactionResult {
        let (sl8, rov) = (dup(&self.sl8), dup(&self.rov));
        let mut signers: Vec<&Keypair> = Vec::new();
        if admins.a_signs && admins.a == sl8.pubkey() {
            signers.push(&sl8);
        }
        if admins.b_signs && admins.b == rov.pubkey() {
            signers.push(&rov);
        }
        self.send(&[ix], &signers)
    }

    pub fn create_ruleset_ok(&mut self, args: &RuleSetArgs) {
        let ix = create_ruleset_ix(self.payer.pubkey(), Admins::both(self), args);
        let a = Admins::both(self);
        assert_ok(self.send_admins(ix, a));
    }

    pub fn create_market_ok(&mut self, args: &CreateMarketArgs) {
        let ix = create_market_ix(self.payer.pubkey(), Admins::both(self), args);
        let a = Admins::both(self);
        assert_ok(self.send_admins(ix, a));
    }
}

/// All events of type `T` found in a successful transaction's logs, in order.
pub fn events_of<T: anchor_lang::Event + anchor_lang::AnchorDeserialize>(logs: &[String]) -> Vec<T> {
    use base64::Engine;
    let mut out = Vec::new();
    for line in logs {
        if let Some(b64) = line.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                if bytes.len() >= 8 && bytes[..8] == *T::DISCRIMINATOR {
                    out.push(T::deserialize(&mut &bytes[8..]).expect("event decodes"));
                }
            }
        }
    }
    out
}

/// Anchor error 3012: account not initialized (e.g. `unban_wallet` on a wallet without a profile).
pub const ACCOUNT_NOT_INITIALIZED: u32 = 3012;
