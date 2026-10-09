use anchor_lang::prelude::*;

use super::admin_action::AdminAction;
use crate::events::PausedChanged;

/// Pausing: either admin. Unpausing: both admins (DESIGN 6.1).
pub fn handle_set_paused(ctx: Context<AdminAction>, paused: bool) -> Result<()> {
    if paused {
        ctx.accounts.either_admin()?;
    } else {
        ctx.accounts.both_admins()?;
    }
    ctx.accounts.config.paused = u8::from(paused);
    emit!(PausedChanged { paused });
    Ok(())
}
