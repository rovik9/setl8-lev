//! Prices, margin, fees, day index, validation (DESIGN 4.9, 7.1, 7.3, 7.4, 7.6, 9.1 to 9.4).
use engine::*;

// ---------------------------------------------------------------- prices

#[test]
fn design_4_9_ask_notional_margin_and_spread_cost() {
    let ask = ask_from_mid(1_080_000_000_000, 30).unwrap();
    assert_eq!(ask, 1_080_032_400_000, "ask = ceil(1.08 x 1.00003) = 1.0800324");
    let qty = 1_000_000u64 * 1_000_000;
    assert_eq!(units::notional_micro(qty, ask), Ok(1_080_032_400_000), "notional $1,080,032.40");
    assert_eq!(margin_micro(qty, ask, 50), Ok(21_600_648_000), "margin $21,600.648 at 1:50");
    let bid = bid_from_mid(1_080_000_000_000, 30).unwrap();
    assert_eq!(bid, 1_079_967_600_000);
    let long = Position { qty: 1_000_000_000_000, avg_entry_e12: ask, timer_start: 0, opened_at: 0, fee_day: 0 };
    assert_eq!(unrealized_micro(&long, bid, ask), Ok(-64_800_000), "spread cost $64.80");
}

#[test]
fn design_4_9_bid_column() {
    for (mid, bid) in [
        (1_080_600_000_000u64, 1_080_567_582_000u64),
        (1_081_300_000_000, 1_081_267_561_000),
        (1_083_100_000_000, 1_083_067_507_000),
        (1_081_100_000_000, 1_081_067_567_000),
        (1_078_000_000_000, 1_077_967_660_000),
        (1_060_000_000_000, 1_059_968_200_000),
    ] {
        assert_eq!(bid_from_mid(mid, 30), Ok(bid), "mid {mid}");
    }
}

#[test]
fn ask_rounds_up_and_bid_rounds_down_by_exactly_one() {
    // mid 1, h = 1 e6: ask = ceil(1.000001) = 2 (floor would be 1); bid = floor(0.999999) = 0 (ceil would be 1)
    assert_eq!(ask_from_mid(1, 1), Ok(2));
    assert_eq!(bid_from_mid(1, 1), Ok(0));
    // an exact result is not rounded
    assert_eq!(ask_from_mid(1_000_000, 1), Ok(1_000_001));
    assert_eq!(bid_from_mid(1_000_000, 1), Ok(999_999));
    // zero half-spread is the mid
    assert_eq!(ask_from_mid(123_456_789, 0), Ok(123_456_789));
    assert_eq!(bid_from_mid(123_456_789, 0), Ok(123_456_789));
    // 3 x 1.000001 = 3.000003 -> 4 up; 3 x 0.999999 = 2.999997 -> 2 down
    assert_eq!(ask_from_mid(3, 1), Ok(4));
    assert_eq!(bid_from_mid(3, 1), Ok(2));
}

#[test]
fn bid_with_a_half_spread_of_exactly_100_percent_is_zero_and_above_is_invalid() {
    assert_eq!(bid_from_mid(1_000_000_000_000, 1_000_000), Ok(0));
    assert_eq!(bid_from_mid(1_000_000_000_000, 1_000_001), Err(EngineError::InvalidInput));
    assert_eq!(bid_from_mid(1_000_000_000_000, u32::MAX), Err(EngineError::InvalidInput));
}

#[test]
fn ask_overflow_is_an_error_not_a_panic() {
    assert_eq!(ask_from_mid(u64::MAX, 1), Err(EngineError::Overflow));
    assert_eq!(ask_from_mid(u64::MAX, 0), Ok(u64::MAX));
    assert!(ask_from_mid(u64::MAX, u32::MAX).is_err());
    assert_eq!(bid_from_mid(u64::MAX, 0), Ok(u64::MAX));
}

#[test]
fn oracle_value_floors_to_twelve_decimals() {
    assert_eq!(price_from_oracle(1_999_999), Ok(1));
    assert_eq!(price_from_oracle(2_000_000), Ok(2));
    assert_eq!(price_from_oracle(999_999), Ok(0));
    assert_eq!(price_from_oracle(67_250_123_456_789_012_345_678), Ok(67_250_123_456_789_012));
    let max = u128::from(u64::MAX) * 1_000_000;
    assert_eq!(price_from_oracle(max), Ok(u64::MAX));
    assert_eq!(price_from_oracle(max + 999_999), Ok(u64::MAX));
    assert_eq!(price_from_oracle(max + 1_000_000), Err(EngineError::Overflow));
    assert_eq!(price_from_oracle(u128::MAX), Err(EngineError::Overflow));
}

// ---------------------------------------------------------------- margin

#[test]
fn margin_rounds_up_by_exactly_one() {
    // N-3 helper: margin of a flip remainder, 500,000 EUR at 1.082, 1:50 -> $10,820
    assert_eq!(margin_micro(500_000_000_000, 1_082_000_000_000, 50), Ok(10_820_000_000));
    // ceil(1 x 1 / 1e12 / 1) = 1 (floor would be 0)
    assert_eq!(margin_micro(1, 1, 1), Ok(1));
    // exact: no rounding
    assert_eq!(margin_micro(1_000_000, 1_000_000_000_000, 1), Ok(1_000_000));
    // notional 5 / leverage 2 = 2.5 -> 3 (floor 2)
    assert_eq!(margin_micro(5, 1_000_000_000_000, 2), Ok(3));
    // notional 9 / 4 = 2.25 -> 3
    assert_eq!(margin_micro(9, 1_000_000_000_000, 4), Ok(3));
    // notional 0.7 / 2 = 0.35 -> 1 (floor 0)
    assert_eq!(margin_micro(1, 700_000_000_000, 2), Ok(1));
    // notional 1.000000000001 (a fraction of a micro-USD) / 1 -> 2
    assert_eq!(margin_micro(1, 1_000_000_000_001, 1), Ok(2));
    // exactly divisible by the leverage: 6 / 3 = 2
    assert_eq!(margin_micro(6, 1_000_000_000_000, 3), Ok(2));
    // zero quantity needs no margin
    assert_eq!(margin_micro(0, 1_000_000_000_000, 50), Ok(0));
}

#[test]
fn margin_scales_with_leverage_and_quantity() {
    let (q, p) = (1_000_000_000_000u64, 1_080_032_400_000u64); // 4.9: notional $1,080,032.40
    assert_eq!(margin_micro(q, p, 1), Ok(1_080_032_400_000));
    assert_eq!(margin_micro(q, p, 2), Ok(540_016_200_000));
    assert_eq!(margin_micro(q, p, 5), Ok(216_006_480_000));
    assert_eq!(margin_micro(q, p, 10), Ok(108_003_240_000));
    assert_eq!(margin_micro(q, p, 20), Ok(54_001_620_000));
    assert_eq!(margin_micro(2 * q, p, 50), Ok(43_201_296_000));
    assert_eq!(margin_micro(q, 2 * p, 50), Ok(43_201_296_000));
}

#[test]
fn margin_rejects_zero_leverage_and_overflows_cleanly() {
    assert_eq!(margin_micro(1, 1, 0), Err(EngineError::InvalidInput));
    // u64::MAX x u64::MAX / 1e12 does not fit i64
    assert_eq!(margin_micro(u64::MAX, u64::MAX, 1), Err(EngineError::Overflow));
    // the i64::MAX boundary
    assert_eq!(margin_micro(9_223_372_036_854_775_807, 1_000_000_000_000, 1), Ok(i64::MAX));
    assert_eq!(margin_micro(9_223_372_036_854_775_808, 1_000_000_000_000, 1), Err(EngineError::Overflow));
    // (2^64 - 2) / 2 = 2^63 - 1 fits; u64::MAX / 2 rounds up to 2^63 and does not
    assert_eq!(margin_micro(u64::MAX - 1, 1_000_000_000_000, 2), Ok(i64::MAX));
    assert_eq!(margin_micro(u64::MAX, 1_000_000_000_000, 2), Err(EngineError::Overflow));
}

// ---------------------------------------------------------------- fees

#[test]
fn fees_round_up() {
    // 1 micro-USD at 1 e6 = 1e-6 micro-USD -> 1 (floor would be 0)
    assert_eq!(fee_ceil_micro(1, 1), Ok(1));
    assert_eq!(fee_ceil_micro(1_000_000, 1), Ok(1));
    assert_eq!(fee_ceil_micro(1_000_001, 1), Ok(2));
    assert_eq!(fee_ceil_micro(0, 100), Ok(0));
    assert_eq!(fee_ceil_micro(1_000_000, 0), Ok(0));
    assert_eq!(fee_ceil_micro(-1, 1), Err(EngineError::InvalidInput));
    assert_eq!(fee_ceil_micro(i64::MIN, 1), Err(EngineError::InvalidInput));
    assert_eq!(fee_ceil_micro(i64::MAX, u32::MAX), Err(EngineError::Overflow));
    assert_eq!(fee_ceil_micro(i64::MAX, 1_000_000), Ok(i64::MAX));
}

#[test]
fn holding_fee_examples_from_the_design() {
    // FX major 0.01% = 100 e6 on $1,080,032.40 for one night = $108.00324
    assert_eq!(holding_fee_micro(1_000_000_000_000, 1_080_032_400_000, 100, 1), Ok(108_003_240));
    // three nights (a weekend) = three charges
    assert_eq!(holding_fee_micro(1_000_000_000_000, 1_080_032_400_000, 100, 3), Ok(324_009_720));
    assert_eq!(holding_fee_micro(1_000_000_000_000, 1_080_032_400_000, 100, 0), Ok(0));
}

#[test]
fn holding_fee_worst_case_per_night_table() {
    // (rate e6, leverage, expected $ per night) on a fully used $100K account: notional = 100,000 x leverage
    for (rate, lev, dollars) in [
        (100u32, 50u64, 500i64),
        (200, 20, 400),
        (400, 20, 800),
        (200, 20, 400),
        (400, 10, 400),
        (400, 5, 200),
        (1000, 5, 500),
        (1600, 2, 320),
    ] {
        let notional_micro = 100_000 * lev * 1_000_000;
        // 1 unit of price 1.0 USD: qty = notional in 1e-6 units
        assert_eq!(holding_fee_micro(notional_micro, 1_000_000_000_000, rate, 1), Ok(dollars * 1_000_000), "rate {rate} lev {lev}");
        assert_eq!(fee_ceil_micro(i64::try_from(notional_micro).unwrap(), rate), Ok(dollars * 1_000_000));
    }
}

#[test]
fn holding_fee_rounds_up_by_exactly_one() {
    // qty 1, price 1e12 -> notional 1 micro-USD; rate 1 e6 -> 1e-6 -> ceil 1 (floor 0)
    assert_eq!(holding_fee_micro(1, 1_000_000_000_000, 1, 1), Ok(1));
    // exactly one micro-USD: 1e6 notional at 1 e6
    assert_eq!(holding_fee_micro(1_000_000, 1_000_000_000_000, 1, 1), Ok(1));
    // one more unit of notional tips it to 2
    assert_eq!(holding_fee_micro(1_000_001, 1_000_000_000_000, 1, 1), Ok(2));
    // nights multiply BEFORE the ceiling: 2 nights of 0.5 micro-USD = exactly 1 (two ceilings would give 2)
    assert_eq!(holding_fee_micro(500_000, 1_000_000_000_000, 1, 2), Ok(1));
}

#[test]
fn holding_fee_overflow_is_an_error() {
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, 1, 1), Err(EngineError::Overflow));
    assert_eq!(holding_fee_micro(u64::MAX, 1_000_000_000_000, u32::MAX, u32::MAX), Err(EngineError::Overflow));
    // a zero rate or zero nights costs nothing even with extreme quantities
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, 0, 1), Ok(0));
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, 1, 0), Ok(0));
}

#[test]
fn swap_fee_table_from_design_9_2() {
    for (cost_dollars, fee_micro) in [
        (30u64, 120_000u64),
        (55, 220_000),
        (99, 396_000),
        (145, 580_000),
        (189, 756_000),
        (235, 940_000),
        (489, 1_956_000),
        (689, 2_756_000),
        (859, 3_436_000),
        (1_599, 6_396_000),
    ] {
        assert_eq!(swap_fee_micro(cost_dollars * 1_000_000, 40), Ok(fee_micro), "${cost_dollars}");
    }
    // ceil: 1 micro-USD at 1 bp -> 1 (floor 0); 10_000 at 1 bp -> exactly 1; 10_001 -> 2
    assert_eq!(swap_fee_micro(1, 1), Ok(1));
    assert_eq!(swap_fee_micro(10_000, 1), Ok(1));
    assert_eq!(swap_fee_micro(10_001, 1), Ok(2));
    assert_eq!(swap_fee_micro(0, 40), Ok(0));
    assert_eq!(swap_fee_micro(1_000_000, 0), Ok(0));
    assert_eq!(swap_fee_micro(u64::MAX, 10_000), Ok(u64::MAX));
    assert_eq!(swap_fee_micro(u64::MAX, 10_001), Err(EngineError::Overflow));
}

// ---------------------------------------------------------------- day index (22:00 UTC rollover)

const DAY: i64 = 86_400;

#[test]
fn day_index_changes_at_22_00_utc() {
    // 2024-01-01 00:00 UTC = 1_704_067_200
    let midnight = 1_704_067_200i64;
    let before = midnight + 22 * 3600 - 1; // 21:59:59
    let at = midnight + 22 * 3600; // 22:00:00
    assert_eq!(day_index(at), day_index(before) + 1);
    assert_eq!(day_index(at + DAY - 1), day_index(at));
    assert_eq!(day_index(at + DAY), day_index(at) + 1);
    assert_eq!(day_index(midnight), day_index(before), "midnight is still the previous rollover's day");
    assert_eq!(day_index(midnight + 3600), day_index(midnight));
}

#[test]
fn day_index_known_values_and_negative_times() {
    assert_eq!(day_index(22 * 3600), 0);
    assert_eq!(day_index(22 * 3600 - 1), -1);
    assert_eq!(day_index(0), -1);
    assert_eq!(day_index(-1), -1);
    assert_eq!(day_index(-2 * 3600), -1, "1969-12-31 22:00 is the rollover that starts index -1");
    assert_eq!(day_index(-2 * 3600 - 1), -2);
    assert_eq!(day_index(22 * 3600 + DAY), 1);
    assert_eq!(day_index(22 * 3600 - DAY), -1);
    assert_eq!(day_index(22 * 3600 - DAY - 1), -2);
    for t in [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1] {
        let _ = day_index(t); // no panic
    }
    assert!(day_index(i64::MAX) > 0);
    assert!(day_index(i64::MIN) < 0);
}

#[test]
fn nights_due_counts_rollovers_and_rejects_the_future() {
    assert_eq!(nights_due(10, 10), Ok(0));
    assert_eq!(nights_due(10, 11), Ok(1));
    assert_eq!(nights_due(10, 13), Ok(3));
    assert_eq!(nights_due(10, 9), Err(EngineError::InvalidInput));
    assert_eq!(nights_due(-5, 5), Ok(10));
    assert_eq!(nights_due(i64::MIN, i64::MAX), Err(EngineError::Overflow));
    assert_eq!(nights_due(0, i64::from(u32::MAX)), Ok(u32::MAX));
    assert_eq!(nights_due(0, i64::from(u32::MAX) + 1), Err(EngineError::Overflow));
}

#[test]
fn a_position_opened_after_the_rollover_is_charged_from_the_next_one() {
    let midnight = 1_704_067_200i64;
    let opened = midnight + 22 * 3600 + 60; // 22:01 after the rollover
    let fee_day = day_index(opened);
    assert_eq!(nights_due(fee_day, day_index(opened + 3600)), Ok(0));
    assert_eq!(nights_due(fee_day, day_index(opened + DAY - 120)), Ok(0)); // 21:59 next day
    assert_eq!(nights_due(fee_day, day_index(opened + DAY - 60)), Ok(1)); // 22:00 next day
    assert_eq!(nights_due(fee_day, day_index(opened + 3 * DAY)), Ok(3)); // a weekend is three charges
}

// ---------------------------------------------------------------- validation

#[test]
fn n1_quantity_rules() {
    assert_eq!(validate_quantity(0, 1_000, 1_000), Err(EngineError::InvalidQuantity));
    assert_eq!(validate_quantity(1_500, 1_000, 1_000), Err(EngineError::InvalidQuantity));
    assert_eq!(validate_quantity(500, 500, 1_000), Err(EngineError::InvalidQuantity), "multiple of the step but below the minimum");
    assert_eq!(validate_quantity(999, 1, 1_000), Err(EngineError::InvalidQuantity));
    assert_eq!(validate_quantity(1_000, 1_000, 1_000), Ok(()));
    assert_eq!(validate_quantity(5_000, 1_000, 1_000), Ok(()));
    assert_eq!(validate_quantity(1, 1, 0), Ok(()));
    assert_eq!(validate_quantity(0, 1, 0), Err(EngineError::InvalidQuantity), "zero is never valid");
    assert_eq!(validate_quantity(10, 0, 0), Err(EngineError::InvalidInput), "a zero step is a misconfigured market");
    assert_eq!(validate_quantity(u64::MAX, 1, 1), Ok(()));
}

#[test]
fn n4_stop_levels_long() {
    let (bid, ask) = (1_000_000_000_000u64, 1_000_100_000_000u64);
    // long: SL below or at the bid, TP above or at the bid
    assert_eq!(validate_stop_level(Side::Buy, StopKind::StopLoss, bid - 1, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Buy, StopKind::StopLoss, bid, bid, ask), Ok(()), "touching is valid (it triggers)");
    assert_eq!(validate_stop_level(Side::Buy, StopKind::StopLoss, bid + 1, bid, ask), Err(EngineError::InvalidStopLevel));
    assert_eq!(validate_stop_level(Side::Buy, StopKind::TakeProfit, bid + 1, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Buy, StopKind::TakeProfit, bid, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Buy, StopKind::TakeProfit, bid - 1, bid, ask), Err(EngineError::InvalidStopLevel));
}

#[test]
fn n4_stop_levels_short_twin() {
    let (bid, ask) = (1_000_000_000_000u64, 1_000_100_000_000u64);
    // short: SL above or at the ask, TP below or at the ask
    assert_eq!(validate_stop_level(Side::Sell, StopKind::StopLoss, ask + 1, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::StopLoss, ask, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::StopLoss, ask - 1, bid, ask), Err(EngineError::InvalidStopLevel));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::TakeProfit, ask - 1, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::TakeProfit, ask, bid, ask), Ok(()));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::TakeProfit, ask + 1, bid, ask), Err(EngineError::InvalidStopLevel));
}

#[test]
fn a_zero_stop_level_is_invalid() {
    for side in [Side::Buy, Side::Sell] {
        for kind in [StopKind::StopLoss, StopKind::TakeProfit] {
            assert_eq!(validate_stop_level(side, kind, 0, 1, 2), Err(EngineError::InvalidStopLevel));
        }
    }
}

// N-10: "a fill price that rounds to exactly the trigger counts as triggered" is the trigger rule (M4); here only the
// validator side is tested: a level equal to the current bid/ask is accepted (see the touching cases above).
