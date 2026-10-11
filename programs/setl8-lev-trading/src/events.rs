use anchor_lang::prelude::*;

#[event]
pub struct EvaluatorChanged {
    pub old: Pubkey,
    pub new: Pubkey,
}

#[event]
pub struct LimitsChanged {
    pub old_pass_delay_secs: u32,
    pub old_max_payout_per_window: u64,
    pub old_window_secs: u32,
    pub new_pass_delay_secs: u32,
    pub new_max_payout_per_window: u64,
    pub new_window_secs: u32,
}

#[event]
pub struct PausedChanged {
    pub paused: bool,
}

#[event]
pub struct RuleSetCreated {
    pub version: u16,
}

#[event]
pub struct MarketCreated {
    pub market_id: u16,
    pub class: u8,
    pub feed_id: [u8; 32],
}

#[event]
pub struct MarketUpdated {
    pub market_id: u16,
}

/// Emitted whenever an admin changes a market's price feed (decision 24: no delay, no lock).
#[event]
pub struct MarketFeedChanged {
    pub market_id: u16,
    pub old_feed: [u8; 32],
    pub new_feed: [u8; 32],
    pub slot: u64,
}

#[event]
pub struct WalletBanned {
    pub trader: Pubkey,
    pub evidence_hash: [u8; 32],
}

#[event]
pub struct WalletUnbanned {
    pub trader: Pubkey,
}

/// Emitted by `open_account` through `emit_cpi!`. NOT part of the hash chain (`seq = 0`, `journal_hash = genesis_hash`).
#[event]
pub struct Opened {
    pub account: Pubkey,
    pub trader: Pubkey,
    pub challenge_id: u64,
    pub size_s: u64,
    pub tier_index: u8,
    pub phase: u8,
    pub ruleset_version: u16,
    pub genesis_hash: [u8; 32],
    pub time: i64,
}
