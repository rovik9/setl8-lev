use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::HoldReleased;
use crate::state::{flag, SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.1 `release_hold`: both admins clear a hold after review (2.9, 3b).
#[event_cpi]
#[derive(Accounts)]
pub struct ReleaseHold<'info> {
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
}

/// Both admins; `held == 1` (`NotHeld`). Clears the hold and the wallet's `held_count - 1`. A pending pass is untouched and
/// its delay clock kept running (`pass_proposed_at` is not written).
pub fn handle_release_hold(ctx: Context<ReleaseHold>) -> Result<()> {
    require_both_admins(&ctx.accounts.config, &ctx.accounts.admin_a, &ctx.accounts.admin_b)?;
    let acct = &mut ctx.accounts.trading_account;
    require!(flag(acct.held)?, Setl8Error::NotHeld);
    acct.held = 0;
    let wallet = &mut ctx.accounts.wallet;
    wallet.held_count = wallet.held_count.checked_sub(1).ok_or(Setl8Error::MathOverflow)?;
    emit_cpi!(HoldReleased { account: acct.key(), trader: acct.trader });
    Ok(())
}
