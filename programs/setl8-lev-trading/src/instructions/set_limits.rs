use anchor_lang::prelude::*;

use super::admin_action::{check_limits, AdminAction};
use crate::events::LimitsChanged;

/// Both admins. Delay clamped to 1 to 6 days; cap and window must be non-zero.
pub fn handle_set_limits(
    ctx: Context<AdminAction>,
    pass_delay_secs: u32,
    max_payout_per_window: u64,
    window_secs: u32,
) -> Result<()> {
    ctx.accounts.both_admins()?;
    check_limits(pass_delay_secs, max_payout_per_window, window_secs)?;
    let config = &mut ctx.accounts.config;
    emit_cpi!(LimitsChanged {
        old_pass_delay_secs: config.pass_delay_secs,
        old_max_payout_per_window: config.max_payout_per_window,
        old_window_secs: config.window_secs,
        new_pass_delay_secs: pass_delay_secs,
        new_max_payout_per_window: max_payout_per_window,
        new_window_secs: window_secs,
    });
    config.pass_delay_secs = pass_delay_secs;
    config.max_payout_per_window = max_payout_per_window;
    config.window_secs = window_secs;
    Ok(())
}
