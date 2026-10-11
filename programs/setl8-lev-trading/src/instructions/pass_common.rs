//! Helpers shared by the pass-flow instructions (M6): return-data discipline (DESIGN 10.2), the vault
//! `record_activity` CPI, the phase reset and the wallet slot release.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::program::{get_return_data, invoke_signed, set_return_data};
use engine::{AccountState, Rules};
use setl8_shared_interfaces::{flag_trader_failed, record_activity, ActivityOutcome, FlagTraderFailedArgs, RecordActivityArgs};

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::state::{flag, TradingAccount, WalletProfile};

/// The single byte a vault instruction answered with, or `ReturnDataInvalid` when there is no return data, it comes from
/// another program than `vault`, or it is not exactly one byte. Pure, so it is unit-tested without a cluster.
pub fn parse_return(vault: &Pubkey, ret: Option<(Pubkey, Vec<u8>)>) -> Result<u8> {
    match ret {
        Some((program, data)) if program == *vault => match data.as_slice() {
            [b] => Ok(*b),
            _ => err!(Setl8Error::ReturnDataInvalid),
        },
        _ => err!(Setl8Error::ReturnDataInvalid),
    }
}

/// Clears the return data (it then belongs to THIS program), so a vault call that sets none cannot be mistaken for one
/// that did by leftovers of an earlier instruction of the transaction.
pub fn clear_return_data() {
    set_return_data(&[]);
}

/// Reads the return data of the vault CPI that just ran.
pub fn vault_return(vault: &Pubkey) -> Result<u8> {
    parse_return(vault, get_return_data())
}

/// `record_activity` through `sector_authority` and the outcome. `Recorded` and `Throttled` are both fine; `Abandoned` is
/// returned to the caller, which marks the local account. Any other byte aborts.
#[allow(clippy::too_many_arguments)]
pub fn record_activity_cpi<'info>(
    vault_program: &AccountInfo<'info>,
    sector_authority: &AccountInfo<'info>,
    authority_bump: u8,
    product_registry: &AccountInfo<'info>,
    vault_trader_state: &AccountInfo<'info>,
    trader: Pubkey,
    program_id: Pubkey,
    challenge_id: u64,
) -> Result<ActivityOutcome> {
    let ix = record_activity(
        vault_program.key(),
        sector_authority.key(),
        product_registry.key(),
        &[AccountMeta::new(vault_trader_state.key(), false)],
        RecordActivityArgs { trader_wallet: trader, product_program_id: program_id, challenge_id },
    );
    clear_return_data();
    invoke_signed(
        &ix,
        &[sector_authority.clone(), product_registry.clone(), vault_trader_state.clone(), vault_program.clone()],
        &[&[SECTOR_AUTHORITY_SEED, &[authority_bump]]],
    )?;
    let byte = vault_return(&vault_program.key())?;
    ActivityOutcome::from_u8(byte).ok_or_else(|| error!(Setl8Error::ReturnDataInvalid))
}

/// The wallet slot of an account that is no longer live (Failed, Abandoned, Graduated) is freed.
pub fn free_wallet_slot(wallet: &mut WalletProfile) -> Result<()> {
    wallet.active_count = wallet.active_count.checked_sub(1).ok_or(Setl8Error::MathOverflow)?;
    Ok(())
}

/// Ends an account for good (RESOLVE (b) of M7): `status` Failed, Abandoned or Graduated, `pass_pending` 0, and
/// `terminal_at` (stored in `pass_proposed_at`, which is meaningless once nothing can be pending) set to `now`. A hold on
/// the account is cleared with the wallet's `held_count` (checked), and the wallet slot is freed (checked).
pub fn end_account(acct: &mut TradingAccount, wallet: &mut WalletProfile, status: u8, now: i64) -> Result<()> {
    acct.status = status;
    acct.pass_pending = PASS_NONE;
    acct.pass_requested_at = 0;
    acct.pass_proposed_at = now;
    if flag(acct.held)? {
        acct.held = 0;
        wallet.held_count = wallet.held_count.checked_sub(1).ok_or(Setl8Error::MathOverflow)?;
    }
    free_wallet_slot(wallet)
}

/// The local account becomes Abandoned (the vault flipped it).
pub fn mark_abandoned(acct: &mut TradingAccount, wallet: &mut WalletProfile, now: i64) -> Result<()> {
    end_account(acct, wallet, STATUS_ABANDONED, now)
}

/// `flag_trader_failed` through `sector_authority` (the vault allows it while the product is paused). The vault sets no
/// return data for it.
#[allow(clippy::too_many_arguments)]
pub fn flag_failed_cpi<'info>(
    vault_program: &AccountInfo<'info>,
    sector_authority: &AccountInfo<'info>,
    authority_bump: u8,
    product_registry: &AccountInfo<'info>,
    vault_trader_state: &AccountInfo<'info>,
    trader: Pubkey,
    program_id: Pubkey,
    challenge_id: u64,
) -> Result<()> {
    let ix = flag_trader_failed(
        vault_program.key(),
        sector_authority.key(),
        product_registry.key(),
        &[AccountMeta::new(vault_trader_state.key(), false)],
        FlagTraderFailedArgs { trader_wallet: trader, product_program_id: program_id, challenge_id },
    );
    invoke_signed(
        &ix,
        &[sector_authority.clone(), product_registry.clone(), vault_trader_state.clone(), vault_program.clone()],
        &[&[SECTOR_AUTHORITY_SEED, &[authority_bump]]],
    )?;
    Ok(())
}

/// The vault's `TraderState` status byte (Borsh enum: Active 0, Graduated 1, Failed 2, Abandoned 3) read from the account
/// itself: the account must be owned by the vault and be the PDA the vault derives for this challenge. Used only to learn
/// that the permissionless `mark_abandoned` already flipped a challenge (then `record_activity` would just error).
pub fn vault_status(vault_program: &Pubkey, program_id: &Pubkey, trader: &Pubkey, challenge_id: u64, info: &AccountInfo) -> Result<u8> {
    let (expected, _) = Pubkey::find_program_address(&[VAULT_TRADER_STATE_SEED, program_id.as_ref(), trader.as_ref(), &challenge_id.to_le_bytes()], vault_program);
    require_keys_eq!(expected, info.key(), Setl8Error::InvalidVaultAccount);
    require_keys_eq!(*info.owner, *vault_program, Setl8Error::InvalidVaultAccount);
    let data = info.try_borrow_data()?;
    // 8 discriminator + 32 + 32 + 8 + 8 + 8, then the status byte
    match data.get(VAULT_STATUS_OFFSET) {
        Some(b) => Ok(*b),
        None => err!(Setl8Error::InvalidVaultAccount),
    }
}

/// Whether the quick tally is above the limit (strict, A-RULE-2): the engine's own `apply_closed_slice`.
pub fn quick_over(st: &AccountState) -> Result<bool> {
    let d = engine::apply_closed_slice(st, &Rules::default(), 0, 0).map_err(|_| error!(Setl8Error::MathOverflow))?;
    Ok(d.fail.is_some())
}

/// Whether the current phase is failed: closed balance at or below the static floor, or quick tally above the limit
/// (the engine's own `apply_closed_slice` decides the latter). An Active account in such a state is waiting for its
/// `report_fail`; it must not pass.
pub fn phase_failed(st: &AccountState) -> Result<bool> {
    let floor = engine::static_floor_micro(st).map_err(|_| error!(Setl8Error::MathOverflow))?;
    if st.b <= floor {
        return Ok(true);
    }
    quick_over(st)
}

/// The pass condition of DESIGN 2.5 on the stored account: not failed, flat, `b >= 1.08 S`. Maps the engine's reasons to
/// the program errors `PhaseFailed`, `NotFlat`, `TargetNotReached`.
pub fn check_pass_condition(acct: &TradingAccount) -> Result<AccountState> {
    let st = acct.to_engine_state()?;
    let failed = phase_failed(&st)?;
    engine::pass_check(&st, st.flat, failed).map_err(|e| match e {
        engine::PassError::PhaseFailed => error!(Setl8Error::PhaseFailed),
        engine::PassError::NotFlat => error!(Setl8Error::NotFlat),
        engine::PassError::TargetNotReached => error!(Setl8Error::TargetNotReached),
    })?;
    Ok(st)
}

/// The payout of this account's pass in micro-USD: `0.08 S` in phases 2 to 5, 0 in phase 1 (`engine::next_phase`).
pub fn pass_amount(acct: &TradingAccount) -> Result<u64> {
    let size = i64::try_from(acct.size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
    let pays = engine::next_phase(acct.phase).map_err(|_| error!(Setl8Error::WrongPhase))?.pays();
    if !pays {
        return Ok(0);
    }
    let amount = engine::payout_micro(size).map_err(|_| error!(Setl8Error::MathOverflow))?;
    u64::try_from(amount).map_err(|_| error!(Setl8Error::MathOverflow))
}

/// Starts phase `to_phase` fresh (RESOLVE (b)): `engine::reset_phase_state` (b = S, q = 0, latch 0, tight off with
/// r/g_peak/l_level 0, flat) with an ACTIVE LOCK KEPT (`lock_until` is not cleared), nothing pending. Seq, hash,
/// `last_batch_time`, `trades_closed`, `vetoes`, `next_request_id` and `last_activity_pushed` are untouched.
pub fn start_phase(acct: &mut TradingAccount, to_phase: u8) -> Result<()> {
    let size = i64::try_from(acct.size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
    let mut st = engine::reset_phase_state(size);
    st.lock_until = acct.lock_until;
    acct.apply_engine_state(&st)?;
    acct.phase = to_phase;
    acct.pass_pending = PASS_NONE;
    acct.pass_requested_at = 0;
    acct.pass_proposed_at = 0;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAULT: Pubkey = Pubkey::new_from_array([7; 32]);
    const OTHER: Pubkey = Pubkey::new_from_array([8; 32]);

    #[test]
    fn return_data_must_come_from_the_vault_with_one_byte() {
        assert_eq!(parse_return(&VAULT, Some((VAULT, vec![0]))).unwrap(), 0);
        assert_eq!(parse_return(&VAULT, Some((VAULT, vec![2]))).unwrap(), 2);
        let code = |r: Result<u8>| match r {
            Err(anchor_lang::error::Error::AnchorError(e)) => e.error_code_number,
            other => panic!("expected an error, got {other:?}"),
        };
        let want = u32::from(Setl8Error::ReturnDataInvalid);
        // wrong program, none, empty, two bytes
        assert_eq!(code(parse_return(&VAULT, Some((OTHER, vec![0])))), want);
        assert_eq!(code(parse_return(&VAULT, None)), want);
        assert_eq!(code(parse_return(&VAULT, Some((VAULT, vec![])))), want);
        assert_eq!(code(parse_return(&VAULT, Some((VAULT, vec![0, 0])))), want);
        // this program's own (cleared) return data is not the vault's
        assert_eq!(code(parse_return(&VAULT, Some((crate::ID, vec![])))), want);
    }

    fn acct() -> TradingAccount {
        let mut d = vec![0u8; TradingAccount::LEN];
        d[..8].copy_from_slice(<TradingAccount as anchor_lang::Discriminator>::DISCRIMINATOR);
        let mut a = TradingAccount::try_deserialize(&mut d.as_slice()).unwrap();
        a.size_s = 100_000_000_000;
        a.b = 108_000_000_000;
        a.flat = 1;
        a.phase = 3;
        a
    }

    #[test]
    fn start_phase_resets_the_engine_fields_and_keeps_the_rest() {
        let mut a = acct();
        a.latch = 1;
        a.q_tally = 5;
        a.lock_until = 99;
        a.seq = 12;
        a.journal_hash = [3; 32];
        a.trades_closed = 4;
        a.vetoes = 2;
        a.next_request_id = 3;
        a.last_batch_time = 777;
        a.pass_pending = PASS_CONFIRMED;
        a.pass_requested_at = 10;
        a.pass_proposed_at = 20;
        start_phase(&mut a, 4).unwrap();
        assert_eq!((a.b, a.q_tally, a.latch, a.tight_on, a.r, a.g_peak, a.l_level, a.flat), (100_000_000_000, 0, 0, 0, 0, 0, 0, 1));
        assert_eq!((a.phase, a.pass_pending, a.pass_requested_at, a.pass_proposed_at), (4, 0, 0, 0));
        assert_eq!((a.lock_until, a.seq, a.journal_hash, a.trades_closed, a.vetoes, a.next_request_id, a.last_batch_time), (99, 12, [3; 32], 4, 2, 3, 777));
    }

    #[test]
    fn pass_amount_is_zero_in_phase_one_and_eight_percent_after() {
        let mut a = acct();
        a.phase = 1;
        assert_eq!(pass_amount(&a).unwrap(), 0);
        for p in 2..=5 {
            a.phase = p;
            assert_eq!(pass_amount(&a).unwrap(), 8_000_000_000);
        }
        a.phase = 6;
        assert!(pass_amount(&a).is_err());
        a.phase = 0;
        assert!(pass_amount(&a).is_err());
    }

    #[test]
    fn pass_condition_boundaries() {
        let mut a = acct();
        a.b = 108_000_000_000;
        assert!(check_pass_condition(&a).is_ok());
        a.b = 108_000_000_000 - 1;
        assert!(check_pass_condition(&a).is_err());
        a.b = 108_000_000_000;
        a.flat = 0;
        assert!(check_pass_condition(&a).is_err());
        a.flat = 1;
        a.q_tally = 2_800_000_001;
        assert!(phase_failed(&a.to_engine_state().unwrap()).unwrap());
        a.q_tally = 2_800_000_000;
        assert!(!phase_failed(&a.to_engine_state().unwrap()).unwrap());
    }

    #[test]
    fn wallet_slot_cannot_underflow() {
        let mut w = WalletProfile { trader: Pubkey::default(), next_challenge_id: 0, active_count: 1, bump: 0, banned: 0, held_count: 0, pad: [0; 4] };
        free_wallet_slot(&mut w).unwrap();
        assert_eq!(w.active_count, 0);
        assert!(free_wallet_slot(&mut w).is_err());
    }
}
