//! Floors, tight mode, quick tally, latch, phases (DESIGN 2, 4.4, 4.9). Money below is in micro-USD via `usd`.
use engine::*;

const S: i64 = 100_000_000_000; // $100,000
const T: i64 = 1_760_000_000;

fn usd(d: i64) -> i64 {
    d * 1_000_000
}

fn fresh() -> AccountState {
    AccountState::new_phase(S)
}

fn rules() -> Rules {
    Rules::default()
}

fn eval(st: &AccountState, p_open: i64, now: i64) -> SampleDecision {
    evaluate_sample(st, &rules(), p_open, 0, now).expect("evaluate")
}

fn credit(st: &AccountState, c: i64, q: i64) -> ClosedSliceDecision {
    apply_closed_slice(st, &rules(), c, q).expect("credit")
}

/// A tight mode started at B = 100k with P = 1.2k (R = 100k, l_start 96k, g_peak 1.2k, L 96.2k).
fn tight_state() -> AccountState {
    let d = eval(&fresh(), usd(1_200), T);
    assert!(matches!(d.action, SampleAction::TightStart { .. }));
    d.state
}

// ---------------------------------------------------------------- constructors

#[test]
fn new_phase_is_a_fresh_account() {
    let s = AccountState::new_phase(S);
    assert_eq!(s.size_micro, S);
    assert_eq!(s.b, S);
    assert!(!s.latch && !s.tight_on && s.flat);
    assert_eq!((s.r, s.l_start, s.g_peak, s.l_level, s.q_tally, s.lock_until), (0, 0, 0, 0, 0, 0));
    assert_eq!(reset_phase_state(S), s);
    let mut dirty = s;
    dirty.latch = true;
    assert_ne!(dirty, reset_phase_state(S));
}

#[test]
fn default_rules_are_the_stated_assumptions() {
    let r = Rules::default();
    assert!(r.frozen_ratchet, "A-RULE-1 default: frozen ratchet");
    assert!(r.quick_strict, "A-RULE-2 default: strictly greater");
}

#[test]
fn thresholds_are_exact_for_all_ten_sizes() {
    for dollars in [2_500i64, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000] {
        let st = AccountState::new_phase(usd(dollars));
        assert_eq!(static_floor_micro(&st), Ok(usd(dollars) / 100 * 92));
        assert_eq!(payout_micro(usd(dollars)), Ok(usd(dollars) / 100 * 8));
    }
}

#[test]
fn frac_exact_rejects_unusable_sizes() {
    assert_eq!(frac_exact(S, 9_200), Ok(92_000_000_000));
    assert_eq!(frac_exact(0, 9_200), Err(EngineError::InvalidInput));
    assert_eq!(frac_exact(-S, 9_200), Err(EngineError::InvalidInput));
    assert_eq!(frac_exact(1_000_001, 9_200), Err(EngineError::InvalidInput), "not whole dollars: 0.92 x S has a remainder");
    assert_eq!(frac_exact(i64::MAX, 20_000), Err(EngineError::Overflow), "2 x i64::MAX does not fit");
    assert_eq!(frac_exact(i64::MAX - 8, 10_000), Ok(i64::MAX - 8));
    assert_eq!(evaluate_sample(&AccountState::new_phase(1_000_001), &rules(), 0, 0, T), Err(EngineError::InvalidInput));
}

// ---------------------------------------------------------------- R1 (DESIGN 2.6)

#[test]
fn r1_table_all_five_rows() {
    let mut st = fresh();
    // t1: P +0.6k, E 100.6k, off
    let d = eval(&st, usd(600), T);
    assert_eq!(d.action, SampleAction::None);
    assert_eq!(d.equity_micro, usd(100_600));
    assert!(!d.state.tight_on);
    assert_eq!(static_floor_micro(&d.state), Ok(usd(92_000)));
    st = d.state;
    // t2: P +1.2k: on, R = 100k, L_start = 96k, G = 1.2k, L = 96.2k
    let d = eval(&st, usd(1_200), T + 1);
    assert_eq!(d.equity_micro, usd(101_200));
    assert_eq!(d.action, SampleAction::TightStart { r: usd(100_000), l_start: usd(96_000), g_peak: usd(1_200), l_level: usd(96_200) });
    st = d.state;
    assert!(st.tight_on);
    // t3: P +3.0k: G_peak 3.0k, L = 98.0k
    let d = eval(&st, usd(3_000), T + 2);
    assert_eq!(d.action, SampleAction::FloorRaised { g_peak: usd(3_000), l_level: usd(98_000) });
    st = d.state;
    // t4: P +1.0k: stays 98.0k
    let d = eval(&st, usd(1_000), T + 3);
    assert_eq!(d.action, SampleAction::None);
    assert_eq!((d.state.g_peak, d.state.l_level), (usd(3_000), usd(98_000)));
    st = d.state;
    // t5: P -2.0k: E 98.0k <= L 98.0k: breach, closed at this sample, locked 12 h, mode ends
    let d = eval(&st, usd(-2_000), T + 4);
    assert_eq!(d.equity_micro, usd(98_000));
    assert_eq!(d.action, SampleAction::Breach { close_all: true, lock_until: T + 4 + 12 * 3600 });
    assert!(!d.state.tight_on);
    assert_eq!(d.state.lock_until, T + 4 + 43_200);
    assert!(d.state.flat);
    // the evaluator closes the position at the sample: B = 98.0k
    let c = credit(&d.state, usd(-2_000), 0);
    assert_eq!(c.state.b, usd(98_000));
    assert_eq!(c.fail, None);
    assert!(!can_increase_exposure(&c.state, T + 4 + 43_199));
    assert!(can_increase_exposure(&c.state, T + 4 + 43_200));
}

// ---------------------------------------------------------------- R2 (permanent shift)

#[test]
fn r2_latch_is_permanent() {
    let mut st = fresh();
    assert_eq!(static_floor_micro(&st), Ok(usd(92_000)));
    st = credit(&st, usd(4_000), 0).state; // B 104k
    assert!(st.latch);
    assert_eq!(static_floor_micro(&st), Ok(usd(96_000)));
    st = credit(&st, usd(-3_000), 0).state; // B 101k
    assert_eq!(st.b, usd(101_000));
    assert!(st.latch, "latch never clears");
    assert_eq!(static_floor_micro(&st), Ok(usd(96_000)));
    // a sample with E <= 96k fails the account
    assert_eq!(eval(&st, usd(-5_000), T).action, SampleAction::Fail(FailReason::StaticFloor)); // E 96k
    assert_eq!(eval(&st, usd(-5_000) + 1, T).action, SampleAction::None); // E 96k + 1 micro
    assert_eq!(eval(&st, usd(-8_000), T).action, SampleAction::Fail(FailReason::StaticFloor)); // 93k: above the old floor, below the new
}

// ---------------------------------------------------------------- R3 (quick credit) and the quick limit

#[test]
fn r3_quick_credit_and_losses() {
    let st = fresh();
    // +1,000 quick slice: the engine reports credit 500 and tally delta 500
    let st = credit(&st, usd(500), usd(500)).state;
    assert_eq!((st.b, st.q_tally), (usd(100_500), usd(500)));
    // -400 quick: B - 400, Q unchanged
    let st = credit(&st, usd(-400), 0).state;
    assert_eq!((st.b, st.q_tally), (usd(100_100), usd(500)));
}

#[test]
fn r3_the_slice_numbers_come_from_the_netting_module() {
    // +1,000 raw quick profit: 1 EUR... use the fill engine for the numbers: 1,000,000 units x 0.001 = 1,000 micro-USD scale
    let open = apply_fill(None, Side::Buy, 1_000_000_000_000, 1_000_000_000_000, T, 0).unwrap();
    let close = apply_fill(open.position, Side::Sell, 1_000_000_000_000, 1_000_000_001_000, T + 10, 0).unwrap();
    assert_eq!(close.closed.unwrap().pnl, 1_000);
    let st = credit(&fresh(), close.balance_delta, close.quick_tally_delta).state;
    assert_eq!((st.b - S, st.q_tally), (500, 500));
}

#[test]
fn quick_limit_strict_q_equal_does_not_fail_q_plus_one_micro_fails() {
    let limit = usd(2_800);
    let at = credit(&fresh(), 0, limit);
    assert_eq!(at.state.q_tally, limit);
    assert_eq!(at.fail, None, "Q == 0.028 S does not fail under strict");
    let over = credit(&fresh(), 0, limit + 1);
    assert_eq!(over.fail, Some(FailReason::PhaseQuick));
    // from just below, crossing in a second slice
    let below = credit(&fresh(), 0, limit - 1);
    assert_eq!(below.fail, None);
    assert_eq!(credit(&below.state, 0, 1).fail, None, "now exactly at the limit");
    assert_eq!(credit(&below.state, 0, 2).fail, Some(FailReason::PhaseQuick));
}

#[test]
fn quick_limit_non_strict_q_equal_fails() {
    let loose = Rules { quick_strict: false, ..Rules::default() };
    let limit = usd(2_800);
    assert_eq!(apply_closed_slice(&fresh(), &loose, 0, limit).unwrap().fail, Some(FailReason::PhaseQuick));
    assert_eq!(apply_closed_slice(&fresh(), &loose, 0, limit - 1).unwrap().fail, None);
    assert_eq!(apply_closed_slice(&fresh(), &loose, 0, limit + 1).unwrap().fail, Some(FailReason::PhaseQuick));
}

#[test]
fn quick_losses_never_reduce_the_tally() {
    let st = credit(&fresh(), usd(500), usd(500)).state;
    let after_loss = credit(&st, usd(-3_000), 0).state;
    assert_eq!(after_loss.q_tally, usd(500));
    // a negative delta (a loss entering the tally) is refused
    assert_eq!(apply_closed_slice(&st, &rules(), usd(-1), -1), Err(EngineError::InvalidInput));
    // a non-quick profit adds to B but not to Q
    let st2 = credit(&st, usd(1_000), 0).state;
    assert_eq!((st2.b, st2.q_tally), (usd(101_500), usd(500)));
}

#[test]
fn the_quick_check_happens_when_the_slice_closes_and_independent_of_balance() {
    let mut st = fresh();
    st = credit(&st, usd(-5_000), usd(2_800)).state; // a loss and a tally at the limit
    assert_eq!(st.q_tally, usd(2_800));
    let d = credit(&st, usd(1), 1);
    assert_eq!(d.fail, Some(FailReason::PhaseQuick));
    assert_eq!(d.state.q_tally, usd(2_800) + 1, "the state still records the slice");
}

// ---------------------------------------------------------------- decision 31 (cap at R)

#[test]
fn decision_31_example_b_93k() {
    // B 93k, F_s 92k, P +1.1k: L_start = min(96k, 93k) = 93k, L = 93k + max(0, 1.1k - 1k) = 93.1k
    let mut st = fresh();
    st.b = usd(93_000);
    let d = eval(&st, usd(1_100), T);
    assert_eq!(d.action, SampleAction::TightStart { r: usd(93_000), l_start: usd(93_000), g_peak: usd(1_100), l_level: usd(93_100) });
}

#[test]
fn the_cap_is_inactive_when_b_is_at_least_f_s_plus_four_percent() {
    // B = F_s + 0.04 S = 96k: min(96k, 96k) = 96k (equal); B = 96k + 1: still 96k
    for (b, want) in [(usd(96_000), usd(96_000)), (usd(96_000) + 1, usd(96_000)), (usd(100_000), usd(96_000))] {
        let mut st = fresh();
        st.b = b;
        let d = eval(&st, usd(1_500), T);
        match d.action {
            SampleAction::TightStart { l_start, .. } => assert_eq!(l_start, want, "b {b}"),
            other => panic!("expected tight start, got {other:?}"),
        }
    }
    // one micro below F_s + 0.04 S the cap bites
    let mut st = fresh();
    st.b = usd(96_000) - 1;
    match eval(&st, usd(1_500), T).action {
        SampleAction::TightStart { l_start, .. } => assert_eq!(l_start, usd(96_000) - 1),
        other => panic!("{other:?}"),
    }
}

#[test]
fn latched_static_floor_changes_l_start() {
    // latched: F_s 96k, so min(100k, R); with R = 104k the cap does nothing -> 100k
    let mut st = fresh();
    st.latch = true;
    st.b = usd(104_000);
    match eval(&st, usd(2_000), T).action {
        SampleAction::TightStart { l_start, g_peak, l_level, r } => {
            assert_eq!((r, l_start, g_peak, l_level), (usd(104_000), usd(100_000), usd(2_000), usd(101_000)));
        }
        other => panic!("{other:?}"),
    }
    // R below F_s + 0.04 S: R is the cap
    st.b = usd(99_000);
    match eval(&st, usd(2_000), T).action {
        SampleAction::TightStart { l_start, .. } => assert_eq!(l_start, usd(99_000)),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- DESIGN 4.9, row by row

#[test]
fn design_4_9_table_row_by_row() {
    let t0 = T;
    let st = fresh();
    // trader buys 1,000,000 EUR at 14:00:00; mid 1.08, half-spread 30 e6
    let ask = ask_from_mid(1_080_000_000_000, 30).unwrap();
    assert_eq!(ask, 1_080_032_400_000, "ask 1.0800324");
    let qty = 1_000_000_000_000u64; // 1,000,000 EUR in 1e-6 units
    assert_eq!(margin_micro(qty, ask, 50), Ok(21_600_648_000), "margin $21,600.648");
    let fill = apply_fill(None, Side::Buy, qty, ask, t0, day_index(t0)).unwrap();
    let pos = fill.position.unwrap();

    let mark = |mid: u64| bid_from_mid(mid, 30).unwrap();
    let p_at = |mid: u64| open_profit_micro(&[(pos, mark(mid), ask_from_mid(mid, 30).unwrap())]).unwrap();

    // 14:00:00 mid 1.08000: bid 1.0799676, P -64.80, E 99,935.20, nothing
    assert_eq!(mark(1_080_000_000_000), 1_079_967_600_000);
    let d = eval(&st, p_at(1_080_000_000_000), t0);
    assert_eq!((d.equity_micro, d.action), (99_935_200_000, SampleAction::None));
    // 14:00:30 mid 1.08060: P +535.18 (below 1%)
    let d = eval(&d.state, p_at(1_080_600_000_000), t0 + 30);
    assert_eq!((p_at(1_080_600_000_000), d.equity_micro, d.action), (535_182_000, 100_535_182_000, SampleAction::None));
    // 14:01:00 mid 1.08130: P +1,235.161: tight mode ON, R = 100,000, l_start = 96,000, g_peak 1,235.161, L 96,235.161
    let d = eval(&d.state, p_at(1_081_300_000_000), t0 + 60);
    assert_eq!(d.equity_micro, 101_235_161_000);
    assert_eq!(
        d.action,
        SampleAction::TightStart { r: 100_000_000_000, l_start: 96_000_000_000, g_peak: 1_235_161_000, l_level: 96_235_161_000 }
    );
    // 14:01:30 mid 1.08310: new high g_peak 3,035.107, L 98,035.107
    let d = eval(&d.state, p_at(1_083_100_000_000), t0 + 90);
    assert_eq!(d.action, SampleAction::FloorRaised { g_peak: 3_035_107_000, l_level: 98_035_107_000 });
    // 14:02:00 mid 1.08110: P +1,035.17; nothing falls
    let d = eval(&d.state, p_at(1_081_100_000_000), t0 + 120);
    assert_eq!(p_at(1_081_100_000_000), 1_035_167_000);
    assert_eq!(d.action, SampleAction::None);
    assert_eq!((d.state.g_peak, d.state.l_level), (3_035_107_000, 98_035_107_000));
    // 14:02:30 mid 1.07800: E 97,935.26 <= 98,035.107: breach
    let tight = d.state;
    let d = eval(&tight, p_at(1_078_000_000_000), t0 + 150);
    assert_eq!(d.equity_micro, 97_935_260_000);
    assert_eq!(d.action, SampleAction::Breach { close_all: true, lock_until: t0 + 150 + 43_200 });
    // close at the sample bid 1.07796766: pnl -2,064.74, slice age 150 s so not quick, B = 97,935.26
    let bid = mark(1_078_000_000_000);
    assert_eq!(bid, 1_077_967_660_000, "bid 1.07796766");
    let close = apply_fill(Some(pos), Side::Sell, qty, bid, t0 + 150, day_index(t0 + 150)).unwrap();
    let slice = close.closed.unwrap();
    assert_eq!((slice.pnl, slice.quick, slice.credit), (-2_064_740_000, false, -2_064_740_000));
    assert_eq!(close.position, None);
    let after = credit(&d.state, close.balance_delta, close.quick_tally_delta);
    assert_eq!(after.state.b, 97_935_260_000);
    assert_eq!(after.state, end_tight_if_flat(&after.state, true));
    assert!(!after.state.tight_on);
    assert_eq!(after.state.lock_until, t0 + 150 + 43_200);
}

#[test]
fn design_4_9_late_sample_is_a_static_fail_not_a_breach() {
    // same account in tight mode (L 98,035.107), then the next sample is mid 1.06000: P -20,064.20, E 79,935.80
    let ask = 1_080_032_400_000u64;
    let pos = Position { qty: 1_000_000_000_000, avg_entry_e12: ask, timer_start: T, opened_at: T, fee_day: 0 };
    let mut st = fresh();
    st.tight_on = true;
    st.r = 100_000_000_000;
    st.l_start = 96_000_000_000;
    st.g_peak = 3_035_107_000;
    st.l_level = 98_035_107_000;
    let bid = bid_from_mid(1_060_000_000_000, 30).unwrap();
    let p = open_profit_micro(&[(pos, bid, 0)]).unwrap();
    assert_eq!(p, -20_064_200_000);
    let d = eval(&st, p, T + 160);
    assert_eq!(d.equity_micro, 79_935_800_000);
    assert!(d.equity_micro <= 98_035_107_000, "the tight floor is hit");
    assert!(d.equity_micro <= 92_000_000_000, "and so is the static floor");
    assert_eq!(d.action, SampleAction::Fail(FailReason::StaticFloor), "the static fail beats the breach");
    // the evaluator closes at the sample: B = 79,935.80
    let close = apply_fill(Some(pos), Side::Sell, 1_000_000_000_000, bid, T + 160, 0).unwrap();
    assert_eq!(credit(&d.state, close.balance_delta, close.quick_tally_delta).state.b, 79_935_800_000);
}

// ---------------------------------------------------------------- boundaries

#[test]
fn boundary_e_equal_f_s_fails_and_f_s_plus_one_does_not() {
    assert_eq!(eval(&fresh(), usd(-8_000), T).action, SampleAction::Fail(FailReason::StaticFloor));
    assert_eq!(eval(&fresh(), usd(-8_000) + 1, T).action, SampleAction::None);
    assert_eq!(eval(&fresh(), usd(-8_000) - 1, T).action, SampleAction::Fail(FailReason::StaticFloor));
}

#[test]
fn boundary_e_equal_l_breaches_and_l_plus_one_does_not() {
    let st = tight_state(); // L = 96.2k, B = 100k
    assert_eq!(st.l_level, usd(96_200));
    let at = eval(&st, usd(-3_800), T + 5); // E = 96.2k
    assert_eq!(at.action, SampleAction::Breach { close_all: true, lock_until: T + 5 + 43_200 });
    let above = eval(&st, usd(-3_800) + 1, T + 5);
    assert_eq!(above.action, SampleAction::None);
    let below = eval(&st, usd(-3_800) - 1, T + 5);
    assert!(matches!(below.action, SampleAction::Breach { .. }));
}

#[test]
fn boundary_tight_start_is_strictly_above_one_percent() {
    let one_pct = usd(1_000);
    assert_eq!(eval(&fresh(), one_pct, T).action, SampleAction::None);
    assert!(matches!(eval(&fresh(), one_pct + 1, T).action, SampleAction::TightStart { .. }));
    assert_eq!(eval(&fresh(), one_pct - 1, T).action, SampleAction::None);
    // the start condition is on P, not E: a commission estimate does not stop it
    let d = evaluate_sample(&fresh(), &rules(), one_pct + 1, usd(1_500), T).unwrap();
    // G = max(0, E - R) = 0 with E below R, so L = l_start + max(0, 0 - 0.01 S) = l_start (not below it)
    assert_eq!(
        d.action,
        SampleAction::TightStart { r: usd(100_000), l_start: usd(96_000), g_peak: 0, l_level: usd(96_000) }
    );
}

#[test]
fn boundary_latch_at_exactly_104_percent() {
    let at = credit(&fresh(), usd(4_000), 0).state;
    assert!(at.latch);
    let below = credit(&fresh(), usd(4_000) - 1, 0).state;
    assert!(!below.latch);
    assert_eq!(static_floor_micro(&below), Ok(usd(92_000)));
    // crossing in two steps
    assert!(credit(&below, 1, 0).state.latch);
    // above stays latched
    assert!(credit(&at, usd(-10), 0).state.latch);
}

#[test]
fn boundary_pass_at_exactly_108_percent() {
    let mut st = fresh();
    st.b = usd(108_000);
    assert_eq!(pass_check(&st, true, false), Ok(()));
    st.b = usd(108_000) - 1;
    assert_eq!(pass_check(&st, true, false), Err(PassError::TargetNotReached));
    st.b = usd(108_000) + 1;
    assert_eq!(pass_check(&st, true, false), Ok(()));
}

// ---------------------------------------------------------------- order of checks and the tight lifecycle

#[test]
fn static_fail_comes_before_tight_breach_even_when_l_was_skipped() {
    let st = tight_state();
    let d = eval(&st, usd(-20_000), T + 5); // E 80k: below both
    assert_eq!(d.action, SampleAction::Fail(FailReason::StaticFloor));
    assert_eq!(d.state, st, "a fail leaves the state as it was");
    // not tight: still a fail
    assert_eq!(eval(&fresh(), usd(-20_000), T).action, SampleAction::Fail(FailReason::StaticFloor));
}

#[test]
fn breach_comes_before_sl_tp() {
    let st = tight_state();
    let d = eval(&st, usd(-4_000), T + 5);
    assert!(matches!(d.action, SampleAction::Breach { .. }));
    assert!(!sl_tp_allowed_after(&d.action));
    assert!(!sl_tp_allowed_after(&SampleAction::Fail(FailReason::StaticFloor)));
    assert!(!sl_tp_allowed_after(&SampleAction::Fail(FailReason::PhaseQuick)));
    assert!(sl_tp_allowed_after(&SampleAction::None));
    assert!(sl_tp_allowed_after(&SampleAction::TightStart { r: 0, l_start: 0, g_peak: 0, l_level: 0 }));
    assert!(sl_tp_allowed_after(&SampleAction::FloorRaised { g_peak: 0, l_level: 0 }));
}

#[test]
fn tight_mode_does_not_end_while_a_position_is_open_and_ends_when_flat() {
    let st = tight_state();
    let still = end_tight_if_flat(&st, false);
    assert!(still.tight_on);
    assert_eq!((still.r, still.l_start, still.g_peak, still.l_level), (st.r, st.l_start, st.g_peak, st.l_level));
    assert!(!still.flat);
    let ended = end_tight_if_flat(&st, true);
    assert!(!ended.tight_on);
    assert!(ended.flat);
    assert_eq!((ended.r, ended.l_start, ended.g_peak, ended.l_level), (0, 0, 0, 0));
    assert_eq!(ended.b, st.b);
    // when tight mode is already off, only `flat` changes
    let f = fresh();
    assert_eq!(end_tight_if_flat(&f, true), f);
    assert!(!end_tight_if_flat(&f, false).flat);
}

#[test]
fn a_breach_sets_flat_even_if_the_account_was_not_flat() {
    // the evaluator closes everything at the breach sample, so the returned state is flat
    let open = end_tight_if_flat(&tight_state(), false);
    assert!(!open.flat);
    let d = eval(&open, usd(-4_000), T + 5);
    assert!(matches!(d.action, SampleAction::Breach { .. }));
    assert!(d.state.flat);
    // other actions leave `flat` as the evaluator last asserted it
    assert!(!eval(&open, usd(100), T + 5).state.flat);
}

#[test]
fn closing_a_winner_does_not_reduce_g() {
    // G = E - R includes realized profit: realizing the open profit keeps G and L
    let st = tight_state();
    let realized = credit(&st, usd(1_500), 0).state; // B 101.5k
    let d = eval(&realized, 0, T + 5); // E = 101.5k, G = 1.5k > g_peak 1.2k: raises
    assert_eq!(d.action, SampleAction::FloorRaised { g_peak: usd(1_500), l_level: usd(96_500) });
    // and a flat sample at the same equity afterwards changes nothing
    assert_eq!(eval(&d.state, 0, T + 6).action, SampleAction::None);
}

#[test]
fn a_new_mode_after_a_breach_starts_fresh() {
    let st = tight_state();
    let breach = eval(&st, usd(-4_000), T + 5);
    let after = credit(&breach.state, usd(-4_000), 0).state; // B 96k, flat, lock set
    assert!(!after.tight_on);
    assert_eq!(after.l_level, 0);
    // later: B 96k; first P > 1k starts a NEW mode with R = 96k and l_start = min(96k, 96k) = 96k
    let d = eval(&after, usd(1_100), T + 50_000);
    assert_eq!(d.action, SampleAction::TightStart { r: usd(96_000), l_start: usd(96_000), g_peak: usd(1_100), l_level: usd(96_100) });
}

#[test]
fn a_locked_account_still_runs_all_floors() {
    let mut st = fresh();
    st.lock_until = T + 1_000;
    assert_eq!(eval(&st, usd(-8_000), T).action, SampleAction::Fail(FailReason::StaticFloor));
    assert!(matches!(eval(&st, usd(1_500), T).action, SampleAction::TightStart { .. }));
    assert!(!can_increase_exposure(&st, T));
    assert!(!can_increase_exposure(&st, T + 999));
    assert!(can_increase_exposure(&st, T + 1_000));
    assert!(can_increase_exposure(&fresh(), 0));
}

#[test]
fn the_ratchet_only_raises_on_a_strictly_higher_g() {
    let st = tight_state(); // g_peak 1.2k
    assert_eq!(eval(&st, usd(1_200), T + 1).action, SampleAction::None, "equal G does not raise");
    assert!(matches!(eval(&st, usd(1_200) + 1, T + 1).action, SampleAction::FloorRaised { .. }));
    assert_eq!(eval(&st, usd(500), T + 1).action, SampleAction::None);
    // L stays put while G is below the peak
    let d = eval(&st, usd(-3_000), T + 1);
    assert_eq!(d.state.l_level, st.l_level);
}

#[test]
fn raising_below_the_slack_keeps_l_at_l_start() {
    // g_peak 0.5k (mode started with E - R small? use B 93k, +1.1k, then G 0.9k < slack): L = l_start
    let mut st = fresh();
    st.b = usd(93_000);
    let started = eval(&st, usd(1_100), T).state; // g_peak 1.1k, L 93.1k
    assert_eq!(started.l_level, usd(93_100));
    let raised = eval(&started, usd(1_300), T + 1);
    assert_eq!(raised.action, SampleAction::FloorRaised { g_peak: usd(1_300), l_level: usd(93_300) });
}

#[test]
fn commission_estimate_lowers_equity_and_can_fail_or_breach() {
    // E = B + P - commission
    let d = evaluate_sample(&fresh(), &rules(), 0, usd(10), T).unwrap();
    assert_eq!(d.equity_micro, usd(99_990));
    // exactly at the static floor through the commission
    let d = evaluate_sample(&fresh(), &rules(), usd(-7_900), usd(100), T).unwrap();
    assert_eq!(d.action, SampleAction::Fail(FailReason::StaticFloor));
    let d = evaluate_sample(&fresh(), &rules(), usd(-7_900), usd(99), T).unwrap();
    assert_eq!(d.action, SampleAction::None);
    // tight breach through the commission
    let st = tight_state();
    let d = evaluate_sample(&st, &rules(), usd(-3_000), usd(800), T + 1).unwrap();
    assert!(matches!(d.action, SampleAction::Breach { .. })); // E = 96.2k
    let d = evaluate_sample(&st, &rules(), usd(-3_000), usd(799), T + 1).unwrap();
    assert_eq!(d.action, SampleAction::None);
    assert_eq!(evaluate_sample(&st, &rules(), 0, -1, T), Err(EngineError::InvalidInput));
}

#[test]
fn evaluate_sample_overflow_is_an_error() {
    let mut st = fresh();
    st.b = i64::MAX;
    assert_eq!(evaluate_sample(&st, &rules(), 1, 0, T), Err(EngineError::Overflow));
    let mut neg = fresh();
    neg.b = i64::MIN;
    assert_eq!(evaluate_sample(&neg, &rules(), -1, 0, T), Err(EngineError::Overflow));
    // a breach whose lock end passes i64::MAX
    assert_eq!(evaluate_sample(&tight_state(), &rules(), usd(-3_900), 0, i64::MAX), Err(EngineError::Overflow));
}

// ---------------------------------------------------------------- A-RULE-1 (mid-mode latch)

#[test]
fn a_rule_1_default_keeps_l_frozen_when_the_latch_flips_mid_mode() {
    let st = tight_state(); // R 100k, l_start 96k, g_peak 1.2k, L 96.2k, F_s 92k
    let d = credit(&st, usd(4_000), 0); // B 104k: latch, F_s 96k
    assert!(d.state.latch);
    assert_eq!((d.state.l_start, d.state.l_level), (usd(96_000), usd(96_200)), "L frozen");
    assert_eq!(effective_floor_micro(&d.state), Ok(usd(96_200)), "max(F_s 96k, L 96.2k)");
    // E 100.1k is far above the frozen floor
    assert_eq!(eval(&d.state, usd(-3_900), T + 5).action, SampleAction::None);
}

#[test]
fn a_rule_1_effective_floor_is_the_maximum_of_f_s_and_l() {
    // a tight mode started with B = 93k has L = 93k; once latched (B 104k) F_s 96k is the larger
    let mut st = fresh();
    st.b = usd(93_000);
    st = eval(&st, usd(1_100), T).state; // L 93.1k
    let st = credit(&st, usd(11_000), 0).state; // B 104k, latched
    assert_eq!(st.l_level, usd(93_100));
    assert_eq!(static_floor_micro(&st), Ok(usd(96_000)));
    assert_eq!(effective_floor_micro(&st), Ok(usd(96_000)));
    // not in tight mode the effective floor is just F_s
    assert_eq!(effective_floor_micro(&fresh()), Ok(usd(92_000)));
    let mut tight_low = tight_state();
    tight_low.l_level = usd(90_000); // below F_s
    assert_eq!(effective_floor_micro(&tight_low), Ok(usd(92_000)));
}

#[test]
fn a_rule_1_off_recomputes_l_when_the_latch_flips_mid_mode() {
    let off = Rules { frozen_ratchet: false, ..Rules::default() };
    let st = tight_state(); // R 100k, g_peak 1.2k
    let d = apply_closed_slice(&st, &off, usd(4_000), 0).unwrap();
    // l_start = min(F_s_new 96k + 4k, R 100k) = 100k; L = 100k + max(0, 1.2k - 1k) = 100.2k; R and g_peak kept
    assert_eq!((d.state.r, d.state.g_peak), (usd(100_000), usd(1_200)));
    assert_eq!((d.state.l_start, d.state.l_level), (usd(100_000), usd(100_200)));
    assert!(d.state.l_level >= st.l_level, "L never falls");
    // E 100.1k breaches the recomputed floor but not the frozen one
    let probe = |rules: &Rules, state: &AccountState| evaluate_sample(state, rules, usd(-3_900), 0, T + 5).unwrap().action;
    assert!(matches!(probe(&off, &d.state), SampleAction::Breach { .. }));
    let frozen = apply_closed_slice(&st, &Rules::default(), usd(4_000), 0).unwrap();
    assert_eq!(probe(&Rules::default(), &frozen.state), SampleAction::None);
}

#[test]
fn a_rule_1_off_does_nothing_without_a_latch_flip_or_tight_mode() {
    let off = Rules { frozen_ratchet: false, ..Rules::default() };
    let st = tight_state();
    // credit that does not cross 1.04 S: levels unchanged
    let d = apply_closed_slice(&st, &off, usd(1_000), 0).unwrap();
    assert_eq!((d.state.l_start, d.state.l_level), (st.l_start, st.l_level));
    // latch flips while tight mode is off: nothing to recompute
    let d = apply_closed_slice(&fresh(), &off, usd(4_000), 0).unwrap();
    assert_eq!((d.state.l_start, d.state.l_level, d.state.tight_on), (0, 0, false));
    // already latched: a further credit never recomputes
    let latched = apply_closed_slice(&st, &off, usd(4_000), 0).unwrap().state;
    let again = apply_closed_slice(&latched, &off, usd(500), 0).unwrap();
    assert_eq!((again.state.l_start, again.state.l_level), (latched.l_start, latched.l_level));
}

#[test]
fn a_rule_1_off_cap_still_applies_with_a_small_r() {
    let off = Rules { frozen_ratchet: false, ..Rules::default() };
    let mut st = fresh();
    st.b = usd(93_000);
    let st = eval(&st, usd(1_100), T).state; // R 93k, l_start 93k, g_peak 1.1k, L 93.1k
    let d = apply_closed_slice(&st, &off, usd(11_000), 0).unwrap(); // B 104k: latch
    // l_start = min(96k + 4k, R 93k) = 93k (the cap R); L = 93k + 0.1k
    assert_eq!((d.state.l_start, d.state.l_level), (usd(93_000), usd(93_100)));
}

// ---------------------------------------------------------------- phases

#[test]
fn pass_check_requires_target_flat_and_not_failed() {
    let mut st = fresh();
    st.b = usd(108_000);
    assert_eq!(pass_check(&st, true, false), Ok(()));
    assert_eq!(pass_check(&st, false, false), Err(PassError::NotFlat));
    assert_eq!(pass_check(&st, true, true), Err(PassError::PhaseFailed));
    let mut low = st;
    low.b = usd(107_999);
    assert_eq!(pass_check(&low, true, false), Err(PassError::TargetNotReached));
    // order of reporting: failed first, then not flat, then target
    assert_eq!(pass_check(&low, false, true), Err(PassError::PhaseFailed));
    assert_eq!(pass_check(&low, false, false), Err(PassError::NotFlat));
    // an unusable size cannot reach any target
    let mut odd = fresh();
    odd.size_micro = 1_000_001;
    odd.b = i64::MAX;
    assert_eq!(pass_check(&odd, true, false), Err(PassError::TargetNotReached));
}

#[test]
fn payout_is_eight_percent_of_s() {
    assert_eq!(payout_micro(S), Ok(usd(8_000)));
    assert_eq!(payout_micro(usd(2_500)), Ok(usd(200)));
    assert_eq!(payout_micro(usd(200_000)), Ok(usd(16_000)));
    assert_eq!(payout_micro(1_000_001), Err(EngineError::InvalidInput));
}

#[test]
fn next_phase_table() {
    assert_eq!(next_phase(1), Ok(NextPhase::Next { phase: 2, payout: false }));
    assert_eq!(next_phase(2), Ok(NextPhase::Next { phase: 3, payout: true }));
    assert_eq!(next_phase(3), Ok(NextPhase::Next { phase: 4, payout: true }));
    assert_eq!(next_phase(4), Ok(NextPhase::Next { phase: 5, payout: true }));
    assert_eq!(next_phase(5), Ok(NextPhase::Graduated));
    assert_eq!(next_phase(0), Err(EngineError::InvalidInput));
    assert_eq!(next_phase(6), Err(EngineError::InvalidInput));
    assert_eq!(next_phase(255), Err(EngineError::InvalidInput));
    assert!(!next_phase(1).unwrap().pays());
    for p in 2..=5 {
        assert!(next_phase(p).unwrap().pays(), "phase {p}");
    }
}

#[test]
fn a_new_phase_after_a_pass_is_fresh() {
    let mut st = fresh();
    st = credit(&st, usd(9_000), usd(1_000)).state;
    st.tight_on = true;
    st.lock_until = 5;
    assert!(st.latch);
    let n = reset_phase_state(st.size_micro);
    assert_eq!(n, AccountState::new_phase(S));
    assert_eq!((n.b, n.q_tally, n.latch, n.tight_on, n.lock_until), (S, 0, false, false, 0));
    assert_eq!(static_floor_micro(&n), Ok(usd(92_000)));
}

#[test]
fn apply_closed_slice_overflow_is_an_error() {
    let mut st = fresh();
    st.b = i64::MAX;
    assert_eq!(apply_closed_slice(&st, &rules(), 1, 0), Err(EngineError::Overflow));
    let mut q = fresh();
    q.q_tally = i64::MAX;
    assert_eq!(apply_closed_slice(&q, &rules(), 0, 1), Err(EngineError::Overflow));
}

// ---------------------------------------------------------------- portfolio helper

#[test]
fn open_profit_sums_positions_at_bid_and_ask() {
    let long = Position { qty: 1_000_000, avg_entry_e12: 1_000_000_000_000, timer_start: 0, opened_at: 0, fee_day: 0 };
    let short = Position { qty: -1_000_000, ..long };
    assert_eq!(open_profit_micro(&[]), Ok(0));
    // long marked at bid 1.1: +100,000; short marked at ask 1.2: -200,000
    assert_eq!(open_profit_micro(&[(long, 1_100_000_000_000, 9), (short, 9, 1_200_000_000_000)]), Ok(-100_000));
    // each position is floored on its own: two -0.5 micro-USD losses make -2, not -1
    let tiny = Position { qty: 1, avg_entry_e12: 1_500_000_000_000, ..long };
    assert_eq!(open_profit_micro(&[(tiny, 1_000_000_000_000, 0), (tiny, 1_000_000_000_000, 0)]), Ok(-2));
    // a zero-quantity position is refused
    assert_eq!(open_profit_micro(&[(Position { qty: 0, ..long }, 1, 1)]), Err(EngineError::InvalidInput));
}

#[test]
fn open_profit_overflow_is_an_error() {
    let big = Position { qty: 1_000_000_000_000, avg_entry_e12: 0, timer_start: 0, opened_at: 0, fee_day: 0 };
    let bid = 9_223_372_036_854_775_807u64;
    assert_eq!(open_profit_micro(&[(big, bid, 0)]), Ok(i64::MAX));
    assert_eq!(open_profit_micro(&[(big, bid, 0), (big, 1_000_000, 0)]), Err(EngineError::Overflow));
    let neg = Position { qty: 1_000_000_000_000, avg_entry_e12: bid, ..big };
    assert_eq!(open_profit_micro(&[(neg, 0, 0), (neg, 0, 0)]), Err(EngineError::Overflow));
}
