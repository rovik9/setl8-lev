//! M12, item 8: the DISHONEST EVALUATOR table (finding prefix TMX). For each thing a dishonest evaluator (the hot key is stolen, or
//! the operator cheats) can try, the dishonest history is built from an honest one, posted to the REAL program and vault in LiteSVM,
//! run through the public verifier, and the outcome must be exactly one of
//!
//! * PROGRAM  : the program rejects (exact error code), the stored state is byte-identical afterwards (the digest covers the account,
//!   the config, the wallet, the rule set, the vault records and every tracked token account) and the verifier has nothing to add;
//! * VERIFIER : the program accepts every batch and the verifier flags exactly the stated finding classes;
//! * BOTH     : the program rejects (exact code, state unchanged) AND the verifier flags exactly the stated classes;
//! * NEITHER  : accepted by the program and VALID for the verifier. Then the case names the row of the NEITHER list of
//!   `handover/trust-matrix.md` it is, and that row must be in the document's NEITHER table and under "Residual trust".
//!
//! Where a case is already a row of the trust matrix (`trust_matrix.rs`) it names the row and the test checks that the document gives
//! the same WHO for that row. The scaffolding (scenarios, `T`, `honest`) is copied from `trust_matrix.rs` because that file's helpers
//! are private. Unlike `trust_matrix.rs` the verifier's finding set is compared for EQUALITY (not "contains"), and a refusal must
//! leave the whole world byte-identical.
#![allow(dead_code)]
mod common;

use ::evaluator::codec::{IntentKind, OrderIntent};
use ::evaluator::testkit::{MarketSpec, Scenario, Step};
use ::evaluator::{AccountParams, Batch, RulesCfg};
use common::evaluator::shared::runner::{items_text, run_path, spec_rules, Chain, Item, SimChain};
use common::evaluator::shared::{self, market_order, order, p4, World, E12, HOUR, MEUR, T_1400};
use common::evaluator::*;
use common::journal::TEST_HALF_SPREAD_E6;
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent, OrderKind, PriceProof, Side};
use setl8_lev_trading::errors::Setl8Error;
use setl8_verify::archive::{ArchiveWriter, Sample};
use setl8_verify::FindingClass as F;
use solana_keypair::Keypair;

const S100K: i64 = 100_000_000_000;

struct Script {
    steps: Vec<Step>,
    nonce: u32,
}

impl Script {
    fn new() -> Script {
        Script { steps: Vec::new(), nonce: 1 }
    }
    fn s(&mut self, m: u16, sec: i64, mid: u64) -> &mut Self {
        self.steps.push(Step::Sample { market: m, sec, mid });
        self
    }
    fn o(&mut self, intent: OrderIntent) -> u32 {
        let n = self.nonce;
        self.nonce += 1;
        self.steps.push(Step::Order { nonce: n, intent });
        n
    }
    fn post(&mut self, sec: i64) -> &mut Self {
        self.steps.push(Step::Post { sec, heartbeat: false });
        self
    }
    fn tick(&mut self, sec: i64) -> &mut Self {
        self.steps.push(Step::Tick { sec });
        self
    }
    fn end(&mut self, sec: i64) {
        self.steps.push(Step::Tick { sec });
        self.steps.push(Step::Post { sec, heartbeat: true });
    }
}

fn eur_spec(id: u16, symbol: &str, spread: u32, max_age: u16, sessions: Vec<(u16, u16)>) -> MarketSpec {
    MarketSpec::plain(id, 0, symbol.to_string(), 2, E12, max_age, spread, sessions)
}

fn base_spec(markets: Vec<MarketSpec>, commission0: u32, phase: u8, steps: Vec<Step>) -> ::evaluator::testkit::PathSpec {
    let mut commission = [0u32; 8];
    commission[0] = commission0;
    ::evaluator::testkit::PathSpec {
        seed: 0,
        scenario: Scenario::Noise,
        tier: 8,
        size_micro: S100K,
        phase,
        opened_time: T_1400 - HOUR,
        markets,
        commission_e6: commission,
        steps,
        batch_max_events: 8,
        end: 0,
    }
}

fn close_order(m: u16, t: i64) -> OrderIntent {
    shared::close_order(m, 0, t)
}

/// DESIGN 4.9 with a 0.002% commission: fill, tight start, raise, breach. A `Post` after every sample: one batch each.
fn sc_breach() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let m = |p: u64| p4(p);
    s.s(0, T_1400, m(10_800));
    s.o(market_order(0, true, 1_000_000_000_000, T_1400));
    s.post(T_1400 + 1);
    for (dt, p) in [(30, 10_806), (60, 10_813), (90, 10_831), (120, 10_811), (150, 10_780)] {
        s.s(0, T_1400 + dt, m(p));
        s.post(T_1400 + dt + 1);
    }
    s.end(T_1400 + 160);
    base_spec(vec![eur_spec(0, "EURUSD", 30, 60, vec![(0, 10_080)])], 20, 2, s.steps)
}

/// A quick profitable round trip with commission, then flat.
fn sc_quick() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_800));
    s.o(market_order(0, true, 200_000_000_000, T_1400));
    s.s(0, T_1400 + 20, p4(10_830));
    s.o(close_order(0, T_1400 + 20));
    s.s(0, T_1400 + 25, p4(10_830));
    s.end(T_1400 + 30);
    base_spec(vec![eur_spec(0, "EURUSD", 30, 60, vec![(0, 10_080)])], 20, 2, s.steps)
}

/// A position held through 22:00 UTC with commission, closed afterwards.
fn sc_night() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let t = T_1400 + 7 * HOUR;
    let r = T_1400 + 8 * HOUR;
    s.s(0, t, p4(10_800));
    s.o(market_order(0, true, 300_000_000_000, t));
    s.s(0, r - 300, p4(10_805));
    s.s(0, r + 300, p4(10_810));
    s.post(r + 301);
    s.s(0, r + 900, p4(10_815));
    s.o(close_order(0, r + 900));
    s.s(0, r + 905, p4(10_815));
    s.end(r + 910);
    base_spec(vec![eur_spec(0, "EURUSD", 30, 900, vec![(0, 10_080)])], 20, 2, s.steps)
}

/// A buy at a moment when the price has been falling; the newest sample is lower than the one an evil evaluator would use.
fn sc_stale() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_100));
    s.s(0, T_1400 + 20, p4(10_000));
    s.o(market_order(0, true, MEUR / 2, T_1400 + 20));
    s.s(0, T_1400 + 30, p4(10_000));
    s.end(T_1400 + 35);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A buy limit that fills, a stop-loss that triggers.
fn sc_limit_sl() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    let lim = order(IntentKind::Limit, 0, 0, 500_000_000_000, p4(9_990), p4(9_950), 0, 0, T_1400);
    s.o(lim);
    s.s(0, T_1400 + 10, p4(9_995));
    s.s(0, T_1400 + 20, p4(9_985)); // the limit fills here
    s.post(T_1400 + 21);
    s.s(0, T_1400 + 30, p4(9_970));
    s.s(0, T_1400 + 40, p4(9_950)); // the stop-loss triggers here
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A slow profitable round trip (age 100 s, +600): a normal slice. No commission.
fn sc_slow() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 100, p4(10_006));
    s.o(close_order(0, T_1400 + 100));
    s.s(0, T_1400 + 105, p4(10_006));
    s.end(T_1400 + 110);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// +8.5% realized, flat, nothing else: a pass can be requested.
fn sc_win() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 100, p4(10_085));
    s.o(close_order(0, T_1400 + 100));
    s.s(0, T_1400 + 110, p4(10_085));
    s.end(T_1400 + 115);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// +8.5% realized (a normal slice), flat; then a new small position stays open.
fn sc_pass_open() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 100, p4(10_085));
    s.o(close_order(0, T_1400 + 100));
    s.s(0, T_1400 + 110, p4(10_085));
    s.post(T_1400 + 111);
    s.o(market_order(0, true, MEUR / 10, T_1400 + 111));
    s.s(0, T_1400 + 120, p4(10_085));
    s.end(T_1400 + 125);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

struct T {
    rules: RulesCfg,
    params: AccountParams,
    batches: Vec<Batch>,
    /// The order in which batches are posted (a replay lists one twice).
    order: Vec<usize>,
    samples: Vec<Sample>,
    intents: Vec<(u32, Vec<u8>, Vec<u8>)>,
    second: Option<Vec<Sample>>,
    /// Chain-side actions before posting the batch at that position of `order`.
    hold_before: Option<usize>,
    signer: Option<[u8; 32]>,
    /// Extra log lines (pass events) appended at the end of the log.
    extra_lines: Vec<String>,
    /// Chain actions run after all batches are posted (the pass flow); their log lines are collected.
    pass_flow: bool,
    /// Raw bytes to post instead of the encoding of a batch (index into `batches`).
    raw: std::collections::BTreeMap<usize, Vec<u8>>,
}

impl T {
    fn find(&self, f: impl Fn(&JournalEvent) -> bool) -> (usize, usize) {
        for (bi, b) in self.batches.iter().enumerate() {
            for (ei, e) in b.batch.events.iter().enumerate() {
                if f(e) {
                    return (bi, ei);
                }
            }
        }
        panic!("event not found");
    }
    fn ev(&mut self, at: (usize, usize)) -> &mut JournalEvent {
        &mut self.batches[at.0].batch.events[at.1]
    }
    /// Recomputes `first_seq` and `prev_hash` of every batch from the genesis (a malicious evaluator keeps its chain consistent).
    fn rechain(&mut self) {
        let h = ::evaluator::sha256::chain_hasher();
        let mut head = journal_types::genesis_hash(&h, &self.params.account);
        let mut seq = 0u64;
        for b in &mut self.batches {
            b.batch.first_seq = seq + 1;
            b.batch.prev_hash = head;
            let n = b.batch.events.len() as u64;
            // events that cannot be hashed (a proof index out of range) leave the head as it is: the program rejects them first
            if let Ok((nh, _)) = journal_types::apply_batch(&h, &head, b.batch.first_seq, &b.batch) {
                head = nh;
            }
            seq += n;
        }
    }
    /// Recomputes every Meta's two hashes from the CURRENT archive and intents, with the committed counts of each batch.
    fn rehash_metas(&mut self) {
        use sha2::{Digest, Sha256};
        let stream: Vec<u8> = self.samples.iter().flat_map(|s| s.to_bytes()).collect();
        let text = self.intents_text();
        for b in &mut self.batches {
            let n = (b.samples_committed * 68).min(stream.len());
            let sh: [u8; 32] = Sha256::digest(&stream[..n]).into();
            let ih: [u8; 32] = Sha256::digest(&text.as_bytes()[..b.intents_committed_bytes.min(text.len())]).into();
            if let Some(JournalEvent::Meta { samples_hash, intents_hash, .. }) = b.batch.events.last_mut() {
                *samples_hash = sh;
                *intents_hash = ih;
            }
        }
    }
    fn archive_bytes(&self) -> Vec<u8> {
        let mut w = ArchiveWriter::new();
        for s in &self.samples {
            w.push(s);
        }
        w.into_bytes()
    }
    fn intents_text(&self) -> String {
        let mut t = setl8_verify::intents::header_line();
        for (n, o, s) in &self.intents {
            t.push_str(&setl8_verify::intents::line_for(*n, o, s));
        }
        t
    }
}

fn vsample(s: &::evaluator::codec::SampleRec) -> Sample {
    Sample { market: s.market, source: s.source, recv_ns: s.recv_ns, ts_ns: s.ts_ns, value: s.value, digest: s.digest }
}

/// Runs a scripted history through the evaluator (no chain) and returns the tampering view plus the live chain it will be posted to.
fn honest(spec: &::evaluator::testkit::PathSpec) -> (T, LiteChain) {
    let chain = LiteChain::new(&{
        let mut s = spec.clone();
        s.seed = 0;
        s
    });
    let world = World::from_params(spec_rules(spec), chain.params.clone());
    let mut sim = SimChain::default();
    let r = run_path(spec, world, &mut sim);
    assert!(r.notes.is_empty(), "{:?}", r.notes);
    let w = &r.world;
    let batches: Vec<Batch> = r.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.clone()) } else { None }).collect();
    let samples = (0..w.ev.sample_count()).filter_map(|i| w.ev.sample(i).map(|s| vsample(&s.rec))).collect();
    let text = String::from_utf8(w.ev.intents_bytes().to_vec()).unwrap();
    let mut intents = Vec::new();
    for line in text.lines().skip(1) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let nonce = v["nonce"].as_u64().unwrap() as u32;
        let o = setl8_verify::hexutil::decode(v["order"].as_str().unwrap()).unwrap();
        let sg = setl8_verify::hexutil::decode(v["sig"].as_str().unwrap()).unwrap();
        intents.push((nonce, o, sg));
    }
    let order = (0..batches.len()).collect();
    let t = T {
        rules: w.rules.clone(),
        params: chain.params.clone(),
        batches,
        order,
        samples,
        intents,
        second: None,
        hold_before: None,
        signer: None,
        extra_lines: Vec::new(),
        pass_flow: false,
        raw: Default::default(),
    };
    (t, chain)
}

fn ev_name(e: &JournalEvent) -> &'static str {
    match e {
        JournalEvent::Fill { .. } => "Fill",
        JournalEvent::Close { .. } => "Close",
        JournalEvent::Fee { kind: FeeKind::Holding, .. } => "FeeH",
        JournalEvent::Fee { .. } => "FeeC",
        JournalEvent::TightStart { .. } => "TightStart",
        JournalEvent::FloorRaised { .. } => "FloorRaised",
        JournalEvent::TightEnd { .. } => "TightEnd",
        JournalEvent::Breach { .. } => "Breach",
        JournalEvent::Fail { .. } => "Fail",
        JournalEvent::Meta { .. } => "Meta",
    }
}

impl T {
    /// Rechain and rehash: the malicious evaluator publishes a consistent record.
    fn fix(&mut self) {
        self.rehash_metas();
        self.rechain();
    }
    fn feed_of(&self, market: u16) -> ([u8; 32], u8) {
        let m = self.rules.market(market).expect("market");
        (m.feed_id, m.source)
    }
    fn proof_of_sample(&self, i: usize) -> PriceProof {
        let s = &self.samples[i];
        let (feed, source) = self.feed_of(s.market);
        PriceProof { source, feed_id: feed, timestamp_ns: s.ts_ns, quantized_value: s.value, signed_update: if source == 3 { s.digest.to_vec() } else { vec![] } }
    }
    fn sample_at(&self, market: u16, sec: i64) -> usize {
        self.samples.iter().position(|s| s.market == market && s.ts_sec() == sec).expect("sample")
    }
    fn mid_at(&self, i: usize) -> u64 {
        u64::try_from(self.samples[i].value / 1_000_000).unwrap()
    }
    fn spread(&self, market: u16) -> u32 {
        self.rules.market(market).unwrap().half_spread_e6
    }
    fn set_meta_time(&mut self, bi: usize, time: i64) {
        self.batches[bi].meta_time = time;
        if let Some(JournalEvent::Meta { time: t, .. }) = self.batches[bi].batch.events.last_mut() {
            *t = time;
        }
    }
    fn set_meta_flat(&mut self, bi: usize, flat: bool) {
        if let Some(JournalEvent::Meta { flat: f, .. }) = self.batches[bi].batch.events.last_mut() {
            *f = flat;
        }
    }
    fn trader_sign(&self, nonce: u32, intent: &OrderIntent) -> (u32, Vec<u8>, Vec<u8>) {
        let s = shared::sign(&self.params.account, nonce, intent);
        (nonce, s.order.to_vec(), s.sig.to_vec())
    }
    /// A hand-built batch appended at the end of the record: events, proofs, a Meta; a PostJournal or a ReportFail.
    fn append(&mut self, kind: ::evaluator::BatchKind, events: Vec<JournalEvent>, proofs: Vec<PriceProof>, meta_time: i64, flat: bool, markets: Vec<u16>) {
        let mut events = events;
        events.push(JournalEvent::Meta { time: meta_time, flat, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: engine::ENGINE_VERSION });
        let last = self.batches.last().cloned().expect("a batch");
        let mut b = last;
        b.kind = kind;
        b.batch.events = events;
        b.batch.proofs = proofs;
        b.markets = markets;
        b.meta_time = meta_time;
        b.samples_committed = self.samples.len();
        b.intents_committed_bytes = self.intents_text().len();
        self.batches.push(b);
        self.order.push(self.batches.len() - 1);
    }
    fn hand_fill(&self, sample: usize, long: bool, qty: u64, time: i64, intent_id: u32, proof: u8) -> JournalEvent {
        let m = self.samples[sample].market;
        let mid = self.mid_at(sample);
        let h = self.spread(m);
        let price = engine::fill_price_e12(mid, h, long).unwrap();
        let lev = self.rules.leverage_x[usize::from(self.rules.market(m).unwrap().class)];
        let margin = u64::try_from(engine::margin_micro(qty, price, lev).unwrap()).unwrap();
        JournalEvent::Fill { market: m, side: if long { Side::Long } else { Side::Short }, order_kind: OrderKind::Market, qty, price_e12: price, time, avg_entry_after: price, margin, sl: 0, tp: 0, intent_id, proof }
    }
    #[allow(clippy::too_many_arguments)]
    fn hand_close(&self, sample: usize, long: bool, qty: u64, entry: u64, time: i64, timer_start: i64, reason: CloseReason, proof: u8) -> JournalEvent {
        let m = self.samples[sample].market;
        let mid = self.mid_at(sample);
        let exit = engine::close_price_e12(mid, self.spread(m), long).unwrap();
        let pnl = engine::pnl_micro(long, qty, entry, exit).unwrap();
        let quick = time - timer_start <= 45;
        let (credited, _) = engine::slice_credit(pnl, quick);
        JournalEvent::Close { market: m, side: if long { Side::Long } else { Side::Short }, qty, entry_e12: entry, exit_e12: exit, time, timer_start, pnl, quick, credited, reason, intent_id: 0, proof }
    }
    fn account_after(&self, bi: usize) -> ::evaluator::AccountSnapshot {
        self.batches[bi].account_after
    }
}

// ------------------------------------------------------------------------------------------------- scenarios of this file

/// Two markets, 1 M EUR long in each (spread `TEST_HALF_SPREAD_E6`), each dips by 0.5% (-5,000) ALONE, never together: the lowest equity
/// is about $95,000, above the static floor ($92,000), so the honest account never fails. The two worst samples (market 0 at +20,
/// market 1 at +31) are both inside `max_age` (60 s) of a batch dated +50, which is what the "mass fail" attack needs.
fn sc_dips() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let t = T_1400;
    s.s(0, t, p4(10_000)).s(1, t + 1, p4(10_000));
    s.o(market_order(0, true, MEUR, t + 1));
    s.o(market_order(1, true, MEUR, t + 1));
    s.s(0, t + 10, p4(10_000)).s(1, t + 11, p4(10_000));
    s.post(t + 12);
    s.s(0, t + 20, p4(9_950)).s(1, t + 21, p4(10_000));
    s.post(t + 22);
    s.s(0, t + 30, p4(10_000)).s(1, t + 31, p4(9_950));
    s.post(t + 32);
    s.s(0, t + 40, p4(10_000)).s(1, t + 41, p4(10_000));
    s.end(t + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Every honest scenario whose account is still Active at the end (the targets of a mass fail).
fn active_scenarios() -> Vec<(&'static str, ::evaluator::testkit::PathSpec)> {
    vec![("breach", sc_breach()), ("quick", sc_quick()), ("night", sc_night()), ("pass_open", sc_pass_open()), ("limit_sl", sc_limit_sl()), ("dips", sc_dips()), ("slow", sc_slow()), ("win", sc_win()), ("stale", sc_stale())]
}

// ------------------------------------------------------------------------------------------------- run: program, then verifier

struct Run {
    /// The first rejected batch (its position in `order`) and the program error code.
    rejected: Option<(usize, Option<u32>)>,
    /// The digest of the whole world (accounts, config, vault records, token accounts) before and after the rejected transaction.
    unchanged_after_refusal: bool,
    posted_ok: usize,
    classes: Vec<F>,
    text: String,
}

fn run(t: &T, chain: &mut LiteChain) -> Run {
    let mut rejected = None;
    let mut unchanged = true;
    let mut posted_ok = 0;
    for (pos, &bi) in t.order.iter().enumerate() {
        let before = chain.j.digest();
        match chain.try_post_bytes(&t.batches[bi], t.raw.get(&bi).cloned(), t.signer.map(Keypair::new_from_array).as_ref()) {
            Ok(_) => posted_ok += 1,
            Err(e) => {
                rejected = Some((pos, e.code));
                unchanged = chain.j.digest() == before;
                break;
            }
        }
    }
    if rejected.is_none() && t.pass_flow {
        let w = World::base();
        let at = chain.clock + 5;
        if chain.request_advance(&w, at) {
            chain.confirm_pass(&w, at + 1);
        }
    }
    let (classes, text) = verify(t, chain);
    Run { rejected, unchanged_after_refusal: unchanged, posted_ok, classes, text }
}

/// The public verifier over everything the dishonest evaluator published (as if all of it had been accepted) plus the program's own
/// pass lines (`chain.lines`) and the forged lines in `t.extra_lines`.
fn verify(t: &T, chain: &mut LiteChain) -> (Vec<F>, String) {
    let mut items: Vec<Item> = t.order.iter().map(|&i| Item::Batch(t.batches[i].clone())).collect();
    for l in chain.lines.drain(..) {
        items.push(Item::Line(l));
    }
    for l in &t.extra_lines {
        items.push(Item::Line(l.clone()));
    }
    let events = items_text(&t.params, &items);
    let rules = shared::rules_json(&t.rules, t.params.size_micro);
    let archive = t.archive_bytes();
    let intents = t.intents_text();
    let second = t.second.as_ref().map(|s| {
        let mut w = ArchiveWriter::new();
        for x in s {
            w.push(x);
        }
        w.into_bytes()
    });
    let rep = setl8_verify::verify_account(&setl8_verify::Inputs { events: events.as_bytes(), rules: rules.as_bytes(), archive: &archive, intents: intents.as_bytes(), snapshot: None, second_archive: second.as_deref(), as_of_seq: None })
        .expect("inputs parse");
    (rep.classes(), shared::describe(&rep))
}

// ------------------------------------------------------------------------------------------------- the doc

fn trust_doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/trust-matrix.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("handover/trust-matrix.md must be readable: {e}"))
}

/// The cells of every markdown table row of `text` (header and separator lines dropped).
fn table_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| l.starts_with("| "))
        .map(|l| l.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect::<Vec<_>>())
        .filter(|cells| cells[0] != "class" && !cells[0].starts_with("---"))
        .collect()
}

/// WHO the document gives a trust-matrix row (batch/log rows and chain rows).
fn doc_who(row: &str) -> String {
    let doc = trust_doc();
    let (head, _) = doc.split_once("## Reset (M8)").expect("Reset section");
    let hits: Vec<Vec<String>> = table_rows(head).into_iter().filter(|c| c[0] == row).collect();
    assert_eq!(hits.len(), 1, "trust-matrix.md must list the row exactly once: {row}");
    hits[0][1].clone()
}

fn doc_neither_rows() -> Vec<String> {
    let doc = trust_doc();
    let part = doc.split_once("## NEITHER").expect("NEITHER section").1.split_once("## Reset (M8)").expect("Reset section").0;
    table_rows(part).into_iter().map(|c| c[0].clone()).collect()
}

// ------------------------------------------------------------------------------------------------- the table

#[derive(Clone, Copy, Debug)]
enum Expect {
    /// The program rejects with this error; the verifier has nothing to add.
    Program(Setl8Error),
    /// The program accepts everything; the verifier flags exactly these classes.
    Verifier(&'static [F]),
    /// The program rejects with this error AND the verifier flags exactly these classes.
    Both(Setl8Error, &'static [F]),
    /// Accepted and VALID. The text is the name of the row of the NEITHER list.
    Neither(&'static str),
    /// Accepted by the program and VALID for the verifier although the DESIGN says the verifier must flag it, and it is NOT a row of the
    /// NEITHER list: a FINDING (the text is its id). The test pins today's behaviour; the `#[ignore]` test of the same id asserts the
    /// behaviour the DESIGN requires.
    Gap(&'static str),
}

impl Expect {
    fn who(&self) -> &'static str {
        match self {
            Expect::Program(_) => "Program",
            Expect::Verifier(_) => "Verifier",
            Expect::Both(..) => "Both",
            Expect::Neither(_) => "Neither",
            Expect::Gap(_) => "Gap",
        }
    }
}

struct Case {
    id: &'static str,
    /// One of the nine things a dishonest evaluator tries (the prompt's list).
    family: &'static str,
    attack: &'static str,
    /// The trust-matrix row of the same attack, when there is one.
    row: Option<&'static str>,
    sc: fn() -> ::evaluator::testkit::PathSpec,
    tamper: fn(&mut T),
    expect: Expect,
    /// Extra assertions on the chain after the run (consequences).
    after: Option<fn(&mut T, &mut LiteChain)>,
}

const FAMILIES: [&str; 9] = ["fabricated pass", "omitted breach", "replayed old price", "mass fail", "double fill", "wrong side of the spread", "skipped rollover fee", "fee applied twice", "omitted fail"];

fn names(c: &[F]) -> Vec<String> {
    let mut v: Vec<String> = c.iter().map(|x| format!("{x:?}")).collect();
    v.sort();
    v.dedup();
    v
}

fn check_case(c: &Case) {
    let (mut t, mut chain) = honest(&(c.sc)());
    (c.tamper)(&mut t);
    let r = run(&t, &mut chain);
    let got_classes = names(&r.classes);
    let ctx = format!("{} [{}]: {}", c.id, c.family, c.attack);
    match c.expect {
        Expect::Program(e) | Expect::Both(e, _) => {
            let (pos, code) = r.rejected.unwrap_or_else(|| panic!("{ctx}: the program accepted every batch (verifier: {got_classes:?})\n{}", r.text));
            assert_eq!(code, Some(u32::from(e)), "{ctx}: wrong program error at batch {pos}, wanted {e:?}");
            assert!(r.unchanged_after_refusal, "{ctx}: the refusal changed the world (digest differs)");
        }
        Expect::Verifier(_) | Expect::Neither(_) | Expect::Gap(_) => {
            assert!(r.rejected.is_none(), "{ctx}: the program rejected batch {:?}, expected it to accept", r.rejected);
            assert_eq!(r.posted_ok, t.order.len(), "{ctx}: not every batch was posted");
        }
    }
    let want_classes = match c.expect {
        Expect::Verifier(f) | Expect::Both(_, f) => names(f),
        Expect::Program(_) | Expect::Neither(_) | Expect::Gap(_) => Vec::new(),
    };
    assert_eq!(got_classes, want_classes, "{ctx}: verifier findings differ\n{}", r.text);
    if let Expect::Neither(row) = c.expect {
        assert!(doc_neither_rows().iter().any(|n| n == row), "{ctx}: '{row}' is not a row of the NEITHER list in trust-matrix.md");
    }
    if let Some(row) = c.row {
        assert_eq!(doc_who(row), c.expect.who(), "{ctx}: the trust matrix gives row '{row}' another WHO");
    }
    if let Some(f) = c.after {
        f(&mut t, &mut chain);
    }
}

fn case_by_id(id: &str) -> Case {
    let mut hits: Vec<Case> = cases().into_iter().filter(|c| c.id == id).collect();
    assert_eq!(hits.len(), 1, "case id must exist exactly once: {id}");
    hits.remove(0)
}

macro_rules! case_tests {
    ($($f:ident => $id:expr),* $(,)?) => {
        $( #[test] fn $f() { check_case(&case_by_id($id)); } )*
        const CASE_TESTS: &[(&str, &str)] = &[ $( (stringify!($f), $id) ),* ];
    };
}

// ---- tampering helpers

fn first_fill_of(t: &T, f: impl Fn(&JournalEvent) -> bool) -> (usize, usize) {
    t.find(f)
}

fn view_of(t: &T, bi: usize) -> ::evaluator::ChainView {
    let a = t.account_after(bi);
    ::evaluator::ChainView { seq: a.seq, journal_hash: a.journal_hash, b: a.b, flat: true, phase: 2 }
}

/// A Close at a wrong price keeps pnl and credit consistent with it (the evaluator's own arithmetic is right).
fn reprice_close(e: &mut JournalEvent, exit: u64) {
    if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = e {
        *exit_e12 = exit;
        *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
        *credited = engine::slice_credit(*pnl, *quick).0;
    }
}

/// `sc_slow` with the Close quantity multiplied: real prices, real proofs, an arithmetic that is right for the NEW quantity.
fn inflate_close_qty(t: &mut T, factor: u64) {
    let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
    if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = t.ev(at) {
        *qty *= factor;
        *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
        *credited = engine::slice_credit(*pnl, *quick).0;
    }
    t.fix();
}

/// The worst two samples of `sc_dips` closed together in a `report_fail(StaticFloor)` dated +50: every claim is arithmetically true,
/// every proof is a real sample inside `max_age`, but the two lows never existed at one instant.
fn dips_fail(t: &mut T) {
    let a = t.sample_at(0, T_1400 + 20);
    let b = t.sample_at(1, T_1400 + 31);
    let time = T_1400 + 50;
    let entry = 1_000_001_000_000u64; // ask(1.0, 1 e6)
    let close_a = t.hand_close(a, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 0);
    let close_b = t.hand_close(b, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 1);
    let snap = t.account_after(t.batches.len() - 1);
    let credited = |e: &JournalEvent| if let JournalEvent::Close { credited, .. } = e { *credited } else { unreachable!() };
    let b_after = snap.b + credited(&close_a) + credited(&close_b);
    assert!(b_after <= 92_000_000_000, "the fabricated balance {b_after} must be at or below the static floor");
    let fail = JournalEvent::Fail { time, reason: FailReason::StaticFloor, b_after, proof: 0 };
    let proofs = vec![t.proof_of_sample(a), t.proof_of_sample(b)];
    t.append(::evaluator::BatchKind::ReportFail(engine::FailReason::StaticFloor), vec![close_a, close_b, fail], proofs, time, true, vec![0, 1]);
    t.fix();
}

/// An honest Active account, then a `report_fail` the account does not deserve (reason and claimed balance as given).
fn append_fail(t: &mut T, reason: FailReason, b_after: Option<i64>) {
    let last = t.batches.len() - 1;
    let snap = t.account_after(last);
    let i = t.samples.len() - 1;
    let time = t.batches[last].meta_time;
    let fail = JournalEvent::Fail { time, reason, b_after: b_after.unwrap_or(snap.b), proof: 0 };
    let kind = match reason {
        FailReason::StaticFloor => engine::FailReason::StaticFloor,
        FailReason::PhaseQuick => engine::FailReason::PhaseQuick,
    };
    let m = t.samples[i].market;
    let proofs = vec![t.proof_of_sample(i)];
    t.append(::evaluator::BatchKind::ReportFail(kind), vec![fail], proofs, time, true, vec![m]);
    t.fix();
}

fn status_failed(_t: &mut T, chain: &mut LiteChain) {
    assert_eq!(chain.j.account().status, 1, "the program accepted the fabricated fail: the account is Failed (the vault has been told)");
}

fn cases() -> Vec<Case> {
    use Expect::*;
    vec![
        // ============================================================ 1. fabricates a pass
        Case {
            id: "DE-01a",
            family: "fabricated pass",
            attack: "the journal is inflated to reach 1.08 S: the exit price of the close is moved up by 0.009 and pnl and credit are recomputed",
            row: Some("close exit price altered (pnl recomputed)"),
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { exit_e12, .. } = t.batches[at.0].batch.events[at.1].clone() {
                    reprice_close(t.ev(at), exit_e12 + 9_000_000_000);
                }
                t.fix();
            },
            expect: Both(Setl8Error::EventPriceMismatch, &[F::PriceMismatch]),
            // the rejected batch never reached the account: it is still below the target, so the pass cannot even be requested
            after: Some(|_t, chain| {
                let r = chain.j.request(chain.clock + 5);
                common::assert_err(&r, Setl8Error::TargetNotReached);
            }),
        },
        Case {
            id: "DE-01b",
            family: "fabricated pass",
            attack: "REAL prices and proofs, but the Close names 20 times the quantity that was opened (+600 becomes +12,000): the program accepts, the pass is requested and confirmed on a fabricated balance",
            row: None,
            sc: sc_slow,
            tamper: |t| {
                inflate_close_qty(t, 20);
                t.pass_flow = true;
            },
            // the program has no position book (DESIGN 7.3): it cannot see that the Close is larger than the position
            expect: Verifier(&[F::PassInvalid, F::PositionMismatch, F::StateMismatch]),
            after: Some(|_t, chain| {
                let a = chain.j.account();
                assert!(a.b >= 108_000_000_000, "the fabricated balance reached the target: b = {}", a.b);
                assert_eq!(a.pass_pending, 2, "the pass is CONFIRMED on the chain (pass_pending 2): only the 2-day delay, the watchdog and the veto stand in the way");
            }),
        },
        Case {
            id: "DE-01f",
            family: "fabricated pass",
            attack: "REAL exit price and proof, but the Close names an entry price 0.009 lower than the position's (pnl and credit consistent with it): +600 becomes +9,600, the pass is requested and confirmed",
            row: Some("close names another entry price (pnl consistent with it)"),
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = t.ev(at) {
                    *entry_e12 -= 9_000_000_000;
                    *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
                    *credited = engine::slice_credit(*pnl, *quick).0;
                }
                t.fix();
                t.pass_flow = true;
            },
            // the program has no position book, so it cannot know the entry price either
            expect: Verifier(&[F::AvgEntryMismatch, F::PassInvalid, F::PnlMismatch]),
            after: Some(|_t, chain| {
                let a = chain.j.account();
                assert!(a.b >= 108_000_000_000, "the fabricated balance reached the target: b = {}", a.b);
                assert_eq!(a.pass_pending, 2, "the fabricated pass is CONFIRMED on the chain");
            }),
        },
        Case {
            id: "DE-01c",
            family: "fabricated pass",
            attack: "pass_requested and pass_confirmed lines are forged into the log of an account that is below the target (the lines are indexer output, nothing on the chain emitted them)",
            row: None,
            sc: sc_slow,
            tamper: |t| {
                use common::evaluator::shared::runner::{pass_confirmed_line, pass_requested_line};
                let v = view_of(t, t.batches.len() - 1);
                let time = t.batches.last().unwrap().meta_time + 5;
                t.extra_lines.push(pass_requested_line(&t.params.account, 2, 8_000_000_000, &v, time));
                t.extra_lines.push(pass_confirmed_line(&t.params.account, 2, 8_000_000_000, &v, time + 172_800));
            },
            expect: Verifier(&[F::PassInvalid]),
            after: None,
        },
        Case {
            id: "DE-01d",
            family: "fabricated pass",
            attack: "the evaluator says 'flat' in the Meta while a position is open and the pass is requested and confirmed",
            row: Some("pass requested and confirmed on an account that is not flat"),
            sc: sc_pass_open,
            tamper: |t| {
                t.set_meta_flat(1, true);
                t.rechain();
                t.pass_flow = true;
            },
            expect: Verifier(&[F::PassInvalid, F::StateMismatch]),
            after: None,
        },
        // ============================================================ 2. omits a breach / a fail
        Case {
            id: "DE-02a",
            family: "omitted breach",
            attack: "the breach batch is replaced by a heartbeat: the trader keeps the position",
            row: Some("breach omitted (the trader keeps the position)"),
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Breach { .. }));
                let meta = t.batches[at.0].batch.events.last().cloned().unwrap();
                t.batches[at.0].batch.events = vec![meta];
                t.batches[at.0].batch.proofs.clear();
                t.batches[at.0].markets.clear();
                for bi in at.0..t.batches.len() {
                    t.set_meta_flat(bi, false);
                }
                t.fix();
            },
            expect: Verifier(&[F::MissedBreach]),
            after: None,
        },
        Case {
            id: "DE-02b",
            family: "omitted fail",
            attack: "the report_fail at the static floor is replaced by a heartbeat",
            row: Some("fail omitted at the static floor"),
            sc: sc_fail,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fail { .. }));
                let meta = t.batches[at.0].batch.events.last().cloned().unwrap();
                t.batches[at.0].batch.events = vec![meta];
                t.batches[at.0].batch.proofs.clear();
                t.batches[at.0].kind = ::evaluator::BatchKind::PostJournal;
                t.batches[at.0].markets.clear();
                t.set_meta_flat(at.0, false);
                t.fix();
            },
            expect: Verifier(&[F::MissedFail]),
            after: None,
        },
        Case {
            id: "DE-02c",
            family: "omitted breach",
            attack: "the sample that caused the breach is dropped from the archive AND the breach omitted, consistently, with no second recorder",
            row: Some("a sample that caused a breach dropped from the archive, breach omitted (no second recorder)"),
            sc: sc_flash,
            tamper: |t| {
                let (doctored, _chain) = honest(&sc_flash_without_the_crash());
                *t = doctored;
            },
            expect: Neither("a sample that caused a breach dropped from the archive, breach omitted (no second recorder)"),
            after: None,
        },
        Case {
            id: "DE-02d",
            family: "omitted breach",
            attack: "the same as DE-02c, but an independent second recorder holds the crash sample",
            row: Some("the same, with an independent second recorder"),
            sc: sc_flash,
            tamper: |t| {
                let original = t.samples.clone();
                let (doctored, _chain) = honest(&sc_flash_without_the_crash());
                *t = doctored;
                t.second = Some(original);
            },
            expect: Verifier(&[F::MissedBreach]),
            after: None,
        },
        // ============================================================ 3. replays an old price inside max_age_secs
        Case {
            id: "DE-03a",
            family: "replayed old price",
            attack: "the breach is posted 30 s late with the old sample (still inside max_age)",
            row: Some("breach posted late with the old sample (replayed price)"),
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Breach { .. }));
                let shift = 30;
                for e in &mut t.batches[at.0].batch.events {
                    match e {
                        JournalEvent::Close { time, .. } | JournalEvent::Fee { time, .. } | JournalEvent::Breach { time, .. } | JournalEvent::Meta { time, .. } => *time += shift,
                        _ => {}
                    }
                }
                let m = t.batches[at.0].meta_time + shift;
                for bi in at.0..t.batches.len() {
                    t.set_meta_time(bi, m);
                }
                t.fix();
            },
            expect: Verifier(&[F::FalseBreach, F::MissedBreach]),
            after: None,
        },
        Case {
            id: "DE-03b",
            family: "replayed old price",
            attack: "a market order is filled at the price of an older sample (20 s old, max_age 60 s) that is worse for the trader than the newest one",
            row: Some("market order executed at an older sample inside max_age (worse price)"),
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                let (qty, nonce, time) = if let JournalEvent::Fill { qty, intent_id, time, .. } = t.ev(at) { (*qty, *intent_id, *time) } else { unreachable!() };
                let old = t.sample_at(0, T_1400);
                let fill = t.hand_fill(old, true, qty, time, nonce, 0);
                t.batches[at.0].batch.events[at.1] = fill;
                t.batches[at.0].batch.proofs = vec![t.proof_of_sample(old)];
                t.fix();
            },
            expect: Neither("market order executed at an older sample inside max_age (worse price)"),
            after: None,
        },
        Case {
            id: "DE-03c",
            family: "replayed old price",
            attack: "the proof is EXACTLY max_age_secs (60 s) older than the fill: the boundary the program still accepts",
            row: None,
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { time, .. } = t.ev(at) {
                    *time += 60;
                }
                let m = t.batches[0].meta_time + 60;
                t.set_meta_time(0, m);
                t.fix();
            },
            expect: Neither("market order executed at an older sample inside max_age (worse price)"),
            after: None,
        },
        Case {
            id: "DE-03d",
            family: "replayed old price",
            attack: "the proof is max_age_secs + 1 (61 s) older than the fill",
            row: Some("proof older than the market's max_age"),
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { time, .. } = t.ev(at) {
                    *time += 61;
                }
                let m = t.batches[0].meta_time + 61;
                t.set_meta_time(0, m);
                t.fix();
            },
            expect: Both(Setl8Error::ProofTooOld, &[F::ProofAge]),
            after: None,
        },
        // ============================================================ 4. mass-fails accounts
        Case {
            id: "DE-04a",
            family: "mass fail",
            attack: "report_fail(StaticFloor) on an honest account far above the floor, with its true balance",
            row: Some("static-floor fail on an account above the floor"),
            sc: sc_quick,
            tamper: |t| append_fail(t, FailReason::StaticFloor, None),
            expect: Both(Setl8Error::FailStaticFloorNotReached, &[F::FalseFail]),
            after: None,
        },
        Case {
            id: "DE-04b",
            family: "mass fail",
            attack: "report_fail(PhaseQuick) on an honest account below the quick limit",
            row: Some("quick-limit fail on an account below the limit"),
            sc: sc_quick,
            tamper: |t| append_fail(t, FailReason::PhaseQuick, None),
            expect: Both(Setl8Error::FailQuickNotExceeded, &[F::FalseFail]),
            after: None,
        },
        Case {
            id: "DE-04c",
            family: "mass fail",
            attack: "report_fail(StaticFloor) with a fabricated balance b_after = 0 on an honest account (the claim is not the account's)",
            row: None,
            sc: sc_quick,
            tamper: |t| append_fail(t, FailReason::StaticFloor, Some(0)),
            expect: Both(Setl8Error::FailBalanceMismatch, &[F::FalseFail, F::FloorMismatch]),
            after: None,
        },
        Case {
            id: "DE-04d",
            family: "mass fail",
            attack: "what the evaluator MUST fabricate to fail an honest account with a position: two real lows of two markets that never existed together, closed in one report_fail",
            row: None, // the Breach version of this story is trust-matrix row "breach built from the worst sample of each market (never simultaneous)" (VERIFIER)
            sc: sc_dips,
            tamper: dips_fail,
            // FINDING TMX-1: the program accepts (every claim is arithmetically true, every proof is a real sample inside max_age) and the
            // verifier says VALID, although the account's equity was never at or below the floor at any instant. For a Breach built the
            // same way the verifier answers FalseBreach (row 31); for a Fail `walk_floor` only reports a MISSED one.
            expect: Gap("TMX-1"),
            after: Some(status_failed),
        },
        // ============================================================ 5. double-fills a pending order
        Case {
            id: "DE-05a",
            family: "double fill",
            attack: "the Fill of a triggered limit order is posted twice in the same batch (same intent, same proof)",
            row: None,
            sc: sc_limit_sl,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. }));
                let f = t.batches[at.0].batch.events[at.1].clone();
                t.batches[at.0].batch.events.insert(at.1 + 1, f);
                t.fix();
            },
            expect: Verifier(&[F::ForgedTrade, F::MarginMismatch, F::StateMismatch, F::TriggerFalse]),
            after: None,
        },
        Case {
            id: "DE-05b",
            family: "double fill",
            attack: "the same limit order is filled a second time in a LATER batch, at a later sample (a new price, the same intent id)",
            row: None,
            sc: sc_limit_sl,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { order_kind: OrderKind::Limit, .. }));
                let (qty, nonce) = if let JournalEvent::Fill { qty, intent_id, .. } = t.batches[at.0].batch.events[at.1] { (qty, intent_id) } else { unreachable!() };
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 5;
                let mut f = t.hand_fill(i, true, qty, time, nonce, 0);
                if let JournalEvent::Fill { order_kind, .. } = &mut f {
                    *order_kind = OrderKind::Limit;
                }
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::PostJournal, vec![f], proofs, time, false, vec![0]);
                t.fix();
            },
            expect: Verifier(&[F::ForgedTrade, F::TriggerFalse]),
            after: None,
        },
        Case {
            id: "DE-05c",
            family: "double fill",
            attack: "a market order (and its commission) is booked twice in the same batch",
            row: None,
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                let f = t.batches[at.0].batch.events[at.1].clone();
                let fee = t.batches[at.0].batch.events[at.1 + 1].clone();
                assert!(matches!(fee, JournalEvent::Fee { .. }));
                t.batches[at.0].batch.events.insert(at.1 + 2, f);
                t.batches[at.0].batch.events.insert(at.1 + 3, fee);
                t.fix();
            },
            expect: Verifier(&[F::ForgedTrade, F::MarginMismatch, F::MissedTightStart, F::StateMismatch]),
            after: None,
        },
        // ============================================================ 6. wrong side of the spread
        Case {
            id: "DE-06a",
            family: "wrong side of the spread",
            attack: "a LONG fill priced at the bid (mid minus half-spread) instead of the ask",
            row: Some("fill price altered by one unit"),
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                let (qty, nonce, time) = if let JournalEvent::Fill { qty, intent_id, time, .. } = t.ev(at) { (*qty, *intent_id, *time) } else { unreachable!() };
                let j = t.sample_at(0, T_1400 + 20);
                let mut f = t.hand_fill(j, false, qty, time, nonce, 0);
                if let JournalEvent::Fill { side, .. } = &mut f {
                    *side = Side::Long;
                }
                t.batches[at.0].batch.events[at.1] = f;
                t.fix();
            },
            expect: Both(Setl8Error::EventPriceMismatch, &[F::PriceMismatch]),
            after: None,
        },
        Case {
            id: "DE-06b",
            family: "wrong side of the spread",
            attack: "the Close of a long is priced at the ask (the better side for the trader would be the bid), pnl and credit recomputed",
            row: Some("close exit price altered (pnl recomputed)"),
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                let time = if let JournalEvent::Close { time, .. } = t.ev(at) { *time } else { unreachable!() };
                let j = t.sample_at(0, time);
                let exit = engine::close_price_e12(t.mid_at(j), t.spread(0), false).unwrap();
                reprice_close(t.ev(at), exit);
                t.fix();
            },
            expect: Both(Setl8Error::EventPriceMismatch, &[F::FeeMismatch, F::PriceMismatch]),
            after: None,
        },
        Case {
            id: "DE-06c",
            family: "wrong side of the spread",
            attack: "the signed order is a BUY, the evaluator books a SELL at the (correct) bid price: every number is right for the side it chose",
            row: None,
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                let (qty, nonce, time) = if let JournalEvent::Fill { qty, intent_id, time, .. } = t.ev(at) { (*qty, *intent_id, *time) } else { unreachable!() };
                let j = t.sample_at(0, T_1400 + 20);
                let f = t.hand_fill(j, false, qty, time, nonce, 0);
                t.batches[at.0].batch.events[at.1] = f;
                t.fix();
            },
            expect: Verifier(&[F::ForgedTrade]),
            after: None,
        },
        // ============================================================ 7. skips a rollover (holding) fee
        Case {
            id: "DE-07a",
            family: "skipped rollover fee",
            attack: "the holding fee at 22:00 UTC is left out",
            row: Some("holding fee omitted at the rollover"),
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
        Case {
            id: "DE-07b",
            family: "skipped rollover fee",
            attack: "the holding fee is booked with amount 0 (the event is there, the money is not)",
            row: None,
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount = 0;
                }
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
        Case {
            id: "DE-07c",
            family: "skipped rollover fee",
            attack: "the holding fee is one micro-USD too low",
            row: Some("holding fee one micro-USD too low"),
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount -= 1;
                }
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
        // ============================================================ 8. applies a fee twice
        Case {
            id: "DE-08a",
            family: "fee applied twice",
            attack: "the holding fee is booked twice",
            row: None,
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                let f = t.batches[at.0].batch.events[at.1].clone();
                t.batches[at.0].batch.events.insert(at.1, f);
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
        Case {
            id: "DE-08b",
            family: "fee applied twice",
            attack: "the commission of a fill is booked twice",
            row: None,
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Commission, .. }));
                let f = t.batches[at.0].batch.events[at.1].clone();
                t.batches[at.0].batch.events.insert(at.1, f);
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
        Case {
            id: "DE-08c",
            family: "fee applied twice",
            attack: "the commission fee is one micro-USD too high",
            row: Some("commission fee one micro-USD too high"),
            sc: sc_quick,
            tamper: |t| {
                let at = last_idx(t, |e| matches!(e, JournalEvent::Fee { .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount += 1;
                }
                t.fix();
            },
            expect: Verifier(&[F::FeeMismatch]),
            after: None,
        },
    ]
}

fn last_idx(t: &T, f: impl Fn(&JournalEvent) -> bool) -> (usize, usize) {
    let mut found = None;
    for (bi, b) in t.batches.iter().enumerate() {
        for (ei, e) in b.batch.events.iter().enumerate() {
            if f(e) {
                found = Some((bi, ei));
            }
        }
    }
    found.expect("event")
}

/// `sc_flash` on a stream from which the crash sample (second +20) was removed.
fn sc_flash() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 10, p4(10_030));
    s.post(T_1400 + 11);
    s.s(0, T_1400 + 20, p4(9_979));
    s.s(0, T_1400 + 30, p4(10_031));
    s.s(0, T_1400 + 40, p4(10_032));
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

fn sc_flash_without_the_crash() -> ::evaluator::testkit::PathSpec {
    let mut spec = sc_flash();
    spec.steps.retain(|st| !matches!(st, Step::Sample { sec, .. } if *sec == T_1400 + 20));
    spec
}

/// A fail the program does not know the fee/limit for: kept for the table.
fn sc_fail() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, 4_500_000_000_000, T_1400));
    s.s(0, T_1400 + 10, p4(10_000));
    s.post(T_1400 + 11);
    s.s(0, T_1400 + 20, p4(9_000));
    s.end(T_1400 + 25);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

case_tests! {
    de_01a_fabricated_pass_inflated_exit_price => "DE-01a",
    de_01b_fabricated_pass_inflated_close_quantity => "DE-01b",
    de_01c_fabricated_pass_forged_log_lines => "DE-01c",
    de_01f_fabricated_pass_lowered_entry_price => "DE-01f",
    de_01d_fabricated_pass_not_flat => "DE-01d",
    de_02a_omitted_breach => "DE-02a",
    de_02b_omitted_fail => "DE-02b",
    de_02c_omitted_breach_sample_dropped_no_second_recorder => "DE-02c",
    de_02d_omitted_breach_sample_dropped_with_second_recorder => "DE-02d",
    de_03a_replayed_price_late_breach => "DE-03a",
    de_03b_replayed_price_older_sample_inside_max_age => "DE-03b",
    de_03c_replayed_price_exactly_max_age => "DE-03c",
    de_03d_replayed_price_one_second_beyond_max_age => "DE-03d",
    de_04a_mass_fail_static_floor_on_honest_account => "DE-04a",
    de_04b_mass_fail_quick_limit_on_honest_account => "DE-04b",
    de_04c_mass_fail_fabricated_balance => "DE-04c",
    de_04d_mass_fail_non_simultaneous_lows_gap_tmx_1 => "DE-04d",
    de_05a_double_fill_limit_same_batch => "DE-05a",
    de_05b_double_fill_limit_later_batch => "DE-05b",
    de_05c_double_fill_market_order_and_commission => "DE-05c",
    de_06a_long_fill_at_the_bid => "DE-06a",
    de_06b_close_at_the_wrong_side => "DE-06b",
    de_06c_opposite_side_to_the_signed_order => "DE-06c",
    de_07a_skipped_holding_fee => "DE-07a",
    de_07b_zero_holding_fee => "DE-07b",
    de_07c_holding_fee_one_micro_low => "DE-07c",
    de_08a_holding_fee_twice => "DE-08a",
    de_08b_commission_twice => "DE-08b",
    de_08c_commission_one_micro_high => "DE-08c",
}

#[test]
fn dishonest_evaluator_table_is_complete() {
    let all = cases();
    assert_eq!(CASE_TESTS.len(), all.len(), "every case has exactly one named test");
    for c in &all {
        assert_eq!(CASE_TESTS.iter().filter(|(_, id)| *id == c.id).count(), 1, "case without exactly one test: {}", c.id);
        assert_eq!(all.iter().filter(|x| x.id == c.id).count(), 1, "duplicate case id {}", c.id);
        assert!(FAMILIES.contains(&c.family), "unknown family {}", c.family);
        assert!(["Program", "Verifier", "Both", "Neither", "Gap"].contains(&c.expect.who()));
        if let Expect::Neither(row) = c.expect {
            assert!(doc_neither_rows().iter().any(|n| n == row), "{}: not a NEITHER row: {row}", c.id);
        }
    }
    // every kind of dishonesty the audit prompt lists is covered at least once
    for f in FAMILIES {
        assert!(all.iter().any(|c| c.family == f), "no case for: {f}");
    }
}

// FINDING TMX-1 (verifier gap, mass fail by a stolen evaluator key): a `report_fail(StaticFloor)` built from the lows of two markets that
// never existed together is accepted by the program (it re-derives only b and the floor from the claimed Closes, whose proofs are real
// samples inside `max_age`) and the verifier answers VALID: `walk_floor` for a Fail only removes the expectation of a MISSED one, and
// `on_fail` justifies the Fail from the replayed closed balance alone. DESIGN 13.2 A-18 and the stolen-key row of 1.3 say this attack is
// "possible and visible"; for a Breach built the same way the verifier does answer FalseBreach (trust-matrix row 31). The Fail is final:
// `flag_trader_failed` has already been called, there is no 2-day window and no veto.
#[test]
#[ignore = "FINDING TMX-1: a Fail built from non-simultaneous lows is accepted by the program and VALID for the verifier"]
fn tmx_1_verifier_must_flag_a_fail_built_from_non_simultaneous_lows() {
    let (mut t, mut chain) = honest(&sc_dips());
    dips_fail(&mut t);
    let r = run(&t, &mut chain);
    assert!(r.rejected.is_none(), "the program accepts it (its limit, A-18)");
    assert!(
        r.classes.contains(&F::FalseFail) || r.classes.contains(&F::FalseBreach),
        "the verifier must flag a Fail whose Closes were never justified by an instant at or below the floor (as it flags the same story as a Breach); it said: {:?}\n{}",
        r.classes,
        r.text
    );
}

// ------------------------------------------------------------------------------------------------- loops over every honest account

/// Mass fail (DE-04, family "mass fail"): every honest Active account of the scenario set gets a `report_fail` of each reason that its
/// state does not justify. Each must be refused with the exact error, leave the whole world byte-identical, and be flagged by the
/// verifier as FalseFail (the same data published as if accepted). 9 accounts x 2 reasons = 18 attacks.
#[test]
fn de_04e_mass_fail_every_honest_account_is_refused_by_the_program_and_flagged_by_the_verifier() {
    let mut n = 0;
    for (name, spec) in active_scenarios() {
        for (reason, want) in [(FailReason::StaticFloor, Setl8Error::FailStaticFloorNotReached), (FailReason::PhaseQuick, Setl8Error::FailQuickNotExceeded)] {
            let (mut t, mut chain) = honest(&spec);
            let honest_batches = t.order.len();
            append_fail(&mut t, reason, None);
            let r = run(&t, &mut chain);
            let (pos, code) = r.rejected.unwrap_or_else(|| panic!("{name}/{reason:?}: the program accepted a wrongful fail"));
            assert_eq!(pos, honest_batches, "{name}/{reason:?}: only the fabricated batch is refused");
            assert_eq!(code, Some(u32::from(want)), "{name}/{reason:?}: wrong error");
            assert!(r.unchanged_after_refusal, "{name}/{reason:?}: the refusal changed the world");
            assert_eq!(chain.j.account().status, 0, "{name}/{reason:?}: the account is still Active");
            // FalseFail always; `pass_open`, `dips` and `stale` still hold a position, so the fabricated `Meta.flat = true` of the fail batch is also a StateMismatch
            let want_classes: Vec<String> = if ["pass_open", "dips", "stale"].contains(&name) { vec!["FalseFail".into(), "StateMismatch".into()] } else { vec!["FalseFail".into()] };
            assert_eq!(names(&r.classes), want_classes, "{name}/{reason:?}: {}", r.text);
            n += 1;
        }
    }
    assert_eq!(n, 18);
}

/// "Fabricates a pass", chain side: the evaluator confirms a pass that nobody requested. Refused with `NoPendingPass`, nothing changes.
#[test]
fn de_01e_confirm_pass_without_a_request_is_refused_and_changes_nothing() {
    let (t, mut chain) = honest(&sc_win());
    for &bi in &t.order {
        chain.try_post(&t.batches[bi], None).unwrap();
    }
    let a = chain.j.account();
    assert!(a.b >= 108_000_000_000, "the account IS above the target: the honest +8.5% story");
    assert_eq!(a.pass_pending, 0);
    let before = chain.j.digest();
    let r = chain.j.confirm(chain.clock + 5);
    common::assert_err(&r, Setl8Error::NoPendingPass);
    assert_eq!(chain.j.digest(), before, "a refused confirm changes nothing");
}
