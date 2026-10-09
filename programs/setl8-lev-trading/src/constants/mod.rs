pub mod admin;
pub use admin::*;

/// PDA seed of `SectorConfig` (singleton).
pub const CONFIG_SEED: &[u8] = b"config";
/// PDA seed of the payout tally. Must equal `setl8_shared_interfaces::payout_tally::PAYOUT_TALLY_SEED`.
pub const PAYOUT_TALLY_SEED: &[u8] = b"payout_tally";
/// PDA seed of `sector_authority`, the signer of every vault CPI.
pub const SECTOR_AUTHORITY_SEED: &[u8] = b"setl8_sector_authority";

/// Maximum number of markets (DESIGN 6.1, `create_market`).
pub const MAX_MARKETS: u16 = 128;

/// Pass delay bounds in seconds: 1 day to 14 days (DESIGN 6.1 `set_limits`).
pub const MIN_PASS_DELAY_SECS: u32 = 86_400;
pub const MAX_PASS_DELAY_SECS: u32 = 1_209_600;
/// Defaults: 2 days, and a 24 h payout window (DESIGN 5.3).
pub const DEFAULT_PASS_DELAY_SECS: u32 = 172_800;
pub const DEFAULT_WINDOW_SECS: u32 = 86_400;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_design() {
        assert_eq!(PAYOUT_TALLY_SEED, setl8_shared_interfaces::payout_tally::PAYOUT_TALLY_SEED);
        assert_eq!(MIN_PASS_DELAY_SECS, 24 * 3600);
        assert_eq!(MAX_PASS_DELAY_SECS, 14 * 24 * 3600);
        assert!((MIN_PASS_DELAY_SECS..=MAX_PASS_DELAY_SECS).contains(&DEFAULT_PASS_DELAY_SECS));
        assert_eq!(DEFAULT_PASS_DELAY_SECS, 172_800);
        assert_eq!(DEFAULT_WINDOW_SECS, 86_400);
        assert_eq!(MAX_MARKETS, 128);
    }
}
