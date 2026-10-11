use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::PassRejected;
use crate::state::{SectorConfig, TradingAccount};

/// DESIGN 6.3 `reject_pass`. Cannot touch money.
#[event_cpi]
#[derive(Accounts)]
pub struct RejectPass<'info> {
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

/// Evaluator only, state 1 only. Reasons: NotFlat 0, JournalBehind 1, TargetNotReached 2, Other 3; anything else is
/// `RejectReasonInvalid`. Sets the state to 0; phase, `b`, `q`, latch and tight state are untouched.
pub fn handle_reject_pass(ctx: Context<RejectPass>, reason: u8) -> Result<()> {
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    require!(
        matches!(reason, REJECT_NOT_FLAT | REJECT_JOURNAL_BEHIND | REJECT_TARGET_NOT_REACHED | REJECT_OTHER),
        Setl8Error::RejectReasonInvalid
    );
    let acct = &mut ctx.accounts.trading_account;
    require!(acct.pass_pending == PASS_REQUESTED, Setl8Error::NoPendingPass);
    acct.pass_pending = PASS_NONE;
    emit_cpi!(PassRejected { account: acct.key(), phase: acct.phase, reason });
    Ok(())
}
