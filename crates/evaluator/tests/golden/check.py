#!/usr/bin/env python3
"""Independent re-implementation of the pure functions in the golden files, in plain Python integers.

Usage: python3 check.py            (from this directory; exits 0 when every case of every checked file matches)

It exists to show that the vectors are enough to port the engine: nothing here is generated from the Rust code, and the
only library used is hashlib for SHA-256. Checked: apply_fill, pnl_and_credit, margin, fees, prices, triggers,
evaluate_sample, sha256, genesis_hash and feed_id. (Batch encoding and the hash chain steps are checked in Rust against the
independent encoder scripts/gen-hash-vectors.py; the scenarios must be reproduced by a full backend.)
"""
import hashlib
import json
import sys

I64 = (-(1 << 63), (1 << 63) - 1)
I128 = (-(1 << 127), (1 << 127) - 1)
U64 = (0, (1 << 64) - 1)
DEN = 10**12


class Err(Exception):
    pass


def fit(x, lo_hi, what="Overflow"):
    if not lo_hi[0] <= x <= lo_hi[1]:
        raise Err(what)
    return x


def floor_div(a, b):
    return a // b


def ceil_div(a, b):
    return -((-a) // b)


def pnl_micro(long, qty, entry, exit_):
    diff = exit_ - entry
    if not long:
        diff = -diff
    num = fit(qty * diff, I128)
    return fit(floor_div(num, DEN), I64)


def slice_credit(pnl, quick):
    credit = pnl // 2 if quick and pnl > 0 else pnl
    tally = max(credit, 0) if quick else 0
    return credit, tally


def margin_micro(qty, price, lev):
    if lev == 0:
        raise Err("InvalidInput")
    num = qty * price
    fit(num, (0, (1 << 128) - 1))
    return fit(ceil_div(num, DEN * lev), I64)


def fee_ceil_micro(notional, rate):
    if notional < 0:
        raise Err("InvalidInput")
    return fit(ceil_div(notional * rate, 10**6), I64)


def holding_fee_micro(qty, mark, rate, nights):
    return fit(ceil_div(qty * mark * rate * nights, DEN * 10**6), I64)


def swap_fee_micro(cost, bps):
    return fit(ceil_div(cost * bps, 10000), U64)


def day_index(t):
    t = max(I64[0], min(I64[1], t))
    return (t - 22 * 3600) // 86400


def nights_due(fee_day, today):
    if today < fee_day:
        raise Err("InvalidInput")
    return fit(today - fee_day, (0, (1 << 32) - 1))


def validate_quantity(qty, step, mn):
    if step == 0:
        raise Err("InvalidInput")
    if qty == 0 or qty % step != 0 or qty < mn:
        raise Err("InvalidQuantity")


def ask_from_mid(mid, h):
    return fit(ceil_div(mid * (10**6 + h), 10**6), U64)


def bid_from_mid(mid, h):
    if h > 10**6:
        raise Err("InvalidInput")
    return fit(mid * (10**6 - h) // 10**6, U64)


def price_from_oracle(v):
    return fit(v // 10**6, U64)


def apply_fill(pos, side, dq, price, t, fee_day_now):
    if dq == 0:
        raise Err("InvalidQuantity")
    if price == 0:
        raise Err("InvalidInput")
    if dq > I64[1]:
        raise Err("InvalidQuantity")
    signed = dq if side == "buy" else -dq
    if pos is None:
        return {"position": {"qty": signed, "avg_entry_e12": price, "timer_start": t, "opened_at": t, "fee_day": fee_day_now},
                "closed": None, "opened": {"qty": signed, "price_e12": price}, "balance_delta": 0, "quick_tally_delta": 0}
    if pos["qty"] == 0:
        raise Err("InvalidInput")
    age = fit(t - pos["timer_start"], I64)
    if age < 0:
        raise Err("InvalidInput")
    long = pos["qty"] > 0
    q_abs = abs(pos["qty"])
    if long == (side == "buy"):
        num = q_abs * pos["avg_entry_e12"] + dq * price
        den = q_abs + dq
        fit(num, I128)
        avg = ceil_div(num, den) if long else num // den
        avg = fit(avg, U64)
        qty = fit(pos["qty"] + signed, I64)
        if qty == I64[0]:
            raise Err("Overflow")
        return {"position": {"qty": qty, "avg_entry_e12": avg, "timer_start": t, "opened_at": pos["opened_at"], "fee_day": pos["fee_day"]},
                "closed": None, "opened": {"qty": signed, "price_e12": price}, "balance_delta": 0, "quick_tally_delta": 0}
    c = min(dq, q_abs)
    pnl = pnl_micro(long, c, pos["avg_entry_e12"], price)
    quick = age <= 45
    credit, tally = slice_credit(pnl, quick)
    closed = {"qty_abs": c, "entry_e12": pos["avg_entry_e12"], "exit_e12": price, "pnl": pnl, "quick": quick, "credit": credit}
    if dq < q_abs:
        newpos = dict(pos)
        newpos["qty"] = fit(pos["qty"] + signed, I64)
        opened = None
    elif dq == q_abs:
        newpos, opened = None, None
    else:
        rest = dq - q_abs
        rest_pos = fit(rest, I64)
        rs = -rest_pos if long else rest_pos
        newpos = {"qty": rs, "avg_entry_e12": price, "timer_start": t, "opened_at": t, "fee_day": fee_day_now}
        opened = {"qty": rs, "price_e12": price}
    return {"position": newpos, "closed": closed, "opened": opened, "balance_delta": credit, "quick_tally_delta": tally}


def frac_exact(size, bps):
    if size <= 0:
        raise Err("InvalidInput")
    n = size * bps
    fit(n, I128)
    if n % 10000 != 0:
        raise Err("InvalidInput")
    return fit(n // 10000, I64)


def evaluate_sample(st, p_open, comm, now):
    if comm < 0:
        raise Err("InvalidInput")
    equity = fit(st["b"] + p_open - comm, I64)
    size = st["size_micro"]
    f_s = frac_exact(size, 9600 if st["latch"] else 9200)
    if equity <= f_s:
        return {"action": {"kind": "fail", "reason": "StaticFloor"}, "state": st, "equity_micro": equity}
    eff = max(f_s, st["l_level"]) if st["tight_on"] else f_s
    if st["tight_on"] and equity <= eff:
        lock = fit(now + 12 * 3600, I64)
        ns = dict(st, tight_on=False, r=0, l_start=0, g_peak=0, l_level=0, lock_until=lock, flat=True)
        return {"action": {"kind": "breach", "close_all": True, "lock_until": lock}, "state": ns, "equity_micro": equity}
    one = frac_exact(size, 100)
    if not st["tight_on"] and p_open > one:
        r = st["b"]
        g = max(0, fit(equity - r, I64))
        cap = fit(f_s + frac_exact(size, 400), I64)
        l_start = min(cap, r)
        l_level = fit(l_start + max(0, fit(g - one, I64)), I64)
        ns = dict(st, tight_on=True, r=r, l_start=l_start, g_peak=g, l_level=l_level)
        return {"action": {"kind": "tight_start", "r": r, "l_start": l_start, "g_peak": g, "l_level": l_level}, "state": ns, "equity_micro": equity}
    if st["tight_on"]:
        g = fit(equity - st["r"], I64)
        if g > st["g_peak"]:
            l_level = fit(st["l_start"] + max(0, fit(g - one, I64)), I64)
            ns = dict(st, g_peak=g, l_level=l_level)
            return {"action": {"kind": "floor_raised", "g_peak": g, "l_level": l_level}, "state": ns, "equity_micro": equity}
    return {"action": {"kind": "none"}, "state": st, "equity_micro": equity}


def protective(side, sl, tp, bid, ask, open_):
    if not open_:
        return None
    if side == "buy":
        price, sl_hit, tp_hit = bid, sl != 0 and bid <= sl, tp != 0 and bid >= tp
    else:
        price, sl_hit, tp_hit = ask, sl != 0 and ask >= sl, tp != 0 and ask <= tp
    if sl_hit:
        return {"kind": "StopLoss", "price_e12": price}
    if tp_hit:
        return {"kind": "TakeProfit", "price_e12": price}
    return None


def pending(side, kind, level, bid, ask, open_, locked):
    if not open_ or locked or level == 0:
        return None
    if side == "buy":
        price = ask
        hit = ask <= level if kind == "Limit" else ask >= level
    else:
        price = bid
        hit = bid >= level if kind == "Limit" else bid <= level
    return price if hit else None


# ---------------------------------------------------------------------------------------------------- JSON <-> python

def num(x):
    return int(x)


def pos_in(p):
    if p is None:
        return None
    return {k: num(v) for k, v in p.items()}


def to_dec(x):
    if isinstance(x, dict):
        return {k: to_dec(v) for k, v in x.items()}
    if isinstance(x, bool) or x is None or isinstance(x, str):
        return x
    return str(x)


def expect(case, produce):
    try:
        got = {"ok": produce()}
    except Err as e:
        got = {"err": str(e)}
    return to_dec(got) == case["out"]


def state_in(s):
    return {k: (v if isinstance(v, bool) else num(v)) for k, v in s.items()}


def check_file(name, fn):
    d = json.load(open(name))
    bad = 0
    for case in d["cases"]:
        if not fn(case):
            bad += 1
            if bad <= 3:
                print("MISMATCH", name, case["name"])
    print(f"{name}: {len(d['cases'])} cases, {bad} mismatches")
    return bad


def main():
    bad = 0

    def f_apply(c):
        i = c["in"]
        return expect(c, lambda: apply_fill(pos_in(i["pos"]), i["side"], num(i["dq"]), num(i["price_e12"]), num(i["t"]), num(i["fee_day_now"])))

    bad += check_file("apply_fill.json", f_apply)

    def f_pnl(c):
        i = c["in"]
        if c.get("kind") == "slice_credit":
            credit, tally = slice_credit(num(i["pnl"]), i["quick"])
            return c["out"] == {"credit": str(credit), "quick_tally_delta": str(tally)}
        try:
            return c["out"] == {"ok": str(pnl_micro(i["long"], num(i["qty_abs"]), num(i["entry_e12"]), num(i["exit_e12"])))}
        except Err as e:
            return c["out"] == {"err": str(e)}

    bad += check_file("pnl_and_credit.json", f_pnl)
    bad += check_file("margin.json", lambda c: expect(c, lambda: margin_micro(num(c["in"]["qty_abs"]), num(c["in"]["price_e12"]), num(c["in"]["leverage_x"]))))

    def f_fee(c):
        i, k = c["in"], c["kind"]
        if k == "fee_ceil_micro":
            return expect(c, lambda: fee_ceil_micro(num(i["notional_micro"]), num(i["rate_e6"])))
        if k == "holding_fee_micro":
            return expect(c, lambda: holding_fee_micro(num(i["qty_abs"]), num(i["mark_e12"]), num(i["rate_e6"]), num(i["nights"])))
        if k == "swap_fee_micro":
            return expect(c, lambda: swap_fee_micro(num(i["tier_cost_micro"]), num(i["fee_bps"])))
        if k == "day_index":
            return c["out"] == str(day_index(num(i["unix_secs"])))
        if k == "nights_due":
            return expect(c, lambda: nights_due(num(i["fee_day"]), num(i["today_index"])))
        if k == "validate_quantity":
            def v():
                validate_quantity(num(i["qty"]), num(i["qty_step"]), num(i["qty_min"]))
                return None
            return expect(c, v)
        return False

    bad += check_file("fees.json", f_fee)

    def f_price(c):
        i, k = c["in"], c["kind"]
        if k == "ask_from_mid":
            return expect(c, lambda: ask_from_mid(num(i["mid_e12"]), num(i["half_spread_e6"])))
        if k == "bid_from_mid":
            return expect(c, lambda: bid_from_mid(num(i["mid_e12"]), num(i["half_spread_e6"])))
        if k == "price_from_oracle":
            return expect(c, lambda: price_from_oracle(num(i["value_e18"])))
        mid, h, long = num(i["mid_e12"]), num(i["half_spread_e6"]), i["long"]
        if k == "fill_price_e12":
            return expect(c, lambda: ask_from_mid(mid, h) if long else bid_from_mid(mid, h))
        if k == "close_price_e12":
            return expect(c, lambda: bid_from_mid(mid, h) if long else ask_from_mid(mid, h))
        return False

    bad += check_file("prices.json", f_price)

    def f_trig(c):
        i = c["in"]
        if c["kind"] == "check_protective":
            r = protective(i["position_side"], num(i["sl_e12"]), num(i["tp_e12"]), num(i["bid_e12"]), num(i["ask_e12"]), i["market_open"])
            return c["out"] == (None if r is None else {"kind": r["kind"], "price_e12": str(r["price_e12"])})
        r = pending(i["order_side"], i["order_kind"], num(i["level_e12"]), num(i["bid_e12"]), num(i["ask_e12"]), i["market_open"], i["locked"])
        return c["out"] == (None if r is None else str(r))

    bad += check_file("triggers.json", f_trig)
    bad += check_file("evaluate_sample.json", lambda c: expect(c, lambda: evaluate_sample(state_in(c["in"]["state"]), num(c["in"]["p_open_micro"]), num(c["in"]["closing_commission_micro"]), num(c["in"]["now"]))))

    def f_sha(c):
        return c["out"] == hashlib.sha256(bytes.fromhex(c["in"]["data"])).hexdigest()

    bad += check_file("sha256.json", f_sha)

    def f_chain(c):
        k, i = c["kind"], c["in"]
        if k == "genesis_hash":
            return c["out"] == hashlib.sha256(b"SL8LEV" + bytes.fromhex(i["account"])).hexdigest()
        if k == "feed_id":
            return c["out"] == hashlib.sha256(b"SL8FEED" + bytes([num(i["source"])]) + i["symbol"].encode()).hexdigest()
        return True  # next_hash / apply_batch: needs the event encoder (see the module docs)

    bad += check_file("hash_chain.json", f_chain)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
