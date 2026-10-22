use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use super::pass_common::*;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::ViolationFail;
use crate::state::{flag, SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.1 `fail_for_violation`: both admins fail a held account (2.9, 3a). Same accounts as `report_fail` minus the batch.
#[event_cpi]
#[derive(Accounts)]
pub struct FailForViolation<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(mut, seeds = [WALLET_SEED, trading_account.trader.as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    /// CHECK: PDA `["setl8_sector_authority"]` of this program; signs the vault CPI.
    #[account(seeds = [SECTOR_AUTHORITY_SEED], bump)]
    pub sector_authority: UncheckedAccount<'info>,
    /// CHECK: must be the vault program stored in the config.
    #[account(address = config.vault_program @ Setl8Error::InvalidVaultProgram)]
    pub vault_program: UncheckedAccount<'info>,
    /// CHECK: the vault's `ProductRegistry` for this program (read-only); the vault verifies its seeds.
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of this challenge; the vault verifies its seeds against the args.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
}

/// Both admins; a non-zero evidence hash; `held == 1`; the account is Active OR Failed (P25c, decision 67 amendment: a Failed + held
/// account is one that `report_fail` failed while held). Active: Failed + `terminal_at`, any pending pass cleared (also state 2), the
/// hold cleared (`held_count - 1`), `flag_trader_failed`, `active_count - 1`. Failed + held: the hold is cleared (`held_count - 1`),
/// `terminal_at` stays, the wallet slot was already freed by `report_fail` (not freed again) and the vault record is already Failed
/// (a second `flag_trader_failed` reverts in the vault), so there is no vault call. Both: the wallet banned. Works while paused.
/// The only writer of `PASS_VIOLATION` in `pass_pending` (the violation marker, decision 60).
pub fn handle_fail_for_violation(ctx: Context<FailForViolation>, evidence_hash: [u8; 32]) -> Result<()> {
    require_both_admins(&ctx.accounts.config, &ctx.accounts.admin_a, &ctx.accounts.admin_b)?;
    require!(evidence_hash != [0u8; 32], Setl8Error::EvidenceMissing);
    let now = Clock::get()?.unix_timestamp;
    let acct = &mut ctx.accounts.trading_account;
    let was_failed = acct.status == STATUS_FAILED;
    require!(acct.status == STATUS_ACTIVE || was_failed, Setl8Error::AccountNotActive);
    require!(flag(acct.held)?, Setl8Error::NotHeld);
    let (account_key, trader, challenge_id) = (acct.key(), acct.trader, acct.challenge_id);
    if was_failed {
        acct.held = 0;
        let wallet = &mut ctx.accounts.wallet;
        wallet.held_count = wallet.held_count.checked_sub(1).ok_or(Setl8Error::MathOverflow)?;
    } else {
        end_account(acct, &mut ctx.accounts.wallet, STATUS_FAILED, now)?;
    }
    // the marker: a violation-failed account is never resettable, banned wallet or not (decision 60)
    acct.pass_pending = PASS_VIOLATION;
    ctx.accounts.wallet.banned = 1;
    if !was_failed {
        flag_failed_cpi(
            &ctx.accounts.vault_program.to_account_info(),
            &ctx.accounts.sector_authority.to_account_info(),
            ctx.bumps.sector_authority,
            &ctx.accounts.product_registry.to_account_info(),
            &ctx.accounts.vault_trader_state.to_account_info(),
            trader,
            *ctx.program_id,
            challenge_id,
        )?;
    }
    emit_cpi!(ViolationFail { account: account_key, trader, evidence_hash });
    Ok(())
}
