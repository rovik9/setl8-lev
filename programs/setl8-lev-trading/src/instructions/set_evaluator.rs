use anchor_lang::prelude::*;

use super::admin_action::AdminAction;
use crate::errors::Setl8Error;
use crate::events::EvaluatorChanged;

/// Both admins. Effective immediately (DESIGN 6.1).
pub fn handle_set_evaluator(ctx: Context<AdminAction>, new_key: Pubkey) -> Result<()> {
    ctx.accounts.both_admins()?;
    require!(new_key != Pubkey::default(), Setl8Error::Unauthorized);
    let config = &mut ctx.accounts.config;
    let old = config.evaluator;
    config.evaluator = new_key;
    emit_cpi!(EvaluatorChanged { old, new: new_key });
    Ok(())
}
