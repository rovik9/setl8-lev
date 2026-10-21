//! M11: MEASURED budgets of every instruction (P19 part C). The table generator runs every instruction of the program in the
//! real LiteSVM world (real vault, real SPL token program) in the shapes that cost the most, records for each the compute units
//! consumed, the serialized size of the signed transaction, the number of distinct accounts, the number of signatures and the
//! deepest invocation level, and ASSERTS a ceiling per instruction: the measured worst case plus 15 %, never above 200,000 compute
//! units or 1,232 bytes. A later change that blows a budget fails here.
//!
//! `cargo test --test budget -- --nocapture` prints the table. `BUDGET_CALIBRATE=1` prints the ceilings to paste into `CEILINGS`
//! instead of asserting them (use it when a budget changes on purpose; the docs table in handover/module-11.md is the record).
//!
//! Shapes: admin instructions are signed by the fee payer and both admins (3 signatures, the largest form); `post_journal`,
//! `report_fail` and the evaluator instructions are signed by the evaluator alone, as fee payer, with NO compute-budget
//! instruction (the SPLIT rule of DESIGN 4.9); trader instructions by the trader (also the payer). The campaign rows come from
//! the differential campaign (`DIFF_PATHS`, default 150 here) and are merged into the same table.
mod common;

use std::collections::BTreeMap;

use anchor_lang::{InstructionData, ToAccountMetas};
use common::evaluator::*;
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::*;
use journal_types::{feed_id, JournalBatch, JournalEvent, NativeSha256, PriceProof};
use litesvm::types::TransactionMetadata;
use setl8_lev_trading::{accounts, instruction, instructions::UpdateMarketArgs};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

const CU_LIMIT: u64 = 200_000;
const TX_LIMIT: usize = 1_232;
const DELAY: i64 = 86_400;
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;

/// Ceilings asserted per instruction: (name, compute units, transaction bytes). Measured worst case + 15 %, rounded up to 100
/// units / 10 bytes, never above the limits. Empty until calibrated.
const CEILINGS: &[(&str, u64, usize)] = &[
    ("ban_wallet", 31_100, 660),
    ("cancel_pass", 22_100, 400),
    ("close_account", 3_900, 360),
    ("confirm_pass", 27_600, 440),
    ("create_market", 36_500, 690),
    ("create_ruleset", 53_400, 1_180),
    ("fail_for_violation", 50_700, 770),
    ("finalize_pass", 97_300, 780),
    ("hold_account", 28_400, 510),
    ("init_config", 18_500, 680),
    ("open_account", 156_000, 970),
    ("post_journal", 144_100, 1_232),
    ("reject_pass", 25_000, 440),
    ("release_account", 60_000, 620),
    ("release_hold", 28_300, 580),
    ("report_fail", 174_000, 1_232),
    ("request_advance", 52_900, 620),
    ("reset_account", 182_000, 1_050),
    ("set_evaluator", 22_300, 550),
    ("set_limits", 21_700, 530),
    ("set_paused", 21_600, 510),
    ("touch_activity", 60_000, 620),
    ("unban_wallet", 23_700, 580),
    ("update_market", 35_400, 650),
    ("veto_pass", 26_100, 550),
];

#[derive(Clone, Debug, Default)]
struct Row {
    cu: u64,
    cu_case: String,
    bytes: usize,
    bytes_case: String,
    accounts: usize,
    signatures: usize,
    depth: u8,
    inner: usize,
    n: u64,
}

#[derive(Default)]
struct Table(BTreeMap<String, Row>);

impl Table {
    fn rec(&mut self, name: &str, case: &str, meta: &TransactionMetadata, shape: TxShape) {
        let r = self.0.entry(name.to_string()).or_default();
        r.n += 1;
        if meta.compute_units_consumed > r.cu {
            r.cu = meta.compute_units_consumed;
            r.cu_case = case.to_string();
        }
        if shape.bytes > r.bytes {
            r.bytes = shape.bytes;
            r.bytes_case = case.to_string();
        }
        r.accounts = r.accounts.max(shape.accounts);
        r.signatures = r.signatures.max(shape.signatures);
        let depth = meta.inner_instructions.iter().flatten().map(|i| i.stack_height).max().unwrap_or(1);
        r.depth = r.depth.max(depth);
        r.inner = r.inner.max(meta.inner_instructions.iter().map(Vec::len).sum());
    }

    /// Records the last transaction `w` sent.
    fn jrec(&mut self, name: &str, case: &str, w: &JEnv, meta: &TransactionMetadata) {
        self.rec(name, case, meta, w.v.e.last_tx);
    }
}

fn ok(r: litesvm::types::TransactionResult) -> TransactionMetadata {
    match r {
        Ok(m) => m,
        Err(f) => panic!("expected success, got {:?}\nlogs:\n{}", f.err, f.meta.logs.join("\n")),
    }
}

// ================================================================ the evaluator's transaction: one signer, no budget instruction

/// `post_journal` signed by the evaluator alone (fee payer), nothing else in the transaction. Returns the metadata and the shape.
fn post_prod(w: &mut JEnv, clock: i64, batch: &JournalBatch, markets: &[u16]) -> (TransactionMetadata, TxShape) {
    w.v.e.set_clock(clock);
    let mut metas = accounts::PostJournal {
        evaluator: w.v.evaluator.pubkey(),
        trading_account: w.acct,
        config: config_pda(),
        ruleset: ruleset_pda(w.account().ruleset_version),
        event_authority: event_authority(),
        program: PROGRAM_ID,
    }
    .to_account_metas(None);
    metas.extend(JEnv::market_metas(markets));
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: metas,
        data: instruction::PostJournal { batch_bytes: batch.to_bytes().expect("batch encodes") }.data(),
    };
    let kp = dup(&w.v.evaluator);
    let svm = &mut w.v.e.svm;
    let bh = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&kp.pubkey()), &bh);
    let mut tx = Transaction::new_unsigned(msg);
    tx.partial_sign(&[&kp], bh);
    let shape = TxShape { bytes: bincode::serialize(&tx).expect("tx").len(), accounts: tx.message.account_keys.len(), signatures: 1 };
    let r = svm.send_transaction(tx);
    svm.expire_blockhash();
    (ok(r), shape)
}

fn sproof(source: u8, symbol: &str, time: i64, mid: u64) -> PriceProof {
    PriceProof {
        source,
        feed_id: feed_id(&NativeSha256, source, symbol),
        timestamp_ns: u64::try_from(time).unwrap() * 1_000_000_000,
        quantized_value: i128::from(mid) * 1_000_000,
        // a signed GMX report (source 3) is archived by the sha256 of its blob (decision 47); the unsigned sources carry nothing
        signed_update: if source == 3 { vec![0xAB; 32] } else { vec![] },
    }
}

/// Traders of fixed keys: the cost of a bump search depends on the key, so the worst case is taken over fixed keys, the same in
/// every run: the first `plain` seeds and the `WORST` keys whose program-derived addresses need the most bump attempts (found by
/// searching the first 8,000 seeds natively; a hand-built worst case).
const WORST: usize = 8;
/// Fills at the mid 1.0 in a market with the test half-spread 1 e6 (decision 64), by hand: long ceil(1e12 x (1 + 1e-6)), short floor(1e12 x (1 - 1e-6)).
const LONG_AT_MID: u64 = 1_000_001_000_000;
const SHORT_AT_MID: u64 = 999_999_000_000;

fn bump_depth(trader: &solana_pubkey::Pubkey) -> u32 {
    use setl8_shared_interfaces as si;
    let t = trader.as_ref();
    let mut d = u32::from(255 - solana_pubkey::Pubkey::find_program_address(&[setl8_lev_trading::constants::WALLET_SEED, t], &PROGRAM_ID).1);
    let mut vts0 = None;
    for id in 0..=1u64 {
        d += u32::from(255 - solana_pubkey::Pubkey::find_program_address(&[setl8_lev_trading::constants::ACCT_SEED, t, &id.to_le_bytes()], &PROGRAM_ID).1);
        let (vts, bump) = solana_pubkey::Pubkey::find_program_address(&[b"trader_state", PROGRAM_ID.as_ref(), t, &id.to_le_bytes()], &VAULT_ID);
        d += u32::from(255 - bump);
        vts0.get_or_insert(vts);
    }
    for request in 1..=4u64 {
        d += u32::from(255 - si::derive_payout_claim(&VAULT_ID, &vts0.expect("vts"), request).1);
    }
    d
}

fn ranked() -> &'static Vec<(u32, u32)> {
    static R: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let key = |seed: u32| {
            let mut bytes = [0x5Au8; 32];
            bytes[..4].copy_from_slice(&seed.to_le_bytes());
            solana_keypair::Keypair::new_from_array(bytes).pubkey()
        };
        let mut r: Vec<(u32, u32)> = (1..=8_000u32).map(|s| (bump_depth(&key(s)), s)).collect();
        r.sort_by(|a, b| b.cmp(a));
        r
    })
}

fn key_set(plain: u32) -> Vec<u32> {
    let ranked = ranked();
    let mut keys: Vec<u32> = (1..=plain).collect();
    keys.extend(ranked.iter().take(WORST).map(|r| r.1).filter(|s| *s > plain));
    keys
}

fn evaluator_base(seed: u32) -> (JEnv, i64) {
    let mut w = JEnv::with_trader(8, Some(seed));
    w.set_limits(DELAY as u32, CAP, WINDOW);
    let t = w.t0 + 1_000;
    (w, t)
}

/// The grid: 1, 4 and 8 events x 0 to 3 proofs x sources 2, 3 and 4 (a market of each source with the same symbol).
fn post_grid(t: &mut Table) {
    let (mut w, mut now) = evaluator_base(1);
    for (source, market) in [(2u8, 5u16), (3, 6), (4, 7)] {
        w.add_market(market, source, "EURUSD", 1);
    }
    for (source, market) in [(2u8, 5u16), (3, 6), (4, 7)] {
        for n in [1usize, 4, 8] {
            for k in 0..=3usize {
                if k + 1 > n {
                    continue;
                }
                now += 10;
                let mut events: Vec<JournalEvent> = (0..k).map(|i| fill(market, i % 2 == 0, MEUR, if i % 2 == 0 { LONG_AT_MID } else { SHORT_AT_MID }, now, u8::try_from(i).unwrap())).collect();
                events.extend((0..n - 1 - k).map(|_| fee(market, 1, now)));
                events.push(meta(now, false));
                assert_eq!(events.len(), n);
                let proofs: Vec<PriceProof> = (0..k).map(|_| sproof(source, "EURUSD", now, MID)).collect();
                let b = w.batch(events, proofs);
                let (m, shape) = post_prod(&mut w, now + 1, &b, &[market]);
                t.rec("post_journal", &format!("source {source}, {n} events, {k} proofs"), &m, shape);
            }
        }
    }
}

/// Every event kind alone (plus its Meta), and the heaviest mix.
fn post_kinds(t: &mut Table) {
    // Fee, Fill, Close, Meta
    let (mut w, now) = evaluator_base(1);
    let b = w.batch(vec![fee(MARKET_EUR, 1, now), meta(now, false)], vec![]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind Fee", &m, s);
    let b = w.batch(vec![fill_long(now, 0), meta(now, false)], vec![p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind Fill", &m, s);
    let b = w.batch(vec![close_long_pnl(now, now - 100, 100), meta(now, false)], vec![p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind Close", &m, s);
    let b = w.batch(vec![meta(now, true)], vec![]);
    let (m, s) = post_prod(&mut w, now + 2, &b, &[]);
    t.rec("post_journal", "kind Meta alone", &m, s);

    // TightStart
    let (mut w, now) = evaluator_base(1);
    let (p, r, ls, g, l) = (usd(1_100), S100K, usd(96_000), usd(1_100), usd(96_000) + usd(100));
    let ts = JournalEvent::TightStart { time: now, p_open: p, r, l_start: ls, g_peak: g, l_level: l, proof: 0 };
    let b = w.batch(vec![ts, meta(now, false)], vec![p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind TightStart", &m, s);

    // FloorRaised, TightEnd, Breach (tight mode on)
    let (mut w, now) = evaluator_base(1);
    set_tight(&mut w, S100K);
    let fr = JournalEvent::FloorRaised { time: now, g_peak: usd(1_201), l_level: usd(96_201), proof: 0 };
    let b = w.batch(vec![fr, meta(now, false)], vec![p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind FloorRaised", &m, s);
    let (mut w, now) = evaluator_base(1);
    set_tight(&mut w, S100K);
    let b = w.batch(vec![JournalEvent::TightEnd { time: now }, meta(now, true)], vec![]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kind TightEnd", &m, s);
    let (mut w, now) = evaluator_base(1);
    set_tight(&mut w, S100K);
    let l = usd(96_200);
    let breach = JournalEvent::Breach { time: now, l_level: l, b_after: l, proof: 0 };
    let b = w.batch(vec![close_long_pnl(now, now - 100, -3_800_000_000), breach, meta(now, true)], vec![p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "kinds Close + Breach", &m, s);

    // the heaviest mix: 8 events, 3 proofs (two fills, two closes, tight start, floor raise, fee, meta)
    let (mut w, now) = evaluator_base(1);
    let (p, r, ls, g, l) = (usd(1_100), S100K + 20, usd(96_000), usd(1_100), usd(96_000) + usd(100));
    let mut events = vec![
        fill(MARKET_EUR, true, MEUR, eur_ask(), now, 0),
        fill(MARKET_EUR, false, MEUR, eur_bid(), now, 1),
        close_long_pnl(now, now - 100, 10),
        close_long_pnl(now, now - 100, 10),
        JournalEvent::TightStart { time: now, p_open: p, r, l_start: ls, g_peak: g, l_level: l, proof: 0 },
        JournalEvent::FloorRaised { time: now, g_peak: usd(1_200), l_level: ls + usd(200), proof: 2 },
        fee(MARKET_EUR, 1, now),
        meta(now, false),
    ];
    for (i, idx) in [(2usize, 0u8), (3, 1)] {
        if let JournalEvent::Close { proof, .. } = &mut events[i] {
            *proof = idx;
        }
    }
    let b = w.batch(events, vec![p_eur(now), p_eur(now), p_eur(now)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
    t.rec("post_journal", "heaviest mix: 8 events, 3 proofs (fill, fill, close, close, tight start, floor raise, fee, meta)", &m, s);

    // the largest transaction: 3 proofs and all 8 market accounts the program accepts
    let (mut w, now) = evaluator_base(1);
    for id in 5..=11u16 {
        w.add_market(id, 2, "EURUSD", 1);
    }
    let all: Vec<u16> = [vec![MARKET_EUR], (5..=11u16).collect()].concat();
    let events = vec![fill(MARKET_EUR, true, MEUR, eur_ask(), now, 0), fill(MARKET_EUR, false, MEUR, eur_bid(), now, 1), fill(MARKET_EUR, true, MEUR, eur_ask(), now, 2), meta(now, false)];
    let b = w.batch(events, vec![sproof(2, "EURUSD", now, MID), sproof(2, "EURUSD", now, MID), sproof(2, "EURUSD", now, MID)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &all);
    t.rec("post_journal", "largest transaction: 3 proofs, 8 market accounts", &m, s);
    // ... and with signed-report digests: the feed of market 6 is source 3
    let (mut w, now) = evaluator_base(1);
    w.add_market(5, 3, "EURUSD", 1);
    for id in 6..=11u16 {
        w.add_market(id, 2, "EURUSD", 1);
    }
    let all: Vec<u16> = [vec![5u16], vec![MARKET_EUR], (6..=11u16).collect()].concat();
    let events = vec![fill(5, true, MEUR, LONG_AT_MID, now, 0), fill(5, false, MEUR, SHORT_AT_MID, now, 1), fill(5, true, MEUR, LONG_AT_MID, now, 2), meta(now, false)];
    let b = w.batch(events, vec![sproof(3, "EURUSD", now, MID), sproof(3, "EURUSD", now, MID), sproof(3, "EURUSD", now, MID)]);
    let (m, s) = post_prod(&mut w, now + 1, &b, &all);
    t.rec("post_journal", "largest transaction with source 3: 3 proofs of 32-byte digests, 8 market accounts", &m, s);
}

/// `report_fail` signed by the evaluator alone (fee payer), nothing else in the transaction.
fn report_prod(w: &mut JEnv, clock: i64, batch: &JournalBatch, markets: &[u16]) -> (TransactionMetadata, TxShape) {
    let kp = dup(&w.v.evaluator);
    let ix = w.report_fail_ix(kp.pubkey(), batch.to_bytes().unwrap(), 0, markets);
    w.v.e.set_clock(clock);
    let svm = &mut w.v.e.svm;
    let bh = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&kp.pubkey()), &bh);
    let mut tx = Transaction::new_unsigned(msg);
    tx.partial_sign(&[&kp], bh);
    let shape = TxShape { bytes: bincode::serialize(&tx).unwrap().len(), accounts: tx.message.account_keys.len(), signatures: 1 };
    let r = svm.send_transaction(tx);
    svm.expire_blockhash();
    (ok(r), shape)
}

fn report_fail(t: &mut Table, seed: u32) {
    use journal_types::FailReason;
    for (fees, case) in [(0usize, "Fail + Meta (2 events, 1 proof)"), (6, "6 fees + Fail + Meta (8 events, 1 proof)")] {
        let (mut w, now) = evaluator_base(seed);
        w.edit_account(|a| a.b = 92_000_000_000);
        // the fees debit nothing (amount 0), so the balance stays at the static floor
        let b = w.fail_batch(now, vec![fee(MARKET_EUR, 0, now); fees], FailReason::StaticFloor, 92_000_000_000, true);
        let (m, s) = report_prod(&mut w, now + 1, &b, &[MARKET_EUR]);
        t.rec("report_fail", case, &m, s);
    }
    // the heaviest: 8 events and 3 proofs (two fills, four zero fees, the Fail, the Meta), also with signed-report digests (source 3)
    for (source, market) in [(2u8, MARKET_EUR), (3, 6u16)] {
        let (mut w, now) = evaluator_base(seed);
        for id in 5..=6u16 {
            w.add_market(id, if id == 5 { 2 } else { 3 }, "EURUSD", 1);
        }
        w.edit_account(|a| a.b = 92_000_000_000);
        let (long, short) = if market == MARKET_EUR { (eur_ask(), eur_bid()) } else { (LONG_AT_MID, SHORT_AT_MID) };
        let mut events = vec![fill(market, true, MEUR, long, now, 0), fill(market, false, MEUR, short, now, 1)];
        events.extend((0..4).map(|_| fee(market, 0, now)));
        events.push(JournalEvent::Fail { time: now, reason: FailReason::StaticFloor, b_after: 92_000_000_000, proof: 2 });
        events.push(meta(now, true));
        let proofs = (0..3).map(|_| sproof(source, "EURUSD", now, MID)).collect();
        let b = w.batch(events, proofs);
        let (m, s) = report_prod(&mut w, now + 1, &b, &[market]);
        t.rec("report_fail", &format!("8 events, 3 proofs, source {source}"), &m, s);
    }
}

// ================================================================ the pass flow, hold, activity, close

fn pass_flow(t: &mut Table, seed: u32) {
    let (mut w, mut now) = evaluator_base(seed);
    let payer = w.new_payer();
    for p in 1..=5u8 {
        w.ready(p);
        let m = w.ok(|w| w.request(now));
        t.jrec("request_advance", &format!("phase {p}"), &w, &m);
        let m = w.ok(|w| w.confirm(now + 1));
        t.jrec("confirm_pass", &format!("phase {p}"), &w, &m);
        if p >= 2 {
            now += 1 + DELAY;
            let m = w.ok(|w| w.finalize(now, &payer));
            t.jrec("finalize_pass", &format!("phase {p}{}", if p == 5 { " (graduates)" } else { "" }), &w, &m);
        }
        now += 100;
    }

    // reject_pass, cancel_pass, veto_pass
    let (mut w, now) = evaluator_base(seed);
    w.ready(2);
    w.ok(|w| w.request(now));
    let m = w.ok(|w| w.reject(now + 1, 0));
    t.jrec("reject_pass", "NotFlat", &w, &m);
    w.ok(|w| w.request(now + 2));
    let m = w.ok(|w| w.cancel(now + 2 + 3_600));
    t.jrec("cancel_pass", "after 3,600 s", &w, &m);
    w.ok(|w| w.request(now + 4_000));
    w.ok(|w| w.confirm(now + 4_001));
    let m = w.ok(|w| w.veto(now + 4_010));
    t.jrec("veto_pass", "confirmed pass", &w, &m);
}

fn lifecycle(t: &mut Table, seed: u32) {
    // hold, release_hold, fail_for_violation
    let (mut w, now) = evaluator_base(seed);
    let m = w.ok(|w| w.hold(now, hash(1)));
    t.jrec("hold_account", "fresh account", &w, &m);
    let m = w.ok(|w| w.release_hold(now + 1));
    t.jrec("release_hold", "held account", &w, &m);
    w.ok(|w| w.hold(now + 2, hash(2)));
    let m = w.ok(|w| w.violation(now + 3, hash(3)));
    t.jrec("fail_for_violation", "held account (vault flag_trader_failed CPI)", &w, &m);
    // close_account after the retention
    let at = w.account().terminal_at();
    let m = w.ok(|w| w.close(at + 7 * 86_400));
    t.jrec("close_account", "violation-failed account", &w, &m);

    // touch_activity and release_account: both call the vault once the activity is older than 86,000 s
    let (mut w, now) = evaluator_base(seed);
    let m = w.ok(|w| w.touch(now + 86_500));
    t.jrec("touch_activity", "with the vault call", &w, &m);
    let m = w.ok(|w| w.touch(now + 86_501));
    t.jrec("touch_activity", "throttled (no vault call)", &w, &m);
    let (mut w, now) = evaluator_base(seed);
    let m = w.ok(|w| w.release_acct(now + 86_500));
    t.jrec("release_account", "with the vault call", &w, &m);
}

// ================================================================ open and reset: the bump search of the trader's keys varies the cost

fn open_and_reset(t: &mut Table) {
    for tier in [0u8, 8, 9] {
        for seed in if tier == 8 { key_set(24) } else { key_set(4) } {
            // open_account: the first account of a wallet (creates the wallet profile and the vault record) and the second
            let mut v = VaultEnv::with(Opts { ruleset: spread_ruleset(), ..Opts::default() });
            let trader = v.new_trader_seeded(seed);
            let m = ok(v.open(&trader, tier));
            t.rec("open_account", &format!("first account of a wallet, tier {tier}, trader key {seed}"), &m, v.e.last_tx);
            let m = ok(v.open(&trader, tier));
            t.rec("open_account", &format!("second account, tier {tier}, trader key {seed}"), &m, v.e.last_tx);
        }
    }
    for phase in 1..=5u8 {
        for tier in [0u8, 8, 9] {
            for seed in if tier == 8 { key_set(24) } else { key_set(4) } {
                let mut w = JEnv::with_trader(tier, Some(seed));
                let now = w.t0 + 1_000;
                w.set_limits(DELAY as u32, CAP, WINDOW);
                w.fail_in(phase, now);
                let m = w.reset_ok(now + 100);
                t.jrec("reset_account", &format!("failed in phase {phase}, tier {tier}, trader key {seed}"), &w, &m);
            }
        }
    }
}

// ================================================================ the admin instructions

fn admin(t: &mut Table) {
    // init_config
    let mut e = Env::new();
    let ix = e.init_ix(&default_args());
    let m = ok(e.send_as_admins(&[ix]));
    t.rec("init_config", "both admins + payer", &m, e.last_tx);

    let mut v = VaultEnv::with(Opts { ruleset: spread_ruleset(), skip_ruleset: true, ..Opts::default() });
    let admins = Admins::both(&v.e);
    // create_market: the largest form (six sessions)
    let mut a = create_market_args(0, 6, 1);
    a.params.session_count = 6;
    for k in 0..6u16 {
        a.params.sessions[usize::from(k)] = setl8_lev_trading::state::Session { start_min: k * 1_000 + 1, end_min: k * 1_000 + 900 };
    }
    let ix = create_market_ix(v.e.payer.pubkey(), admins, &a);
    let m = ok(v.e.send_admins(ix, admins));
    t.rec("create_market", "six sessions", &m, v.e.last_tx);
    // create_ruleset: the full 552-byte account
    let rs = spread_ruleset();
    let ix = create_ruleset_ix(v.e.payer.pubkey(), admins, &rs);
    let m = ok(v.e.send_admins(ix, admins));
    t.rec("create_ruleset", "552-byte rule set, 10 tiers", &m, v.e.last_tx);
    // update_market: feed swap with six sessions
    let mut p = a.params.clone();
    p.feed_id = feed_id(&NativeSha256, 4, "xyz:AAPL");
    let ix = update_market_ix(admins, &UpdateMarketArgs { market_id: 0, params: p });
    let m = ok(v.e.send_admins(ix, admins));
    t.rec("update_market", "feed remap, six sessions", &m, v.e.last_tx);
    // set_evaluator, set_limits, set_paused (both ways)
    let ix = set_evaluator_ix(both(&v.e), solana_pubkey::Pubkey::new_unique());
    let m = ok(v.e.send_as_admins(&[ix]));
    t.rec("set_evaluator", "both admins", &m, v.e.last_tx);
    let ix = set_limits_ix(both(&v.e), 172_800, CAP, WINDOW);
    let m = ok(v.e.send_as_admins(&[ix]));
    t.rec("set_limits", "both admins", &m, v.e.last_tx);
    for paused in [true, false] {
        let ix = set_paused_ix(both(&v.e), paused);
        let m = ok(v.e.send_as_admins(&[ix]));
        t.rec("set_paused", if paused { "pause" } else { "unpause" }, &m, v.e.last_tx);
    }
    // ban_wallet (creates the wallet profile) and unban_wallet
    let stranger = solana_pubkey::Pubkey::new_unique();
    let ix = ban_wallet_ix(v.e.payer.pubkey(), admins, stranger, [7; 32]);
    let m = ok(v.e.send_admins(ix, admins));
    t.rec("ban_wallet", "wallet profile created", &m, v.e.last_tx);
    let ix = unban_wallet_ix(admins, stranger);
    let m = ok(v.e.send_admins(ix, admins));
    t.rec("unban_wallet", "existing profile", &m, v.e.last_tx);
}

// ================================================================ the differential campaign: worst case over many histories

fn campaign(t: &mut Table) {
    use ::evaluator::testkit::generate;
    use common::evaluator::shared::runner::{run_path, spec_rules};
    use common::evaluator::shared::World;
    let n: u64 = std::env::var("DIFF_PATHS").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    for seed in 1..=n {
        let spec = generate(seed);
        let mut chain = LiteChain::new(&spec);
        let world = World::from_params(spec_rules(&spec), chain.params.clone());
        let r = run_path(&spec, world, &mut chain);
        assert!(r.world.ev.fault().is_none(), "seed {seed}");
        for (shape, s) in &chain.shapes {
            let r = t.0.entry(format!("campaign: {shape}")).or_default();
            r.n += s.count;
            if s.max_cu > r.cu {
                r.cu = s.max_cu;
                r.cu_case = format!("seed {seed}");
            }
            if s.max_bytes > r.bytes {
                r.bytes = s.max_bytes;
                r.bytes_case = format!("seed {seed}");
            }
            r.signatures = 1;
            // the instruction's own row takes the campaign maximum too
            let name = if shape.starts_with("post_journal") { "post_journal" } else { "report_fail" };
            let row = t.0.entry(name.to_string()).or_default();
            row.n += s.count;
            if s.max_cu > row.cu {
                row.cu = s.max_cu;
                row.cu_case = format!("campaign seed {seed}: {shape}");
            }
            if s.max_bytes > row.bytes {
                row.bytes = s.max_bytes;
                row.bytes_case = format!("campaign seed {seed}: {shape}");
            }
        }
    }
}

// ================================================================ the table

fn suggested(cu: u64, bytes: usize) -> (u64, usize) {
    let c = (cu * 115).div_ceil(100).div_ceil(100) * 100;
    let b = (bytes * 115).div_ceil(100).div_ceil(10) * 10;
    (c.min(CU_LIMIT), b.min(TX_LIMIT))
}

#[test]
fn measured_budgets_of_every_instruction() {
    let mut t = Table::default();
    admin(&mut t);
    open_and_reset(&mut t);
    for seed in key_set(8) {
        pass_flow(&mut t, seed);
        lifecycle(&mut t, seed);
        report_fail(&mut t, seed);
    }
    post_grid(&mut t);
    post_kinds(&mut t);
    campaign(&mut t);

    println!("\n{:<44} {:>9} {:>7} {:>5} {:>4} {:>5} {:>6}   worst CU case | worst bytes case", "instruction", "CU (max)", "bytes", "accts", "sigs", "depth", "n");
    for (name, r) in &t.0 {
        println!("{:<44} {:>9} {:>7} {:>5} {:>4} {:>5} {:>6}   {} | {}", name, r.cu, r.bytes, r.accounts, r.signatures, r.depth, r.n, r.cu_case, r.bytes_case);
    }
    if std::env::var("BUDGET_CALIBRATE").is_ok() {
        println!("\nconst CEILINGS: &[(&str, u64, usize)] = &[");
        for (name, r) in t.0.iter().filter(|(n, _)| !n.starts_with("campaign:")) {
            let (c, b) = suggested(r.cu, r.bytes);
            println!("    (\"{name}\", {c}, {b}),");
        }
        println!("];");
        return;
    }
    let mut problems = Vec::new();
    for (name, r) in t.0.iter().filter(|(n, _)| !n.starts_with("campaign:")) {
        if r.cu > CU_LIMIT || r.bytes > TX_LIMIT {
            problems.push(format!("{name}: {} CU / {} bytes is over the limits {CU_LIMIT} / {TX_LIMIT}", r.cu, r.bytes));
        }
        match CEILINGS.iter().find(|c| c.0 == name) {
            None => problems.push(format!("{name}: no ceiling (measured {} CU, {} bytes)", r.cu, r.bytes)),
            Some((_, cu, bytes)) => {
                if r.cu > *cu {
                    problems.push(format!("{name}: {} CU is over the ceiling {cu} (worst case: {})", r.cu, r.cu_case));
                }
                if r.bytes > *bytes {
                    problems.push(format!("{name}: {} bytes is over the ceiling {bytes} (worst case: {})", r.bytes, r.bytes_case));
                }
            }
        }
    }
    for (name, cu, bytes) in CEILINGS {
        if !t.0.contains_key(*name) {
            problems.push(format!("ceiling for {name} but no measurement"));
        }
        if *cu > CU_LIMIT || *bytes > TX_LIMIT {
            problems.push(format!("{name}: ceiling {cu} CU / {bytes} bytes is above the limits"));
        }
    }
    assert!(problems.is_empty(), "budget problems:\n{}", problems.join("\n"));
}

// ================================================================ rent and the cost of opening an account

#[test]
fn rent_of_every_account_type_and_the_cost_of_opening_one_account() {
    let mut v = VaultEnv::with(Opts { ruleset: spread_ruleset(), ..Opts::default() });
    let rent = |v: &VaultEnv, len: usize| v.e.svm.minimum_balance_for_rent_exemption(len);
    let len_of = |v: &VaultEnv, a: &solana_pubkey::Pubkey| v.e.svm.get_account(a).map(|x| x.data.len()).unwrap_or(0);
    let (config_len, tally_len) = (len_of(&v, &config_pda()), len_of(&v, &tally_pda()));
    let ruleset_len = len_of(&v, &ruleset_pda(1));
    v.e.create_market_ok(&create_market_args(0, 0, 1));
    let market_len = len_of(&v, &market_pda(0));
    let trader = v.new_trader();
    let before_sol = v.e.svm.get_balance(&trader.pubkey()).unwrap();
    let tk = v.traders[&trader.pubkey()];
    let before_usdc = v.token_balance(&tk);
    let m = ok(v.open(&trader, 8));
    let after_sol = v.e.svm.get_balance(&trader.pubkey()).unwrap();
    let after_usdc = v.token_balance(&tk);
    let acct = acct_pda(&trader.pubkey(), 0);
    let (acct_len, wallet_len) = (len_of(&v, &acct), len_of(&v, &wallet_pda(&trader.pubkey())));
    let vts_len = len_of(&v, &vault_trader_state_pda(&trader.pubkey(), 0));
    // in production the trader is also the fee payer of `open_account` (payer == trader), one signature at 5,000 lamports;
    // the harness pays the fee from a separate payer, so the trader's balance moves by the rents only
    let fee_lamports = 5_000u64;
    println!("\naccount type              bytes      rent-exempt lamports");
    for (name, len) in [("TradingAccount", acct_len), ("WalletProfile", wallet_len), ("Market", market_len), ("RuleSet", ruleset_len), ("SectorConfig", config_len), ("PayoutTally", tally_len), ("vault TraderState", vts_len)] {
        println!("{name:<22} {len:>8} {:>20}", rent(&v, len));
    }
    let rents = rent(&v, acct_len) + rent(&v, wallet_len) + rent(&v, vts_len);
    println!(
        "open_account (first account of a wallet, $100K tier): the trader's SOL moved by {} lamports = rent {} (TradingAccount {} + WalletProfile {} + vault TraderState {}); plus the signature fee {} when the trader pays it = {} lamports in all; USDC spent {} (tier price 859,000,000 + fee 3,436,000); CU {}",
        before_sol - after_sol,
        rents,
        rent(&v, acct_len),
        rent(&v, wallet_len),
        rent(&v, vts_len),
        fee_lamports,
        rents + fee_lamports,
        before_usdc - after_usdc,
        m.compute_units_consumed
    );
    assert_eq!((acct_len, wallet_len, market_len, ruleset_len), (207, 56, 128, 552));
    assert_eq!(before_sol - after_sol, rents, "the trader paid exactly the three rents");
    assert_eq!(before_usdc - after_usdc, 859_000_000 + 3_436_000);
    // a second account of the same wallet pays no wallet profile rent
    let before_sol = v.e.svm.get_balance(&trader.pubkey()).unwrap();
    ok(v.open(&trader, 8));
    let second = before_sol - v.e.svm.get_balance(&trader.pubkey()).unwrap();
    assert_eq!(second, rent(&v, acct_len) + rent(&v, vts_len));
    println!("open_account (second account of the wallet): {second} lamports of rent (+ {fee_lamports} fee)");
    let _ = Keypair::new();
}

// ================================================================ wire sizes (DESIGN 4.9 and 8.2 said "about 300 bytes" per proof)

#[test]
fn encoded_sizes_of_events_and_proofs() {
    let now = 1_700_000_000i64;
    let sizes: Vec<(&str, usize)> = vec![
        ("Fill", fill(MARKET_EUR, true, MEUR, eur_ask(), now, 0).to_bytes().len()),
        ("Close", close_long_pnl(now, now - 100, 10).to_bytes().len()),
        ("Fee", fee(MARKET_EUR, 1, now).to_bytes().len()),
        ("TightStart", JournalEvent::TightStart { time: now, p_open: 1, r: 1, l_start: 1, g_peak: 1, l_level: 1, proof: 0 }.to_bytes().len()),
        ("FloorRaised", JournalEvent::FloorRaised { time: now, g_peak: 1, l_level: 1, proof: 0 }.to_bytes().len()),
        ("TightEnd", JournalEvent::TightEnd { time: now }.to_bytes().len()),
        ("Breach", JournalEvent::Breach { time: now, l_level: 1, b_after: 1, proof: 0 }.to_bytes().len()),
        ("Fail", JournalEvent::Fail { time: now, reason: journal_types::FailReason::StaticFloor, b_after: 1, proof: 0 }.to_bytes().len()),
        ("Meta", meta(now, true).to_bytes().len()),
        ("proof, source 2 (unsigned)", sproof(2, "BTC", now, MID).to_bytes().unwrap().len()),
        ("proof, source 3 (32-byte digest)", sproof(3, "BTC", now, MID).to_bytes().unwrap().len()),
        ("proof, source 4 (unsigned)", sproof(4, "xyz:AAPL", now, MID).to_bytes().unwrap().len()),
        ("batch of one Meta event, no proof", JournalBatch { first_seq: 1, prev_hash: [0; 32], events: vec![meta(now, true)], proofs: vec![] }.to_bytes().unwrap().len()),
    ];
    println!();
    for (name, n) in &sizes {
        println!("{name:<60} {n:>4} bytes");
    }
    let got: BTreeMap<&str, usize> = sizes.into_iter().collect();
    // Borsh sizes, pinned: a change of the wire format changes the transaction budget
    let want = [("Fill", 66), ("Close", 67), ("Fee", 20), ("TightStart", 50), ("FloorRaised", 26), ("TightEnd", 9), ("Breach", 26), ("Fail", 19), ("Meta", 76)];
    for (k, n) in want {
        assert_eq!(got[k], n, "{k}");
    }
    assert_eq!(got["proof, source 2 (unsigned)"], 61);
    assert_eq!(got["proof, source 3 (32-byte digest)"], 93);
    assert_eq!(got["proof, source 4 (unsigned)"], 61);
    // first_seq 8 + prev_hash 32 + the two length prefixes 4 + 4 around the events and proofs
    assert_eq!(got["batch of one Meta event, no proof"], 48 + got["Meta"]);
}
