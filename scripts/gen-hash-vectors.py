#!/usr/bin/env python3
"""Golden vectors for crates/journal-types (DESIGN 8.2 and 8.3).

Written from the DESIGN text with hashlib and struct only. It deliberately shares no code with the Rust encoder, so a
bug in the Rust encoder cannot also be in the vectors. Output: crates/journal-types/tests/vectors.json.
Usage: python3 scripts/gen-hash-vectors.py
"""
import hashlib
import json
import os
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "crates", "journal-types", "tests", "vectors.json")


def sha(*parts):
    h = hashlib.sha256()
    for p in parts:
        h.update(p)
    return h.digest()


# --- Borsh primitives (little endian) -------------------------------------------------
def u8(v):  return struct.pack("<B", v)
def u16(v): return struct.pack("<H", v)
def u32(v): return struct.pack("<I", v)
def u64(v): return struct.pack("<Q", v)
def i64(v): return struct.pack("<q", v)
def i128(v): return v.to_bytes(16, "little", signed=True)
def vec_u8(b): return u32(len(b)) + b


# --- DESIGN 8.2: PriceProof ----------------------------------------------------------
def feed_id(source, symbol):
    return sha(b"SL8FEED", bytes([source]), symbol.encode("utf-8"))


def proof_bytes(p):
    return (u8(p["source"]) + p["feed_id"] + u64(p["timestamp_ns"]) + i128(p["quantized_value"])
            + vec_u8(p["signed_update"]))


# --- DESIGN 8.2: JournalEvent (tag = declaration order) ------------------------------
def event_bytes(e):
    k = e["kind"]
    if k == "Fill":
        return (u8(0) + u16(e["market"]) + u8(e["side"]) + u8(e["order_kind"]) + u64(e["qty"]) + u64(e["price_e12"])
                + i64(e["time"]) + u64(e["avg_entry_after"]) + u64(e["margin"]) + u64(e["sl"]) + u64(e["tp"])
                + u32(e["intent_id"]) + u8(e["proof"]))
    if k == "Close":
        return (u8(1) + u16(e["market"]) + u8(e["side"]) + u64(e["qty"]) + u64(e["entry_e12"]) + u64(e["exit_e12"])
                + i64(e["time"]) + i64(e["timer_start"]) + i64(e["pnl"]) + u8(e["quick"]) + i64(e["credited"])
                + u8(e["reason"]) + u32(e["intent_id"]) + u8(e["proof"]))
    if k == "Fee":
        return u8(2) + u8(e["fee_kind"]) + u16(e["market"]) + u64(e["amount"]) + i64(e["time"])
    if k == "TightStart":
        return (u8(3) + i64(e["time"]) + i64(e["p_open"]) + i64(e["r"]) + i64(e["l_start"]) + i64(e["g_peak"])
                + i64(e["l_level"]) + u8(e["proof"]))
    if k == "FloorRaised":
        return u8(4) + i64(e["time"]) + i64(e["g_peak"]) + i64(e["l_level"]) + u8(e["proof"])
    if k == "TightEnd":
        return u8(5) + i64(e["time"])
    if k == "Breach":
        return u8(6) + i64(e["time"]) + i64(e["l_level"]) + i64(e["b_after"]) + u8(e["proof"])
    if k == "Fail":
        return u8(7) + i64(e["time"]) + u8(e["reason"]) + i64(e["b_after"]) + u8(e["proof"])
    if k == "Meta":
        return (u8(8) + i64(e["time"]) + u8(e["flat"]) + e["samples_hash"] + e["intents_hash"]
                + u16(e["engine_version"]))
    raise ValueError(k)


NO_PROOF = 255
HAS_PROOF_FIELD = {"Fill", "Close", "TightStart", "FloorRaised", "Breach", "Fail"}


# --- DESIGN 8.3: hash chain ----------------------------------------------------------
def genesis(pubkey):
    return sha(b"SL8LEV", pubkey)


def next_hash(prev, seq, event, proof):
    digest = sha(proof_bytes(proof)) if proof is not None else b"\x00" * 32
    return sha(b"SL8LEV1", prev, u64(seq), event_bytes(event), digest)


# --- fixed inputs (the Rust tests build the same values by hand) ----------------------
PUBKEY = bytes(range(1, 33))

PROOFS = {
    "src2": {"source": 2, "feed_id": feed_id(2, "BTC"), "timestamp_ns": 1_760_000_000_123_456_789,
             "quantized_value": 67_250_123_456_789_012_345_678 * 1, "signed_update": b""},
    "src3": {"source": 3, "feed_id": feed_id(3, "ETH"), "timestamp_ns": 1_760_000_100_000_000_001,
             "quantized_value": 2_500 * 10**18, "signed_update": bytes(range(40))},
    "src4": {"source": 4, "feed_id": feed_id(4, "xyz:AAPL"), "timestamp_ns": 1_760_000_200_999_999_999,
             "quantized_value": 231_450_000_000_000_000_000, "signed_update": b""},
    "src255": {"source": 255, "feed_id": feed_id(255, "TEST"), "timestamp_ns": 0,
               "quantized_value": -123_456_789_012_345_678_901_234_567_890, "signed_update": bytes([0xAB] * 512)},
}

EVENTS = {
    "Fill": {"kind": "Fill", "market": 3, "side": 1, "order_kind": 2, "qty": 123_456_789,
             "price_e12": 67_250_123_456_789_012, "time": 1_760_000_123, "avg_entry_after": 67_000_000_000_000_001,
             "margin": 9_876_543_210, "sl": 70_000_000_000_000_000, "tp": 60_000_000_000_000_000,
             "intent_id": 3_735_928_559, "proof": 0},
    "Close": {"kind": "Close", "market": 3, "side": 1, "qty": 123_456_789, "entry_e12": 67_000_000_000_000_001,
              "exit_e12": 66_900_000_000_000_002, "time": 1_760_000_456, "timer_start": 1_760_000_400,
              "pnl": -12_345_678, "quick": 1, "credited": -12_345_678, "reason": 2, "intent_id": 42, "proof": 1},
    "Fee": {"kind": "Fee", "fee_kind": 1, "market": 3, "amount": 777_000, "time": 1_760_000_500},
    "TightStart": {"kind": "TightStart", "time": 1_760_000_600, "p_open": 1_234_567_890, "r": 100_000_000_000,
                   "l_start": 96_000_000_000, "g_peak": 0, "l_level": 96_000_000_000, "proof": 0},
    "FloorRaised": {"kind": "FloorRaised", "time": 1_760_000_700, "g_peak": 2_500_000_000,
                    "l_level": 97_500_000_000, "proof": 1},
    "TightEnd": {"kind": "TightEnd", "time": 1_760_000_800},
    "Breach": {"kind": "Breach", "time": 1_760_000_900, "l_level": 97_500_000_000, "b_after": -1, "proof": 0},
    "Fail": {"kind": "Fail", "time": 1_760_001_000, "reason": 1, "b_after": 91_999_999_999, "proof": 1},
    "Meta": {"kind": "Meta", "time": 1_760_001_100, "flat": 1, "samples_hash": bytes([0xAA] * 32),
             "intents_hash": bytes([0xBB] * 32), "engine_version": 0x0102},
}
ORDER = ["Fill", "Close", "Fee", "TightStart", "FloorRaised", "TightEnd", "Breach", "Fail", "Meta"]
BASE_SEQ = 0x0102030405060708
PROOF_LIST = [PROOFS["src2"], PROOFS["src3"]]  # proof index 0 and 1


def hx(b):
    return b.hex()


def main():
    g = genesis(PUBKEY)
    out = {
        "genesis": {"pubkey": hx(PUBKEY), "hash": hx(g)},
        "base_seq": BASE_SEQ,
        "proofs": {name: {"bytes": hx(proof_bytes(p)), "digest": hx(sha(proof_bytes(p)))} for name, p in PROOFS.items()},
        "events": {},
        "feed_ids": [],
    }
    for k, name in enumerate(ORDER):
        e = EVENTS[name]
        seq = BASE_SEQ + k
        entry = {"bytes": hx(event_bytes(e)), "seq": seq}
        if name in HAS_PROOF_FIELD:
            entry["next_hash_with_proof"] = hx(next_hash(g, seq, e, PROOF_LIST[e["proof"]]))
            noproof = dict(e, proof=NO_PROOF)
            entry["bytes_noproof"] = hx(event_bytes(noproof))
            entry["next_hash_noproof"] = hx(next_hash(g, seq, noproof, None))
        else:
            entry["next_hash"] = hx(next_hash(g, seq, e, None))
        out["events"][name] = entry

    # a full batch: Fill(proof 0), Close(proof 1), Meta; first_seq 1, prev = genesis, proofs [src2, src3]
    chain_events = [EVENTS["Fill"], EVENTS["Close"], EVENTS["Meta"]]
    batch = (u64(1) + g + u32(len(chain_events)) + b"".join(event_bytes(e) for e in chain_events)
             + u32(len(PROOF_LIST)) + b"".join(proof_bytes(p) for p in PROOF_LIST))
    hashes = []
    prev = g
    for i, e in enumerate(chain_events):
        ref = e.get("proof", NO_PROOF)
        prev = next_hash(prev, 1 + i, e, PROOF_LIST[ref] if ref != NO_PROOF else None)
        hashes.append(hx(prev))
    out["batch3"] = {"bytes": hx(batch), "first_seq": 1, "hashes": hashes}

    for source, symbol in [(2, "BTC"), (4, "xyz:AAPL"), (0, "EURUSD")]:
        out["feed_ids"].append({"source": source, "symbol": symbol, "feed_id": hx(feed_id(source, symbol))})

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as f:
        json.dump(out, f, indent=2, sort_keys=True)
        f.write("\n")
    print("genesis      ", hx(g))
    print("chain h1     ", hashes[0])
    print("chain h2     ", hashes[1])
    print("chain h3     ", hashes[2])
    print("wrote        ", os.path.relpath(OUT, ROOT))


if __name__ == "__main__":
    main()
