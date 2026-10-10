//! setl8-lev-trading: sector program for leveraged-trading challenges (docs/DESIGN.md).
//!
//! M1a: errors, admin keys, `SectorConfig`, `PayoutTally`, `init_config`, `set_evaluator`,
//! `set_limits`, `set_paused`. M1b: `RuleSet`, `Market`, `WalletProfile`, `create_ruleset`, `create_market`,
//! `update_market`, `ban_wallet`, `unban_wallet`. The program id is a PLACEHOLDER: sha256("setl8-lev-trading/placeholder-program-id/v1")
//! in base58, so no keypair exists for it. (The M0 placeholder, the System Program id, cannot be used: a program
//! loaded at that address replaces the system program and breaks every account-creating CPI.) Rovik generates
//! the real keypair locally (never printed or committed) and replaces the id before any deploy.
use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7");

#[program]
pub mod setl8_lev_trading {
    use super::*;

    pub fn init_config(ctx: Context<InitConfig>, args: InitConfigArgs) -> Result<()> {
        instructions::init_config::handle_init_config(ctx, args)
    }

    pub fn set_evaluator(ctx: Context<AdminAction>, new_key: Pubkey) -> Result<()> {
        instructions::set_evaluator::handle_set_evaluator(ctx, new_key)
    }

    pub fn set_limits(
        ctx: Context<AdminAction>,
        pass_delay_secs: u32,
        max_payout_per_window: u64,
        window_secs: u32,
    ) -> Result<()> {
        instructions::set_limits::handle_set_limits(ctx, pass_delay_secs, max_payout_per_window, window_secs)
    }

    pub fn set_paused(ctx: Context<AdminAction>, paused: bool) -> Result<()> {
        instructions::set_paused::handle_set_paused(ctx, paused)
    }

    pub fn create_ruleset(ctx: Context<CreateRuleset>, args: RuleSetArgs) -> Result<()> {
        instructions::create_ruleset::handle_create_ruleset(ctx, args)
    }

    pub fn create_market(ctx: Context<CreateMarket>, args: CreateMarketArgs) -> Result<()> {
        instructions::create_market::handle_create_market(ctx, args)
    }

    pub fn update_market(ctx: Context<UpdateMarket>, args: UpdateMarketArgs) -> Result<()> {
        instructions::update_market::handle_update_market(ctx, args)
    }

    pub fn ban_wallet(ctx: Context<BanWallet>, trader: Pubkey, evidence_hash: [u8; 32]) -> Result<()> {
        instructions::ban_wallet::handle_ban_wallet(ctx, trader, evidence_hash)
    }

    pub fn unban_wallet(ctx: Context<UnbanWallet>, trader: Pubkey) -> Result<()> {
        instructions::unban_wallet::handle_unban_wallet(ctx, trader)
    }
}
