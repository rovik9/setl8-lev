use anchor_lang::prelude::*;

use super::admin_action::require_both_admins;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::RuleSetCreated;
use crate::state::{RuleSet, SectorConfig};

/// Everything in `RuleSet` except `bump` and `reserved`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RuleSetArgs {
    pub version: u16,
    pub fee_bps: u16,
    pub tier_count: u8,
    pub ladder_len: u8,
    pub leverage_x: [u16; 8],
    pub holding_fee_e6: [u32; 8],
    pub commission_e6: [u32; 8],
    pub tier_size: [u64; 10],
    pub tier_cost: [u64; 10],
    pub reset_ladder_bps: [u16; 5],
    pub spread_e6: [u16; 128],
}

#[derive(Accounts)]
#[instruction(args: RuleSetArgs)]
pub struct CreateRuleset<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
    #[account(
        init,
        payer = payer,
        space = RuleSet::SIZE,
        seeds = [RULESET_SEED, &args.version.to_le_bytes()],
        bump
    )]
    pub ruleset: Box<Account<'info, RuleSet>>,
    pub system_program: Program<'info, System>,
}

/// Both admins. Rule sets are immutable: this is the only instruction that writes one, and it only creates.
pub fn handle_create_ruleset(ctx: Context<CreateRuleset>, args: RuleSetArgs) -> Result<()> {
    let accounts = ctx.accounts;
    require_both_admins(&accounts.config, &accounts.admin_a, &accounts.admin_b)?;

    let next = accounts.config.current_ruleset.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
    require!(args.version == next, Setl8Error::InvalidTier);
    validate_ruleset(&args)?;

    accounts.config.current_ruleset = args.version;
    accounts.ruleset.set_inner(RuleSet {
        version: args.version,
        fee_bps: args.fee_bps,
        tier_count: args.tier_count,
        ladder_len: args.ladder_len,
        leverage_x: args.leverage_x,
        holding_fee_e6: args.holding_fee_e6,
        commission_e6: args.commission_e6,
        tier_size: args.tier_size,
        tier_cost: args.tier_cost,
        reset_ladder_bps: args.reset_ladder_bps,
        spread_e6: args.spread_e6,
        bump: ctx.bumps.ruleset,
        reserved: [0; 31],
    });
    emit!(RuleSetCreated { version: args.version });
    Ok(())
}

/// Tier, ladder, leverage and fee checks (all failures are `InvalidTier`). Commission, spread and
/// holding fee are PENDING numbers: any value is accepted.
fn validate_ruleset(a: &RuleSetArgs) -> Result<()> {
    let tiers = usize::from(a.tier_count);
    let ladder = usize::from(a.ladder_len);
    require!((1..=MAX_TIERS).contains(&tiers), Setl8Error::InvalidTier);
    require!((1..=MAX_LADDER).contains(&ladder), Setl8Error::InvalidTier);
    require!(a.fee_bps <= MAX_FEE_BPS, Setl8Error::InvalidTier);
    require!(a.leverage_x.iter().all(|&x| x >= 1), Setl8Error::InvalidTier);

    let (size, size_unused) = a.tier_size.split_at(tiers);
    let (cost, cost_unused) = a.tier_cost.split_at(tiers);
    require!(size.iter().all(|&x| x > 0) && size.is_sorted_by(|a, b| a < b), Setl8Error::InvalidTier);
    require!(cost.iter().all(|&x| x > 0) && cost.is_sorted_by(|a, b| a < b), Setl8Error::InvalidTier);
    require!(size_unused.iter().all(|&x| x == 0) && cost_unused.iter().all(|&x| x == 0), Setl8Error::InvalidTier);

    let (steps, steps_unused) = a.reset_ladder_bps.split_at(ladder);
    require!(steps.iter().all(|&x| x > 0) && steps.is_sorted_by(|a, b| a <= b), Setl8Error::InvalidTier);
    require!(steps_unused.iter().all(|&x| x == 0), Setl8Error::InvalidTier);
    Ok(())
}
