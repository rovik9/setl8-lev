use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use crate::constants::*;
use crate::events::WalletUnbanned;
use crate::state::{SectorConfig, WalletProfile};

#[derive(Accounts)]
#[instruction(trader: Pubkey)]
pub struct UnbanWallet<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
    /// The profile must already exist (Anchor's own not-initialized error otherwise).
    #[account(mut, seeds = [WALLET_SEED, trader.as_ref()], bump = wallet.bump)]
    pub wallet: Account<'info, WalletProfile>,
}

/// Both admins. Clears `banned`; does not revive failed accounts.
pub fn handle_unban_wallet(ctx: Context<UnbanWallet>, trader: Pubkey) -> Result<()> {
    let accounts = ctx.accounts;
    require_both_admins(&accounts.config, &accounts.admin_a, &accounts.admin_b)?;
    accounts.wallet.banned = 0;
    emit!(WalletUnbanned { trader });
    Ok(())
}
