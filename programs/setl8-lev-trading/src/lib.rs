//! setl8-lev-trading: sector program for leveraged-trading challenges (docs/DESIGN.md).
//!
//! M1a: errors, admin keys, `SectorConfig`, `PayoutTally`, `init_config`, `set_evaluator`,
//! `set_limits`, `set_paused`. M5a: `TradingAccount`, `open_account` (vault `deposit_fee` CPI). M1b: `RuleSet`, `Market`, `WalletProfile`, `create_ruleset`, `create_market`,
//! `update_market`, `ban_wallet`, `unban_wallet`. The program id is a PLACEHOLDER: sha256("setl8-lev-trading/placeholder-program-id/v1")
//! in base58, so no keypair exists for it. (The M0 placeholder, the System Program id, cannot be used: a program
//! loaded at that address replaces the system program and breaks every account-creating CPI.) Rovik generates
//! the real keypair locally (never printed or committed) and replaces the id before any deploy.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::arithmetic_side_effects,
        clippy::float_arithmetic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]
use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod reset_price;
pub mod state;
pub mod vault_view;

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

    pub fn open_account(ctx: Context<OpenAccount>, tier_index: u8) -> Result<()> {
        instructions::open_account::handle_open_account(ctx, tier_index)
    }

    pub fn reset_account(ctx: Context<ResetAccount>, prev_challenge_id: u64) -> Result<()> {
        instructions::reset_account::handle_reset_account(ctx, prev_challenge_id)
    }

    pub fn post_journal<'info>(ctx: Context<'_, '_, 'info, 'info, PostJournal<'info>>, batch_bytes: Vec<u8>) -> Result<()> {
        instructions::post_journal::handle_post_journal(ctx, batch_bytes)
    }

    pub fn request_advance(ctx: Context<RequestAdvance>) -> Result<()> {
        instructions::request_advance::handle_request_advance(ctx)
    }

    pub fn cancel_pass(ctx: Context<CancelPass>) -> Result<()> {
        instructions::cancel_pass::handle_cancel_pass(ctx)
    }

    pub fn confirm_pass(ctx: Context<ConfirmPass>) -> Result<()> {
        instructions::confirm_pass::handle_confirm_pass(ctx)
    }

    pub fn reject_pass(ctx: Context<RejectPass>, reason: u8) -> Result<()> {
        instructions::reject_pass::handle_reject_pass(ctx, reason)
    }

    pub fn veto_pass(ctx: Context<VetoPass>) -> Result<()> {
        instructions::veto_pass::handle_veto_pass(ctx)
    }

    pub fn finalize_pass(ctx: Context<FinalizePass>) -> Result<()> {
        instructions::finalize_pass::handle_finalize_pass(ctx)
    }

    pub fn report_fail<'info>(ctx: Context<'_, '_, 'info, 'info, ReportFail<'info>>, batch_bytes: Vec<u8>, reason: u8) -> Result<()> {
        instructions::report_fail::handle_report_fail(ctx, batch_bytes, reason)
    }

    pub fn hold_account(ctx: Context<HoldAccount>, evidence_hash: [u8; 32]) -> Result<()> {
        instructions::hold_account::handle_hold_account(ctx, evidence_hash)
    }

    pub fn release_hold(ctx: Context<ReleaseHold>) -> Result<()> {
        instructions::release_hold::handle_release_hold(ctx)
    }

    pub fn fail_for_violation(ctx: Context<FailForViolation>, evidence_hash: [u8; 32]) -> Result<()> {
        instructions::fail_for_violation::handle_fail_for_violation(ctx, evidence_hash)
    }

    pub fn touch_activity(ctx: Context<TouchActivity>) -> Result<()> {
        instructions::activity::handle_touch_activity(ctx)
    }

    pub fn release_account(ctx: Context<ReleaseAccount>) -> Result<()> {
        instructions::activity::handle_release_account(ctx)
    }

    pub fn close_account(ctx: Context<CloseAccount>) -> Result<()> {
        instructions::close_account::handle_close_account(ctx)
    }
}
