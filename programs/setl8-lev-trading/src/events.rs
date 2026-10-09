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
