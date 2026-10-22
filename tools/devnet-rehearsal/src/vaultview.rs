//! The vault's accounts decoded by hand from their byte layouts (the vault crate is not a dependency of this tool): the product
//! registry, a trader's record and a payout claim. Offsets as in the LiteSVM harness (`tests-rs/tests/common/vault.rs`, `pass.rs`).
use anchor_lang::solana_program::pubkey::Pubkey;

#[derive(Debug, PartialEq, Eq)]
pub struct Registry {
    pub product_program_id: Pubkey,
    pub tiers: Vec<(u64, u64)>,
    pub fee_split_bps: u16,
    pub max_payout_count: u64,
    pub active: bool,
    pub total_requests_emitted: u64,
    pub reset_price_bps: Vec<u16>,
}

pub fn parse_registry(d: &[u8]) -> Option<Registry> {
    let u64_at = |o: usize| d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])) as usize);
    let mut o = 8;
    let product_program_id = Pubkey::new_from_array(d.get(o..o + 32)?.try_into().ok()?);
    o += 32;
    let n = u32_at(o)?;
    o += 4;
    let mut tiers = Vec::new();
    for _ in 0..n {
        tiers.push((u64_at(o)?, u64_at(o + 8)?));
        o += 16;
    }
    let fee_split_bps = u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?);
    o += 2;
    let max_payout_count = u64_at(o)?;
    o += 8;
    let active = *d.get(o)? != 0;
    o += 1;
    let total_requests_emitted = u64_at(o)?;
    o += 8;
    let m = u32_at(o)?;
    o += 4;
    let mut reset_price_bps = Vec::new();
    for i in 0..m {
        reset_price_bps.push(u16::from_le_bytes(d.get(o + 2 * i..o + 2 * i + 2)?.try_into().ok()?));
    }
    Some(Registry { product_program_id, tiers, fee_split_bps, max_payout_count, active, total_requests_emitted, reset_price_bps })
}

/// The vault's `TraderState`: status 2 = Failed (as the harness reads it).
#[derive(Debug, PartialEq, Eq)]
pub struct TraderState {
    pub trader_wallet: Pubkey,
    pub product_program_id: Pubkey,
    pub challenge_id: u64,
    pub account_size: u64,
    pub payout_count: u64,
    pub status: u8,
    pub last_activity_timestamp: i64,
}

pub fn parse_trader_state(d: &[u8]) -> Option<TraderState> {
    if d.len() < 115 {
        return None;
    }
    let u64_at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap_or([0; 8]));
    Some(TraderState {
        trader_wallet: Pubkey::new_from_array(d[8..40].try_into().ok()?),
        product_program_id: Pubkey::new_from_array(d[40..72].try_into().ok()?),
        challenge_id: u64_at(72),
        account_size: u64_at(80),
        payout_count: u64_at(88),
        status: d[96],
        last_activity_timestamp: i64::from_le_bytes(d[97..105].try_into().ok()?),
    })
}

/// A payout claim: who is owed, for which request, how much.
#[derive(Debug, PartialEq, Eq)]
pub struct Claim {
    pub trader_wallet: Pubkey,
    pub trader_state: Pubkey,
    pub request_id: u64,
    pub owed: u64,
}

pub fn parse_claim(d: &[u8]) -> Option<Claim> {
    if d.len() < 120 {
        return None;
    }
    let u64_at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap_or([0; 8]));
    Some(Claim { trader_wallet: Pubkey::new_from_array(d[8..40].try_into().ok()?), trader_state: Pubkey::new_from_array(d[40..72].try_into().ok()?), request_id: u64_at(104), owed: u64_at(112) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_is_decoded_from_its_layout() {
        let prog = Pubkey::new_unique();
        let mut d = vec![0u8; 8];
        d.extend_from_slice(prog.as_ref());
        d.extend_from_slice(&2u32.to_le_bytes());
        for (s, c) in [(2_500u64, 30u64), (5_000, 55)] {
            d.extend_from_slice(&s.to_le_bytes());
            d.extend_from_slice(&c.to_le_bytes());
        }
        d.extend_from_slice(&6500u16.to_le_bytes());
        d.extend_from_slice(&4u64.to_le_bytes());
        d.push(1);
        d.extend_from_slice(&9u64.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        for b in [10_000u16, 11_500, 12_000] {
            d.extend_from_slice(&b.to_le_bytes());
        }
        let r = parse_registry(&d).unwrap();
        assert_eq!(r, Registry { product_program_id: prog, tiers: vec![(2_500, 30), (5_000, 55)], fee_split_bps: 6500, max_payout_count: 4, active: true, total_requests_emitted: 9, reset_price_bps: vec![10_000, 11_500, 12_000] });
        assert!(parse_registry(&d[..60]).is_none(), "a truncated account is refused, not read out of range");
    }

    #[test]
    fn a_trader_state_and_a_claim_are_decoded_at_their_offsets() {
        let mut d = vec![0u8; 115];
        d[8..40].copy_from_slice(&[1; 32]);
        d[40..72].copy_from_slice(&[2; 32]);
        d[72..80].copy_from_slice(&5u64.to_le_bytes());
        d[80..88].copy_from_slice(&2_500u64.to_le_bytes());
        d[88..96].copy_from_slice(&1u64.to_le_bytes());
        d[96] = 2;
        d[97..105].copy_from_slice(&77i64.to_le_bytes());
        let s = parse_trader_state(&d).unwrap();
        assert_eq!((s.challenge_id, s.account_size, s.payout_count, s.status, s.last_activity_timestamp), (5, 2_500, 1, 2, 77));
        assert!(parse_trader_state(&d[..100]).is_none());
        let mut c = vec![0u8; 154];
        c[104..112].copy_from_slice(&3u64.to_le_bytes());
        c[112..120].copy_from_slice(&200_000_000u64.to_le_bytes());
        let cl = parse_claim(&c).unwrap();
        assert_eq!((cl.request_id, cl.owed), (3, 200_000_000));
        assert!(parse_claim(&c[..100]).is_none());
    }
}
