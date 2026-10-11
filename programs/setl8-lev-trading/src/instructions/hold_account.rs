use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::AccountHeld;
use crate::state::{flag, SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.3 `hold_account`: the evaluator pauses an account for review (2.9). Reversible, moves no money.
#[event_cpi]
#[derive(Accounts)]
pub struct HoldAccount<'info> {
    pub evaluator: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(mut, seeds = [WALLET_SEED, trading_account.trader.as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
}

/// Evaluator only; Active; non-zero evidence hash; not already held. Sets `held = 1` and the wallet's `held_count + 1`.
/// Works in every `pass_pending` state; changes no phase state.
pub fn handle_hold_account(ctx: Context<HoldAccount>, evidence_hash: [u8; 32]) -> Result<()> {
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    let acct = &mut ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(evidence_hash != [0u8; 32], Setl8Error::EvidenceMissing);
    require!(!flag(acct.held)?, Setl8Error::AlreadyHeld);
    acct.held = 1;
    let wallet = &mut ctx.accounts.wallet;
    wallet.held_count = wallet.held_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
    emit_cpi!(AccountHeld { account: acct.key(), trader: acct.trader, evidence_hash });
    Ok(())
}
