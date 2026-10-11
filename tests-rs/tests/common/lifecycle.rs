//! M7 harness: builders and senders for `report_fail`, `hold_account`, `release_hold`, `fail_for_violation`,
//! `touch_activity`, `release_account`, `close_account`, plus a failing-batch builder and a second-account helper.
#![allow(dead_code)]

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey};
use anchor_lang::{InstructionData, ToAccountMetas};
use journal_types::{FailReason, JournalBatch, JournalEvent};
use litesvm::types::TransactionResult;
use setl8_lev_trading::{accounts, instruction};
use setl8_shared_interfaces as si;
use solana_keypair::Keypair;
use solana_signer::Signer;

use super::journal::*;
use super::pass::*;
use super::vault::*;
use super::*;

/// Account indexes of `report_fail` / `fail_for_violation` instructions (for swap attacks).
pub mod rf {
    pub const EVALUATOR: usize = 0;
    pub const TRADING_ACCOUNT: usize = 1;
    pub const CONFIG: usize = 2;
    pub const RULESET: usize = 3;
    pub const WALLET: usize = 4;
    pub const SECTOR_AUTHORITY: usize = 5;
    pub const VAULT_PROGRAM: usize = 6;
    pub const REGISTRY: usize = 7;
    pub const VAULT_TRADER_STATE: usize = 8;
}
pub mod fv {
    pub const CONFIG: usize = 0;
    pub const ADMIN_A: usize = 1;
    pub const ADMIN_B: usize = 2;
    pub const TRADING_ACCOUNT: usize = 3;
    pub const WALLET: usize = 4;
    pub const SECTOR_AUTHORITY: usize = 5;
    pub const VAULT_PROGRAM: usize = 6;
    pub const REGISTRY: usize = 7;
    pub const VAULT_TRADER_STATE: usize = 8;
}

pub fn fail_ev(t: i64, reason: FailReason, b_after: i64, proof: u8) -> JournalEvent {
    JournalEvent::Fail { time: t, reason, b_after, proof }
}

/// 92,000 / 96,000 (the static floors of the $100k account) in micro-USD.
pub const FLOOR: i64 = 92_000_000_000;
pub const LATCHED_FLOOR: i64 = 96_000_000_000;
/// 0.028 S of the $100k account.
pub const QUICK_LIMIT: i64 = 2_800_000_000;
pub const RETENTION: i64 = 604_800;

impl JEnv {
    fn base_ix_metas_wallet(&self) -> Pubkey {
        wallet_pda(&self.account().trader)
    }

    pub fn report_fail_ix(&self, signer: Pubkey, bytes: Vec<u8>, reason: u8, markets: &[u16]) -> Instruction {
        let a = self.account();
        let mut metas = accounts::ReportFail {
            evaluator: signer,
            trading_account: self.acct,
            config: config_pda(),
            ruleset: ruleset_pda(a.ruleset_version),
            wallet: self.base_ix_metas_wallet(),
            sector_authority: sector_authority_pda(),
            vault_program: VAULT_ID,
            product_registry: registry_pda(),
            vault_trader_state: self.vts(),
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(JEnv::market_metas(markets));
        Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::ReportFail { batch_bytes: bytes, reason }.data() }
    }

    /// `report_fail` signed by the evaluator at cluster time `clock`.
    pub fn report_fail(&mut self, clock: i64, batch: &JournalBatch, reason: u8, markets: &[u16]) -> TransactionResult {
        self.set_time(clock);
        let ev = dup(&self.v.evaluator);
        let ix = self.report_fail_ix(ev.pubkey(), batch.to_bytes().expect("encodes"), reason, markets);
        self.v.e.send(&[ix], &[&ev])
    }

    /// A failing batch: `pre..., Fail{reason, b_after, proof 0}, Meta(flat)` with one EUR proof at `t`.
    pub fn fail_batch(&self, t: i64, mut pre: Vec<JournalEvent>, reason: FailReason, b_after: i64, flat: bool) -> JournalBatch {
        pre.push(fail_ev(t, reason, b_after, 0));
        pre.push(meta(t, flat));
        self.batch(pre, vec![p_eur(t)])
    }

    pub fn hold_ix(&self, signer: Pubkey, hash: [u8; 32]) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::HoldAccount {
                evaluator: signer,
                trading_account: self.acct,
                wallet: wallet_pda(&self.account().trader),
                config: config_pda(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::HoldAccount { evidence_hash: hash }.data(),
        }
    }

    pub fn hold(&mut self, t: i64, hash: [u8; 32]) -> TransactionResult {
        self.set_time(t);
        let ev = dup(&self.v.evaluator);
        let ix = self.hold_ix(ev.pubkey(), hash);
        self.v.e.send(&[ix], &[&ev])
    }

    fn patch_admins(ix: &mut Instruction, admins: Admins) {
        for m in ix.accounts.iter_mut() {
            if m.pubkey == admins.a {
                m.is_signer = admins.a_signs;
            } else if m.pubkey == admins.b {
                m.is_signer = admins.b_signs;
            }
        }
    }

    pub fn release_hold_ix(&self, admins: Admins) -> Instruction {
        let mut ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::ReleaseHold {
                config: config_pda(),
                admin_a: admins.a,
                admin_b: admins.b,
                trading_account: self.acct,
                wallet: wallet_pda(&self.account().trader),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::ReleaseHold {}.data(),
        };
        Self::patch_admins(&mut ix, admins);
        ix
    }

    pub fn release_hold(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let a = Admins::both(&self.v.e);
        let ix = self.release_hold_ix(a);
        self.v.e.send_admins(ix, a)
    }

    pub fn violation_ix(&self, admins: Admins, hash: [u8; 32]) -> Instruction {
        let mut ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::FailForViolation {
                config: config_pda(),
                admin_a: admins.a,
                admin_b: admins.b,
                trading_account: self.acct,
                wallet: wallet_pda(&self.account().trader),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: self.vts(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::FailForViolation { evidence_hash: hash }.data(),
        };
        Self::patch_admins(&mut ix, admins);
        ix
    }

    pub fn violation(&mut self, t: i64, hash: [u8; 32]) -> TransactionResult {
        self.set_time(t);
        let a = Admins::both(&self.v.e);
        let ix = self.violation_ix(a, hash);
        self.v.e.send_admins(ix, a)
    }

    pub fn touch_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::TouchActivity {
                evaluator: signer,
                trading_account: self.acct,
                wallet: wallet_pda(&self.account().trader),
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: self.vts(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::TouchActivity {}.data(),
        }
    }

    pub fn touch(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let ev = dup(&self.v.evaluator);
        let ix = self.touch_ix(ev.pubkey());
        self.v.e.send(&[ix], &[&ev])
    }

    pub fn release_acct_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::ReleaseAccount {
                trader: signer,
                trading_account: self.acct,
                wallet: wallet_pda(&self.account().trader),
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: self.vts(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::ReleaseAccount {}.data(),
        }
    }

    pub fn release_acct(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let tr = dup(&self.trader);
        let ix = self.release_acct_ix(tr.pubkey());
        self.v.e.send(&[ix], &[&tr])
    }

    pub fn close_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::CloseAccount { trader: signer, trading_account: self.acct }.to_account_metas(None),
            data: instruction::CloseAccount {}.data(),
        }
    }

    pub fn close(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let tr = dup(&self.trader);
        let ix = self.close_ix(tr.pubkey());
        self.v.e.send(&[ix], &[&tr])
    }

    /// The vault's permissionless `mark_abandoned` for this challenge (anyone calls it; the payer signs).
    pub fn vault_mark_abandoned(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let a = self.account();
        let ix = si::mark_abandoned(
            VAULT_ID,
            self.v.e.payer.pubkey(),
            registry_pda(),
            &[anchor_lang::solana_program::instruction::AccountMeta::new(self.vts(), false)],
            si::MarkAbandonedArgs { trader_wallet: a.trader, product_program_id: PROGRAM_ID, challenge_id: a.challenge_id },
        );
        self.v.e.send(&[ix], &[])
    }

    /// Opens another account for the same trader and makes it the active one (`self.acct`). Returns its challenge id.
    pub fn open_another(&mut self) -> u64 {
        let tr = dup(&self.trader);
        assert_ok(self.v.open(&tr, 8));
        let id = self.v.wallet_opt(&tr.pubkey()).unwrap().next_challenge_id - 1;
        self.acct = acct_pda(&tr.pubkey(), id);
        id
    }

    /// A terminal account for the close tests: status, terminal_at.
    pub fn make_terminal(&mut self, status: u8, at: i64) {
        self.edit_account(|a| {
            a.status = status;
            a.pass_pending = 0;
            a.pass_proposed_at = at;
        });
    }
}

pub fn no_hash() -> [u8; 32] {
    [0u8; 32]
}
pub fn hash(n: u8) -> [u8; 32] {
    [n; 32]
}
pub fn some_key() -> Keypair {
    Keypair::new()
}
