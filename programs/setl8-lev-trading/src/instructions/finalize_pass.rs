use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::program::invoke_signed;
use setl8_shared_interfaces::{request_payout, PayoutOutcome, PayoutTally, RequestPayoutArgs, PAYOUT_TALLY_MIN_LEN};

use super::pass_common::*;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::{AccountAbandoned, PassFinalized, PhaseAdvanced};
use crate::state::{SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.4 `finalize_pass`. Anyone pays (and signs for) the `PayoutClaim` rent.
#[event_cpi]
#[derive(Accounts)]
pub struct FinalizePass<'info> {
    /// Any payer; pays the claim's rent. Not related to the trader.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    /// Written: the daily window counters.
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// CHECK: PDA `["payout_tally"]` owned by this program; raw 25-byte layout of `setl8_shared_interfaces::PayoutTally`
    /// (no Anchor discriminator), parsed and written by hand.
    #[account(mut, seeds = [PAYOUT_TALLY_SEED], bump, owner = crate::ID)]
    pub payout_tally: UncheckedAccount<'info>,
    #[account(mut, seeds = [WALLET_SEED, trading_account.trader.as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    /// CHECK: PDA `["setl8_sector_authority"]` of this program; signs the vault CPI.
    #[account(seeds = [SECTOR_AUTHORITY_SEED], bump)]
    pub sector_authority: UncheckedAccount<'info>,
    /// CHECK: must be the vault program stored in the config.
    #[account(address = config.vault_program @ Setl8Error::InvalidVaultProgram)]
    pub vault_program: UncheckedAccount<'info>,
    /// CHECK: the vault's `ProductRegistry` for this program; the vault verifies its seeds. Written by the vault.
    #[account(mut)]
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of this challenge; the vault verifies its seeds against the args.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
    /// CHECK: the vault's `VaultState`; verified by the vault. Written by the vault.
    #[account(mut)]
    pub vault_state: UncheckedAccount<'info>,
    /// CHECK: the new claim PDA `["payout_claim", trader_state, request_id]` under the vault; created by the vault, which
    /// verifies the address against `proposed_request_id`.
    #[account(mut)]
    pub payout_claim: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// The daily window of DESIGN 10.6 for a payout of `amount` at `now`, computed without writing: returns the
/// `(window_start, window_total)` that would be stored. Order: roll the window first (`now >= window_start +
/// window_secs`), then `window_total + amount <= max_payout_per_window` or `DailyPayoutCapReached`.
pub fn window_after(config: &SectorConfig, now: i64, amount: u64) -> Result<(i64, u64)> {
    let ends = config.window_start.checked_add(i64::from(config.window_secs)).ok_or(Setl8Error::MathOverflow)?;
    let (start, total) = if now >= ends { (now, 0) } else { (config.window_start, config.window_total) };
    let new_total = total.checked_add(amount).ok_or(Setl8Error::MathOverflow)?;
    require!(new_total <= config.max_payout_per_window, Setl8Error::DailyPayoutCapReached);
    Ok((start, new_total))
}

/// Checks (in this order): not paused; Active; state 2; not held; delay `now >= pass_proposed_at + config.pass_delay_secs`
/// (the CURRENT config value); the pass condition still holds; the daily cap. Then `request_payout(amount = 0.08 S,
/// proposed_request_id = next_request_id)` and the return data: `Paid` bumps the tally, `next_request_id`, the window and
/// starts the next phase (or graduates after phase 5); `Abandoned` marks the account Abandoned and changes nothing else.
/// Any error reverts everything.
pub fn handle_finalize_pass(ctx: Context<FinalizePass>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(ctx.accounts.config.paused == 0, Setl8Error::Paused);
    let acct = &ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(acct.pass_pending == PASS_CONFIRMED, Setl8Error::NoPendingPass);
    require!(acct.held == 0, Setl8Error::AccountHeld);
    let eligible = acct
        .pass_proposed_at
        .checked_add(i64::from(ctx.accounts.config.pass_delay_secs))
        .ok_or(Setl8Error::MathOverflow)?;
    require!(now >= eligible, Setl8Error::PassDelayNotElapsed);
    // post_journal is frozen while confirmed, so this cannot fail unless the account was altered; it is the cheap guard
    check_pass_condition(acct)?;
    let amount = pass_amount(acct)?;
    require!(amount > 0, Setl8Error::WrongPhase);
    let next = engine::next_phase(acct.phase).map_err(|_| error!(Setl8Error::WrongPhase))?;
    let (window_start, window_total) = window_after(&ctx.accounts.config, now, amount)?;

    let account_key = acct.key();
    let trader = acct.trader;
    let (challenge_id, request_id, phase) = (acct.challenge_id, acct.next_request_id, acct.phase);
    let ix = request_payout(
        ctx.accounts.vault_program.key(),
        ctx.accounts.sector_authority.key(),
        ctx.accounts.product_registry.key(),
        &[
            AccountMeta::new(ctx.accounts.vault_trader_state.key(), false),
            AccountMeta::new(ctx.accounts.vault_state.key(), false),
            AccountMeta::new(ctx.accounts.payout_claim.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
        ],
        RequestPayoutArgs {
            trader_wallet: trader,
            amount,
            product_program_id: *ctx.program_id,
            challenge_id,
            proposed_request_id: request_id,
        },
    );
    clear_return_data();
    invoke_signed(
        &ix,
        &[
            ctx.accounts.sector_authority.to_account_info(),
            ctx.accounts.product_registry.to_account_info(),
            ctx.accounts.vault_trader_state.to_account_info(),
            ctx.accounts.vault_state.to_account_info(),
            ctx.accounts.payout_claim.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.vault_program.to_account_info(),
        ],
        &[&[SECTOR_AUTHORITY_SEED, &[ctx.bumps.sector_authority]]],
    )?;
    let outcome = PayoutOutcome::from_u8(vault_return(&ctx.accounts.vault_program.key())?)
        .ok_or_else(|| error!(Setl8Error::ReturnDataInvalid))?;

    match outcome {
        PayoutOutcome::Abandoned => {
            mark_abandoned(&mut ctx.accounts.trading_account, &mut ctx.accounts.wallet, now)?;
            emit_cpi!(AccountAbandoned { account: account_key, trader, time: now });
        }
        PayoutOutcome::Paid => {
            // the tally: checked adds, never saturating, never decreasing
            let info = ctx.accounts.payout_tally.to_account_info();
            let mut data = info.try_borrow_mut_data()?;
            require!(data.len() >= PAYOUT_TALLY_MIN_LEN, Setl8Error::PayoutTallyInvalid);
            let mut tally = PayoutTally::parse(&data).map_err(|_| error!(Setl8Error::PayoutTallyInvalid))?;
            tally.requested_count = tally.requested_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
            tally.requested_total = tally.requested_total.checked_add(amount).ok_or(Setl8Error::MathOverflow)?;
            tally.write_into(&mut data).map_err(|_| error!(Setl8Error::PayoutTallyInvalid))?;
            drop(data);

            let config = &mut ctx.accounts.config;
            config.window_start = window_start;
            config.window_total = window_total;

            let acct = &mut ctx.accounts.trading_account;
            acct.next_request_id = acct.next_request_id.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
            let to_phase = match next {
                engine::NextPhase::Next { phase: p, .. } => {
                    start_phase(acct, p)?;
                    p
                }
                engine::NextPhase::Graduated => {
                    end_account(acct, &mut ctx.accounts.wallet, STATUS_GRADUATED, now)?;
                    0
                }
            };
            emit_cpi!(PassFinalized {
                account: account_key,
                phase,
                to_phase,
                amount,
                request_id,
                tally_count: tally.requested_count,
                tally_total: tally.requested_total,
                window_total,
                time: now,
            });
            if to_phase != 0 {
                emit_cpi!(PhaseAdvanced { account: account_key, from_phase: phase, to_phase, time: now });
            }
        }
    }
    Ok(())
}
