//! DESIGN 6.2 `reset_account` (M8): the trader pays the phase-reset price for a locally Failed account and gets a NEW
//! account in the SAME phase with fresh engine state. The price is the vault's (`deposit_reset`), computed here with
//! `reset_price::reset_amount_micro` from the registry's tier list and ladder as they are now; the 0.4% fee goes on top
//! (assumption A-FEE-1, the one `rs.fee_bps` that `open_account` uses). Payout progress is carried over by the VAULT
//! (`payout_count` and `account_size` are copied from the failed record); this instruction reads the result back.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::{transfer_checked, Mint, Token, TokenAccount, TransferChecked};
use journal_types::genesis_hash;
use setl8_shared_interfaces::{deposit_reset, DepositResetArgs};
use solana_sha256_hasher::hashv;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::{AccountReset, Opened};
use crate::reset_price::reset_amount_micro;
use crate::state::{FreshAccount, RuleSet, SectorConfig, TradingAccount, WalletProfile};
use crate::vault_view::{check_registry, check_trader_state};

/// Account order: the accounts of `open_account` in their order, with the previous local account after the wallet and the
/// previous vault record after the new one.
#[event_cpi]
#[derive(Accounts)]
pub struct ResetAccount<'info> {
    /// The trader. Signs the token transfers and is the `trader_wallet` of both vault records.
    #[account(mut)]
    pub trader: Signer<'info>,
    /// Pays the rent of the new accounts. Must be the trader (checked in the handler).
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// The rule set of `config.current_ruleset` (version checked in the handler): the fee rate and the snapshot of the new account.
    pub ruleset: Box<Account<'info, RuleSet>>,
    /// The wallet must exist: a Failed account of this trader is what is being reset.
    #[account(mut, seeds = [WALLET_SEED, trader.key().as_ref()], bump = wallet.bump)]
    pub wallet: Box<Account<'info, WalletProfile>>,
    /// The Failed account being reset. Marked reset (`reset_at`) in the same instruction.
    #[account(
        mut,
        seeds = [ACCT_SEED, prev_trading_account.trader.as_ref(), &prev_trading_account.challenge_id.to_le_bytes()],
        bump = prev_trading_account.bump,
        constraint = prev_trading_account.trader == trader.key() @ Setl8Error::Unauthorized
    )]
    pub prev_trading_account: Box<Account<'info, TradingAccount>>,
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
    /// CHECK: the vault's `ProductRegistry` of THIS product: owner, PDA address and header are checked in the handler
    /// (`vault_view::check_registry`) before its tier list and ladder are used; the vault verifies it again.
    #[account(mut)]
    pub product_registry: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of the NEW challenge; created by the vault, read back after the CPI.
    #[account(mut)]
    pub vault_trader_state: UncheckedAccount<'info>,
    /// CHECK: the vault's `TraderState` of the FAILED challenge; owner and PDA address checked in the handler, marked
    /// reset-used by the vault.
    #[account(mut)]
    pub prev_vault_trader_state: UncheckedAccount<'info>,
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

/// Checks run in this order: sector not paused; payer is the trader; rule set is the current one; wallet not banned; the
/// previous account is the one named, locally `Failed`, not failed for a violation (`ViolationNoReset`, decision 60), not still held (`ReviewPending`, decision 67 amendment) and not reset yet; 5-account cap (the failed account is not
/// counted, the new one will be); the registry is the vault-owned PDA of THIS product; the price
/// (`ResetTierInvalid`, `ResetPhaseOutOfRange`, `ResetPriceOverflow`); the fee. Then the vault `deposit_reset` CPI (signed
/// by `sector_authority`, never with `payout_count` or `account_size`), the read-back of the vault's `payout_count`, the 0.4%
/// fee (rounded UP) trader -> SL8 wallet, the new account (same phase, fresh state) and the mark on the previous one.
/// Any failure reverts everything.
pub fn handle_reset_account(ctx: Context<ResetAccount>, prev_challenge_id: u64) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config = &ctx.accounts.config;
    require!(config.paused == 0, Setl8Error::Paused);
    require_keys_eq!(ctx.accounts.payer.key(), ctx.accounts.trader.key(), Setl8Error::Unauthorized);

    // the rule set must be the current one (immutable, created only at its own PDA, so version + bump prove it)
    let rs = &ctx.accounts.ruleset;
    require!(config.current_ruleset != 0 && rs.version == config.current_ruleset, Setl8Error::RuleSetMissing);
    let expected = Pubkey::create_program_address(&[RULESET_SEED, &rs.version.to_le_bytes(), &[rs.bump]], ctx.program_id)
        .map_err(|_| error!(Setl8Error::RuleSetMissing))?;
    require_keys_eq!(expected, rs.key(), Setl8Error::RuleSetMissing);

    let wallet = &mut ctx.accounts.wallet;
    require!(wallet.banned == 0, Setl8Error::WalletBanned);

    // the previous account: the one named, Failed, not reset yet (the failed account was not counted in `active_count`)
    let prev = &ctx.accounts.prev_trading_account;
    require!(prev.challenge_id == prev_challenge_id, Setl8Error::ResetNotAllowed);
    require!(prev.status == STATUS_FAILED, Setl8Error::ResetNotAllowed);
    // DESIGN 2.9: no reset after a violation, whatever the wallet's ban says now (decision 60); before any CPI
    require!(!prev.violation_failed(), Setl8Error::ViolationNoReset);
    // P25c (decision 67 amendment): an account failed while held waits for the two admins; before any CPI
    require!(prev.held == 0, Setl8Error::ReviewPending);
    require!(prev.reset_at() == 0, Setl8Error::AccountAlreadyReset);
    let (prev_phase, size_s) = (prev.phase, prev.size_s);
    require!(wallet.active_count < MAX_ACTIVE_ACCOUNTS, Setl8Error::WalletCapReached);

    // the registry and the price, read NOW; the only pricing code is `reset_amount_micro`
    let vault = ctx.accounts.vault_program.key();
    let registry = check_registry(&ctx.accounts.product_registry.to_account_info(), &vault, ctx.program_id)?;
    let trader = ctx.accounts.trader.key();
    check_trader_state(&ctx.accounts.prev_vault_trader_state.to_account_info(), &vault, ctx.program_id, &trader, prev_challenge_id)?;
    let phase_idx = usize::from(prev_phase.checked_sub(1).ok_or(Setl8Error::ResetPhaseOutOfRange)?);
    let amount = reset_amount_micro(&registry.tiers, size_s, phase_idx, &registry.ladder)?;
    let fee = engine::swap_fee_micro(amount, rs.fee_bps).map_err(|_| error!(Setl8Error::MathOverflow))?;
    let reset_phase = u8::try_from(phase_idx).map_err(|_| error!(Setl8Error::ResetPhaseOutOfRange))?;

    let challenge_id = wallet.next_challenge_id;
    wallet.next_challenge_id = challenge_id.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
    wallet.active_count = wallet.active_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;

    // the vault takes the price; it copies `payout_count` and `account_size` from the failed record itself
    let ix = deposit_reset(
        vault,
        ctx.accounts.sector_authority.key(),
        ctx.accounts.product_registry.key(),
        &[
            AccountMeta::new(ctx.accounts.prev_vault_trader_state.key(), false),
            AccountMeta::new(ctx.accounts.vault_trader_state.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
            AccountMeta::new_readonly(ctx.accounts.vault_state.key(), false),
            AccountMeta::new_readonly(trader, true),
            AccountMeta::new(ctx.accounts.trader_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.mint.key(), false),
            AccountMeta::new(ctx.accounts.pool_token_account.key(), false),
            AccountMeta::new(ctx.accounts.vault_sl8_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
        ],
        DepositResetArgs {
            amount,
            trader_wallet: trader,
            product_program_id: *ctx.program_id,
            prev_challenge_id,
            new_challenge_id: challenge_id,
            reset_phase,
        },
    );
    let infos = [
        ctx.accounts.sector_authority.to_account_info(),
        ctx.accounts.product_registry.to_account_info(),
        ctx.accounts.prev_vault_trader_state.to_account_info(),
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
    invoke_signed(&ix, &infos, &[&[SECTOR_AUTHORITY_SEED, &[ctx.bumps.sector_authority]]])?;

    // the payouts already taken: what the VAULT recorded for the new challenge (it copied them from the failed record)
    let fresh = check_trader_state(&ctx.accounts.vault_trader_state.to_account_info(), &vault, ctx.program_id, &trader, challenge_id)?;
    // the vault must have copied the size of the account being reset: the price paid is the price of THIS size
    require!(fresh.account_size == size_s, Setl8Error::InvalidVaultAccount);
    let next_request_id = fresh.payout_count.checked_add(1).ok_or(Setl8Error::MathOverflow)?;

    // the 0.4% fee on top of the reset price: ceil(price x fee_bps / 10_000), a plain SPL transfer signed by the trader
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

    // the new account: the SAME phase, fresh engine state, a new journal
    let account_key = ctx.accounts.trading_account.key();
    let h0 = genesis_hash(&|parts: &[&[u8]]| hashv(parts).to_bytes(), &account_key.to_bytes());
    let ruleset_version = rs.version;
    let tier_count = usize::from(rs.tier_count);
    let tier_index = rs.tier_size.iter().take(tier_count).position(|s| *s == size_s).and_then(|i| u8::try_from(i).ok()).unwrap_or(u8::MAX);
    ctx.accounts.trading_account.init_fresh(&FreshAccount {
        trader,
        challenge_id,
        size_s,
        now,
        genesis_hash: h0,
        ruleset_version,
        phase: prev_phase,
        next_request_id,
        bump: ctx.bumps.trading_account,
    })?;
    // the previous account can never be reset again (the vault's `ResetNotAllowed` is the backstop)
    let prev_account_key = ctx.accounts.prev_trading_account.key();
    ctx.accounts.prev_trading_account.pass_requested_at = now.max(1);

    emit_cpi!(Opened {
        account: account_key,
        trader,
        challenge_id,
        size_s,
        tier_index,
        phase: prev_phase,
        ruleset_version,
        genesis_hash: h0,
        time: now,
    });
    emit_cpi!(AccountReset {
        prev_account: prev_account_key,
        account: account_key,
        trader,
        prev_challenge_id,
        challenge_id,
        phase: prev_phase,
        price: amount,
        fee,
        payout_count: fresh.payout_count,
        time: now,
    });
    Ok(())
}
