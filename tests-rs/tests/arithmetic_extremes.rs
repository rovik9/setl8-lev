//! M12 item 6 (second half), ARITHMETIC AND ROUNDING EXTREMES THROUGH THE REAL PROGRAM (DESIGN 4.4 I-1 to I-12, 7.1, 7.4, 9.2,
//! 9.3, 9.5). SELF-audit by the builder, written to BREAK the program, not to confirm it.
//!
//! The engine-level property tests live in `crates/evaluator/tests/rounding_properties.rs` (31 passing + 2 ignored findings ARI-1,
//! ARI-2, 100,000 random cases per formula, fixed seed). THIS file drives the same formulas through `post_journal`, `open_account`,
//! `reset_account` and `finalize_pass` of the built localnet `.so` in LiteSVM with the real vault fixture, with u64::MAX prices and
//! quantities, i128 intermediates at their limits, balances and tallies at the i64 limits, the smallest and the largest account
//! size, and boundary inputs on both sides of every rounding step.
//!
//! Every expected number comes from an INDEPENDENT integer reference written in this file (`u128` / `i128`, plain `/` and `%`, no
//! call into `engine`). Where the prompt asks for "exactly the engine's number" the test ALSO calls the engine and compares all
//! three (program, engine, reference). Every refusal test asserts the exact `Setl8Error` and that the whole world (207-byte
//! account as raw bytes, config, wallet, vault records, markets) is byte-identical afterwards, and that no `EventLog` was emitted.
//! A panic or abort of the program would show as `ProgramFailedToComplete` / a non-custom `InstructionError`; `assert_err_at`
//! refuses those, so a panic is a failing test (= a FINDING).
//!
//! Classes: BLOCKED (exact refusal), LIMITED (the test pins EXACTLY what still passes), OFF-CHAIN, POLICY. Findings carry a
//! `// FINDING ARI-n` comment and an `#[ignore = "FINDING ..."]` test that asserts the DESIGN behaviour.
//!
//! Fragment: `handover/audit/06a-arithmetic-extremes.md`. Seed of every random stream: `SEED` (printed with `--nocapture`).
#![allow(clippy::too_many_arguments, clippy::type_complexity)]
mod common;

use anchor_lang::{AccountDeserialize, AccountSerialize};
use common::journal::*;
use common::vault::*;
use common::*;
use journal_types::{CloseReason, FeeKind, JournalEvent, PriceProof, Side};
use litesvm::types::TransactionResult;
use setl8_lev_trading::{errors::Setl8Error as E, state::{RuleSet, SectorConfig, TradingAccount}};
use solana_account::Account as RawAccount;
use solana_signer::Signer;

/// Fixed seed of every random stream of this file (each test derives its own stream with `Rng::for_test`).
const SEED: u64 = 0x5E78_4D12_A21F_0A06;
const E6: u128 = 1_000_000;
const DEN: i128 = 1_000_000_000_000;
/// 0.92 S, 1.04 S and 0.028 S of the $100,000 account written out by hand (independent of the engine's constants).
const FLOOR_100K: i64 = 92_000_000_000;
const LATCH_100K: i64 = 104_000_000_000;
const QLIMIT_100K: i64 = 2_800_000_000;

// ------------------------------------------------------------------------------------------------------------ random + reference

/// splitmix64: tiny, fixed, identical everywhere.
struct Rng(u64);
impl Rng {
    fn for_test(name: &str) -> Rng {
        let mut h = SEED;
        for b in name.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
        }
        println!("[arithmetic_extremes] {name}: seed {SEED:#018x}, stream {h:#018x}");
        Rng(h)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = u128::from(hi - lo) + 1;
        lo + u64::try_from(u128::from(self.next()) % span).unwrap()
    }
    /// Log-uniform over the bit length, so small, medium and huge values all appear.
    fn log(&mut self) -> u64 {
        let bits = 1 + self.next() % 64;
        self.next() >> (64 - bits)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[usize::try_from(self.next() % xs.len() as u64).unwrap()]
    }
}

/// `ceil(mid x (1e6 + h) / 1e6)`; `None` when it does not fit `u64`.
fn ref_ask(mid: u64, h: u32) -> Option<u64> {
    let n = u128::from(mid) * (E6 + u128::from(h));
    u64::try_from(n.div_ceil(E6)).ok()
}
/// `floor(mid x (1e6 - h) / 1e6)`.
fn ref_bid(mid: u64, h: u32) -> u64 {
    u64::try_from(u128::from(mid) * (E6 - u128::from(h)) / E6).unwrap()
}
/// A fill's price: a buy pays the ask, a sell gets the bid.
fn ref_fill(mid: u64, h: u32, long: bool) -> Option<u64> {
    if long { ref_ask(mid, h) } else { Some(ref_bid(mid, h)) }
}
/// A close's price: a long closes at the bid, a short at the ask.
fn ref_close(mid: u64, h: u32, long: bool) -> Option<u64> {
    ref_fill(mid, h, !long)
}
fn floor_div(n: i128, d: i128) -> i128 {
    let q = n / d;
    if n % d != 0 && ((n < 0) != (d < 0)) { q - 1 } else { q }
}
/// `floor(sign x qty x (exit - entry) / DEN)`; `Err` when the i128 product or the i64 result overflows.
fn ref_pnl(long: bool, qty: u64, entry: u64, exit: u64) -> Result<i64, ()> {
    let diff = i128::from(exit) - i128::from(entry);
    let signed = if long { diff } else { -diff };
    let n = i128::from(qty).checked_mul(signed).ok_or(())?;
    let p = floor_div(n, DEN);
    // direction: floor means p x DEN <= n < (p + 1) x DEN
    assert!(p * DEN <= n && n < (p + 1) * DEN, "reference floor is wrong");
    i64::try_from(p).map_err(|_| ())
}
/// `(credit, tally_delta)`: a quick profit is credited as floor(p / 2), everything else in full; only a quick slice feeds the tally.
fn ref_credit(pnl: i64, quick: bool) -> (i64, i64) {
    let credit = if quick && pnl > 0 { i64::try_from(floor_div(i128::from(pnl), 2)).unwrap() } else { pnl };
    (credit, if quick { credit.max(0) } else { 0 })
}

// ------------------------------------------------------------------------------------------------------------ world and snapshots

fn world() -> (JEnv, i64) {
    let w = JEnv::new();
    let t = w.t0 + 1_000;
    (w, t)
}

/// What "nothing changed" means: the raw 207 bytes of the trading account and the digest of everything a journal
/// instruction can reach (config, wallet, vault state and records, rule sets, markets).
#[derive(Debug, PartialEq, Eq)]
struct Snap {
    acct: Vec<u8>,
    digest: [u8; 32],
}
fn snap(w: &JEnv) -> Snap {
    let extra: Vec<anchor_lang::solana_program::pubkey::Pubkey> = (0..6u16).map(market_pda).collect();
    Snap { acct: w.v.e.svm.get_account(&w.acct).map(|a| a.data).unwrap_or_default(), digest: w.v.digest(&w.trader.pubkey(), &extra) }
}
fn save(w: &JEnv) -> RawAccount {
    w.v.e.svm.get_account(&w.acct).expect("trading account")
}
fn restore(w: &mut JEnv, a: &RawAccount) {
    w.v.e.svm.set_account(w.acct, a.clone()).unwrap();
}
fn edit_ruleset(w: &mut JEnv, f: impl FnOnce(&mut RuleSet)) {
    let k = ruleset_pda(1);
    let mut acc = w.v.e.svm.get_account(&k).expect("rule set 1");
    let mut r = RuleSet::try_deserialize(&mut acc.data.as_slice()).unwrap();
    f(&mut r);
    let mut data = Vec::new();
    r.try_serialize(&mut data).unwrap();
    acc.data = data;
    w.v.e.svm.set_account(k, acc).unwrap();
}
/// The half-spread (e6) of the EUR market in the account's rule set.
fn set_spread(w: &mut JEnv, h: u16) {
    edit_ruleset(w, |r| r.spread_e6[usize::from(MARKET_EUR)] = h);
}

/// One `post_journal` of `events` (markets: EUR) at cluster time `t + 1`.
fn send(w: &mut JEnv, t: i64, events: Vec<JournalEvent>, proofs: Vec<PriceProof>) -> TransactionResult {
    let b = w.batch(events, proofs);
    w.post(t + 1, &b, &[MARKET_EUR])
}

/// Exact refusal `want`, no `EventLog`, nothing changed.
fn refuse(w: &mut JEnv, t: i64, events: Vec<JournalEvent>, proofs: Vec<PriceProof>, want: E) {
    let before = snap(w);
    let r = send(w, t, events, proofs);
    assert_err_at(&r, 1, want);
    let meta = &r.as_ref().unwrap_err().meta;
    assert!(event_logs(meta).is_empty(), "a refused batch emitted an EventLog ({want:?})");
    assert_eq!(snap(w), before, "a refused batch changed the world ({want:?})");
}

/// The batch is accepted. Returns the account as the program stored it (raw) and the batch's expected chain head.
fn accept(w: &mut JEnv, t: i64, events: Vec<JournalEvent>, proofs: Vec<PriceProof>) -> (TradingAccount, ([u8; 32], u64)) {
    let b = w.batch(events, proofs);
    let head = w.expected_head(&b);
    match w.post(t + 1, &b, &[MARKET_EUR]) {
        Ok(_) => {}
        Err(f) => panic!("post_journal refused a batch it must accept: {:?}\n{}", f.err, f.meta.logs.join("\n")),
    }
    (w.account(), head)
}

/// Asserts that the stored account is `before` with exactly the fields in `edit` changed (plus the chain fields from the real
/// `journal_types` head, `last_batch_time = t` and `flat = 0`, which every batch of this file ends with): builds the expected
/// account, serializes it, and compares the 207 raw bytes. A mismatch names the byte offsets.
fn assert_account_is(w: &JEnv, before: &TradingAccount, head: ([u8; 32], u64), t: i64, edit: impl FnOnce(&mut TradingAccount)) {
    let mut want = before.clone();
    want.seq = head.1;
    want.journal_hash = head.0;
    want.last_batch_time = t;
    want.flat = 0;
    edit(&mut want);
    let mut bytes = Vec::new();
    want.try_serialize(&mut bytes).unwrap();
    let got = w.v.e.svm.get_account(&w.acct).unwrap().data;
    let diff: Vec<usize> = (0..got.len().max(bytes.len())).filter(|&i| got.get(i) != bytes.get(i)).collect();
    assert!(diff.is_empty(), "the stored account differs from the expected one at byte offsets {diff:?}");
}

fn mid_proof(t: i64, mid_e12: u64) -> PriceProof {
    proof(MARKET_EUR, t, mid_e12)
}
fn proof_q(t: i64, quantized: i128) -> PriceProof {
    let mut p = proof(MARKET_EUR, t, 1);
    p.quantized_value = quantized;
    p
}
fn close_ev(long: bool, qty: u64, entry: u64, exit: u64, t: i64, timer_start: i64, pnl: i64, quick: bool, credited: i64) -> JournalEvent {
    JournalEvent::Close {
        market: MARKET_EUR,
        side: if long { Side::Long } else { Side::Short },
        qty,
        entry_e12: entry,
        exit_e12: exit,
        time: t,
        timer_start,
        pnl,
        quick,
        credited,
        reason: CloseReason::Manual,
        intent_id: 1,
        proof: 0,
    }
}
fn fee_ev(kind: FeeKind, amount: u64, t: i64) -> JournalEvent {
    JournalEvent::Fee { kind, market: MARKET_EUR, amount, time: t }
}

// ============================================================================================================ FILL PRICE

/// Largest mid whose ask still fits `u64` for half-spread `h` (reference), so `mid + 1` overflows.
fn max_mid_for_ask(h: u32) -> u64 {
    let m = u64::try_from(u128::from(u64::MAX) * E6 / (E6 + u128::from(h))).unwrap();
    assert!(ref_ask(m, h).is_some(), "reference boundary");
    assert!(ref_ask(m + 1, h).is_none(), "reference boundary + 1");
    m
}

/// Accepts the fill at the reference price, or checks the exact refusal. Returns `true` when the fill was accepted.
fn check_fill(w: &mut JEnv, t: i64, snapshot: &RawAccount, long: bool, mid: u64, remainder: u64, h: u32, qty: u64) -> bool {
    let quantized = i128::from(mid) * 1_000_000 + i128::from(remainder);
    let side = if long { "buy" } else { "sell" };
    let expected = ref_fill(mid, h, long);
    // the engine, the reference and the cross-multiplication agree (direction against the trader)
    assert_eq!(engine::fill_price_e12(mid, h, long).ok(), expected, "engine vs reference: {side} mid {mid} h {h}");
    if let Some(p) = expected {
        let exact = u128::from(mid) * (if long { E6 + u128::from(h) } else { E6 - u128::from(h) });
        if long {
            // ask: p >= exact price, and p - 1 < exact price
            assert!(u128::from(p) * E6 >= exact && (u128::from(p) - 1) * E6 < exact, "ask is not the ceiling");
        } else {
            // bid: p <= exact price < p + 1
            assert!(u128::from(p) * E6 <= exact && exact < (u128::from(p) + 1) * E6, "bid is not the floor");
        }
    }
    let mkt_proof = || vec![proof_q(t, quantized)];
    let ev = |price: u64| vec![fill(MARKET_EUR, long, qty, price, t, 0), meta(t, false)];
    match expected {
        None => {
            // the ask does not fit u64: whatever price the event claims, the program says MathOverflow
            for claimed in [0u64, 1, u64::MAX] {
                refuse(w, t, ev(claimed), mkt_proof(), E::MathOverflow);
            }
            false
        }
        Some(p) => {
            // against the trader on both sides: the neighbours are refused (a cheaper buy / dearer sell is not accepted either)
            for claimed in [p.checked_sub(1), p.checked_add(1)].into_iter().flatten() {
                let want = if claimed == 0 { E::PriceInvalid } else { E::EventPriceMismatch };
                refuse(w, t, ev(claimed), mkt_proof(), want);
            }
            if p == 0 {
                // a sell whose bid floors to 0 can never be filled: the exact price is not a price
                refuse(w, t, ev(0), mkt_proof(), E::PriceInvalid);
                false
            } else {
                let before = w.account();
                let (_, head) = accept(w, t, ev(p), mkt_proof());
                assert_account_is(w, &before, head, t, |_| {});
                restore(w, snapshot);
                true
            }
        }
    }
}

/// FILL PRICE, BOTH SIDES (BLOCKED): for random and boundary mids and half-spreads (1 to 65,535 e6) the program accepts exactly
/// `ceil(mid x (1e6 + h) / 1e6)` for a buy and `floor(mid x (1e6 - h) / 1e6)` for a sell (reference, engine and program agree), the
/// neighbours `p - 1` and `p + 1` are refused with `EventPriceMismatch` (`PriceInvalid` for 0), an ask beyond `u64::MAX` is
/// `MathOverflow`, and a proof value with a sub-e12 remainder is floored (never rounded up) before the spread. An accepted fill
/// changes ONLY seq, hash and last_batch_time.
#[test]
fn fill_price_both_sides_match_the_reference_and_the_neighbours_are_refused() {
    let mut rng = Rng::for_test("fill_price");
    let (mut w, t) = world();
    let snapshot = save(&w);
    let hs: [u16; 6] = [1, 2, 30, 50, 1_000, 65_535];
    let mut mids: Vec<u64> = vec![1, 2, 3, 999_999, 1_000_000, 1_000_001, 999_999_999_999, MID, MID + 1, (1 << 63) - 1, 1 << 63, u64::MAX - 1, u64::MAX];
    for &h in &hs {
        let m = max_mid_for_ask(u32::from(h));
        mids.extend([m - 1, m, m + 1]);
    }
    let (mut accepted, mut overflow, mut zero_bid, mut cases) = (0u32, 0u32, 0u32, 0u32);
    for &h in &hs {
        set_spread(&mut w, h);
        for &mid in &mids {
            for long in [true, false] {
                for remainder in [0u64, 999_999] {
                    cases += 1;
                    match check_fill(&mut w, t, &snapshot, long, mid, remainder, u32::from(h), 1_000_000) {
                        true => accepted += 1,
                        false => match ref_fill(mid, u32::from(h), long) {
                            None => overflow += 1,
                            Some(_) => zero_bid += 1,
                        },
                    }
                }
            }
        }
    }
    // random cases: a random half-spread, a random mid (log-uniform), a random remainder
    for _ in 0..350 {
        let h = u16::try_from(1 + rng.next() % 65_535).unwrap();
        set_spread(&mut w, h);
        let mid = rng.log().max(1);
        let remainder = if rng.chance(50) { rng.range(0, 999_999) } else { 0 };
        let long = rng.chance(50);
        let qty = 1_000 * rng.range(1_000, 1_000_000_000_000);
        cases += 1;
        match check_fill(&mut w, t, &snapshot, long, mid, remainder, u32::from(h), qty) {
            true => accepted += 1,
            false => match ref_fill(mid, u32::from(h), long) {
                None => overflow += 1,
                Some(_) => zero_bid += 1,
            },
        }
    }
    println!("fill_price: {cases} cases, {accepted} accepted, {overflow} ask overflows, {zero_bid} bids that floor to 0");
    assert!(accepted > 300 && overflow >= 12 && zero_bid >= 6, "the generator must reach all three outcomes");
}

/// THE ORACLE VALUE IS FLOORED TO e12 BEFORE THE SPREAD (LIMITED): `quantized = k x 1e6 + 999,999` is price `k`, not `k + 1`. A fill
/// priced from `k + 1` is refused; one priced from `k` is accepted. Values that floor to 0 or are not positive or do not fit `u64`
/// are `ProofValueInvalid`; `u64::MAX x 1e6 + 999,999` is the largest price (a buy there overflows, a sell is accepted).
#[test]
fn oracle_value_edges_are_floored_and_bounded() {
    let (mut w, t) = world();
    set_spread(&mut w, 30);
    let snapshot = save(&w);
    let ev = |long: bool, price: u64| vec![fill(MARKET_EUR, long, 1_000_000, price, t, 0), meta(t, false)];
    // not positive, floors to 0, too large: ProofValueInvalid for any claimed price
    let max_q = i128::from(u64::MAX) * 1_000_000 + 999_999;
    for q in [i128::MIN, -1_000_000, -1, 0, 1, 999_999, max_q + 1, i128::from(u64::MAX) * 1_000_000 + 1_000_000, i128::MAX] {
        refuse(&mut w, t, ev(true, 1), vec![proof_q(t, q)], E::ProofValueInvalid);
    }
    // 1_000_000 is the smallest valid value (price 1): the buy is ceil(1.00003) = 2, the sell is floor(0.99997) = 0 (not a price)
    refuse(&mut w, t, ev(false, 0), vec![proof_q(t, 1_000_000)], E::PriceInvalid);
    refuse(&mut w, t, ev(false, 1), vec![proof_q(t, 1_999_999)], E::EventPriceMismatch);
    accept(&mut w, t, ev(true, 2), vec![proof_q(t, 1_999_999)]);
    restore(&mut w, &snapshot);
    refuse(&mut w, t, ev(true, 3), vec![proof_q(t, 1_999_999)], E::EventPriceMismatch);
    // the largest representable price: the sell is accepted at floor(u64::MAX x (1 - 30e-6)), the buy overflows
    let sell = ref_bid(u64::MAX, 30);
    let before = w.account();
    let (_, head) = accept(&mut w, t, ev(false, sell), vec![proof_q(t, max_q)]);
    assert_account_is(&w, &before, head, t, |_| {});
    restore(&mut w, &snapshot);
    refuse(&mut w, t, ev(true, u64::MAX), vec![proof_q(t, max_q)], E::MathOverflow);
    // mid = u64::MAX + 1 (one e12 unit above) is not a price
    refuse(&mut w, t, ev(false, sell), vec![proof_q(t, max_q + 1)], E::ProofValueInvalid);
}

// ============================================================================================================ FILL QUANTITY
/// Market 0: `qty_step` 1,000, `qty_min` 1,000,000 (see `market_args`).
const STEP: u64 = 1_000;
const QTY_MIN: u64 = 1_000_000;

/// QUANTITY EXTREMES (BLOCKED + LIMITED): zero, one unit, below the minimum, off the step and `u64::MAX` are `QuantityInvalid`;
/// the largest multiple of the step (`u64::MAX - 615`) and the minimum itself are ACCEPTED. LIMITED: the program has no maximum
/// quantity, no notional cap and no margin check (OFF-CHAIN: the evaluator's `max_notional` and margin rules): such a fill at the
/// largest price changes nothing but seq, hash and last_batch_time, although the engine's own notional overflows `i128`.
#[test]
fn fill_quantity_extremes() {
    let (mut w, t) = world();
    let snapshot = save(&w);
    let price = ref_ask(MID, H_EUR).unwrap();
    let ev = |qty: u64| vec![fill(MARKET_EUR, true, qty, price, t, 0), meta(t, false)];
    for qty in [0u64, 1, 2, 999, STEP, QTY_MIN - STEP, QTY_MIN - 1, QTY_MIN + 1, QTY_MIN + STEP - 1, (1 << 63) - 1, 1 << 63, u64::MAX - 1, u64::MAX] {
        refuse(&mut w, t, ev(qty), vec![p_eur(t)], E::QuantityInvalid);
    }
    for qty in [QTY_MIN, QTY_MIN + STEP, 1_000_000_000_000, (1 << 62) / STEP * STEP, u64::MAX - u64::MAX % STEP] {
        let before = w.account();
        let (_, head) = accept(&mut w, t, ev(qty), vec![p_eur(t)]);
        assert_account_is(&w, &before, head, t, |_| {});
        restore(&mut w, &snapshot);
    }
    // largest multiple of the step at the largest price: the engine's notional is an i128 overflow, the program does not compute it
    set_spread(&mut w, 1);
    let snapshot2 = save(&w);
    let top = ref_bid(u64::MAX, 1);
    let qty = u64::MAX - u64::MAX % STEP;
    assert_eq!(engine::units::notional_micro(qty, top), Err(engine::EngineError::Overflow), "the engine refuses this notional");
    let before = w.account();
    let (_, head) = accept(&mut w, t, vec![fill(MARKET_EUR, false, qty, top, t, 0), meta(t, false)], vec![mid_proof(t, u64::MAX)]);
    assert_account_is(&w, &before, head, t, |_| {});
    restore(&mut w, &snapshot2);
}

// FINDING ARI-3: the program accepts a `Fill` whose quantity is above `i64::MAX` units, a quantity that `engine::apply_fill` refuses
// (`InvalidQuantity`, N-1: "a fill larger than i64::MAX units") and therefore the evaluator and the verifier replay refuse. DESIGN 4.4
// does not list a quantity cap for I-1..I-12, so this is a place where the program is looser than the engine: a compromised evaluator
// can journal a position the verifier cannot replay. Severity low (the evaluator is already the trusted party for fills and the
// effect is a verifier alarm, no money moves on-chain from a fill). A fix is a plain implementation change (reject `qty > i64::MAX`
// with `QuantityInvalid` in the `Fill` arm); whether to add the check is a DESIGN decision only in the sense that I-4/I-1 never listed it.
#[test]
#[ignore = "FINDING ARI-3: post_journal accepts a Fill with qty > i64::MAX that engine::apply_fill refuses (InvalidQuantity)"]
fn finding_ari_3_fill_quantity_above_i64_max_must_be_refused() {
    let (mut w, t) = world();
    let price = ref_ask(MID, H_EUR).unwrap();
    let qty = (u64::try_from(i64::MAX).unwrap() / STEP + 1) * STEP;
    assert!(qty > u64::try_from(i64::MAX).unwrap() && qty.is_multiple_of(STEP));
    assert_eq!(engine::apply_fill(None, engine::Side::Buy, qty, price, t, 0), Err(engine::EngineError::InvalidQuantity), "the engine refuses it");
    refuse(&mut w, t, vec![fill(MARKET_EUR, true, qty, price, t, 0), meta(t, false)], vec![p_eur(t)], E::QuantityInvalid);
}

/// LIMITED, the other half of ARI-3: today's behaviour pinned exactly, so the fix has to change this test.
#[test]
fn fill_quantity_above_i64_max_is_accepted_today() {
    let (mut w, t) = world();
    let price = ref_ask(MID, H_EUR).unwrap();
    let qty = (u64::try_from(i64::MAX).unwrap() / STEP + 1) * STEP;
    let before = w.account();
    let (_, head) = accept(&mut w, t, vec![fill(MARKET_EUR, true, qty, price, t, 0), meta(t, false)], vec![p_eur(t)]);
    assert_account_is(&w, &before, head, t, |_| {});
}

// ============================================================================================================ CLOSE: P&L, QUICK HALF, CREDIT

/// What the program must do with a close, from the reference alone: `Ok((b, q, latch))` after the slice, or the exact error. The
/// order is the program's: P&L overflow is checked before everything else of the slice (the caller handles it), then
/// `b + credit` and `q + tally` (`MathOverflow`), then the quick limit `q > 0.028 S` (`QuickLimitExceeded`).
fn expect_slice(b0: i64, q0: i64, latch0: bool, pnl: i64, quick: bool) -> Result<(i64, i64, bool), E> {
    let (credit, tally) = ref_credit(pnl, quick);
    let b = b0.checked_add(credit).ok_or(E::MathOverflow)?;
    let q = q0.checked_add(tally).ok_or(E::MathOverflow)?;
    if q > QLIMIT_100K {
        return Err(E::QuickLimitExceeded);
    }
    Ok((b, q, latch0 || b >= LATCH_100K))
}

#[derive(Default, Debug)]
struct Tally {
    accepted: u32,
    pnl_overflow: u32,
    balance_or_tally_overflow: u32,
    quick_limit: u32,
    price_invalid: u32,
    ask_overflow: u32,
    refusals_checked: u32,
}

/// One close case: all variants, against the reference. `b0`, `q0`, `latch0` are already written into the account and `snapshot` is it.
#[allow(clippy::too_many_arguments)]
fn check_close_case(w: &mut JEnv, t: i64, snapshot: &RawAccount, tl: &mut Tally, long: bool, mid: u64, h: u32, qty: u64, entry: u64, age: i64, b0: i64, q0: i64, latch0: bool) {
    let timer = t - age;
    let proofs = || vec![mid_proof(t, mid)];
    let run = |events: Vec<JournalEvent>| [events, vec![meta(t, false)]].concat();
    let Some(exit) = ref_close(mid, h, long) else {
        // a short closes at the ask: it does not fit u64
        tl.ask_overflow += 1;
        refuse(w, t, run(vec![close_ev(long, qty, entry, u64::MAX, t, timer, 0, false, 0)]), proofs(), E::MathOverflow);
        return;
    };
    assert_eq!(engine::close_price_e12(mid, h, long).ok(), Some(exit), "engine vs reference close price");
    if exit == 0 {
        tl.price_invalid += 1;
        refuse(w, t, run(vec![close_ev(long, qty, entry, 0, t, timer, 0, false, 0)]), proofs(), E::PriceInvalid);
        return;
    }
    let quick = age <= 45;
    let pnl = match ref_pnl(long, qty, entry, exit) {
        Ok(p) => p,
        Err(()) => {
            tl.pnl_overflow += 1;
            assert_eq!(engine::pnl_micro(long, qty, entry, exit), Err(engine::EngineError::Overflow), "the engine also refuses");
            refuse(w, t, run(vec![close_ev(long, qty, entry, exit, t, timer, 0, quick, 0)]), proofs(), E::MathOverflow);
            return;
        }
    };
    assert_eq!(engine::pnl_micro(long, qty, entry, exit), Ok(pnl), "engine vs reference pnl");
    let (credit, tally) = ref_credit(pnl, quick);
    assert_eq!(engine::slice_credit(pnl, quick), (credit, tally), "engine vs reference credit");
    // the quick half is a floor: 2 x credit <= pnl < 2 x credit + 2 for a quick profit, i.e. never above half
    if quick && pnl > 0 {
        assert!(2 * i128::from(credit) <= i128::from(pnl) && i128::from(pnl) < 2 * i128::from(credit) + 2);
    }
    let good = close_ev(long, qty, entry, exit, t, timer, pnl, quick, credit);
    // trader-favouring or otherwise wrong claims are refused first (they do not depend on the balance or the tally)
    let mut variants: Vec<(JournalEvent, E)> = vec![
        (close_ev(long, qty, entry, exit, t, timer, pnl, !quick, credit), E::QuickMismatch),
        (close_ev(long, qty, entry, exit, t, timer, pnl, quick, credit.wrapping_add(1)), E::CreditMismatch),
        (close_ev(long, qty, entry, exit, t, timer, pnl, quick, credit.wrapping_sub(1)), E::CreditMismatch),
    ];
    if let Some(p) = pnl.checked_add(1) {
        variants.push((close_ev(long, qty, entry, exit, t, timer, p, quick, ref_credit(p, quick).0), E::PnlMismatch));
    }
    if let Some(p) = pnl.checked_sub(1) {
        variants.push((close_ev(long, qty, entry, exit, t, timer, p, quick, ref_credit(p, quick).0), E::PnlMismatch));
    }
    for (ev, want) in variants {
        tl.refusals_checked += 1;
        refuse(w, t, run(vec![ev]), proofs(), want);
    }
    match expect_slice(b0, q0, latch0, pnl, quick) {
        Err(e) => {
            match e {
                E::QuickLimitExceeded => tl.quick_limit += 1,
                _ => tl.balance_or_tally_overflow += 1,
            }
            refuse(w, t, run(vec![good]), proofs(), e);
        }
        Ok((b, q, latch)) => {
            tl.accepted += 1;
            // the engine's number for the same slice
            let mut st = engine::AccountState::new_phase(100_000_000_000);
            st.b = b0;
            st.q_tally = q0;
            st.latch = latch0;
            let d = engine::apply_closed_slice(&st, &engine::Rules::default(), credit, tally).expect("engine accepts");
            assert_eq!((d.state.b, d.state.q_tally, d.state.latch, d.fail), (b, q, latch, None), "engine vs reference slice");
            let before = w.account();
            let (_, head) = accept(w, t, run(vec![good]), proofs());
            assert_account_is(w, &before, head, t, |a| {
                a.b = b;
                a.q_tally = q;
                a.latch = u8::from(latch);
                a.trades_closed += 1;
            });
            restore(w, snapshot);
        }
    }
}

fn put_state(w: &mut JEnv, b0: i64, q0: i64, latch0: bool) -> RawAccount {
    w.edit_account(|a| {
        a.b = b0;
        a.q_tally = q0;
        a.latch = u8::from(latch0);
    });
    save(w)
}

/// CLOSE P&L, QUICK HALF AND CREDIT, RANDOM AND BOUNDARY (BLOCKED): for random and extreme quantities, entries, mids, half-spreads,
/// ages (0, 44, 45, 46, ...) and starting balances/tallies the program accepts exactly the reference `floor(sign x qty x (exit -
/// entry) / 1e12)`, `credited = floor(p / 2)` for a quick profit (else `p`) and the tally delta, with the resulting balance, tally
/// and latch equal to the reference AND to `engine::apply_closed_slice`; every claim that is off by one in the trader's favour (or
/// against him) is refused with its own code; an i128 or i64 overflow anywhere is `MathOverflow`, a tally above 0.028 S is
/// `QuickLimitExceeded`; the account is byte-identical after every refusal.
#[test]
fn close_pnl_quick_half_and_credit_match_the_reference() {
    let mut rng = Rng::for_test("close_pnl");
    let (mut w, t) = world();
    let mut tl = Tally::default();
    let starts: [(i64, i64, bool); 6] = [
        (100_000_000_000, 0, false),
        (100_000_000_000, 2_000_000_000, false),
        (103_999_999_990, 0, false),
        (0, 0, false),
        (i64::MAX - 1_000_000, 0, true),
        (i64::MIN + 1_000_000, 0, false),
    ];
    let mut cases = 0u32;
    for case in 0..450u32 {
        let (b0, q0, latch0) = starts[usize::try_from(case % 6).unwrap()];
        let snapshot = put_state(&mut w, b0, q0, latch0);
        let long = rng.chance(50);
        let age = rng.pick(&[0i64, 1, 44, 45, 46, 47, 1_000, 100_000]);
        let h = if rng.chance(30) { u32::try_from(rng.range(1, 65_535)).unwrap() } else { rng.pick(&[1u32, 30, 50]) };
        set_spread(&mut w, u16::try_from(h).unwrap());
        let (mid, qty, entry) = match rng.next() % 10 {
            // realistic sizes and prices
            0..=2 => {
                let mid = rng.range(100_000_000_000, 10_000_000_000_000);
                let qty = 1_000 * rng.range(1_000, 10_000_000_000);
                let exit = ref_close(mid, h, long).unwrap_or(mid);
                let d = rng.range(0, 100_000_000_000);
                let entry = if rng.chance(50) { exit.saturating_add(d) } else { exit.saturating_sub(d).max(1) };
                (mid, qty, entry)
            }
            // tiny quantities and tiny differences: flooring of single micro-USD
            3..=5 => {
                let mid = rng.range(1_000_000_000, 5_000_000_000_000);
                let qty = rng.range(1, 3_000_000);
                let exit = ref_close(mid, h, long).unwrap_or(mid);
                let d = rng.range(0, 3_000_000_000_000);
                let entry = if rng.chance(50) { exit.saturating_add(d) } else { exit.saturating_sub(d).max(1) };
                (mid, qty, entry)
            }
            // the full range
            _ => (rng.log().max(1), rng.log().max(1), rng.log().max(1)),
        };
        cases += 1;
        check_close_case(&mut w, t, &snapshot, &mut tl, long, mid, h, qty, entry.max(1), age, b0, q0, latch0);
    }
    println!("close_pnl: {cases} cases: {tl:?}");
    assert!(tl.accepted >= 80, "too few accepted closes: {tl:?}");
    assert!(tl.pnl_overflow >= 10 && tl.balance_or_tally_overflow >= 5 && tl.quick_limit >= 5, "the generator must reach every outcome: {tl:?}");
}

/// A close case written by hand: the exit is the reference close price of `mid`.
struct C {
    long: bool,
    mid: u64,
    qty: u64,
    /// entry = exit - `entry_below` (a signed offset, so the trader's P&L per unit is `entry_below` e12 for a long).
    entry_below: i128,
    age: i64,
}
const H_CLOSE: u32 = 1;
impl C {
    fn exit(&self) -> u64 {
        ref_close(self.mid, H_CLOSE, self.long).expect("exit fits")
    }
    fn entry(&self) -> u64 {
        u64::try_from(i128::from(self.exit()) - self.entry_below).expect("entry fits")
    }
    /// Event with every claim computed by the reference. Panics if the reference P&L overflows.
    fn good(&self, t: i64) -> JournalEvent {
        let (entry, exit) = (self.entry(), self.exit());
        let pnl = ref_pnl(self.long, self.qty, entry, exit).expect("pnl fits");
        let quick = self.age <= 45;
        close_ev(self.long, self.qty, entry, exit, t, t - self.age, pnl, quick, ref_credit(pnl, quick).0)
    }
    /// Event with arbitrary claims for pnl / credited (used for overflow and mismatch cases).
    fn claim(&self, t: i64, timer: i64, pnl: i64, quick: bool, credited: i64) -> JournalEvent {
        close_ev(self.long, self.qty, self.entry(), self.exit(), t, timer, pnl, quick, credited)
    }
}
fn with_meta(ev: JournalEvent, t: i64) -> Vec<JournalEvent> {
    vec![ev, meta(t, false)]
}

/// CLOSE, THE i64 AND i128 LIMITS (BLOCKED and exact): a P&L of exactly `i64::MAX` and `i64::MIN` is accepted when the balance
/// has room and is `MathOverflow` one micro-USD more; `qty = u64::MAX` against a price difference of `u64::MAX - 1` overflows
/// `i128` and is `MathOverflow` (never a panic, never a wrapped number); `qty = 2^63` with a difference of `2^63` fits `i128` but not
/// `i64`: `MathOverflow`; a short that closes at an ask above `u64::MAX` is `MathOverflow`.
#[test]
fn close_i64_and_i128_limits() {
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    let i64max = i128::from(i64::MAX);
    let q = 1_000_000_000_000u64; // 1e6 units: pnl == (exit - entry) in micro-USD
    // P&L == i64::MAX exactly (long, exit - entry == i64::MAX)
    let top = C { long: true, mid: 15_000_000_000_000_000_000, qty: q, entry_below: i64max, age: 1_000 };
    assert_eq!(ref_pnl(true, q, top.entry(), top.exit()), Ok(i64::MAX));
    assert_eq!(engine::pnl_micro(true, q, top.entry(), top.exit()), Ok(i64::MAX), "the engine agrees");
    for (b0, ok) in [(0i64, true), (1, false), (i64::MAX, false), (-1, true), (i64::MIN, true)] {
        let snapshot = put_state(&mut w, b0, 0, false);
        if ok {
            let before = w.account();
            let (_, head) = accept(&mut w, t, with_meta(top.good(t), t), vec![mid_proof(t, top.mid)]);
            let b = b0 + i64::MAX; // literal: no overflow in these rows
            assert_account_is(&w, &before, head, t, |a| {
                a.b = b;
                a.latch = u8::from(b >= LATCH_100K);
                a.trades_closed += 1;
            });
            restore(&mut w, &snapshot);
        } else {
            refuse(&mut w, t, with_meta(top.good(t), t), vec![mid_proof(t, top.mid)], E::MathOverflow);
        }
    }
    // after crediting i64::MAX to b = 0 the balance IS i64::MAX and latch is set (literal check)
    put_state(&mut w, 0, 0, false);
    accept(&mut w, t, with_meta(top.good(t), t), vec![mid_proof(t, top.mid)]);
    assert_eq!((w.account().b, w.account().latch), (i64::MAX, 1));
    // P&L == i64::MAX + 1: the claim cannot even be written, the program says MathOverflow
    let over = C { entry_below: i64max + 1, ..top };
    assert_eq!(ref_pnl(true, q, over.entry(), over.exit()), Err(()));
    put_state(&mut w, 0, 0, false);
    refuse(&mut w, t, with_meta(over.claim(t, t - 1_000, i64::MAX, false, i64::MAX), t), vec![mid_proof(t, over.mid)], E::MathOverflow);
    // P&L == i64::MIN exactly (long, exit - entry == -2^63), then one more
    let low = C { long: true, mid: 5_000_000_000_000_000_000, qty: q, entry_below: -(i64max + 1), age: 1_000 };
    assert_eq!(ref_pnl(true, q, low.entry(), low.exit()), Ok(i64::MIN));
    assert_eq!(engine::pnl_micro(true, q, low.entry(), low.exit()), Ok(i64::MIN));
    put_state(&mut w, 0, 0, false);
    let before = w.account();
    let (_, head) = accept(&mut w, t, with_meta(low.good(t), t), vec![mid_proof(t, low.mid)]);
    assert_account_is(&w, &before, head, t, |a| {
        a.b = i64::MIN;
        a.trades_closed += 1;
    });
    put_state(&mut w, -1, 0, false);
    refuse(&mut w, t, with_meta(low.good(t), t), vec![mid_proof(t, low.mid)], E::MathOverflow);
    let lower = C { entry_below: -(i64max + 2), ..low };
    assert_eq!(ref_pnl(true, q, lower.entry(), lower.exit()), Err(()));
    put_state(&mut w, 0, 0, false);
    refuse(&mut w, t, with_meta(lower.claim(t, t - 1_000, i64::MIN, false, i64::MIN), t), vec![mid_proof(t, lower.mid)], E::MathOverflow);
    // floor, not truncation: a loss of one e12 unit on one unit of quantity is -1 micro-USD, a profit of one is 0
    let loss1 = C { long: true, mid: MID, qty: 1, entry_below: -1, age: 1_000 };
    assert_eq!(ref_pnl(true, 1, loss1.entry(), loss1.exit()), Ok(-1));
    put_state(&mut w, S100K, 0, false);
    let before = w.account();
    let (_, head) = accept(&mut w, t, with_meta(loss1.good(t), t), vec![mid_proof(t, MID)]);
    assert_account_is(&w, &before, head, t, |a| {
        a.b = S100K - 1;
        a.trades_closed += 1;
    });
    let gain1 = C { entry_below: 1, ..loss1 };
    assert_eq!(ref_pnl(true, 1, gain1.entry(), gain1.exit()), Ok(0));
    // the largest quantity against the largest price difference: i128 product overflow, then i128 fits but i64 does not
    put_state(&mut w, 0, 0, false);
    let mid = u64::MAX / 2;
    let huge = C { long: true, mid, qty: u64::MAX, entry_below: i128::from(ref_close(mid, H_CLOSE, true).unwrap()) - 1, age: 1_000 };
    assert_eq!(huge.entry(), 1);
    assert_eq!(ref_pnl(true, u64::MAX, 1, huge.exit()), Err(()));
    assert_eq!(engine::pnl_micro(true, u64::MAX, 1, huge.exit()), Err(engine::EngineError::Overflow));
    refuse(&mut w, t, with_meta(huge.claim(t, t - 1_000, 0, false, 0), t), vec![mid_proof(t, mid)], E::MathOverflow);
    // qty = 2^63, diff = 2^63 (a long closing 2^63 below its entry... as a profit): product 2^126 fits i128, the result 8.5e25 does not fit i64
    let mid2 = 1u64 << 63;
    let mid_ok = C { long: true, mid: mid2, qty: 1 << 63, entry_below: i128::from(ref_close(mid2, H_CLOSE, true).unwrap()) - 1, age: 1_000 };
    assert!(i128::from(1u64 << 63).checked_mul(i128::from(mid_ok.exit()) - 1).is_some(), "the product fits i128");
    assert_eq!(ref_pnl(true, 1 << 63, 1, mid_ok.exit()), Err(()));
    refuse(&mut w, t, with_meta(mid_ok.claim(t, t - 1_000, 0, false, 0), t), vec![mid_proof(t, mid2)], E::MathOverflow);
    // a short closes at the ask: at mid = u64::MAX the ask does not fit u64 (MathOverflow), at the last mid that fits it is accepted
    let m = max_mid_for_ask(H_CLOSE);
    let short_over = C { long: false, mid: m + 1, qty: q, entry_below: 0, age: 1_000 };
    refuse(&mut w, t, with_meta(close_ev(false, q, 5, u64::MAX, t, t - 1_000, 0, false, 0), t), vec![mid_proof(t, short_over.mid)], E::MathOverflow);
    let short_ok = C { long: false, mid: m, qty: 1, entry_below: 0, age: 1_000 };
    assert_eq!(short_ok.exit(), u64::MAX);
    put_state(&mut w, S100K, 0, false);
    accept(&mut w, t, with_meta(short_ok.good(t), t), vec![mid_proof(t, m)]);
}

/// QUICK HALF, TALLY AND LATCH BOUNDARIES (BLOCKED and exact): the 45 s edge (45 quick, 46 not), floor(p/2) for p = 1, 2, 3, 11
/// (credit 0, 1, 1, 5), a quick LOSS credited in full with no tally, `q = 0.028 S` accepted and `q = 0.028 S + 1` is
/// `QuickLimitExceeded` (strict), `q + tally` beyond `i64::MAX` is `MathOverflow` (checked before the limit), the latch at exactly 1.04 S.
#[test]
fn close_quick_half_tally_and_latch_boundaries() {
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    let q = 1_000_000_000_000u64;
    let mk = |pnl: i128, age: i64| C { long: true, mid: MID, qty: q, entry_below: pnl, age };
    // (pnl, age, start b, start q, expected credit, expected tally): written by hand from DESIGN 7.2 / 7.4
    let rows: [(i128, i64, i64, i64, i64, i64); 12] = [
        (1, 45, S100K, 0, 0, 0),    // quick profit of 1 micro: floor(1/2) = 0 credited, nothing in the tally
        (2, 45, S100K, 0, 1, 1),
        (3, 45, S100K, 0, 1, 1),    // floor(3/2) = 1, never 2
        (11, 45, S100K, 0, 5, 5),
        (11, 46, S100K, 0, 11, 0),  // one second later: not quick, credited in full, tally untouched
        (-3, 45, S100K, 0, -3, 0),  // a quick loss is credited in full, tally untouched
        (0, 45, S100K, 0, 0, 0),
        (10, 45, S100K, QLIMIT_100K - 5, 5, 5),  // q lands exactly on 0.028 S: still allowed (strict >)
        (11, 45, S100K, QLIMIT_100K - 5, 5, 5),  // credit 5 again
        (1_000_000_000, 1_000, LATCH_100K - 1_000_000_000, 0, 1_000_000_000, 0), // b lands exactly on 1.04 S: latch
        (999_999_999, 1_000, LATCH_100K - 1_000_000_000, 0, 999_999_999, 0),      // one micro short of 1.04 S: no latch
        (4, 0, S100K, 0, 2, 2),     // age 0
    ];
    for (pnl, age, b0, q0, credit, tally) in rows {
        let c = mk(pnl, age);
        let snapshot = put_state(&mut w, b0, q0, false);
        let ev = c.good(t);
        if let JournalEvent::Close { pnl: p, credited, quick, .. } = &ev {
            assert_eq!((i128::from(*p), *credited, *quick), (pnl, credit, age <= 45), "hand table vs event");
        }
        let before = w.account();
        let (_, head) = accept(&mut w, t, with_meta(ev, t), vec![mid_proof(t, MID)]);
        assert_account_is(&w, &before, head, t, |a| {
            a.b = b0 + credit;
            a.q_tally = q0 + tally;
            a.latch = u8::from(b0 + credit >= LATCH_100K);
            a.trades_closed += 1;
        });
        restore(&mut w, &snapshot);
    }
    // the tally one above the limit: pnl 12 quick credits 6, q = limit - 5 + 6 = limit + 1
    put_state(&mut w, S100K, QLIMIT_100K - 5, false);
    refuse(&mut w, t, with_meta(mk(12, 45).good(t), t), vec![mid_proof(t, MID)], E::QuickLimitExceeded);
    // q + tally beyond i64::MAX is MathOverflow, checked before the limit
    put_state(&mut w, S100K, i64::MAX - 1, false);
    refuse(&mut w, t, with_meta(mk(4, 45).good(t), t), vec![mid_proof(t, MID)], E::MathOverflow);
    // q already at i64::MAX and a LOSS (tally 0): no overflow, but the tally is over the limit
    put_state(&mut w, S100K, i64::MAX, false);
    refuse(&mut w, t, with_meta(mk(-3, 45).good(t), t), vec![mid_proof(t, MID)], E::QuickLimitExceeded);
    // the engine's slice credit for the same table (not the program's code)
    for (pnl, age, _, _, credit, tally) in rows {
        assert_eq!(engine::slice_credit(i64::try_from(pnl).unwrap(), age <= 45), (credit, tally), "engine vs hand table, pnl {pnl} age {age}");
    }
}

/// CLOSE, DEGENERATE CLAIMS (BLOCKED): zero quantity `QuantityInvalid`, entry 0 and exit 0 `PriceInvalid`, an exit that is not the
/// close price of the proof `EventPriceMismatch`, a timer after the close `TimerInvalid` (also `i64::MAX`), a timer so old that the
/// age overflows `i64` (`i64::MIN`) `MathOverflow`, a negative timer is an ordinary huge age (not quick), and a quantity of 1
/// (below the market minimum) is fine for a CLOSE (only fills check the step and the minimum).
#[test]
fn close_degenerate_claims() {
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    let c = C { long: true, mid: MID, qty: 1_000_000_000_000, entry_below: 5, age: 1_000 };
    let (entry, exit) = (c.entry(), c.exit());
    let pnl = ref_pnl(true, c.qty, entry, exit).unwrap();
    let ev = |qty: u64, entry: u64, exit: u64, timer: i64| with_meta(close_ev(true, qty, entry, exit, t, timer, pnl, false, pnl), t);
    let p = || vec![mid_proof(t, MID)];
    refuse(&mut w, t, ev(0, entry, exit, t - 1_000), p(), E::QuantityInvalid);
    refuse(&mut w, t, ev(c.qty, 0, exit, t - 1_000), p(), E::PriceInvalid);
    refuse(&mut w, t, ev(c.qty, entry, 0, t - 1_000), p(), E::PriceInvalid);
    refuse(&mut w, t, ev(c.qty, entry, exit + 1, t - 1_000), p(), E::EventPriceMismatch);
    refuse(&mut w, t, ev(c.qty, entry, exit - 1, t - 1_000), p(), E::EventPriceMismatch);
    refuse(&mut w, t, ev(c.qty, entry, u64::MAX, t - 1_000), p(), E::EventPriceMismatch);
    refuse(&mut w, t, ev(c.qty, entry, exit, t + 1), p(), E::TimerInvalid);
    refuse(&mut w, t, ev(c.qty, entry, exit, i64::MAX), p(), E::TimerInvalid);
    refuse(&mut w, t, ev(c.qty, entry, exit, i64::MIN), p(), E::MathOverflow);
    // a negative timer: age = t + 5 (about 1.7e9 s), not quick; the honest claim is accepted
    let good = close_ev(true, c.qty, entry, exit, t, -5, pnl, false, pnl);
    accept(&mut w, t, with_meta(good, t), p());
    // the same claim with quick = true is refused (age is far above 45)
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    refuse(&mut w, t, with_meta(close_ev(true, c.qty, entry, exit, t, -5, pnl, true, pnl / 2), t), p(), E::QuickMismatch);
    // quantity 1 closes fine
    let one = C { qty: 1, ..c };
    accept(&mut w, t, with_meta(one.good(t), t), p());
    // LIMITED (OFF-CHAIN: positions live in the evaluator): a close of u64::MAX units at price 1 with entry == exit has pnl 0 and is accepted; the program
    // cannot know that no such position exists. Only trades_closed, seq, hash and last_batch_time change.
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    let tiny = C { long: true, mid: 2, qty: u64::MAX, entry_below: 0, age: 1_000 };
    assert_eq!((tiny.exit(), tiny.entry()), (1, 1), "mid 2 closes a long at 1 (floor(2 x 0.999999))");
    let before = w.account();
    let (_, head) = accept(&mut w, t, with_meta(tiny.good(t), t), vec![mid_proof(t, 2)]);
    assert_account_is(&w, &before, head, t, |a| a.trades_closed += 1);
}

// ============================================================================================================ FEES

fn expect_fee(b0: i64, amount: u64) -> Result<i64, E> {
    let a = i64::try_from(amount).map_err(|_| E::FeeInvalid)?;
    b0.checked_sub(a).ok_or(E::MathOverflow)
}

/// FEE AMOUNTS AT THE i64 LIMITS (BLOCKED + LIMITED): an amount above `i64::MAX` is `FeeInvalid`, a debit that would take the balance
/// below `i64::MIN` is `MathOverflow`, a debit that lands exactly on `i64::MIN` is accepted, a fee of `i64::MAX` is ACCEPTED while
/// the batch does not end flat (LIMITED: the program has no cap on a fee beyond the balance arithmetic; DESIGN 9.3 "the program only
/// checks that b is debited by exactly that amount", the amount itself is the VERIFIER's check) and `MustReportFail` when it ends flat at
/// or below the static floor. Two fees whose SUM overflows roll the whole batch back. 150 random amounts and balances against the reference.
#[test]
fn fee_amounts_at_the_i64_limits() {
    let mut rng = Rng::for_test("fee");
    let (mut w, t) = world();
    for kind in [FeeKind::Holding, FeeKind::Commission] {
        let run = |amount: u64, flat: bool| vec![fee_ev(kind, amount, t), meta(t, flat)];
        // above i64::MAX
        for amount in [1u64 << 63, (1 << 63) + 1, u64::MAX - 1, u64::MAX] {
            let _ = put_state(&mut w, S100K, 0, false);
            refuse(&mut w, t, run(amount, false), vec![], E::FeeInvalid);
        }
        // i64::MAX from the opening balance: accepted, the balance is S - i64::MAX (negative), literal
        let snapshot = put_state(&mut w, S100K, 0, false);
        let before = w.account();
        let (_, head) = accept(&mut w, t, run(i64::MAX as u64, false), vec![]);
        assert_account_is(&w, &before, head, t, |a| a.b = S100K - i64::MAX);
        assert_eq!(w.account().b, 100_000_000_000 - 9_223_372_036_854_775_807);
        // ... and the same fee in a batch that ends flat is a certain fail: report_fail only
        restore(&mut w, &snapshot);
        refuse(&mut w, t, run(i64::MAX as u64, true), vec![], E::MustReportFail);
        // the second of two fees overflows: the first one is rolled back with it
        refuse(&mut w, t, vec![fee_ev(kind, i64::MAX as u64, t), fee_ev(kind, i64::MAX as u64, t), meta(t, false)], vec![], E::MathOverflow);
        // balance at the bottom
        put_state(&mut w, i64::MIN + 5, 0, false);
        refuse(&mut w, t, run(6, false), vec![], E::MathOverflow);
        refuse(&mut w, t, run(i64::MAX as u64, false), vec![], E::MathOverflow);
        let snapshot = save(&w);
        let before = w.account();
        let (_, head) = accept(&mut w, t, run(5, false), vec![]);
        assert_account_is(&w, &before, head, t, |a| a.b = i64::MIN);
        restore(&mut w, &snapshot);
        // fee 0 changes nothing but the chain
        let before = w.account();
        let (_, head) = accept(&mut w, t, run(0, false), vec![]);
        assert_account_is(&w, &before, head, t, |_| {});
        // at i64::MIN even one micro-USD is an overflow
        put_state(&mut w, i64::MIN, 0, false);
        refuse(&mut w, t, run(1, false), vec![], E::MathOverflow);
        // the static floor, to the micro-USD: b = F + 5; fee 4 leaves F + 1 (allowed), fee 5 lands ON the floor (a fail)
        let snapshot = put_state(&mut w, FLOOR_100K + 5, 0, false);
        let before = w.account();
        let (_, head) = accept(&mut w, t, run(4, true), vec![]);
        assert_account_is(&w, &before, head, t, |a| {
            a.b = FLOOR_100K + 1;
            a.flat = 1;
        });
        restore(&mut w, &snapshot);
        refuse(&mut w, t, run(5, true), vec![], E::MustReportFail);
        refuse(&mut w, t, run(6, true), vec![], E::MustReportFail);
    }
    // random amounts and balances
    let (mut accepted, mut invalid, mut overflow) = (0u32, 0u32, 0u32);
    for _ in 0..150 {
        let b0 = match rng.next() % 4 {
            0 => S100K,
            1 => i64::MIN + i64::try_from(rng.range(0, 1 << 40)).unwrap(),
            2 => i64::MAX - i64::try_from(rng.range(0, 1 << 40)).unwrap(),
            _ => i64::try_from(rng.log() >> 1).unwrap() * if rng.chance(50) { 1 } else { -1 },
        };
        let amount = match rng.next() % 4 {
            0 => rng.log(),
            1 => u64::try_from(i64::MAX).unwrap() - rng.range(0, 1 << 40),
            2 => rng.range(0, 2_000_000_000),
            _ => (1 << 63) + rng.range(0, 1 << 40),
        };
        let kind = if rng.chance(50) { FeeKind::Holding } else { FeeKind::Commission };
        let snapshot = put_state(&mut w, b0, 0, false);
        let ev = vec![fee_ev(kind, amount, t), meta(t, false)];
        // the engine's number for the same debit (the program uses `apply_closed_slice` with a negative credit)
        let mut st = engine::AccountState::new_phase(100_000_000_000);
        st.b = b0;
        let engine_says = i64::try_from(amount).ok().map(|a| engine::apply_closed_slice(&st, &engine::Rules::default(), -a, 0));
        match (&engine_says, expect_fee(b0, amount)) {
            (None, Err(E::FeeInvalid)) => {}
            (Some(Err(engine::EngineError::Overflow)), Err(E::MathOverflow)) => {}
            (Some(Ok(d)), Ok(b)) => assert_eq!((d.state.b, d.state.latch), (b, b >= LATCH_100K), "engine vs reference fee"),
            other => panic!("engine and reference disagree on a fee of {amount} from {b0}: {other:?}"),
        }
        match expect_fee(b0, amount) {
            Err(e) => {
                if matches!(e, E::FeeInvalid) { invalid += 1 } else { overflow += 1 }
                refuse(&mut w, t, ev, vec![], e);
            }
            Ok(b) => {
                accepted += 1;
                let before = w.account();
                let (_, head) = accept(&mut w, t, ev, vec![]);
                assert_account_is(&w, &before, head, t, |a| {
                    a.b = b;
                    a.latch = u8::from(b >= LATCH_100K);
                });
                restore(&mut w, &snapshot);
            }
        }
    }
    println!("fee: random 150 cases: {accepted} accepted, {invalid} FeeInvalid, {overflow} MathOverflow");
    assert!(accepted >= 20 && invalid >= 20 && overflow >= 5);
}

/// `ceil(nights x rate x qty x mark / (1e12 x 1e6))` with the engine's own order of u128 products; `None` when a product leaves u128.
fn ref_holding(qty: u64, mark: u64, rate: u32, nights: u32) -> Option<u128> {
    let n = u128::from(qty).checked_mul(u128::from(mark))?.checked_mul(u128::from(rate))?.checked_mul(u128::from(nights))?;
    let d = 1_000_000_000_000u128 * 1_000_000u128;
    Some(n / d + u128::from(n % d != 0))
}

/// HOLDING FEE THROUGH THE PROGRAM (LIMITED, OFF-CHAIN formula): for extreme quantity, mark, rate and number of nights the engine
/// either returns `Overflow` (then no event can be written; the reference agrees that the u128 chain overflows or the fee exceeds
/// `i64::MAX`) or a fee `f` with `f x 1e18 >= n > (f - 1) x 1e18` (a ceiling, never below the exact fee). The program DEBITS that
/// amount exactly, including the absurd ones (a fee of 4.29e18 micro-USD after `u32::MAX` nights): it does not recompute holding
/// fees, the VERIFIER does (DESIGN 9.3). Pinned so a later program-side check shows up as a test change.
#[test]
fn holding_fee_extremes_engine_reference_and_program_debit() {
    let (mut w, t) = world();
    let big = u64::MAX;
    // (qty, mark, rate_e6, nights)
    let grid: [(u64, u64, u32, u32); 14] = [
        (1_000_000_000_000, MID, 100, 1),
        (1_000_000_000_000, MID, 1_600, 3),
        (1, 1, 1, 1),                      // a fraction of a micro-USD: the ceiling makes it 1
        (1_000_000, MID, 100, 0),          // zero nights: nothing owed
        (big, MID, 1_600, 1),
        (big, big, 1_600, 1),              // u128 product 3.4e38 x 1600 overflows
        (big, big / 2, 1, 1),
        (1_000_000_000_000, MID, 1_000, u32::MAX),
        (1_000_000_000_000, MID, u32::MAX, u32::MAX),
        (1 << 40, 1 << 50, 1_000, 36_500), // a century of nights on a large position
        (999_999_999_999, 999_999_999_999, 1, 1), // just below one micro-USD per e18
        (1_000_000_000_001, 1_000_000_000_000, 1, 1),
        (big, 1, u32::MAX, u32::MAX),
        (1 << 63, 1 << 63, 1, 1),
    ];
    let (mut posted, mut engine_overflow) = (0u32, 0u32);
    for (qty, mark, rate, nights) in grid {
        let e = engine::holding_fee_micro(qty, mark, rate, nights);
        let r = ref_holding(qty, mark, rate, nights);
        match (e, r) {
            (Ok(f), Some(exact_ceil)) => {
                assert_eq!(u128::try_from(f).unwrap(), exact_ceil, "engine vs reference holding fee ({qty},{mark},{rate},{nights})");
                posted += 1;
                let snapshot = put_state(&mut w, S100K, 0, false);
                let before = w.account();
                let amount = u64::try_from(f).unwrap();
                let (_, head) = accept(&mut w, t, vec![fee_ev(FeeKind::Holding, amount, t), meta(t, false)], vec![]);
                assert_account_is(&w, &before, head, t, |a| a.b = S100K - f);
                restore(&mut w, &snapshot);
            }
            (Err(engine::EngineError::Overflow), Some(c)) => {
                assert!(c > u128::try_from(i64::MAX).unwrap(), "the engine refuses a fee that fits i64: {c} ({qty},{mark},{rate},{nights})");
                engine_overflow += 1;
            }
            (Err(engine::EngineError::Overflow), None) => engine_overflow += 1,
            other => panic!("engine and reference disagree for ({qty},{mark},{rate},{nights}): {other:?}"),
        }
    }
    println!("holding fee: {posted} fees posted through the program, {engine_overflow} engine overflows");
    assert!(posted >= 7 && engine_overflow >= 4);
    // the huge-age fee is accepted by the program (LIMITED, pinned): b = S - 4_294_967_295_000_000_000
    let f = engine::holding_fee_micro(1_000_000_000_000, MID, 1_000, u32::MAX).unwrap();
    assert_eq!(f, 4_294_967_295_000_000_000);
}

// ============================================================================================================ ACCOUNT SIZE S

fn set_size(w: &mut JEnv, s: u64, b: i64) {
    w.edit_account(|a| {
        a.size_s = s;
        a.b = b;
        a.q_tally = 0;
        a.latch = 0;
    });
}

/// THE ACCOUNT SIZE AT BOTH ENDS (BLOCKED, exact codes): for S = 0, 1 USD, a size that is not whole dollars, the 2,500 and 200,000 tiers, the
/// largest S for which 1.04 S fits `i64`, a larger one for which it does not, `i64::MAX`, 2^63 and `u64::MAX`, three batches are tried:
/// A `[Meta]` (touches no threshold), B `[Fee 1, Meta]` (`b >= 1.04 S`, `Q > 0.028 S`), C `[Meta flat]` (static floor 0.92 S). The expected
/// result of every cell is written by hand from the engine's exactness rule (`S x bps / 10_000` exact, result in `i64`): an unusable size is
/// `MathOverflow`, never a panic, never a wrapped threshold.
#[test]
fn account_size_extremes_smallest_largest_and_unusable() {
    let (mut w, t) = world();
    const M: u64 = 8_868_000_000_000_000_000; // 1.04 M = 9.22272e18 <= i64::MAX
    const M2: u64 = 8_870_000_000_000_000_000; // 1.04 M2 = 9.2248e18 > i64::MAX
    const M3: u64 = 9_223_372_036_000_000_000; // whole dollars, <= i64::MAX, 1.04 M3 overflows
    // (S, name, A, B, C) with None = accepted
    let ok = None;
    let ov = Some(E::MathOverflow);
    let rows: [(u64, &str, Option<E>, Option<E>, Option<E>); 13] = [
        (0, "S = 0", ok, ov, ov),
        (1_000_000, "S = 1 USD", ok, ok, ok),
        (1_000_001, "S = 1 USD + 1 micro (not whole dollars)", ok, ov, ov),
        (2_500_000_000, "smallest tier 2,500", ok, ok, ok),
        (200_000_000_000, "largest tier 200,000", ok, ok, ok),
        (M, "largest S with 1.04 S in i64", ok, ok, ok),
        (M2, "1.04 S overflows i64", ok, ov, ok),
        (M3, "whole dollars just below i64::MAX", ok, ov, ok),
        (i64::MAX as u64, "i64::MAX (odd)", ok, ov, ov),
        (1 << 63, "2^63", ov, ov, ov),
        (u64::MAX, "u64::MAX", ov, ov, ov),
        (u64::MAX - u64::MAX % 1_000_000, "largest whole dollars in u64", ov, ov, ov),
        (25, "25 micro-USD (0.92 S = 23 and 1.04 S = 26 exact, 0.028 S = 0.7 not)", ok, ov, ok),
    ];
    for (s, _name, a, b_, c) in rows {
        let b = i64::try_from(s).unwrap_or(0);
        let batches: [(&str, Vec<JournalEvent>, Option<E>); 3] = [
            ("A [Meta]", vec![meta(t, false)], a),
            ("B [Fee 1, Meta]", vec![fee_ev(FeeKind::Commission, 1, t), meta(t, false)], b_),
            ("C [Meta flat]", vec![meta(t, true)], c),
        ];
        for (label, events, want) in batches {
            set_size(&mut w, s, b);
            let snapshot = save(&w);
            let before = w.account();
            match want {
                Some(e) => refuse(&mut w, t, events, vec![], e),
                None => {
                    let flat_batch = label.starts_with("C");
                    let fee_batch = label.starts_with("B");
                    let (_, head) = accept(&mut w, t, events, vec![]);
                    assert_account_is(&w, &before, head, t, |acc| {
                        if flat_batch {
                            acc.flat = 1;
                        }
                        if fee_batch {
                            acc.b -= 1;
                        }
                    });
                }
            }
            restore(&mut w, &snapshot);
        }
    }
}

/// Hand-written thresholds of an account size `s` (micro-USD): 0.92 S, 0.96 S, 1.04 S, 0.028 S.
fn hand(s: i64) -> (i64, i64, i64, i64) {
    (s / 100 * 92, s / 100 * 96, s / 100 * 104, s / 10_000 * 280)
}

/// THRESHOLD BOUNDARIES AT THE SMALLEST AND LARGEST SIZE (BLOCKED + exact): for S = 1 USD, 2,500 USD (smallest tier) and 200,000 USD (largest
/// tier), to the micro-USD: the static-floor fail (a flat batch landing ON 0.92 S is `MustReportFail`, one above is accepted), the latched floor
/// 0.96 S, the latch at exactly 1.04 S, the quick limit 0.028 S (equal allowed, one above `QuickLimitExceeded`). All thresholds are written by hand
/// (`hand`) and compared with `engine::frac_exact` as well.
#[test]
fn thresholds_to_the_micro_usd_at_the_smallest_and_largest_size() {
    let (mut w, t) = world();
    for s in [1_000_000i64, 2_500_000_000, 200_000_000_000] {
        let (floor, lfloor, latch, qlim) = hand(s);
        assert_eq!(engine::frac_exact(s, 9_200), Ok(floor));
        assert_eq!(engine::frac_exact(s, 9_600), Ok(lfloor));
        assert_eq!(engine::frac_exact(s, 10_400), Ok(latch));
        assert_eq!(engine::frac_exact(s, 280), Ok(qlim));
        let fee = |amount: i64, flat: bool| vec![fee_ev(FeeKind::Commission, u64::try_from(amount).unwrap(), t), meta(t, flat)];
        let size_u64 = u64::try_from(s).unwrap();
        // static floor 0.92 S
        for (b0, latched, lvl) in [(floor + 5, false, floor), (lfloor + 5, true, lfloor)] {
            set_size(&mut w, size_u64, b0);
            w.edit_account(|a| a.latch = u8::from(latched));
            let snapshot = save(&w);
            let before = w.account();
            let (_, head) = accept(&mut w, t, fee(4, true), vec![]);
            assert_account_is(&w, &before, head, t, |a| {
                a.b = lvl + 1;
                a.flat = 1;
            });
            restore(&mut w, &snapshot);
            refuse(&mut w, t, fee(5, true), vec![], E::MustReportFail);
            refuse(&mut w, t, fee(6, true), vec![], E::MustReportFail);
            // not flat: the same debit onto the floor is accepted (the account is only failed by report_fail)
            accept(&mut w, t, fee(5, false), vec![]);
        }
        // a non-quick profit of exactly the room up to 1.04 S latches, one micro less does not
        let c = |pnl: i128| C { long: true, mid: MID, qty: 1_000_000_000_000, entry_below: pnl, age: 1_000 };
        set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
        for (pnl, want_latch) in [(1_000i128, true), (999, false)] {
            set_size(&mut w, size_u64, latch - 1_000);
            let snapshot = save(&w);
            let before = w.account();
            let (_, head) = accept(&mut w, t, with_meta(c(pnl).good(t), t), vec![mid_proof(t, MID)]);
            assert_account_is(&w, &before, head, t, |a| {
                a.b = latch - 1_000 + i64::try_from(pnl).unwrap();
                a.latch = u8::from(want_latch);
                a.trades_closed += 1;
            });
            restore(&mut w, &snapshot);
        }
        // quick limit: a quick profit credits floor(p/2) to the tally; q = limit allowed, limit + 1 refused
        for (q0, pnl, ok) in [(qlim - 5, 10i128, true), (qlim - 5, 11, true), (qlim - 5, 12, false), (qlim, 1, true), (qlim, 2, false), (qlim + 1, -7, false)] {
            set_size(&mut w, size_u64, s);
            w.edit_account(|a| a.q_tally = q0);
            // a quick close (age 10 s)
            let ev = with_meta(C { age: 10, ..c(pnl) }.good(t), t);
            if ok {
                let snapshot = save(&w);
                let before = w.account();
                let (_, head) = accept(&mut w, t, ev, vec![mid_proof(t, MID)]);
                let credit = if pnl > 0 { i64::try_from(pnl / 2).unwrap() } else { i64::try_from(pnl).unwrap() };
                assert_account_is(&w, &before, head, t, |a| {
                    a.b = s + credit;
                    a.q_tally = q0 + credit.max(0);
                    a.trades_closed += 1;
                });
                restore(&mut w, &snapshot);
            } else {
                refuse(&mut w, t, ev, vec![mid_proof(t, MID)], E::QuickLimitExceeded);
            }
        }
    }
}

/// TIGHT START AT THE SMALLEST AND LARGEST SIZE (BLOCKED + exact): `p_open > 0.01 S` strictly, `r == b`, `l_start = min(0.92 S + 0.04 S, r)`,
/// `l_level = l_start + max(0, g_peak - 0.01 S)`, all written by hand for S = 1 USD and S = 200,000 USD; every claim one micro-USD off is refused with its
/// own code; `p_open = g_peak = i64::MAX` overflows the level (`MathOverflow`); `g_peak < 0` and `p_open = i64::MIN` have their own codes.
#[test]
fn tight_start_at_the_smallest_and_largest_size_and_i64_extremes() {
    let (mut w, t) = world();
    for s in [1_000_000i64, 200_000_000_000] {
        let (floor, _, _, _) = hand(s);
        let one_pct = s / 100;
        let cap = floor + s / 25; // 0.92 S + 0.04 S
        let l_start = cap.min(s); // r = b = S
        assert_eq!(l_start, s / 100 * 96, "hand check");
        let tight = |p_open: i64, r: i64, ls: i64, g: i64, ll: i64| {
            vec![JournalEvent::TightStart { time: t, p_open, r, l_start: ls, g_peak: g, l_level: ll, proof: 0 }, meta(t, false)]
        };
        set_size(&mut w, u64::try_from(s).unwrap(), s);
        let snapshot = save(&w);
        let p = || vec![mid_proof(t, MID)];
        // exactly 0.01 S is NOT enough (strict), one micro more is
        refuse(&mut w, t, tight(one_pct, s, l_start, one_pct, l_start), p(), E::TightStartPOpen);
        refuse(&mut w, t, tight(i64::MIN, s, l_start, 0, l_start), p(), E::TightStartPOpen);
        refuse(&mut w, t, tight(one_pct + 1, s, l_start, -1, l_start), p(), E::TightStartGPeak);
        refuse(&mut w, t, tight(one_pct + 1, s, l_start, one_pct + 2, l_start), p(), E::TightStartGPeak);
        refuse(&mut w, t, tight(one_pct + 1, s - 1, l_start, one_pct + 1, l_start + 1), p(), E::TightStartR);
        refuse(&mut w, t, tight(one_pct + 1, s, l_start - 1, one_pct + 1, l_start), p(), E::TightStartLStart);
        refuse(&mut w, t, tight(one_pct + 1, s, l_start + 1, one_pct + 1, l_start + 2), p(), E::TightStartLStart);
        // l_level = l_start + max(0, g - 0.01 S) = l_start + 1 for g = 0.01 S + 1
        refuse(&mut w, t, tight(one_pct + 1, s, l_start, one_pct + 1, l_start), p(), E::TightStartLLevel);
        refuse(&mut w, t, tight(one_pct + 1, s, l_start, one_pct + 1, l_start + 2), p(), E::TightStartLLevel);
        // the extremes: the level l_start + (i64::MAX - 0.01 S) does not fit i64
        refuse(&mut w, t, tight(i64::MAX, s, l_start, i64::MAX, i64::MAX), p(), E::MathOverflow);
        // accepted: g_peak just above the slack, and g_peak at the slack with a larger p_open (level == l_start)
        let before = w.account();
        let (_, head) = accept(&mut w, t, tight(one_pct + 1, s, l_start, one_pct + 1, l_start + 1), p());
        assert_account_is(&w, &before, head, t, |a| {
            a.tight_on = 1;
            a.r = s;
            a.g_peak = one_pct + 1;
            a.l_level = l_start + 1;
        });
        restore(&mut w, &snapshot);
        let before = w.account();
        let (_, head) = accept(&mut w, t, tight(one_pct + 7, s, l_start, one_pct, l_start), p());
        assert_account_is(&w, &before, head, t, |a| {
            a.tight_on = 1;
            a.r = s;
            a.g_peak = one_pct;
            a.l_level = l_start;
        });
        restore(&mut w, &snapshot);
        // the engine's own function gives the same account state
        let mut st = engine::AccountState::new_phase(s);
        st.b = s;
        let ts = engine::check_tight_start(&st, one_pct + 1, s, l_start, one_pct + 1, l_start + 1).expect("engine accepts");
        assert_eq!((ts.r, ts.l_start, ts.g_peak, ts.l_level), (s, l_start, one_pct + 1, l_start + 1));
    }
}

// ============================================================================================================ TIME AND PROOF TIME ARITHMETIC

/// TIME AT THE i64 / u64 LIMITS (BLOCKED + LIMITED): an event time of `i64::MAX` is `TimeInFuture`, `i64::MIN` and `last_batch_time - 1`
/// are `TimeBackwards`, `last_batch_time = i64::MAX` blocks every later event (`TimeBackwards`); a proof timestamp of `u64::MAX` ns is
/// `ProofInFuture`, of 0 `ProofTooOld`; the proof time is the timestamp FLOORED to seconds (`t - 60` + 999,999,999 ns still counts as `t - 60`, one nanosecond
/// below `t - 60` is `t - 61`, too old; `t + 2` + 999,999,999 ns is still `t + 2`, allowed). A cluster clock of `i64::MAX - 1` makes `now + 2`
/// overflow: `MathOverflow`; at `i64::MAX - 2` it does not.
#[test]
fn time_and_proof_time_extremes() {
    let (mut w, t) = world();
    let ev = |time: i64| vec![meta(time, false)];
    refuse(&mut w, t, ev(i64::MAX), vec![], E::TimeInFuture);
    refuse(&mut w, t, ev(t + 4), vec![], E::TimeInFuture);
    refuse(&mut w, t, ev(i64::MIN), vec![], E::TimeBackwards);
    let last = w.account().last_batch_time;
    refuse(&mut w, t, ev(last - 1), vec![], E::TimeBackwards);
    // now + 2 is allowed (clock t + 1 inside `send`, so event time t + 3)
    accept(&mut w, t, ev(t + 3), vec![]);
    // a batch time of i64::MAX is stuck: every later event is backwards
    let (mut w2, t2) = world();
    w2.edit_account(|a| a.last_batch_time = i64::MAX);
    refuse(&mut w2, t2, ev(t2), vec![], E::TimeBackwards);
    refuse(&mut w2, t2, ev(i64::MAX), vec![], E::TimeInFuture);
    // the cluster clock at the end of i64: `now + 2` is checked arithmetic
    let (mut w3, t3) = world();
    let b = w3.batch(vec![meta(t3, false)], vec![]);
    let before = snap(&w3);
    let r = w3.post(i64::MAX - 1, &b, &[]);
    assert_err_at(&r, 1, E::MathOverflow);
    assert_eq!(snap(&w3), before);
    let mut b2 = w3.batch(vec![meta(i64::MAX, false)], vec![]);
    b2.first_seq = w3.account().seq + 1;
    let (_, head) = (w3.account(), w3.expected_head(&b2));
    let r = w3.post(i64::MAX - 2, &b2, &[]);
    assert!(r.is_ok(), "now + 2 == i64::MAX is representable: {:?}", r.err().map(|f| f.err));
    assert_eq!((w3.account().last_batch_time, w3.account().seq, w3.account().journal_hash), (i64::MAX, head.1, head.0));

    // proof timestamps: the market EUR has max_age_secs = 60
    let (mut w, t) = world();
    let snapshot = save(&w);
    let fill_ev = |price| vec![fill(MARKET_EUR, true, 1_000_000, price, t, 0), meta(t, false)];
    let ask_px = ref_ask(MID, H_EUR).unwrap();
    let with_ts = |ns: u64| {
        let mut p = mid_proof(t, MID);
        p.timestamp_ns = ns;
        vec![p]
    };
    let sec = |s: i64| u64::try_from(s).unwrap() * 1_000_000_000;
    refuse(&mut w, t, fill_ev(ask_px), with_ts(u64::MAX), E::ProofInFuture);
    refuse(&mut w, t, fill_ev(ask_px), with_ts(0), E::ProofTooOld);
    refuse(&mut w, t, fill_ev(ask_px), with_ts(sec(t - 61) + 999_999_999), E::ProofTooOld);
    refuse(&mut w, t, fill_ev(ask_px), with_ts(sec(t - 60) - 1), E::ProofTooOld);
    refuse(&mut w, t, fill_ev(ask_px), with_ts(sec(t + 3)), E::ProofInFuture);
    for ns in [sec(t - 60), sec(t - 60) + 999_999_999, sec(t), sec(t + 2), sec(t + 2) + 999_999_999] {
        let before = w.account();
        let (_, head) = accept(&mut w, t, fill_ev(ask_px), with_ts(ns));
        assert_account_is(&w, &before, head, t, |_| {});
        restore(&mut w, &snapshot);
    }
}

// ============================================================================================================ PAYOUT, OPEN FEE, RESET PRICE

/// `ceil(a x b / d)` in u128.
fn ceil_mul_div(a: u64, b: u64, d: u64) -> u64 {
    u64::try_from((u128::from(a) * u128::from(b)).div_ceil(u128::from(d))).expect("fits")
}

/// PAYOUT = EXACTLY 0.08 S FOR EVERY TIER AND EVERY PAYING PHASE (BLOCKED, through the real vault): for each of the ten launch tiers, phase 1
/// moves to phase 2 with no claim, phases 2 to 5 create a claim of `S / 100 * 8` (hand arithmetic, 0.08 S, exact because S is whole dollars) with the
/// request id the account carried, the pass needs `b >= 1.08 S` to the micro-USD (`1.08 S - 1` is `TargetNotReached`, nothing changes), and the 5th pass
/// graduates. The number is compared with `engine::payout_micro` too. No rounding is involved for a tier size (exact), so there is no direction to test
/// except that nothing above or below the exact amount is ever paid.
#[test]
fn payout_is_exactly_eight_percent_of_s_for_every_tier_and_phase() {
    const DELAY: i64 = 172_800;
    let sizes = [2_500u64, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
    let mut claims = 0u32;
    for (tier, dollars) in sizes.iter().enumerate() {
        let mut w = JEnv::with_tier(u8::try_from(tier).unwrap());
        let s = i64::try_from(dollars * 1_000_000).unwrap();
        assert_eq!(w.account().size_s, u64::try_from(s).unwrap());
        w.set_limits(172_800, 1_000_000_000_000, 86_400);
        let payer = w.new_payer();
        let mut t = w.t0 + 1_000;
        let ready = |w: &mut JEnv, phase: u8, b: i64| {
            w.edit_account(|a| {
                a.phase = phase;
                a.b = b;
                a.flat = 1;
                a.tight_on = 0;
                a.q_tally = 0;
            });
        };
        for phase in 1u8..=5 {
            // one micro-USD short of 1.08 S: refused with TargetNotReached, world unchanged
            ready(&mut w, phase, s / 100 * 108 - 1);
            w.fails(E::TargetNotReached, |w| w.request(t));
            ready(&mut w, phase, s / 100 * 108);
            let req_id = w.account().next_request_id;
            w.ok(|w| w.request(t));
            w.ok(|w| w.confirm(t + 1));
            if phase == 1 {
                assert_eq!(w.account().phase, 2, "phase 1 passes into phase 2 at the confirmation");
                assert!(w.claim(req_id).is_none(), "phase 1 pays nothing");
            } else {
                t += 1 + DELAY;
                w.ok(|w| w.finalize(t, &payer));
                let claim = w.claim(req_id).expect("a claim exists");
                let want = s / 100 * 8;
                assert_eq!(i64::try_from(claim.owed).unwrap(), want, "tier {tier} phase {phase}: 0.08 S");
                assert_eq!(engine::payout_micro(s), Ok(want));
                assert_eq!(claim.owed * 100, u64::try_from(s).unwrap() * 8, "0.08 S exactly, no rounding");
                claims += 1;
                assert_eq!(w.account().next_request_id, req_id + 1);
            }
            t += 100;
        }
        assert_eq!(w.account().status, setl8_lev_trading::constants::STATUS_GRADUATED, "the 5th pass graduates");
    }
    assert_eq!(claims, 40, "ten tiers x four paying phases");
}

/// The fresh VaultEnv for a custom rule set (tier costs), no markets.
fn vault_with(args: RuleSetArgs) -> VaultEnv {
    VaultEnv::with(Opts { ruleset: args, ..Opts::default() })
}
type RuleSetArgs = setl8_lev_trading::instructions::RuleSetArgs;

/// OPEN FEE = ceil(cost x fee_bps / 10_000), TIER COST PAID EXACTLY (BLOCKED, through the real vault): for fee_bps in {0, 1, 39, 40, 41, 999, 1000 (the
/// maximum)} and tier costs chosen on both sides of every rounding step (1, 24, 25, 26, 249, 250, 251, 9_999, 10_000, 10_001 micro-USD, then costs that
/// are not whole cents) the fee token account gains exactly the reference fee, the trader pays exactly `cost + fee`, the vault receives exactly `cost`,
/// and `fee * 10_000 >= cost * bps > (fee - 1) * 10_000` (never below the exact fee, never more than one unit above). A fee of 0 moves nothing.
/// `MAX_FEE_BPS = 1000` keeps `swap_fee_micro` from overflowing: its `MathOverflow` is unreachable through the program (documented, not testable).
#[test]
fn open_account_fee_rounds_up_for_boundary_costs_and_rates() {
    let cost_sets: [[u64; 10]; 2] = [
        [1, 24, 25, 26, 249, 250, 251, 9_999, 10_000, 10_001],
        [3, 12_340_001, 12_340_002, 99_999_999, 100_000_001, 859_000_001, 1_599_999_999, 10_000_000_001, 123_456_789_012, 1_000_000_000_000_000],
    ];
    let mut checked = 0u32;
    for bps in [0u16, 1, 39, 40, 41, 999, 1_000] {
        for costs in cost_sets {
            let mut a = launch_ruleset_args();
            a.fee_bps = bps;
            a.tier_cost = costs;
            let mut v = vault_with(a.clone());
            for (tier, &cost) in costs.iter().enumerate() {
                let trader = v.new_trader_with(u64::MAX / 4);
                let tk = trader.pubkey();
                let ta = v.trader_token(&tk);
                let fee = ceil_mul_div(cost, u64::from(bps), 10_000);
                assert_eq!(fee, v.expected_fee(tier), "harness reference vs this file's reference");
                assert_eq!(engine::swap_fee_micro(cost, bps), Ok(fee), "engine vs reference");
                assert!(u128::from(fee) * 10_000 >= u128::from(cost) * u128::from(bps), "fee below the exact fee");
                assert!(fee == 0 || u128::from(fee - 1) * 10_000 < u128::from(cost) * u128::from(bps), "fee more than one unit above the exact fee");
                let (t0, fee0, pool0, sl80) = (v.token_balance(&ta), v.token_balance(&v.fee_usdc), v.token_balance(&v.usdc_pool), v.token_balance(&v.vault_sl8_usdc));
                assert_ok(v.open(&trader, u8::try_from(tier).unwrap()));
                assert_eq!(v.token_balance(&v.fee_usdc) - fee0, fee, "fee account, cost {cost} bps {bps}");
                assert_eq!(t0 - v.token_balance(&ta), cost + fee, "trader pays cost + fee, cost {cost} bps {bps}");
                assert_eq!((v.token_balance(&v.usdc_pool) - pool0) + (v.token_balance(&v.vault_sl8_usdc) - sl80), cost, "the vault receives exactly the tier cost");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 7 * 2 * 10);
}

/// A world for resets with a custom rule set: the real vault, the EUR market, one $-tier account.
fn reset_world(args: RuleSetArgs, tier: u8) -> JEnv {
    let mut v = vault_with(args);
    let mut s = [setl8_lev_trading::state::Session::default(); 6];
    s[0] = setl8_lev_trading::state::Session { start_min: 0, end_min: 10_080 };
    v.e.create_market_ok(&setl8_lev_trading::instructions::CreateMarketArgs {
        market_id: MARKET_EUR,
        class: 0,
        params: setl8_lev_trading::instructions::MarketParams {
            status: 1,
            feed_id: feed(symbol_of(MARKET_EUR)),
            max_age_secs: 60,
            qty_step: 1_000,
            qty_min: 1_000_000,
            max_notional_usd: 1_000_000_000_000_000,
            session_count: 1,
            sessions: s,
            quote_conv_market: setl8_lev_trading::constants::NO_QUOTE_CONV,
        },
    });
    let trader = v.new_trader();
    assert_ok(v.open(&trader, tier));
    let acct = acct_pda(&trader.pubkey(), 0);
    let t0 = v.account(&acct).created_at;
    JEnv { v, trader, acct, t0 }
}

/// RESET PRICE = ceil(CURRENT tier cost x ladder[phase] / 10_000), PLUS THE 0.4% FEE ROUNDED UP, FOR ALL TEN TIERS x FIVE PHASES (BLOCKED, real
/// `report_fail` then real `reset_account` against the real vault): the trader's token balance falls by exactly `price + ceil(price x 40 / 10_000)`,
/// the vault receives `price`, the fee account gains the fee. Run once with the launch costs (exact multiples) and once with every cost + 1 micro-USD
/// (so that every ladder step has a fraction and must round UP: 1.15 x c + 0.15, ...). The reference is `ceil_mul_div` in this file, not the shared
/// helper; the shared helper and the program's `reset_amount_micro` are compared as well.
#[test]
fn reset_price_and_fee_round_up_for_every_tier_and_phase() {
    let ladder = [10_000u64, 11_500, 12_000, 12_750, 13_000];
    let mut resets = 0u32;
    for plus in [0u64, 1] {
        for tier in 0u8..10 {
            let mut a = launch_ruleset_args();
            for c in a.tier_cost.iter_mut() {
                *c += plus;
            }
            let cost = a.tier_cost[usize::from(tier)];
            let mut w = reset_world(a, tier);
            let mut t = w.t0 + 1_000;
            for (idx, bps) in ladder.iter().enumerate() {
                let phase = u8::try_from(idx + 1).unwrap();
                w.fail_in(phase, t);
                let price = ceil_mul_div(cost, *bps, 10_000);
                assert_eq!(setl8_shared_interfaces::reset_price(cost, u16::try_from(*bps).unwrap()), Some(price), "shared helper vs reference");
                let fee = ceil_mul_div(price, 40, 10_000);
                let tk = w.trader.pubkey();
                let ta = w.v.trader_token(&tk);
                let (t0, fee0, pool0, sl80) = (w.v.token_balance(&ta), w.v.token_balance(&w.v.fee_usdc), w.v.token_balance(&w.v.usdc_pool), w.v.token_balance(&w.v.vault_sl8_usdc));
                w.reset_ok(t + 10);
                assert_eq!(t0 - w.v.token_balance(&ta), price + fee, "tier {tier} phase {phase} plus {plus}: price + fee");
                assert_eq!(w.v.token_balance(&w.v.fee_usdc) - fee0, fee, "fee account");
                assert_eq!((w.v.token_balance(&w.v.usdc_pool) - pool0) + (w.v.token_balance(&w.v.vault_sl8_usdc) - sl80), price, "vault receives the price");
                assert!(u128::from(price) * 10_000 >= u128::from(cost) * u128::from(*bps), "reset price below the exact price");
                assert!(u128::from(price - 1) * 10_000 < u128::from(cost) * u128::from(*bps), "reset price more than one unit above the exact price");
                resets += 1;
                t += 100;
            }
        }
    }
    assert_eq!(resets, 100);
}

fn edit_config(w: &mut JEnv, f: impl FnOnce(&mut SectorConfig)) {
    let k = config_pda();
    let mut acc = w.v.e.svm.get_account(&k).expect("config");
    let mut c = SectorConfig::try_deserialize(&mut acc.data.as_slice()).unwrap();
    f(&mut c);
    let mut data = Vec::new();
    c.try_serialize(&mut data).unwrap();
    acc.data = data;
    w.v.e.svm.set_account(k, acc).unwrap();
}
fn edit_tally(w: &mut JEnv, count: u64, total: u64) {
    let mut acc = w.v.e.svm.get_account(&tally_pda()).expect("tally");
    setl8_shared_interfaces::PayoutTally { requested_count: count, requested_total: total }.write_into(&mut acc.data).unwrap();
    w.v.e.svm.set_account(tally_pda(), acc).unwrap();
}

/// FINALIZE_PASS ARITHMETIC AT THE u64 / i64 EDGES (BLOCKED + LIMITED, real vault): the daily window and the payout tally use checked adds.
/// `window_total + 0.08 S` beyond `u64::MAX` is `MathOverflow` (never the cap error, never a wrap) and a total that lands EXACTLY on `u64::MAX` is accepted;
/// `window_start + window_secs` beyond `i64::MAX` and `pass_proposed_at + delay` beyond `i64::MAX` are `MathOverflow`; a tally whose total or count would
/// pass `u64::MAX` is `MathOverflow` AFTER the vault CPI created its claim, and the whole transaction (claim, vault record, config, account) rolls back:
/// the pass digest is identical. A tally landing exactly on `u64::MAX` is accepted.
#[test]
fn finalize_pass_window_tally_and_delay_arithmetic_at_the_edges() {
    const DELAY: i64 = 172_800;
    let amount: u64 = 8_000_000_000; // 0.08 S of the $100k tier, by hand
    let fresh = || {
        let mut w = JEnv::new();
        w.set_limits(172_800, 1_000_000_000_000_000, 86_400);
        let payer = w.new_payer();
        let t = w.t0 + 1_000;
        w.ready(2);
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        (w, payer, t + 1 + DELAY)
    };
    // window_total + amount: one above the representable range, then exactly on it (cap raised to u64::MAX so only the add can fail)
    let (mut w, payer, tf) = fresh();
    edit_config(&mut w, |c| {
        c.max_payout_per_window = u64::MAX;
        c.window_start = tf - 10;
        c.window_total = u64::MAX - amount + 1;
    });
    w.fails(E::MathOverflow, |w| w.finalize(tf, &payer));
    edit_config(&mut w, |c| c.window_total = u64::MAX - amount);
    w.ok(|w| w.finalize(tf, &payer));
    assert_eq!(w.v.e.config().window_total, u64::MAX, "the window total lands exactly on u64::MAX");
    assert_eq!(w.account().phase, 3);
    // with the ordinary cap the same total is the cap error (the add did not overflow)
    let (mut w, payer, tf) = fresh();
    edit_config(&mut w, |c| {
        c.window_start = tf - 10;
        c.window_total = 1_000_000_000_000_000 - amount + 1;
    });
    w.fails(E::DailyPayoutCapReached, |w| w.finalize(tf, &payer));
    // the window end overflows i64
    let (mut w, payer, tf) = fresh();
    edit_config(&mut w, |c| c.window_start = i64::MAX - 10);
    w.fails(E::MathOverflow, |w| w.finalize(tf, &payer));
    // the delay deadline overflows i64
    let (mut w, payer, tf) = fresh();
    w.edit_account(|a| a.pass_proposed_at = i64::MAX);
    w.fails(E::MathOverflow, |w| w.finalize(tf, &payer));
    // the tally: total and count at the edge
    let (mut w, payer, tf) = fresh();
    edit_tally(&mut w, 0, u64::MAX - amount + 1);
    w.fails(E::MathOverflow, |w| w.finalize(tf, &payer));
    edit_tally(&mut w, u64::MAX, 0);
    w.fails(E::MathOverflow, |w| w.finalize(tf, &payer));
    edit_tally(&mut w, 7, u64::MAX - amount);
    w.ok(|w| w.finalize(tf, &payer));
    let tl = w.tally();
    assert_eq!((tl.requested_count, tl.requested_total), (8, u64::MAX));
    assert_eq!(w.claim(1).expect("claim").owed, amount);
}

/// FILL FIELDS THE PROGRAM DOES NOT RECOMPUTE (LIMITED, OFF-CHAIN control): `margin`, `avg_entry_after`, `sl`, `tp` and `intent_id` of a `Fill` are hashed
/// into the chain but never read by the program (DESIGN 4.4 has no invariant for them; the VERIFIER recomputes margin and the average entry from the engine).
/// A fill with `margin = u64::MAX`, `avg_entry_after = 0`, `sl = tp = u64::MAX` and a margin that no balance could pay is ACCEPTED; the account changes only in
/// seq, hash and last_batch_time. Pinned so a later on-chain margin check shows up as a test change.
#[test]
fn fill_margin_average_and_protective_fields_are_not_checked_on_chain() {
    let (mut w, t) = world();
    let price = ref_ask(MID, H_EUR).unwrap();
    for (margin, avg, sl, tp, intent) in [(u64::MAX, 0u64, u64::MAX, u64::MAX, u32::MAX), (0, u64::MAX, 0, 0, 0), (1, 1, 1, 1, 1)] {
        let snapshot = save(&w);
        let ev = JournalEvent::Fill {
            market: MARKET_EUR,
            side: Side::Long,
            order_kind: journal_types::OrderKind::Market,
            qty: 1_000_000,
            price_e12: price,
            time: t,
            avg_entry_after: avg,
            margin,
            sl,
            tp,
            intent_id: intent,
            proof: 0,
        };
        let before = w.account();
        let (_, head) = accept(&mut w, t, vec![ev, meta(t, false)], vec![p_eur(t)]);
        assert_account_is(&w, &before, head, t, |_| {});
        restore(&mut w, &snapshot);
    }
}

/// SEQUENCE ARITHMETIC AT THE END OF u64 WITH A FULL BATCH (BLOCKED + LIMITED): from `seq = u64::MAX - 8` an 8-event batch (7 fees and the Meta) ends exactly on
/// `u64::MAX` and is accepted; from `seq = u64::MAX - 7` the same batch would need `2^64` and is `MathOverflow`, byte-identical afterwards. (The single-event
/// and `seq + 1` edges are in `journal_attacks.rs`.)
#[test]
fn sequence_end_of_u64_with_a_full_batch() {
    let (mut w, t) = world();
    let evs = |n: usize| {
        let mut v: Vec<JournalEvent> = (0..n - 1).map(|_| fee_ev(FeeKind::Commission, 1, t)).collect();
        v.push(meta(t, false));
        v
    };
    w.edit_account(|a| a.seq = u64::MAX - 7);
    refuse(&mut w, t, evs(8), vec![], E::MathOverflow);
    w.edit_account(|a| a.seq = u64::MAX - 8);
    let before = w.account();
    let (_, head) = accept(&mut w, t, evs(8), vec![]);
    assert_eq!(head.1, u64::MAX);
    assert_account_is(&w, &before, head, t, |a| a.b = S100K - 7);
    assert_eq!(w.account().seq, u64::MAX);
}

/// RESET PRICE OVERFLOW THROUGH THE PROGRAM (BLOCKED, real vault): a tier whose cost is 1.5e19 micro-USD opens (the trader holds `u64::MAX` tokens); after a
/// fail in phase 4 the ladder price is ceil(1.5e19 x 1.275) = 1.9125e19 > `u64::MAX`: `reset_account` is `ResetPriceOverflow`, nothing changes; in phase 2
/// (115%, 1.725e19) the price fits, so the overflow is not an off-by-one of the ladder.
#[test]
fn reset_price_overflow_is_a_named_error_through_the_program() {
    let mut a = launch_ruleset_args();
    a.tier_cost[9] = 15_000_000_000_000_000_000;
    let mut v = vault_with(a);
    // the market for report_fail
    let mut w = {
        let mut s = [setl8_lev_trading::state::Session::default(); 6];
        s[0] = setl8_lev_trading::state::Session { start_min: 0, end_min: 10_080 };
        v.e.create_market_ok(&setl8_lev_trading::instructions::CreateMarketArgs {
            market_id: MARKET_EUR,
            class: 0,
            params: setl8_lev_trading::instructions::MarketParams {
                status: 1,
                feed_id: feed(symbol_of(MARKET_EUR)),
                max_age_secs: 60,
                qty_step: 1_000,
                qty_min: 1_000_000,
                max_notional_usd: 1_000_000_000_000_000,
                session_count: 1,
                sessions: s,
                quote_conv_market: setl8_lev_trading::constants::NO_QUOTE_CONV,
            },
        });
        let trader = v.new_trader_with(u64::MAX);
        assert_ok(v.open(&trader, 9));
        let acct = acct_pda(&trader.pubkey(), 0);
        let t0 = v.account(&acct).created_at;
        JEnv { v, trader, acct, t0 }
    };
    let t = w.t0 + 1_000;
    w.fail_in(4, t);
    // the failed record's phase decides the ladder step; the ladder prices of the 1.5e19 tier are 1.5e19, 1.725e19, 1.8e19 (fit u64) and 1.9125e19, 1.95e19 (do not)
    for (phase, fits) in [(1u8, true), (2, true), (3, true), (4, false), (5, false)] {
        w.edit_account(|a| a.phase = phase);
        let price = (15_000_000_000_000_000_000u128 * u128::from([10_000u64, 11_500, 12_000, 12_750, 13_000][usize::from(phase) - 1])).div_ceil(10_000);
        assert_eq!(price <= u128::from(u64::MAX), fits, "hand table: phase {phase}");
        let before = w.pass_digest();
        let r = w.reset(t + 10);
        if fits {
            // the price is computed; the trader cannot pay 1.5e19 again (about 3.4e18 left), so the TOKEN program refuses: any error but the overflow
            match &r {
                Err(f) => match &f.err {
                    solana_transaction_error::TransactionError::InstructionError(0, anchor_lang::solana_program::instruction::error::InstructionError::Custom(code)) => {
                        assert_ne!(*code, u32::from(E::ResetPriceOverflow), "phase {phase} must not overflow");
                    }
                    other => panic!("unexpected failure shape {other:?}"),
                },
                Ok(_) => panic!("the trader cannot afford this reset"),
            }
        } else {
            assert_err_at(&r, 0, E::ResetPriceOverflow);
        }
        assert_eq!(w.pass_digest(), before, "a refused reset changed the world (phase {phase})");
    }
}

// ============================================================================================================ TIME ARITHMETIC OF THE LIFECYCLE INSTRUCTIONS

/// TIME SUMS AND DIFFERENCES OF THE PASS / LIFECYCLE INSTRUCTIONS (BLOCKED + LIMITED): `confirm_pass` computes `now + delay` (checked: a clock within
/// `delay` of `i64::MAX` is `MathOverflow`; the last representable clock is accepted), `cancel_pass` computes `now - requested_at` (an `i64::MIN` request
/// time overflows: `MathOverflow`; a request time in the future is `CancelTooEarly`), `close_account` computes `terminal_at + 7 d` (an `i64::MAX` terminal time:
/// `MathOverflow`), `touch_activity` computes `now - last_activity_pushed` (`i64::MIN`: `MathOverflow`). `veto_pass` SATURATES the veto counter at 255
/// (LIMITED: the counter never wraps back to 0, so a history of vetoes cannot be erased by vetoing 256 times). Refusals leave the pass digest identical.
#[test]
fn lifecycle_time_arithmetic_at_the_edges() {
    const DELAY: i64 = 172_800;
    // confirm_pass: now + delay
    let mut w = JEnv::new();
    w.set_limits(172_800, 1_000_000_000_000_000, 86_400);
    let t = w.t0 + 1_000;
    w.ready(2);
    w.ok(|w| w.request(t));
    w.fails(E::MathOverflow, |w| w.confirm(i64::MAX - 10));
    w.fails(E::MathOverflow, |w| w.confirm(i64::MAX - DELAY + 1));
    w.ok(|w| w.confirm(i64::MAX - DELAY));
    assert_eq!((w.account().pass_pending, w.account().pass_proposed_at), (2, i64::MAX - DELAY));
    // cancel_pass: now - requested_at
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.ready(2);
    w.ok(|w| w.request(t));
    w.edit_account(|a| a.pass_requested_at = i64::MIN);
    w.fails(E::MathOverflow, |w| w.cancel(t + 10_000));
    w.edit_account(|a| a.pass_requested_at = i64::MAX);
    w.fails(E::CancelTooEarly, |w| w.cancel(t + 10_000));
    w.edit_account(|a| a.pass_requested_at = t);
    w.fails(E::CancelTooEarly, |w| w.cancel(t + 3_599));
    w.ok(|w| w.cancel(t + 3_600));
    // close_account: terminal_at + 7 days
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.make_terminal(setl8_lev_trading::constants::STATUS_FAILED, i64::MAX);
    w.fails(E::MathOverflow, |w| w.close(t));
    w.make_terminal(setl8_lev_trading::constants::STATUS_FAILED, i64::MAX - 604_800 + 1);
    w.fails(E::MathOverflow, |w| w.close(t));
    w.make_terminal(setl8_lev_trading::constants::STATUS_FAILED, t - 604_800);
    w.fails(E::CloseTooEarly, |w| w.close(t - 1));
    // touch_activity: now - last_activity_pushed
    let mut w = JEnv::new();
    let t = w.t0 + 100_000;
    w.edit_account(|a| a.last_activity_pushed = i64::MIN);
    w.fails(E::MathOverflow, |w| w.touch(t));
    // veto counter saturates
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.ready(2);
    w.ok(|w| w.request(t));
    w.edit_account(|a| a.vetoes = 255);
    w.ok(|w| w.veto(t + 1));
    assert_eq!((w.account().vetoes, w.account().pass_pending), (255, 0), "the veto counter saturates at 255, it does not wrap to 0");
    w.edit_account(|a| a.vetoes = 254);
    w.ready(2);
    w.ok(|w| w.request(t + 2));
    w.ok(|w| w.veto(t + 3));
    assert_eq!(w.account().vetoes, 255);
}

/// FLOOR RAISE AT THE i64 LIMIT (BLOCKED + exact): in tight mode (R 100k, l_start 96k, peak 1.2k) the new level is `l_start + (g_peak - 0.01 S)`; a `g_peak`
/// that makes the level exactly `i64::MAX` is accepted, one more is `MathOverflow`; `g_peak = i64::MAX` is `MathOverflow`; a `g_peak` not above the stored one
/// is `FloorNotRaised`; a level off by one is `FloorLLevel`.
#[test]
fn floor_raise_at_the_i64_limit() {
    let (mut w, t) = world();
    set_tight(&mut w, usd(100_500));
    let slack = 1_000_000_000i64; // 0.01 S by hand
    let l_start = 96_000_000_000i64; // L 96.2k - (1.2k - 1k)
    let raise = |g: i64, l: i64| vec![JournalEvent::FloorRaised { time: t, g_peak: g, l_level: l, proof: 0 }, meta(t, false)];
    let p = || vec![mid_proof(t, MID)];
    let snapshot = save(&w);
    // peak equal or lower: FloorNotRaised
    refuse(&mut w, t, raise(usd(1_200), usd(96_200)), p(), E::FloorNotRaised);
    refuse(&mut w, t, raise(0, l_start), p(), E::FloorNotRaised);
    refuse(&mut w, t, raise(i64::MIN, l_start), p(), E::FloorNotRaised);
    // level off by one in either direction
    refuse(&mut w, t, raise(usd(1_300), usd(96_300) - 1), p(), E::FloorLLevel);
    refuse(&mut w, t, raise(usd(1_300), usd(96_300) + 1), p(), E::FloorLLevel);
    // the level lands exactly on i64::MAX
    let g_ok = i64::MAX - l_start + slack;
    assert_eq!(l_start + (g_ok - slack), i64::MAX, "hand check");
    let before = w.account();
    let (_, head) = accept(&mut w, t, raise(g_ok, i64::MAX), p());
    assert_account_is(&w, &before, head, t, |a| {
        a.g_peak = g_ok;
        a.l_level = i64::MAX;
    });
    restore(&mut w, &snapshot);
    refuse(&mut w, t, raise(g_ok + 1, i64::MAX), p(), E::MathOverflow);
    refuse(&mut w, t, raise(i64::MAX, i64::MAX), p(), E::MathOverflow);
}

/// `report_fail` WITH THE BALANCE AT THE i64 LIMITS (BLOCKED + exact, real vault): `b = i64::MIN` is a static-floor fail and is accepted with `b_after = i64::MIN`;
/// `b = i64::MAX` is not at the floor (`FailStaticFloorNotReached`) and with `b_after` one off it is `FailBalanceMismatch`; a quick tally of `i64::MAX` is a
/// `PhaseQuick` fail (accepted) but not a `StaticFloor` one (`QuickLimitNeedsPhaseQuick`).
#[test]
fn report_fail_with_balance_and_tally_at_the_i64_limits() {
    let (mut w, t) = world();
    let rf = |w: &mut JEnv, reason: journal_types::FailReason, wire: u8, b_after: i64| {
        let batch = w.fail_batch(t, vec![], reason, b_after, true);
        let before = w.pass_digest();
        let r = w.report_fail(t + 1, &batch, wire, &[MARKET_EUR]);
        (r, before)
    };
    use journal_types::FailReason as F;
    w.edit_account(|a| a.b = i64::MAX);
    let (r, before) = rf(&mut w, F::StaticFloor, 0, i64::MAX);
    assert_err_at(&r, 0, E::FailStaticFloorNotReached);
    assert_eq!(w.pass_digest(), before);
    let (r, before) = rf(&mut w, F::StaticFloor, 0, i64::MAX - 1);
    assert_err_at(&r, 0, E::FailBalanceMismatch);
    assert_eq!(w.pass_digest(), before);
    // tally at i64::MAX with a healthy balance: PhaseQuick is accepted... but first the wrong reason is refused
    w.edit_account(|a| {
        a.b = S100K;
        a.q_tally = i64::MAX;
    });
    let (r, before) = rf(&mut w, F::StaticFloor, 0, S100K);
    assert_err_at(&r, 0, E::QuickLimitNeedsPhaseQuick);
    assert_eq!(w.pass_digest(), before);
    let (r, _) = rf(&mut w, F::PhaseQuick, 1, S100K);
    assert_ok(r);
    assert_eq!(w.account().status, setl8_lev_trading::constants::STATUS_FAILED);
    // b = i64::MIN: a static-floor fail
    let (mut w, t) = world();
    w.edit_account(|a| a.b = i64::MIN);
    let batch = w.fail_batch(t, vec![], F::StaticFloor, i64::MIN, true);
    assert_ok(w.report_fail(t + 1, &batch, 0, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.b, a.seq), (setl8_lev_trading::constants::STATUS_FAILED, i64::MIN, 2));
}

// ============================================================================================================ RULE SET TIER SIZES THE ENGINE CANNOT USE

/// The two kinds of tier size the engine cannot handle: not whole dollars (`S x bps / 10_000` is inexact: `InvalidInput`), and a size whose 1.04 S
/// does not fit `i64` (`Overflow`) or that does not fit `i64` at all.
const BAD_TIER_SIZES: [(u64, usize, &str); 3] = [
    (1_000_001, 0, "not whole dollars (1 USD + 1 micro)"),
    (8_870_000_000_000_000_000, 9, "whole dollars, 1.04 S does not fit i64"),
    (1 << 63, 9, "does not fit i64"),
];

/// The launch rule set with tier `tier` replaced by `size` (tier 0 for a small size, tier 9 for a huge one, so the table stays increasing).
fn bad_size_args(size: u64, tier: usize) -> RuleSetArgs {
    let mut a = launch_ruleset_args();
    a.tier_size[tier] = size;
    a
}

// FINDING ARI-4: `create_ruleset` (both admins) validates that tier sizes are > 0 and strictly increasing, but NOT that a size is something the engine can
// use. The engine's contract (`engine::account::frac_exact`: "S is a whole number of dollars so S x bps / 10_000 is exact, an inexact size is
// InvalidInput") is not enforced where sizes enter the system. A rule set with a tier of 1_000_001 micro-USD (or a size whose 1.04 S overflows i64, or above
// i64::MAX) is accepted; the vault is registered with it; a trader can BUY such an account (price paid to the vault); and from then on every batch with a fee or
// a flat Meta, every `report_fail`, every pass request is `MathOverflow`: the account is dead and can never be failed, passed or traded to its floor (see the
// LIMITED test below for the exact trace). Rule sets are immutable, so the bad version cannot be repaired, only superseded. Severity low (needs BOTH admins to
// publish a wrong rule set; launch values are fine; the deploy checklist reads the rule set back), but the loss is the trader's purchase price. A fix is a
// plain implementation addition in `validate_ruleset` (size % 1_000_000 == 0 and 1.04 S <= i64::MAX), no DESIGN change.
#[test]
#[ignore = "FINDING ARI-4: create_ruleset accepts tier sizes the engine cannot use (not whole dollars, 1.04 S beyond i64, above i64::MAX)"]
fn finding_ari_4_create_ruleset_must_refuse_tier_sizes_the_engine_cannot_use() {
    for (size, tier, why) in BAD_TIER_SIZES {
        let mut e = Env::initialised();
        let ix = create_ruleset_ix(e.payer.pubkey(), Admins::both(&e), &bad_size_args(size, tier));
        let a = Admins::both(&e);
        let r = e.send_admins(ix, a);
        assert!(r.is_err(), "create_ruleset accepted a tier size that is {why} ({size})");
        assert_err(&r, E::InvalidTier);
    }
}

/// LIMITED, the trace behind ARI-4 pinned exactly: with a tier of 1_000_001 micro-USD the rule set is accepted, the account is sold (the trader pays and the
/// vault records S = 1_000_001), a batch that does not touch a threshold is accepted, and every operation that needs a threshold is `MathOverflow`: a fee,
/// a flat Meta, `report_fail` (so the account can never be failed), and the pass request (so it can never pass). A later fix of ARI-4 changes the FIRST step
/// (the rule set is refused) and this test with it.
#[test]
fn rule_set_with_an_unusable_tier_size_sells_an_account_that_can_do_nothing() {
    let mut w = reset_world(bad_size_args(1_000_001, 0), 0);
    assert_eq!(w.account().size_s, 1_000_001, "the account was sold with the unusable size");
    let t = w.t0 + 1_000;
    // the harness world has no spread rule set edit; EUR half-spread is 0 in the launch rule set, so only non-fill events are used
    accept(&mut w, t, vec![meta(t, false)], vec![]);
    refuse(&mut w, t + 1, vec![fee_ev(FeeKind::Commission, 1, t + 1), meta(t + 1, false)], vec![], E::MathOverflow);
    refuse(&mut w, t + 1, vec![meta(t + 1, true)], vec![], E::MathOverflow);
    // report_fail: the floor cannot be computed
    let b = w.account().b;
    let batch = w.fail_batch(t + 2, vec![], journal_types::FailReason::StaticFloor, b, true);
    let before = w.pass_digest();
    assert_err_at(&w.report_fail(t + 3, &batch, 0, &[MARKET_EUR]), 0, E::MathOverflow);
    assert_eq!(w.pass_digest(), before);
    // the pass cannot be requested either (the target 1.08 S is inexact, the failed-state check needs the floor)
    w.edit_account(|a| a.flat = 1);
    let before = w.pass_digest();
    let r = w.request(t + 4);
    assert_err_at(&r, 0, E::MathOverflow);
    assert_eq!(w.pass_digest(), before);
}

/// STORED FIELDS AT THE LIMITS THAT THE PROGRAM ITSELF NEVER WRITES (BLOCKED, account edited by the test): `trades_closed = u32::MAX` makes the next `Close` a
/// `MathOverflow` (checked add; one below the maximum is accepted and lands on it); a tight-mode account whose `g_peak` or `l_level` would overflow the
/// derived `l_start = l_level - max(0, g_peak - 0.01 S)` is `MathOverflow` for EVERY batch (the engine view of the account cannot be built; no panic, no wrap).
#[test]
fn stored_fields_at_the_limits_never_panic() {
    let (mut w, t) = world();
    set_spread(&mut w, u16::try_from(H_CLOSE).unwrap());
    let c = C { long: true, mid: MID, qty: 1_000_000_000_000, entry_below: 5, age: 1_000 };
    let one_close = || with_meta(c.good(t), t);
    w.edit_account(|a| a.trades_closed = u32::MAX);
    refuse(&mut w, t, one_close(), vec![mid_proof(t, MID)], E::MathOverflow);
    w.edit_account(|a| a.trades_closed = u32::MAX - 1);
    let before = w.account();
    let (_, head) = accept(&mut w, t, one_close(), vec![mid_proof(t, MID)]);
    assert_account_is(&w, &before, head, t, |a| {
        a.b = S100K + 5;
        a.trades_closed = u32::MAX;
    });
    // derived l_start overflows
    let (mut w, t) = world();
    w.edit_account(|a| {
        a.tight_on = 1;
        a.g_peak = i64::MIN;
    });
    refuse(&mut w, t, vec![meta(t, false)], vec![], E::MathOverflow);
    w.edit_account(|a| {
        a.tight_on = 1;
        a.g_peak = i64::MAX;
        a.l_level = i64::MIN;
    });
    refuse(&mut w, t, vec![meta(t, false)], vec![], E::MathOverflow);
    refuse(&mut w, t, vec![fee_ev(FeeKind::Holding, 1, t), meta(t, false)], vec![], E::MathOverflow);
}
