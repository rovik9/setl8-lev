//! UNVERIFIED SKELETON (M0): not compiled yet because the build sandbox could not
//! download anchor-lang. The program id below is the System Program id as a
//! PLACEHOLDER; Rovik must generate the real program keypair (never printed or
//! committed) and replace it before any deploy.
use anchor_lang::prelude::*;

declare_id!("11111111111111111111111111111111");

#[program]
pub mod setl8_lev_trading {
    use super::*;

    /// Placeholder so the skeleton has one instruction; replaced by `init_config` in M1.
    pub fn ping(_ctx: Context<Ping>) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Ping {}
