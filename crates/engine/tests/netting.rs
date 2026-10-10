//! apply_fill (DESIGN 7.2), worked examples (7.5, 4.9), edge cases (7.6), boundaries, short twins, rounding rows.
use engine::*;

const EUR: u64 = 1_000_000; // one EUR in 1e-6 units
const T0: i64 = 1_760_000_000;

fn pos(qty: i64, avg: u64, timer: i64) -> Position {
    Position { qty, avg_entry_e12: avg, timer_start: timer, opened_at: timer - 1, fee_day: 7 }
}

fn fill(p: Option<Position>, side: Side, dq: u64, price: u64, t: i64) -> FillOutcome {
    apply_fill(p, side, dq, price, t, 99).expect("fill succeeds")
}

fn long_eur(n_eur: u64, avg: u64, timer: i64) -> Position {
    pos(i64::try_from(n_eur * EUR).unwrap(), avg, timer)
}

fn short_eur(n_eur: u64, avg: u64, timer: i64) -> Position {
    pos(-i64::try_from(n_eur * EUR).unwrap(), avg, timer)
}

// ---------------------------------------------------------------- 1. open

#[test]
fn open_long_from_flat() {
    let o = fill(None, Side::Buy, 5 * EUR, 1_080_032_400_000, T0);
    assert_eq!(
        o.position,
        Some(Position { qty: 5_000_000, avg_entry_e12: 1_080_032_400_000, timer_start: T0, opened_at: T0, fee_day: 99 })
    );
    assert_eq!(o.opened, Some(OpenSlice { qty: 5_000_000, price_e12: 1_080_032_400_000 }));
    assert_eq!(o.closed, None);
    assert_eq!((o.balance_delta, o.quick_tally_delta), (0, 0));
}

#[test]
fn open_short_from_flat_mirror() {
    let o = fill(None, Side::Sell, 5 * EUR, 1_080_032_400_000, T0);
    assert_eq!(
        o.position,
        Some(Position { qty: -5_000_000, avg_entry_e12: 1_080_032_400_000, timer_start: T0, opened_at: T0, fee_day: 99 })
    );
    assert_eq!(o.opened, Some(OpenSlice { qty: -5_000_000, price_e12: 1_080_032_400_000 }));
    assert_eq!(o.closed, None);
    assert_eq!((o.balance_delta, o.quick_tally_delta), (0, 0));
}

// ---------------------------------------------------------------- 2. merge

#[test]
fn worked_merge_long_average_rounds_up() {
    // DESIGN 7.5: long 3 @ 1.10 then buy 4 @ 1.13: 7.82e24 / 7e12 = 1_117_142_857_142.857 -> up
    let p = long_eur(3, 1_100_000_000_000, T0);
    let o = fill(Some(p), Side::Buy, 4 * EUR, 1_130_000_000_000, T0 + 10);
    let n = o.position.unwrap();
    assert_eq!(n.avg_entry_e12, 1_117_142_857_143);
    assert_eq!(n.qty, 7 * i64::try_from(EUR).unwrap());
    assert_eq!(n.timer_start, T0 + 10, "timer restarts on every add");
    assert_eq!(n.opened_at, p.opened_at, "opened_at is kept");
    assert_eq!(n.fee_day, p.fee_day, "fee_day is kept");
    assert_eq!(o.opened, Some(OpenSlice { qty: 4_000_000, price_e12: 1_130_000_000_000 }));
    assert_eq!(o.closed, None);
    assert_eq!((o.balance_delta, o.quick_tally_delta), (0, 0));
}

#[test]
fn worked_merge_short_average_rounds_down() {
    let p = short_eur(3, 1_100_000_000_000, T0);
    let o = fill(Some(p), Side::Sell, 4 * EUR, 1_130_000_000_000, T0 + 10);
    let n = o.position.unwrap();
    assert_eq!(n.avg_entry_e12, 1_117_142_857_142);
    assert_eq!(n.qty, -7 * i64::try_from(EUR).unwrap());
    assert_eq!(n.timer_start, T0 + 10);
    assert_eq!(o.opened, Some(OpenSlice { qty: -4_000_000, price_e12: 1_130_000_000_000 }));
}

#[test]
fn merge_with_an_exact_average_does_not_round() {
    for (side, q) in [(Side::Buy, 2_000_000i64), (Side::Sell, -2_000_000)] {
        let p = pos(q, 1_000_000_000_000, T0);
        let o = fill(Some(p), side, 2_000_000, 1_200_000_000_000, T0 + 1);
        assert_eq!(o.position.unwrap().avg_entry_e12, 1_100_000_000_000);
    }
}

#[test]
fn merge_at_the_same_second_is_allowed_and_restarts_the_timer() {
    let p = pos(1_000_000, 1_000_000_000_000, T0);
    let o = fill(Some(p), Side::Buy, 1, 1_000_000_000_000, T0);
    assert_eq!(o.position.unwrap().timer_start, T0);
}

// ---------------------------------------------------------------- 3. opposite: reduce, flatten, flip

#[test]
fn worked_partial_reduce_long() {
    // DESIGN 7.5: long 700,000 @ 1.117142857143, sell 300,000 at bid 1.12
    let start = pos(700_000_000_000, 1_117_142_857_143, T0);
    let o = fill(Some(start), Side::Sell, 300_000_000_000, 1_120_000_000_000, T0 + 100);
    let c = o.closed.unwrap();
    assert_eq!(c.pnl, 857_142_857); // $857.142857
    assert_eq!((c.qty_abs, c.entry_e12, c.exit_e12), (300_000_000_000, 1_117_142_857_143, 1_120_000_000_000));
    assert!(!c.quick);
    assert_eq!(c.credit, 857_142_857);
    assert_eq!(o.balance_delta, 857_142_857);
    assert_eq!(o.quick_tally_delta, 0);
    let n = o.position.unwrap();
    assert_eq!(n.qty, 400_000_000_000);
    assert_eq!(n.avg_entry_e12, 1_117_142_857_143, "average unchanged");
    assert_eq!(n.timer_start, T0, "a partial close does not reset the timer");
    assert_eq!((n.opened_at, n.fee_day), (start.opened_at, start.fee_day));
    assert_eq!(o.opened, None);
}

#[test]
fn partial_reduce_short_twin() {
    let start = pos(-700_000_000_000, 1_117_142_857_143, T0);
    // short profits when the price falls: buy back 300,000 at 1.11 -> 300_000e6 x 0.007142857143 / ...
    let o = fill(Some(start), Side::Buy, 300_000_000_000, 1_110_000_000_000, T0 + 100);
    let c = o.closed.unwrap();
    // 3e11 x (1_117_142_857_143 - 1_110_000_000_000) / 1e12 = 3e11 x 7_142_857_143 / 1e12 = 2_142_857_142.9
    assert_eq!(c.pnl, 2_142_857_142);
    assert_eq!(o.position.unwrap().qty, -400_000_000_000);
    assert_eq!(o.position.unwrap().timer_start, T0);
    assert_eq!(o.position.unwrap().avg_entry_e12, 1_117_142_857_143);
}

#[test]
fn exact_flatten_clears_the_position() {
    // N-2
    for (q, side) in [(2_000_000i64, Side::Sell), (-2_000_000, Side::Buy)] {
        let o = fill(Some(pos(q, 1_000_000_000_000, T0)), side, 2_000_000, 1_000_000_000_000, T0 + 100);
        assert_eq!(o.position, None);
        assert_eq!(o.opened, None);
        let c = o.closed.unwrap();
        assert_eq!((c.qty_abs, c.pnl, c.credit), (2_000_000, 0, 0));
        assert_eq!(o.balance_delta, 0);
    }
}

#[test]
fn worked_flip_example() {
    // DESIGN 7.5: long 1,000,000 @ 1.0800324, sell 1,500,000 at bid 1.0820000
    let start = long_eur(1_000_000, 1_080_032_400_000, T0);
    let o = apply_fill(Some(start), Side::Sell, 1_500_000 * EUR, 1_082_000_000_000, T0 + 120, 123).unwrap();
    let c = o.closed.unwrap();
    assert_eq!(c.pnl, 1_967_600_000); // +$1,967.60
    assert_eq!(c.qty_abs, 1_000_000 * EUR, "the old position closes in full");
    assert!(!c.quick);
    assert_eq!(c.credit, 1_967_600_000);
    assert_eq!(o.quick_tally_delta, 0);
    let n = o.position.unwrap();
    assert_eq!(n.qty, -500_000 * i64::try_from(EUR).unwrap(), "remainder short 500,000");
    assert_eq!(n.avg_entry_e12, 1_082_000_000_000, "remainder opens at the fill price, not the old average");
    assert_eq!((n.timer_start, n.opened_at, n.fee_day), (T0 + 120, T0 + 120, 123), "fresh timer, opened_at, fee_day");
    assert_eq!(o.opened, Some(OpenSlice { qty: -500_000 * i64::try_from(EUR).unwrap(), price_e12: 1_082_000_000_000 }));
    // margin on the remainder: 500,000 x 1.082 / 50 = $10,820
    assert_eq!(margin_micro(n.qty.unsigned_abs(), 1_082_000_000_000, 50), Ok(10_820_000_000));
}

#[test]
fn flip_short_to_long_twin() {
    // short 1,000,000 @ 1.0820000, buy 1,500,000 at 1.0800324: short profit 1_967_600_000, remainder long 500,000
    let start = short_eur(1_000_000, 1_082_000_000_000, T0);
    let o = apply_fill(Some(start), Side::Buy, 1_500_000 * EUR, 1_080_032_400_000, T0 + 120, 123).unwrap();
    let c = o.closed.unwrap();
    assert_eq!(c.pnl, 1_967_600_000);
    assert_eq!(c.qty_abs, 1_000_000 * EUR);
    let n = o.position.unwrap();
    assert_eq!(n.qty, 500_000 * i64::try_from(EUR).unwrap());
    assert_eq!(n.avg_entry_e12, 1_080_032_400_000);
    assert_eq!((n.timer_start, n.opened_at, n.fee_day), (T0 + 120, T0 + 120, 123));
    assert_eq!(o.opened, Some(OpenSlice { qty: 500_000 * i64::try_from(EUR).unwrap(), price_e12: 1_080_032_400_000 }));
}

#[test]
fn flip_remainder_does_not_keep_the_old_average() {
    for (q, side, entry, px) in [(1_000_000i64, Side::Sell, 1_000_000_000_000u64, 2_000_000_000_000u64), (-1_000_000, Side::Buy, 2_000_000_000_000, 1_000_000_000_000)] {
        let o = fill(Some(pos(q, entry, T0)), side, 1_000_001, px, T0 + 1000);
        assert_eq!(o.position.unwrap().avg_entry_e12, px);
        assert_eq!(o.position.unwrap().qty.unsigned_abs(), 1);
    }
}

#[test]
fn worked_quick_flip_credit() {
    // DESIGN 7.5: the same flip 30 s after the long opened: credit = floor(1967.60 / 2) = 983.80, tally += 983.80
    let start = long_eur(1_000_000, 1_080_032_400_000, T0);
    let o = fill(Some(start), Side::Sell, 1_500_000 * EUR, 1_082_000_000_000, T0 + 30);
    let c = o.closed.unwrap();
    assert!(c.quick);
    assert_eq!(c.pnl, 1_967_600_000);
    assert_eq!(c.credit, 983_800_000);
    assert_eq!(o.balance_delta, 983_800_000);
    assert_eq!(o.quick_tally_delta, 983_800_000);
}

#[test]
fn quick_flip_short_twin() {
    let start = short_eur(1_000_000, 1_082_000_000_000, T0);
    let o = fill(Some(start), Side::Buy, 1_500_000 * EUR, 1_080_032_400_000, T0 + 30);
    let c = o.closed.unwrap();
    assert!(c.quick);
    assert_eq!((c.pnl, c.credit), (1_967_600_000, 983_800_000));
    assert_eq!((o.balance_delta, o.quick_tally_delta), (983_800_000, 983_800_000));
}

// ---------------------------------------------------------------- quick rule

#[test]
fn quick_is_45_seconds_inclusive() {
    for (q, side, entry, exit) in [(1_000_000i64, Side::Sell, 1_000_000_000_000u64, 1_001_000_000_000u64), (-1_000_000, Side::Buy, 1_001_000_000_000, 1_000_000_000_000)] {
        let at45 = fill(Some(pos(q, entry, T0)), side, 1_000_000, exit, T0 + 45);
        assert!(at45.closed.unwrap().quick, "45 s is quick");
        assert_eq!(at45.closed.unwrap().pnl, 1_000);
        assert_eq!(at45.closed.unwrap().credit, 500);
        assert_eq!(at45.quick_tally_delta, 500);
        let at46 = fill(Some(pos(q, entry, T0)), side, 1_000_000, exit, T0 + 46);
        assert!(!at46.closed.unwrap().quick, "46 s is not");
        assert_eq!(at46.closed.unwrap().credit, 1_000);
        assert_eq!(at46.quick_tally_delta, 0);
        let at0 = fill(Some(pos(q, entry, T0)), side, 1_000_000, exit, T0);
        assert!(at0.closed.unwrap().quick, "0 s is quick");
    }
}

#[test]
fn quick_losses_are_credited_in_full_and_never_touch_the_tally() {
    for (q, side, entry, exit) in [(1_000_000i64, Side::Sell, 1_001_000_000_000u64, 1_000_000_000_000u64), (-1_000_000, Side::Buy, 1_000_000_000_000, 1_001_000_000_000)] {
        let o = fill(Some(pos(q, entry, T0)), side, 1_000_000, exit, T0 + 5);
        let c = o.closed.unwrap();
        assert!(c.quick);
        assert_eq!((c.pnl, c.credit), (-1_000, -1_000));
        assert_eq!((o.balance_delta, o.quick_tally_delta), (-1_000, 0));
    }
}

#[test]
fn a_quick_slice_with_zero_profit_credits_nothing() {
    let o = fill(Some(pos(1_000_000, 1_000_000_000_000, T0)), Side::Sell, 1_000_000, 1_000_000_000_000, T0 + 5);
    assert_eq!((o.balance_delta, o.quick_tally_delta), (0, 0));
}

#[test]
fn quick_profit_credit_floors_an_odd_amount() {
    // pnl = 3 micro-USD (3 units x 1e12 / 1e12): quick credit floor(3/2) = 1 (ceil would give 2)
    for (q, side, entry, exit) in [(3i64, Side::Sell, 1_000_000_000_000u64, 2_000_000_000_000u64), (-3, Side::Buy, 2_000_000_000_000, 1_000_000_000_000)] {
        let o = fill(Some(pos(q, entry, T0)), side, 3, exit, T0 + 1);
        let c = o.closed.unwrap();
        assert_eq!((c.pnl, c.credit), (3, 1));
        assert_eq!(o.quick_tally_delta, 1);
    }
}

#[test]
fn n7_adding_restarts_the_timer_so_a_later_close_is_quick() {
    // add 44 s after open, close 20 s later: the slice is 20 s old (quick) although the position is 64 s old
    for (q, open_side, close_side) in [(1_000_000i64, Side::Buy, Side::Sell), (-1_000_000, Side::Sell, Side::Buy)] {
        let p = pos(q, 1_000_000_000_000, T0);
        let added = fill(Some(p), open_side, 1_000_000, 1_000_000_000_000, T0 + 44).position.unwrap();
        assert_eq!(added.timer_start, T0 + 44);
        let exit = if q > 0 { 1_001_000_000_000 } else { 999_000_000_000 };
        let closed = fill(Some(added), close_side, 2_000_000, exit, T0 + 64);
        assert!(closed.closed.unwrap().quick);
        // without the restart (an add at the very start) the same close 64 s after open is not quick
        assert!(!fill(Some(p), close_side, 1_000_000, exit, T0 + 64).closed.unwrap().quick);
    }
}

#[test]
fn n8_sequential_fills_at_one_sample_time() {
    // two fills with the same T apply one after the other
    let a = fill(None, Side::Buy, 1_000_000, 1_000_000_000_000, T0);
    let b = fill(a.position, Side::Buy, 1_000_000, 1_000_000_000_000, T0);
    assert_eq!(b.position.unwrap().qty, 2_000_000);
    let c = fill(b.position, Side::Sell, 2_000_000, 1_000_000_000_000, T0);
    assert_eq!(c.position, None);
}

#[test]
fn n9_a_fill_into_an_existing_position_nets_normally() {
    // a pending order that fills where a position exists: merge, reduce, or flip
    let p = pos(2_000_000, 1_000_000_000_000, T0);
    assert_eq!(fill(Some(p), Side::Buy, 1, 1_000_000_000_000, T0 + 1).position.unwrap().qty, 2_000_001);
    assert_eq!(fill(Some(p), Side::Sell, 1, 1_000_000_000_000, T0 + 1).position.unwrap().qty, 1_999_999);
    assert_eq!(fill(Some(p), Side::Sell, 2_000_001, 1_000_000_000_000, T0 + 1).position.unwrap().qty, -1);
    let s = pos(-2_000_000, 1_000_000_000_000, T0);
    assert_eq!(fill(Some(s), Side::Sell, 1, 1_000_000_000_000, T0 + 1).position.unwrap().qty, -2_000_001);
    assert_eq!(fill(Some(s), Side::Buy, 1, 1_000_000_000_000, T0 + 1).position.unwrap().qty, -1_999_999);
    assert_eq!(fill(Some(s), Side::Buy, 2_000_001, 1_000_000_000_000, T0 + 1).position.unwrap().qty, 1);
}

// N-5 (SL and TP satisfied by one gap sample), N-6 (q_tally > 0.028 S) and the trigger rule of N-10 belong to M4.

// ---------------------------------------------------------------- boundaries and errors

#[test]
fn zero_quantity_is_rejected_everywhere() {
    for p in [None, Some(pos(1_000_000, 1_000_000_000_000, T0)), Some(pos(-1_000_000, 1_000_000_000_000, T0))] {
        for side in [Side::Buy, Side::Sell] {
            assert_eq!(apply_fill(p, side, 0, 1_000_000_000_000, T0 + 1, 0), Err(EngineError::InvalidQuantity));
        }
    }
}

#[test]
fn a_single_unit_fill_works() {
    let o = fill(None, Side::Buy, 1, 1_000_000_000_000, T0);
    assert_eq!(o.position.unwrap().qty, 1);
    let c = fill(o.position, Side::Sell, 1, 3_000_000_000_000, T0 + 100);
    assert_eq!(c.position, None);
    assert_eq!(c.closed.unwrap().pnl, 2); // 1 x 2e12 / 1e12
}

#[test]
fn a_zero_price_or_an_oversized_fill_is_rejected() {
    assert_eq!(apply_fill(None, Side::Buy, 1, 0, T0, 0), Err(EngineError::InvalidInput));
    assert_eq!(apply_fill(None, Side::Sell, 1, 0, T0, 0), Err(EngineError::InvalidInput));
    let too_big = u64::try_from(i64::MAX).unwrap() + 1;
    assert_eq!(apply_fill(None, Side::Buy, too_big, 1, T0, 0), Err(EngineError::InvalidQuantity));
    assert_eq!(apply_fill(None, Side::Sell, too_big, 1, T0, 0), Err(EngineError::InvalidQuantity));
    // i64::MAX itself opens
    let o = apply_fill(None, Side::Sell, u64::try_from(i64::MAX).unwrap(), 1, T0, 0).unwrap();
    assert_eq!(o.position.unwrap().qty, -i64::MAX);
}

#[test]
fn a_zero_quantity_position_and_a_backwards_clock_are_invalid_input() {
    assert_eq!(apply_fill(Some(pos(0, 1, T0)), Side::Buy, 1, 1, T0, 0), Err(EngineError::InvalidInput));
    for q in [5i64, -5] {
        for side in [Side::Buy, Side::Sell] {
            assert_eq!(apply_fill(Some(pos(q, 1, T0)), side, 1, 1, T0 - 1, 0), Err(EngineError::InvalidInput));
        }
    }
    assert_eq!(apply_fill(Some(pos(5, 1, -1)), Side::Sell, 1, 1, i64::MAX, 0), Err(EngineError::Overflow));
}

#[test]
fn merging_past_i64_max_is_overflow_not_a_panic() {
    let p = pos(i64::MAX, u64::MAX, T0);
    assert_eq!(apply_fill(Some(p), Side::Buy, 1, u64::MAX, T0, 0), Err(EngineError::Overflow));
    // a short may not reach i64::MIN either: |qty| stays within i64::MAX so long and short are symmetric
    let s = pos(-i64::MAX, u64::MAX, T0);
    assert_eq!(apply_fill(Some(s), Side::Sell, 1, u64::MAX, T0, 0), Err(EngineError::Overflow));
    // the largest merge that still fits works, with the extreme price
    let p = pos(i64::MAX - 1, u64::MAX, T0);
    let o = apply_fill(Some(p), Side::Buy, 1, u64::MAX, T0, 0).unwrap();
    assert_eq!(o.position.unwrap().qty, i64::MAX);
    assert_eq!(o.position.unwrap().avg_entry_e12, u64::MAX);
}

#[test]
fn a_pnl_that_does_not_fit_i64_is_overflow() {
    // qty i64::MAX with prices 1 and u64::MAX: pnl ~ 1.7e26 micro-USD
    let p = pos(i64::MAX, 1, T0);
    assert_eq!(apply_fill(Some(p), Side::Sell, u64::try_from(i64::MAX).unwrap(), u64::MAX, T0 + 100, 0), Err(EngineError::Overflow));
    let s = pos(-i64::MAX, 1, T0);
    assert_eq!(apply_fill(Some(s), Side::Buy, u64::try_from(i64::MAX).unwrap(), u64::MAX, T0 + 100, 0), Err(EngineError::Overflow));
    assert_eq!(pnl_micro(true, u64::MAX, 0, u64::MAX), Err(EngineError::Overflow));
    assert_eq!(pnl_micro(false, u64::MAX, u64::MAX, 0), Err(EngineError::Overflow));
    // a huge loss that does not fit is Overflow too
    assert_eq!(pnl_micro(true, u64::MAX, u64::MAX, 0), Err(EngineError::Overflow));
    // pnl exactly at i64::MAX and i64::MIN still converts
    assert_eq!(pnl_micro(true, 1_000_000_000_000, 0, 9_223_372_036_854_775_807), Ok(i64::MAX));
    assert_eq!(pnl_micro(false, 1_000_000_000_000, 0, 9_223_372_036_854_775_808), Ok(i64::MIN));
    assert_eq!(pnl_micro(true, 1_000_000_000_000, 9_223_372_036_854_775_808, 0), Ok(i64::MIN));
}

// ---------------------------------------------------------------- pnl, unrealized

#[test]
fn pnl_floors_toward_minus_infinity_for_both_sides() {
    // 1 unit x +/-0.5e12 / 1e12 = +/-0.5 micro-USD
    assert_eq!(pnl_micro(true, 1, 1_000_000_000_000, 1_500_000_000_000), Ok(0)); // profit rounds down
    assert_eq!(pnl_micro(true, 1, 1_500_000_000_000, 1_000_000_000_000), Ok(-1)); // loss rounds to the larger loss
    assert_eq!(pnl_micro(false, 1, 1_500_000_000_000, 1_000_000_000_000), Ok(0)); // short profit down
    assert_eq!(pnl_micro(false, 1, 1_000_000_000_000, 1_500_000_000_000), Ok(-1)); // short loss up
    // exact amounts are not rounded
    assert_eq!(pnl_micro(true, 2, 1_000_000_000_000, 1_500_000_000_000), Ok(1));
    assert_eq!(pnl_micro(true, 2, 1_500_000_000_000, 1_000_000_000_000), Ok(-1));
    assert_eq!(pnl_micro(true, 5, 7, 7), Ok(0));
}

#[test]
fn design_4_9_unrealized_and_close_numbers() {
    let long = pos(1_000_000_000_000, 1_080_032_400_000, T0);
    // (bid mark, expected open P&L in micro-USD) for the rows of the 4.9 table
    for (bid, want) in [
        (1_079_967_600_000u64, -64_800_000i64),
        (1_080_567_582_000, 535_182_000),
        (1_081_267_561_000, 1_235_161_000),
        (1_083_067_507_000, 3_035_107_000),
        (1_081_067_567_000, 1_035_167_000),
        (1_077_967_660_000, -2_064_740_000),
        (1_059_968_200_000, -20_064_200_000),
    ] {
        assert_eq!(unrealized_micro(&long, bid, u64::MAX), Ok(want), "bid {bid}");
    }
    // the breach close at the sample bid: slice age 150 s -> not quick, B -= 2,064.74
    let o = fill(Some(long), Side::Sell, 1_000_000_000_000, 1_077_967_660_000, T0 + 150);
    let c = o.closed.unwrap();
    assert_eq!((c.pnl, c.quick, c.credit), (-2_064_740_000, false, -2_064_740_000));
    assert_eq!(o.position, None);
}

#[test]
fn unrealized_of_a_zero_quantity_position_is_invalid_input() {
    assert_eq!(unrealized_micro(&pos(0, 1_000_000_000_000, T0), 1, 2), Err(EngineError::InvalidInput));
}

#[test]
fn quick_tally_follows_the_credited_half_profit_only() {
    // pnl 1 micro-USD, quick: credit floor(1/2) = 0, tally 0; pnl 2: credit 1, tally 1; loss: tally 0
    for (pnl_units, want_credit, want_tally) in [(1i64, 0i64, 0i64), (2, 1, 1), (-1, -1, 0), (0, 0, 0)] {
        let exit = if pnl_units >= 0 { 1_000_000_000_000 + 1_000_000_000_000 * pnl_units.unsigned_abs() } else { 1_000_000_000_000 - 1_000_000_000_000 * pnl_units.unsigned_abs() / 2 };
        // 1 unit: pnl = 1 x (exit - entry) / 1e12 ; for the loss case use 2 units to get -1 exactly
        let (qty, dq) = if pnl_units < 0 { (2i64, 2u64) } else { (1, 1) };
        let exit = if pnl_units < 0 { 500_000_000_000 } else { exit };
        let o = fill(Some(pos(qty, 1_000_000_000_000, T0)), Side::Sell, dq, exit, T0 + 1);
        assert_eq!(o.closed.unwrap().pnl, pnl_units);
        assert_eq!((o.balance_delta, o.quick_tally_delta), (want_credit, want_tally), "pnl {pnl_units}");
    }
}

#[test]
fn unrealized_marks_longs_at_bid_and_shorts_at_ask() {
    let long = pos(1_000_000, 1_000_000_000_000, T0);
    let short = pos(-1_000_000, 1_000_000_000_000, T0);
    // bid 0.9, ask 1.1
    assert_eq!(unrealized_micro(&long, 900_000_000_000, 1_100_000_000_000), Ok(-100_000)); // uses the bid
    assert_eq!(unrealized_micro(&short, 900_000_000_000, 1_100_000_000_000), Ok(-100_000)); // uses the ask
    assert_eq!(unrealized_micro(&long, 1_100_000_000_000, 900_000_000_000), Ok(100_000));
    assert_eq!(unrealized_micro(&short, 1_100_000_000_000, 900_000_000_000), Ok(100_000));
    // floor on both sides
    assert_eq!(unrealized_micro(&pos(1, 1_500_000_000_000, T0), 1_000_000_000_000, 0), Ok(-1));
    assert_eq!(unrealized_micro(&pos(-1, 1_000_000_000_000, T0), 0, 1_500_000_000_000), Ok(-1));
    assert_eq!(unrealized_micro(&pos(1, 1_000_000_000_000, T0), 1_500_000_000_000, 0), Ok(0));
    assert_eq!(unrealized_micro(&pos(-1, 1_500_000_000_000, T0), 0, 1_000_000_000_000), Ok(0));
}

// ---------------------------------------------------------------- rounding rows (7.4): floor vs ceil differ by exactly 1

#[test]
fn rounding_row_quick_credit_is_floor() {
    let o = fill(Some(pos(1, 1_000_000_000_000, T0)), Side::Sell, 1, 4_000_000_000_000, T0 + 1);
    assert_eq!(o.closed.unwrap().pnl, 3);
    assert_eq!(o.balance_delta, 1);
}
