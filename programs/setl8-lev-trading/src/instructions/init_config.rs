use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Allocate, Assign, CreateAccount, Transfer};
use setl8_shared_interfaces::{PayoutTally, PAYOUT_TALLY_MIN_LEN};

use super::admin_action::check_limits;
use crate::constants::*;
use crate::errors::Setl8Error;
use crate::state::SectorConfig;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitConfigArgs {
    pub evaluator: Pubkey,
    pub vault_program: Pubkey,
    pub sl8_wallet: Pubkey,
    pub pass_delay_secs: u32,
    pub max_payout_per_window: u64,
    pub window_secs: u32,
}

/// Both signers are checked against the COMPILED-IN admin keys (never against caller input),
/// so nobody else can create the singleton first.
#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(address = SL8_ADMIN @ Setl8Error::Unauthorized)]
    pub sl8_admin: Signer<'info>,
    #[account(address = ROV_ADMIN @ Setl8Error::Unauthorized)]
    pub rov_admin: Signer<'info>,
    #[account(
        init,
        payer = payer,
        space = SectorConfig::SIZE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, SectorConfig>,
    /// CHECK: PDA `["payout_tally"]`, created here as a raw 25-byte account owned by this program
    /// (no Anchor discriminator; layout owned by `setl8_shared_interfaces::PayoutTally`).
    #[account(mut, seeds = [PAYOUT_TALLY_SEED], bump)]
    pub payout_tally: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handle_init_config(ctx: Context<InitConfig>, args: InitConfigArgs) -> Result<()> {
    check_limits(args.pass_delay_secs, args.max_payout_per_window, args.window_secs)?;
    require!(args.evaluator != Pubkey::default(), Setl8Error::Unauthorized);
    require!(args.vault_program != Pubkey::default(), Setl8Error::Unauthorized);
    require!(args.sl8_wallet != Pubkey::default(), Setl8Error::Unauthorized);

    create_payout_tally(&ctx)?;

    let config = &mut ctx.accounts.config;
    config.admin_a = SL8_ADMIN;
    config.admin_b = ROV_ADMIN;
    config.evaluator = args.evaluator;
    config.vault_program = args.vault_program;
    config.sl8_wallet = args.sl8_wallet;
    config.current_ruleset = 0;
    config.market_count = 0;
    config.paused = 0;
    config.bump = ctx.bumps.config;
    config.pass_delay_secs = args.pass_delay_secs;
    config.max_payout_per_window = args.max_payout_per_window;
    config.window_secs = args.window_secs;
    config.window_start = 0;
    config.window_total = 0;
    config.reserved = [0; 64];
    Ok(())
}

/// Creates the tally PDA (25 bytes, owner = this program) and writes 0/0. If someone sent lamports
/// to the address beforehand, `create_account` would fail, so that case tops up, allocates and assigns instead.
fn create_payout_tally(ctx: &Context<InitConfig>) -> Result<()> {
    let tally = ctx.accounts.payout_tally.to_account_info();
    let payer = ctx.accounts.payer.to_account_info();
    let system = ctx.accounts.system_program.to_account_info();
    let seeds: &[&[u8]] = &[PAYOUT_TALLY_SEED, &[ctx.bumps.payout_tally]];
    let signer = &[seeds];
    let space = PAYOUT_TALLY_MIN_LEN as u64;
    let needed = Rent::get()?.minimum_balance(PAYOUT_TALLY_MIN_LEN);

    if tally.lamports() == 0 {
        system_program::create_account(
            CpiContext::new_with_signer(
                system.clone(),
                CreateAccount { from: payer, to: tally.clone() },
                signer,
            ),
            needed,
            space,
            &crate::ID,
        )?;
    } else {
        let shortfall = needed.saturating_sub(tally.lamports());
        if shortfall > 0 {
            system_program::transfer(
                CpiContext::new(system.clone(), Transfer { from: payer, to: tally.clone() }),
                shortfall,
            )?;
        }
        system_program::allocate(
            CpiContext::new_with_signer(system.clone(), Allocate { account_to_allocate: tally.clone() }, signer),
            space,
        )?;
        system_program::assign(
            CpiContext::new_with_signer(system, Assign { account_to_assign: tally.clone() }, signer),
            &crate::ID,
        )?;
    }

    let mut data = tally.try_borrow_mut_data()?;
    PayoutTally::default()
        .write_into(&mut data)
        .map_err(|_| ProgramError::InvalidAccountData)?;
    Ok(())
}
