use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::state::{Market, Session};

/// The market fields an admin can set at creation and change later (class and id are fixed).
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MarketParams {
    pub status: u8,
    pub feed_id: [u8; 32],
    pub max_age_secs: u16,
    pub qty_step: u64,
    pub qty_min: u64,
    pub max_notional_usd: u64,
    pub session_count: u8,
    pub sessions: [Session; 6],
    pub quote_conv_market: u16,
}

impl MarketParams {
    /// Shared by `create_market` and `update_market`. `own_id` is the market's id. A conversion market must be
    /// an EARLIER market (`< own_id`): that is "an existing market that is not itself" at creation (where
    /// `own_id == market_count`) and, on update, it keeps the conversion graph acyclic.
    /// Every failure is `MarketUnknown` (DESIGN 12 has no market-parameter error).
    pub fn validate(&self, own_id: u16) -> Result<()> {
        require!(self.status <= MARKET_STATUS_REDUCE_ONLY, Setl8Error::MarketUnknown);
        require!(self.feed_id != [0u8; 32], Setl8Error::MarketUnknown);
        require!(self.max_age_secs > 0, Setl8Error::MarketUnknown);
        require!(self.qty_step > 0 && self.qty_min >= self.qty_step, Setl8Error::MarketUnknown);
        require!(self.max_notional_usd > 0, Setl8Error::MarketUnknown);

        let used = usize::from(self.session_count);
        require!(used <= MAX_SESSIONS, Setl8Error::MarketUnknown);
        let (live, unused) = self.sessions.split_at(used);
        require!(
            live.iter().all(|s| s.start_min < s.end_min && s.end_min <= MINUTES_PER_WEEK),
            Setl8Error::MarketUnknown
        );
        require!(unused.iter().all(|s| *s == Session::default()), Setl8Error::MarketUnknown);

        require!(
            self.quote_conv_market == NO_QUOTE_CONV || self.quote_conv_market < own_id,
            Setl8Error::MarketUnknown
        );
        Ok(())
    }

    pub fn apply(&self, market: &mut Market) {
        market.status = self.status;
        market.feed_id = self.feed_id;
        market.max_age_secs = self.max_age_secs;
        market.qty_step = self.qty_step;
        market.qty_min = self.qty_min;
        market.max_notional_usd = self.max_notional_usd;
        market.session_count = self.session_count;
        market.sessions = self.sessions;
        market.quote_conv_market = self.quote_conv_market;
    }
}
