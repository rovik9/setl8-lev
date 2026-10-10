use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::state::SectorConfig;

/// Accounts of every instruction authorised by the stored admin pair.
/// The admin accounts are NOT typed `Signer` because `set_paused` accepts a single signer;
/// each handler calls `both_admins` or `either_admin`.
#[derive(Accounts)]
pub struct AdminAction<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, SectorConfig>,
    /// CHECK: compared with `config.admin_a` and its signer flag in the handler.
    pub admin_a: UncheckedAccount<'info>,
    /// CHECK: compared with `config.admin_b` and its signer flag in the handler.
    pub admin_b: UncheckedAccount<'info>,
}

impl<'info> AdminAction<'info> {
    /// Both stored admins signed.
    pub fn both_admins(&self) -> Result<()> {
        require_both_admins(&self.config, &self.admin_a, &self.admin_b)
    }

    /// At least one stored admin signed.
    pub fn either_admin(&self) -> Result<()> {
        require_either_admin(&self.config, &self.admin_a, &self.admin_b)
    }
}

fn a_signed(config: &SectorConfig, admin_a: &AccountInfo) -> bool {
    admin_a.is_signer && admin_a.key() == config.admin_a
}

fn b_signed(config: &SectorConfig, admin_b: &AccountInfo) -> bool {
    admin_b.is_signer && admin_b.key() == config.admin_b
}

/// Both stored admins signed (positional: `admin_a` slot = `config.admin_a`, `admin_b` slot = `config.admin_b`).
/// Used by every instruction whose accounts struct carries the admin pair next to other accounts.
pub fn require_both_admins(config: &SectorConfig, admin_a: &AccountInfo, admin_b: &AccountInfo) -> Result<()> {
    require!(a_signed(config, admin_a) && b_signed(config, admin_b), Setl8Error::Unauthorized);
    Ok(())
}

/// At least one stored admin signed.
pub fn require_either_admin(config: &SectorConfig, admin_a: &AccountInfo, admin_b: &AccountInfo) -> Result<()> {
    require!(a_signed(config, admin_a) || b_signed(config, admin_b), Setl8Error::Unauthorized);
    Ok(())
}

/// Shared by `init_config` and `set_limits`: delay in 1 to 14 days, cap > 0, window > 0.
pub fn check_limits(pass_delay_secs: u32, max_payout_per_window: u64, window_secs: u32) -> Result<()> {
    require!(
        (MIN_PASS_DELAY_SECS..=MAX_PASS_DELAY_SECS).contains(&pass_delay_secs)
            && max_payout_per_window > 0
            && window_secs > 0,
        Setl8Error::InvalidLimits
    );
    Ok(())
}
