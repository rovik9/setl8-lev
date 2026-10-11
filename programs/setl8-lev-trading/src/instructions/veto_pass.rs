use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::PassVetoed;
use crate::state::{SectorConfig, TradingAccount};

/// DESIGN 6.1 / 10.7 `veto_pass`: both admins in one transaction.
#[event_cpi]
#[derive(Accounts)]
pub struct VetoPass<'info> {
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
}

/// Needs `pass_pending != 0` (state 1 or 2). Sets it to 0 and counts the veto (saturating at 255). Never touches `b`,
/// the phase, the tally or the vault, and works whatever the account status is (it only unfreezes).
pub fn handle_veto_pass(ctx: Context<VetoPass>) -> Result<()> {
    require_both_admins(&ctx.accounts.config, &ctx.accounts.admin_a, &ctx.accounts.admin_b)?;
    let slot = Clock::get()?.slot;
    let acct = &mut ctx.accounts.trading_account;
    require!(acct.pass_pending != PASS_NONE, Setl8Error::NoPendingPass);
    acct.pass_pending = PASS_NONE;
    acct.vetoes = acct.vetoes.saturating_add(1);
    emit_cpi!(PassVetoed {
        account: acct.key(),
        phase: acct.phase,
        admin_a: ctx.accounts.config.admin_a,
        admin_b: ctx.accounts.config.admin_b,
        slot,
    });
    Ok(())
}
