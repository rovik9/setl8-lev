//! `touch_activity` (evaluator) and `release_account` (trader): the same vault `record_activity` push (DESIGN 6.2, 6.3).
use anchor_lang::prelude::*;
use setl8_shared_interfaces::ActivityOutcome;

use super::pass_common::*;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::AccountAbandoned;
use crate::state::{SectorConfig, TradingAccount, WalletProfile};

/// What an activity push did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// Last push younger than `ACTIVITY_THROTTLE_SECS`: success without a vault call.
    Skipped,
    Recorded,
    Throttled,
    Abandoned,
}

/// Pushes activity for an Active account. A younger last push is skipped; if the vault already holds the challenge as
/// Abandoned (the permissionless `mark_abandoned` flipped it, so `record_activity` would only error) the local account is
/// marked Abandoned; otherwise `record_activity` runs and its return data decides: `Recorded` stores `last_activity_pushed`,
/// `Throttled` stores nothing, `Abandoned` marks the account.
#[allow(clippy::too_many_arguments)]
pub fn push_activity<'info>(
    acct: &mut TradingAccount,
    wallet: &mut WalletProfile,
    now: i64,
    program_id: &Pubkey,
    vault_program: &AccountInfo<'info>,
    sector_authority: &AccountInfo<'info>,
    authority_bump: u8,
    product_registry: &AccountInfo<'info>,
    vault_trader_state: &AccountInfo<'info>,
) -> Result<Push> {
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    let since = now.checked_sub(acct.last_activity_pushed).ok_or(Setl8Error::MathOverflow)?;
    if since < ACTIVITY_THROTTLE_SECS {
        return Ok(Push::Skipped);
    }
    let status = vault_status(&vault_program.key(), program_id, &acct.trader, acct.challenge_id, vault_trader_state)?;
    if status == VAULT_STATUS_ABANDONED {
        mark_abandoned(acct, wallet, now)?;
        return Ok(Push::Abandoned);
    }
    let outcome = record_activity_cpi(
        vault_program,
        sector_authority,
        authority_bump,
        product_registry,
        vault_trader_state,
        acct.trader,
        *program_id,
        acct.challenge_id,
    )?;
    Ok(match outcome {
        ActivityOutcome::Abandoned => {
            mark_abandoned(acct, wallet, now)?;
            Push::Abandoned
        }
        ActivityOutcome::Recorded => {
            acct.last_activity_pushed = now;
            Push::Recorded
        }
        ActivityOutcome::Throttled => Push::Throttled,
    })
}

#[event_cpi]
#[derive(Accounts)]
pub struct TouchActivity<'info> {
    pub evaluator: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(mut, seeds = [WALLET_SEED, trading_account.trader.as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: PDA `["setl8_sector_authority"]` of this program; signs the vault CPI.
    #[account(seeds = [SECTOR_AUTHORITY_SEED], bump)]
    pub sector_authority: UncheckedAccount<'info>,
    /// CHECK: must be the vault program stored in the config.
    #[account(address = config.vault_program @ Setl8Error::InvalidVaultProgram)]
    pub vault_program: UncheckedAccount<'info>,
    /// CHECK: the vault's `ProductRegistry` for this program; the vault verifies its seeds.
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of this challenge; checked against the vault's PDA and owner, then by the vault.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
}

/// Evaluator only (the backend calls it when the trader sent a signed order).
pub fn handle_touch_activity(ctx: Context<TouchActivity>) -> Result<()> {
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    let now = Clock::get()?.unix_timestamp;
    let (account_key, trader) = (ctx.accounts.trading_account.key(), ctx.accounts.trading_account.trader);
    let push = push_activity(
        &mut ctx.accounts.trading_account,
        &mut ctx.accounts.wallet,
        now,
        ctx.program_id,
        &ctx.accounts.vault_program.to_account_info(),
        &ctx.accounts.sector_authority.to_account_info(),
        ctx.bumps.sector_authority,
        &ctx.accounts.product_registry.to_account_info(),
        &ctx.accounts.vault_trader_state.to_account_info(),
    )?;
    if push == Push::Abandoned {
        emit_cpi!(AccountAbandoned { account: account_key, trader, time: now });
    }
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct ReleaseAccount<'info> {
    pub trader: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump,
        constraint = trading_account.trader == trader.key() @ Setl8Error::Unauthorized
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(mut, seeds = [WALLET_SEED, trading_account.trader.as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: PDA `["setl8_sector_authority"]` of this program; signs the vault CPI.
    #[account(seeds = [SECTOR_AUTHORITY_SEED], bump)]
    pub sector_authority: UncheckedAccount<'info>,
    /// CHECK: must be the vault program stored in the config.
    #[account(address = config.vault_program @ Setl8Error::InvalidVaultProgram)]
    pub vault_program: UncheckedAccount<'info>,
    /// CHECK: the vault's `ProductRegistry` for this program; the vault verifies its seeds.
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of this challenge; checked against the vault's PDA and owner, then by the vault.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
}

/// The trader's own way to keep the vault clock alive and to learn that the vault abandoned the challenge. A call inside
/// the 86,000 s window is a harmless success without a vault call.
pub fn handle_release_account(ctx: Context<ReleaseAccount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let (account_key, trader) = (ctx.accounts.trading_account.key(), ctx.accounts.trading_account.trader);
    let push = push_activity(
        &mut ctx.accounts.trading_account,
        &mut ctx.accounts.wallet,
        now,
        ctx.program_id,
        &ctx.accounts.vault_program.to_account_info(),
        &ctx.accounts.sector_authority.to_account_info(),
        ctx.bumps.sector_authority,
        &ctx.accounts.product_registry.to_account_info(),
        &ctx.accounts.vault_trader_state.to_account_info(),
    )?;
    if push == Push::Abandoned {
        emit_cpi!(AccountAbandoned { account: account_key, trader, time: now });
    }
    Ok(())
}
