use anchor_lang::prelude::*;

/// One weekly session window in minutes of the week (UTC), `start < end <= 10080`. `(0, 0)` = unused slot.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Session {
    pub start_min: u16,
    pub end_min: u16,
}

/// DESIGN 3.2. PDA `["market", market_id u16 LE]`; 128 bytes including the discriminator.
/// Field order is fixed by the M1b prompt (DESIGN gave the fields, not the order).
#[account]
pub struct Market {
    pub market_id: u16,
    /// Market class 0..=7 (index into the rule set's per-class arrays). Fixed at creation.
    pub class: u8,
    /// 0 disabled, 1 enabled, 2 reduce-only.
    pub status: u8,
    pub feed_id: [u8; 32],
    pub max_age_secs: u16,
    /// Quantity step and minimum, 1e-6 base units.
    pub qty_step: u64,
    pub qty_min: u64,
    /// Per-position notional cap, micro-USD.
    pub max_notional_usd: u64,
    pub session_count: u8,
    pub sessions: [Session; 6],
    /// Market whose price converts this market's quote currency to USD; 0xFFFF = none.
    pub quote_conv_market: u16,
    pub bump: u8,
    pub reserved: [u8; 30],
}

impl Market {
    /// 2+1+1+32+2+8+8+8+1+24+2+1+30 bytes after the discriminator.
    pub const BODY_LEN: usize = 2 + 1 + 1 + 32 + 2 + 8 + 8 + 8 + 1 + 24 + 2 + 1 + 30;
    pub const SIZE: usize = 8 + Self::BODY_LEN;
}

const _: () = assert!(Market::SIZE == 128);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_layout_offsets() {
        let m = Market {
            market_id: 0x0102,
            class: 7,
            status: 2,
            feed_id: [0xEE; 32],
            max_age_secs: 0x0304,
            qty_step: 0x1111_1111_1111_1111,
            qty_min: 0x2222_2222_2222_2222,
            max_notional_usd: 0x3333_3333_3333_3333,
            session_count: 6,
            sessions: [Session { start_min: 0x0A0B, end_min: 0x0C0D }; 6],
            quote_conv_market: 0xFFFF,
            bump: 0xAB,
            reserved: [0; 30],
        };
        let mut d = Vec::new();
        m.try_serialize(&mut d).unwrap();
        assert_eq!(d.len(), 128);
        assert_eq!(&d[8..10], &0x0102u16.to_le_bytes()); // market_id
        assert_eq!(d[10], 7); // class
        assert_eq!(d[11], 2); // status
        assert_eq!(&d[12..44], &[0xEE; 32]); // feed_id
        assert_eq!(&d[44..46], &0x0304u16.to_le_bytes()); // max_age_secs
        assert_eq!(&d[46..54], &0x1111_1111_1111_1111u64.to_le_bytes()); // qty_step
        assert_eq!(&d[54..62], &0x2222_2222_2222_2222u64.to_le_bytes()); // qty_min
        assert_eq!(&d[62..70], &0x3333_3333_3333_3333u64.to_le_bytes()); // max_notional_usd
        assert_eq!(d[70], 6); // session_count
        assert_eq!(&d[71..73], &0x0A0Bu16.to_le_bytes()); // sessions[0].start
        assert_eq!(&d[73..75], &0x0C0Du16.to_le_bytes()); // sessions[0].end
        assert_eq!(&d[91..93], &0x0A0Bu16.to_le_bytes()); // sessions[5].start
        assert_eq!(&d[93..95], &0x0C0Du16.to_le_bytes()); // sessions[5].end
        assert_eq!(&d[95..97], &0xFFFFu16.to_le_bytes()); // quote_conv_market
        assert_eq!(d[97], 0xAB); // bump
        assert_eq!(&d[98..128], &[0u8; 30]); // reserved
    }
}
