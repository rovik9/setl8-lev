use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::{transfer_checked, Mint, Token, TokenAccount, TransferChecked};
use journal_types::genesis_hash;
use solana_sha256_hasher::hashv;
use setl8_shared_interfaces::{deposit_fee, DepositFeeArgs};

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::Opened;
use crate::state::{FreshAccount, RuleSet, SectorConfig, TradingAccount, WalletProfile};

/// DESIGN 6.2 `open_account`. Account order is the order of the fields below.
#[event_cpi]
#[derive(Accounts)]
pub struct OpenAccount<'info> {
    /// The buyer. Signs the token transfers and is the `trader_wallet` of the vault record.
    #[account(mut)]
    pub trader: Signer<'info>,
    /// Pays the rent of the new accounts. Must be the trader (checked in the handler).
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// The rule set of `config.current_ruleset` (version checked in the handler).
    pub ruleset: Box<Account<'info, RuleSet>>,
    #[account(
        init_if_needed,
        payer = payer,
        space = WalletProfile::SIZE,
        seeds = [WALLET_SEED, trader.key().as_ref()],
        bump
    )]
    pub wallet: Box<Account<'info, WalletProfile>>,
    #[account(
        init,
        payer = payer,
        space = TradingAccount::LEN,
        seeds = [ACCT_SEED, trader.key().as_ref(), &wallet.next_challenge_id.to_le_bytes()],
        bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    /// CHECK: PDA `["setl8_sector_authority"]` of this program; signs the vault CPI.
    #[account(seeds = [SECTOR_AUTHORITY_SEED], bump)]
    pub sector_authority: UncheckedAccount<'info>,
    /// CHECK: must be the vault program stored in the config.
    #[account(address = config.vault_program @ Setl8Error::InvalidVaultProgram)]
    pub vault_program: UncheckedAccount<'info>,
    /// CHECK: the vault's `ProductRegistry` for this program; the vault verifies its seeds.
    #[account(mut)]
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's new `TraderState`; created by the vault, which verifies its seeds.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
    /// CHECK: the vault's `VaultState`; verified by the vault.
    pub vault_state: UncheckedAccount<'info>,
    #[account(
        mut,
        constraint = trader_token_account.owner == trader.key() && trader_token_account.mint == mint.key()
            @ Setl8Error::InvalidTokenAccount
    )]
    pub trader_token_account: Box<Account<'info, TokenAccount>>,
    /// The stablecoin; the vault accepts only its own two mints and rejects the rest.
    pub mint: Box<Account<'info, Mint>>,
    /// CHECK: the vault's payout pool for `mint`; verified by the vault.
    #[account(mut)]
    pub pool_token_account: UncheckedAccount<'info>,
    /// CHECK: the vault's SL8 destination (owned by the vault's `sl8_wallet`); verified by the vault.
    #[account(mut)]
    pub vault_sl8_token_account: UncheckedAccount<'info>,
    /// Receives the 0.4% swap-cost fee: a `mint` token account owned by `config.sl8_wallet`.
    #[account(
        mut,
        constraint = fee_token_account.owner == config.sl8_wallet && fee_token_account.mint == mint.key()
            @ Setl8Error::InvalidTokenAccount
    )]
    pub fee_token_account: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Checks run in this order: paused; payer is the trader; rule set is the current one; tier index; banned wallet;
/// 5-account cap. Then the account is initialised, the 0.4% fee (rounded UP) moves trader -> SL8 wallet, and the vault
/// `deposit_fee` CPI takes the price (`amount = tier_cost`, never anything else). Any failure reverts everything.
pub fn handle_open_account(ctx: Context<OpenAccount>, tier_index: u8) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config = &ctx.accounts.config;
    require!(config.paused == 0, Setl8Error::Paused);
    require_keys_eq!(ctx.accounts.payer.key(), ctx.accounts.trader.key(), Setl8Error::Unauthorized);

    // the rule set must be the current one (it is immutable and only created at its own PDA, so version + bump prove it)
    let rs = &ctx.accounts.ruleset;
    require!(config.current_ruleset != 0 && rs.version == config.current_ruleset, Setl8Error::RuleSetMissing);
    let expected = Pubkey::create_program_address(&[RULESET_SEED, &rs.version.to_le_bytes(), &[rs.bump]], ctx.program_id)
        .map_err(|_| error!(Setl8Error::RuleSetMissing))?;
    require_keys_eq!(expected, rs.key(), Setl8Error::RuleSetMissing);

    // tier
    require!(tier_index < rs.tier_count, Setl8Error::InvalidTier);
    let idx = usize::from(tier_index);
    let size_s = *rs.tier_size.get(idx).ok_or(Setl8Error::InvalidTier)?;
    let tier_cost = *rs.tier_cost.get(idx).ok_or(Setl8Error::InvalidTier)?;

    // wallet profile: created by `init_if_needed` with all-zero data on first use
    let wallet = &mut ctx.accounts.wallet;
    if wallet.trader == Pubkey::default() {
        wallet.trader = ctx.accounts.trader.key();
        wallet.next_challenge_id = 0;
        wallet.active_count = 0;
        wallet.banned = 0;
        wallet.bump = ctx.bumps.wallet;
        wallet.held_count = 0;
        wallet.pad = [0; 4];
    }
    require!(wallet.banned == 0, Setl8Error::WalletBanned);
    require!(wallet.active_count < MAX_ACTIVE_ACCOUNTS, Setl8Error::WalletCapReached);
    let challenge_id = wallet.next_challenge_id;
    wallet.next_challenge_id = challenge_id.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
    wallet.active_count = wallet.active_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;

    // the 0.4% fee: ceil(cost x fee_bps / 10_000), a plain SPL transfer signed by the trader
    let fee = engine::swap_fee_micro(tier_cost, rs.fee_bps).map_err(|_| error!(Setl8Error::MathOverflow))?;
    if fee > 0 {
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.trader_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.fee_token_account.to_account_info(),
                    authority: ctx.accounts.trader.to_account_info(),
                },
            ),
            fee,
            ctx.accounts.mint.decimals,
        )?;
    }

    // the vault takes the price: amount == tier_cost, account_size == S, signed by sector_authority
    let ix = deposit_fee(
        ctx.accounts.vault_program.key(),
        ctx.accounts.sector_authority.key(),
        ctx.accounts.product_registry.key(),
        &[
            AccountMeta::new(ctx.accounts.vault_trader_state.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
            AccountMeta::new_readonly(ctx.accounts.vault_state.key(), false),
            AccountMeta::new_readonly(ctx.accounts.trader.key(), true),
            AccountMeta::new(ctx.accounts.trader_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.mint.key(), false),
            AccountMeta::new(ctx.accounts.pool_token_account.key(), false),
            AccountMeta::new(ctx.accounts.vault_sl8_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
        ],
        DepositFeeArgs {
            amount: tier_cost,
            product_program_id: *ctx.program_id,
            challenge_id,
            trader_wallet: ctx.accounts.trader.key(),
            account_size: size_s,
        },
    );
    let infos = [
        ctx.accounts.sector_authority.to_account_info(),
        ctx.accounts.product_registry.to_account_info(),
        ctx.accounts.vault_trader_state.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        ctx.accounts.vault_state.to_account_info(),
        ctx.accounts.trader.to_account_info(),
        ctx.accounts.trader_token_account.to_account_info(),
        ctx.accounts.mint.to_account_info(),
        ctx.accounts.pool_token_account.to_account_info(),
        ctx.accounts.vault_sl8_token_account.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.vault_program.to_account_info(),
    ];
    let (_, authority_bump) = Pubkey::find_program_address(&[SECTOR_AUTHORITY_SEED], ctx.program_id);
    invoke_signed(&ix, &infos, &[&[SECTOR_AUTHORITY_SEED, &[authority_bump]]])?;

    // initialise the account: B = S, phase 1, seq 0, journal_hash = h_0 (sol_sha256 through the Sha256 trait)
    let account_key = ctx.accounts.trading_account.key();
    let h0 = genesis_hash(&|parts: &[&[u8]]| hashv(parts).to_bytes(), &account_key.to_bytes());
    let ruleset_version = rs.version;
    let acct = &mut ctx.accounts.trading_account;
    acct.init_fresh(&FreshAccount {
        trader: ctx.accounts.trader.key(),
        challenge_id,
        size_s,
        now,
        genesis_hash: h0,
        ruleset_version,
        phase: 1,
        next_request_id: 1,
        bump: ctx.bumps.trading_account,
    })?;

    emit_cpi!(Opened {
        account: account_key,
        trader: ctx.accounts.trader.key(),
        challenge_id,
        size_s,
        tier_index,
        phase: 1,
        ruleset_version,
        genesis_hash: h0,
        time: now,
    });
    Ok(())
}
