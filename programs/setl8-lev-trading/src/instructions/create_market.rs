use anchor_lang::prelude::*;

use super::admin_action::require_either_admin;
use super::market_params::MarketParams;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::MarketCreated;
use crate::state::{Market, SectorConfig};

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct CreateMarketArgs {
    /// Must equal `config.market_count` (ids are sequential, no gaps).
    pub market_id: u16,
    /// 0..=7, fixed for the life of the market.
    pub class: u8,
    pub params: MarketParams,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: CreateMarketArgs)]
pub struct CreateMarket<'info> {
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
        space = Market::SIZE,
        seeds = [MARKET_SEED, &args.market_id.to_le_bytes()],
        bump
    )]
    pub market: Account<'info, Market>,
    pub system_program: Program<'info, System>,
}

/// Either admin (DESIGN 6.1).
pub fn handle_create_market(ctx: Context<CreateMarket>, args: CreateMarketArgs) -> Result<()> {
    let accounts = &mut *ctx.accounts;
    require_either_admin(&accounts.config, &accounts.admin_a, &accounts.admin_b)?;

    require!(args.market_id < MAX_MARKETS, Setl8Error::MarketUnknown);
    require!(args.market_id == accounts.config.market_count, Setl8Error::MarketUnknown);
    require!(usize::from(args.class) < CLASS_COUNT, Setl8Error::MarketUnknown);
    args.params.validate(args.market_id)?;

    accounts.config.market_count = accounts.config.market_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;

    let market = &mut accounts.market;
    market.market_id = args.market_id;
    market.class = args.class;
    args.params.apply(market);
    market.bump = ctx.bumps.market;
    market.reserved = [0; 30];
    emit_cpi!(MarketCreated { market_id: args.market_id, class: args.class, feed_id: args.params.feed_id });
    Ok(())
}
