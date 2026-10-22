//! Program ids, derived addresses and instruction builders for the two programs, with the program ids given AT RUN TIME (the
//! rehearsal deploys both under throwaway ids). The accounts and data come from the program crate's own client types
//! (`accounts::*`, `instruction::*`), the same ones the LiteSVM suite builds its instructions with.
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token;
use setl8_lev_trading::{
    accounts, constants as c, instruction,
    instructions::{CreateMarketArgs, InitConfigArgs, RuleSetArgs},
};
use setl8_shared_interfaces as si;
use sha2::{Digest, Sha256};

pub const ATA_PROGRAM: Pubkey = anchor_lang::prelude::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const COMPUTE_BUDGET: Pubkey = anchor_lang::prelude::pubkey!("ComputeBudget111111111111111111111111111111");

/// The two program ids of a rehearsal and every address derived from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub lev: Pubkey,
    pub vault: Pubkey,
}

impl Ids {
    pub fn config(&self) -> Pubkey {
        Pubkey::find_program_address(&[c::CONFIG_SEED], &self.lev).0
    }
    pub fn tally(&self) -> Pubkey {
        si::derive_payout_tally(&self.lev).0
    }
    pub fn ruleset(&self, version: u16) -> Pubkey {
        Pubkey::find_program_address(&[c::RULESET_SEED, &version.to_le_bytes()], &self.lev).0
    }
    pub fn market(&self, id: u16) -> Pubkey {
        Pubkey::find_program_address(&[c::MARKET_SEED, &id.to_le_bytes()], &self.lev).0
    }
    pub fn wallet(&self, trader: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[c::WALLET_SEED, trader.as_ref()], &self.lev).0
    }
    pub fn acct(&self, trader: &Pubkey, challenge_id: u64) -> Pubkey {
        Pubkey::find_program_address(&[c::ACCT_SEED, trader.as_ref(), &challenge_id.to_le_bytes()], &self.lev).0
    }
    pub fn event_authority(&self) -> Pubkey {
        Pubkey::find_program_address(&[b"__event_authority"], &self.lev).0
    }
    pub fn sector_authority(&self) -> Pubkey {
        si::derive_sector_authority(&self.lev).0
    }
    pub fn registry(&self) -> Pubkey {
        Pubkey::find_program_address(&[b"product_registry", self.lev.as_ref()], &self.vault).0
    }
    pub fn vault_state(&self, sl8_admin: &Pubkey, rov_admin: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"vault_state", sl8_admin.as_ref(), rov_admin.as_ref()], &self.vault).0
    }
    pub fn pool(&self, vault_state: &Pubkey, mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"pool", vault_state.as_ref(), mint.as_ref()], &self.vault).0
    }
    pub fn vault_trader_state(&self, trader: &Pubkey, challenge_id: u64) -> Pubkey {
        Pubkey::find_program_address(&[b"trader_state", self.lev.as_ref(), trader.as_ref(), &challenge_id.to_le_bytes()], &self.vault).0
    }
    pub fn claim(&self, trader_state: &Pubkey, request_id: u64) -> Pubkey {
        si::derive_payout_claim(&self.vault, trader_state, request_id).0
    }
}

/// Marks every account of `ix` whose key is in `keys` as a signer (the admin slots of the program are plain accounts that the handler
/// checks against the signer flag; the client types do not set it).
pub fn with_signers(mut ix: Instruction, keys: &[&Pubkey]) -> Instruction {
    for m in ix.accounts.iter_mut() {
        if keys.iter().any(|k| **k == m.pubkey) {
            m.is_signer = true;
        }
    }
    ix
}

pub fn anchor_disc(name: &str) -> [u8; 8] {
    let h = Sha256::digest(format!("global:{name}").as_bytes());
    let mut d = [0u8; 8];
    d.copy_from_slice(&h[..8]);
    d
}

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), spl_token::ID.as_ref(), mint.as_ref()], &ATA_PROGRAM).0
}

/// `CreateIdempotent` of the associated token program: the token account of `owner` for `mint`, paid by `payer`.
pub fn create_ata_idempotent(payer: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    Instruction {
        program_id: ATA_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata(owner, mint), false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![1],
    }
}

/// The two instructions that create a 6-decimal mint at the address of `mint` (a throwaway keypair that signs the transaction).
pub fn create_mint(payer: &Pubkey, mint: &Pubkey, rent: u64, mint_authority: &Pubkey, freeze_authority: &Pubkey) -> Vec<Instruction> {
    vec![
        anchor_lang::solana_program::system_instruction::create_account(payer, mint, rent, spl_token::state::Mint::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_mint2(&spl_token::ID, mint, mint_authority, Some(freeze_authority), 6).expect("initialize_mint2"),
    ]
}

pub fn mint_to(mint: &Pubkey, dest: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    spl_token::instruction::mint_to(&spl_token::ID, mint, dest, authority, &[], amount).expect("mint_to")
}

pub fn compute_budget_limit(units: u32) -> Instruction {
    Instruction { program_id: COMPUTE_BUDGET, accounts: vec![], data: [vec![2u8], units.to_le_bytes().to_vec()].concat() }
}

use anchor_spl::token::spl_token::solana_program::program_pack::Pack;

// ---------------------------------------------------------------------------------------------- the vault

/// The vault's `init_vault(usdc_mint, usdt_mint)` (hand-built: this repo does not depend on the vault crate).
pub fn init_vault(ids: &Ids, sl8_admin: &Pubkey, rov_admin: &Pubkey, usdc: &Pubkey, usdt: &Pubkey) -> Instruction {
    let vault_state = ids.vault_state(sl8_admin, rov_admin);
    Instruction {
        program_id: ids.vault,
        accounts: vec![
            AccountMeta::new(*sl8_admin, true),
            AccountMeta::new_readonly(*rov_admin, true),
            AccountMeta::new(vault_state, false),
            AccountMeta::new_readonly(*usdc, false),
            AccountMeta::new_readonly(*usdt, false),
            AccountMeta::new(ids.pool(&vault_state, usdc), false),
            AccountMeta::new(ids.pool(&vault_state, usdt), false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: [anchor_disc("init_vault").to_vec(), usdc.to_bytes().to_vec(), usdt.to_bytes().to_vec()].concat(),
    }
}

/// The registration of THIS program as a product of the vault (both vault admins sign).
pub fn register_product(ids: &Ids, sl8_admin: &Pubkey, rov_admin: &Pubkey, tiers: &[(u64, u64)], fee_split_bps: u16, max_payout_count: u64, ladder: &[u16]) -> Instruction {
    si::register_product(
        ids.vault,
        *sl8_admin,
        *rov_admin,
        ids.registry(),
        &[AccountMeta::new_readonly(system_program::ID, false)],
        si::RegisterProductArgs {
            product_program_id: ids.lev,
            fee_split_bps,
            challenge_sizes: tiers.iter().map(|(size, cost)| si::ChallengeSize { size: *size, cost: *cost }).collect(),
            max_payout_count,
            reset_price_bps: ladder.to_vec(),
        },
    )
}

/// `update_product_config` with the given tiers, keeping the other registered values.
pub fn update_product_tiers(ids: &Ids, sl8_admin: &Pubkey, rov_admin: &Pubkey, tiers: &[(u64, u64)], fee_split_bps: u16, max_payout_count: u64, ladder: &[u16]) -> Instruction {
    si::update_product_config(
        ids.vault,
        *sl8_admin,
        *rov_admin,
        ids.registry(),
        &[],
        si::UpdateProductConfigArgs {
            product_program_id: ids.lev,
            challenge_sizes: tiers.iter().map(|(size, cost)| si::ChallengeSize { size: *size, cost: *cost }).collect(),
            fee_split_bps,
            max_payout_count,
            reset_price_bps: ladder.to_vec(),
        },
    )
}

// ---------------------------------------------------------------------------------------------- our program: admin

pub fn init_config(ids: &Ids, payer: &Pubkey, sl8: &Pubkey, rov: &Pubkey, args: &InitConfigArgs) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::InitConfig { payer: *payer, sl8_admin: *sl8, rov_admin: *rov, config: ids.config(), payout_tally: ids.tally(), system_program: system_program::ID }.to_account_metas(None),
        data: instruction::InitConfig { args: args.clone() }.data(),
    }, &[payer, sl8, rov])
}

pub fn create_market(ids: &Ids, payer: &Pubkey, a: &Pubkey, b: &Pubkey, args: &CreateMarketArgs) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::CreateMarket {
            payer: *payer,
            config: ids.config(),
            admin_a: *a,
            admin_b: *b,
            market: ids.market(args.market_id),
            system_program: system_program::ID,
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::CreateMarket { args: args.clone() }.data(),
    }, &[payer, a, b])
}

pub fn create_ruleset(ids: &Ids, payer: &Pubkey, a: &Pubkey, b: &Pubkey, args: &RuleSetArgs) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::CreateRuleset {
            payer: *payer,
            config: ids.config(),
            admin_a: *a,
            admin_b: *b,
            ruleset: ids.ruleset(args.version),
            system_program: system_program::ID,
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::CreateRuleset { args: args.clone() }.data(),
    }, &[payer, a, b])
}

/// The metas of an admin instruction with config, the two admins (read-only signers) and the event accounts.
fn admin_metas(ids: &Ids, a: &Pubkey, b: &Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(ids.config(), false),
        AccountMeta::new_readonly(*a, true),
        AccountMeta::new_readonly(*b, true),
        AccountMeta::new_readonly(ids.event_authority(), false),
        AccountMeta::new_readonly(ids.lev, false),
    ]
}

pub fn set_paused(ids: &Ids, a: &Pubkey, b: &Pubkey, paused: bool) -> Instruction {
    with_signers(Instruction { program_id: ids.lev, accounts: admin_metas(ids, a, b), data: instruction::SetPaused { paused }.data() }, &[a, b])
}

pub fn set_evaluator(ids: &Ids, a: &Pubkey, b: &Pubkey, new_key: &Pubkey) -> Instruction {
    with_signers(Instruction { program_id: ids.lev, accounts: admin_metas(ids, a, b), data: instruction::SetEvaluator { new_key: *new_key }.data() }, &[a, b])
}

pub fn update_market(ids: &Ids, a: &Pubkey, b: &Pubkey, args: &setl8_lev_trading::instructions::UpdateMarketArgs) -> Instruction {
    with_signers(
        Instruction {
            program_id: ids.lev,
            accounts: accounts::UpdateMarket { config: ids.config(), admin_a: *a, admin_b: *b, market: ids.market(args.market_id), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
            data: instruction::UpdateMarket { args: args.clone() }.data(),
        },
        &[a, b],
    )
}

pub fn ban_wallet(ids: &Ids, payer: &Pubkey, a: &Pubkey, b: &Pubkey, trader: &Pubkey, evidence_hash: [u8; 32]) -> Instruction {
    with_signers(
        Instruction {
            program_id: ids.lev,
            accounts: accounts::BanWallet { payer: *payer, config: ids.config(), admin_a: *a, admin_b: *b, wallet: ids.wallet(trader), system_program: system_program::ID, event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
            data: instruction::BanWallet { trader: *trader, evidence_hash }.data(),
        },
        &[payer, a, b],
    )
}

pub fn close_account(ids: &Ids, trader: &Pubkey, challenge_id: u64) -> Instruction {
    with_signers(
        Instruction { program_id: ids.lev, accounts: accounts::CloseAccount { trader: *trader, trading_account: ids.acct(trader, challenge_id) }.to_account_metas(None), data: instruction::CloseAccount {}.data() },
        &[trader],
    )
}

/// `SetComputeUnitPrice` (micro-lamports per unit).
pub fn compute_budget_price(micro_lamports: u64) -> Instruction {
    Instruction { program_id: COMPUTE_BUDGET, accounts: vec![], data: [vec![3u8], micro_lamports.to_le_bytes().to_vec()].concat() }
}

pub fn set_limits(ids: &Ids, a: &Pubkey, b: &Pubkey, pass_delay_secs: u32, max_payout_per_window: u64, window_secs: u32) -> Instruction {
    with_signers(Instruction { program_id: ids.lev, accounts: admin_metas(ids, a, b), data: instruction::SetLimits { pass_delay_secs, max_payout_per_window, window_secs }.data() }, &[a, b])
}

// ---------------------------------------------------------------------------------------------- our program: trader

/// What `open_account` and `reset_account` share: the token side of the purchase.
#[derive(Clone, Copy, Debug)]
pub struct Pay {
    pub vault_state: Pubkey,
    pub trader_token: Pubkey,
    pub mint: Pubkey,
    pub pool: Pubkey,
    pub vault_sl8_token: Pubkey,
    pub fee_token: Pubkey,
}

pub fn open_account(ids: &Ids, trader: &Pubkey, ruleset_version: u16, next_challenge_id: u64, pay: &Pay, tier_index: u8) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::OpenAccount {
            trader: *trader,
            payer: *trader,
            config: ids.config(),
            ruleset: ids.ruleset(ruleset_version),
            wallet: ids.wallet(trader),
            trading_account: ids.acct(trader, next_challenge_id),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, next_challenge_id),
            vault_state: pay.vault_state,
            trader_token_account: pay.trader_token,
            mint: pay.mint,
            pool_token_account: pay.pool,
            vault_sl8_token_account: pay.vault_sl8_token,
            fee_token_account: pay.fee_token,
            token_program: spl_token::ID,
            system_program: system_program::ID,
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::OpenAccount { tier_index }.data(),
    }, &[trader])
}

pub fn reset_account(ids: &Ids, trader: &Pubkey, ruleset_version: u16, prev_challenge_id: u64, next_challenge_id: u64, pay: &Pay) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::ResetAccount {
            trader: *trader,
            payer: *trader,
            config: ids.config(),
            ruleset: ids.ruleset(ruleset_version),
            wallet: ids.wallet(trader),
            prev_trading_account: ids.acct(trader, prev_challenge_id),
            trading_account: ids.acct(trader, next_challenge_id),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, next_challenge_id),
            prev_vault_trader_state: ids.vault_trader_state(trader, prev_challenge_id),
            vault_state: pay.vault_state,
            trader_token_account: pay.trader_token,
            mint: pay.mint,
            pool_token_account: pay.pool,
            vault_sl8_token_account: pay.vault_sl8_token,
            fee_token_account: pay.fee_token,
            token_program: spl_token::ID,
            system_program: system_program::ID,
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::ResetAccount { prev_challenge_id }.data(),
    }, &[trader])
}

pub fn request_advance(ids: &Ids, trader: &Pubkey, challenge_id: u64) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::RequestAdvance {
            trader: *trader,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            config: ids.config(),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, challenge_id),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::RequestAdvance {}.data(),
    }
}

pub fn cancel_pass(ids: &Ids, trader: &Pubkey, challenge_id: u64) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::CancelPass { trader: *trader, trading_account: ids.acct(trader, challenge_id), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
        data: instruction::CancelPass {}.data(),
    }
}

pub fn finalize_pass(ids: &Ids, payer: &Pubkey, trader: &Pubkey, challenge_id: u64, next_request_id: u64, vault_state: &Pubkey) -> Instruction {
    let vts = ids.vault_trader_state(trader, challenge_id);
    Instruction {
        program_id: ids.lev,
        accounts: accounts::FinalizePass {
            payer: *payer,
            trading_account: ids.acct(trader, challenge_id),
            config: ids.config(),
            payout_tally: ids.tally(),
            wallet: ids.wallet(trader),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: vts,
            vault_state: *vault_state,
            payout_claim: ids.claim(&vts, next_request_id),
            system_program: system_program::ID,
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::FinalizePass {}.data(),
    }
}

pub fn release_account(ids: &Ids, trader: &Pubkey, challenge_id: u64) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::ReleaseAccount {
            trader: *trader,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            config: ids.config(),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, challenge_id),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::ReleaseAccount {}.data(),
    }
}

// ---------------------------------------------------------------------------------------------- our program: evaluator

fn market_metas(ids: &Ids, markets: &[u16]) -> Vec<AccountMeta> {
    markets.iter().map(|m| AccountMeta::new_readonly(ids.market(*m), false)).collect()
}

pub fn post_journal(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64, ruleset_version: u16, batch_bytes: Vec<u8>, markets: &[u16]) -> Instruction {
    let mut metas = accounts::PostJournal {
        evaluator: *evaluator,
        trading_account: ids.acct(trader, challenge_id),
        config: ids.config(),
        ruleset: ids.ruleset(ruleset_version),
        event_authority: ids.event_authority(),
        program: ids.lev,
    }
    .to_account_metas(None);
    metas.extend(market_metas(ids, markets));
    Instruction { program_id: ids.lev, accounts: metas, data: instruction::PostJournal { batch_bytes }.data() }
}

#[allow(clippy::too_many_arguments)]
pub fn report_fail(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64, ruleset_version: u16, batch_bytes: Vec<u8>, reason: u8, markets: &[u16]) -> Instruction {
    let mut metas = accounts::ReportFail {
        evaluator: *evaluator,
        trading_account: ids.acct(trader, challenge_id),
        config: ids.config(),
        ruleset: ids.ruleset(ruleset_version),
        wallet: ids.wallet(trader),
        sector_authority: ids.sector_authority(),
        vault_program: ids.vault,
        product_registry: ids.registry(),
        vault_trader_state: ids.vault_trader_state(trader, challenge_id),
        event_authority: ids.event_authority(),
        program: ids.lev,
    }
    .to_account_metas(None);
    metas.extend(market_metas(ids, markets));
    Instruction { program_id: ids.lev, accounts: metas, data: instruction::ReportFail { batch_bytes, reason }.data() }
}

pub fn confirm_pass(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::ConfirmPass { evaluator: *evaluator, trading_account: ids.acct(trader, challenge_id), config: ids.config(), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
        data: instruction::ConfirmPass {}.data(),
    }
}

pub fn reject_pass(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64, reason: u8) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::RejectPass { evaluator: *evaluator, trading_account: ids.acct(trader, challenge_id), config: ids.config(), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
        data: instruction::RejectPass { reason }.data(),
    }
}

pub fn hold_account(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64, evidence_hash: [u8; 32]) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::HoldAccount {
            evaluator: *evaluator,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            config: ids.config(),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::HoldAccount { evidence_hash }.data(),
    }
}

pub fn touch_activity(ids: &Ids, evaluator: &Pubkey, trader: &Pubkey, challenge_id: u64) -> Instruction {
    Instruction {
        program_id: ids.lev,
        accounts: accounts::TouchActivity {
            evaluator: *evaluator,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            config: ids.config(),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, challenge_id),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::TouchActivity {}.data(),
    }
}

// ---------------------------------------------------------------------------------------------- our program: both admins

pub fn veto_pass(ids: &Ids, a: &Pubkey, b: &Pubkey, trader: &Pubkey, challenge_id: u64) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::VetoPass { config: ids.config(), admin_a: *a, admin_b: *b, trading_account: ids.acct(trader, challenge_id), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
        data: instruction::VetoPass {}.data(),
    }, &[a, b])
}

pub fn release_hold(ids: &Ids, a: &Pubkey, b: &Pubkey, trader: &Pubkey, challenge_id: u64) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::ReleaseHold {
            config: ids.config(),
            admin_a: *a,
            admin_b: *b,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::ReleaseHold {}.data(),
    }, &[a, b])
}

pub fn unban_wallet(ids: &Ids, a: &Pubkey, b: &Pubkey, trader: &Pubkey) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::UnbanWallet { config: ids.config(), admin_a: *a, admin_b: *b, wallet: ids.wallet(trader), event_authority: ids.event_authority(), program: ids.lev }.to_account_metas(None),
        data: instruction::UnbanWallet { trader: *trader }.data(),
    }, &[a, b])
}

pub fn fail_for_violation(ids: &Ids, a: &Pubkey, b: &Pubkey, trader: &Pubkey, challenge_id: u64, evidence_hash: [u8; 32]) -> Instruction {
    with_signers(Instruction {
        program_id: ids.lev,
        accounts: accounts::FailForViolation {
            config: ids.config(),
            admin_a: *a,
            admin_b: *b,
            trading_account: ids.acct(trader, challenge_id),
            wallet: ids.wallet(trader),
            sector_authority: ids.sector_authority(),
            vault_program: ids.vault,
            product_registry: ids.registry(),
            vault_trader_state: ids.vault_trader_state(trader, challenge_id),
            event_authority: ids.event_authority(),
            program: ids.lev,
        }
        .to_account_metas(None),
        data: instruction::FailForViolation { evidence_hash }.data(),
    }, &[a, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Ids {
        Ids { lev: Pubkey::new_from_array([1; 32]), vault: Pubkey::new_from_array([2; 32]) }
    }

    #[test]
    fn addresses_follow_the_seeds_and_the_ids_given_at_run_time() {
        let i = ids();
        let j = Ids { lev: Pubkey::new_from_array([3; 32]), ..i };
        assert_ne!(i.config(), j.config(), "a different program id gives a different config PDA");
        assert_eq!(i.config(), Pubkey::find_program_address(&[b"config"], &i.lev).0);
        let t = Pubkey::new_from_array([9; 32]);
        assert_eq!(i.acct(&t, 3), Pubkey::find_program_address(&[b"acct", t.as_ref(), &3u64.to_le_bytes()], &i.lev).0);
        assert_eq!(i.vault_trader_state(&t, 3), Pubkey::find_program_address(&[b"trader_state", i.lev.as_ref(), t.as_ref(), &3u64.to_le_bytes()], &i.vault).0);
        assert_eq!(i.registry(), Pubkey::find_program_address(&[b"product_registry", i.lev.as_ref()], &i.vault).0);
        assert_eq!(i.event_authority(), Pubkey::find_program_address(&[b"__event_authority"], &i.lev).0);
    }

    #[test]
    fn the_instruction_builders_use_the_given_program_ids() {
        let i = ids();
        let t = Pubkey::new_from_array([9; 32]);
        let ev = Pubkey::new_from_array([7; 32]);
        let post = post_journal(&i, &ev, &t, 0, 1, vec![1, 2, 3], &[0, 2]);
        assert_eq!(post.program_id, i.lev);
        assert_eq!(post.accounts[0].pubkey, ev);
        assert!(post.accounts[0].is_signer);
        assert_eq!(post.accounts.last().map(|m| m.pubkey), Some(i.market(2)), "markets come last");
        let fin = finalize_pass(&i, &t, &t, 4, 2, &Pubkey::new_unique());
        assert!(fin.accounts.iter().any(|m| m.pubkey == i.claim(&i.vault_trader_state(&t, 4), 2)), "the claim PDA of the request id");
        let veto = veto_pass(&i, &Pubkey::new_from_array([5; 32]), &Pubkey::new_from_array([6; 32]), &t, 0);
        assert_eq!(veto.accounts.iter().filter(|m| m.is_signer).count(), 2);
    }

    #[test]
    fn the_associated_token_address_is_the_documented_pda() {
        let owner = Pubkey::new_from_array([4; 32]);
        let mint = Pubkey::new_from_array([5; 32]);
        let want = Pubkey::find_program_address(&[owner.as_ref(), spl_token::ID.as_ref(), mint.as_ref()], &ATA_PROGRAM).0;
        assert_eq!(ata(&owner, &mint), want);
        let ix = create_ata_idempotent(&owner, &owner, &mint);
        assert_eq!((ix.data.clone(), ix.accounts[1].pubkey), (vec![1], want));
    }

    #[test]
    fn anchor_discriminator_of_init_vault() {
        // sha256("global:init_vault")[..8]
        let d = anchor_disc("init_vault");
        assert_eq!(d.len(), 8);
        assert_ne!(d, anchor_disc("init_vaults"));
    }
}

#[cfg(test)]
mod bump_probe {
    use super::*;
    #[test]
    #[ignore]
    fn print_bumps() {
        let repo: Pubkey = "9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7".parse().unwrap();
        let ours: Pubkey = "HmkvQfgCMnihGzdZE1FTKVacZN76JS9bfEu2QBJBAdvY".parse().unwrap();
        for (n, id) in [("repo", repo), ("throwaway", ours)] {
            println!("{n}: event_authority bump {}, config bump {}, sector_authority bump {}, market0 bump {}, ruleset1 bump {}",
                Pubkey::find_program_address(&[b"__event_authority"], &id).1, Pubkey::find_program_address(&[c::CONFIG_SEED], &id).1,
                Pubkey::find_program_address(&[c::SECTOR_AUTHORITY_SEED], &id).1, Pubkey::find_program_address(&[c::MARKET_SEED, &0u16.to_le_bytes()], &id).1,
                Pubkey::find_program_address(&[c::RULESET_SEED, &1u16.to_le_bytes()], &id).1);
        }
    }
}
