use anchor_lang::prelude::*;
use setl8_shared_interfaces::ActivityOutcome;

use super::pass_common::*;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::{AccountAbandoned, PassRequested};
use crate::state::{SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.2 `request_advance` ("request payout and move to next phase"). Moves no money. The wallet profile is passed
/// (DESIGN 6.2 lists none) because an `Abandoned` answer of the vault must free the wallet slot.
#[event_cpi]
#[derive(Accounts)]
pub struct RequestAdvance<'info> {
    /// The owner of the account; no payer account is needed (nothing is created, no lamports move).
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
    /// CHECK: the vault's `TraderState` of this challenge; the vault verifies its seeds against the args.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
}

/// Checks: not paused; Active; not held; nothing pending; tight mode off; the pass condition (not failed, flat,
/// `b >= 1.08 S`). Then `record_activity` through `sector_authority`; `Abandoned` marks the account Abandoned, frees the
/// wallet slot and ends the call WITHOUT a pending pass; otherwise `pass_pending = 1` and `PassRequested` is emitted.
pub fn handle_request_advance(ctx: Context<RequestAdvance>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(ctx.accounts.config.paused == 0, Setl8Error::Paused);
    let acct = &ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(acct.held == 0, Setl8Error::AccountHeld);
    require!(acct.pass_pending == PASS_NONE, Setl8Error::PassAlreadyPending);
    require!(acct.tight_on == 0, Setl8Error::TightModeOn);
    check_pass_condition(acct)?;
    let amount = pass_amount(acct)?;

    let account_key = acct.key();
    let (trader, challenge_id) = (acct.trader, acct.challenge_id);
    let outcome = record_activity_cpi(
        &ctx.accounts.vault_program.to_account_info(),
        &ctx.accounts.sector_authority.to_account_info(),
        ctx.bumps.sector_authority,
        &ctx.accounts.product_registry.to_account_info(),
        &ctx.accounts.vault_trader_state.to_account_info(),
        trader,
        *ctx.program_id,
        challenge_id,
    )?;

    if outcome == ActivityOutcome::Abandoned {
        mark_abandoned(&mut ctx.accounts.trading_account, &mut ctx.accounts.wallet, now)?;
        emit_cpi!(AccountAbandoned { account: account_key, trader, time: now });
        return Ok(());
    }
    let acct = &mut ctx.accounts.trading_account;
    if outcome == ActivityOutcome::Recorded {
        acct.last_activity_pushed = now;
    }
    acct.pass_pending = PASS_REQUESTED;
    acct.pass_requested_at = now;
    emit_cpi!(PassRequested {
        account: account_key,
        phase: acct.phase,
        amount,
        b: acct.b,
        seq: acct.seq,
        journal_hash: acct.journal_hash,
        time: now,
    });
    Ok(())
}
