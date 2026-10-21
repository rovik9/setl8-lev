use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::PassCancelled;
use crate::state::TradingAccount;

/// DESIGN 6.2 `cancel_pass`: the trader withdraws a request the evaluator never answered.
#[event_cpi]
#[derive(Accounts)]
pub struct CancelPass<'info> {
    pub trader: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump,
        constraint = trading_account.trader == trader.key() @ Setl8Error::Unauthorized
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
}

/// Only in state 1 (`NoPendingPass` otherwise) and only `CANCEL_PASS_AFTER_SECS` (3,600) after the request
/// (`CancelTooEarly`); phase, `b`, `q`, latch and tight state are untouched.
pub fn handle_cancel_pass(ctx: Context<CancelPass>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let acct = &mut ctx.accounts.trading_account;
    require!(acct.pass_pending == PASS_REQUESTED, Setl8Error::NoPendingPass);
    let waited = now.checked_sub(acct.pass_requested_at).ok_or(Setl8Error::MathOverflow)?;
    require!(waited >= CANCEL_PASS_AFTER_SECS, Setl8Error::CancelTooEarly);
    acct.pass_pending = PASS_NONE;
    emit_cpi!(PassCancelled { account: acct.key(), phase: acct.phase, time: now });
    Ok(())
}
