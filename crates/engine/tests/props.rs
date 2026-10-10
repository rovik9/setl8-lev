//! Property tests (256 cases each): netting invariants and "no value created by rounding".
use engine::*;
use proptest::prelude::*;

const DEN: i128 = 1_000_000_000_000;

#[derive(Clone, Debug)]
struct F {
    buy: bool,
    dq: u64,
    price: u64,
    dt: i64,
}

fn arb_fill() -> impl Strategy<Value = F> {
    (any::<bool>(), 1u64..=5_000_000_000u64, 1u64..=5_000_000_000_000u64, 0i64..=120)
        .prop_map(|(buy, dq, price, dt)| F { buy, dq, price, dt })
}

fn side(buy: bool) -> Side {
    if buy {
        Side::Buy
    } else {
        Side::Sell
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// (a) The quantity moves by exactly the fill's size every time, so after any sequence it equals the signed sum of
    /// the fills, and the market is flat iff that sum is zero.
    #[test]
    fn a_quantity_follows_the_signed_sum_and_flat_iff_zero(fills in proptest::collection::vec(arb_fill(), 1..30)) {
        let mut pos: Option<Position> = None;
        let mut sum: i128 = 0;
        let mut t = 1_760_000_000i64;
        for f in fills {
            t += f.dt;
            let before = pos.map_or(0i128, |p| i128::from(p.qty));
            let o = apply_fill(pos, side(f.buy), f.dq, f.price, t, 0).unwrap();
            pos = o.position;
            let after = pos.map_or(0i128, |p| i128::from(p.qty));
            let signed = if f.buy { i128::from(f.dq) } else { -i128::from(f.dq) };
            sum += signed;
            prop_assert_eq!(after - before, signed);
            prop_assert!((after - before).abs() <= i128::from(f.dq));
            prop_assert_eq!(after, sum);
            prop_assert_eq!(pos.is_none(), sum == 0);
            if let Some(p) = pos {
                prop_assert!(p.qty != 0);
            }
            // closing slice never exceeds either side
            if let Some(c) = o.closed {
                prop_assert!(c.qty_abs <= f.dq);
                prop_assert!(i128::from(c.qty_abs) <= before.abs());
            }
            // credit never exceeds the pnl, and equals it for losses
            if let Some(c) = o.closed {
                prop_assert!(c.credit <= c.pnl.max(0));
                if c.pnl <= 0 { prop_assert_eq!(c.credit, c.pnl); }
                prop_assert_eq!(o.balance_delta, c.credit);
                prop_assert!(o.quick_tally_delta >= 0);
            } else {
                prop_assert_eq!((o.balance_delta, o.quick_tally_delta), (0, 0));
            }
        }
    }

    /// (b) Rounding never creates value: realized (floored per slice, after the credit rule is ignored) plus unrealized at the
    /// final mark is at most the exact rational profit of the whole fill stream.
    #[test]
    fn b_rounding_never_creates_value(fills in proptest::collection::vec(arb_fill(), 1..30), mark in 1u64..=5_000_000_000_000u64) {
        let mut pos: Option<Position> = None;
        let mut realized: i128 = 0;
        let mut cash_e18: i128 = 0; // exact: sum of -signed_qty x price, in 1e-18 USD
        let mut t = 1_760_000_000i64;
        for f in fills {
            t += f.dt;
            let o = apply_fill(pos, side(f.buy), f.dq, f.price, t, 0).unwrap();
            if let Some(c) = o.closed { realized += i128::from(c.pnl); }
            let signed = if f.buy { i128::from(f.dq) } else { -i128::from(f.dq) };
            cash_e18 -= signed * i128::from(f.price);
            pos = o.position;
        }
        let (unreal, qty) = match pos {
            Some(p) => (i128::from(unrealized_micro(&p, mark, mark).unwrap()), i128::from(p.qty)),
            None => (0, 0),
        };
        let exact_e18 = cash_e18 + qty * i128::from(mark); // exact total profit x 1e12 (in micro-USD x DEN)
        prop_assert!((realized + unreal) * DEN <= exact_e18, "rounded {} > exact {}", (realized + unreal) * DEN, exact_e18);
        // and it loses less than one micro-USD per slice plus per merge average: bounded below as well
        prop_assert!((realized + unreal) * DEN > exact_e18 - 120 * DEN);
    }

    /// (c1) A long round trip and its mirror image (a short round trip with the price moves reflected) agree exactly.
    #[test]
    fn c1_mirror_trade_has_the_same_pnl(dq in 1u64..=1_000_000_000_000u64, p in 1u64..=1_000_000_000_000_000u64, d in 0u64..=1_000_000_000_000u64, up in any::<bool>()) {
        let t = 1_760_000_000;
        let delta = d.min(p - 1); // keep both exit prices positive
        let (long_exit, short_exit) = if up { (p + delta, p - delta) } else { (p - delta, p + delta) };
        let long_open = apply_fill(None, Side::Buy, dq, p, t, 0).unwrap().position;
        let long = apply_fill(long_open, Side::Sell, dq, long_exit, t + 100, 0).unwrap().closed.unwrap();
        let short_open = apply_fill(None, Side::Sell, dq, p, t, 0).unwrap().position;
        let short = apply_fill(short_open, Side::Buy, dq, short_exit, t + 100, 0).unwrap().closed.unwrap();
        prop_assert_eq!(long.pnl, short.pnl);
    }

    /// (c2) The same price move on a long and a short: the two P&Ls are opposite up to one micro-USD of rounding
    /// (both round down), i.e. a + b is 0 or -1 and |a| - |b| is within 1.
    #[test]
    fn c2_opposite_sides_same_move_cancel_up_to_rounding(dq in 1u64..=1_000_000_000_000u64, p in 1u64..=1_000_000_000_000_000u64, exit in 1u64..=1_000_000_000_000_000u64) {
        let t = 1_760_000_000;
        let lo = apply_fill(None, Side::Buy, dq, p, t, 0).unwrap().position;
        let a = apply_fill(lo, Side::Sell, dq, exit, t + 100, 0).unwrap().closed.unwrap();
        let so = apply_fill(None, Side::Sell, dq, p, t, 0).unwrap().position;
        let b = apply_fill(so, Side::Buy, dq, exit, t + 100, 0).unwrap().closed.unwrap();
        let sum = i128::from(a.pnl) + i128::from(b.pnl);
        prop_assert!(sum == 0 || sum == -1, "a={} b={}", a.pnl, b.pnl);
        prop_assert!((i128::from(a.pnl.unsigned_abs()) - i128::from(b.pnl.unsigned_abs())).abs() <= 1);
        prop_assert_eq!(a.quick, b.quick);
    }

    /// Margin is at least the exact value and less than one micro-USD above it.
    #[test]
    fn margin_is_a_tight_ceiling(q in 0u64..=1_000_000_000_000_000u64, p in 1u64..=1_000_000_000_000_000u64, lev in 1u16..=50u16) {
        let m = i128::from(margin_micro(q, p, lev).unwrap());
        let exact_num = i128::try_from(u128::from(q) * u128::from(p)).unwrap(); // in 1e-12 micro-USD, before /lev
        let scaled = m * DEN * i128::from(lev);
        prop_assert!(scaled >= exact_num);
        prop_assert!(scaled - exact_num < DEN * i128::from(lev));
    }

    /// ask >= mid >= bid and the spread is at most one unit wider than the exact value.
    #[test]
    fn ask_and_bid_bracket_the_mid(mid in 1u64..=1_000_000_000_000_000u64, h in 0u32..=100_000u32) {
        let ask = ask_from_mid(mid, h).unwrap();
        let bid = bid_from_mid(mid, h).unwrap();
        prop_assert!(bid <= mid && mid <= ask);
        let exact_up = u128::from(mid) * (1_000_000 + u128::from(h));
        prop_assert!(u128::from(ask) * 1_000_000 >= exact_up);
        prop_assert!(u128::from(ask) * 1_000_000 - exact_up < 1_000_000);
        let exact_dn = u128::from(mid) * (1_000_000 - u128::from(h));
        prop_assert!(u128::from(bid) * 1_000_000 <= exact_dn);
        prop_assert!(exact_dn - u128::from(bid) * 1_000_000 < 1_000_000);
    }

    /// apply_fill never panics on arbitrary input, whatever it returns.
    #[test]
    fn apply_fill_never_panics(
        q in any::<i64>(), avg in any::<u64>(), timer in any::<i64>(), buy in any::<bool>(),
        dq in any::<u64>(), price in any::<u64>(), t in any::<i64>(), fd in any::<i64>()
    ) {
        let p = Position { qty: q, avg_entry_e12: avg, timer_start: timer, opened_at: 0, fee_day: 0 };
        let _ = apply_fill(Some(p), side(buy), dq, price, t, fd);
        let _ = apply_fill(None, side(buy), dq, price, t, fd);
        let _ = unrealized_micro(&p, price, avg);
        let _ = margin_micro(dq, price, 50);
        let _ = holding_fee_micro(dq, price, 1000, 3);
    }
}
