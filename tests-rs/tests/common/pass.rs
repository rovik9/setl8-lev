//! Pass-flow test harness (M6): instruction builders for `request_advance`, `cancel_pass`, `confirm_pass`, `reject_pass`,
//! `veto_pass`, `finalize_pass` on top of `JEnv`, readers for the vault claims and the payout tally, a vault pause helper
//! and a "fails with exactly this error and changes nothing" wrapper.
#![allow(dead_code)]

use anchor_lang::solana_program::{
    instruction::Instruction,
    pubkey::Pubkey,
    system_program,
};
use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use litesvm::types::{TransactionMetadata, TransactionResult};
use setl8_lev_trading::{accounts, errors::Setl8Error, instruction};
use setl8_shared_interfaces as si;
use solana_keypair::Keypair;
use solana_signer::Signer;

use super::journal::*;
use super::vault::*;
use super::*;

pub fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

/// A vault `PayoutClaim` as stored (decoded by hand from its 154-byte layout).
#[derive(Debug, PartialEq, Eq)]
pub struct VaultClaim {
    pub trader_wallet: Pubkey,
    pub trader_state: Pubkey,
    pub request_id: u64,
    pub owed: u64,
}

/// Every account of `finalize_pass`, with the defaults a correct call uses; tests override single fields.
#[derive(Clone)]
pub struct FinalizeAccts {
    pub payer: Pubkey,
    pub trading_account: Pubkey,
    pub config: Pubkey,
    pub payout_tally: Pubkey,
    pub wallet: Pubkey,
    pub sector_authority: Pubkey,
    pub vault_program: Pubkey,
    pub product_registry: Pubkey,
    pub vault_trader_state: Pubkey,
    pub vault_state: Pubkey,
    pub payout_claim: Pubkey,
    pub system_program: Pubkey,
}

impl FinalizeAccts {
    pub fn to_ix(&self) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::FinalizePass {
                payer: self.payer,
                trading_account: self.trading_account,
                config: self.config,
                payout_tally: self.payout_tally,
                wallet: self.wallet,
                sector_authority: self.sector_authority,
                vault_program: self.vault_program,
                product_registry: self.product_registry,
                vault_trader_state: self.vault_trader_state,
                vault_state: self.vault_state,
                payout_claim: self.payout_claim,
                system_program: self.system_program,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::FinalizePass {}.data(),
        }
    }
}

/// All events of type `T` emitted through `emit_cpi!` (inner instructions to our program).
pub fn cpi_events<T: Discriminator + anchor_lang::AnchorDeserialize>(meta: &TransactionMetadata) -> Vec<T> {
    let mut out = Vec::new();
    for group in &meta.inner_instructions {
        for inner in group {
            let d = &inner.instruction.data;
            if d.len() > 16 && d[..8] == *anchor_lang::event::EVENT_IX_TAG_LE && d[8..16] == *T::DISCRIMINATOR {
                out.push(T::deserialize(&mut &d[16..]).expect("event decodes"));
            }
        }
    }
    out
}

pub fn claim_pda(trader_state: &Pubkey, request_id: u64) -> Pubkey {
    si::derive_payout_claim(&VAULT_ID, trader_state, request_id).0
}

impl JEnv {
    pub fn set_time(&mut self, t: i64) {
        self.v.e.set_clock(t);
    }

    pub fn vts(&self) -> Pubkey {
        vault_trader_state_pda(&self.trader.pubkey(), self.account().challenge_id)
    }

    // ------------------------------------------------------------- instruction builders

    pub fn request_ix(&self) -> Instruction {
        let a = self.account();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::RequestAdvance {
                trader: self.trader.pubkey(),
                trading_account: self.acct,
                wallet: wallet_pda(&a.trader),
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: self.vts(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::RequestAdvance {}.data(),
        }
    }

    pub fn cancel_ix(&self) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::CancelPass { trader: self.trader.pubkey(), trading_account: self.acct }.to_account_metas(None),
            data: instruction::CancelPass {}.data(),
        }
    }

    pub fn confirm_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::ConfirmPass {
                evaluator: signer,
                trading_account: self.acct,
                config: config_pda(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::ConfirmPass {}.data(),
        }
    }

    pub fn reject_ix(&self, signer: Pubkey, reason: u8) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::RejectPass {
                evaluator: signer,
                trading_account: self.acct,
                config: config_pda(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::RejectPass { reason }.data(),
        }
    }

    pub fn veto_ix(&self, admins: Admins) -> Instruction {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: accounts::VetoPass {
                config: config_pda(),
                admin_a: admins.a,
                admin_b: admins.b,
                trading_account: self.acct,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::VetoPass {}.data(),
        };
        let mut ix = ix;
        for m in ix.accounts.iter_mut() {
            if m.pubkey == admins.a {
                m.is_signer = admins.a_signs;
            } else if m.pubkey == admins.b {
                m.is_signer = admins.b_signs;
            }
        }
        ix
    }

    pub fn finalize_accts(&self, payer: Pubkey) -> FinalizeAccts {
        let a = self.account();
        let vts = self.vts();
        FinalizeAccts {
            payer,
            trading_account: self.acct,
            config: config_pda(),
            payout_tally: tally_pda(),
            wallet: wallet_pda(&a.trader),
            sector_authority: sector_authority_pda(),
            vault_program: VAULT_ID,
            product_registry: registry_pda(),
            vault_trader_state: vts,
            vault_state: self.v.vault_state,
            payout_claim: claim_pda(&vts, a.next_request_id),
            system_program: system_program::ID,
        }
    }

    // ------------------------------------------------------------- senders (set the clock, sign for real)

    pub fn request(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let (ix, tr) = (self.request_ix(), dup(&self.trader));
        self.v.e.send(&[ix], &[&tr])
    }

    pub fn cancel(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let (ix, tr) = (self.cancel_ix(), dup(&self.trader));
        self.v.e.send(&[ix], &[&tr])
    }

    pub fn confirm(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let ev = dup(&self.v.evaluator);
        let ix = self.confirm_ix(ev.pubkey());
        self.v.e.send(&[ix], &[&ev])
    }

    pub fn reject(&mut self, t: i64, reason: u8) -> TransactionResult {
        self.set_time(t);
        let ev = dup(&self.v.evaluator);
        let ix = self.reject_ix(ev.pubkey(), reason);
        self.v.e.send(&[ix], &[&ev])
    }

    pub fn veto(&mut self, t: i64) -> TransactionResult {
        self.set_time(t);
        let admins = Admins::both(&self.v.e);
        let ix = self.veto_ix(admins);
        self.v.e.send_admins(ix, admins)
    }

    /// `finalize_pass` paid by `payer` (any keypair with lamports).
    pub fn finalize(&mut self, t: i64, payer: &Keypair) -> TransactionResult {
        self.set_time(t);
        let ix = self.finalize_accts(payer.pubkey()).to_ix();
        let p = dup(payer);
        self.v.e.send(&[ix], &[&p])
    }

    pub fn new_payer(&mut self) -> Keypair {
        let k = Keypair::new();
        self.v.e.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    // ------------------------------------------------------------- expectations

    /// Runs `f`, expects exactly `want` on instruction 0 and an unchanged world.
    pub fn fails(&mut self, want: Setl8Error, f: impl FnOnce(&mut JEnv) -> TransactionResult) {
        let before = self.pass_digest();
        let r = f(self);
        assert_err(&r, want);
        assert_eq!(self.pass_digest(), before, "a rejected call changed the world ({want:?})");
    }

    /// Like `fails` with an Anchor framework error code.
    pub fn fails_anchor(&mut self, code: u32, f: impl FnOnce(&mut JEnv) -> TransactionResult) {
        let before = self.pass_digest();
        let r = f(self);
        assert_anchor_code(&r, code);
        assert_eq!(self.pass_digest(), before, "a rejected call changed the world (anchor {code})");
    }

    /// Runs `f`, expects success.
    pub fn ok(&mut self, f: impl FnOnce(&mut JEnv) -> TransactionResult) -> TransactionMetadata {
        match f(self) {
            Ok(m) => m,
            Err(e) => panic!("expected success, got {:?}\n{}", e.err, e.meta.logs.join("\n")),
        }
    }

    /// Digest of everything a pass instruction can touch: the base digest plus the payout claims of the first six request ids
    /// and the vault trader state of every challenge of the trader.
    pub fn pass_digest(&self) -> [u8; 32] {
        let vts = self.vts();
        let extra: Vec<Pubkey> = (1..=6u64).map(|i| claim_pda(&vts, i)).collect();
        self.v.digest(&self.trader.pubkey(), &extra)
    }

    // ------------------------------------------------------------- state helpers

    /// An account that meets the pass condition in `phase`: `b = 1.08 S`, flat, nothing pending.
    pub fn ready(&mut self, phase: u8) {
        self.edit_account(|a| {
            a.phase = phase;
            a.b = 108_000_000_000;
            a.flat = 1;
            a.tight_on = 0;
            a.q_tally = 0;
        });
    }

    /// Writes `pass_pending = state` with the matching timestamps.
    pub fn set_pending(&mut self, state: u8, requested_at: i64, proposed_at: i64) {
        self.edit_account(|a| {
            a.pass_pending = state;
            a.pass_requested_at = requested_at;
            a.pass_proposed_at = proposed_at;
        });
    }

    // ------------------------------------------------------------- readers

    pub fn claim(&self, request_id: u64) -> Option<VaultClaim> {
        let acc = self.v.e.svm.get_account(&claim_pda(&self.vts(), request_id))?;
        if acc.owner != VAULT_ID || acc.data.len() < 120 {
            return None;
        }
        let d = &acc.data;
        let u64_at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        Some(VaultClaim {
            trader_wallet: Pubkey::new_from_array(d[8..40].try_into().unwrap()),
            trader_state: Pubkey::new_from_array(d[40..72].try_into().unwrap()),
            request_id: u64_at(104),
            owed: u64_at(112),
        })
    }

    pub fn tally_bytes(&self) -> Vec<u8> {
        self.v.e.svm.get_account(&tally_pda()).expect("tally").data
    }

    pub fn tally(&self) -> si::PayoutTally {
        si::PayoutTally::parse(&self.tally_bytes()).expect("tally parses")
    }

    pub fn wallet_profile(&self) -> setl8_lev_trading::state::WalletProfile {
        self.v.e.wallet(&self.trader.pubkey())
    }

    pub fn vault_status(&self) -> VaultTraderState {
        self.v.vault_trader_state(&self.trader.pubkey(), self.account().challenge_id).expect("vault trader state")
    }

    /// The vault's `ProductRegistry` counters: (total_requests_emitted, total_requested_amount).
    pub fn registry_totals(&self) -> (u64, u64) {
        let acc = self.v.e.svm.get_account(&registry_pda()).expect("registry");
        // layout: disc 8, product_program_id 32, challenge_sizes Vec<ChallengeSize{size,cost}> (4 + n*16), fee_split u16,
        // max_payout_count u64, active u8, total_requests_emitted u64, reset_price_bps Vec<u16> (4 + m*2), pause_reason u8,
        // paused_since i64, total_paused_secs i64, total_requested_amount u64, bump u8
        let d = &acc.data;
        let mut o = 8 + 32;
        let n = u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        o += 4 + n * 16 + 2 + 8 + 1;
        let emitted = u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        o += 8;
        let m = u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        o += 4 + m * 2 + 1 + 8 + 8;
        let amount = u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        (emitted, amount)
    }

    // ------------------------------------------------------------- vault and config switches

    /// Pauses this product in the real vault (both vault admins, flagged as signers: sigverify is off).
    pub fn pause_vault_product(&mut self) {
        let ix = si::pause_product(
            VAULT_ID,
            VAULT_SL8_ADMIN,
            VAULT_ROV_ADMIN,
            registry_pda(),
            &[],
            si::PauseProductArgs { product_program_id: PROGRAM_ID },
        );
        assert_ok(self.v.e.send(&[ix], &[]));
    }

    /// Both admins set the delay, cap and window.
    pub fn set_limits(&mut self, delay: u32, cap: u64, window: u32) {
        let ix = set_limits_ix(both(&self.v.e), delay, cap, window);
        assert_ok(self.v.e.send_as_admins(&[ix]));
    }

    pub fn pause_sector(&mut self, paused: bool) {
        let ix = set_paused_ix(both(&self.v.e), paused);
        assert_ok(self.v.e.send_as_admins(&[ix]));
    }
}

pub const USD: i64 = 1_000_000;
/// 0.08 S of the $100k account.
pub const PAYOUT_100K: u64 = 8_000_000_000;
