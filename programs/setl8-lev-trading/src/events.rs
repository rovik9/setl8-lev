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

/// One journal event as posted, emitted through `emit_cpi!` by `post_journal` (an inner instruction, not a text log).
/// `event_bytes` and `proof_bytes` are the canonical Borsh bytes that went into the hash chain (`proof_bytes` is empty
/// when the event references no proof), so a verifier recomputes the chain from these alone.
#[event]
pub struct EventLog {
    pub account: Pubkey,
    pub seq: u64,
    pub event_bytes: Vec<u8>,
    pub proof_bytes: Vec<u8>,
}

/// `request_advance` (through `emit_cpi!`): the evaluator reads it to freeze the account and compare its engine state.
/// `amount` is 0 in phase 1, else 0.08 S. `seq` and `journal_hash` are the chain head the request was made against.
#[event]
pub struct PassRequested {
    pub account: Pubkey,
    pub phase: u8,
    pub amount: u64,
    pub b: i64,
    pub seq: u64,
    pub journal_hash: [u8; 32],
    pub time: i64,
}

/// `cancel_pass`: the trader withdrew an unanswered request.
#[event]
pub struct PassCancelled {
    pub account: Pubkey,
    pub phase: u8,
    pub time: i64,
}

/// `confirm_pass` for phases 2 to 5. `eligible_at` is `now + config.pass_delay_secs` at the moment of confirmation
/// (`finalize_pass` reads the CURRENT delay, so an admin change in between moves the real date).
#[event]
pub struct PassConfirmed {
    pub account: Pubkey,
    pub phase: u8,
    pub amount: u64,
    pub b: i64,
    pub seq: u64,
    pub journal_hash: [u8; 32],
    pub eligible_at: i64,
}

#[event]
pub struct PassRejected {
    pub account: Pubkey,
    pub phase: u8,
    pub reason: u8,
}

#[event]
pub struct PassVetoed {
    pub account: Pubkey,
    pub phase: u8,
    pub admin_a: Pubkey,
    pub admin_b: Pubkey,
    pub slot: u64,
}

/// A new phase started fresh (phase 1 to 2 at `confirm_pass`, or after a `finalize_pass` payout).
#[event]
pub struct PhaseAdvanced {
    pub account: Pubkey,
    pub from_phase: u8,
    pub to_phase: u8,
    pub time: i64,
}

/// `finalize_pass` queued the vault payout claim. `to_phase` is 0 when the account graduated (status 3).
/// `tally_count` and `tally_total` are the payout tally after the update.
#[event]
pub struct PassFinalized {
    pub account: Pubkey,
    pub phase: u8,
    pub to_phase: u8,
    pub amount: u64,
    pub request_id: u64,
    pub tally_count: u64,
    pub tally_total: u64,
    pub window_total: u64,
    pub time: i64,
}

/// The vault answered `Abandoned`: the local account is Abandoned and the wallet slot is free.
#[event]
pub struct AccountAbandoned {
    pub account: Pubkey,
    pub trader: Pubkey,
    pub time: i64,
}
