//! M12 item 6 (first half), ARITHMETIC AND ROUNDING EXTREMES of `crates/engine` as the program, the evaluator and the verifier use it.
//!
//! SELF-AUDIT by the builder. Every formula of the engine that rounds is compared with an INDEPENDENT reference written here with
//! `u128`/`i128` and a tiny fixed-width 256-bit unsigned type (`U256`, four `u64` limbs: multiply by `u64`, divide by `u64`, compare).
//! The reference never calls the engine and shares no code with it. For each formula the test
//!   1. feeds the boundary grid (0, 1, 2, `u64::MAX`, `i64::MAX`/`MIN`, price 1, price `u64::MAX`, S = 1 USD, S = 200,000 USD, huge ages ...),
//!   2. feeds 100,000 random cases from a fixed seed (`SEED`, printed with `--nocapture`; the generator mixes the boundary values,
//!      small values, log-uniform values and full-range values),
//!   3. wraps every engine call in `catch_unwind`: a panic is a FINDING, never a pass,
//!   4. asserts the exact value (or the exact error: `EngineError::Overflow` when the exact result does not fit its type, never a wrapped number),
//!   5. asserts the DIRECTION with a cross-multiplication that does not use the division of the reference
//!      (a ceiling `c` of `n/d` satisfies `c*d >= n` and `(c-1)*d < n`; a floor `f` satisfies `f*d <= n < (f+1)*d`),
//!   6. counts how many cases really rounded (`rounded`) and how many returned a value (`ok`), and fails if the generator was vacuous.
//!
//! DIRECTION TABLE (DESIGN 7.4, "always against the trader"):
//!   formula                       rounds   why that is against the trader
//!   ask_from_mid                  UP       a buyer pays at least the exact price
//!   bid_from_mid                  DOWN     a seller receives at most the exact price
//!   price_from_oracle             DOWN     oracle to e12: floor (DESIGN 7.4)
//!   fill_price / close_price      ask UP, bid DOWN (a long closes at the bid, a short at the ask)
//!   margin_micro                  UP       one ceiling over the whole expression
//!   fee_ceil_micro (commission)   UP       a fee is never below the exact fee
//!   holding_fee_micro             UP       one ceiling over the exact expression
//!   swap_fee_micro (0.4 % fee)    UP
//!   pnl_micro / unrealized        DOWN     toward minus infinity: a profit is cut, a loss is enlarged
//!   slice_credit                  DOWN     a quick profit is credited as floor(p/2), a loss in full
//!   average entry on a merge      long UP, short DOWN   (the trader's break-even moves against him)
//!   frac_exact (floors, limits)   NEVER ROUNDS: an inexact division is `InvalidInput`
//!   notional_micro                TOWARD ZERO (floor) - NOT conservative where used as a cap, see FINDING ARI-2
//!
//! LIMITED controls are pinned exactly (`notional_micro` truncation, the `holding_fee_micro` overflow with `nights == 0`, the double ceiling of the
//! evaluator's commission). Real defects are `#[ignore = "FINDING ARI-n ..."]` tests that assert the CORRECT behaviour.
//!
//! Run: `cargo test --offline -p setl8-lev-evaluator --test rounding_properties -- --nocapture`
//!      `cargo test --offline -p setl8-lev-evaluator --test rounding_properties -- --ignored` (fails ONLY on the findings)
mod common;

use std::cmp::Ordering;
use std::fmt::Debug;
use std::panic::{catch_unwind, AssertUnwindSafe};

use engine::consts::{
    LATCHED_FLOOR_BPS, LATCH_TRIGGER_BPS, PAYOUT_BPS, QUICK_LIMIT_BPS, QUICK_SECS, RATCHET_SLACK_BPS, STATIC_FLOOR_BPS, TARGET_BPS, TIGHT_OFFSET_BPS,
    TIGHT_START_BPS,
};
use engine::units::{notional_micro, DEN};
use engine::{
    apply_closed_slice, apply_fill, ask_from_mid, bid_from_mid, can_increase_exposure, check_breach, check_close, check_floor_raised, check_pending,
    check_protective, check_tight_end, check_tight_start, close_price_e12, day_index, effective_floor_micro, end_tight_if_flat, evaluate_sample,
    fee_ceil_micro, fill_price_e12, frac_exact, holding_fee_micro, margin_micro, next_phase, nights_due, open_profit_micro, pass_check, payout_micro,
    pnl_micro, price_from_oracle, reset_phase_state, slice_credit, static_floor_micro, swap_fee_micro, unrealized_micro, validate_quantity,
    validate_stop_level, AccountState, ClaimError, CloseClaim, EngineError, NextPhase, PassError, PendingKind, Position, Rules, SampleAction,
    Side, StopKind,
};
use evaluator::testkit::Rng;

/// The fixed seed of every property test in this file. Each formula derives its own stream from it (`rng_for`), printed as well.
const SEED: u64 = 0x5E78_4D12_A21F_0006;
/// Random cases per formula (the boundary grid comes on top).
const CASES: u32 = 100_000;

// ------------------------------------------------------------------------------------------------------------- reference arithmetic

/// 256-bit unsigned integer, little-endian limbs. Just enough for products of up to four engine inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct U256([u64; 4]);

impl U256 {
    fn from_u128(v: u128) -> U256 {
        U256([v as u64, (v >> 64) as u64, 0, 0])
    }
    fn from_u64(v: u64) -> U256 {
        U256::from_u128(u128::from(v))
    }
    fn mul_u64(self, m: u64) -> U256 {
        let mut out = [0u64; 4];
        let mut carry: u128 = 0;
        for (o, limb) in out.iter_mut().zip(self.0.iter()) {
            let p = u128::from(*limb) * u128::from(m) + carry;
            *o = p as u64;
            carry = p >> 64;
        }
        assert_eq!(carry, 0, "reference U256 overflow (test bug)");
        U256(out)
    }
    fn divrem_u64(self, d: u64) -> (U256, u64) {
        assert!(d != 0);
        let mut out = [0u64; 4];
        let mut rem: u128 = 0;
        for i in (0..4).rev() {
            let cur = (rem << 64) | u128::from(self.0[i]);
            out[i] = (cur / u128::from(d)) as u64;
            rem = cur % u128::from(d);
        }
        (U256(out), rem as u64)
    }
    fn add_u64(self, a: u64) -> U256 {
        let mut out = self.0;
        let mut carry = u128::from(a);
        for limb in out.iter_mut() {
            let s = u128::from(*limb) + carry;
            *limb = s as u64;
            carry = s >> 64;
        }
        assert_eq!(carry, 0, "reference U256 overflow (test bug)");
        U256(out)
    }
    fn is_zero(self) -> bool {
        self.0 == [0; 4]
    }
    fn to_u128(self) -> Option<u128> {
        if self.0[2] != 0 || self.0[3] != 0 {
            None
        } else {
            Some(u128::from(self.0[0]) | (u128::from(self.0[1]) << 64))
        }
    }
    fn to_u64(self) -> Option<u64> {
        self.to_u128().and_then(|v| u64::try_from(v).ok())
    }
    fn to_i64(self) -> Option<i64> {
        self.to_u128().and_then(|v| i64::try_from(v).ok())
    }
    /// `ceil(self / d)`.
    fn ceil_div(self, d: u64) -> (U256, bool) {
        let (q, r) = self.divrem_u64(d);
        if r == 0 {
            (q, false)
        } else {
            (q.add_u64(1), true)
        }
    }
    /// `floor(self / d)`.
    fn floor_div(self, d: u64) -> (U256, bool) {
        let (q, r) = self.divrem_u64(d);
        (q, r != 0)
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, o: &U256) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for U256 {
    fn cmp(&self, o: &U256) -> Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&o.0[i]) {
                Ordering::Equal => continue,
                other => return other,
            }
        }
        Ordering::Equal
    }
}

const DEN_U64: u64 = 1_000_000_000_000;
const E6: u64 = 1_000_000;

/// Reference of a ceiling formula `ceil(num / den)` that must fit `i64`: checks the engine result and the direction. Returns whether it rounded.
fn check_ceil_i64(what: &str, inputs: &dyn Debug, got: Result<i64, EngineError>, num: U256, den: u64, t: &mut Tally) {
    let (c, rounded) = num.ceil_div(den);
    match c.to_i64() {
        Some(want) => {
            assert_eq!(got, Ok(want), "{what}{inputs:?}");
            let c256 = U256::from_u64(want as u64);
            assert!(c256.mul_u64(den) >= num, "{what}{inputs:?}: result {want} is below the exact value (favours the trader)");
            if want > 0 {
                assert!(U256::from_u64((want - 1) as u64).mul_u64(den) < num, "{what}{inputs:?}: result {want} is not the smallest ceiling");
            }
            t.ok += 1;
            t.rounded += u64::from(rounded);
        }
        None => {
            assert_eq!(got, Err(EngineError::Overflow), "{what}{inputs:?}: the exact result does not fit i64, the engine must say Overflow");
            t.err += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------------------- generators

const E64: [u64; 17] = [
    0,
    1,
    2,
    3,
    7,
    999_999,
    1_000_000,
    1_000_001,
    999_999_999_999,
    1_000_000_000_000,
    1_000_000_000_001,
    4_294_967_295,
    4_294_967_296,
    9_223_372_036_854_775_807,
    9_223_372_036_854_775_808,
    18_446_744_073_709_551_614,
    18_446_744_073_709_551_615,
];
const E32: [u32; 9] = [0, 1, 2, 30, 999_999, 1_000_000, 1_000_001, u32::MAX - 1, u32::MAX];
const E16: [u16; 9] = [0, 1, 2, 20, 50, 10_000, 40_000, u16::MAX - 1, u16::MAX];
const EI64: [i64; 15] = [
    i64::MIN,
    i64::MIN + 1,
    -1_000_000,
    -2,
    -1,
    0,
    1,
    2,
    1_000_000,
    100_000_000_000,
    200_000_000_000,
    1_000_000_000_000_000,
    i64::MAX / 2,
    i64::MAX - 1,
    i64::MAX,
];
/// 1 USD and the ten tier sizes, micro-USD.
const SIZES: [i64; 11] =
    [1_000_000, 2_500_000_000, 5_000_000_000, 10_000_000_000, 15_000_000_000, 20_000_000_000, 25_000_000_000, 50_000_000_000, 75_000_000_000, 100_000_000_000, 200_000_000_000];

/// Uniform on `[0, 2^bits)` after drawing the bit width uniformly in `0..=max_bits`: small and large magnitudes equally likely.
fn log_u64(r: &mut Rng, max_bits: i64) -> u64 {
    let bits = r.range(0, max_bits) as u32;
    if bits == 0 {
        0
    } else {
        r.next_u64() >> (64 - bits)
    }
}

fn g64(r: &mut Rng) -> u64 {
    match r.range(0, 9) {
        0 | 1 => r.pick(&E64),
        2 => r.range(0, 2_000) as u64,
        3 | 4 => log_u64(r, 64),
        5..=7 => log_u64(r, 50),
        _ => r.next_u64(),
    }
}
fn g32(r: &mut Rng) -> u32 {
    match r.range(0, 5) {
        0 => r.pick(&E32),
        1 => r.range(0, 5_000) as u32,
        2 | 3 => log_u64(r, 32) as u32,
        _ => r.next_u64() as u32,
    }
}
fn g16(r: &mut Rng) -> u16 {
    match r.range(0, 4) {
        0 => r.pick(&E16),
        1 => r.range(0, 300) as u16,
        _ => log_u64(r, 16) as u16,
    }
}
fn gi64(r: &mut Rng) -> i64 {
    match r.range(0, 9) {
        0 | 1 => r.pick(&EI64),
        2 | 3 => r.range(-2_000, 2_000),
        4..=6 => {
            let m = log_u64(r, 63) as i64;
            if r.chance(500_000) {
                -m
            } else {
                m
            }
        }
        _ => r.next_u64() as i64,
    }
}
/// A leverage a rule set could hold, plus the extremes.
fn glev(r: &mut Rng) -> u16 {
    match r.range(0, 3) {
        0 => r.pick(&[0u16, 1, 2, 5, 10, 20, 50, 100, 500, u16::MAX]),
        1 => g16(r),
        _ => r.pick(&[1u16, 2, 5, 10, 20, 50]),
    }
}

fn rng_for(formula: &str) -> Rng {
    let mut h = SEED;
    for b in formula.bytes() {
        h = h.wrapping_mul(0x0000_0100_0000_01B3) ^ u64::from(b);
    }
    println!("SEED {SEED:#x} formula {formula} stream-seed {h:#x} random-cases {CASES}");
    Rng::new(h)
}

/// Counters that prove a property test was not vacuous.
#[derive(Default)]
struct Tally {
    random: u64,
    grid: u64,
    ok: u64,
    err: u64,
    rounded: u64,
}

impl Tally {
    fn finish(&self, name: &str) {
        println!("RESULT {name}: seed={SEED:#x} random_cases={} grid_cases={} ok={} err={} rounded={}", self.random, self.grid, self.ok, self.err, self.rounded);
        assert!(self.random >= u64::from(CASES), "{name}: fewer random cases than required");
        assert!(self.ok >= 20_000, "{name}: only {} cases returned a value; the generator is too hostile to prove anything", self.ok);
        assert!(self.rounded >= 10_000, "{name}: only {} cases actually rounded; the generator is vacuous for rounding", self.rounded);
        assert!(self.err >= 1, "{name}: no case reached an error path");
    }
}

/// Calls `f` under `catch_unwind`: a panic becomes a test failure that names the formula and the inputs.
fn guarded<T>(what: &str, inputs: &dyn Debug, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => panic!("FINDING: PANIC in {what} with inputs {inputs:?} (the engine must return an error, never panic)"),
    }
}

// ================================================================================================================ PRICES

#[test]
fn ask_from_mid_rounds_up_for_the_buyer() {
    const NAME: &str = "ask_from_mid";
    let check = |mid: u64, h: u32, t: &mut Tally| {
        let got = guarded(NAME, &(mid, h), || ask_from_mid(mid, h));
        let num = U256::from_u64(mid).mul_u64(E6 + u64::from(h));
        let (c, rounded) = num.ceil_div(E6);
        match c.to_u64() {
            Some(want) => {
                assert_eq!(got, Ok(want), "ask_from_mid({mid}, {h})");
                assert!(U256::from_u64(want).mul_u64(E6) >= num, "ask {want} below the exact price for ({mid},{h})");
                if want > 0 {
                    assert!(U256::from_u64(want - 1).mul_u64(E6) < num, "ask {want} not minimal for ({mid},{h})");
                }
                assert!(want >= mid, "a buyer paid below the mid");
                t.ok += 1;
                t.rounded += u64::from(rounded);
            }
            None => {
                assert_eq!(got, Err(EngineError::Overflow), "ask_from_mid({mid}, {h}) does not fit u64");
                t.err += 1;
            }
        }
    };
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    for &m in &E64 {
        for &h in &E32 {
            check(m, h, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let mid = if rng.chance(600_000) { log_u64(&mut rng, 62) } else { g64(&mut rng) };
        let h = if rng.chance(700_000) { rng.range(0, 3_000) as u32 } else { g32(&mut rng) };
        check(mid, h, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn bid_from_mid_rounds_down_for_the_seller() {
    const NAME: &str = "bid_from_mid";
    let check = |mid: u64, h: u32, t: &mut Tally| {
        let got = guarded(NAME, &(mid, h), || bid_from_mid(mid, h));
        if u64::from(h) > E6 {
            assert_eq!(got, Err(EngineError::InvalidInput), "half-spread {h} above 100 % must be InvalidInput");
            t.err += 1;
            return;
        }
        let num = U256::from_u64(mid).mul_u64(E6 - u64::from(h));
        let (f, rounded) = num.floor_div(E6);
        let want = f.to_u64().expect("a bid never exceeds the mid");
        assert_eq!(got, Ok(want), "bid_from_mid({mid}, {h})");
        assert!(U256::from_u64(want).mul_u64(E6) <= num, "bid {want} above the exact price");
        assert!(U256::from_u64(want).add_u64(1).mul_u64(E6) > num, "bid {want} not maximal");
        assert!(want <= mid, "a seller received above the mid");
        t.ok += 1;
        t.rounded += u64::from(rounded);
    };
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    for &m in &E64 {
        for &h in &E32 {
            check(m, h, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let mid = if rng.chance(600_000) { log_u64(&mut rng, 62) } else { g64(&mut rng) };
        let h = match rng.range(0, 9) {
            0 => rng.range(999_000, 1_000_100) as u32,
            1 => g32(&mut rng),
            _ => rng.range(0, 3_000) as u32,
        };
        check(mid, h, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn price_from_oracle_floors() {
    const NAME: &str = "price_from_oracle";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |v: u128, t: &mut Tally| {
        let got = guarded(NAME, &v, || price_from_oracle(v));
        let (q, r) = (v / 1_000_000, v % 1_000_000);
        match u64::try_from(q) {
            Ok(want) => {
                assert_eq!(got, Ok(want), "price_from_oracle({v})");
                assert!(u128::from(want) * 1_000_000 <= v && v < (u128::from(want) + 1) * 1_000_000 || want == u64::MAX, "not a floor for {v}");
                t.ok += 1;
                t.rounded += u64::from(r != 0);
            }
            Err(_) => {
                assert_eq!(got, Err(EngineError::Overflow), "price_from_oracle({v})");
                t.err += 1;
            }
        }
    };
    let edges: [u128; 12] = [0, 1, 999_999, 1_000_000, 1_000_001, u128::from(u64::MAX), u128::from(u64::MAX) * 1_000_000, u128::from(u64::MAX) * 1_000_000 + 999_999, (u128::from(u64::MAX) + 1) * 1_000_000, u128::MAX - 1, u128::MAX, 1 << 100];
    for &v in &edges {
        check(v, &mut t);
        t.grid += 1;
    }
    for _ in 0..CASES {
        let hi = u128::from(g64(&mut rng));
        let lo = u128::from(g64(&mut rng));
        let v = match rng.range(0, 3) {
            0 => (hi << 64) | lo,
            1 => hi.wrapping_mul(1_000_000).wrapping_add(lo % 2_000_000),
            _ => lo,
        };
        check(v, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn fill_and_close_prices_use_the_side_that_is_against_the_trader() {
    const NAME: &str = "fill_price_e12 / close_price_e12";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |mid: u64, h: u32, t: &mut Tally| {
        for long in [true, false] {
            let f = guarded(NAME, &(mid, h, long), || fill_price_e12(mid, h, long));
            let c = guarded(NAME, &(mid, h, long), || close_price_e12(mid, h, long));
            // a long fill buys at the ask, a short fill sells at the bid; a long close sells at the bid, a short close buys at the ask
            let want_f = if long { ask_from_mid(mid, h) } else { bid_from_mid(mid, h) };
            let want_c = if long { bid_from_mid(mid, h) } else { ask_from_mid(mid, h) };
            assert_eq!(f.map_err(|e| e), want_f.map_err(ClaimError::Engine), "fill_price_e12({mid},{h},{long})");
            assert_eq!(c.map_err(|e| e), want_c.map_err(ClaimError::Engine), "close_price_e12({mid},{h},{long})");
            if let (Ok(f), Ok(c)) = (f, c) {
                // opening and closing the same side at the same mid can never gain: you buy at ask >= mid and sell at bid <= mid
                if long {
                    assert!(c <= mid && mid <= f, "long round trip would gain: fill {f} close {c} mid {mid}");
                } else {
                    assert!(f <= mid && mid <= c, "short round trip would gain: fill {f} close {c} mid {mid}");
                }
                t.ok += 1;
                t.rounded += u64::from(U256::from_u64(mid).mul_u64(E6 + u64::from(h)).divrem_u64(E6).1 != 0);
            } else {
                t.err += 1;
            }
        }
    };
    for &m in &E64 {
        for &h in &E32 {
            check(m, h, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let mid = if rng.chance(700_000) { log_u64(&mut rng, 62) } else { g64(&mut rng) };
        let h = if rng.chance(800_000) { rng.range(0, 3_000) as u32 } else { g32(&mut rng) };
        check(mid, h, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

// ================================================================================================================ NOTIONAL (LIMITED: truncates)

#[test]
fn notional_micro_truncates_toward_zero_and_never_wraps() {
    const NAME: &str = "notional_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |q: u64, p: u64, t: &mut Tally| {
        let got = guarded(NAME, &(q, p), || notional_micro(q, p));
        let prod = u128::from(q) * u128::from(p);
        if prod >= 1u128 << 127 {
            // LIMITED: the product is formed in i128, so a product at or above 2^127 is Overflow even though the quotient would fit.
            assert_eq!(got, Err(EngineError::Overflow), "notional_micro({q},{p})");
            t.err += 1;
        } else {
            let want = (prod / u128::from(DEN_U64)) as i128;
            assert_eq!(got, Ok(want), "notional_micro({q},{p})");
            // pinned direction: the result is the FLOOR of the exact notional (never above it, within 1 micro-USD below it)
            assert!(want as u128 * u128::from(DEN_U64) <= prod && prod < (want as u128 + 1) * u128::from(DEN_U64));
            t.ok += 1;
            t.rounded += u64::from(prod % u128::from(DEN_U64) != 0);
        }
    };
    assert_eq!(DEN, 1_000_000_000_000);
    for &q in &E64 {
        for &p in &E64 {
            check(q, p, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let (q, p) = (if rng.chance(700_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) }, if rng.chance(700_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) });
        check(q, p, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

// ================================================================================================================ MARGIN AND FEES

#[test]
fn margin_micro_rounds_up_with_one_ceiling() {
    const NAME: &str = "margin_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |q: u64, p: u64, lev: u16, t: &mut Tally| {
        let got = guarded(NAME, &(q, p, lev), || margin_micro(q, p, lev));
        if lev == 0 {
            assert_eq!(got, Err(EngineError::InvalidInput), "zero leverage");
            t.err += 1;
            return;
        }
        let num = U256::from_u128(u128::from(q) * u128::from(p));
        check_ceil_i64("margin_micro", &(q, p, lev), got, num, DEN_U64 * u64::from(lev), t);
    };
    for &q in &E64 {
        for &p in &E64 {
            for &lev in &[0u16, 1, 2, 50, u16::MAX] {
                check(q, p, lev, &mut t);
                t.grid += 1;
            }
        }
    }
    for _ in 0..CASES {
        let (q, p) = if rng.chance(700_000) { (log_u64(&mut rng, 52), log_u64(&mut rng, 52)) } else { (g64(&mut rng), g64(&mut rng)) };
        let lev = glev(&mut rng);
        check(q, p, lev, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn margin_never_beats_the_two_step_ceiling_and_equals_the_single_ceiling() {
    // one ceiling over the whole expression is <= ceil(ceil(notional)/lev) and >= the exact value: the evaluator and the program never use a weaker form
    let mut rng = rng_for("margin_vs_two_step");
    let mut n = 0u64;
    for _ in 0..CASES {
        let (q, p) = (log_u64(&mut rng, 50), log_u64(&mut rng, 50));
        let lev = rng.pick(&[1u16, 2, 5, 10, 20, 50]);
        let Ok(single) = margin_micro(q, p, lev) else { continue };
        let notional_ceil = margin_micro(q, p, 1).expect("lev 1 fits when lev>1 fits");
        let two_step = (i128::from(notional_ceil) + i128::from(lev) - 1) / i128::from(lev);
        assert!(i128::from(single) <= two_step, "single ceiling above the two-step ceiling for ({q},{p},{lev})");
        assert!(i128::from(single) * i128::from(lev) * i128::from(DEN_U64 as i64) >= i128::try_from(u128::from(q) * u128::from(p)).unwrap_or(i128::MAX));
        n += 1;
    }
    assert!(n > 50_000);
}

#[test]
fn fee_ceil_micro_rounds_up() {
    const NAME: &str = "fee_ceil_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |n: i64, rate: u32, t: &mut Tally| {
        let got = guarded(NAME, &(n, rate), || fee_ceil_micro(n, rate));
        if n < 0 {
            assert_eq!(got, Err(EngineError::InvalidInput), "negative notional");
            t.err += 1;
            return;
        }
        let num = U256::from_u128(n as u128 * u128::from(rate));
        check_ceil_i64("fee_ceil_micro", &(n, rate), got, num, E6, t);
    };
    for &n in &EI64 {
        for &r in &E32 {
            check(n, r, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let n = if rng.chance(800_000) { (log_u64(&mut rng, 62)) as i64 } else { gi64(&mut rng) };
        let rate = if rng.chance(700_000) { rng.range(0, 3_000) as u32 } else { g32(&mut rng) };
        check(n, rate, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn holding_fee_micro_rounds_up_and_nights_zero_overflow_is_pinned() {
    const NAME: &str = "holding_fee_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let mut spurious = 0u64;
    let check = |q: u64, mark: u64, rate: u32, nights: u32, t: &mut Tally, spurious: &mut u64| {
        let got = guarded(NAME, &(q, mark, rate, nights), || holding_fee_micro(q, mark, rate, nights));
        // exact numerator q * mark * rate * nights, up to 2^192
        let qpr = U256::from_u128(u128::from(q) * u128::from(mark)).mul_u64(u64::from(rate));
        let num = qpr.mul_u64(u64::from(nights));
        let den = DEN_U64 * E6; // 1e18, fits u64
        let (c, rounded) = num.ceil_div(den);
        // the engine multiplies in u128 left to right: q*mark, then *rate, then *nights; it overflows when an intermediate passes 2^128
        let chain_overflows = qpr.to_u128().is_none() || num.to_u128().is_none();
        match (c.to_i64(), chain_overflows) {
            (Some(want), false) => {
                assert_eq!(got, Ok(want), "holding_fee_micro({q},{mark},{rate},{nights})");
                assert!(U256::from_u64(want as u64).mul_u64(den) >= num, "holding fee {want} below the exact fee for ({q},{mark},{rate},{nights})");
                if want > 0 {
                    assert!(U256::from_u64((want - 1) as u64).mul_u64(den) < num, "holding fee {want} not minimal");
                }
                t.ok += 1;
                t.rounded += u64::from(rounded);
            }
            (None, _) => {
                assert_eq!(got, Err(EngineError::Overflow), "exact fee does not fit i64 for ({q},{mark},{rate},{nights})");
                t.err += 1;
            }
            (Some(exact), true) => {
                // LIMITED: the u128 chain overflowed although the exact fee fits. Only possible when `nights == 0` (otherwise the exact fee is
                // at least 2^128/1e18 > i64::MAX). The engine answers Overflow instead of 0: conservative, never a wrapped or a smaller fee.
                assert_eq!(got, Err(EngineError::Overflow));
                assert_eq!((exact, nights), (0, 0), "a spurious Overflow is only allowed for nights == 0 (exact fee 0)");
                *spurious += 1;
                t.err += 1;
            }
        }
    };
    for &q in &E64 {
        for &m in &E64 {
            for &r in &[0u32, 1, 100, 1_600, u32::MAX] {
                for &n in &[0u32, 1, 3, u32::MAX] {
                    check(q, m, r, n, &mut t, &mut spurious);
                    t.grid += 1;
                }
            }
        }
    }
    for _ in 0..CASES {
        let (q, m) = if rng.chance(700_000) { (log_u64(&mut rng, 52), log_u64(&mut rng, 52)) } else { (g64(&mut rng), g64(&mut rng)) };
        let rate = if rng.chance(700_000) { rng.pick(&[0u32, 100, 200, 400, 1_000, 1_600]) } else { g32(&mut rng) };
        let nights = if rng.chance(700_000) { rng.range(0, 5) as u32 } else { g32(&mut rng) };
        check(q, m, rate, nights, &mut t, &mut spurious);
        t.random += 1;
    }
    t.finish(NAME);
    println!("PINNED holding_fee_micro: {spurious} cases returned Overflow for an exact fee of 0 (nights == 0 with q*mark*rate >= 2^128)");
    // the pinned example: an absurd notional charged for zero nights
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, u32::MAX, 0), Err(EngineError::Overflow));
    // and the neighbours that must stay Ok: zero nights, ordinary notional; zero rate, absurd notional
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, u32::MAX, 0).is_err(), true);
    assert_eq!(holding_fee_micro(1_000_000_000_000, 1_000_000_000_000, 100, 0), Ok(0));
    assert_eq!(holding_fee_micro(u64::MAX, u64::MAX, 0, u32::MAX), Ok(0));
    // DESIGN 9.3: 0.01 % of 1,000,000 EUR at 1.0805 for one night
    assert_eq!(holding_fee_micro(1_000_000_000_000, 1_080_500_000_000, 100, 1), Ok(108_050_000));
}

#[test]
fn swap_fee_micro_rounds_up() {
    const NAME: &str = "swap_fee_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |cost: u64, bps: u16, t: &mut Tally| {
        let got = guarded(NAME, &(cost, bps), || swap_fee_micro(cost, bps));
        let num = U256::from_u128(u128::from(cost) * u128::from(bps));
        let (c, rounded) = num.ceil_div(10_000);
        match c.to_u64() {
            Some(want) => {
                assert_eq!(got, Ok(want), "swap_fee_micro({cost},{bps})");
                assert!(U256::from_u64(want).mul_u64(10_000) >= num, "fee {want} below the exact fee");
                if want > 0 {
                    assert!(U256::from_u64(want - 1).mul_u64(10_000) < num);
                }
                t.ok += 1;
                t.rounded += u64::from(rounded);
            }
            None => {
                assert_eq!(got, Err(EngineError::Overflow), "swap_fee_micro({cost},{bps}) does not fit u64");
                t.err += 1;
            }
        }
    };
    for &c in &E64 {
        for &b in &E16 {
            check(c, b, &mut t);
            t.grid += 1;
        }
    }
    // the launch tier costs of DESIGN 9.2 are in the generator: $30 ... $1,599 at 40 bps
    for cost_usd in [30u64, 55, 99, 145, 189, 235, 489, 689, 859, 1_599] {
        check(cost_usd * 1_000_000, 40, &mut t);
        t.grid += 1;
    }
    assert_eq!(swap_fee_micro(99_000_000, 40), Ok(396_000));
    assert_eq!(swap_fee_micro(30_000_000, 40), Ok(120_000));
    for _ in 0..CASES {
        let cost = if rng.chance(700_000) { log_u64(&mut rng, 40) } else { g64(&mut rng) };
        let bps = if rng.chance(600_000) { rng.range(0, 1_000) as u16 } else { g16(&mut rng) };
        check(cost, bps, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn commission_double_ceiling_of_the_evaluator_is_never_below_the_exact_single_ceiling() {
    // LIMITED: the evaluator's commission (decide.rs `commission_micro`) is fee_ceil_micro(margin_micro(q, p, 1), rate): the notional is
    // rounded UP first, then the fee is rounded UP. That is >= the single exact ceiling and exceeds it by at most ceil(rate/1e6) micro-USD.
    // Against the trader (or equal), never in his favour. Pinned so a later change to a floor breaks this test.
    const NAME: &str = "commission_double_ceiling";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let mut strictly_above = 0u64;
    for _ in 0..CASES {
        let (q, p) = if rng.chance(800_000) { (log_u64(&mut rng, 52), log_u64(&mut rng, 52)) } else { (g64(&mut rng), g64(&mut rng)) };
        let rate = if rng.chance(800_000) { rng.range(0, 2_000) as u32 } else { g32(&mut rng) };
        let got = guarded(NAME, &(q, p, rate), || margin_micro(q, p, 1).and_then(|n| fee_ceil_micro(n, rate)));
        let num = U256::from_u128(u128::from(q) * u128::from(p)).mul_u64(u64::from(rate));
        let (single, rounded) = num.ceil_div(DEN_U64 * E6);
        match (got, single.to_i64()) {
            (Ok(double), Some(single)) => {
                assert!(double >= single, "the double ceiling is BELOW the exact commission for ({q},{p},{rate}): {double} < {single}");
                let slack = (u64::from(rate) + E6 - 1) / E6;
                assert!(i128::from(double) - i128::from(single) <= i128::from(slack), "double ceiling exceeds the single by more than {slack} for ({q},{p},{rate})");
                strictly_above += u64::from(double > single);
                t.ok += 1;
                t.rounded += u64::from(rounded);
            }
            (Err(EngineError::Overflow), _) => t.err += 1,
            (Ok(_), None) => panic!("an Ok commission although the exact commission does not fit i64: ({q},{p},{rate})"),
            (Err(e), _) => panic!("unexpected error {e:?} for ({q},{p},{rate})"),
        }
        t.random += 1;
    }
    println!("PINNED commission: the double ceiling exceeded the single exact ceiling in {strictly_above} of {CASES} cases (by at most ceil(rate/1e6) micro-USD)");
    assert!(strictly_above > 0, "the pin is vacuous: the double ceiling never differed");
    t.finish(NAME);
}

// ================================================================================================================ P&L

/// Reference `floor(sign x qty x (exit - entry) / 1e12)`: `Some((value, rounded))` if it fits i64.
fn ref_pnl(long: bool, qty: u64, entry: u64, exit: u64) -> Option<(i64, bool)> {
    let diff = i128::from(exit) - i128::from(entry);
    let negative = if long { diff < 0 } else { diff > 0 };
    let mag = u128::from(qty) * diff.unsigned_abs();
    let (q, r) = (mag / u128::from(DEN_U64), mag % u128::from(DEN_U64));
    if !negative {
        i64::try_from(q).ok().map(|v| (v, r != 0))
    } else {
        let m = q + u128::from(r != 0);
        if m <= 1u128 << 63 {
            Some(((-(m as i128)) as i64, r != 0))
        } else {
            None
        }
    }
}

fn check_pnl_value(what: &str, inputs: &dyn Debug, got: Result<i64, EngineError>, long: bool, qty: u64, entry: u64, exit: u64, t: &mut Tally) {
    match ref_pnl(long, qty, entry, exit) {
        Some((want, rounded)) => {
            assert_eq!(got, Ok(want), "{what}{inputs:?}");
            let diff = i128::from(exit) - i128::from(entry);
            let signed_diff = if long { diff } else { -diff };
            let mag = u128::from(qty) * diff.unsigned_abs();
            if mag < 1u128 << 126 {
                let num = if signed_diff < 0 { -(mag as i128) } else { mag as i128 };
                let den = i128::from(DEN_U64);
                // FLOOR: want * DEN <= exact < (want + 1) * DEN, so a profit is cut and a loss is enlarged, never the reverse
                assert!(i128::from(want) * den <= num && num < (i128::from(want) + 1) * den, "{what}{inputs:?}: {want} is not the floor of the exact P&L");
            }
            t.ok += 1;
            t.rounded += u64::from(rounded);
        }
        None => {
            assert_eq!(got, Err(EngineError::Overflow), "{what}{inputs:?}: exact P&L does not fit i64");
            t.err += 1;
        }
    }
}

#[test]
fn pnl_micro_floors_toward_minus_infinity() {
    const NAME: &str = "pnl_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    for &q in &E64 {
        for &a in &E64 {
            for &b in &E64 {
                for long in [true, false] {
                    let got = guarded(NAME, &(long, q, a, b), || pnl_micro(long, q, a, b));
                    check_pnl_value("pnl_micro", &(long, q, a, b), got, long, q, a, b, &mut t);
                    t.grid += 1;
                }
            }
        }
    }
    for _ in 0..CASES {
        let long = rng.chance(500_000);
        let (q, a) = if rng.chance(700_000) { (log_u64(&mut rng, 52), log_u64(&mut rng, 52)) } else { (g64(&mut rng), g64(&mut rng)) };
        // exit near entry half the time: small differences are where flooring a loss matters
        let b = match rng.range(0, 3) {
            0 => a.saturating_add(rng.range(0, 3_000) as u64),
            1 => a.saturating_sub(rng.range(0, 3_000) as u64),
            _ => if rng.chance(700_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) },
        };
        let got = guarded(NAME, &(long, q, a, b), || pnl_micro(long, q, a, b));
        check_pnl_value("pnl_micro", &(long, q, a, b), got, long, q, a, b, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn unrealized_micro_marks_longs_at_the_bid_and_shorts_at_the_ask() {
    const NAME: &str = "unrealized_micro";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |qty: i64, avg: u64, bid: u64, ask: u64, t: &mut Tally| {
        let pos = Position { qty, avg_entry_e12: avg, timer_start: 0, opened_at: 0, fee_day: 0 };
        let got = guarded(NAME, &(qty, avg, bid, ask), || unrealized_micro(&pos, bid, ask));
        if qty == 0 {
            assert_eq!(got, Err(EngineError::InvalidInput));
            t.err += 1;
            return;
        }
        let long = qty > 0;
        // the mark is the side the position would CLOSE at: long -> bid, short -> ask
        let mark = if long { bid } else { ask };
        check_pnl_value("unrealized_micro", &(qty, avg, bid, ask), got, long, qty.unsigned_abs(), avg, mark, t);
    };
    for &q in &EI64 {
        for &a in &E64 {
            for &m in &E64 {
                check(q, a, m, m.saturating_add(1), &mut t);
                t.grid += 1;
            }
        }
    }
    for _ in 0..CASES {
        let qty = if rng.chance(800_000) { let m = log_u64(&mut rng, 50) as i64; if rng.chance(500_000) { -m } else { m } } else { gi64(&mut rng) };
        let avg = if rng.chance(700_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) };
        let bid = if rng.chance(700_000) { avg.saturating_add_signed(rng.range(-3_000, 3_000)) } else { g64(&mut rng) };
        let ask = bid.saturating_add(rng.range(0, 3_000) as u64);
        check(qty, avg, bid, ask, &mut t);
        t.random += 1;
    }
    t.finish(NAME);
}

#[test]
fn open_profit_micro_sums_without_wrapping() {
    let mut rng = rng_for("open_profit_micro");
    let mut ok = 0u64;
    let mut over = 0u64;
    for _ in 0..CASES {
        let n = rng.range(0, 8) as usize;
        let list: Vec<(Position, u64, u64)> = (0..n)
            .map(|_| {
                let qty = if rng.chance(500_000) { gi64(&mut rng) } else { rng.range(-1_000_000_000_000, 1_000_000_000_000) };
                let avg = if rng.chance(500_000) { g64(&mut rng) } else { 1_000_000_000_000 + rng.range(0, 1_000_000_000) as u64 };
                let bid = if rng.chance(500_000) { g64(&mut rng) } else { 1_000_000_000_000 + rng.range(0, 1_000_000_000) as u64 };
                (Position { qty, avg_entry_e12: avg, timer_start: 0, opened_at: 0, fee_day: 0 }, bid, bid.saturating_add(100))
            })
            .collect();
        let got = guarded("open_profit_micro", &list, || open_profit_micro(&list));
        // reference: i128 sum of the per-position floors, Overflow if any position fails or the sum leaves i64
        let mut total: i128 = 0;
        let mut expect: Result<i64, EngineError> = Ok(0);
        for (p, bid, ask) in &list {
            if p.qty == 0 {
                expect = Err(EngineError::InvalidInput);
                break;
            }
            let long = p.qty > 0;
            match ref_pnl(long, p.qty.unsigned_abs(), p.avg_entry_e12, if long { *bid } else { *ask }) {
                Some((v, _)) => total += i128::from(v),
                None => {
                    expect = Err(EngineError::Overflow);
                    break;
                }
            }
            if i64::try_from(total).is_err() {
                expect = Err(EngineError::Overflow);
                break;
            }
        }
        if expect.is_ok() {
            expect = Ok(total as i64);
        }
        assert_eq!(got, expect, "open_profit_micro({list:?})");
        match got {
            Ok(_) => ok += 1,
            Err(_) => over += 1,
        }
    }
    println!("RESULT open_profit_micro: seed={SEED:#x} random_cases={CASES} ok={ok} err={over}");
    assert!(ok > 20_000 && over > 100);
}

// ================================================================================================================ CREDIT

#[test]
fn slice_credit_rounds_down_and_the_tally_never_goes_negative() {
    const NAME: &str = "slice_credit";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let check = |pnl: i64, quick: bool, t: &mut Tally| {
        let (credit, tally) = guarded(NAME, &(pnl, quick), || slice_credit(pnl, quick));
        // never more than the realized P&L: the trader is never credited above what he made
        assert!(credit <= pnl, "credit {credit} above pnl {pnl}");
        if quick && pnl > 0 {
            // floor(pnl / 2): 2*credit <= pnl < 2*credit + 2
            assert_eq!(credit, pnl / 2, "quick profit credit for pnl {pnl}");
            let d = i128::from(pnl) - 2 * i128::from(credit);
            assert!(d == 0 || d == 1, "credit {credit} is not floor(pnl/2) for {pnl}");
            t.rounded += u64::from(d == 1);
        } else {
            assert_eq!(credit, pnl, "a normal profit and every loss are credited in full ({pnl}, quick {quick})");
        }
        // the quick tally takes the credited half-profit of a profitable quick slice, nothing else; it never decreases
        assert_eq!(tally, if quick { credit.max(0) } else { 0 }, "tally for ({pnl},{quick})");
        assert!(tally >= 0);
        t.ok += 1;
    };
    for &p in &EI64 {
        for quick in [true, false] {
            check(p, quick, &mut t);
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let pnl = gi64(&mut rng);
        check(pnl, rng.chance(500_000), &mut t);
        t.random += 1;
    }
    t.err += 1; // slice_credit has no error path: it is total
    t.finish(NAME);
}

// ================================================================================================================ NETTING

/// An engine position.
fn pos(qty: i64, avg: u64, timer: i64) -> Position {
    Position { qty, avg_entry_e12: avg, timer_start: timer, opened_at: timer, fee_day: 7 }
}

/// Errors the DESIGN allows for `apply_fill` for the given inputs (None = it must succeed).
fn allowed_apply_errors(p: Option<Position>, side: Side, dq: u64, price: u64, t: i64) -> Option<Vec<EngineError>> {
    let mut errs = Vec::new();
    if dq == 0 || dq > i64::MAX as u64 {
        errs.push(EngineError::InvalidQuantity);
    }
    if price == 0 {
        errs.push(EngineError::InvalidInput);
    }
    if !errs.is_empty() {
        return Some(errs);
    }
    let signed = if side == Side::Buy { i128::from(dq) } else { -i128::from(dq) };
    let Some(p) = p else { return None };
    if p.qty == 0 {
        return Some(vec![EngineError::InvalidInput]);
    }
    let age = i128::from(t) - i128::from(p.timer_start);
    if i64::try_from(age).is_err() {
        return Some(vec![EngineError::Overflow]);
    }
    if age < 0 {
        return Some(vec![EngineError::InvalidInput]);
    }
    let long = p.qty > 0;
    if long == (side == Side::Buy) {
        let sum = i128::from(p.qty) + signed;
        if sum > i128::from(i64::MAX) || sum < -i128::from(i64::MAX) {
            return Some(vec![EngineError::Overflow]);
        }
        None
    } else {
        let c = dq.min(p.qty.unsigned_abs());
        if ref_pnl(long, c, p.avg_entry_e12, price).is_none() {
            return Some(vec![EngineError::Overflow]);
        }
        None
    }
}

/// Branch counters of the netting test.
#[derive(Default)]
struct Branches {
    open: u64,
    merge_long: u64,
    merge_short: u64,
    partial: u64,
    flat: u64,
    flip: u64,
    errors: u64,
    merge_rounded: u64,
}

#[allow(clippy::too_many_arguments)]
fn check_apply(p: Option<Position>, side: Side, dq: u64, price: u64, t: i64, day: i64, b: &mut Branches) {
    let got = guarded("apply_fill", &(p, side, dq, price, t), || apply_fill(p, side, dq, price, t, day));
    let allowed = allowed_apply_errors(p, side, dq, price, t);
    let o = match (allowed, got) {
        (Some(list), Err(e)) => {
            assert!(list.contains(&e), "apply_fill({p:?},{side:?},{dq},{price},{t}) returned {e:?}, allowed {list:?}");
            b.errors += 1;
            return;
        }
        (Some(list), Ok(o)) => panic!("apply_fill({p:?},{side:?},{dq},{price},{t}) succeeded ({o:?}) but must fail with one of {list:?}"),
        (None, Err(e)) => panic!("apply_fill({p:?},{side:?},{dq},{price},{t}) failed with {e:?} but every input is legal"),
        (None, Ok(o)) => o,
    };
    let signed = if side == Side::Buy { i128::from(dq) } else { -i128::from(dq) };
    let new_qty = o.position.map_or(0, |x| i128::from(x.qty));
    let Some(p) = p else {
        b.open += 1;
        assert_eq!(o.position, Some(Position { qty: signed as i64, avg_entry_e12: price, timer_start: t, opened_at: t, fee_day: day }));
        assert_eq!((o.closed, o.balance_delta, o.quick_tally_delta), (None, 0, 0));
        assert_eq!(o.opened.map(|s| (i128::from(s.qty), s.price_e12)), Some((signed, price)));
        return;
    };
    // quantity is conserved exactly, in every branch
    assert_eq!(new_qty, i128::from(p.qty) + signed, "quantity not conserved for {p:?} {side:?} {dq}");
    assert!(new_qty.unsigned_abs() <= u128::from(i64::MAX as u64), "a position beyond i64::MAX");
    let long = p.qty > 0;
    let q_abs = p.qty.unsigned_abs();
    if long == (side == Side::Buy) {
        // MERGE: long rounds the average UP, short DOWN
        let num = U256::from_u128(u128::from(q_abs) * u128::from(p.avg_entry_e12) + u128::from(dq) * u128::from(price));
        let den = q_abs + dq;
        let (want, rounded) = if long { num.ceil_div(den) } else { num.floor_div(den) };
        let want = want.to_u64().expect("an average of two u64 prices fits u64");
        let got = o.position.expect("a merge leaves a position");
        assert_eq!(got.avg_entry_e12, want, "average entry for {p:?} {side:?} {dq} @ {price}");
        // direction: long: avg*den >= num and (avg-1)*den < num; short: avg*den <= num < (avg+1)*den
        let cost = U256::from_u64(want).mul_u64(den);
        if long {
            assert!(cost >= num, "long average {want} is BELOW the exact weighted mean (favours the trader)");
            assert!(want == 0 || U256::from_u64(want - 1).mul_u64(den) < num);
            b.merge_long += 1;
        } else {
            assert!(cost <= num, "short average {want} is ABOVE the exact weighted mean (favours the trader)");
            assert!(U256::from_u64(want).add_u64(1).mul_u64(den) > num);
            b.merge_short += 1;
        }
        b.merge_rounded += u64::from(rounded);
        let (lo, hi) = (p.avg_entry_e12.min(price), p.avg_entry_e12.max(price));
        assert!(want >= lo && want <= hi, "average {want} outside [{lo},{hi}]");
        assert_eq!((got.qty, got.timer_start, got.opened_at, got.fee_day), (new_qty as i64, t, p.opened_at, p.fee_day));
        assert_eq!((o.closed, o.balance_delta, o.quick_tally_delta), (None, 0, 0));
        assert_eq!(o.opened.map(|s| (i128::from(s.qty), s.price_e12)), Some((signed, price)));
        return;
    }
    // OPPOSITE DIRECTION: closing slice c = min(dq, |q|)
    let c = dq.min(q_abs);
    let (pnl, _) = ref_pnl(long, c, p.avg_entry_e12, price).expect("checked by allowed_apply_errors");
    let age = t - p.timer_start;
    let quick = age <= QUICK_SECS;
    let credit = if quick && pnl > 0 { pnl / 2 } else { pnl };
    let tally = if quick { credit.max(0) } else { 0 };
    let cs = o.closed.expect("an opposite fill closes a slice");
    assert_eq!((cs.qty_abs, cs.entry_e12, cs.exit_e12, cs.pnl, cs.quick, cs.credit), (c, p.avg_entry_e12, price, pnl, quick, credit));
    assert_eq!((o.balance_delta, o.quick_tally_delta), (credit, tally));
    assert!(o.balance_delta <= cs.pnl, "credited more than the realized P&L");
    if dq < q_abs {
        b.partial += 1;
        assert_eq!(o.position, Some(Position { qty: p.qty + signed as i64, ..p }), "a partial reduce changes only the quantity");
        assert_eq!(o.opened, None);
    } else if dq == q_abs {
        b.flat += 1;
        assert_eq!((o.position, o.opened), (None, None));
    } else {
        b.flip += 1;
        let rest = dq - q_abs;
        let rest_signed = if long { -(rest as i64) } else { rest as i64 };
        assert_eq!(o.position, Some(Position { qty: rest_signed, avg_entry_e12: price, timer_start: t, opened_at: t, fee_day: day }));
        assert_eq!(o.opened.map(|s| (s.qty, s.price_e12)), Some((rest_signed, price)));
    }
}

fn gen_position(rng: &mut Rng, t: i64) -> Option<Position> {
    if rng.chance(150_000) {
        return None;
    }
    let qty = match rng.range(0, 9) {
        0 => 0,
        1 => rng.pick(&[i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1, -1, 1]),
        2 | 3 => rng.range(-5_000, 5_000),
        4..=7 => {
            let m = log_u64(rng, 55) as i64;
            if rng.chance(500_000) {
                -m
            } else {
                m
            }
        }
        _ => gi64(rng),
    };
    let avg = if rng.chance(700_000) { log_u64(rng, 52) } else { g64(rng) };
    let timer = match rng.range(0, 9) {
        0 => rng.pick(&[i64::MIN, i64::MAX, 0, t, t.saturating_add(1)]),
        _ => t.saturating_sub(rng.pick(&[0i64, 1, 44, 45, 46, 100, 3_600, 86_400, 31_536_000, -1, -1_000])),
    };
    Some(pos(qty, avg, timer))
}

#[test]
fn apply_fill_every_branch_matches_the_reference_and_rounds_against_the_trader() {
    const NAME: &str = "apply_fill";
    let mut rng = rng_for(NAME);
    let mut b = Branches::default();
    // boundary grid: extreme positions x extreme fills x both sides
    let ts = [1_700_000_000i64, i64::MAX, i64::MIN];
    let mut grid = 0u64;
    for &q in &[i64::MIN, i64::MIN + 1, -1, 0, 1, 2, 1_000_000, i64::MAX - 1, i64::MAX] {
        for &a in &[0u64, 1, 1_000_000_000_000, u64::MAX] {
            for &dq in &[0u64, 1, 2, i64::MAX as u64 - 1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
                for &px in &[0u64, 1, u64::MAX] {
                    for &t in &ts {
                        for side in [Side::Buy, Side::Sell] {
                            for timer in [t.saturating_sub(10), 0, i64::MIN, i64::MAX] {
                                check_apply(Some(pos(q, a, timer)), side, dq, px, t, 9, &mut b);
                                grid += 1;
                            }
                            check_apply(None, side, dq, px, t, 9, &mut b);
                            grid += 1;
                        }
                    }
                }
            }
        }
    }
    for _ in 0..CASES {
        let t = if rng.chance(100_000) { rng.pick(&ts) } else { 1_700_000_000 + rng.range(0, 100_000_000) };
        let p = gen_position(&mut rng, t);
        let side = if rng.chance(500_000) { Side::Buy } else { Side::Sell };
        let q_abs = p.map_or(1, |x| x.qty.unsigned_abs());
        let dq = match rng.range(0, 7) {
            0 => q_abs,
            1 => q_abs.wrapping_sub(1),
            2 => q_abs.wrapping_add(1),
            3 => rng.range(0, 3) as u64,
            4 => g64(&mut rng),
            _ => log_u64(&mut rng, 55),
        };
        let price = if rng.chance(800_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) };
        // same-direction merges are what the average rounding is about: make half of the fills go the position's way
        let side = match p {
            Some(x) if x.qty != 0 && rng.chance(500_000) => if x.qty > 0 { Side::Buy } else { Side::Sell },
            _ => side,
        };
        check_apply(p, side, dq, price, t, 9, &mut b);
    }
    println!(
        "RESULT {NAME}: seed={SEED:#x} random_cases={CASES} grid_cases={grid} open={} merge_long={} merge_short={} partial={} flat={} flip={} errors={} merges_that_rounded={}",
        b.open, b.merge_long, b.merge_short, b.partial, b.flat, b.flip, b.errors, b.merge_rounded
    );
    for (n, v) in [("open", b.open), ("merge_long", b.merge_long), ("merge_short", b.merge_short), ("partial", b.partial), ("flat", b.flat), ("flip", b.flip), ("errors", b.errors)] {
        assert!(v >= 1_000, "branch {n} reached only {v} times: the generator does not cover it");
    }
    assert!(b.merge_rounded >= 10_000, "only {} merges rounded", b.merge_rounded);
}

#[test]
fn apply_fill_merge_error_paths_return_errors_never_wrap() {
    // exact, hand-written boundaries of the checked arithmetic of the merge/flip/age paths
    let big = i64::MAX;
    // merged quantity beyond i64::MAX in magnitude (long and short), and exactly i64::MIN (refused: symmetry)
    assert_eq!(apply_fill(Some(pos(big, 5, 0)), Side::Buy, 1, 5, 10, 1), Err(EngineError::Overflow));
    assert_eq!(apply_fill(Some(pos(-big, 5, 0)), Side::Sell, 1, 5, 10, 1), Err(EngineError::Overflow), "a short may not reach i64::MIN");
    assert!(apply_fill(Some(pos(big - 1, 5, 0)), Side::Buy, 1, 5, 10, 1).is_ok());
    assert!(apply_fill(Some(pos(-(big - 1), 5, 0)), Side::Sell, 1, 5, 10, 1).is_ok());
    // dq above i64::MAX is InvalidQuantity, not a wrapped negative
    assert_eq!(apply_fill(None, Side::Buy, big as u64 + 1, 5, 0, 0), Err(EngineError::InvalidQuantity));
    assert_eq!(apply_fill(None, Side::Sell, u64::MAX, 5, 0, 0), Err(EngineError::InvalidQuantity));
    assert_eq!(apply_fill(None, Side::Buy, 0, 5, 0, 0), Err(EngineError::InvalidQuantity));
    assert_eq!(apply_fill(None, Side::Buy, 1, 0, 0, 0), Err(EngineError::InvalidInput));
    // a stored zero-quantity position is refused, time never runs backwards, and an age beyond i64 is Overflow
    assert_eq!(apply_fill(Some(pos(0, 5, 0)), Side::Buy, 1, 5, 0, 0), Err(EngineError::InvalidInput));
    assert_eq!(apply_fill(Some(pos(1, 5, 100)), Side::Buy, 1, 5, 99, 0), Err(EngineError::InvalidInput));
    assert_eq!(apply_fill(Some(pos(1, 5, i64::MIN)), Side::Buy, 1, 5, i64::MAX, 0), Err(EngineError::Overflow));
    // a huge age is fine: not quick, full credit
    let o = apply_fill(Some(pos(10, 1_000_000_000_000, 0)), Side::Sell, 10, 2_000_000_000_000, i64::MAX, 3).unwrap();
    assert_eq!(o.closed.map(|c| (c.quick, c.credit, c.pnl)), Some((false, 10, 10)));
    // pnl beyond i64 on a close is Overflow, the position is NOT returned half-applied
    assert_eq!(apply_fill(Some(pos(big, 1, 0)), Side::Sell, big as u64, u64::MAX, 100, 1), Err(EngineError::Overflow));
    // a flip with the largest remainder: long 1 flips to short i64::MAX - 1... and the largest legal flip
    let o = apply_fill(Some(pos(1, 1_000, 0)), Side::Sell, big as u64, 1_000, 100, 1).unwrap();
    assert_eq!(o.position.map(|p| p.qty), Some(-(big - 1)));
    assert_eq!(o.opened.map(|s| s.qty), Some(-(big - 1)));
}

// ----------------------------------------------------------------------------------------------- end to end: cost basis vs exact rational

/// Signed magnitude in U256, for exact comparisons of rational P&L.
#[derive(Clone, Copy, Debug)]
struct Signed {
    neg: bool,
    mag: U256,
}

fn signed_le(a: Signed, b: Signed) -> bool {
    let a_zero = a.mag.is_zero();
    let b_zero = b.mag.is_zero();
    match (a.neg && !a_zero, b.neg && !b_zero) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => a.mag <= b.mag,
        (true, true) => a.mag >= b.mag,
    }
}

#[test]
fn a_position_built_by_merges_never_realizes_more_than_the_exact_rational_pnl() {
    // End to end through the real `apply_fill`: 1 to 6 same-direction fills (every merge rounds the average: long up, short down), then a
    // partial or full close at a random exit. The engine's realized P&L must be <= floor of the EXACT rational P&L of the same fills,
    // computed with the cost basis C = sum(q_i x p_i), Q = sum(q_i): exact = c x (x*Q - C) / (Q x DEN) for a long, the negative of that for a short.
    // I.e. the tiny rounding of the average can only ever cost the trader, however the merges are ordered.
    const NAME: &str = "merge_chain_pnl";
    let mut rng = rng_for(NAME);
    let (mut longs, mut shorts, mut strictly_below) = (0u64, 0u64, 0u64);
    for _ in 0..CASES {
        let long = rng.chance(500_000);
        let n = rng.range(1, 6) as usize;
        let mut fills = Vec::new();
        let mut qsum: u128 = 0;
        for _ in 0..n {
            let q = match rng.range(0, 3) {
                0 => rng.range(1, 3) as u64,
                1 => rng.range(1, 1_000_000) as u64,
                _ => 1 + log_u64(&mut rng, 48),
            };
            let p = match rng.range(0, 3) {
                0 => rng.range(1, 5) as u64,
                1 => (1_000_000_000_000i64 + rng.range(-1_000_000, 1_000_000)) as u64,
                _ => 1 + log_u64(&mut rng, 52),
            };
            fills.push((q, p));
            qsum += u128::from(q);
        }
        let side_open = if long { Side::Buy } else { Side::Sell };
        let side_close = if long { Side::Sell } else { Side::Buy };
        let mut position = None;
        let mut t = 1_000i64;
        for &(q, p) in &fills {
            position = apply_fill(position, side_open, q, p, t, 1).expect("merge").position;
            t += 100;
        }
        let q_total = u64::try_from(qsum).expect("small");
        let c = match rng.range(0, 3) {
            0 => q_total,
            1 => 1 + (rng.next_u64() % q_total),
            _ => q_total.saturating_sub(1).max(1),
        };
        let exit = match rng.range(0, 3) {
            0 => fills[0].1.saturating_add(rng.range(0, 2_000) as u64),
            1 => fills[0].1.saturating_sub(rng.range(0, 2_000) as u64).max(1),
            _ => 1 + log_u64(&mut rng, 52),
        };
        let o = apply_fill(position, side_close, c, exit, t + 1_000, 1);
        let Ok(o) = o else {
            // the only legal failure is a P&L beyond i64
            assert_eq!(o, Err(EngineError::Overflow));
            continue;
        };
        let engine_pnl = o.closed.expect("closing slice").pnl;
        // exact: cost basis in e12 x units
        let cost: u128 = fills.iter().map(|&(q, p)| u128::from(q) * u128::from(p)).sum();
        let x_q = u128::from(exit) * qsum; // x * Q
        let (neg_long, diff) = if x_q >= cost { (false, x_q - cost) } else { (true, cost - x_q) };
        // numerator N = c * (x*Q - C), sign flipped for a short; exact pnl = N / (Q * DEN)
        let n_mag = U256::from_u128(diff).mul_u64(c);
        let n = Signed { neg: if long { neg_long } else { !neg_long }, mag: n_mag };
        let q_den = U256::from_u64(DEN_U64).mul_u64(q_total); // Q * DEN fits U256
        let engine_scaled = Signed { neg: engine_pnl < 0, mag: q_den_mul(q_den, engine_pnl.unsigned_abs()) };
        assert!(
            signed_le(engine_scaled, n),
            "FINDING: engine realized {engine_pnl} > exact rational P&L ({}, long={long}, fills={fills:?}, close {c} @ {exit})",
            if n.neg { "negative" } else { "non-negative" }
        );
        if long {
            longs += 1;
        } else {
            shorts += 1;
        }
        // is the engine strictly below the exact rational value (rounding actually cost something)?
        strictly_below += u64::from(!signed_le(n, engine_scaled));
    }
    println!("RESULT {NAME}: seed={SEED:#x} random_cases={CASES} longs={longs} shorts={shorts} strictly_below_exact={strictly_below}");
    assert!(longs >= 20_000 && shorts >= 20_000);
    assert!(strictly_below >= 10_000, "rounding never cost anything: the test is vacuous");
}

/// `q_den * m` for a U256 `q_den` and a u64 `m`.
fn q_den_mul(q_den: U256, m: u64) -> U256 {
    q_den.mul_u64(m)
}

// ================================================================================================================ FLOORS AND LIMITS (never round)

fn ref_frac(size: i64, bps: i64) -> Result<i64, EngineError> {
    if size <= 0 {
        return Err(EngineError::InvalidInput);
    }
    let n = i128::from(size) * i128::from(bps);
    if n % 10_000 != 0 {
        return Err(EngineError::InvalidInput);
    }
    i64::try_from(n / 10_000).map_err(|_| EngineError::Overflow)
}

#[test]
fn frac_exact_never_rounds_it_is_exact_or_invalid_input() {
    const NAME: &str = "frac_exact";
    let mut rng = rng_for(NAME);
    let mut t = Tally::default();
    let all_bps = [STATIC_FLOOR_BPS, LATCHED_FLOOR_BPS, LATCH_TRIGGER_BPS, TARGET_BPS, PAYOUT_BPS, TIGHT_START_BPS, TIGHT_OFFSET_BPS, QUICK_LIMIT_BPS, RATCHET_SLACK_BPS];
    let check = |size: i64, bps: i64, t: &mut Tally| {
        let got = guarded(NAME, &(size, bps), || frac_exact(size, bps));
        let want = ref_frac(size, bps);
        assert_eq!(got, want, "frac_exact({size},{bps})");
        match got {
            Ok(v) => {
                // exact: no remainder was dropped
                assert_eq!(i128::from(v) * 10_000, i128::from(size) * i128::from(bps));
                t.ok += 1;
            }
            Err(EngineError::InvalidInput) => {
                t.rounded += 1; // an inexact division is REFUSED instead of rounded
                t.err += 1;
            }
            Err(_) => t.err += 1,
        }
    };
    for &s in &EI64 {
        for &b in &all_bps {
            check(s, b, &mut t);
            t.grid += 1;
        }
    }
    for &s in &SIZES {
        for &b in &all_bps {
            // every tier, and S = 1 USD, are exact for every threshold of DESIGN 2
            let v = frac_exact(s, b).unwrap_or_else(|e| panic!("tier {s} bps {b}: {e:?}"));
            assert_eq!(i128::from(v), i128::from(s) * i128::from(b) / 10_000);
            assert_eq!(i128::from(v) * 10_000, i128::from(s) * i128::from(b), "inexact threshold for size {s}, bps {b}");
            t.grid += 1;
        }
    }
    for _ in 0..CASES {
        let size = match rng.range(0, 7) {
            0 | 1 => rng.pick(&SIZES),
            2 | 3 => 1_000_000 * (1 + log_u64(&mut rng, 22) as i64),
            4 => gi64(&mut rng),
            5 => 1_000_000 * (1 + log_u64(&mut rng, 22) as i64) + rng.range(-3, 3),
            _ => (rng.range(0, 400_000_000)) * 100,
        };
        let bps = if rng.chance(700_000) { rng.pick(&all_bps) } else { rng.range(-100, 40_000) };
        check(size, bps, &mut t);
        t.random += 1;
    }
    // the thresholds are consecutive and exact for every whole-dollar size: F_s < F_latched < latch < target, payout = target - S
    for k in 1..=300_000i64 {
        let s = k * 1_000_000;
        let (f, fl, lt, tg, pay) = (
            frac_exact(s, STATIC_FLOOR_BPS).unwrap(),
            frac_exact(s, LATCHED_FLOOR_BPS).unwrap(),
            frac_exact(s, LATCH_TRIGGER_BPS).unwrap(),
            frac_exact(s, TARGET_BPS).unwrap(),
            payout_micro(s).unwrap(),
        );
        assert!(f < fl && fl < lt && lt < tg && tg < s + s);
        assert_eq!(tg - s, pay, "payout is exactly target minus S for S = {k} USD");
        assert_eq!((f, fl, lt, tg, pay), (92 * k * 10_000, 96 * k * 10_000, 104 * k * 10_000, 108 * k * 10_000, 8 * k * 10_000));
    }
    t.ok += 0;
    println!("RESULT {NAME}: seed={SEED:#x} random_cases={} grid_cases={} ok={} err={} refused_inexact={}; whole-dollar sizes 1..=300000 USD checked exhaustively", t.random, t.grid, t.ok, t.err, t.rounded);
    assert!(t.random >= u64::from(CASES) && t.ok >= 20_000 && t.rounded >= 1_000);
}

#[test]
fn static_floor_effective_floor_and_payout_follow_frac_exact() {
    let mut rng = rng_for("floors");
    for _ in 0..CASES {
        let size = if rng.chance(500_000) { rng.pick(&SIZES) } else { 1_000_000 * (1 + log_u64(&mut rng, 23) as i64) + if rng.chance(100_000) { 1 } else { 0 } };
        let mut st = AccountState::new_phase(size);
        st.latch = rng.chance(500_000);
        st.tight_on = rng.chance(500_000);
        st.l_level = gi64(&mut rng);
        let got = guarded("static_floor_micro", &st, || static_floor_micro(&st));
        assert_eq!(got, ref_frac(size, if st.latch { LATCHED_FLOOR_BPS } else { STATIC_FLOOR_BPS }));
        let eff = guarded("effective_floor_micro", &st, || effective_floor_micro(&st));
        match (got, eff) {
            (Ok(f), Ok(e)) => assert_eq!(e, if st.tight_on { f.max(st.l_level) } else { f }, "effective floor is max(F_s, L) only in tight mode"),
            (Err(a), Err(b)) => assert_eq!(a, b),
            other => panic!("floors disagree: {other:?}"),
        }
        let pay = guarded("payout_micro", &size, || payout_micro(size));
        assert_eq!(pay, ref_frac(size, PAYOUT_BPS));
        // reset_phase_state never touches the size, and a fresh phase starts at B = S flat with everything off
        let fresh = reset_phase_state(size);
        assert_eq!((fresh.size_micro, fresh.b, fresh.latch, fresh.tight_on, fresh.q_tally, fresh.lock_until, fresh.flat), (size, size, false, false, 0, 0, true));
    }
    // 1 USD and the largest tier, spelled out
    assert_eq!(static_floor_micro(&AccountState::new_phase(1_000_000)), Ok(920_000));
    assert_eq!(payout_micro(1_000_000), Ok(80_000));
    assert_eq!(payout_micro(200_000_000_000), Ok(16_000_000_000));
    assert_eq!(static_floor_micro(&AccountState::new_phase(200_000_000_000)), Ok(184_000_000_000));
    // a size that does not fit after scaling is Overflow, not a wrapped floor
    assert_eq!(payout_micro(i64::MAX), Err(EngineError::InvalidInput)); // i64::MAX is not a whole number of dollars
    assert_eq!(payout_micro(i64::MAX - (i64::MAX % 1_000_000)), Ok((i64::MAX - (i64::MAX % 1_000_000)) / 10_000 * 800));
    assert_eq!(frac_exact(i64::MAX - (i64::MAX % 10_000), TARGET_BPS), Err(EngineError::Overflow));
    assert_eq!(payout_micro(0), Err(EngineError::InvalidInput));
    assert_eq!(payout_micro(-1_000_000), Err(EngineError::InvalidInput));
}

#[test]
fn next_phase_and_the_pass_gate_are_total_and_exact() {
    // the payout formula by phase: 1 -> 2 pays nothing, 2..4 -> next pays 0.08 S, 5 -> Graduated pays 0.08 S, everything else is InvalidInput
    for phase in 0..=u8::MAX {
        let got = guarded("next_phase", &phase, || next_phase(phase));
        match phase {
            1 => assert_eq!(got, Ok(NextPhase::Next { phase: 2, payout: false })),
            2..=4 => assert_eq!(got, Ok(NextPhase::Next { phase: phase + 1, payout: true })),
            5 => assert_eq!(got, Ok(NextPhase::Graduated)),
            _ => assert_eq!(got, Err(EngineError::InvalidInput), "phase {phase}"),
        }
        if let Ok(n) = got {
            assert_eq!(n.pays(), phase != 1, "only phase 1 -> 2 is free");
        }
    }
    // the sum over a full record chain of payouts is 4 x 0.08 S = 0.32 S exactly, for every whole-dollar size
    for &s in &SIZES {
        let mut total = 0i64;
        for phase in 1..=5u8 {
            if next_phase(phase).unwrap().pays() {
                total += payout_micro(s).unwrap();
            }
        }
        assert_eq!(i128::from(total) * 100, i128::from(s) * 32, "R-1 chain bound is exactly 32 % of S for S = {s}");
    }
    // pass gate: failed, then flat, then target; boundary B = 1.08 S - 1, 1.08 S, 1.08 S + 1
    let mut rng = rng_for("pass_check");
    for _ in 0..CASES {
        let s = if rng.chance(600_000) { rng.pick(&SIZES) } else { 1_000_000 * (1 + log_u64(&mut rng, 23) as i64) };
        let target = s / 10_000 * TARGET_BPS;
        let b = match rng.range(0, 4) {
            0 => target - 1,
            1 => target,
            2 => target + 1,
            _ => gi64(&mut rng),
        };
        let mut st = AccountState::new_phase(s);
        st.b = b;
        let (flat, failed) = (rng.chance(500_000), rng.chance(300_000));
        let got = guarded("pass_check", &(s, b, flat, failed), || pass_check(&st, flat, failed));
        let want = if failed {
            Err(PassError::PhaseFailed)
        } else if !flat {
            Err(PassError::NotFlat)
        } else if b >= target {
            Ok(())
        } else {
            Err(PassError::TargetNotReached)
        };
        assert_eq!(got, want, "pass_check(S={s}, B={b}, flat={flat}, failed={failed})");
    }
    // an unusable size can never reach a target
    for size in [0i64, -1, i64::MIN, i64::MAX, 1_000_001] {
        let mut st = AccountState::new_phase(size);
        st.b = i64::MAX;
        assert_eq!(pass_check(&st, true, false), Err(PassError::TargetNotReached), "size {size}");
    }
}

// ================================================================================================================ CLOSED SLICES AND SAMPLES

fn gen_state(rng: &mut Rng) -> AccountState {
    let size = if rng.chance(600_000) { rng.pick(&SIZES) } else { 1_000_000 * (1 + log_u64(rng, 23) as i64) };
    let mut st = AccountState::new_phase(size);
    st.b = match rng.range(0, 5) {
        0 => size / 100 * rng.range(80, 140),
        1 => size / 10_000 * rng.pick(&[9_200i64, 9_600, 10_400, 10_800, 9_201, 10_399, 10_401]) + rng.range(-2, 2),
        2 => gi64(rng),
        _ => size + rng.range(-size / 5, size / 3),
    };
    st.latch = rng.chance(300_000);
    st.q_tally = match rng.range(0, 4) {
        0 => size / 10_000 * QUICK_LIMIT_BPS + rng.range(-2, 2),
        1 => 0,
        2 => (gi64(rng) / 2).abs(),
        _ => rng.range(0, size / 30),
    };
    st.tight_on = rng.chance(400_000);
    if st.tight_on {
        st.r = if rng.chance(800_000) { st.b } else { gi64(rng) };
        st.g_peak = rng.range(0, size / 10);
        let f = size / 10_000 * if st.latch { LATCHED_FLOOR_BPS } else { STATIC_FLOOR_BPS };
        st.l_start = (f + size / 10_000 * TIGHT_OFFSET_BPS).min(st.r);
        st.l_level = st.l_start + (st.g_peak - size / 10_000 * RATCHET_SLACK_BPS).max(0);
    }
    st.lock_until = if rng.chance(300_000) { rng.range(0, 3_000_000_000) } else { 0 };
    st.flat = rng.chance(500_000);
    st
}

#[test]
fn apply_closed_slice_matches_the_reference_and_never_wraps() {
    const NAME: &str = "apply_closed_slice";
    let mut rng = rng_for(NAME);
    let (mut ok, mut err, mut latched, mut failed_q, mut boundary) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for i in 0..CASES {
        let st = gen_state(&mut rng);
        let rules = Rules { frozen_ratchet: rng.chance(500_000), quick_strict: rng.chance(500_000) };
        let size = st.size_micro;
        // aim credits and tallies at the latch trigger and the quick limit so the boundaries are exercised exactly
        let credit = match rng.range(0, 6) {
            0 => (size / 10_000 * LATCH_TRIGGER_BPS).saturating_sub(st.b).saturating_add(rng.range(-1, 1)),
            1 => rng.range(-size / 50, size / 50),
            2 => gi64(&mut rng),
            _ => rng.range(-3_000, 3_000),
        };
        let delta = match rng.range(0, 6) {
            0 => (size / 10_000 * QUICK_LIMIT_BPS).saturating_sub(st.q_tally).saturating_add(rng.range(-1, 1)),
            1 => rng.range(-5, 5),
            2 => gi64(&mut rng),
            _ => rng.range(0, 3_000),
        };
        let got = guarded(NAME, &(st, rules.frozen_ratchet, rules.quick_strict, credit, delta), || apply_closed_slice(&st, &rules, credit, delta));
        // reference
        let want: Result<_, EngineError> = (|| {
            if delta < 0 {
                return Err(EngineError::InvalidInput);
            }
            let b = i64::try_from(i128::from(st.b) + i128::from(credit)).map_err(|_| EngineError::Overflow)?;
            let q = i64::try_from(i128::from(st.q_tally) + i128::from(delta)).map_err(|_| EngineError::Overflow)?;
            let latch_at = ref_frac(size, LATCH_TRIGGER_BPS)?;
            let mut next = st;
            next.b = b;
            next.q_tally = q;
            if !st.latch && b >= latch_at {
                next.latch = true;
                if st.tight_on && !rules.frozen_ratchet {
                    let f_new = ref_frac(size, LATCHED_FLOOR_BPS)?;
                    let cap = f_new + ref_frac(size, TIGHT_OFFSET_BPS)?;
                    next.l_start = cap.min(st.r);
                    let slack = ref_frac(size, RATCHET_SLACK_BPS)?;
                    next.l_level = next.l_start + (st.g_peak - slack).max(0);
                }
            }
            let limit = ref_frac(size, QUICK_LIMIT_BPS)?;
            let over = if rules.quick_strict { q > limit } else { q >= limit };
            Ok((next, over))
        })();
        match (got, want) {
            (Ok(d), Ok((next, over))) => {
                assert_eq!(d.state, next, "apply_closed_slice state, iteration {i}");
                assert_eq!(d.fail.is_some(), over, "quick-limit decision at the boundary: Q={} limit={}", next.q_tally, ref_frac(size, QUICK_LIMIT_BPS).unwrap());
                ok += 1;
                latched += u64::from(next.latch && !st.latch);
                failed_q += u64::from(over);
                boundary += u64::from(next.q_tally == ref_frac(size, QUICK_LIMIT_BPS).unwrap());
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "apply_closed_slice({st:?}, {credit}, {delta})");
                err += 1;
            }
            (g, w) => panic!("apply_closed_slice disagrees with the reference for {st:?} credit {credit} delta {delta}: engine {g:?}, reference {w:?}"),
        }
    }
    println!("RESULT {NAME}: seed={SEED:#x} random_cases={CASES} ok={ok} err={err} latches={latched} quick_fails={failed_q} exactly_at_limit={boundary}");
    assert!(ok > 30_000 && err > 1_000 && latched > 500 && failed_q > 500 && boundary > 100);
}

fn check_no_panic_state_fns(st: &AccountState, rules: &Rules, a: i64, b: i64, now: i64) {
    let _ = guarded("evaluate_sample", &(st, a, b, now), || evaluate_sample(st, rules, a, b, now));
    let _ = guarded("apply_closed_slice", &(st, a, b), || apply_closed_slice(st, rules, a, b));
    let _ = guarded("effective_floor_micro", st, || effective_floor_micro(st));
    let _ = guarded("static_floor_micro", st, || static_floor_micro(st));
    let _ = guarded("can_increase_exposure", &(st, now), || can_increase_exposure(st, now));
    let _ = guarded("end_tight_if_flat", st, || end_tight_if_flat(st, a > 0));
    let _ = guarded("pass_check", st, || pass_check(st, a > 0, b > 0));
    let _ = guarded("check_tight_start", &(st, a, b), || check_tight_start(st, a, b, a, b, a));
    let _ = guarded("check_floor_raised", &(st, a, b), || check_floor_raised(st, a, b));
    let _ = guarded("check_tight_end", st, || check_tight_end(st));
    let _ = guarded("check_breach", &(st, now, a, b), || check_breach(st, now, a, b));
}

#[test]
fn no_account_state_function_panics_on_hostile_states() {
    // every field of the AccountState is an arbitrary i64 / flag: the program feeds the engine from stored bytes, so the engine must return an error
    let mut rng = rng_for("hostile_states");
    for _ in 0..CASES {
        let st = AccountState {
            size_micro: gi64(&mut rng),
            b: gi64(&mut rng),
            latch: rng.chance(500_000),
            tight_on: rng.chance(500_000),
            r: gi64(&mut rng),
            l_start: gi64(&mut rng),
            g_peak: gi64(&mut rng),
            l_level: gi64(&mut rng),
            q_tally: gi64(&mut rng),
            lock_until: gi64(&mut rng),
            flat: rng.chance(500_000),
        };
        let rules = Rules { frozen_ratchet: rng.chance(500_000), quick_strict: rng.chance(500_000) };
        check_no_panic_state_fns(&st, &rules, gi64(&mut rng), gi64(&mut rng), gi64(&mut rng));
    }
    // and the all-extremes cartesian grid
    for &size in &[0i64, 1, 1_000_000, 200_000_000_000, i64::MAX, i64::MIN] {
        for &b in &EI64 {
            for &x in &EI64 {
                for &now in &[i64::MIN, 0, i64::MAX] {
                    let mut st = AccountState::new_phase(size);
                    st.b = b;
                    st.r = x;
                    st.g_peak = x;
                    st.l_level = x;
                    st.q_tally = x;
                    st.tight_on = true;
                    check_no_panic_state_fns(&st, &Rules::default(), x, b, now);
                }
            }
        }
    }
    // lock_end overflow: a breach / sample at the end of time is an error, not a wrapped lock
    let mut st = AccountState::new_phase(100_000_000_000);
    st.tight_on = true;
    st.r = st.b;
    st.l_start = 96_000_000_000;
    st.l_level = 96_000_000_000;
    assert_eq!(check_breach(&st, i64::MAX, 96_000_000_000, st.b - 4_000_000_000).map_err(|e| e), Err(ClaimError::BreachBalance));
    st.b = 96_000_000_000;
    st.r = st.b;
    assert_eq!(check_breach(&st, i64::MAX, 96_000_000_000, 96_000_000_000), Err(ClaimError::Engine(EngineError::Overflow)), "lock_until = now + 12 h must not wrap");
    assert!(check_breach(&st, 1_700_000_000, 96_000_000_000, 96_000_000_000).is_ok());
}

#[test]
fn evaluate_sample_matches_an_independent_reference_and_the_program_checks_accept_it() {
    const NAME: &str = "evaluate_sample";
    let mut rng = rng_for(NAME);
    let (mut fails, mut breaches, mut starts, mut raises, mut nones) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for _ in 0..CASES {
        let st = gen_state(&mut rng);
        // keep every number far from the i64 limits: this test is about the decision rules (extremes are in the hostile test above)
        if st.b.unsigned_abs() > 1 << 55 || st.r.unsigned_abs() > 1 << 55 {
            continue;
        }
        let size = st.size_micro;
        let p = match rng.range(0, 4) {
            0 => size / 10_000 * TIGHT_START_BPS + rng.range(-2, 2),
            1 => rng.range(-size / 5, size / 5),
            2 => st.g_peak + st.r - st.b + rng.range(-2, 2),
            _ => rng.range(-size / 50, size / 20),
        };
        let comm = rng.range(0, size / 1_000);
        let now = 1_700_000_000 + rng.range(0, 100_000);
        let rules = Rules::default();
        let got = guarded(NAME, &(st, p, comm, now), || evaluate_sample(&st, &rules, p, comm, now));
        let d = got.unwrap_or_else(|e| panic!("evaluate_sample({st:?}, {p}, {comm}) failed with {e:?}"));
        // independent reference of DESIGN 2.2 / 2.3, order (a) to (e)
        let f_s = size / 10_000 * if st.latch { LATCHED_FLOOR_BPS } else { STATIC_FLOOR_BPS };
        let slack = size / 10_000 * RATCHET_SLACK_BPS;
        let eq = st.b + p - comm;
        assert_eq!(d.equity_micro, eq);
        let eff = if st.tight_on { f_s.max(st.l_level) } else { f_s };
        if eq <= f_s {
            assert_eq!((d.action, d.state), (SampleAction::Fail(engine::FailReason::StaticFloor), st));
            fails += 1;
        } else if st.tight_on && eq <= eff {
            let lock = now + 12 * 3_600;
            assert_eq!(d.action, SampleAction::Breach { close_all: true, lock_until: lock });
            assert_eq!(d.state, AccountState { tight_on: false, r: 0, l_start: 0, g_peak: 0, l_level: 0, lock_until: lock, flat: true, ..st });
            breaches += 1;
        } else if !st.tight_on && p > slack {
            let g = (eq - st.b).max(0);
            let l_start = (f_s + size / 10_000 * TIGHT_OFFSET_BPS).min(st.b);
            let l_level = l_start + (g - slack).max(0);
            assert_eq!(d.action, SampleAction::TightStart { r: st.b, l_start, g_peak: g, l_level });
            assert_eq!(d.state, AccountState { tight_on: true, r: st.b, l_start, g_peak: g, l_level, ..st });
            // the program's claim check must accept exactly what the evaluator produced
            assert_eq!(check_tight_start(&st, p, st.b, l_start, g, l_level), Ok(d.state), "program rejects the evaluator's TightStart");
            starts += 1;
        } else if st.tight_on && eq - st.r > st.g_peak {
            let g = eq - st.r;
            let l_level = st.l_start + (g - slack).max(0);
            assert_eq!(d.action, SampleAction::FloorRaised { g_peak: g, l_level });
            assert_eq!(check_floor_raised(&st, g, l_level), Ok(d.state), "program rejects the evaluator's FloorRaised");
            raises += 1;
        } else {
            assert_eq!((d.action, d.state), (SampleAction::None, st));
            nones += 1;
        }
    }
    println!("RESULT {NAME}: seed={SEED:#x} fails={fails} breaches={breaches} tight_starts={starts} floor_raises={raises} nothing={nones}");
    assert!(fails > 500 && breaches > 500 && starts > 500 && raises > 500 && nones > 500, "a decision branch is not covered");
}

// ================================================================================================================ CLAIM TAMPERING

#[test]
fn check_close_rejects_every_claim_that_favours_the_trader_by_one_micro_usd() {
    const NAME: &str = "check_close";
    let mut rng = rng_for(NAME);
    let (mut accepted, mut tampered) = (0u64, 0u64);
    for _ in 0..CASES {
        let long = rng.chance(500_000);
        let qty = 1 + if rng.chance(800_000) { log_u64(&mut rng, 50) } else { g64(&mut rng) % u64::MAX };
        let entry = 1 + if rng.chance(800_000) { log_u64(&mut rng, 52) } else { g64(&mut rng) % u64::MAX };
        let exit = match rng.range(0, 3) {
            0 => entry.saturating_add(rng.range(0, 5_000) as u64).max(1),
            1 => entry.saturating_sub(rng.range(0, 5_000) as u64).max(1),
            _ => 1 + log_u64(&mut rng, 52),
        };
        let timer = 1_700_000_000i64;
        let time = timer + rng.pick(&[0i64, 1, 44, 45, 46, 47, 1_000, 86_400]);
        let Some((pnl, _)) = ref_pnl(long, qty, entry, exit) else {
            // an unrepresentable P&L cannot be claimed truthfully; the claim check must say Overflow whatever pnl is claimed
            let c = CloseClaim { long, qty, entry_e12: entry, exit_e12: exit, time, timer_start: timer, pnl: 0, quick: false, credited: 0 };
            assert_eq!(guarded(NAME, &c, || check_close(&c)).err(), Some(ClaimError::Engine(EngineError::Overflow)));
            continue;
        };
        let quick = time - timer <= QUICK_SECS;
        let credited = if quick && pnl > 0 { pnl / 2 } else { pnl };
        let tally = if quick { credited.max(0) } else { 0 };
        let honest = CloseClaim { long, qty, entry_e12: entry, exit_e12: exit, time, timer_start: timer, pnl, quick, credited };
        let r = guarded(NAME, &honest, || check_close(&honest));
        assert_eq!(r.map(|c| (c.credit, c.tally_delta)), Ok((credited, tally)), "an honest claim must be accepted: {honest:?}");
        accepted += 1;
        // +1 / -1 micro-USD on the P&L, on the credit, a flipped quick flag, a zero quantity / price, a timer after the close
        let up = |v: i64| v.checked_add(1).unwrap_or(v - 1);
        let cases: [(CloseClaim, ClaimError); 7] = [
            (CloseClaim { pnl: up(pnl), ..honest }, ClaimError::PnlMismatch),
            (CloseClaim { pnl: pnl.checked_sub(1).unwrap_or(pnl + 1), ..honest }, ClaimError::PnlMismatch),
            (CloseClaim { credited: up(credited), ..honest }, ClaimError::CreditMismatch),
            (CloseClaim { credited: credited.checked_sub(1).unwrap_or(credited + 1), ..honest }, ClaimError::CreditMismatch),
            (CloseClaim { quick: !quick, ..honest }, ClaimError::QuickMismatch),
            (CloseClaim { qty: 0, ..honest }, ClaimError::ZeroQuantity),
            (CloseClaim { timer_start: time + 1, ..honest }, ClaimError::TimerAfterTime),
        ];
        for (c, want) in cases {
            assert_eq!(guarded(NAME, &c, || check_close(&c)).err(), Some(want), "tampered claim accepted or wrong error: {c:?}");
            tampered += 1;
        }
        assert_eq!(check_close(&CloseClaim { exit_e12: 0, ..honest }).err(), Some(ClaimError::ZeroPrice));
        assert_eq!(check_close(&CloseClaim { entry_e12: 0, ..honest }).err(), Some(ClaimError::ZeroPrice));
        tampered += 2;
    }
    println!("RESULT {NAME}: seed={SEED:#x} random_cases={CASES} honest_accepted={accepted} tampered_refused={tampered}");
    assert!(accepted >= 90_000 && tampered >= 630_000);
    // timer after time with an age that would overflow: time = i64::MIN, timer = i64::MAX is TimerAfterTime, not a wrapped age
    let c = CloseClaim { long: true, qty: 1, entry_e12: 1, exit_e12: 1, time: i64::MIN, timer_start: i64::MAX, pnl: 0, quick: true, credited: 0 };
    assert_eq!(check_close(&c).err(), Some(ClaimError::TimerAfterTime));
    // age beyond i64: Overflow
    let c = CloseClaim { time: i64::MAX, timer_start: i64::MIN, ..c };
    assert_eq!(check_close(&c).err(), Some(ClaimError::Engine(EngineError::Overflow)));
}

// ================================================================================================================ TIME

#[test]
fn day_index_and_nights_due_are_exact_floors_at_every_extreme() {
    let mut rng = rng_for("day_index");
    let check = |t: i64| {
        let got = guarded("day_index", &t, || day_index(t));
        let want = (i128::from(t) - 79_200).div_euclid(86_400);
        assert_eq!(i128::from(got), want, "day_index({t})");
    };
    for &t in &EI64 {
        check(t);
    }
    // the rollover instant itself and the second before, around several days, negative and positive
    for d in -50i64..50 {
        let r = d * 86_400 + 79_200;
        check(r - 1);
        check(r);
        check(r + 1);
        assert_eq!(day_index(r), d, "a position held through 22:00:00 on day {d}");
        assert_eq!(day_index(r - 1), d - 1);
    }
    for _ in 0..CASES {
        check(gi64(&mut rng));
    }
    // nights_due: today - fee_day as u32, InvalidInput when today < fee_day, Overflow beyond u32 (and beyond i64)
    for _ in 0..CASES {
        let (fee_day, today) = if rng.chance(500_000) { (rng.range(-3, 3), rng.range(-3, 3)) } else { (gi64(&mut rng), gi64(&mut rng)) };
        let got = guarded("nights_due", &(fee_day, today), || nights_due(fee_day, today));
        let diff = i128::from(today) - i128::from(fee_day);
        let want = if diff < 0 {
            Err(EngineError::InvalidInput)
        } else if u32::try_from(diff).is_ok() {
            Ok(diff as u32)
        } else {
            Err(EngineError::Overflow)
        };
        assert_eq!(got, want, "nights_due({fee_day},{today})");
    }
    // a charge for u32::MAX nights is computed without wrap (or Overflow): never silently small
    let fee = holding_fee_micro(1_000_000, 1_000_000_000_000, 1_600, u32::MAX);
    assert_eq!(fee, Ok(i64::try_from((1_000_000u128 * 1_000_000_000_000 * 1_600 * u128::from(u32::MAX)).div_ceil(1_000_000_000_000_000_000)).unwrap()));
}

// ================================================================================================================ ORDER VALIDATION

#[test]
fn validate_quantity_and_stop_levels_match_their_tables() {
    let mut rng = rng_for("validate");
    for _ in 0..CASES {
        let (qty, step, min) = (g64(&mut rng), if rng.chance(300_000) { g64(&mut rng) } else { rng.pick(&[1u64, 1_000, 10_000, 1_000_000]) }, g64(&mut rng));
        let got = guarded("validate_quantity", &(qty, step, min), || validate_quantity(qty, step, min));
        let want = if step == 0 {
            Err(EngineError::InvalidInput)
        } else if qty == 0 || qty % step != 0 || qty < min {
            Err(EngineError::InvalidQuantity)
        } else {
            Ok(())
        };
        assert_eq!(got, want, "validate_quantity({qty},{step},{min})");
        let (level, bid) = (if rng.chance(500_000) { g64(&mut rng) } else { rng.pick(&E64) }, g64(&mut rng));
        let ask = if rng.chance(500_000) { bid.saturating_add(g64(&mut rng) % 5) } else { g64(&mut rng) };
        for side in [Side::Buy, Side::Sell] {
            for kind in [StopKind::StopLoss, StopKind::TakeProfit] {
                let got = guarded("validate_stop_level", &(side, level, bid, ask), || validate_stop_level(side, kind, level, bid, ask));
                let ok = level != 0
                    && match (side, kind) {
                        (Side::Buy, StopKind::StopLoss) => level <= bid,
                        (Side::Buy, StopKind::TakeProfit) => level >= bid,
                        (Side::Sell, StopKind::StopLoss) => level >= ask,
                        (Side::Sell, StopKind::TakeProfit) => level <= ask,
                    };
                assert_eq!(got, if ok { Ok(()) } else { Err(EngineError::InvalidStopLevel) }, "{side:?} {kind:?} level {level} bid {bid} ask {ask}");
            }
        }
    }
    // touching counts as valid (N-10), one step across does not
    assert_eq!(validate_stop_level(Side::Buy, StopKind::StopLoss, 100, 100, 101), Ok(()));
    assert_eq!(validate_stop_level(Side::Buy, StopKind::StopLoss, 101, 100, 101), Err(EngineError::InvalidStopLevel));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::TakeProfit, 101, 100, 101), Ok(()));
    assert_eq!(validate_stop_level(Side::Sell, StopKind::TakeProfit, 102, 100, 101), Err(EngineError::InvalidStopLevel));
}

#[test]
fn triggers_never_panic_and_touching_counts() {
    let mut rng = rng_for("triggers");
    for _ in 0..CASES {
        let (sl, tp, bid) = (g64(&mut rng), g64(&mut rng), g64(&mut rng));
        let ask = if rng.chance(500_000) { bid.saturating_add(1) } else { g64(&mut rng) };
        let open = rng.chance(900_000);
        for side in [Side::Buy, Side::Sell] {
            let got = guarded("check_protective", &(side, sl, tp, bid, ask), || check_protective(side, sl, tp, bid, ask, open));
            let (px, sl_hit, tp_hit) = match side {
                Side::Buy => (bid, sl != 0 && bid <= sl, tp != 0 && bid >= tp),
                Side::Sell => (ask, sl != 0 && ask >= sl, tp != 0 && ask <= tp),
            };
            let want = if !open {
                None
            } else if sl_hit {
                Some((engine::ProtectiveKind::StopLoss, px))
            } else if tp_hit {
                Some((engine::ProtectiveKind::TakeProfit, px))
            } else {
                None
            };
            assert_eq!(got.map(|t| (t.kind, t.price_e12)), want, "protective {side:?} sl {sl} tp {tp} bid {bid} ask {ask}");
            for kind in [PendingKind::Limit, PendingKind::Stop] {
                let locked = rng.chance(200_000);
                let got = guarded("check_pending", &(side, kind, sl, bid, ask), || check_pending(side, kind, sl, bid, ask, open, locked));
                let (px, hit) = match (side, kind) {
                    (Side::Buy, PendingKind::Limit) => (ask, ask <= sl),
                    (Side::Sell, PendingKind::Limit) => (bid, bid >= sl),
                    (Side::Buy, PendingKind::Stop) => (ask, ask >= sl),
                    (Side::Sell, PendingKind::Stop) => (bid, bid <= sl),
                };
                let want = if !open || locked || sl == 0 || !hit { None } else { Some(px) };
                assert_eq!(got, want, "pending {side:?} {kind:?} level {sl} bid {bid} ask {ask}");
            }
        }
    }
}

// ================================================================================================================ NO PANIC ANYWHERE

#[test]
fn every_public_engine_function_survives_the_full_extreme_grid_without_panicking() {
    // a plain cartesian grid, nothing random: every function with up to four arguments over the boundary values of its argument types
    let mut calls = 0u64;
    for &a in &E64 {
        for &b in &E64 {
            let _ = guarded("notional_micro", &(a, b), || notional_micro(a, b));
            for &c in &E32 {
                let _ = guarded("ask_from_mid", &(a, c), || ask_from_mid(a, c));
                let _ = guarded("bid_from_mid", &(a, c), || bid_from_mid(a, c));
                for &d in &E32 {
                    let _ = guarded("holding_fee_micro", &(a, b, c, d), || holding_fee_micro(a, b, c, d));
                    calls += 1;
                }
            }
            for &lev in &E16 {
                let _ = guarded("margin_micro", &(a, b, lev), || margin_micro(a, b, lev));
                let _ = guarded("swap_fee_micro", &(a, lev), || swap_fee_micro(a, lev));
                calls += 1;
            }
            for long in [true, false] {
                for &x in &E64 {
                    let _ = guarded("pnl_micro", &(long, a, b, x), || pnl_micro(long, a, b, x));
                    let _ = guarded("validate_quantity", &(a, b, x), || validate_quantity(a, b, x));
                    calls += 1;
                }
            }
        }
        let _ = guarded("price_from_oracle", &a, || price_from_oracle(u128::from(a) << 64 | u128::from(a)));
    }
    for &n in &EI64 {
        for &r in &E32 {
            let _ = guarded("fee_ceil_micro", &(n, r), || fee_ceil_micro(n, r));
        }
        for quick in [true, false] {
            let _ = guarded("slice_credit", &(n, quick), || slice_credit(n, quick));
        }
        for &bps in &EI64 {
            let _ = guarded("frac_exact", &(n, bps), || frac_exact(n, bps));
        }
        let _ = guarded("payout_micro", &n, || payout_micro(n));
        let _ = guarded("day_index", &n, || day_index(n));
        for &m in &EI64 {
            let _ = guarded("nights_due", &(n, m), || nights_due(n, m));
        }
    }
    for phase in 0..=u8::MAX {
        let _ = guarded("next_phase", &phase, || next_phase(phase));
    }
    println!("RESULT every_public_engine_function_survives_the_full_extreme_grid: {calls} calls in the dense grids, no panic");
}

// ================================================================================================================ FINDINGS

// FINDING ARI-1: `engine::consts::frac` and `static_floor` multiply in plain i64 (`size_micro * bps / BPS`, `#[allow(clippy::arithmetic_side_effects)]`).
// With overflow-checks on (the dev profile of this test and `[profile.release] overflow-checks = true` of the workspace, hence also the SBF build)
// a size above i64::MAX / 10_800 (about 8.5e14 micro-USD, i.e. USD 854 million) PANICS. Nothing in the program, the evaluator or the verifier calls these
// two functions (grep: only the engine's own unit tests), so it is not reachable today; it is a public panic path of the engine crate.
// Severity info, plain implementation error (return a Result or use `frac_exact`).
#[test]
#[ignore = "FINDING ARI-1: engine::consts::frac / static_floor panic (overflow-checks) for a size above ~8.5e14 micro-USD; unused by production code"]
fn finding_ari_1_consts_frac_does_not_panic_for_an_extreme_size() {
    for size in [i64::MAX, 900_000_000_000_000, 853_000_000_000_000, i64::MIN] {
        for bps in [10_800i64, 9_200] {
            let r = catch_unwind(|| engine::consts::frac(size, bps));
            assert!(r.is_ok(), "engine::consts::frac({size}, {bps}) panicked: the engine must return an error, never panic");
        }
        let r = catch_unwind(|| engine::consts::static_floor(size, true));
        assert!(r.is_ok(), "engine::consts::static_floor({size}, true) panicked");
    }
}

#[test]
fn consts_frac_is_correct_for_every_size_the_tier_table_allows() {
    // LIMITED: pins what still works. For every size up to USD 200,000 (and far beyond, up to the first overflow) `frac` equals `frac_exact`.
    for &s in &SIZES {
        for bps in [STATIC_FLOOR_BPS, LATCHED_FLOOR_BPS, LATCH_TRIGGER_BPS, TARGET_BPS, PAYOUT_BPS, TIGHT_START_BPS, TIGHT_OFFSET_BPS, QUICK_LIMIT_BPS] {
            assert_eq!(Ok(engine::consts::frac(s, bps)), frac_exact(s, bps));
        }
    }
    assert_eq!(engine::consts::static_floor(200_000_000_000, false), 184_000_000_000);
    // the last size that does not overflow at the largest threshold (10_800 bps): i64::MAX / 10_800
    let last = i64::MAX / 10_800;
    assert_eq!(engine::consts::frac(last, 10_800), last * 10_800 / 10_000);
}

// FINDING ARI-2: the notional-cap check of the evaluator compares the TRUNCATED notional (`engine::units::notional_micro` rounds toward zero) with
// `max_notional_micro` using `>` (exec.rs, both at arrival and at execution). An order whose exact notional is cap + 0.0000005 micro-USD (any fraction
// of a micro-USD above the cap) has a truncated notional equal to the cap and is ACCEPTED, so the cap is exceeded by up to (but less than) 1 micro-USD.
// DESIGN 7.4 says rounding is always against the trader. The effect is below one micro-USD of notional (no balance changes), so severity is info; it is a plain
// implementation error (compare `qty * price > cap * DEN` exactly, or round the notional up).
#[test]
#[ignore = "FINDING ARI-2: the evaluator's notional cap compares the notional truncated toward zero, so a position up to 1 micro-USD above the cap is accepted"]
fn finding_ari_2_notional_cap_is_enforced_on_the_exact_notional() {
    use common::*;
    use evaluator::codec::IntentKind;
    use evaluator::OrderReject as R;
    // qty 1,000 units (1e-3), limit price 1.0000000005: exact notional = 1000 * 1_000_000_000_500 / 1e12 = 1000.0000005 micro-USD, truncated 1000
    let (qty, price) = (1_000u64, 1_000_000_000_500u64);
    assert_eq!(notional_micro(qty, price), Ok(1_000), "truncated notional is the cap");
    assert!(u128::from(qty) * u128::from(price) > 1_000u128 * u128::from(DEN_U64), "the exact notional is above the cap");
    let mut m = eur_market(0);
    m.max_notional_micro = 1_000;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    let lim = order(IntentKind::Limit, 0, 0, qty, price, 0, 0, 0, T_1400);
    assert_eq!(w.order(lim), Err(R::NotionalCap), "the exact notional is above the cap: the order must be refused");
}

#[test]
fn notional_cap_truncation_is_pinned_the_equal_cap_is_accepted_and_one_micro_more_is_refused() {
    // LIMITED: what the evaluator does today, pinned exactly (a fix of ARI-2 must change THIS test and un-ignore the finding).
    use common::*;
    use evaluator::codec::IntentKind;
    use evaluator::OrderReject as R;
    let mut m = eur_market(0);
    m.max_notional_micro = 1_000;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    let lim = |price| order(IntentKind::Limit, 0, 0, 1_000, price, 0, 0, 0, T_1400);
    assert!(w.order(lim(1_000_000_000_000)).is_ok(), "exact notional equal to the cap is allowed");
    assert!(w.order(lim(1_000_000_000_999)).is_ok(), "PINNED: 1000.000999 micro-USD of exact notional is accepted although it is above the cap of 1000");
    assert_eq!(w.order(lim(1_001_000_000_000)), Err(R::NotionalCap), "1001 micro-USD is refused");
}
