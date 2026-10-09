use anchor_lang::prelude::*;

/// DESIGN 5.3. Singleton PDA `["config"]`; 270 bytes including the 8-byte discriminator.
#[account]
pub struct SectorConfig {
    pub admin_a: Pubkey,
    pub admin_b: Pubkey,
    pub evaluator: Pubkey,
    pub vault_program: Pubkey,
    pub sl8_wallet: Pubkey,
    pub current_ruleset: u16,
    pub market_count: u16,
    pub paused: u8,
    pub bump: u8,
    /// Seconds between `confirm_pass` and `finalize_pass` (default 172,800).
    pub pass_delay_secs: u32,
    /// Daily payout cap in micro-USD.
    pub max_payout_per_window: u64,
    /// Payout window length in seconds (default 86,400).
    pub window_secs: u32,
    pub window_start: i64,
    pub window_total: u64,
    pub reserved: [u8; 64],
}

impl SectorConfig {
    /// Bytes after the discriminator: 5x32 + 2 + 2 + 1 + 1 + 4 + 8 + 4 + 8 + 8 + 64.
    pub const BODY_LEN: usize = 5 * 32 + 2 + 2 + 1 + 1 + 4 + 8 + 4 + 8 + 8 + 64;
    /// Total account size (DESIGN 5.3).
    pub const SIZE: usize = 8 + Self::BODY_LEN;
}

const _: () = assert!(SectorConfig::SIZE == 270);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_size_is_270_with_discriminator() {
        let c = SectorConfig {
            admin_a: Pubkey::default(),
            admin_b: Pubkey::default(),
            evaluator: Pubkey::default(),
            vault_program: Pubkey::default(),
            sl8_wallet: Pubkey::default(),
            current_ruleset: 0,
            market_count: 0,
            paused: 0,
            bump: 0,
            pass_delay_secs: 0,
            max_payout_per_window: 0,
            window_secs: 0,
            window_start: 0,
            window_total: 0,
            reserved: [0; 64],
        };
        let mut data = Vec::new();
        c.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), 270);
        assert_eq!(8 + SectorConfig::BODY_LEN, 270);
    }
}
