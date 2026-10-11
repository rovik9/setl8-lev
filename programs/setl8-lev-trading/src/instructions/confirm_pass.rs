use anchor_lang::prelude::*;

use super::pass_common::*;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::{PassConfirmed, PhaseAdvanced};
use crate::state::{SectorConfig, TradingAccount};

/// DESIGN 6.3 `confirm_pass`. Works while the sector is paused (only `request_advance` and `finalize_pass` check it).
#[event_cpi]
#[derive(Accounts)]
pub struct ConfirmPass<'info> {
    pub evaluator: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
}

/// Evaluator only. Requires Active, state 1, not held, and the pass condition again (flat, `b >= 1.08 S`, not failed).
/// Phase 1 starts phase 2 at once (no money). Phases 2 to 5 go to state 2 with `pass_proposed_at = now`.
pub fn handle_confirm_pass(ctx: Context<ConfirmPass>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    let acct = &ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(acct.pass_pending == PASS_REQUESTED, Setl8Error::NoPendingPass);
    require!(acct.held == 0, Setl8Error::AccountHeld);
    check_pass_condition(acct)?;
    let amount = pass_amount(acct)?;
    let next = engine::next_phase(acct.phase).map_err(|_| error!(Setl8Error::WrongPhase))?;
    let account_key = acct.key();
    let from_phase = acct.phase;

    match next {
        engine::NextPhase::Next { phase, payout: false } => {
            let acct = &mut ctx.accounts.trading_account;
            start_phase(acct, phase)?;
            emit_cpi!(PhaseAdvanced { account: account_key, from_phase, to_phase: phase, time: now });
        }
        _ => {
            let eligible_at = now.checked_add(i64::from(ctx.accounts.config.pass_delay_secs)).ok_or(Setl8Error::MathOverflow)?;
            let acct = &mut ctx.accounts.trading_account;
            acct.pass_pending = PASS_CONFIRMED;
            acct.pass_proposed_at = now;
            emit_cpi!(PassConfirmed {
                account: account_key,
                phase: from_phase,
                amount,
                b: acct.b,
                seq: acct.seq,
                journal_hash: acct.journal_hash,
                eligible_at,
            });
        }
    }
    Ok(())
}
