//! Read-only views of the vault's accounts that `reset_account` needs (M8): the `ProductRegistry` (the tier list and the
//! reset ladder AS THEY ARE NOW) and the `TraderState` of the fresh challenge (`account_size`, `payout_count`).
//!
//! The vault's account structs are not part of `setl8-shared-interfaces`, so the Borsh layout is decoded by hand
//! (vault core commit 9a5c179). Every decoder is bounds-checked and only reads; a malformed or short account is
//! `InvalidVaultAccount`. The accounts themselves are verified by `check_registry` / `check_trader_state` (owner and PDA
//! address) BEFORE their bytes are trusted.
use anchor_lang::prelude::*;

use crate::errors::Setl8Error;

/// PDA seed of the vault's `ProductRegistry`: `["product_registry", product_program_id]`.
pub const VAULT_REGISTRY_SEED: &[u8] = b"product_registry";
/// Anchor discriminators of the vault's accounts: `sha256("account:<Name>")[..8]` (checked by a unit test).
pub const PRODUCT_REGISTRY_DISCRIMINATOR: [u8; 8] = [111, 200, 47, 24, 234, 152, 75, 133];
pub const TRADER_STATE_DISCRIMINATOR: [u8; 8] = [124, 33, 101, 17, 158, 79, 26, 140];
/// Byte offsets inside the vault `TraderState` (8 disc, 32 wallet, 32 product, 8 challenge_id, ...).
pub const VAULT_ACCOUNT_SIZE_OFFSET: usize = 80;
pub const VAULT_PAYOUT_COUNT_OFFSET: usize = 88;
/// Vault `TraderStatus::Active` as stored.
pub const VAULT_STATUS_ACTIVE: u8 = 0;
/// The vault registers at most this many tiers (its account space); a larger count in the bytes is a lie.
const MAX_TIERS: usize = 64;
/// ... and at most this many reset ladder entries.
const MAX_LADDER: usize = 64;

/// The tier list and the reset ladder of a registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryView {
    /// `(size, cost)` in registration order, micro-USD.
    pub tiers: Vec<(u64, u64)>,
    /// `reset_price_bps`, indexed by the 0-based phase the trader failed in.
    pub ladder: Vec<u16>,
}

/// A bounds-checked reader over a byte slice.
struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(Setl8Error::InvalidVaultAccount)?;
        let s = self.d.get(self.at..end).ok_or(Setl8Error::InvalidVaultAccount)?;
        self.at = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| error!(Setl8Error::InvalidVaultAccount))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array::<2>()?))
    }

    fn u8(&mut self) -> Result<u8> {
        let [b] = self.array::<1>()?;
        Ok(b)
    }
}

/// Decodes a `ProductRegistry`: discriminator, `product_program_id` (must be THIS program), `challenge_sizes`
/// (`Vec<{size u64, cost u64}>`), `fee_split_bps u16`, `max_payout_count u64`, `active bool`, `total_requests_emitted u64`,
/// `reset_price_bps Vec<u16>`. The trailing fields (pause bookkeeping, `bump`) are not read.
pub fn parse_registry(data: &[u8], product_program_id: &Pubkey) -> Result<RegistryView> {
    let mut r = Reader { d: data, at: 0 };
    require!(r.array::<8>()? == PRODUCT_REGISTRY_DISCRIMINATOR, Setl8Error::InvalidVaultAccount);
    require_keys_eq!(Pubkey::new_from_array(r.array::<32>()?), *product_program_id, Setl8Error::InvalidVaultAccount);
    let n = usize::try_from(r.u32()?).map_err(|_| error!(Setl8Error::InvalidVaultAccount))?;
    require!(n <= MAX_TIERS, Setl8Error::InvalidVaultAccount);
    let mut tiers = Vec::with_capacity(n);
    for _ in 0..n {
        let size = r.u64()?;
        let cost = r.u64()?;
        tiers.push((size, cost));
    }
    let _fee_split_bps = r.u16()?;
    let _max_payout_count = r.u64()?;
    let _active = r.u8()?;
    let _total_requests_emitted = r.u64()?;
    let m = usize::try_from(r.u32()?).map_err(|_| error!(Setl8Error::InvalidVaultAccount))?;
    require!(m <= MAX_LADDER, Setl8Error::InvalidVaultAccount);
    let mut ladder = Vec::with_capacity(m);
    for _ in 0..m {
        ladder.push(r.u16()?);
    }
    Ok(RegistryView { tiers, ladder })
}

/// The registry account of THIS product: owned by the vault program, at the PDA `["product_registry", program_id]` of the
/// vault. Never trust a passed-in account on its address alone.
pub fn check_registry(info: &AccountInfo, vault_program: &Pubkey, program_id: &Pubkey) -> Result<RegistryView> {
    let (expected, _) = Pubkey::find_program_address(&[VAULT_REGISTRY_SEED, program_id.as_ref()], vault_program);
    require_keys_eq!(expected, info.key(), Setl8Error::InvalidVaultAccount);
    require_keys_eq!(*info.owner, *vault_program, Setl8Error::InvalidVaultAccount);
    let data = info.try_borrow_data()?;
    parse_registry(&data, program_id)
}

/// What the fresh vault record says (read after `deposit_reset`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraderStateView {
    pub account_size: u64,
    pub payout_count: u64,
    pub status: u8,
}

/// Decodes the fields of a vault `TraderState` that `reset_account` compares: discriminator, wallet, product, challenge id,
/// `account_size`, `payout_count`, `status`.
pub fn parse_trader_state(data: &[u8], trader: &Pubkey, program_id: &Pubkey, challenge_id: u64) -> Result<TraderStateView> {
    let mut r = Reader { d: data, at: 0 };
    require!(r.array::<8>()? == TRADER_STATE_DISCRIMINATOR, Setl8Error::InvalidVaultAccount);
    require_keys_eq!(Pubkey::new_from_array(r.array::<32>()?), *trader, Setl8Error::InvalidVaultAccount);
    require_keys_eq!(Pubkey::new_from_array(r.array::<32>()?), *program_id, Setl8Error::InvalidVaultAccount);
    require!(r.u64()? == challenge_id, Setl8Error::InvalidVaultAccount);
    let account_size = r.u64()?;
    let payout_count = r.u64()?;
    let status = r.u8()?;
    Ok(TraderStateView { account_size, payout_count, status })
}

/// The vault `TraderState` PDA of `(program_id, trader, challenge_id)`: owned by the vault, at its derived address.
/// Returns the decoded record.
pub fn check_trader_state(
    info: &AccountInfo,
    vault_program: &Pubkey,
    program_id: &Pubkey,
    trader: &Pubkey,
    challenge_id: u64,
) -> Result<TraderStateView> {
    let (expected, _) = Pubkey::find_program_address(
        &[crate::constants::VAULT_TRADER_STATE_SEED, program_id.as_ref(), trader.as_ref(), &challenge_id.to_le_bytes()],
        vault_program,
    );
    require_keys_eq!(expected, info.key(), Setl8Error::InvalidVaultAccount);
    require_keys_eq!(*info.owner, *vault_program, Setl8Error::InvalidVaultAccount);
    let data = info.try_borrow_data()?;
    parse_trader_state(&data, trader, program_id, challenge_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sha256_hasher::hashv;

    const PROGRAM: Pubkey = Pubkey::new_from_array([9; 32]);
    const TRADER: Pubkey = Pubkey::new_from_array([4; 32]);

    fn registry_bytes(tiers: &[(u64, u64)], ladder: &[u16]) -> Vec<u8> {
        let mut d = PRODUCT_REGISTRY_DISCRIMINATOR.to_vec();
        d.extend_from_slice(PROGRAM.as_ref());
        d.extend_from_slice(&u32::try_from(tiers.len()).unwrap().to_le_bytes());
        for (s, c) in tiers {
            d.extend_from_slice(&s.to_le_bytes());
            d.extend_from_slice(&c.to_le_bytes());
        }
        d.extend_from_slice(&6500u16.to_le_bytes());
        d.extend_from_slice(&4u64.to_le_bytes());
        d.push(1);
        d.extend_from_slice(&7u64.to_le_bytes());
        d.extend_from_slice(&u32::try_from(ladder.len()).unwrap().to_le_bytes());
        for b in ladder {
            d.extend_from_slice(&b.to_le_bytes());
        }
        // pause bookkeeping and bump: never read
        d.extend_from_slice(&[0u8; 1 + 8 + 8 + 8 + 1]);
        d
    }

    fn state_bytes(size: u64, payouts: u64, status: u8, challenge_id: u64) -> Vec<u8> {
        let mut d = TRADER_STATE_DISCRIMINATOR.to_vec();
        d.extend_from_slice(TRADER.as_ref());
        d.extend_from_slice(PROGRAM.as_ref());
        d.extend_from_slice(&challenge_id.to_le_bytes());
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&payouts.to_le_bytes());
        d.push(status);
        d.extend_from_slice(&[0u8; 8 + 8 + 1 + 1]);
        d
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> u32 {
        match r {
            Err(Error::AnchorError(e)) => e.error_code_number,
            other => panic!("expected an Anchor error, got {other:?}"),
        }
    }

    #[test]
    fn the_discriminators_are_the_anchor_hashes() {
        assert_eq!(hashv(&[b"account:ProductRegistry"]).to_bytes()[..8], PRODUCT_REGISTRY_DISCRIMINATOR);
        assert_eq!(hashv(&[b"account:TraderState"]).to_bytes()[..8], TRADER_STATE_DISCRIMINATOR);
    }

    #[test]
    fn the_registry_decodes_tiers_and_ladder() {
        let tiers = [(2_500_000_000u64, 30_000_000u64), (100_000_000_000, 859_000_000)];
        let ladder = [10_000u16, 11_500, 12_000, 12_750, 13_000];
        let v = parse_registry(&registry_bytes(&tiers, &ladder), &PROGRAM).unwrap();
        assert_eq!(v.tiers, tiers.to_vec());
        assert_eq!(v.ladder, ladder.to_vec());
        let empty = parse_registry(&registry_bytes(&[], &[]), &PROGRAM).unwrap();
        assert!(empty.tiers.is_empty() && empty.ladder.is_empty());
    }

    #[test]
    fn a_registry_of_another_product_or_with_a_bad_header_is_refused() {
        let good = registry_bytes(&[(1, 2)], &[10_000]);
        let bad = u32::from(Setl8Error::InvalidVaultAccount);
        assert_eq!(code(parse_registry(&good, &Pubkey::new_from_array([8; 32]))), bad);
        let mut wrong_disc = good.clone();
        wrong_disc[0] ^= 1;
        assert_eq!(code(parse_registry(&wrong_disc, &PROGRAM)), bad);
        assert_eq!(code(parse_registry(&[], &PROGRAM)), bad);
    }

    #[test]
    fn a_truncated_or_inflated_registry_is_refused() {
        let good = registry_bytes(&[(1, 2), (3, 4)], &[10_000, 11_500]);
        let bad = u32::from(Setl8Error::InvalidVaultAccount);
        // cut anywhere before the end of the ladder: always InvalidVaultAccount, never a panic
        let ladder_end = good.len() - (1 + 8 + 8 + 8 + 1);
        for cut in 0..ladder_end {
            assert_eq!(code(parse_registry(&good[..cut], &PROGRAM)), bad, "cut at {cut}");
        }
        assert!(parse_registry(&good[..ladder_end], &PROGRAM).is_ok(), "the trailing bookkeeping is not read");
        // a tier count that promises more bytes than there are
        let mut lie = good.clone();
        lie[40..44].copy_from_slice(&1_000u32.to_le_bytes());
        assert_eq!(code(parse_registry(&lie, &PROGRAM)), bad);
        let mut huge = good.clone();
        huge[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(code(parse_registry(&huge, &PROGRAM)), bad);
    }

    #[test]
    fn the_tier_and_ladder_counts_are_capped() {
        let bad = u32::from(Setl8Error::InvalidVaultAccount);
        let tiers: Vec<(u64, u64)> = (0..=MAX_TIERS as u64).map(|i| (i, i)).collect();
        assert_eq!(code(parse_registry(&registry_bytes(&tiers, &[10_000]), &PROGRAM)), bad);
        let ok_tiers: Vec<(u64, u64)> = (0..MAX_TIERS as u64).map(|i| (i, i)).collect();
        assert!(parse_registry(&registry_bytes(&ok_tiers, &[10_000]), &PROGRAM).is_ok());
        let ladder = vec![10_000u16; MAX_LADDER + 1];
        assert_eq!(code(parse_registry(&registry_bytes(&[(1, 2)], &ladder), &PROGRAM)), bad);
        assert!(parse_registry(&registry_bytes(&[(1, 2)], &ladder[..MAX_LADDER]), &PROGRAM).is_ok());
    }

    #[test]
    fn the_trader_state_decodes_size_payouts_and_status() {
        let v = parse_trader_state(&state_bytes(100_000_000_000, 3, 0, 7), &TRADER, &PROGRAM, 7).unwrap();
        assert_eq!(v, TraderStateView { account_size: 100_000_000_000, payout_count: 3, status: 0 });
        // the byte offsets the module documents
        let d = state_bytes(0x0102_0304_0506_0708, 0x1112_1314_1516_1718, 2, 7);
        assert_eq!(d[VAULT_ACCOUNT_SIZE_OFFSET..VAULT_ACCOUNT_SIZE_OFFSET + 8], 0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(d[VAULT_PAYOUT_COUNT_OFFSET..VAULT_PAYOUT_COUNT_OFFSET + 8], 0x1112_1314_1516_1718u64.to_le_bytes());
        assert_eq!(d[crate::constants::VAULT_STATUS_OFFSET], 2);
    }

    #[test]
    fn a_trader_state_of_another_wallet_product_or_challenge_is_refused() {
        let bad = u32::from(Setl8Error::InvalidVaultAccount);
        let d = state_bytes(5, 1, 0, 7);
        assert_eq!(code(parse_trader_state(&d, &Pubkey::new_from_array([5; 32]), &PROGRAM, 7)), bad);
        assert_eq!(code(parse_trader_state(&d, &TRADER, &Pubkey::new_from_array([5; 32]), 7)), bad);
        assert_eq!(code(parse_trader_state(&d, &TRADER, &PROGRAM, 8)), bad);
        let mut wrong_disc = d.clone();
        wrong_disc[3] ^= 1;
        assert_eq!(code(parse_trader_state(&wrong_disc, &TRADER, &PROGRAM, 7)), bad);
        for cut in 0..=VAULT_STATUS_OFFSET_FOR_TEST {
            assert_eq!(code(parse_trader_state(&d[..cut], &TRADER, &PROGRAM, 7)), bad, "cut at {cut}");
        }
    }

    const VAULT_STATUS_OFFSET_FOR_TEST: usize = crate::constants::VAULT_STATUS_OFFSET;
}
