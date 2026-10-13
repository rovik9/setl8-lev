//! Phase-reset price (decision 51, DESIGN 9.5): `ceil(CURRENT tier cost x ladder[failed phase] / 10_000)`.
//!
//! Pure function, no accounts, no CPI: M8's `reset_account` will call it with the tier list it reads from the
//! vault's `ProductRegistry`. The rounding is NOT done here: it is `setl8_shared_interfaces::reset_price`, the one
//! function the vault itself calls, so the sector and the vault compute the identical number.
use anchor_lang::prelude::*;
use setl8_shared_interfaces::reset_price;

use crate::errors::Setl8Error;

/// The amount `deposit_reset` must be called with, in micro-USD.
///
/// - `tiers`: the `(size, cost)` pairs of the vault's `ProductRegistry.challenge_sizes` as they are NOW.
/// - `account_size`: the failed record's account size.
/// - `failed_phase_idx`: the 0-based phase the trader failed in (phase 1 = index 0).
/// - `ladder_bps`: the registered `reset_price_bps`, `[10000, 11500, 12000, 12750, 13000]` at launch.
///
/// Errors, checked in this order:
/// 1. `ResetTierInvalid` unless EXACTLY ONE tier has `size == account_size`. Stricter than the vault on purpose: the
///    vault accepts several tiers of one size as long as their costs are equal (and rejects different costs), this
///    function rejects any duplicate, because launch tier sizes are unique (decision 51) and a duplicate means the
///    registry was misconfigured.
/// 2. `ResetPhaseOutOfRange` if `failed_phase_idx >= ladder_bps.len()`.
/// 3. `ResetPriceOverflow` if the price does not fit in a `u64` (`reset_price` returned `None`).
///
/// A ladder entry of 0 gives a price of 0 and `Ok(0)`: that is the honest result of the formula, and whether a free
/// reset is acceptable is the vault's and the registered ladder's business, not this function's. A tier of cost 0
/// likewise gives 0.
pub fn reset_amount_micro(
    tiers: &[(u64, u64)],
    account_size: u64,
    failed_phase_idx: usize,
    ladder_bps: &[u16],
) -> Result<u64> {
    let mut same_size = tiers.iter().filter(|(size, _)| *size == account_size);
    let (_, tier_cost) = same_size.next().ok_or(Setl8Error::ResetTierInvalid)?;
    require!(same_size.next().is_none(), Setl8Error::ResetTierInvalid);
    let bps = ladder_bps.get(failed_phase_idx).ok_or(Setl8Error::ResetPhaseOutOfRange)?;
    reset_price(*tier_cost, *bps).ok_or_else(|| error!(Setl8Error::ResetPriceOverflow))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD: u64 = 1_000_000;
    const LADDER: [u16; 5] = [10_000, 11_500, 12_000, 12_750, 13_000];
    const SIZE_2_5K: u64 = 2_500 * USD;
    const SIZE_100K: u64 = 100_000 * USD;
    const COST_2_5K: u64 = 30 * USD;
    const COST_100K: u64 = 859 * USD;

    /// The Anchor error number of a failed call.
    fn code(r: Result<u64>) -> u32 {
        match r {
            Err(Error::AnchorError(e)) => e.error_code_number,
            other => panic!("expected an Anchor error, got {other:?}"),
        }
    }

    fn prices(tiers: &[(u64, u64)], size: u64) -> Vec<u64> {
        (0..LADDER.len()).map(|p| reset_amount_micro(tiers, size, p, &LADDER).unwrap()).collect()
    }

    #[test]
    fn the_100k_tier_gives_the_five_ladder_prices() {
        assert_eq!(
            prices(&[(SIZE_100K, COST_100K)], SIZE_100K),
            [859_000_000, 987_850_000, 1_030_800_000, 1_095_225_000, 1_116_700_000]
        );
    }

    #[test]
    fn non_proportional_tiers_in_one_table_each_use_their_own_cost() {
        // $30 for 2.5K and $859 for 100K: the price follows the tier cost, not the size
        let tiers = [(SIZE_2_5K, COST_2_5K), (SIZE_100K, COST_100K)];
        assert_eq!(prices(&tiers, SIZE_2_5K), [30_000_000, 34_500_000, 36_000_000, 38_250_000, 39_000_000]);
        assert_eq!(
            prices(&tiers, SIZE_100K),
            [859_000_000, 987_850_000, 1_030_800_000, 1_095_225_000, 1_116_700_000]
        );
        // order of the table does not matter
        let rev = [tiers[1], tiers[0]];
        assert_eq!(prices(&rev, SIZE_2_5K), prices(&tiers, SIZE_2_5K));
    }

    #[test]
    fn a_cost_that_is_not_a_multiple_rounds_up_in_every_phase() {
        // cost 30_000_001: x1 = 30_000_001; x1.15 = 34_500_001.15; x1.2 = 36_000_001.2; x1.275 = 38_250_001.275;
        // x1.3 = 39_000_001.3. Every fraction is positive, so each one rounds up by one unit.
        let cost = COST_2_5K + 1;
        assert_eq!(
            prices(&[(SIZE_2_5K, cost)], SIZE_2_5K),
            [30_000_001, 34_500_002, 36_000_002, 38_250_002, 39_000_002]
        );
        // cost 859_000_001: 987_850_001.15, 1_030_800_001.2, 1_095_225_001.275, 1_116_700_001.3
        assert_eq!(
            prices(&[(SIZE_100K, COST_100K + 1)], SIZE_100K),
            [859_000_001, 987_850_002, 1_030_800_002, 1_095_225_002, 1_116_700_002]
        );
        // cost 1: x1 = 1; x1.15 = 1.15 -> 2; x1.2 -> 2; x1.275 -> 2; x1.3 -> 2
        assert_eq!(prices(&[(SIZE_2_5K, 1)], SIZE_2_5K), [1, 2, 2, 2, 2]);
    }

    #[test]
    fn a_size_that_is_not_a_tier_is_refused() {
        let tiers = [(SIZE_2_5K, COST_2_5K), (SIZE_100K, COST_100K)];
        for size in [0, 1, SIZE_2_5K - 1, SIZE_2_5K + 1, 5_000 * USD, u64::MAX] {
            assert_eq!(code(reset_amount_micro(&tiers, size, 0, &LADDER)), 6104, "size {size}");
        }
        assert_eq!(code(reset_amount_micro(&[], SIZE_100K, 0, &LADDER)), 6104, "empty tier list");
    }

    #[test]
    fn two_tiers_of_the_same_size_are_refused_even_with_equal_costs() {
        // the vault would accept the equal-cost pair; the sector is stricter on purpose
        let equal = [(SIZE_100K, COST_100K), (SIZE_100K, COST_100K)];
        assert_eq!(code(reset_amount_micro(&equal, SIZE_100K, 0, &LADDER)), 6104);
        // different costs (the vault refuses these too)
        let different = [(SIZE_100K, COST_100K), (SIZE_2_5K, COST_2_5K), (SIZE_100K, COST_100K + 1)];
        assert_eq!(code(reset_amount_micro(&different, SIZE_100K, 0, &LADDER)), 6104);
        // a duplicate of ANOTHER size does not matter for this size
        assert_eq!(reset_amount_micro(&different, SIZE_2_5K, 0, &LADDER).unwrap(), COST_2_5K);
    }

    #[test]
    fn a_phase_beyond_the_ladder_is_refused() {
        let tiers = [(SIZE_100K, COST_100K)];
        for idx in [LADDER.len(), LADDER.len() + 1, 255, 256, usize::MAX] {
            assert_eq!(code(reset_amount_micro(&tiers, SIZE_100K, idx, &LADDER)), 6105, "idx {idx}");
        }
        // an empty ladder offers no reset at all
        assert_eq!(code(reset_amount_micro(&tiers, SIZE_100K, 0, &[])), 6105);
        // the last valid index still works
        assert_eq!(reset_amount_micro(&tiers, SIZE_100K, LADDER.len() - 1, &LADDER).unwrap(), 1_116_700_000);
    }

    #[test]
    fn a_price_that_does_not_fit_in_u64_is_the_overflow_error() {
        assert_eq!(code(reset_amount_micro(&[(SIZE_100K, u64::MAX)], SIZE_100K, 4, &LADDER)), 6106);
        assert_eq!(code(reset_amount_micro(&[(SIZE_100K, u64::MAX)], SIZE_100K, 1, &LADDER)), 6106);
        // 100% of u64::MAX still fits
        assert_eq!(reset_amount_micro(&[(SIZE_100K, u64::MAX)], SIZE_100K, 0, &LADDER).unwrap(), u64::MAX);
        // exact boundary at 130%: ceil(c x 1.3) = u64::MAX for c = 14_189_803_133_622_732_011, one more overflows
        let c = 14_189_803_133_622_732_011;
        assert_eq!(reset_amount_micro(&[(SIZE_100K, c)], SIZE_100K, 4, &LADDER).unwrap(), u64::MAX);
        assert_eq!(code(reset_amount_micro(&[(SIZE_100K, c + 1)], SIZE_100K, 4, &LADDER)), 6106);
    }

    #[test]
    fn bps_zero_and_cost_zero_give_a_price_of_zero() {
        // decision: the helper reports the formula's honest result (0), it does not invent a floor
        let tiers = [(SIZE_100K, COST_100K)];
        assert_eq!(reset_amount_micro(&tiers, SIZE_100K, 0, &[0, 10_000]).unwrap(), 0);
        assert_eq!(reset_amount_micro(&tiers, SIZE_100K, 1, &[0, 10_000]).unwrap(), COST_100K);
        assert_eq!(reset_amount_micro(&[(SIZE_100K, 0)], SIZE_100K, 3, &LADDER).unwrap(), 0);
    }

    #[test]
    fn errors_are_checked_in_the_documented_order() {
        // bad tier AND bad phase: the tier error wins; bad phase AND overflow cannot both happen (no bps to use)
        assert_eq!(code(reset_amount_micro(&[], SIZE_100K, 99, &LADDER)), 6104);
        let dup = [(SIZE_100K, COST_100K), (SIZE_100K, COST_100K)];
        assert_eq!(code(reset_amount_micro(&dup, SIZE_100K, 99, &LADDER)), 6104);
        assert_eq!(code(reset_amount_micro(&[(SIZE_100K, u64::MAX)], SIZE_100K, 99, &LADDER)), 6105);
    }

    #[test]
    fn amounts_equal_the_shared_helper_for_every_ladder_entry() {
        // a future change of setl8_shared_interfaces::reset_price (and so of the vault) shows up here
        let costs = [0, 1, 2, 3, 9_999, 10_000, 10_001, COST_2_5K, COST_2_5K + 1, COST_100K, COST_100K + 1, 1_599 * USD, u64::MAX / 2];
        for cost in costs {
            let tiers = [(SIZE_2_5K, 123), (SIZE_100K, cost)];
            for (idx, bps) in LADDER.iter().enumerate() {
                let want = setl8_shared_interfaces::reset_price(cost, *bps);
                match want {
                    Some(w) => assert_eq!(reset_amount_micro(&tiers, SIZE_100K, idx, &LADDER).unwrap(), w, "cost {cost} idx {idx}"),
                    None => assert_eq!(code(reset_amount_micro(&tiers, SIZE_100K, idx, &LADDER)), 6106, "cost {cost} idx {idx}"),
                }
            }
        }
    }
}
