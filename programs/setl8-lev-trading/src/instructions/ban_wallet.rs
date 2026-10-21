use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::WalletBanned;
use crate::state::{SectorConfig, WalletProfile};

#[event_cpi]
#[derive(Accounts)]
#[instruction(trader: Pubkey, evidence_hash: [u8; 32])]
pub struct BanWallet<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
    #[account(
        init_if_needed,
        payer = payer,
        space = WalletProfile::SIZE,
        seeds = [WALLET_SEED, trader.as_ref()],
        bump
    )]
    pub wallet: Account<'info, WalletProfile>,
    pub system_program: Program<'info, System>,
}

/// Both admins. Creates the profile if missing (`payer` pays); idempotent if already banned.
/// Refused while the profile's `held_count` is above 0 (M7, DESIGN 6.1 "no held account").
pub fn handle_ban_wallet(ctx: Context<BanWallet>, trader: Pubkey, evidence_hash: [u8; 32]) -> Result<()> {
    let accounts = &mut *ctx.accounts;
    require_both_admins(&accounts.config, &accounts.admin_a, &accounts.admin_b)?;
    require!(evidence_hash != [0u8; 32], Setl8Error::EvidenceMissing);

    let wallet = &mut accounts.wallet;
    if wallet.trader == Pubkey::default() {
        wallet.trader = trader;
        wallet.next_challenge_id = 0;
        wallet.active_count = 0;
        wallet.bump = ctx.bumps.wallet;
        wallet.held_count = 0;
        wallet.pad = [0; 4];
    }
    // DESIGN 6.1: only a linked wallet with no held account (the hold must be released or failed first)
    require!(wallet.held_count == 0, Setl8Error::WalletHasHeldAccount);
    wallet.banned = 1;
    emit_cpi!(WalletBanned { trader, evidence_hash });
    Ok(())
}
