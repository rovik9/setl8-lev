//! Claim checks (DESIGN 4.4 I-5 to I-10, 7.1): every boundary violated by exactly one unit, with the exact-boundary pass.
use engine::*;

const S: i64 = 100_000_000_000; // $100,000
fn usd(d: i64) -> i64 {
    d * 1_000_000
}
fn fresh() -> AccountState {
    AccountState::new_phase(S)
}
fn tight() -> AccountState {
    // started at B = 100k with p_open 1.2k: l_start 96k, g_peak 1.2k, L 96.2k
    check_tight_start(&fresh(), usd(1_200), usd(100_000), usd(96_000), usd(1_200), usd(96_200)).unwrap()
}

// ---------------------------------------------------------------- executed prices (RESOLVE a)

#[test]
fn design_4_9_fill_price_is_the_ask_of_the_proof_mid() {
    // mid 1.08, h = 30 e6: ask 1.0800324
    assert_eq!(fill_price_e12(1_080_000_000_000, 30, true), Ok(1_080_032_400_000));
    // a short fill gets the bid
    assert_eq!(fill_price_e12(1_080_000_000_000, 30, false), Ok(1_079_967_600_000));
}

#[test]
fn closing_a_long_sells_at_the_bid_and_closing_a_short_buys_at_the_ask() {
    // 4.9 breach row: mid 1.078 -> bid 1.07796766
    assert_eq!(close_price_e12(1_078_000_000_000, 30, true), Ok(1_077_967_660_000));
    assert_eq!(close_price_e12(1_078_000_000_000, 30, false), Ok(1_078_032_340_000));
    // the two rules are mirror images: the close of a long equals the fill of a short and vice versa
    for mid in [1u64, 3, 1_000_000, 1_080_000_000_001, u64::MAX / 2] {
        for h in [0u32, 1, 30, 1_000] {
            assert_eq!(close_price_e12(mid, h, true), fill_price_e12(mid, h, false));
            assert_eq!(close_price_e12(mid, h, false), fill_price_e12(mid, h, true));
        }
    }
}

#[test]
fn executed_prices_round_against_the_trader() {
    // mid 1, h 1: ask = ceil(1.000001) = 2, bid = floor(0.999999) = 0
    assert_eq!(fill_price_e12(1, 1, true), Ok(2));
    assert_eq!(fill_price_e12(1, 1, false), Ok(0));
    assert_eq!(fill_price_e12(3, 0, true), Ok(3));
    assert_eq!(fill_price_e12(3, 0, false), Ok(3));
    assert_eq!(fill_price_e12(1, 1_000_001, false), Err(ClaimError::Engine(EngineError::InvalidInput)));
}

// ---------------------------------------------------------------- I-5 close

fn close(long: bool) -> CloseClaim {
    // 1,000 units at +/- 0.001 USD = 1_000 micro-USD profit at age 30 s (quick)
    let (entry, exit) = if long { (1_000_000_000_000, 1_001_000_000_000) } else { (1_001_000_000_000, 1_000_000_000_000) };
    CloseClaim { long, qty: 1_000_000, entry_e12: entry, exit_e12: exit, time: 1_000, timer_start: 970, pnl: 1_000, quick: true, credited: 500 }
}

#[test]
fn a_correct_close_is_accepted_for_both_sides() {
    for long in [true, false] {
        assert_eq!(check_close(&close(long)), Ok(CloseCredit { credit: 500, tally_delta: 500 }), "long {long}");
    }
}

#[test]
fn pnl_must_match_to_the_micro_usd() {
    for long in [true, false] {
        for delta in [-1i64, 1] {
            let c = CloseClaim { pnl: 1_000 + delta, ..close(long) };
            assert_eq!(check_close(&c), Err(ClaimError::PnlMismatch), "long {long} delta {delta}");
        }
    }
    // a long's pnl with the short sign convention is wrong
    assert_eq!(check_close(&CloseClaim { pnl: -1_000, ..close(true) }), Err(ClaimError::PnlMismatch));
    // floor toward minus infinity: 1 unit x -0.5 micro-USD = -1, not 0
    let tiny = CloseClaim { long: true, qty: 1, entry_e12: 1_500_000_000_000, exit_e12: 1_000_000_000_000, time: 100, timer_start: 0, pnl: -1, quick: false, credited: -1 };
    assert_eq!(check_close(&tiny), Ok(CloseCredit { credit: -1, tally_delta: 0 }));
    assert_eq!(check_close(&CloseClaim { pnl: 0, credited: 0, ..tiny }), Err(ClaimError::PnlMismatch));
}

#[test]
fn quick_is_45_seconds_inclusive() {
    for long in [true, false] {
        let at45 = CloseClaim { timer_start: 1_000 - 45, ..close(long) };
        assert_eq!(check_close(&at45), Ok(CloseCredit { credit: 500, tally_delta: 500 }));
        // claiming "not quick" at 45 s is wrong, and claiming "quick" at 46 s is wrong
        assert_eq!(check_close(&CloseClaim { quick: false, credited: 1_000, ..at45 }), Err(ClaimError::QuickMismatch));
        let at46 = CloseClaim { timer_start: 1_000 - 46, quick: false, credited: 1_000, ..close(long) };
        assert_eq!(check_close(&at46), Ok(CloseCredit { credit: 1_000, tally_delta: 0 }));
        assert_eq!(check_close(&CloseClaim { quick: true, credited: 500, ..at46 }), Err(ClaimError::QuickMismatch));
        // age zero is quick
        assert_eq!(check_close(&CloseClaim { timer_start: 1_000, ..close(long) }).map(|c| c.credit), Ok(500));
    }
}

#[test]
fn credited_is_half_the_profit_floored_when_quick_and_full_otherwise() {
    for long in [true, false] {
        // odd quick profit 3 -> credit 1 (ceil would be 2); entry/exit one micro-USD apart per 3 units
        let (entry, exit) = if long { (1_000_000_000_000, 2_000_000_000_000) } else { (2_000_000_000_000, 1_000_000_000_000) };
        let odd = CloseClaim { long, qty: 3, entry_e12: entry, exit_e12: exit, time: 10, timer_start: 5, pnl: 3, quick: true, credited: 1 };
        assert_eq!(check_close(&odd), Ok(CloseCredit { credit: 1, tally_delta: 1 }));
        assert_eq!(check_close(&CloseClaim { credited: 2, ..odd }), Err(ClaimError::CreditMismatch));
        assert_eq!(check_close(&CloseClaim { credited: 0, ..odd }), Err(ClaimError::CreditMismatch));
        assert_eq!(check_close(&CloseClaim { credited: 3, ..odd }), Err(ClaimError::CreditMismatch));
        // not quick: the full profit
        let slow = CloseClaim { timer_start: 0, time: 100, quick: false, credited: 3, ..odd };
        assert_eq!(check_close(&slow), Ok(CloseCredit { credit: 3, tally_delta: 0 }));
        assert_eq!(check_close(&CloseClaim { credited: 1, ..slow }), Err(ClaimError::CreditMismatch));
        // a quick LOSS is credited in full and never touches the tally
        let (entry, exit) = if long { (2_000_000_000_000, 1_000_000_000_000) } else { (1_000_000_000_000, 2_000_000_000_000) };
        let loss = CloseClaim { entry_e12: entry, exit_e12: exit, pnl: -3, credited: -3, ..odd };
        assert_eq!(check_close(&loss), Ok(CloseCredit { credit: -3, tally_delta: 0 }));
        assert_eq!(check_close(&CloseClaim { credited: -1, ..loss }), Err(ClaimError::CreditMismatch));
        assert_eq!(check_close(&CloseClaim { credited: -2, ..loss }), Err(ClaimError::CreditMismatch));
    }
}

#[test]
fn a_quick_slice_of_zero_profit_credits_nothing() {
    let z = CloseClaim { long: true, qty: 5, entry_e12: 1_000_000_000_000, exit_e12: 1_000_000_000_000, time: 10, timer_start: 9, pnl: 0, quick: true, credited: 0 };
    assert_eq!(check_close(&z), Ok(CloseCredit { credit: 0, tally_delta: 0 }));
    assert_eq!(check_close(&CloseClaim { credited: 1, ..z }), Err(ClaimError::CreditMismatch));
}

#[test]
fn degenerate_closes_are_refused() {
    let c = close(true);
    assert_eq!(check_close(&CloseClaim { qty: 0, ..c }), Err(ClaimError::ZeroQuantity));
    assert_eq!(check_close(&CloseClaim { entry_e12: 0, ..c }), Err(ClaimError::ZeroPrice));
    assert_eq!(check_close(&CloseClaim { exit_e12: 0, ..c }), Err(ClaimError::ZeroPrice));
    assert_eq!(check_close(&CloseClaim { timer_start: c.time + 1, ..c }), Err(ClaimError::TimerAfterTime));
    assert!(check_close(&CloseClaim { timer_start: c.time, ..c }).is_ok());
    // overflow is an error, not a panic
    let big = CloseClaim { qty: u64::MAX, entry_e12: 1, exit_e12: u64::MAX, pnl: 0, credited: 0, ..c };
    assert_eq!(check_close(&big), Err(ClaimError::Engine(EngineError::Overflow)));
    let far = CloseClaim { time: i64::MAX, timer_start: -1, ..c };
    assert_eq!(check_close(&far), Err(ClaimError::Engine(EngineError::Overflow)));
}

// ---------------------------------------------------------------- I-9 tight start

#[test]
fn tight_start_accepts_the_design_4_9_values() {
    let st = check_tight_start(&fresh(), 1_235_161_000, usd(100_000), usd(96_000), 1_235_161_000, 96_235_161_000).unwrap();
    assert!(st.tight_on);
    assert_eq!((st.r, st.l_start, st.g_peak, st.l_level), (usd(100_000), usd(96_000), 1_235_161_000, 96_235_161_000));
    assert_eq!(st.b, S);
}

#[test]
fn tight_start_p_open_must_be_strictly_above_one_percent() {
    let ok = |p: i64| check_tight_start(&fresh(), p, usd(100_000), usd(96_000), usd(1_000).min(p), usd(96_000));
    assert_eq!(ok(usd(1_000)), Err(ClaimError::TightStartPOpen));
    assert_eq!(ok(usd(1_000) - 1), Err(ClaimError::TightStartPOpen));
    assert!(ok(usd(1_000) + 1).is_ok());
}

#[test]
fn tight_start_r_must_equal_the_closed_balance() {
    for r in [usd(100_000) - 1, usd(100_000) + 1, 0] {
        assert_eq!(check_tight_start(&fresh(), usd(1_200), r, usd(96_000), usd(1_200), usd(96_200)), Err(ClaimError::TightStartR), "r {r}");
    }
}

#[test]
fn tight_start_l_start_is_min_of_floor_plus_four_percent_and_r() {
    for l in [usd(96_000) - 1, usd(96_000) + 1] {
        assert_eq!(check_tight_start(&fresh(), usd(1_200), usd(100_000), l, usd(1_200), l + usd(200)), Err(ClaimError::TightStartLStart), "l_start {l}");
    }
    // the cap at R (decision 31): B 93k -> l_start 93k, not 96k
    let mut st = fresh();
    st.b = usd(93_000);
    let ok = check_tight_start(&st, usd(1_100), usd(93_000), usd(93_000), usd(1_100), usd(93_100)).unwrap();
    assert_eq!(ok.l_level, usd(93_100));
    assert_eq!(check_tight_start(&st, usd(1_100), usd(93_000), usd(96_000), usd(1_100), usd(96_100)), Err(ClaimError::TightStartLStart));
    // latched: floor 96k, so 100k when R is large
    let mut latched = fresh();
    latched.latch = true;
    latched.b = usd(104_000);
    assert!(check_tight_start(&latched, usd(2_000), usd(104_000), usd(100_000), usd(2_000), usd(101_000)).is_ok());
    assert_eq!(check_tight_start(&latched, usd(2_000), usd(104_000), usd(96_000), usd(2_000), usd(97_000)), Err(ClaimError::TightStartLStart));
}

#[test]
fn tight_start_l_level_follows_the_formula_to_the_micro_usd() {
    for l in [usd(96_200) - 1, usd(96_200) + 1] {
        assert_eq!(check_tight_start(&fresh(), usd(1_200), usd(100_000), usd(96_000), usd(1_200), l), Err(ClaimError::TightStartLLevel), "l_level {l}");
    }
    // g_peak under the slack: L == l_start
    assert!(check_tight_start(&fresh(), usd(1_200), usd(100_000), usd(96_000), usd(500), usd(96_000)).is_ok());
    assert_eq!(check_tight_start(&fresh(), usd(1_200), usd(100_000), usd(96_000), usd(500), usd(95_999)), Err(ClaimError::TightStartLLevel));
}

#[test]
fn tight_start_g_peak_is_between_zero_and_p_open() {
    let try_g = |g: i64| check_tight_start(&fresh(), usd(1_200), usd(100_000), usd(96_000), g, usd(96_000) + (g - usd(1_000)).max(0));
    assert!(try_g(0).is_ok());
    assert!(try_g(usd(1_200)).is_ok());
    assert_eq!(try_g(-1), Err(ClaimError::TightStartGPeak));
    assert_eq!(try_g(usd(1_200) + 1), Err(ClaimError::TightStartGPeak));
}

#[test]
fn tight_start_is_refused_while_tight_mode_is_on() {
    assert_eq!(check_tight_start(&tight(), usd(1_200), usd(100_000), usd(96_000), usd(1_200), usd(96_200)), Err(ClaimError::TightAlreadyOn));
}

// ---------------------------------------------------------------- I-9 floor raised, tight end

#[test]
fn floor_raised_needs_a_strictly_higher_g_peak_and_the_formula_l() {
    let st = tight(); // g_peak 1.2k, L 96.2k, l_start 96k
    let raised = check_floor_raised(&st, usd(3_000), usd(98_000)).unwrap();
    assert_eq!((raised.g_peak, raised.l_level, raised.l_start, raised.r), (usd(3_000), usd(98_000), usd(96_000), usd(100_000)));
    assert_eq!(check_floor_raised(&st, usd(1_200), usd(96_200)), Err(ClaimError::FloorNotRaised));
    assert_eq!(check_floor_raised(&st, usd(1_200) - 1, usd(96_200)), Err(ClaimError::FloorNotRaised));
    assert!(check_floor_raised(&st, usd(1_200) + 1, usd(96_200) + 1).is_ok());
    for l in [usd(98_000) - 1, usd(98_000) + 1] {
        assert_eq!(check_floor_raised(&st, usd(3_000), l), Err(ClaimError::FloorLLevel), "l {l}");
    }
    assert_eq!(check_floor_raised(&fresh(), usd(3_000), usd(98_000)), Err(ClaimError::TightNotOn));
}

#[test]
fn tight_end_needs_tight_mode_and_resets_it() {
    let ended = check_tight_end(&tight()).unwrap();
    assert!(!ended.tight_on && ended.flat);
    assert_eq!((ended.r, ended.l_start, ended.g_peak, ended.l_level), (0, 0, 0, 0));
    assert_eq!(check_tight_end(&fresh()), Err(ClaimError::TightNotOn));
}

// ---------------------------------------------------------------- I-10 breach

fn breach_state(b: i64) -> AccountState {
    let mut st = tight();
    st.b = b;
    st
}

#[test]
fn breach_balance_must_be_at_or_below_the_tight_floor() {
    let l = usd(96_200);
    assert!(check_breach(&breach_state(l), 5_000, l, l).is_ok(), "== L breaches");
    assert_eq!(check_breach(&breach_state(l + 1), 5_000, l, l + 1), Err(ClaimError::BreachNotBelowFloor), "L + 1 micro does not");
    assert!(check_breach(&breach_state(l - 1), 5_000, l, l - 1).is_ok());
}

#[test]
fn breach_claims_must_match_the_stored_state() {
    let st = breach_state(usd(96_000) + 1);
    assert_eq!(check_breach(&st, 5_000, usd(96_200) + 1, usd(96_000) + 1), Err(ClaimError::BreachLLevel));
    assert_eq!(check_breach(&st, 5_000, usd(96_200) - 1, usd(96_000) + 1), Err(ClaimError::BreachLLevel));
    assert_eq!(check_breach(&st, 5_000, usd(96_200), usd(96_000) + 2), Err(ClaimError::BreachBalance));
    assert_eq!(check_breach(&st, 5_000, usd(96_200), usd(96_000)), Err(ClaimError::BreachBalance));
    assert_eq!(check_breach(&fresh(), 5_000, 0, S), Err(ClaimError::TightNotOn));
}

#[test]
fn breach_at_or_below_the_static_floor_is_a_fail_not_a_breach() {
    let f = usd(92_000);
    assert_eq!(check_breach(&breach_state(f), 5_000, usd(96_200), f), Err(ClaimError::BreachBelowStaticFloor));
    assert_eq!(check_breach(&breach_state(f - 1), 5_000, usd(96_200), f - 1), Err(ClaimError::BreachBelowStaticFloor));
    assert!(check_breach(&breach_state(f + 1), 5_000, usd(96_200), f + 1).is_ok());
}

#[test]
fn breach_returns_a_flat_state_even_if_the_stored_flag_was_not_flat() {
    let l = usd(96_200);
    let mut st = breach_state(l);
    st.flat = false;
    assert!(check_breach(&st, 5_000, l, l).unwrap().flat);
    // the other fields are carried over
    let after = check_breach(&st, 5_000, l, l).unwrap();
    assert_eq!((after.size_micro, after.b, after.latch, after.q_tally), (S, l, false, 0));
}

#[test]
fn breach_sets_the_twelve_hour_lock_and_ends_tight_mode() {
    let l = usd(96_200);
    let after = check_breach(&breach_state(l), 5_000, l, l).unwrap();
    assert_eq!(after.lock_until, 5_000 + 12 * 3600);
    assert!(!after.tight_on && after.flat);
    assert_eq!((after.r, after.l_start, after.g_peak, after.l_level), (0, 0, 0, 0));
    assert_eq!(after.b, l);
    assert_eq!(check_breach(&breach_state(l), i64::MAX, l, l), Err(ClaimError::Engine(EngineError::Overflow)));
}

// ---------------------------------------------------------------- slice_credit

#[test]
fn slice_credit_table() {
    assert_eq!(slice_credit(1_000, true), (500, 500));
    assert_eq!(slice_credit(1_001, true), (500, 500));
    assert_eq!(slice_credit(1, true), (0, 0));
    assert_eq!(slice_credit(0, true), (0, 0));
    assert_eq!(slice_credit(-400, true), (-400, 0));
    assert_eq!(slice_credit(-1, true), (-1, 0));
    assert_eq!(slice_credit(1_001, false), (1_001, 0));
    assert_eq!(slice_credit(-5, false), (-5, 0));
    assert_eq!(slice_credit(i64::MAX, true), (i64::MAX / 2, i64::MAX / 2));
}
