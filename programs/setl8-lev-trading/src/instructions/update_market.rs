use anchor_lang::prelude::*;

use super::admin_action::require_either_admin;
use super::market_params::MarketParams;
use crate::constants::*;
use crate::events::{MarketFeedChanged, MarketUpdated};
use crate::state::{Market, SectorConfig};

/// There is deliberately no `class` field: the class of a market never changes.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct UpdateMarketArgs {
    pub market_id: u16,
    pub params: MarketParams,
}

#[derive(Accounts)]
#[instruction(args: UpdateMarketArgs)]
pub struct UpdateMarket<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
    #[account(mut, seeds = [MARKET_SEED, &args.market_id.to_le_bytes()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// Either admin. DECISION 24 EXCEPTION (Rovik): an admin may change a market's feed, status and limits at any
/// time, with no delay and no lock. The only guard is the `MarketFeedChanged` event on every feed change.
pub fn handle_update_market(ctx: Context<UpdateMarket>, args: UpdateMarketArgs) -> Result<()> {
    let accounts = ctx.accounts;
    require_either_admin(&accounts.config, &accounts.admin_a, &accounts.admin_b)?;
    args.params.validate(args.market_id)?;

    let market = &mut accounts.market;
    let old_feed = market.feed_id;
    args.params.apply(market);
    if old_feed != market.feed_id {
        emit!(MarketFeedChanged {
            market_id: args.market_id,
            old_feed,
            new_feed: market.feed_id,
            slot: Clock::get()?.slot,
        });
    }
    emit!(MarketUpdated { market_id: args.market_id });
    Ok(())
}
