use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::state::TradingAccount;

/// DESIGN 6.2 `close_account`: the rent goes back to the trader. The PDA address is never reused: `challenge_id` only increases.
#[derive(Accounts)]
pub struct CloseAccount<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(
        mut,
        close = trader,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump,
        constraint = trading_account.trader == trader.key() @ Setl8Error::Unauthorized
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
}

/// Only a terminal status (Failed, Abandoned, Graduated) and `now >= terminal_at + 7 days` (inclusive). Not a Failed account that
/// is still held (`ReviewPending`, P25c): closing it would delete the review's evidence and leave the wallet's `held_count` above
/// zero for ever, which would refuse `ban_wallet` for good.
pub fn handle_close_account(ctx: Context<CloseAccount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let acct = &ctx.accounts.trading_account;
    require!(matches!(acct.status, STATUS_FAILED | STATUS_ABANDONED | STATUS_GRADUATED), Setl8Error::AccountNotTerminal);
    require!(acct.held == 0, Setl8Error::ReviewPending);
    let ready = acct.terminal_at().checked_add(TERMINAL_RETENTION_SECS).ok_or(Setl8Error::MathOverflow)?;
    require!(now >= ready, Setl8Error::CloseTooEarly);
    Ok(())
}
