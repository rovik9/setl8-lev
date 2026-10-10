//! Rule constants (DESIGN 2). All fractions of account size S are in basis points
//! (1 bp = 0.01%), so every threshold is exact integer math on micro-USD sizes.
//! Sizes are multiples of 1,000,000 micro-USD ($1), so `S * bps / 10_000` is exact.

pub const BPS: i64 = 10_000;

/// Static floor F_s = 0.92 S (DESIGN 2.2).
pub const STATIC_FLOOR_BPS: i64 = 9_200;
/// Static floor after the latch = 0.96 S.
pub const LATCHED_FLOOR_BPS: i64 = 9_600;
/// Latch trigger: closed balance B >= 1.04 S at any time.
pub const LATCH_TRIGGER_BPS: i64 = 10_400;
/// Phase target: B >= 1.08 S.
pub const TARGET_BPS: i64 = 10_800;
/// Payout per pass in phases 2 to 5: 0.08 S.
pub const PAYOUT_BPS: i64 = 800;
/// Tight mode starts when open profit P > 0.01 S (strictly).
pub const TIGHT_START_BPS: i64 = 100;
/// Tight floor starts at min(F_s + 0.04 S, R) (decision 31).
pub const TIGHT_OFFSET_BPS: i64 = 400;
/// Ratchet slack: L = L_start + max(0, G_peak - 0.01 S).
pub const RATCHET_SLACK_BPS: i64 = 100;
/// Quick tally limit: phase fails when Q > 0.35 x 0.08 S = 0.028 S (A-RULE-2).
pub const QUICK_LIMIT_BPS: i64 = 280;

/// A slice closed within this many seconds of the position timer start is quick.
pub const QUICK_SECS: i64 = 45;
/// Lock after a tight-floor breach (blocks opening and adding only).
pub const LOCK_SECS: i64 = 12 * 60 * 60;

/// `size * bps / 10_000`, exact for sizes that are whole dollars.
/// (Plain arithmetic: sizes are at most $200,000 in micro-USD; M4 revisits these helpers.)
#[allow(clippy::arithmetic_side_effects)]
pub const fn frac(size_micro: i64, bps: i64) -> i64 {
    size_micro * bps / BPS
}

/// Current static floor level for an account (DESIGN 2.2).
pub const fn static_floor(size_micro: i64, latched: bool) -> i64 {
    frac(size_micro, if latched { LATCHED_FLOOR_BPS } else { STATIC_FLOOR_BPS })
}

#[cfg(test)]
mod tests {
    use super::*;

    const S100K: i64 = 100_000_000_000; // $100,000 in micro-USD

    #[test]
    fn levels_for_100k_match_design() {
        assert_eq!(static_floor(S100K, false), 92_000_000_000);
        assert_eq!(static_floor(S100K, true), 96_000_000_000);
        assert_eq!(frac(S100K, LATCH_TRIGGER_BPS), 104_000_000_000);
        assert_eq!(frac(S100K, TARGET_BPS), 108_000_000_000);
        assert_eq!(frac(S100K, PAYOUT_BPS), 8_000_000_000);
        assert_eq!(frac(S100K, TIGHT_START_BPS), 1_000_000_000);
        assert_eq!(frac(S100K, TIGHT_OFFSET_BPS), 4_000_000_000);
        assert_eq!(frac(S100K, QUICK_LIMIT_BPS), 2_800_000_000);
    }

    /// 0.35 x 0.08 S must equal the quick limit exactly (rule 12).
    #[test]
    fn quick_limit_is_35_percent_of_payout() {
        assert_eq!(PAYOUT_BPS * 35 / 100, QUICK_LIMIT_BPS);
    }

    /// The two time constants of DESIGN 2.4 and 2.3.
    #[test]
    fn time_constants_match_design() {
        assert_eq!(QUICK_SECS, 45);
        assert_eq!(LOCK_SECS, 43_200);
        assert_eq!(LOCK_SECS, 12 * 3600);
    }

    #[test]
    fn all_ten_sizes_are_exact() {
        for dollars in [2_500i64, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000] {
            let s = dollars * 1_000_000;
            for bps in [STATIC_FLOOR_BPS, LATCHED_FLOOR_BPS, LATCH_TRIGGER_BPS, TARGET_BPS, PAYOUT_BPS, TIGHT_START_BPS, TIGHT_OFFSET_BPS, QUICK_LIMIT_BPS] {
                assert_eq!((s * bps) % BPS, 0, "inexact: {dollars} x {bps}bps");
            }
        }
    }
}
