//! Shared harness for the setl8-lev-trading LiteSVM suite.
//!
//! Loads the LOCALNET (public test admin keys) build from `../target/test-deploy/`. Signature
//! verification is ON, so a signer that did not sign makes the transaction fail like on a real cluster.
#![allow(dead_code)]

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
    accounts, constants as c, errors::Setl8Error, instruction, instructions::InitConfigArgs,
    state::SectorConfig,
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
}

impl Env {
    pub fn new() -> Self {
        let so = so_path();
        let mut svm = LiteSVM::new().with_sigverify(true);
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
        Self { svm, sl8, rov, payer }
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
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        r
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
