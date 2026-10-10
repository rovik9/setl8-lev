use anchor_lang::prelude::*;

/// DESIGN 5.4. PDA `["ruleset", version u16 LE]`; 552 bytes including the discriminator; immutable after creation
/// (there is no update instruction).
#[account]
pub struct RuleSet {
    pub version: u16,
    /// Swap-cost fee in basis points (launch: 40).
    pub fee_bps: u16,
    pub tier_count: u8,
    pub ladder_len: u8,
    /// Maximum leverage per market class (launch: 50, 20, 20, 20, 10, 5, 5, 2).
    pub leverage_x: [u16; 8],
    /// Holding fee per night per class, e6 of notional.
    pub holding_fee_e6: [u32; 8],
    /// Commission per class, e6 (PENDING numbers).
    pub commission_e6: [u32; 8],
    /// Account sizes in micro-USD, strictly increasing over the first `tier_count`.
    pub tier_size: [u64; 10],
    /// Account prices in micro-USD, strictly increasing over the first `tier_count`.
    pub tier_cost: [u64; 10],
    /// Reset price as bps of the tier cost, per phase, non-decreasing over the first `ladder_len`.
    pub reset_ladder_bps: [u16; 5],
    /// Half-spread per market id, e6 (PENDING numbers).
    pub spread_e6: [u16; 128],
    pub bump: u8,
    pub reserved: [u8; 31],
}

impl RuleSet {
    /// 2+2+1+1 + 16 + 32 + 32 + 80 + 80 + 10 + 256 + 1 + 31 bytes after the discriminator.
    pub const BODY_LEN: usize = 2 + 2 + 1 + 1 + 16 + 32 + 32 + 80 + 80 + 10 + 256 + 1 + 31;
    /// Total account size (DESIGN 5.4).
    pub const SIZE: usize = 8 + Self::BODY_LEN;
}

const _: () = assert!(RuleSet::SIZE == 552);

#[cfg(test)]
mod tests {
    use super::*;

    /// Offsets inside the serialized account (discriminator first).
    #[test]
    fn byte_layout_offsets() {
        let mut spread = [0u16; 128];
        spread[0] = 0x0101;
        spread[127] = 0xBEEF;
        let r = RuleSet {
            version: 0x0102,
            fee_bps: 0x0304,
            tier_count: 10,
            ladder_len: 5,
            leverage_x: [0x1111; 8],
            holding_fee_e6: [0x2222_2222; 8],
            commission_e6: [0x3333_3333; 8],
            tier_size: [0x4444_4444_4444_4444; 10],
            tier_cost: [0x5555_5555_5555_5555; 10],
            reset_ladder_bps: [0x6666; 5],
            spread_e6: spread,
            bump: 0xAB,
            reserved: [0; 31],
        };
        let mut d = Vec::new();
        r.try_serialize(&mut d).unwrap();
        assert_eq!(d.len(), 552);
        assert_eq!(&d[8..10], &0x0102u16.to_le_bytes()); // version
        assert_eq!(&d[10..12], &0x0304u16.to_le_bytes()); // fee_bps
        assert_eq!(d[12], 10); // tier_count
        assert_eq!(d[13], 5); // ladder_len
        assert_eq!(&d[14..16], &0x1111u16.to_le_bytes()); // leverage_x[0]
        assert_eq!(&d[30..34], &0x2222_2222u32.to_le_bytes()); // holding_fee_e6[0]
        assert_eq!(&d[62..66], &0x3333_3333u32.to_le_bytes()); // commission_e6[0]
        assert_eq!(&d[94..102], &0x4444_4444_4444_4444u64.to_le_bytes()); // tier_size[0]
        assert_eq!(&d[174..182], &0x5555_5555_5555_5555u64.to_le_bytes()); // tier_cost[0]
        assert_eq!(&d[254..256], &0x6666u16.to_le_bytes()); // reset_ladder_bps[0]
        assert_eq!(&d[264..266], &0x0101u16.to_le_bytes()); // spread_e6[0]
        assert_eq!(&d[518..520], &0xBEEFu16.to_le_bytes()); // spread_e6[127]
        assert_eq!(d[520], 0xAB); // bump
        assert_eq!(&d[521..552], &[0u8; 31]); // reserved
    }
}
