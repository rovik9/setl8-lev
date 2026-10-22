use anchor_lang::prelude::*;
use journal_types::{FailReason, JournalBatch};

use super::pass_common::*;
use super::post_journal::{decode_err, run_batch, Mode};
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::AccountFailed;
use crate::state::{RuleSet, SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.3 `report_fail`: the final batch of an account. The Market accounts the batch references follow the fixed
/// accounts as remaining accounts (as in `post_journal`).
#[event_cpi]
#[derive(Accounts)]
pub struct ReportFail<'info> {
    /// The evaluator key (also the fee payer).
    #[account(mut)]
    pub evaluator: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// The rule set the account was bought under (spreads).
    #[account(
        seeds = [RULESET_SEED, &trading_account.ruleset_version.to_le_bytes()],
        bump = ruleset.bump,
        constraint = ruleset.version == trading_account.ruleset_version @ Setl8Error::RuleSetMissing
    )]
    pub ruleset: Box<Account<'info, RuleSet>>,
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

/// Evaluator only. Applies the batch under the invariants of `post_journal`, with the batch shape `events..., Fail, Meta`
/// (`Meta.flat == 1`), the reason justified by the engine's floors at that point (I-11), a quick overshoot allowing
/// nothing but the Fail after it (I-8). Rejected while `pass_pending == 2`; state 1 is cleared. Sets status Failed and
/// `terminal_at`, calls `flag_trader_failed`, frees the wallet slot. A HOLD STAYS (P25c, decision 67 amendment): a held account
/// is not frozen for losses, so it can still be failed on its floor, but it remains Failed + held for the two admins
/// (`release_hold` or `fail_for_violation`); `reset_account` and `close_account` refuse it with `ReviewPending`.
/// Works while the sector or the vault product is paused.
pub fn handle_report_fail<'info>(ctx: Context<'_, '_, 'info, 'info, ReportFail<'info>>, batch_bytes: Vec<u8>, reason: u8) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    require!(FailReason::try_from(reason).is_ok(), Setl8Error::FailReasonInvalid);
    let acct = &ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(acct.pass_pending != PASS_CONFIRMED, Setl8Error::PassPending);

    let batch = JournalBatch::decode(&batch_bytes).map_err(|e| error!(decode_err(&batch_bytes, e)))?;
    let account_key = acct.key();
    let (trader, challenge_id) = (acct.trader, acct.challenge_id);
    let out = run_batch(ctx.program_id, account_key, acct, &ctx.accounts.ruleset, ctx.remaining_accounts, &batch, now, Mode::ReportFail(reason))?;
    require!(out.fail.is_some(), Setl8Error::FailMissing);
    for log in out.logs {
        emit_cpi!(log);
    }

    let acct = &mut ctx.accounts.trading_account;
    acct.apply_engine_state(&out.st)?;
    acct.trades_closed = out.trades_closed;
    acct.last_batch_time = out.last_time;
    acct.seq = out.seq;
    acct.journal_hash = out.hash;
    acct.flat = 1;
    end_account_keep_hold(acct, &mut ctx.accounts.wallet, STATUS_FAILED, now)?;
    let (b, seq, journal_hash) = (acct.b, acct.seq, acct.journal_hash);

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
    emit_cpi!(AccountFailed { account: account_key, reason, b, seq, journal_hash });
    Ok(())
}
