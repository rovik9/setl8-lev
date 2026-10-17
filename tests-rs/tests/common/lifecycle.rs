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

// ====================================================================== M8: reset_account

/// Account indexes of the `reset_account` instruction (for swap attacks).
pub mod ra {
    pub const TRADER: usize = 0;
    pub const PAYER: usize = 1;
    pub const CONFIG: usize = 2;
    pub const RULESET: usize = 3;
    pub const WALLET: usize = 4;
    pub const PREV_ACCOUNT: usize = 5;
    pub const NEW_ACCOUNT: usize = 6;
    pub const SECTOR_AUTHORITY: usize = 7;
    pub const VAULT_PROGRAM: usize = 8;
    pub const REGISTRY: usize = 9;
    pub const VAULT_TRADER_STATE: usize = 10;
    pub const PREV_VAULT_TRADER_STATE: usize = 11;
    pub const VAULT_STATE: usize = 12;
    pub const TRADER_TOKEN: usize = 13;
    pub const MINT: usize = 14;
    pub const POOL: usize = 15;
    pub const VAULT_SL8: usize = 16;
    pub const FEE_TOKEN: usize = 17;
    pub const TOKEN_PROGRAM: usize = 18;
    pub const SYSTEM_PROGRAM: usize = 19;
}

/// Every account of `reset_account`, with the defaults a correct call uses; tests override single fields.
#[derive(Clone)]
pub struct ResetAccts {
    pub trader: Pubkey,
    pub payer: Pubkey,
    pub config: Pubkey,
    pub ruleset: Pubkey,
    pub wallet: Pubkey,
    pub prev_trading_account: Pubkey,
    pub trading_account: Pubkey,
    pub sector_authority: Pubkey,
    pub vault_program: Pubkey,
    pub product_registry: Pubkey,
    pub vault_trader_state: Pubkey,
    pub prev_vault_trader_state: Pubkey,
    pub vault_state: Pubkey,
    pub trader_token_account: Pubkey,
    pub mint: Pubkey,
    pub pool_token_account: Pubkey,
    pub vault_sl8_token_account: Pubkey,
    pub fee_token_account: Pubkey,
    pub token_program: Pubkey,
    pub system_program: Pubkey,
    pub trader_signs: bool,
    pub payer_signs: bool,
}

impl ResetAccts {
    pub fn to_ix(&self, prev_challenge_id: u64) -> Instruction {
        let mut metas = accounts::ResetAccount {
            trader: self.trader,
            payer: self.payer,
            config: self.config,
            ruleset: self.ruleset,
            wallet: self.wallet,
            prev_trading_account: self.prev_trading_account,
            trading_account: self.trading_account,
            sector_authority: self.sector_authority,
            vault_program: self.vault_program,
            product_registry: self.product_registry,
            vault_trader_state: self.vault_trader_state,
            prev_vault_trader_state: self.prev_vault_trader_state,
            vault_state: self.vault_state,
            trader_token_account: self.trader_token_account,
            mint: self.mint,
            pool_token_account: self.pool_token_account,
            vault_sl8_token_account: self.vault_sl8_token_account,
            fee_token_account: self.fee_token_account,
            token_program: self.token_program,
            system_program: self.system_program,
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        for (i, signs) in [(ra::TRADER, self.trader_signs), (ra::PAYER, self.payer_signs)] {
            metas[i] = anchor_lang::solana_program::instruction::AccountMeta { is_signer: signs, ..metas[i].clone() };
        }
        Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::ResetAccount { prev_challenge_id }.data() }
    }
}

/// 100,000 USDC for every pass-through test: the trader starts with `TRADER_START`.
impl JEnv {
    /// The accounts of a correct `reset_account` of the CURRENT account (`self.acct`), which must be Failed.
    pub fn reset_accts(&self) -> ResetAccts {
        let prev = self.account();
        let trader = prev.trader;
        let next_id = self.v.wallet_opt(&trader).map_or(0, |w| w.next_challenge_id);
        ResetAccts {
            trader,
            payer: trader,
            config: config_pda(),
            ruleset: ruleset_pda(1),
            wallet: wallet_pda(&trader),
            prev_trading_account: self.acct,
            trading_account: acct_pda(&trader, next_id),
            sector_authority: sector_authority_pda(),
            vault_program: VAULT_ID,
            product_registry: registry_pda(),
            vault_trader_state: vault_trader_state_pda(&trader, next_id),
            prev_vault_trader_state: vault_trader_state_pda(&trader, prev.challenge_id),
            vault_state: self.v.vault_state,
            trader_token_account: self.v.trader_token(&trader),
            mint: self.v.usdc,
            pool_token_account: self.v.usdc_pool,
            vault_sl8_token_account: self.v.vault_sl8_usdc,
            fee_token_account: self.v.fee_usdc,
            token_program: anchor_spl::token::spl_token::ID,
            system_program: anchor_lang::solana_program::system_program::ID,
            trader_signs: true,
            payer_signs: true,
        }
    }

    /// Sends `reset_account` for `prev_challenge_id` at clock `t`, signed by the trader.
    pub fn send_reset(&mut self, t: i64, ra: &ResetAccts, prev_challenge_id: u64) -> TransactionResult {
        self.set_time(t);
        let ix = ra.to_ix(prev_challenge_id);
        let tr = dup(&self.trader);
        self.v.e.send(&[ix], &[&tr])
    }

    /// A correct `reset_account` of `self.acct` at clock `t`.
    pub fn reset(&mut self, t: i64) -> TransactionResult {
        let ra = self.reset_accts();
        let prev_id = self.account().challenge_id;
        self.send_reset(t, &ra, prev_id)
    }

    /// A correct reset that must succeed; `self.acct` becomes the NEW account. Returns the transaction metadata.
    pub fn reset_ok(&mut self, t: i64) -> litesvm::types::TransactionMetadata {
        let ra = self.reset_accts();
        let m = self.ok(|w| w.reset(t));
        self.acct = ra.trading_account;
        m
    }

    /// Fails the account in `phase` through the real `report_fail` at the static floor (`b` is set to the floor first; the
    /// phase is edited in, as the other lifecycle tests do). `t` is the batch time; the clock is `t + 1`.
    pub fn fail_in(&mut self, phase: u8, t: i64) {
        // F_s = 0.92 S of THIS account (the $2.5K tier has a different floor than the $100K one)
        let floor = i64::try_from(self.account().size_s / 100 * 92).unwrap();
        self.edit_account(|a| {
            a.phase = phase;
            a.b = floor;
        });
        let b = self.fail_batch(t, vec![], FailReason::StaticFloor, floor, true);
        let m = self.report_fail(t + 1, &b, 0, &[MARKET_EUR]);
        assert_ok(m);
        assert_eq!(self.account().status, setl8_lev_trading::constants::STATUS_FAILED);
    }

    /// Moves a fresh account to the start of `phase` with REAL instructions: for every phase before it the account is made
    /// to meet the pass condition (`ready`), requested, confirmed and (phases 2 to 5) finalized after the delay, so the vault
    /// really pays and counts each payout. Returns the clock after the last step.
    pub fn advance_to(&mut self, phase: u8, mut t: i64, payer: &Keypair, delay: i64) -> i64 {
        for p in 1..phase {
            self.ready(p);
            self.ok(|w| w.request(t));
            self.ok(|w| w.confirm(t + 1));
            if p >= 2 {
                t += 1 + delay;
                self.ok(|w| w.finalize(t, payer));
            }
            t += 100;
        }
        assert_eq!(self.account().phase, phase);
        t
    }
}
