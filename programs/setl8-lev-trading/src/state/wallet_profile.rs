use anchor_lang::prelude::*;

/// DESIGN 5.2. PDA `["wallet", trader]`; 56 bytes including the discriminator.
/// In M1b it is created only by `ban_wallet`; `open_account` (M5) will create it for new traders.
#[account]
pub struct WalletProfile {
    pub trader: Pubkey,
    pub next_challenge_id: u64,
    pub active_count: u8,
    pub bump: u8,
    /// 1 blocks `open_account` and `reset_account` (`WalletBanned`).
    pub banned: u8,
    pub pad: [u8; 5],
}

impl WalletProfile {
    /// 32+8+1+1+1+5 bytes after the discriminator.
    pub const BODY_LEN: usize = 32 + 8 + 1 + 1 + 1 + 5;
    pub const SIZE: usize = 8 + Self::BODY_LEN;
}

const _: () = assert!(WalletProfile::SIZE == 56);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_layout_offsets() {
        let w = WalletProfile {
            trader: Pubkey::new_from_array([0xAA; 32]),
            next_challenge_id: 0x0102_0304_0506_0708,
            active_count: 3,
            bump: 0xAB,
            banned: 1,
            pad: [0; 5],
        };
        let mut d = Vec::new();
        w.try_serialize(&mut d).unwrap();
        assert_eq!(d.len(), 56);
        assert_eq!(&d[8..40], &[0xAA; 32]);
        assert_eq!(&d[40..48], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(d[48], 3); // active_count
        assert_eq!(d[49], 0xAB); // bump
        assert_eq!(d[50], 1); // banned
        assert_eq!(&d[51..56], &[0u8; 5]);
    }
}
