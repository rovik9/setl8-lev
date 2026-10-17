//! Golden vectors for non-Rust backends (`tests/golden/`): JSON inputs and outputs of the pure functions a backend must
//! reproduce, and complete small scenarios. All integers are decimal strings; byte strings are lower-case hex; keys are
//! sorted. This test regenerates every file and compares it byte for byte with the committed one.
//! `GOLDEN_WRITE=1 cargo test -p setl8-lev-evaluator --test golden` rewrites the files.
mod common;
use common::*;
use engine::*;
use evaluator::codec::{IntentKind, OrderIntent};
use evaluator::testkit::Rng;
use evaluator::*;
use journal_types::{JournalBatch, JournalEvent, PriceProof};
use serde_json::{json, Value};

fn n<T: ToString>(x: T) -> Value {
    Value::String(x.to_string())
}

fn hexv(b: &[u8]) -> Value {
    Value::String(evaluator::codec::hex(b))
}

fn res<T>(r: engine::Result<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": f(v)}),
        Err(e) => json!({"err": format!("{e:?}")}),
    }
}

fn file(function: &str, description: &str, cases: Vec<Value>) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("\"function\": {},\n", serde_json::to_string(function).unwrap()));
    s.push_str(&format!("\"description\": {},\n", serde_json::to_string(description).unwrap()));
    s.push_str("\"version\": \"1\",\n\"cases\": [\n");
    let last = cases.len().saturating_sub(1);
    for (i, c) in cases.iter().enumerate() {
        s.push_str(&serde_json::to_string(c).unwrap());
        s.push_str(if i == last { "\n" } else { ",\n" });
    }
    s.push_str("]\n}\n");
    s
}

// ------------------------------------------------------------------------------------------------ small converters

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn pos_json(p: &Position) -> Value {
    json!({"qty": n(p.qty), "avg_entry_e12": n(p.avg_entry_e12), "timer_start": n(p.timer_start), "opened_at": n(p.opened_at), "fee_day": n(p.fee_day)})
}

fn opt_pos(p: &Option<Position>) -> Value {
    p.as_ref().map(pos_json).unwrap_or(Value::Null)
}

fn state_json(s: &AccountState) -> Value {
    json!({"size_micro": n(s.size_micro), "b": n(s.b), "latch": s.latch, "tight_on": s.tight_on, "r": n(s.r), "l_start": n(s.l_start),
        "g_peak": n(s.g_peak), "l_level": n(s.l_level), "q_tally": n(s.q_tally), "lock_until": n(s.lock_until), "flat": s.flat})
}

fn action_json(a: &SampleAction) -> Value {
    match a {
        SampleAction::None => json!({"kind": "none"}),
        SampleAction::Fail(r) => json!({"kind": "fail", "reason": format!("{r:?}")}),
        SampleAction::Breach { close_all, lock_until } => json!({"kind": "breach", "close_all": close_all, "lock_until": n(lock_until)}),
        SampleAction::TightStart { r, l_start, g_peak, l_level } => json!({"kind": "tight_start", "r": n(r), "l_start": n(l_start), "g_peak": n(g_peak), "l_level": n(l_level)}),
        SampleAction::FloorRaised { g_peak, l_level } => json!({"kind": "floor_raised", "g_peak": n(g_peak), "l_level": n(l_level)}),
    }
}

// ------------------------------------------------------------------------------------------------ apply_fill

fn apply_fill_case(name: &str, pos: Option<Position>, side: Side, dq: u64, price: u64, t: i64, fee_day_now: i64) -> Value {
    let out = apply_fill(pos, side, dq, price, t, fee_day_now);
    json!({"name": name,
        "in": {"pos": opt_pos(&pos), "side": side_name(side), "dq": n(dq), "price_e12": n(price), "t": n(t), "fee_day_now": n(fee_day_now)},
        "out": res(out, |o| json!({
            "position": opt_pos(&o.position),
            "closed": o.closed.map(|c| json!({"qty_abs": n(c.qty_abs), "entry_e12": n(c.entry_e12), "exit_e12": n(c.exit_e12), "pnl": n(c.pnl), "quick": c.quick, "credit": n(c.credit)})).unwrap_or(Value::Null),
            "opened": o.opened.map(|c| json!({"qty": n(c.qty), "price_e12": n(c.price_e12)})).unwrap_or(Value::Null),
            "balance_delta": n(o.balance_delta),
            "quick_tally_delta": n(o.quick_tally_delta)}))})
}

fn rand_pos(r: &mut Rng, base_t: i64) -> Position {
    let qty = r.range(1_000, 3_000_000_000_000) * if r.chance(500_000) { 1 } else { -1 };
    let price = if r.chance(700_000) { r.range(900_000_000_000, 1_400_000_000_000) } else { r.range(1_000_000_000, 90_000_000_000_000_000) };
    let timer = base_t - r.range(0, 400);
    Position { qty, avg_entry_e12: price as u64, timer_start: timer, opened_at: timer - r.range(0, 4000), fee_day: engine::day_index(timer) }
}

fn apply_fill_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let t = 1_791_813_600i64;
    let fd = engine::day_index(t);
    let p = |qty: i64, avg: u64, timer: i64| Position { qty, avg_entry_e12: avg, timer_start: timer, opened_at: timer, fee_day: fd };
    // hand-written: DESIGN 7.5 and the edges
    v.push(apply_fill_case("open long from flat", None, Side::Buy, 1_000_000_000_000, 1_080_032_400_000, t, fd));
    v.push(apply_fill_case("open short from flat", None, Side::Sell, 500_000, 1_080_000_000_000, t, fd));
    v.push(apply_fill_case("merge long rounds up (7.5)", Some(p(3_000_000_000_000, 1_100_000_000_000, t)), Side::Buy, 4_000_000_000_000, 1_130_000_000_000, t + 100, fd));
    v.push(apply_fill_case("merge short rounds down", Some(p(-3_000_000_000_000, 1_100_000_000_000, t)), Side::Sell, 4_000_000_000_000, 1_130_000_000_000, t + 100, fd));
    v.push(apply_fill_case("partial reduce (7.5)", Some(p(700_000_000_000, 1_117_142_857_143, t)), Side::Sell, 300_000_000_000, 1_120_000_000_000, t + 100, fd));
    v.push(apply_fill_case("flip (7.5)", Some(p(1_000_000_000_000, 1_080_032_400_000, t)), Side::Sell, 1_500_000_000_000, 1_082_000_000_000, t + 300, fd));
    v.push(apply_fill_case("flip, quick", Some(p(1_000_000_000_000, 1_080_032_400_000, t)), Side::Sell, 1_500_000_000_000, 1_082_000_000_000, t + 30, fd));
    for age in [44, 45, 46] {
        v.push(apply_fill_case(&format!("close with a profit at age {age}"), Some(p(1_000_000_000_000, 1_000_000_000_000, t)), Side::Sell, 1_000_000_000_000, 1_001_000_000_000, t + age, fd));
    }
    v.push(apply_fill_case("quick loss is credited in full", Some(p(1_000_000_000_000, 1_000_000_000_000, t)), Side::Sell, 1_000_000_000_000, 999_000_000_000, t + 10, fd));
    v.push(apply_fill_case("odd quick profit floors", Some(p(1, 1_000_000_000_000, t)), Side::Sell, 1_000_000, 1_000_003_000_000, t + 10, fd));
    v.push(apply_fill_case("exact flatten", Some(p(-250_000, 2_000_000_000_000, t)), Side::Buy, 250_000, 1_900_000_000_000, t + 90, fd));
    v.push(apply_fill_case("zero quantity", None, Side::Buy, 0, 1, t, fd));
    v.push(apply_fill_case("quantity above i64", None, Side::Buy, u64::MAX, 1, t, fd));
    v.push(apply_fill_case("zero price", None, Side::Buy, 1, 0, t, fd));
    v.push(apply_fill_case("time before the timer", Some(p(1_000, 1_000_000_000_000, t)), Side::Sell, 1_000, 1_000_000_000_000, t - 1, fd));
    v.push(apply_fill_case("merge overflows the quantity", Some(p(i64::MAX - 5, 1_000_000_000_000, t)), Side::Buy, 10, 1_000_000_000_000, t + 1, fd));
    v.push(apply_fill_case("short may not reach i64::MIN", Some(p(-(i64::MAX - 5), 1_000_000_000_000, t)), Side::Sell, 6, 1_000_000_000_000, t + 1, fd));
    v.push(apply_fill_case("pnl overflow", Some(p(i64::MAX, 1, t)), Side::Sell, i64::MAX as u64, u64::MAX, t + 100, fd));
    v.push(apply_fill_case("stored position with zero quantity", Some(p(0, 1_000_000_000_000, t)), Side::Buy, 1, 1_000_000_000_000, t, fd));
    // random
    let mut r = Rng::new(0x5EED_0001);
    for i in 0..560 {
        let pos = if r.chance(250_000) { None } else { Some(rand_pos(&mut r, t)) };
        let side = if r.chance(500_000) { Side::Buy } else { Side::Sell };
        let dq = match (r.range(0, 9), &pos) {
            (0, _) => r.range(1, 1_000) as u64,
            (1..=3, Some(p)) => p.qty.unsigned_abs(), // exactly flat
            (4..=5, Some(p)) => p.qty.unsigned_abs() / 2 + 1,
            (6..=7, Some(p)) => p.qty.unsigned_abs() + r.range(1, 1_000_000_000) as u64, // flip
            _ => r.range(1_000, 4_000_000_000_000) as u64,
        };
        let price = pos.map(|p| p.avg_entry_e12).filter(|_| r.chance(800_000)).map(|a| (i128::from(a) + i128::from(r.range(-30_000_000_000, 30_000_000_000))).max(1) as u64).unwrap_or_else(|| r.range(900_000_000_000, 1_400_000_000_000) as u64);
        let tt = t + r.range(0, 120);
        v.push(apply_fill_case(&format!("random {i}"), pos, side, dq, price, tt, engine::day_index(tt)));
    }
    v
}

// ------------------------------------------------------------------------------------------------ pnl, credit, claims

fn pnl_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let mut r = Rng::new(0x5EED_0002);
    let mut push = |name: String, long: bool, qty: u64, entry: u64, exit: u64| {
        let pnl = pnl_micro(long, qty, entry, exit);
        v.push(json!({"name": name, "in": {"long": long, "qty_abs": n(qty), "entry_e12": n(entry), "exit_e12": n(exit)}, "out": res(pnl, n)}));
    };
    push("DESIGN 4.9 breach close".into(), true, 1_000_000_000_000, 1_080_032_400_000, 1_077_967_660_000);
    push("floor rounds toward minus infinity (loss)".into(), true, 1, 1_000_000_000_001, 1_000_000_000_000);
    push("floor rounds toward minus infinity (profit)".into(), true, 1, 1_000_000_000_000, 1_000_000_000_001);
    push("short profit".into(), false, 1_000_000, 1_000_000_000_000, 900_000_000_000);
    push("zero move".into(), true, 123_456_789, 1_234_567_890_123, 1_234_567_890_123);
    push("overflow".into(), true, u64::MAX, 1, u64::MAX);
    for i in 0..170 {
        let long = r.chance(500_000);
        let entry = r.range(1_000_000_000, 90_000_000_000_000_000) as u64;
        let exit = (i128::from(entry) + i128::from(r.range(-500_000_000_000, 500_000_000_000))).max(1) as u64;
        push(format!("random {i}"), long, r.range(1, 5_000_000_000_000) as u64, entry, exit);
    }
    // slice_credit
    let mut credit = Vec::new();
    for (pnl, quick) in [(0i64, true), (0, false), (1, true), (2, true), (3, true), (1_000, true), (1_000, false), (-1, true), (-3, true), (-1_000_000, true), (-1_000_000, false), (i64::MAX, true), (i64::MIN, true), (i64::MIN, false)] {
        let (c, t) = slice_credit(pnl, quick);
        credit.push(json!({"name": format!("pnl {pnl} quick {quick}"), "in": {"pnl": n(pnl), "quick": quick}, "out": {"credit": n(c), "quick_tally_delta": n(t)}}));
    }
    v.extend(credit.into_iter().map(|c| {
        let mut c = c;
        c["kind"] = json!("slice_credit");
        c
    }));
    v
}

fn fees_and_margin_cases() -> (Vec<Value>, Vec<Value>) {
    let mut margin = Vec::new();
    let mut fees = Vec::new();
    let mut r = Rng::new(0x5EED_0003);
    let m = |name: String, qty: u64, price: u64, lev: u16| json!({"name": name, "in": {"qty_abs": n(qty), "price_e12": n(price), "leverage_x": n(lev)}, "out": res(margin_micro(qty, price, lev), n)});
    margin.push(m("DESIGN 4.9: 1,000,000 EUR at 1.0800324 / 50".into(), 1_000_000_000_000, 1_080_032_400_000, 50));
    margin.push(m("7.5 flip remainder".into(), 500_000_000_000, 1_082_000_000_000, 50));
    margin.push(m("ceiling".into(), 1, 1, 1));
    margin.push(m("zero leverage".into(), 1, 1, 0));
    margin.push(m("zero quantity".into(), 0, 1_000_000_000_000, 5));
    margin.push(m("overflow".into(), u64::MAX, u64::MAX, 1));
    for i in 0..190 {
        margin.push(m(format!("random {i}"), r.range(1, 9_000_000_000_000) as u64, r.range(1_000_000_000, 90_000_000_000_000_000) as u64, r.pick(&[1u16, 2, 5, 10, 20, 50])));
    }
    // fee_ceil_micro, holding_fee_micro, swap_fee_micro, day_index, nights_due
    for i in 0..60 {
        let notional = r.range(0, 4_000_000_000_000);
        let rate = r.pick(&[0u32, 1, 20, 30, 100, 1000, 1600, 5000]);
        fees.push(json!({"kind": "fee_ceil_micro", "name": format!("random {i}"), "in": {"notional_micro": n(notional), "rate_e6": n(rate)}, "out": res(fee_ceil_micro(notional, rate), n)}));
    }
    fees.push(json!({"kind": "fee_ceil_micro", "name": "negative notional", "in": {"notional_micro": n(-1), "rate_e6": n(100)}, "out": res(fee_ceil_micro(-1, 100), n)}));
    for i in 0..80 {
        let qty = r.range(1, 9_000_000_000_000) as u64;
        let mark = r.range(1_000_000_000, 90_000_000_000_000_000) as u64;
        let rate = r.pick(&[0u32, 20, 100, 200, 400, 1000, 1600]);
        let nights = r.range(1, 4) as u32;
        fees.push(json!({"kind": "holding_fee_micro", "name": format!("random {i}"), "in": {"qty_abs": n(qty), "mark_e12": n(mark), "rate_e6": n(rate), "nights": n(nights)}, "out": res(holding_fee_micro(qty, mark, rate, nights), n)}));
    }
    fees.push(json!({"kind": "holding_fee_micro", "name": "DESIGN 9.3: 0.01% of 1,000,000 EUR at 1.0805", "in": {"qty_abs": n(1_000_000_000_000u64), "mark_e12": n(1_080_500_000_000u64), "rate_e6": n(100), "nights": n(1)}, "out": res(holding_fee_micro(1_000_000_000_000, 1_080_500_000_000, 100, 1), n)}));
    for tier_cost in [30_000_000u64, 55_000_000, 99_000_000, 145_000_000, 189_000_000, 235_000_000, 489_000_000, 689_000_000, 859_000_000, 1_599_000_000] {
        fees.push(json!({"kind": "swap_fee_micro", "name": format!("cost {tier_cost}"), "in": {"tier_cost_micro": n(tier_cost), "fee_bps": n(40)}, "out": res(swap_fee_micro(tier_cost, 40), n)}));
    }
    for t in [0i64, 79_199, 79_200, 79_201, 86_399, 86_400, 1_791_813_600, 1_791_842_399, 1_791_842_400, -1, -79_200, -79_201, i64::MIN, i64::MAX] {
        fees.push(json!({"kind": "day_index", "name": format!("t {t}"), "in": {"unix_secs": n(t)}, "out": n(day_index(t))}));
    }
    for (fd, today) in [(10i64, 10i64), (10, 11), (10, 13), (11, 10), (0, 5_000_000_000), (-5, 5)] {
        fees.push(json!({"kind": "nights_due", "name": format!("fee_day {fd} today {today}"), "in": {"fee_day": n(fd), "today_index": n(today)}, "out": res(nights_due(fd, today), n)}));
    }
    for (q, step, min) in [(1_000u64, 1_000u64, 1_000u64), (1_500, 1_000, 1_000), (0, 1_000, 1_000), (999, 1, 1_000), (5_000, 0, 1), (10_000_000, 1_000, 10_000_000), (9_999_000, 1_000, 10_000_000)] {
        fees.push(json!({"kind": "validate_quantity", "name": format!("qty {q} step {step} min {min}"), "in": {"qty": n(q), "qty_step": n(step), "qty_min": n(min)}, "out": res(validate_quantity(q, step, min), |_| Value::Null)}));
    }
    (margin, fees)
}

// ------------------------------------------------------------------------------------------------ prices and triggers

fn price_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let mut r = Rng::new(0x5EED_0004);
    for (mid, h) in [(1_080_000_000_000u64, 30u32), (1_078_000_000_000, 30), (1, 1), (u64::MAX, 0), (u64::MAX, 1), (1_000_000_000_000, 1_000_000), (1_000_000_000_000, 1_000_001)] {
        v.push(json!({"kind": "ask_from_mid", "name": format!("mid {mid} h {h}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h)}, "out": res(ask_from_mid(mid, h), n)}));
        v.push(json!({"kind": "bid_from_mid", "name": format!("mid {mid} h {h}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h)}, "out": res(bid_from_mid(mid, h), n)}));
    }
    for i in 0..60 {
        let mid = r.range(1_000_000, 90_000_000_000_000_000) as u64;
        let h = r.pick(&[0u32, 1, 10, 30, 60, 100, 1000]);
        v.push(json!({"kind": "ask_from_mid", "name": format!("random {i}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h)}, "out": res(ask_from_mid(mid, h), n)}));
        v.push(json!({"kind": "bid_from_mid", "name": format!("random {i}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h)}, "out": res(bid_from_mid(mid, h), n)}));
        let long = r.chance(500_000);
        v.push(json!({"kind": "fill_price_e12", "name": format!("random {i}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h), "long": long}, "out": res(fill_price_e12(mid, h, long).map_err(|e| match e { ClaimError::Engine(x) => x, _ => EngineError::InvalidInput }), n)}));
        v.push(json!({"kind": "close_price_e12", "name": format!("random {i}"), "in": {"mid_e12": n(mid), "half_spread_e6": n(h), "long": long}, "out": res(close_price_e12(mid, h, long).map_err(|e| match e { ClaimError::Engine(x) => x, _ => EngineError::InvalidInput }), n)}));
    }
    for val in [0u128, 999_999, 1_000_000, 1_080_000_000_000_000_000, 1_079_999_999_999_999_999, u128::MAX] {
        v.push(json!({"kind": "price_from_oracle", "name": format!("value {val}"), "in": {"value_e18": n(val)}, "out": res(price_from_oracle(val), n)}));
    }
    v
}

fn trigger_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let mut r = Rng::new(0x5EED_0005);
    for i in 0..110 {
        let side = if r.chance(500_000) { Side::Buy } else { Side::Sell };
        let bid = r.range(900_000_000_000, 1_100_000_000_000) as u64;
        let ask = bid + r.range(0, 100_000_000) as u64;
        let pick_level = |r: &mut Rng, base: u64| -> u64 {
            match r.range(0, 5) {
                0 => 0,
                1 => base,
                2 => base + 1,
                3 => base.saturating_sub(1),
                _ => (i128::from(base) + i128::from(r.range(-50_000_000_000, 50_000_000_000))).max(1) as u64,
            }
        };
        let (sl, tp) = (pick_level(&mut r, if side == Side::Buy { bid } else { ask }), pick_level(&mut r, if side == Side::Buy { bid } else { ask }));
        let open = r.chance(850_000);
        let out = check_protective(side, sl, tp, bid, ask, open);
        v.push(json!({"kind": "check_protective", "name": format!("random {i}"), "in": {"position_side": side_name(side), "sl_e12": n(sl), "tp_e12": n(tp), "bid_e12": n(bid), "ask_e12": n(ask), "market_open": open},
            "out": out.map(|t| json!({"kind": format!("{:?}", t.kind), "price_e12": n(t.price_e12)})).unwrap_or(Value::Null)}));
    }
    for i in 0..110 {
        let side = if r.chance(500_000) { Side::Buy } else { Side::Sell };
        let kind = if r.chance(500_000) { PendingKind::Limit } else { PendingKind::Stop };
        let bid = r.range(900_000_000_000, 1_100_000_000_000) as u64;
        let ask = bid + r.range(0, 100_000_000) as u64;
        let base = if side == Side::Buy { ask } else { bid };
        let level = match r.range(0, 4) {
            0 => 0,
            1 => base,
            2 => base + 1,
            3 => base.saturating_sub(1),
            _ => (i128::from(base) + i128::from(r.range(-50_000_000_000, 50_000_000_000))).max(1) as u64,
        };
        let (open, locked) = (r.chance(900_000), r.chance(150_000));
        let out = check_pending(side, kind, level, bid, ask, open, locked);
        v.push(json!({"kind": "check_pending", "name": format!("random {i}"), "in": {"order_side": side_name(side), "order_kind": format!("{kind:?}"), "level_e12": n(level), "bid_e12": n(bid), "ask_e12": n(ask), "market_open": open, "locked": locked},
            "out": out.map(n).unwrap_or(Value::Null)}));
    }
    v
}

// ------------------------------------------------------------------------------------------------ evaluate_sample

fn eval_case(name: String, st: AccountState, p_open: i64, comm: i64, now: i64) -> Value {
    let d = evaluate_sample(&st, &Rules::default(), p_open, comm, now);
    json!({"name": name, "in": {"state": state_json(&st), "p_open_micro": n(p_open), "closing_commission_micro": n(comm), "now": n(now)},
        "out": res(d, |d| json!({"action": action_json(&d.action), "state": state_json(&d.state), "equity_micro": n(d.equity_micro)}))})
}

fn eval_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let sizes = [2_500i64, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
    let mut r = Rng::new(0x5EED_0006);
    let now = 1_791_813_600i64;
    // the DESIGN 4.9 and R1 rows, then every boundary of a $100,000 account, then random states
    let s = 100_000_000_000i64;
    let fresh = AccountState::new_phase(s);
    v.push(eval_case("4.9 tight start".into(), fresh, 1_235_161_000, 0, now));
    let started = AccountState { tight_on: true, r: s, l_start: 96_000_000_000, g_peak: 1_235_161_000, l_level: 96_235_161_000, ..fresh };
    v.push(eval_case("4.9 raise".into(), started, 3_035_107_000, 0, now));
    let raised = AccountState { g_peak: 3_035_107_000, l_level: 98_035_107_000, ..started };
    v.push(eval_case("4.9 nothing at +1,035".into(), raised, 1_035_170_000, 0, now));
    v.push(eval_case("4.9 breach".into(), raised, -2_064_740_000, 0, now));
    v.push(eval_case("4.9 late sample: fail".into(), raised, -20_064_200_000, 0, now));
    for (label, p) in [("E == F_s fails", -8_000_000_000i64), ("E == F_s + 1 does not", -7_999_999_999), ("P == 1% is not tight mode", 1_000_000_000), ("P == 1% + 1 starts it", 1_000_000_001)] {
        v.push(eval_case(format!("fresh $100k: {label}"), fresh, p, 0, now));
    }
    for (label, p) in [("E == L breaches", -1_964_893_000i64), ("E == L + 1 does not", -1_964_892_999)] {
        v.push(eval_case(format!("tight on, L = 98,035.107: {label}"), raised, p, 0, now));
    }
    let mut i = 0;
    while v.len() < 600 {
        i += 1;
        let size = sizes[r.range(0, 9) as usize] * 1_000_000;
        let latch = r.chance(250_000);
        let f_s = size / 100 * if latch { 96 } else { 92 };
        let b = (size + r.range(-7, 12) * size / 100 + r.range(-1_000_000, 1_000_000)).max(f_s + 1);
        let quick_q = r.range(0, size / 100 * 3);
        let mut st = AccountState { size_micro: size, b, latch: latch || b >= size / 100 * 104, q_tally: quick_q, lock_until: if r.chance(200_000) { now + r.range(1, 40_000) } else { 0 }, ..AccountState::new_phase(size) };
        let f_s = static_floor_micro(&st).unwrap();
        if r.chance(500_000) {
            let g = r.range(0, size / 100 * 8);
            let l_start = (f_s + size / 100 * 4).min(b);
            st = AccountState { tight_on: true, r: b, l_start, g_peak: g, l_level: l_start + (g - size / 100).max(0), flat: false, ..st };
        }
        let comm = if r.chance(300_000) { r.range(0, 5_000_000) } else { 0 };
        // aim at a boundary half of the time
        let eff = effective_floor_micro(&st).unwrap();
        let p = match r.range(0, 7) {
            0 => f_s - st.b + comm + r.range(-1, 1),
            1 => eff - st.b + comm + r.range(-1, 1),
            2 => size / 100 + r.range(-1, 1),
            3 if st.tight_on => st.g_peak + st.r - st.b + comm + r.range(-2, 2),
            _ => r.range(-size / 100 * 12, size / 100 * 10),
        };
        v.push(eval_case(format!("random {i}"), st, p, comm, now + r.range(0, 100)));
    }
    v
}

fn claim_cases() -> Vec<Value> {
    let mut v = Vec::new();
    let mut r = Rng::new(0x5EED_0007);
    let s = 100_000_000_000i64;
    for i in 0..40 {
        let long = r.chance(500_000);
        let qty = r.range(1, 3_000_000_000_000) as u64;
        let entry = r.range(900_000_000_000, 1_400_000_000_000) as u64;
        let exit = (i128::from(entry) + i128::from(r.range(-30_000_000_000, 30_000_000_000))).max(1) as u64;
        let time = 1_791_813_600i64;
        let timer = time - r.range(0, 120);
        let pnl = pnl_micro(long, qty, entry, exit).unwrap();
        let quick = time - timer <= 45;
        let (credited, _) = slice_credit(pnl, quick);
        let (bad_pnl, bad_quick, bad_credit) = match i % 4 {
            1 => (pnl + 1, quick, credited),
            2 => (pnl, !quick, credited),
            3 => (pnl, quick, credited + 1),
            _ => (pnl, quick, credited),
        };
        let claim = CloseClaim { long, qty, entry_e12: entry, exit_e12: exit, time, timer_start: timer, pnl: bad_pnl, quick: bad_quick, credited: bad_credit };
        v.push(json!({"kind": "check_close", "name": format!("random {i}"),
            "in": {"long": long, "qty": n(qty), "entry_e12": n(entry), "exit_e12": n(exit), "time": n(time), "timer_start": n(timer), "pnl": n(bad_pnl), "quick": bad_quick, "credited": n(bad_credit)},
            "out": match check_close(&claim) { Ok(c) => json!({"ok": {"credit": n(c.credit), "tally_delta": n(c.tally_delta)}}), Err(e) => json!({"err": format!("{e:?}")}) }}));
    }
    let fresh = AccountState::new_phase(s);
    for (label, p_open, r_, l_start, g, l) in [
        ("exact", 1_235_161_000i64, s, 96_000_000_000i64, 1_235_161_000i64, 96_235_161_000i64),
        ("p_open at 1%", 1_000_000_000, s, 96_000_000_000, 1_000_000_000, 96_000_000_000),
        ("r wrong", 1_235_161_000, s + 1, 96_000_000_000, 1_235_161_000, 96_235_161_000),
        ("l_start wrong", 1_235_161_000, s, 96_000_001, 1_235_161_000, 96_235_161_000),
        ("l_level wrong", 1_235_161_000, s, 96_000_000_000, 1_235_161_000, 96_235_160_999),
        ("g_peak above p_open", 1_235_161_000, s, 96_000_000_000, 1_235_161_001, 96_235_161_001),
        ("g_peak negative", 1_235_161_000, s, 96_000_000_000, -1, 96_000_000_000),
    ] {
        v.push(json!({"kind": "check_tight_start", "name": label, "in": {"state": state_json(&fresh), "p_open": n(p_open), "r": n(r_), "l_start": n(l_start), "g_peak": n(g), "l_level": n(l)},
            "out": match check_tight_start(&fresh, p_open, r_, l_start, g, l) { Ok(st) => json!({"ok": state_json(&st)}), Err(e) => json!({"err": format!("{e:?}")}) }}));
    }
    let on = AccountState { tight_on: true, r: s, l_start: 96_000_000_000, g_peak: 1_235_161_000, l_level: 96_235_161_000, ..fresh };
    for (label, g, l) in [("raise", 3_035_107_000i64, 98_035_107_000i64), ("not higher", 1_235_161_000, 96_235_161_000), ("wrong level", 3_035_107_000, 98_035_106_999)] {
        v.push(json!({"kind": "check_floor_raised", "name": label, "in": {"state": state_json(&on), "g_peak": n(g), "l_level": n(l)},
            "out": match check_floor_raised(&on, g, l) { Ok(st) => json!({"ok": state_json(&st)}), Err(e) => json!({"err": format!("{e:?}")}) }}));
    }
    let on_b = AccountState { b: 97_935_260_000, ..on };
    let on_b = AccountState { g_peak: 3_035_107_000, l_level: 98_035_107_000, ..on_b };
    for (label, l, b_after) in [("breach", 98_035_107_000i64, 97_935_260_000i64), ("l_level differs", 98_035_106_000, 97_935_260_000), ("b_after differs", 98_035_107_000, 97_935_259_999), ("b_after above L", 98_035_107_000, 98_035_108_000)] {
        let st = AccountState { b: if label == "b_after above L" { 98_035_108_000 } else { on_b.b }, ..on_b };
        v.push(json!({"kind": "check_breach", "name": label, "in": {"state": state_json(&st), "time": n(1_791_813_750i64), "l_level": n(l), "b_after": n(b_after)},
            "out": match check_breach(&st, 1_791_813_750, l, b_after) { Ok(st) => json!({"ok": state_json(&st)}), Err(e) => json!({"err": format!("{e:?}")}) }}));
    }
    v
}

// ------------------------------------------------------------------------------------------------ hash chain and batch encoding

fn event_json(e: &JournalEvent) -> Value {
    use JournalEvent as E;
    match e {
        E::Fill { market, side, order_kind, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, proof } => json!({"type": "Fill", "market": n(market), "side": n(side.as_u8()), "order_kind": n(*order_kind as u8), "qty": n(qty), "price_e12": n(price_e12), "time": n(time), "avg_entry_after": n(avg_entry_after), "margin": n(margin), "sl": n(sl), "tp": n(tp), "intent_id": n(intent_id), "proof": n(proof)}),
        E::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason, intent_id, proof } => json!({"type": "Close", "market": n(market), "side": n(side.as_u8()), "qty": n(qty), "entry_e12": n(entry_e12), "exit_e12": n(exit_e12), "time": n(time), "timer_start": n(timer_start), "pnl": n(pnl), "quick": quick, "credited": n(credited), "reason": n(reason.as_u8()), "intent_id": n(intent_id), "proof": n(proof)}),
        E::Fee { kind, market, amount, time } => json!({"type": "Fee", "kind": n(kind.as_u8()), "market": n(market), "amount": n(amount), "time": n(time)}),
        E::TightStart { time, p_open, r, l_start, g_peak, l_level, proof } => json!({"type": "TightStart", "time": n(time), "p_open": n(p_open), "r": n(r), "l_start": n(l_start), "g_peak": n(g_peak), "l_level": n(l_level), "proof": n(proof)}),
        E::FloorRaised { time, g_peak, l_level, proof } => json!({"type": "FloorRaised", "time": n(time), "g_peak": n(g_peak), "l_level": n(l_level), "proof": n(proof)}),
        E::TightEnd { time } => json!({"type": "TightEnd", "time": n(time)}),
        E::Breach { time, l_level, b_after, proof } => json!({"type": "Breach", "time": n(time), "l_level": n(l_level), "b_after": n(b_after), "proof": n(proof)}),
        E::Fail { time, reason, b_after, proof } => json!({"type": "Fail", "time": n(time), "reason": n(reason.as_u8()), "b_after": n(b_after), "proof": n(proof)}),
        E::Meta { time, flat, samples_hash, intents_hash, engine_version } => json!({"type": "Meta", "time": n(time), "flat": flat, "samples_hash": hexv(samples_hash), "intents_hash": hexv(intents_hash), "engine_version": n(engine_version)}),
    }
}

fn proof_json(p: &PriceProof) -> Value {
    json!({"source": n(p.source), "feed_id": hexv(&p.feed_id), "timestamp_ns": n(p.timestamp_ns), "quantized_value": n(p.quantized_value), "signed_update": hexv(&p.signed_update)})
}

fn rand_event(r: &mut Rng, with_proof: bool, n_proofs: usize) -> JournalEvent {
    use journal_types::{CloseReason, FailReason, FeeKind, OrderKind, Side};
    let proof = if with_proof && n_proofs > 0 { r.range(0, n_proofs as i64 - 1) as u8 } else { journal_types::NO_PROOF };
    let time = 1_791_813_600 + r.range(0, 100_000);
    match r.range(0, 8) {
        0 => JournalEvent::Fill { market: r.range(0, 5) as u16, side: if r.chance(500_000) { Side::Long } else { Side::Short }, order_kind: OrderKind::try_from(r.range(0, 2) as u8).unwrap(), qty: r.range(1_000, 9_000_000_000_000) as u64, price_e12: r.range(1_000_000_000, 9_000_000_000_000_000) as u64, time, avg_entry_after: r.range(1_000_000_000, 9_000_000_000_000_000) as u64, margin: r.range(0, 90_000_000_000) as u64, sl: r.range(0, 2_000_000_000_000) as u64, tp: r.range(0, 2_000_000_000_000) as u64, intent_id: r.range(0, 5000) as u32, proof },
        1 => JournalEvent::Close { market: r.range(0, 5) as u16, side: if r.chance(500_000) { Side::Long } else { Side::Short }, qty: r.range(1_000, 9_000_000_000_000) as u64, entry_e12: r.range(1_000_000_000, 9_000_000_000_000_000) as u64, exit_e12: r.range(1_000_000_000, 9_000_000_000_000_000) as u64, time, timer_start: time - r.range(0, 5000), pnl: r.range(-5_000_000_000, 5_000_000_000), quick: r.chance(300_000), credited: r.range(-5_000_000_000, 5_000_000_000), reason: CloseReason::try_from(r.range(0, 5) as u8).unwrap(), intent_id: r.range(0, 5000) as u32, proof },
        2 => JournalEvent::Fee { kind: FeeKind::try_from(r.range(0, 1) as u8).unwrap(), market: r.range(0, 5) as u16, amount: r.range(0, 900_000_000) as u64, time },
        3 => JournalEvent::TightStart { time, p_open: r.range(1_000_000_000, 9_000_000_000), r: r.range(90_000_000_000, 110_000_000_000), l_start: r.range(90_000_000_000, 100_000_000_000), g_peak: r.range(0, 9_000_000_000), l_level: r.range(90_000_000_000, 110_000_000_000), proof },
        4 => JournalEvent::FloorRaised { time, g_peak: r.range(0, 9_000_000_000), l_level: r.range(90_000_000_000, 110_000_000_000), proof },
        5 => JournalEvent::TightEnd { time },
        6 => JournalEvent::Breach { time, l_level: r.range(90_000_000_000, 110_000_000_000), b_after: r.range(90_000_000_000, 110_000_000_000), proof },
        7 => JournalEvent::Fail { time, reason: FailReason::try_from(r.range(0, 1) as u8).unwrap(), b_after: r.range(60_000_000_000, 100_000_000_000), proof },
        _ => JournalEvent::Meta { time, flat: r.chance(500_000), samples_hash: (0..32).map(|_| r.range(0, 255) as u8).collect::<Vec<_>>().try_into().unwrap(), intents_hash: (0..32).map(|_| r.range(0, 255) as u8).collect::<Vec<_>>().try_into().unwrap(), engine_version: 1 },
    }
}

fn rand_proof(r: &mut Rng) -> PriceProof {
    let source = r.pick(&[2u8, 3, 4]);
    PriceProof {
        source,
        feed_id: (0..32).map(|_| r.range(0, 255) as u8).collect::<Vec<_>>().try_into().unwrap(),
        timestamp_ns: (1_791_813_600u64 + r.range(0, 100_000) as u64) * 1_000_000_000 + r.range(0, 999_999_999) as u64,
        quantized_value: i128::from(r.range(1_000_000_000, 9_000_000_000_000_000)) * 1_000_000 + i128::from(r.range(0, 999_999)),
        signed_update: if source == 3 { (0..32).map(|_| r.range(0, 255) as u8).collect() } else { vec![] },
    }
}

fn chain_and_encoding_cases() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    use evaluator::sha256::{chain_hasher, sha256};
    let h = chain_hasher();
    let mut sha = Vec::new();
    for s in ["", "abc", "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", "SL8LEV", "The quick brown fox jumps over the lazy dog"] {
        sha.push(json!({"name": format!("{s:?}"), "in": {"data": hexv(s.as_bytes())}, "out": hexv(&sha256(s.as_bytes()))}));
    }
    let mut r = Rng::new(0x5EED_0008);
    for i in 0..30 {
        let len = r.range(0, 300) as usize;
        let data: Vec<u8> = (0..len).map(|_| r.range(0, 255) as u8).collect();
        sha.push(json!({"name": format!("random {i}"), "in": {"data": hexv(&data)}, "out": hexv(&sha256(&data))}));
    }
    let mut chain = Vec::new();
    for i in 0..5u8 {
        let acct = [i.wrapping_mul(37).wrapping_add(1); 32];
        chain.push(json!({"kind": "genesis_hash", "name": format!("account {i}"), "in": {"account": hexv(&acct)}, "out": hexv(&journal_types::genesis_hash(&h, &acct))}));
    }
    let mut encoding = Vec::new();
    // feed ids
    for (src, sym) in [(2u8, "BTC"), (4, "xyz:AAPL"), (2, "EURUSD"), (3, "ETH")] {
        chain.push(json!({"kind": "feed_id", "name": format!("{src} {sym}"), "in": {"source": n(src), "symbol": sym}, "out": hexv(&journal_types::feed_id(&h, src, sym))}));
    }
    // single steps: next_hash for each event with and without a proof
    for i in 0..60 {
        let proof = rand_proof(&mut r);
        let with_proof = r.chance(600_000);
        let ev = rand_event(&mut r, with_proof, 1);
        let needs = ev.proof_index().is_some();
        let prev: [u8; 32] = (0..32).map(|_| r.range(0, 255) as u8).collect::<Vec<_>>().try_into().unwrap();
        let seq = r.range(1, 1_000_000) as u64;
        let pr = if needs { Some(&proof) } else { None };
        chain.push(json!({"kind": "next_hash", "name": format!("random {i}"), "in": {"prev": hexv(&prev), "seq": n(seq), "event": event_json(&ev), "event_bytes": hexv(&ev.to_bytes()), "proof": pr.map(proof_json).unwrap_or(Value::Null), "proof_bytes": pr.map(|p| hexv(&p.to_bytes().unwrap())).unwrap_or(Value::Null)},
            "out": match journal_types::next_hash(&h, &prev, seq, &ev, pr) { Ok(x) => hexv(&x), Err(e) => Value::String(format!("{e:?}")) }}));
    }
    // whole batches: encoding and apply_batch
    for i in 0..110 {
        let n_events = r.range(1, 8) as usize;
        let n_proofs = r.range(0, 3) as usize;
        let proofs: Vec<PriceProof> = (0..n_proofs).map(|_| rand_proof(&mut r)).collect();
        let mut events: Vec<JournalEvent> = (0..n_events - 1).map(|_| rand_event(&mut r, true, n_proofs)).filter(|e| !matches!(e, JournalEvent::Meta { .. })).collect();
        events.push(rand_event(&mut r, false, 0));
        while !matches!(events.last(), Some(JournalEvent::Meta { .. })) {
            events.pop();
            events.push(JournalEvent::Meta { time: 1_791_813_600, flat: true, samples_hash: [i as u8; 32], intents_hash: [255 - i as u8; 32], engine_version: 1 });
        }
        let first_seq = r.range(1, 100_000) as u64;
        let prev: [u8; 32] = (0..32).map(|_| r.range(0, 255) as u8).collect::<Vec<_>>().try_into().unwrap();
        let batch = JournalBatch { first_seq, prev_hash: prev, events, proofs };
        let bytes = batch.to_bytes();
        encoding.push(json!({"kind": "batch", "name": format!("random {i}"),
            "in": {"first_seq": n(first_seq), "prev_hash": hexv(&prev), "events": batch.events.iter().map(event_json).collect::<Vec<_>>(), "proofs": batch.proofs.iter().map(proof_json).collect::<Vec<_>>()},
            "out": match &bytes { Ok(b) => json!({"bytes": hexv(b)}), Err(e) => json!({"err": format!("{e:?}")}) }}));
        if let Ok(batch_bytes) = &bytes {
            let r_ = journal_types::apply_batch(&h, &prev, first_seq, &batch);
            chain.push(json!({"kind": "apply_batch", "name": format!("random {i}"), "in": {"prev": hexv(&prev), "first_seq": n(first_seq), "batch_bytes": hexv(batch_bytes)},
                "out": match r_ { Ok((head, last)) => json!({"head": hexv(&head), "last_seq": n(last)}), Err(e) => json!({"err": format!("{e:?}")}) }}));
        }
    }
    for i in 0..30 {
        let p = rand_proof(&mut r);
        encoding.push(json!({"kind": "proof", "name": format!("random {i}"), "in": proof_json(&p), "out": {"bytes": hexv(&p.to_bytes().unwrap()), "digest": hexv(&journal_types::proof_digest(&h, &p).unwrap())}}));
    }
    (sha, chain, encoding)
}

// ------------------------------------------------------------------------------------------------ full scenarios

struct Sc {
    name: &'static str,
    description: &'static str,
    world: World,
    steps: Vec<Value>,
    batches: Vec<Batch>,
}

impl Sc {
    fn new(name: &'static str, description: &'static str, world: World) -> Sc {
        Sc { name, description, world, steps: Vec::new(), batches: Vec::new() }
    }
    fn sample(&mut self, m: u16, sec: i64, mid: u64) {
        let ts = (sec as u64) * 1_000_000_000;
        let value = i128::from(mid) * 1_000_000;
        self.steps.push(json!({"op": "sample", "market": n(m), "ts_ns": n(ts), "value": n(value), "recv_ns": n(ts + 1_000_000), "digest": hexv(&[0u8; 32])}));
        self.world.ev.on_sample(m, ts, value, ts + 1_000_000, [0; 32]).unwrap();
    }
    fn order(&mut self, nonce: u32, intent: OrderIntent) {
        let s = sign(&self.world.opened.account, nonce, &intent);
        self.steps.push(json!({"op": "order", "nonce": n(nonce), "order": hexv(&s.order), "sig": hexv(&s.sig), "expect": "accepted"}));
        self.world.ev.on_order(&s).unwrap();
    }
    fn tick(&mut self, sec: i64) {
        self.steps.push(json!({"op": "tick", "sec": n(sec)}));
        self.world.ev.tick(sec);
    }
    fn drain(&mut self, max_events: usize, heartbeat: bool) {
        self.steps.push(json!({"op": "drain", "max_events": n(max_events), "heartbeat": heartbeat}));
        let bs = self.world.ev.drain_batches(&BatchPolicy::MAX.with_max_events(max_events).with_heartbeat(heartbeat));
        self.batches.extend(bs);
    }
    fn to_json(&self) -> Value {
        let w = &self.world;
        let rules = &w.rules;
        json!({
            "name": self.name,
            "description": self.description,
            "rules": {"ruleset_version": n(rules.ruleset_version), "leverage_x": rules.leverage_x.iter().map(n).collect::<Vec<_>>(), "holding_fee_e6": rules.holding_fee_e6.iter().map(n).collect::<Vec<_>>(), "commission_e6": rules.commission_e6.iter().map(n).collect::<Vec<_>>(),
                "markets": rules.markets.iter().map(|m| json!({"id": n(m.id), "class": n(m.class), "source": n(m.source), "feed_id": hexv(&m.feed_id), "max_age_secs": n(m.max_age_secs), "status": n(m.status), "qty_step": n(m.qty_step), "qty_min": n(m.qty_min), "max_notional_micro": n(m.max_notional_micro), "sessions": m.sessions.iter().map(|s| json!([n(s.0), n(s.1)])).collect::<Vec<_>>(), "half_spread_e6": n(m.half_spread_e6)})).collect::<Vec<_>>()},
            "account": {"account": hexv(&w.opened.account), "trader": hexv(&w.opened.trader), "challenge_id": n(w.opened.challenge_id), "size_micro": n(w.opened.size_micro), "phase": n(w.opened.phase), "tier_index": n(w.opened.tier_index), "opened_time": n(w.opened.opened_time)},
            "steps": self.steps,
            "batches": self.batches.iter().map(|b| json!({
                "kind": match b.kind { BatchKind::PostJournal => "post_journal".to_string(), BatchKind::ReportFail(r) => format!("report_fail:{r:?}") },
                "batch_bytes": hexv(&b.batch.to_bytes().unwrap()),
                "markets": b.markets.iter().map(n).collect::<Vec<_>>(),
                "meta_time": n(b.meta_time), "samples_committed": n(b.samples_committed), "intents_committed_bytes": n(b.intents_committed_bytes),
                "tx_bytes": n(b.tx_bytes), "cu": n(b.cu),
                "account_after": {"b": n(b.account_after.b), "q_tally": n(b.account_after.q_tally), "lock_until": n(b.account_after.lock_until), "last_batch_time": n(b.account_after.last_batch_time), "seq": n(b.account_after.seq), "journal_hash": hexv(&b.account_after.journal_hash), "trades_closed": n(b.account_after.trades_closed), "latch": b.account_after.latch, "flat": b.account_after.flat, "tight_on": b.account_after.tight_on, "r": n(b.account_after.r), "g_peak": n(b.account_after.g_peak), "l_level": n(b.account_after.l_level), "failed": b.account_after.failed}
            })).collect::<Vec<_>>(),
            "archive": hexv(w.ev.archive_bytes()),
            "intents": String::from_utf8_lossy(w.ev.intents_bytes()),
        })
    }
}

/// The R1 / breach scenarios (a long of 1 M EUR bought at the ask 1.000001, marks at the bid): the peak is the bid of mid 1.0030,
/// 1_002_998_997_000 (1_003_000_000_000 - 1_003_000), P = +2,997.997, G = 2,997.997, L = 96,000 + (G - 1,000) = 97,997.997. The breach
/// needs E = 100,000 + P <= L, i.e. a bid of 1_002_998_997_000 - 5_000_000_000 = 997_998_997_000 (the same 0.005 under the peak as the
/// 0.998 of the zero-spread version). That bid is floor(997_999_995_000 x (1 - 1e-6)) = 997_999_995_000 - 998_000, so E = L exactly:
/// touching counts, and the breach is not lost to the spread (mid 0.9980 would end 4e-3 USD above L).
const MID_TOUCHING_L: u64 = 997_999_995_000;

fn scenarios() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let t = T_1400;
    // 1. DESIGN 4.9
    let mut s = Sc::new("design_4_9", "DESIGN 4.9: +1.2% opens the tight floor, a new high raises it, the reversal breaches it.", World::base());
    s.sample(0, t, p4(10_800));
    s.order(1, market_order(0, true, 1_000_000_000_000, t));
    for (dt, p) in [(30, 10_806), (60, 10_813), (90, 10_831), (120, 10_811), (150, 10_780)] {
        s.sample(0, t + dt, p4(p));
    }
    s.tick(t + 151);
    s.drain(8, true);
    out.push(s);
    // 2. R1 with the smallest priced market (decision 64: a zero half-spread is untradable)
    let mut s = Sc::new("r1_minimal_spread", "DESIGN 2.6 R1 on the smallest priced market: half-spread 1e-6 (the ask is 1.000001 at mid 1.0, the bid 0.999999).", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, market_order(0, true, MEUR, t));
    for (dt, p) in [(30, 10_006), (60, 10_012), (90, 10_030), (120, 10_010)] {
        s.sample(0, t + dt, p4(p));
    }
    s.sample(0, t + 150, MID_TOUCHING_L);
    s.tick(t + 151);
    s.drain(3, true);
    out.push(s);
    // 3. quick profit with commission
    let mut w = World::new(base_rules(vec![eur_market(0)]), S100K, 2, t - HOUR);
    w.rules.commission_e6[0] = 20;
    let rules = RulesCfg { commission_e6: w.rules.commission_e6, ..w.rules.clone() };
    let w = World::from_params(rules, w.opened.clone());
    let mut s = Sc::new("quick_profit_with_commission", "A 20 s round trip: half the profit is credited, the quick tally grows, commissions follow each fill and close.", w);
    s.sample(0, t, p4(10_800));
    s.order(1, market_order(0, true, 200_000_000_000, t));
    s.sample(0, t + 20, p4(10_830));
    s.order(2, shared_close(t + 20));
    s.sample(0, t + 25, p4(10_830));
    s.tick(t + 30);
    s.drain(8, true);
    out.push(s);
    // 4. holding fee over a weekend
    let mut m = eur_market(0);
    m.max_age_secs = 3_600;
    let mut s = Sc::new("holding_fees_over_a_weekend", "A position held from Friday 21:00 to Monday: three nights are charged.", World::new(base_rules(vec![m]), S100K, 2, t - HOUR));
    let fri = t + 4 * 86_400 + 7 * HOUR;
    s.sample(0, fri, p4(10_800));
    s.order(1, market_order(0, true, 300_000_000_000, fri));
    for k in 1..=60 {
        s.sample(0, fri + k * 3_000, p4(10_800 + (k as u64 % 7)));
    }
    s.tick(fri + 61 * 3_000);
    s.drain(8, true);
    out.push(s);
    // 5. flip
    let mut s = Sc::new("flip_quick", "A long flipped to a short 30 s later: Close(Flip) and Fill under one intent, quick credit.", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, 1_080_032_400_000);
    s.order(1, market_order(0, true, 1_000_000_000_000, t));
    s.sample(0, t + 30, 1_082_000_000_000);
    s.order(2, market_order(0, false, 1_500_000_000_000, t + 30));
    s.sample(0, t + 40, 1_082_000_000_000);
    s.tick(t + 41);
    s.drain(8, true);
    out.push(s);
    // 6. static fail
    let mut s = Sc::new("static_floor_fail", "A 10% gap: the account fails at the sample, report_fail batch.", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, market_order(0, true, 4_500_000_000_000, t));
    s.sample(0, t + 10, p4(10_000));
    s.drain(8, false);
    s.sample(0, t + 20, p4(9_000));
    s.tick(t + 25);
    s.drain(8, true);
    out.push(s);
    // 7. quick-limit fail
    let mut s = Sc::new("quick_limit_fail", "Q passes 0.028 S at a quick close: Fail(PhaseQuick).", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, market_order(0, true, MEUR, t));
    s.sample(0, t + 30, p4(10_060));
    s.order(2, shared_close(t + 30));
    s.sample(0, t + 40, p4(10_060));
    s.tick(t + 41);
    s.drain(8, true);
    out.push(s);
    // 8. limit and stop-loss
    let mut s = Sc::new("limit_fill_then_stop_loss", "A buy limit fills at the ask, its stop-loss triggers at the bid.", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, order(IntentKind::Limit, 0, 0, 500_000_000_000, p4(9_990), p4(9_950), 0, 0, t));
    s.sample(0, t + 10, p4(9_995));
    s.sample(0, t + 20, p4(9_985));
    s.sample(0, t + 30, p4(9_970));
    s.sample(0, t + 40, p4(9_950));
    s.tick(t + 45);
    s.drain(8, true);
    out.push(s);
    // 9. two markets, a night, fees on both
    let mk = |i: u16, sym: &str| { let mut m = common::market(i, 0, sym, TEST_HALF_SPREAD_E6); m.max_age_secs = 3_600; m };
    let mut s = Sc::new("two_markets_one_night", "A long and a short held through 22:00: two holding fees at their own marks.", World::new(base_rules(vec![mk(0, "AAA"), mk(1, "BBB")]), S100K, 2, t - HOUR));
    let r = t + 8 * HOUR;
    s.sample(0, r - 3_600, p4(10_000));
    s.sample(1, r - 3_599, p4(20_000));
    s.order(1, market_order(0, true, MEUR / 100, r - 3_599));
    s.order(2, market_order(1, false, MEUR / 100, r - 3_599));
    s.sample(0, r - 5, p4(10_010));
    s.sample(1, r - 4, p4(20_020));
    s.sample(0, r + 5, p4(10_500));
    s.tick(r + 10);
    s.drain(8, true);
    out.push(s);
    // 10. a pass target
    let mut s = Sc::new("target_reached_flat", "+8.5% on the mid in a normal slice: bought at the ask 1.000001, closed at the bid 1.0084989915 of mid 1.0085, pnl 8,497.9915: B = 1.084979915 S, flat, tight mode ended.", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, market_order(0, true, MEUR, t));
    s.sample(0, t + 100, p4(10_085));
    s.order(2, shared_close(t + 100));
    s.sample(0, t + 110, p4(10_085));
    s.tick(t + 111);
    s.drain(8, true);
    out.push(s);
    // 11. lock then a pending order that survives it
    let mut s = Sc::new("breach_lock_and_pending_order", "A breach locks the account for 12 h; a limit order placed during the lock fills after it.", fx_world(TEST_HALF_SPREAD_E6));
    s.sample(0, t, p4(10_000));
    s.order(1, market_order(0, true, MEUR, t));
    for (dt, p) in [(30, 10_006), (60, 10_012), (90, 10_030)] {
        s.sample(0, t + dt, p4(p));
    }
    s.sample(0, t + 150, MID_TOUCHING_L);
    s.order(2, order(IntentKind::Limit, 0, 0, MEUR / 10, p4(9_900), 0, 0, 0, t + 150));
    s.sample(0, t + 600, p4(9_950));
    s.sample(0, t + 43_300, p4(9_890)); // after the lock (t + 150 + 43,200 = t + 43,350): not yet
    s.sample(0, t + 43_400, p4(9_885));
    s.tick(t + 43_410);
    s.drain(8, true);
    out.push(s);
    out.into_iter().map(|s| (format!("scenarios/{}.json", s.name), serde_json::to_string_pretty(&s.to_json()).unwrap() + "\n")).collect()
}

fn shared_close(t: i64) -> OrderIntent {
    close_order(0, 0, t)
}

// ------------------------------------------------------------------------------------------------ assembly

fn all_files() -> Vec<(String, String, usize)> {
    let mut files = Vec::new();
    let c = apply_fill_cases();
    let k = c.len();
    files.push(("apply_fill.json".to_string(), file("apply_fill", "engine::apply_fill: netting of one fill into the net position of a market (DESIGN 7.2). pos null = flat.", c), k));
    let c = pnl_cases();
    let k = c.len();
    files.push(("pnl_and_credit.json".to_string(), file("pnl_micro / slice_credit", "pnl_micro(long, qty_abs, entry_e12, exit_e12) floors toward minus infinity; slice_credit(pnl, quick) gives the balance credit and the quick tally delta.", c), k));
    let (margin, fees) = fees_and_margin_cases();
    let k = margin.len();
    files.push(("margin.json".to_string(), file("margin_micro", "margin_micro(qty_abs, price_e12, leverage_x): one ceiling over the whole expression.", margin), k));
    let k = fees.len();
    files.push(("fees.json".to_string(), file("fees and time", "fee_ceil_micro, holding_fee_micro, swap_fee_micro, day_index, nights_due, validate_quantity; the case field kind says which.", fees), k));
    let c = price_cases();
    let k = c.len();
    files.push(("prices.json".to_string(), file("prices", "ask_from_mid, bid_from_mid, price_from_oracle, fill_price_e12 (a long pays the ask, a short gets the bid), close_price_e12 (closing a long sells at the bid).", c), k));
    let c = trigger_cases();
    let k = c.len();
    files.push(("triggers.json".to_string(), file("check_protective / check_pending", "Stop-loss and take-profit of a position, limit and stop entry orders; touching counts; a level of 0 never triggers; closed markets and locked accounts trigger nothing.", c), k));
    let c = eval_cases();
    let k = c.len();
    files.push(("evaluate_sample.json".to_string(), file("evaluate_sample", "One sample against an account: none, fail (static floor), breach, tight start, floor raised; the state after and the equity.", c), k));
    let c = claim_cases();
    let k = c.len();
    files.push(("claims.json".to_string(), file("check_close / check_tight_start / check_floor_raised / check_breach", "The checks the program makes of an evaluator's claims (I-5, I-9, I-10).", c), k));
    let (sha, chain, enc) = chain_and_encoding_cases();
    let k = sha.len();
    files.push(("sha256.json".to_string(), file("sha256", "SHA-256 of a byte string.", sha), k));
    let k = chain.len();
    files.push(("hash_chain.json".to_string(), file("hash chain", "genesis_hash, feed_id, next_hash (one chain step), apply_batch (a whole batch); the case field kind says which.", chain), k));
    let k = enc.len();
    files.push(("batch_encoding.json".to_string(), file("batch encoding", "Canonical Borsh bytes of a JournalBatch (events and proofs) and of a PriceProof, with the proof digest.", enc), k));
    for (name, text) in scenarios() {
        files.push((name, text, 1));
    }
    files
}

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn golden_files_are_regenerated_byte_for_byte() {
    let files = all_files();
    let total: usize = files.iter().map(|f| f.2).sum();
    assert!(total >= 2_000, "at least 2,000 cases, have {total}");
    if std::env::var("GOLDEN_WRITE").is_ok() {
        std::fs::create_dir_all(dir().join("scenarios")).unwrap();
        for (name, text, _) in &files {
            std::fs::write(dir().join(name), text).unwrap();
        }
        println!("wrote {} files, {total} cases", files.len());
        return;
    }
    for (name, text, _) in &files {
        let on_disk = std::fs::read_to_string(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e} (GOLDEN_WRITE=1 writes the files)"));
        assert!(&on_disk == text, "{name} differs from what the code produces now (GOLDEN_WRITE=1 rewrites)");
    }
    // nothing else is in the directory
    let mut listed: Vec<String> = Vec::new();
    for e in std::fs::read_dir(dir()).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() {
            for f in std::fs::read_dir(e.path()).unwrap() {
                listed.push(format!("{name}/{}", f.unwrap().file_name().to_string_lossy()));
            }
        } else {
            listed.push(name);
        }
    }
    listed.retain(|n| n != "README.md" && n != "check.py");
    let mut want: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    listed.sort();
    want.sort();
    assert_eq!(listed, want);
}

#[test]
fn every_golden_file_is_valid_json_with_decimal_string_integers() {
    fn walk(v: &Value, path: &str) {
        match v {
            Value::Number(_) => panic!("a bare JSON number at {path}: integers must be decimal strings"),
            Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| walk(x, &format!("{path}[{i}]"))),
            Value::Object(o) => o.iter().for_each(|(k, x)| walk(x, &format!("{path}.{k}"))),
            _ => {}
        }
    }
    for (name, text, _) in all_files() {
        let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        walk(&v, &name);
    }
}
