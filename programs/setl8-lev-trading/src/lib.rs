//! setl8-lev-trading: sector program for leveraged-trading challenges (docs/DESIGN.md).
//!
//! M1a: errors, admin keys, `SectorConfig`, `PayoutTally`, `init_config`, `set_evaluator`,
//! `set_limits`, `set_paused`. The program id is a PLACEHOLDER: sha256("setl8-lev-trading/placeholder-program-id/v1")
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
}
