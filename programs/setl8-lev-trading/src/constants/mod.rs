pub mod admin;
pub use admin::*;

/// PDA seed of `SectorConfig` (singleton).
pub const CONFIG_SEED: &[u8] = b"config";
/// PDA seed of the payout tally. Must equal `setl8_shared_interfaces::payout_tally::PAYOUT_TALLY_SEED`.
pub const PAYOUT_TALLY_SEED: &[u8] = b"payout_tally";
/// PDA seed of `sector_authority`, the signer of every vault CPI.
pub const SECTOR_AUTHORITY_SEED: &[u8] = b"setl8_sector_authority";

/// PDA seeds of `RuleSet` (`["ruleset", version u16 LE]`), `Market` (`["market", id u16 LE]`) and
/// `WalletProfile` (`["wallet", trader]`).
pub const RULESET_SEED: &[u8] = b"ruleset";
pub const MARKET_SEED: &[u8] = b"market";
pub const WALLET_SEED: &[u8] = b"wallet";
/// `TradingAccount`: `["acct", trader, challenge_id u64 LE]`.
pub const ACCT_SEED: &[u8] = b"acct";

/// At most 5 simultaneously Active accounts per wallet (decision 28).
pub const MAX_ACTIVE_ACCOUNTS: u8 = 5;

/// `TradingAccount.status`.
pub const STATUS_ACTIVE: u8 = 0;
pub const STATUS_FAILED: u8 = 1;
pub const STATUS_ABANDONED: u8 = 2;
pub const STATUS_GRADUATED: u8 = 3;
/// An event may be at most this many seconds ahead of the block time (DESIGN I-3).
pub const EVENT_FUTURE_SKEW_SECS: i64 = 2;

/// `TradingAccount.pass_pending` (DESIGN 10.3).
pub const PASS_NONE: u8 = 0;
pub const PASS_REQUESTED: u8 = 1;
pub const PASS_CONFIRMED: u8 = 2;
/// Not a pending-pass state: the marker `fail_for_violation` (and nothing else) leaves on the Failed account it ends, so
/// `reset_account` can refuse it for good (decision 60). Every reader that tests `pass_pending` only knows 0, 1 and 2.
pub const PASS_VIOLATION: u8 = 3;

/// `cancel_pass` is allowed this many seconds after the request (the evaluator never answered), DESIGN 6.2.
pub const CANCEL_PASS_AFTER_SECS: i64 = 3_600;

/// Seconds a Failed, Abandoned or Graduated account stays before `close_account` (7 days, DESIGN 6.2).
pub const TERMINAL_RETENTION_SECS: i64 = 604_800;
/// `touch_activity` and `release_account` skip the vault call if the last push is younger than this (DESIGN 6.3).
pub const ACTIVITY_THROTTLE_SECS: i64 = 86_000;
/// Seed and status byte offset of the vault's `TraderState` (vault `constants` and account layout).
pub const VAULT_TRADER_STATE_SEED: &[u8] = b"trader_state";
pub const VAULT_STATUS_OFFSET: usize = 96;
/// Vault `TraderStatus::Abandoned` as stored.
pub const VAULT_STATUS_ABANDONED: u8 = 3;

/// `reject_pass` reasons (DESIGN 6.3).
pub const REJECT_NOT_FLAT: u8 = 0;
pub const REJECT_JOURNAL_BEHIND: u8 = 1;
pub const REJECT_TARGET_NOT_REACHED: u8 = 2;
pub const REJECT_OTHER: u8 = 3;

/// Maximum number of markets (DESIGN 6.1, `create_market`).
pub const MAX_MARKETS: u16 = 128;

/// Pass delay bounds in seconds: 1 day to 14 days (DESIGN 6.1 `set_limits`).
pub const MIN_PASS_DELAY_SECS: u32 = 86_400;
pub const MAX_PASS_DELAY_SECS: u32 = 1_209_600;
/// Defaults: 2 days, and a 24 h payout window (DESIGN 5.3).
pub const DEFAULT_PASS_DELAY_SECS: u32 = 172_800;
pub const DEFAULT_WINDOW_SECS: u32 = 86_400;

/// Rule set limits (DESIGN 5.4): up to 10 tiers, 5 reset-ladder steps, 8 market classes, fee at most 10%.
pub const MAX_TIERS: usize = 10;
pub const MAX_LADDER: usize = 5;
pub const CLASS_COUNT: usize = 8;
pub const MAX_FEE_BPS: u16 = 1000;

/// Market limits (DESIGN 3.2, 3.4): 6 weekly session windows, minutes in a week, "no quote conversion" marker.
pub const MAX_SESSIONS: usize = 6;
pub const MINUTES_PER_WEEK: u16 = 10_080;
pub const NO_QUOTE_CONV: u16 = 0xFFFF;
pub const MARKET_STATUS_REDUCE_ONLY: u8 = 2;

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
