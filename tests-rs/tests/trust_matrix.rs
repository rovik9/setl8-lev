//! The trust matrix (M10, input of the M12 audit report): tamper classes applied to an otherwise honest evaluator output,
//! and WHO catches each one: the PROGRAM (it rejects the batch, with an error code), the public VERIFIER (a finding class),
//! BOTH, or NEITHER (the honest limit of option C). The table at the bottom is asserted; `handover/trust-matrix.md` is
//! written from it (`TRUST_MATRIX_PRINT=1 cargo test --test trust_matrix -- --nocapture` prints the markdown).
//!
//! Method. An honest history is run through the evaluator (a scripted list of samples and signed orders). Its batches, the
//! archive and the intents are then edited by a "malicious evaluator" that keeps the hash chain consistent (it can always do
//! that: it holds the key). Program verdict: the edited batches are posted, in order, to the real program in LiteSVM until
//! the first rejection. Verifier verdict: the edited events (as if all had been accepted), archive and intents go through
//! `verify_account`. BOTH = the program rejects AND the verifier would also flag the same data.
mod common;

use ::evaluator::codec::{IntentKind, OrderIntent};
use ::evaluator::testkit::{MarketSpec, Scenario, Step};
use ::evaluator::{AccountParams, Batch, RulesCfg};
use common::evaluator::shared::runner::{items_text, run_path, spec_rules, Chain, Item, SimChain};
use common::evaluator::shared::{self, market_order, order, p4, World, E12, HOUR, MEUR, T_1400};
use common::evaluator::*;
use common::{assert_ok, dup};
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent, Side};
use setl8_lev_trading::errors::Setl8Error;
use setl8_verify::archive::{ArchiveWriter, Sample};
use setl8_verify::FindingClass as F;
use solana_keypair::Keypair;
use solana_signer::Signer;

const S100K: i64 = 100_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Program,
    Verifier,
    Both,
    Neither,
}

// ------------------------------------------------------------------------------------------------- scripts

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
    MarketSpec { id, class: 0, symbol: symbol.to_string(), source: 2, start_price: E12, max_age_secs: max_age, spread_e6: spread, sessions }
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

fn close_order(m: u16, t: i64) -> OrderIntent {
    shared::close_order(m, 0, t)
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

/// A gap through the static floor: 4.5M EUR long, price -10%.
fn sc_fail() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, 4_500_000_000_000, T_1400));
    s.s(0, T_1400 + 10, p4(10_000));
    s.post(T_1400 + 11);
    s.s(0, T_1400 + 20, p4(9_000));
    s.end(T_1400 + 25);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Two positions in two markets; each dips alone (never together) so the honest account never breaches.
/// Spread 0. Market 0 = EURUSD-like, market 1 = GBPUSD-like, both 1,000,000 units long from 1.0.
fn sc_two() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let t = T_1400;
    s.s(0, t, p4(10_000)).s(1, t + 1, p4(10_000));
    s.o(market_order(0, true, MEUR, t + 1));
    s.o(market_order(1, true, MEUR, t + 1));
    // both up: P = +2.0k each -> tight on, L = 96k + (4k - 1k) = 99k
    s.s(0, t + 10, p4(10_020)).s(1, t + 11, p4(10_020));
    s.post(t + 12);
    // A dips to -1.5k alone while B stays +2.0k: E = 100.5k
    s.s(0, t + 20, p4(9_985)).s(1, t + 21, p4(10_020));
    s.post(t + 22);
    // A recovers, B dips: E = 100.5k
    s.s(0, t + 30, p4(10_020)).s(1, t + 31, p4(9_985));
    s.post(t + 32);
    s.s(0, t + 40, p4(10_020)).s(1, t + 41, p4(10_020));
    s.end(t + 45);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A flash crash: one sample breaches the tight floor, the next one is back up.
fn sc_flash() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 10, p4(10_030)); // +3.0k: tight, L = 96k + 2k = 98k
    s.post(T_1400 + 11);
    s.s(0, T_1400 + 20, p4(9_980)); // -2.0k: E = 98.0k <= L: breach
    s.s(0, T_1400 + 30, p4(10_031));
    s.s(0, T_1400 + 40, p4(10_032));
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// `sc_flash` on a stream from which the crash sample (second +20) was removed.
fn sc_flash_without_the_crash() -> ::evaluator::testkit::PathSpec {
    let mut spec = sc_flash();
    spec.steps.retain(|st| !matches!(st, Step::Sample { sec, .. } if *sec == T_1400 + 20));
    spec
}

/// A buy at a moment when the price has been falling; the newest sample is lower than the one an evil evaluator would use.
fn sc_stale() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_100));
    s.s(0, T_1400 + 20, p4(10_000));
    s.o(market_order(0, true, MEUR / 2, T_1400 + 20));
    s.s(0, T_1400 + 30, p4(10_000));
    s.end(T_1400 + 35);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A market that closes: sessions Monday 00:00-14:30 only.
fn sc_session() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400 + 29 * 60, p4(10_000)); // 14:29
    s.o(market_order(0, true, MEUR / 10, T_1400 + 29 * 60));
    s.s(0, T_1400 + 29 * 60 + 20, p4(10_000));
    s.end(T_1400 + 29 * 60 + 25);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 14 * 60 + 30)])], 0, 2, s.steps)
}

// ---- M9b scenarios (decisions D1 to D5)

/// D1. A long 100,000 EUR (own batch), then a sell limit of 25,000 at 1.0050 that triggers at +120 s and only reduces (no tight mode:
/// the open profit stays under 1% of S). Spread 0.
fn sc_pend_reduce() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR / 10, T_1400));
    s.o(order(IntentKind::Limit, 0, 1, MEUR / 40, p4(10_050), 0, 0, 0, T_1400));
    s.s(0, T_1400 + 10, p4(10_000));
    s.post(T_1400 + 11);
    s.s(0, T_1400 + 120, p4(10_060));
    s.post(T_1400 + 121);
    s.end(T_1400 + 130);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D1. A long 1 M EUR; a sell limit that no sample reaches (nonce 2); a set_sl_tp (nonce 3); the trader's close (nonce 4).
fn sc_pend_untriggered() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.o(order(IntentKind::Limit, 0, 1, MEUR / 4, p4(10_500), 0, 0, 0, T_1400));
    s.s(0, T_1400 + 10, p4(10_000));
    s.tick(T_1400 + 11);
    s.o(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(9_000), 0, 0, T_1400 + 11));
    s.o(close_order(0, T_1400 + 12));
    s.s(0, T_1400 + 20, p4(10_001));
    s.end(T_1400 + 30);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D2. A long 1 M EUR with a commission of 20 ppm; +5,602 raw in 30 s: the closing slice's credit 2,801 is over 0.028 S.
fn sc_overshoot() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.post(T_1400 + 1);
    s.s(0, T_1400 + 30, 1_005_602_000_000);
    s.o(close_order(0, T_1400 + 30));
    s.s(0, T_1400 + 40, 1_005_602_000_000);
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 20, 2, s.steps)
}

/// D2. A long 400,000 EUR, +0.20% in 20 s (credit 400, no overshoot, no tight mode), then a market sell of 1.2 M: a flip, `Close(Flip)`
/// and a `Fill` of the remaining 800,000.
fn sc_flip() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, 4 * MEUR / 10, T_1400));
    s.s(0, T_1400 + 5, p4(10_000));
    s.s(0, T_1400 + 20, p4(10_020));
    s.post(T_1400 + 21);
    s.tick(T_1400 + 21);
    s.o(market_order(0, false, 12 * MEUR / 10, T_1400 + 21));
    s.s(0, T_1400 + 25, p4(10_020));
    s.end(T_1400 + 30);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D3. A long 4.9 M EUR in market 0 (margin $98,000 of $100,000); a buy limit of 1 M in market 1 (flat) triggers at +21 s and cannot be
/// margined. The last batch is a heartbeat `Meta` dated +21 s, so a Fill dated +21 s can be put before it.
fn sc_margin_short() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let t = T_1400;
    s.s(0, t, p4(10_000)).s(1, t + 1, p4(10_000));
    s.o(market_order(0, true, 4_900 * MEUR / 1000, t + 1));
    s.o(order(IntentKind::Limit, 1, 0, MEUR, p4(9_990), 0, 0, 0, t + 1));
    s.s(0, t + 10, p4(10_000)).s(1, t + 11, p4(10_000));
    s.s(0, t + 20, p4(10_000)).s(1, t + 21, p4(9_985));
    s.end(t + 22);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D4, static floor. B = 92,500 after a realized loss; two positions; A +1,500 (tight mode), B -1,500 (the breach); the closes end at 91,750.
fn sc_conv_static() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    let t0 = T_1400;
    s.s(0, t0, p4(10_000)).s(1, t0 + 1, p4(10_000));
    s.o(market_order(0, true, MEUR, t0 + 1));
    s.s(0, t0 + 100, p4(9_925)).s(1, t0 + 101, p4(10_000));
    s.o(close_order(0, t0 + 101));
    s.s(0, t0 + 110, p4(9_925)).s(1, t0 + 111, p4(10_000));
    s.post(t0 + 112);
    s.s(0, t0 + 113, p4(10_000)).s(1, t0 + 114, p4(10_000));
    s.o(market_order(0, true, MEUR, t0 + 114));
    s.o(market_order(1, true, MEUR, t0 + 114));
    s.s(0, t0 + 123, p4(10_015)).s(1, t0 + 124, p4(10_000));
    s.s(1, t0 + 153, p4(9_985));
    s.end(t0 + 160);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D4, quick limit. A long 1 M EUR, +10.7% in 20 s (tight mode), back to +5.7% (the breach); the quick slice credits 2,850 > 2,800.
fn sc_conv_quick() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.post(T_1400 + 1);
    s.s(0, T_1400 + 20, p4(11_070));
    s.s(0, T_1400 + 40, p4(10_570));
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Two flat markets and nothing else: a place to put a fill the evaluator should not have posted.
fn sc_flat_two() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000)).s(1, T_1400 + 1, p4(10_000));
    s.s(0, T_1400 + 10, p4(10_000)).s(1, T_1400 + 11, p4(10_000));
    s.end(T_1400 + 15);
    base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

// ------------------------------------------------------------------------------------------------- the tampering view

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

struct Outcome {
    /// The first rejected batch (its position in `order`) and the error code, if any.
    rejected: Option<(usize, Option<u32>)>,
    classes: Vec<F>,
    text: String,
}

fn run_case(t: &T, chain: &mut LiteChain) -> Outcome {
    // ---- program
    let mut rejected = None;
    let mut chain_lines: Vec<String> = Vec::new();
    for (pos, &bi) in t.order.iter().enumerate() {
        if t.hold_before == Some(pos) {
            let hash = [9u8; 32];
            chain.j.set_time(chain.clock);
            let ev = dup(&chain.j.v.evaluator);
            let ix = chain.j.hold_ix(ev.pubkey(), hash);
            assert_ok(chain.j.v.e.send(&[ix], &[&ev]));
        }
        match chain.try_post_bytes(&t.batches[bi], t.raw.get(&bi).cloned(), t.signer.map(Keypair::new_from_array).as_ref()) {
            Ok(_) => {}
            Err(e) => {
                rejected = Some((pos, e.code));
                chain_lines.push(format!("# rejected: {}", e.text.lines().next().unwrap_or("")));
                break;
            }
        }
    }
    // the pass flow, when the story calls for it (the trader presses the button, the evaluator confirms)
    if rejected.is_none() && t.pass_flow {
        let w = World::base();
        let at = chain.clock + 5;
        if chain.request_advance(&w, at) {
            chain.confirm_pass(&w, at + 1);
        }
    }
    // ---- verifier, over everything the malicious evaluator published (as if all had been accepted)
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
    let rep = setl8_verify::verify_account(&setl8_verify::Inputs {
        events: events.as_bytes(),
        rules: rules.as_bytes(),
        archive: &archive,
        intents: intents.as_bytes(),
        snapshot: None,
        second_archive: second.as_deref(),
        as_of_seq: None,
    })
    .expect("inputs parse");
    let classes: Vec<F> = rep.classes();
    let text = shared::describe(&rep);
    Outcome { rejected, classes, text }
}

fn who(o: &Outcome) -> Who {
    match (o.rejected.is_some(), o.classes.is_empty()) {
        (true, false) => Who::Both,
        (true, true) => Who::Program,
        (false, false) => Who::Verifier,
        (false, true) => Who::Neither,
    }
}

#[test]
fn dump_scenarios() {
    if std::env::var("TRUST_DUMP").is_err() {
        return;
    }
    for (name, sc) in [("breach", sc_breach()), ("quick", sc_quick()), ("night", sc_night()), ("fail", sc_fail()), ("pass_open", sc_pass_open()), ("limit_sl", sc_limit_sl()), ("two", sc_two()), ("flash", sc_flash()), ("stale", sc_stale()), ("session", sc_session())] {
        let (t, _chain) = honest(&sc);
        println!("== {name}: {} batches", t.batches.len());
        for (i, b) in t.batches.iter().enumerate() {
            let kinds: Vec<String> = b.batch.events.iter().map(|e| ev_name(e).to_string()).collect();
            println!("   #{i} {:?} {:?} proofs {}", b.kind, kinds, b.batch.proofs.len());
        }
    }
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

#[test]
fn honest_outputs_are_accepted_by_both() {
    for (name, sc) in [("breach", sc_breach()), ("quick", sc_quick()), ("night", sc_night()), ("fail", sc_fail()), ("pass_open", sc_pass_open()), ("limit_sl", sc_limit_sl()), ("two", sc_two()), ("flash", sc_flash()), ("stale", sc_stale()), ("session", sc_session())] {
        let (t, mut chain) = honest(&sc);
        let o = run_case(&t, &mut chain);
        assert!(o.rejected.is_none(), "{name}: the program rejected an honest batch: {:?}", o.rejected);
        assert!(o.classes.is_empty(), "{name}: the verifier flagged honest output:\n{}", o.text);
    }
}


// ------------------------------------------------------------------------------------------------- helpers for hand-built events

use journal_types::{OrderKind, PriceProof};

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

fn sc_breach_one_batch() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_800));
    s.o(market_order(0, true, 1_000_000_000_000, T_1400));
    for (dt, p) in [(30, 10_806), (60, 10_813), (90, 10_831), (120, 10_811), (150, 10_780)] {
        s.s(0, T_1400 + dt, p4(p));
    }
    s.end(T_1400 + 160);
    base_spec(vec![eur_spec(0, "EURUSD", 30, 60, vec![(0, 10_080)])], 20, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

// ------------------------------------------------------------------------------------------------- the rows

enum D5 {
    Finalized,
    Advanced(u8, u8),
}

/// Appends the pass lines of a finalization of phase 2 (the story `sc_win` ends flat at +8.5%) as an indexer would write them.
fn d5_lines(t: &mut T, lines: &[D5]) {
    use common::evaluator::shared::runner::{pass_finalized_line, phase_advanced_line};
    let time = T_1400 + 200_000;
    for l in lines {
        t.extra_lines.push(match l {
            D5::Finalized => pass_finalized_line(&t.params.account, 2, 3, 8_000_000_000, 1, time),
            D5::Advanced(from, to) => phase_advanced_line(&t.params.account, *from, *to, time),
        });
    }
}

type Tamper = fn(&mut T);

struct Row {
    name: &'static str,
    sc: fn() -> ::evaluator::testkit::PathSpec,
    tamper: Tamper,
    who: Who,
    program: Option<Setl8Error>,
    verifier: &'static [F],
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

fn rows() -> Vec<Row> {
    use Who::*;
    vec![
        // ---------------------------------------------------------------- arithmetic claims (the program re-checks them)
        Row {
            name: "fill price altered by one unit",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { price_e12, .. } = t.ev(at) {
                    *price_e12 += 1;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::EventPriceMismatch),
            verifier: &[F::PriceMismatch],
        },
        Row {
            name: "close exit price altered (pnl recomputed)",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = t.ev(at) {
                    *exit_e12 += 1;
                    *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
                    *credited = engine::slice_credit(*pnl, *quick).0;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::EventPriceMismatch),
            verifier: &[F::PriceMismatch],
        },
        Row {
            name: "close pnl wrong by one micro-USD",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { pnl, .. } = t.ev(at) {
                    *pnl += 1;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::PnlMismatch),
            verifier: &[F::PnlMismatch],
        },
        Row {
            name: "quick flag flipped on a quick slice",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { quick, .. } = t.ev(at) {
                    *quick = !*quick;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::QuickMismatch),
            verifier: &[F::QuickMismatch],
        },
        Row {
            name: "quick profit credited in full",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { pnl, credited, .. } = t.ev(at) {
                    *credited = *pnl;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::CreditMismatch),
            verifier: &[F::CreditMismatch],
        },
        Row {
            name: "fee amount beyond i64",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount = u64::MAX;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::FeeInvalid),
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "tight start: l_level one micro off",
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::TightStart { .. }));
                if let JournalEvent::TightStart { l_level, .. } = t.ev(at) {
                    *l_level += 1;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::TightStartLLevel),
            verifier: &[F::FloorMismatch],
        },
        Row {
            name: "floor raise: l_level one micro off",
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::FloorRaised { .. }));
                if let JournalEvent::FloorRaised { l_level, .. } = t.ev(at) {
                    *l_level -= 1;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::FloorLLevel),
            verifier: &[F::FloorMismatch],
        },
        Row {
            name: "floor raise dropped (the breach then names a stale level)",
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::FloorRaised { .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.batches[at.0].batch.proofs.clear();
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::BreachLLevel),
            verifier: &[F::FloorMismatch],
        },
        Row {
            name: "tight start and floor raise swapped",
            sc: sc_breach_one_batch,
            tamper: |t| {
                let a = t.find(|e| matches!(e, JournalEvent::TightStart { .. }));
                let b = t.find(|e| matches!(e, JournalEvent::FloorRaised { .. }));
                assert_eq!(a.0, b.0, "both in one batch");
                t.batches[a.0].batch.events.swap(a.1, b.1);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::TightNotOn),
            verifier: &[F::FloorMismatch],
        },
        Row {
            name: "meta: engine version wrong",
            sc: sc_quick,
            tamper: |t| {
                if let Some(JournalEvent::Meta { engine_version, .. }) = t.batches[0].batch.events.last_mut() {
                    *engine_version = 2;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::EngineVersionMismatch),
            verifier: &[F::EngineVersion],
        },
        // ---------------------------------------------------------------- sequence, chain, signer, time, proofs, limits
        Row {
            name: "batch replayed",
            sc: sc_breach,
            tamper: |t| {
                t.order = vec![0, 1, 1, 2, 3, 4];
            },
            who: Both,
            program: Some(Setl8Error::BadSequence),
            verifier: &[F::SequenceGap],
        },
        Row {
            name: "wrong prev_hash",
            sc: sc_breach,
            tamper: |t| {
                t.batches[2].batch.prev_hash = [1; 32];
            },
            who: Program,
            program: Some(Setl8Error::BadPrevHash),
            verifier: &[],
        },
        Row {
            name: "posted by a key that is not the evaluator",
            sc: sc_quick,
            tamper: |t| {
                t.signer = Some([5; 32]);
            },
            who: Program,
            program: Some(Setl8Error::Unauthorized),
            verifier: &[],
        },
        Row {
            name: "event time goes backwards",
            sc: sc_breach,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::TightStart { .. }));
                if let JournalEvent::TightStart { time, .. } = t.ev(at) {
                    *time = T_1400 - 5;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::TimeBackwards),
            verifier: &[F::StateMismatch],
        },
        Row {
            name: "event dated 30 s after the block time",
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { time, .. } = t.ev(at) {
                    *time += 30;
                }
                let m = t.batches[0].meta_time + 30;
                if let Some(JournalEvent::Meta { time, .. }) = t.batches[0].batch.events.last_mut() {
                    *time = m;
                }
                // the block time stays the honest one
                t.fix();
            },
            who: Program,
            program: Some(Setl8Error::TimeInFuture),
            verifier: &[],
        },
        Row {
            name: "proof carries another market's feed",
            sc: sc_quick,
            tamper: |t| {
                t.batches[0].batch.proofs[0].feed_id[0] ^= 1;
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::ProofWrongFeed),
            verifier: &[F::ProofFeed],
        },
        Row {
            name: "proof older than the market's max_age",
            sc: sc_stale,
            tamper: |t| {
                // the fill is dated 61 s after its proof (max_age is 60); the block time follows
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { time, .. } = t.ev(at) {
                    *time += 61;
                }
                let m = t.batches[0].meta_time + 61;
                t.set_meta_time(0, m);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::ProofTooOld),
            verifier: &[F::ProofAge],
        },
        Row {
            name: "batch of nine events",
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                let fee = t.batches[at.0].batch.events[at.1].clone();
                for _ in 0..5 {
                    t.batches[at.0].batch.events.insert(at.1, fee.clone());
                }
                // the encoder refuses nine events: the raw bytes are written by hand
                let b = &t.batches[at.0].batch;
                let events: Vec<Vec<u8>> = b.events.iter().map(|e| e.to_bytes()).collect();
                let proofs: Vec<Vec<u8>> = b.proofs.iter().map(|p| p.to_bytes().unwrap()).collect();
                let mut raw = Vec::new();
                raw.extend_from_slice(&b.first_seq.to_le_bytes());
                raw.extend_from_slice(&b.prev_hash);
                raw.extend_from_slice(&(events.len() as u32).to_le_bytes());
                for e in &events {
                    raw.extend_from_slice(e);
                }
                raw.extend_from_slice(&(proofs.len() as u32).to_le_bytes());
                for p in &proofs {
                    raw.extend_from_slice(p);
                }
                t.raw.insert(at.0, raw);
            },
            who: Both,
            program: Some(Setl8Error::BatchTooLarge),
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "fill in a closed session",
            sc: sc_session,
            tamper: |t| {
                // a fresh sample at 14:31:00 (the market closed at 14:30) justifies a fill dated then
                let sec = T_1400 + 31 * 60;
                let mut s = t.samples[0];
                s.ts_ns = (sec as u64) * 1_000_000_000;
                s.recv_ns = s.ts_ns;
                t.samples.push(s);
                let idx = t.samples.len() - 1;
                let pr = t.proof_of_sample(idx);
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                let (qty, nonce) = if let JournalEvent::Fill { qty, intent_id, .. } = t.ev(at) { (*qty, *intent_id) } else { unreachable!() };
                // replace the honest fill by one dated after the close, still backed by a signed intent of that time
                let intent = market_order(0, true, qty, sec);
                let line = t.trader_sign(nonce + 10, &intent);
                t.intents.push(line);
                let fill = t.hand_fill(idx, true, qty, sec, nonce + 10, 0);
                t.batches[at.0].batch.events[at.1] = fill;
                t.batches[at.0].batch.proofs = vec![pr];
                t.set_meta_time(0, sec);
                t.batches[0].samples_committed = t.samples.len();
                t.batches[0].intents_committed_bytes = t.intents_text().len();
                t.fix();
            },
            who: Program,
            program: Some(Setl8Error::MarketClosed),
            verifier: &[],
        },
        Row {
            name: "fill quantity not a multiple of the step",
            sc: sc_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { qty, .. } = t.ev(at) {
                    *qty += 1;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::QuantityInvalid),
            verifier: &[F::ForgedTrade],
        },
        // ---------------------------------------------------------------- what only the verifier sees (the program cannot)
        Row {
            name: "commission fee missing after a close",
            sc: sc_quick,
            tamper: |t| {
                let at = last_idx(t, |e| matches!(e, JournalEvent::Fee { .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "commission fee one micro-USD too high",
            sc: sc_quick,
            tamper: |t| {
                let at = last_idx(t, |e| matches!(e, JournalEvent::Fee { .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount += 1;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "commission fee placed before its fill",
            sc: sc_quick,
            tamper: |t| {
                t.batches[0].batch.events.swap(0, 1);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "holding fee omitted at the rollover",
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "holding fee one micro-USD too low",
            sc: sc_night,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. }));
                if let JournalEvent::Fee { amount, .. } = t.ev(at) {
                    *amount -= 1;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "Meta.samples_hash is not a prefix of the archive",
            sc: sc_quick,
            tamper: |t| {
                if let Some(JournalEvent::Meta { samples_hash, .. }) = t.batches[0].batch.events.last_mut() {
                    samples_hash[0] ^= 1;
                }
                t.rechain();
            },
            who: Verifier,
            program: None,
            verifier: &[F::SamplesHashMismatch],
        },
        Row {
            name: "Meta.intents_hash is not a prefix of the intents file",
            sc: sc_quick,
            tamper: |t| {
                if let Some(JournalEvent::Meta { intents_hash, .. }) = t.batches[0].batch.events.last_mut() {
                    intents_hash[0] ^= 1;
                }
                t.rechain();
            },
            who: Verifier,
            program: None,
            verifier: &[F::IntentsHashMismatch],
        },
        Row {
            name: "Meta.flat says flat while a position is open",
            sc: sc_pass_open,
            tamper: |t| {
                t.set_meta_flat(1, true);
                t.rechain();
            },
            who: Verifier,
            program: None,
            verifier: &[F::StateMismatch],
        },
        Row {
            name: "breach posted late with the old sample (replayed price)",
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
            who: Verifier,
            program: None,
            verifier: &[F::MissedBreach, F::FalseBreach],
        },
        Row {
            name: "breach built from the worst sample of each market (never simultaneous)",
            sc: sc_two,
            tamper: |t| {
                let a = t.sample_at(0, T_1400 + 20);
                let b = t.sample_at(1, T_1400 + 31);
                let time = T_1400 + 50;
                let entry = 1_000_000_000_000u64;
                let close_a = t.hand_close(a, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 0);
                let close_b = t.hand_close(b, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 1);
                let snap = t.account_after(1);
                let b_after = snap.b - 3_000_000_000;
                let breach = JournalEvent::Breach { time, l_level: snap.l_level, b_after, proof: 0 };
                let proofs = vec![t.proof_of_sample(a), t.proof_of_sample(b)];
                t.append(::evaluator::BatchKind::PostJournal, vec![close_a, close_b, breach], proofs, time, true, vec![0, 1]);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FalseBreach],
        },
        Row {
            name: "breach omitted (the trader keeps the position)",
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
            who: Verifier,
            program: None,
            verifier: &[F::MissedBreach],
        },
        Row {
            name: "fail omitted at the static floor",
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
            who: Verifier,
            program: None,
            verifier: &[F::MissedFail],
        },
        Row {
            name: "static-floor fail on an account above the floor",
            sc: sc_quick,
            tamper: |t| {
                let last = t.batches.len() - 1;
                let snap = t.account_after(last);
                let i = t.samples.len() - 1;
                let time = t.batches[last].meta_time;
                let fail = JournalEvent::Fail { time, reason: FailReason::StaticFloor, b_after: snap.b, proof: 0 };
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::ReportFail(engine::FailReason::StaticFloor), vec![fail], proofs, time, true, vec![0]);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::FailStaticFloorNotReached),
            verifier: &[F::FalseFail],
        },
        Row {
            name: "quick-limit fail on an account below the limit",
            sc: sc_quick,
            tamper: |t| {
                let last = t.batches.len() - 1;
                let snap = t.account_after(last);
                let i = t.samples.len() - 1;
                let time = t.batches[last].meta_time;
                let fail = JournalEvent::Fail { time, reason: FailReason::PhaseQuick, b_after: snap.b, proof: 0 };
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::ReportFail(engine::FailReason::PhaseQuick), vec![fail], proofs, time, true, vec![0]);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::FailQuickNotExceeded),
            verifier: &[F::FalseFail],
        },
        Row {
            name: "pass requested and confirmed on an account that is not flat",
            sc: sc_pass_open,
            tamper: |t| {
                t.set_meta_flat(1, true);
                t.rechain();
                t.pass_flow = true;
            },
            who: Verifier,
            program: None,
            verifier: &[F::PassInvalid, F::StateMismatch],
        },
        Row {
            name: "slow profitable slice booked as quick (half credited)",
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { time, timer_start, quick, pnl, credited, .. } = t.ev(at) {
                    *timer_start = *time - 10;
                    *quick = true;
                    *credited = pnl.div_euclid(2);
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::QuickMismatch],
        },
        Row {
            name: "close names another entry price (pnl consistent with it)",
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = t.ev(at) {
                    *entry_e12 -= 100_000_000;
                    *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
                    *credited = engine::slice_credit(*pnl, *quick).0;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::AvgEntryMismatch],
        },
        Row {
            name: "fill reports a wrong average entry",
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { avg_entry_after, .. } = t.ev(at) {
                    *avg_entry_after += 1;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::AvgEntryMismatch],
        },
        Row {
            name: "fill reports a wrong margin",
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                if let JournalEvent::Fill { margin, .. } = t.ev(at) {
                    *margin += 1;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::MarginMismatch],
        },
        Row {
            name: "trade with no signed intent (forged trade)",
            sc: sc_slow,
            tamper: |t| {
                t.intents.clear();
                let len = setl8_verify::intents::header_line().len();
                for b in &mut t.batches {
                    b.intents_committed_bytes = len;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::ForgedTrade],
        },
        Row {
            name: "intent signed by a key that is not the trader's",
            sc: sc_slow,
            tamper: |t| {
                use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signer};
                let secret = SecretKey::from_bytes(&[0x43; 32]).unwrap();
                let public: PublicKey = (&secret).into();
                let kp = Keypair { secret, public };
                for (n, o, sg) in &mut t.intents {
                    *sg = kp.sign(&::evaluator::codec::signing_message(&t.params.account, *n, o)).to_bytes().to_vec();
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::BadIntentSignature],
        },
        Row {
            name: "archive: the price of the proof's sample altered",
            sc: sc_slow,
            tamper: |t| {
                t.samples[0].value += 1_000_000;
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::ArchiveMissing],
        },
        Row {
            name: "archive: the sample behind a fill dropped",
            sc: sc_slow,
            tamper: |t| {
                t.samples.remove(0);
                for b in &mut t.batches {
                    b.samples_committed = b.samples_committed.saturating_sub(1);
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::ArchiveMissing],
        },
        Row {
            name: "stop-loss triggered but not executed",
            sc: sc_limit_sl,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.batches[at.0].batch.proofs.clear();
                t.batches[at.0].markets.clear();
                t.set_meta_flat(at.0, false);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::TriggerMissed],
        },
        Row {
            name: "limit order triggered but never filled",
            sc: sc_limit_sl,
            tamper: |t| {
                for bi in 0..2 {
                    let meta = t.batches[bi].batch.events.last().cloned().unwrap();
                    t.batches[bi].batch.events = vec![meta];
                    t.batches[bi].batch.proofs.clear();
                    t.batches[bi].markets.clear();
                    t.set_meta_flat(bi, true);
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::TriggerMissed],
        },
        Row {
            name: "stop-loss close with no trigger",
            sc: sc_slow,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { reason, intent_id, .. } = t.ev(at) {
                    *reason = CloseReason::StopLoss;
                    *intent_id = 0;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::TriggerFalse],
        },
        // ---------------------------------------------------------------- state rules only the program can see
        Row {
            name: "fill on a held account",
            sc: sc_quick,
            tamper: |t| {
                t.hold_before = Some(0);
            },
            who: Program,
            program: Some(Setl8Error::AccountHeld),
            verifier: &[],
        },
        Row {
            name: "fill during the 12 h lock after a breach",
            sc: sc_breach,
            tamper: |t| {
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 20;
                let qty = 10_000_000_000u64;
                let intent = market_order(0, true, qty, time);
                let line = t.trader_sign(99, &intent);
                t.intents.push(line);
                let fill = t.hand_fill(i, true, qty, time, 99, 0);
                let price = if let JournalEvent::Fill { price_e12, .. } = &fill { *price_e12 } else { unreachable!() };
                let fee = engine::fee_ceil_micro(engine::margin_micro(qty, price, 1).unwrap(), 20).unwrap();
                let fee = JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: u64::try_from(fee).unwrap(), time };
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill, fee], proofs, time, false, vec![0]);
                t.fix();
            },
            who: Program,
            program: Some(Setl8Error::AccountLocked),
            verifier: &[],
        },
        Row {
            name: "fill on an account that already failed",
            sc: sc_fail,
            tamper: |t| {
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 5;
                let qty = 10_000_000_000u64;
                let intent = market_order(0, true, qty, time);
                let line = t.trader_sign(99, &intent);
                t.intents.push(line);
                let fill = t.hand_fill(i, true, qty, time, 99, 0);
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill], proofs, time, false, vec![0]);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::AccountNotActive),
            verifier: &[F::StateMismatch],
        },
        // ---------------------------------------------------------------- the honest limits of option C
        Row {
            name: "a sample that caused a breach dropped from the archive, breach omitted (no second recorder)",
            sc: sc_flash,
            tamper: |t| {
                // the malicious evaluator runs its books on a stream without the crash sample: a consistent record
                let (doctored, _chain) = honest(&sc_flash_without_the_crash());
                *t = doctored;
            },
            who: Neither,
            program: None,
            verifier: &[],
        },
        Row {
            name: "the same, with an independent second recorder",
            sc: sc_flash,
            tamper: |t| {
                let original = t.samples.clone();
                let (doctored, _chain) = honest(&sc_flash_without_the_crash());
                *t = doctored;
                t.second = Some(original);
            },
            who: Verifier,
            program: None,
            verifier: &[F::MissedBreach],
        },
        Row {
            name: "market order executed at an older sample inside max_age (worse price)",
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
            who: Neither,
            program: None,
            verifier: &[],
        },
        Row {
            name: "a trader's market order ignored (no fill, intent on file)",
            sc: sc_stale,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.batches[at.0].batch.proofs.clear();
                t.batches[at.0].markets.clear();
                t.set_meta_flat(at.0, true);
                t.fix();
            },
            who: Neither,
            program: None,
            verifier: &[],
        },

        // ---------------------------------------------------------------- M9b: the relaxed expectations (decisions D1 to D5)
        Row {
            name: "pending order that only reduces: the Close that executes it is missing",
            sc: sc_pend_reduce,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                t.batches[at.0].batch.events.remove(at.1);
                t.batches[at.0].batch.proofs.clear();
                t.batches[at.0].markets.clear();
                t.set_meta_flat(at.0, false);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::TriggerMissed],
        },
        Row {
            name: "pending order that only reduces: its Close at a price altered by one unit",
            sc: sc_pend_reduce,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { side, qty, entry_e12, exit_e12, pnl, credited, quick, .. } = t.ev(at) {
                    *exit_e12 += 1;
                    *pnl = engine::pnl_micro(*side == Side::Long, *qty, *entry_e12, *exit_e12).unwrap();
                    *credited = engine::slice_credit(*pnl, *quick).0;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::EventPriceMismatch),
            verifier: &[F::PriceMismatch],
        },
        Row {
            name: "Close under the nonce of a pending order that no sample triggers",
            sc: sc_pend_untriggered,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { intent_id, .. } = t.ev(at) {
                    *intent_id = 2;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::TriggerFalse],
        },
        Row {
            name: "Close under the nonce of a set_sl_tp",
            sc: sc_pend_untriggered,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                if let JournalEvent::Close { intent_id, .. } = t.ev(at) {
                    *intent_id = 3;
                }
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::ForgedTrade],
        },
        Row {
            name: "commission fee posted after the slice that overshoots the quick limit",
            sc: sc_overshoot,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
                let JournalEvent::Close { qty, exit_e12, time, .. } = t.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
                let fee = engine::fee_ceil_micro(engine::margin_micro(qty, exit_e12, 1).unwrap(), 20).unwrap();
                let fee_ev = JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: u64::try_from(fee).unwrap(), time };
                t.batches[at.0].batch.events.insert(at.1 + 1, fee_ev);
                t.batches[at.0].markets = vec![0];
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::QuickLimitExceeded),
            verifier: &[F::FeeMismatch],
        },
        Row {
            name: "flip that drops its opening remainder although its closing slice did not overshoot",
            sc: sc_flip,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fill { qty, .. } if *qty == 8 * MEUR / 10));
                t.batches[at.0].batch.events.remove(at.1);
                t.set_meta_flat(at.0, true);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::ForgedTrade],
        },
        Row {
            name: "pending order the account could not margin is filled anyway",
            sc: sc_margin_short,
            tamper: |t| {
                let i = t.sample_at(1, T_1400 + 21);
                let time = T_1400 + 21;
                let mut fill = t.hand_fill(i, true, MEUR, time, 2, 0);
                if let JournalEvent::Fill { order_kind, .. } = &mut fill {
                    *order_kind = ::journal_types::OrderKind::Limit;
                }
                let proofs = vec![t.proof_of_sample(i)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill], proofs, time, false, vec![1]);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::PendingFillWithoutMargin],
        },
        Row {
            name: "breach posted where its closes end at or below the static floor",
            sc: sc_conv_static,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fail { .. }));
                let JournalEvent::Fail { time, b_after, proof, .. } = t.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
                let l = t.account_after(at.0 - 1).l_level;
                t.batches[at.0].batch.events[at.1] = JournalEvent::Breach { time, l_level: l, b_after, proof };
                t.batches[at.0].kind = ::evaluator::BatchKind::PostJournal;
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::BreachBelowStaticFloor),
            verifier: &[F::FalseBreach],
        },
        Row {
            name: "Fail omitted (a TightEnd instead) where a breach's closes end at or below the static floor",
            sc: sc_conv_static,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fail { .. }));
                let JournalEvent::Fail { time, .. } = t.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
                t.batches[at.0].batch.events[at.1] = JournalEvent::TightEnd { time };
                t.batches[at.0].kind = ::evaluator::BatchKind::PostJournal;
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::MustReportFail),
            verifier: &[F::MissedFail],
        },
        Row {
            name: "breach posted where its closes overshoot the quick limit",
            sc: sc_conv_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Fail { .. }));
                let JournalEvent::Fail { time, b_after, proof, .. } = t.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
                let l = t.account_after(at.0 - 1).l_level;
                t.batches[at.0].batch.events[at.1] = JournalEvent::Breach { time, l_level: l, b_after, proof };
                t.batches[at.0].kind = ::evaluator::BatchKind::PostJournal;
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::QuickLimitExceeded),
            verifier: &[F::FalseBreach],
        },
        Row {
            name: "phase_advanced to another phase after pass_finalized",
            sc: sc_win,
            tamper: |t| {
                d5_lines(t, &[D5::Finalized, D5::Advanced(2, 4)]);
            },
            who: Verifier,
            program: None,
            verifier: &[F::StateMismatch],
        },
        Row {
            name: "phase_advanced repeated after pass_finalized",
            sc: sc_win,
            tamper: |t| {
                d5_lines(t, &[D5::Finalized, D5::Advanced(2, 3), D5::Advanced(2, 3)]);
            },
            who: Verifier,
            program: None,
            verifier: &[F::StateMismatch],
        },
        Row {
            name: "phase_advanced with no pass line before it",
            sc: sc_win,
            tamper: |t| {
                d5_lines(t, &[D5::Advanced(2, 3)]);
            },
            who: Verifier,
            program: None,
            verifier: &[F::StateMismatch],
        },
        // ---------------------------------------------------------------- the honest limit M9b made visible
        Row {
            name: "a market fill the account cannot margin",
            sc: sc_flat_two,
            tamper: |t| {
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 20;
                // 6 M EUR needs $120,000 of margin on an account of $100,000: nothing on the chain or in the verifier says no
                let qty = 6 * MEUR;
                let intent = market_order(0, true, qty, time);
                let line = t.trader_sign(99, &intent);
                t.intents.push(line);
                let j = t.sample_at(0, T_1400 + 10);
                let fill = t.hand_fill(j, true, qty, time, 99, 0);
                let proofs = vec![t.proof_of_sample(j)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill], proofs, time, false, vec![0]);
                t.fix();
            },
            who: Neither,
            program: None,
            verifier: &[],
        },
    ]
}

#[test]
fn trust_matrix() {
    let mut table = Vec::new();
    let mut bad = Vec::new();
    for row in rows() {
        if std::env::var("TRUST_MATRIX_TRACE").is_ok() {
            eprintln!("row: {}", row.name);
        }
        let (mut t, mut chain) = honest(&(row.sc)());
        (row.tamper)(&mut t);
        let o = run_case(&t, &mut chain);
        let got = who(&o);
        let prog = o.rejected.map(|(_, c)| c);
        let want_code = row.program.map(u32::from);
        let classes: Vec<String> = o.classes.iter().map(|c| format!("{c:?}")).collect();
        table.push(format!("| {} | {:?} | {} | {} |", row.name, got, row.program.map(|e| format!("{e:?} ({})", u32::from(e))).unwrap_or_else(|| "-".into()), if row.verifier.is_empty() { "-".to_string() } else { format!("{:?}", row.verifier) }));
        let mut problems = Vec::new();
        if got != row.who {
            problems.push(format!("who: expected {:?}, got {:?}", row.who, got));
        }
        if row.program.is_some() && prog != Some(want_code) {
            problems.push(format!("program: expected {:?}, got {:?}", want_code, prog));
        }
        for c in row.verifier {
            if !o.classes.contains(c) {
                problems.push(format!("verifier: expected {c:?} among {classes:?}"));
            }
        }
        if !problems.is_empty() {
            bad.push(format!("{}: {}\n   verifier says:\n{}", row.name, problems.join("; "), o.text));
        }
    }
    if std::env::var("TRUST_MATRIX_PRINT").is_ok() {
        println!("{}", table.join("\n"));
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n\n"));
}

// ------------------------------------------------------------------------------------------------- chain-level rows (the pass flow)

struct ChainRow {
    name: &'static str,
    sc: fn() -> ::evaluator::testkit::PathSpec,
    run: fn(&mut T, &mut LiteChain) -> Option<u32>,
    program: Setl8Error,
}

fn code_of(r: litesvm::types::TransactionResult) -> Option<u32> {
    use anchor_lang::solana_program::instruction::error::InstructionError;
    use solana_transaction_error::TransactionError;
    match r {
        Ok(_) => None,
        Err(f) => match f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => Some(c),
            _ => None,
        },
    }
}

fn post_honest(t: &T, chain: &mut LiteChain) {
    for &bi in &t.order {
        chain.try_post(&t.batches[bi], None).unwrap_or_else(|e| panic!("an honest batch was rejected: {}", e.text));
    }
}

fn chain_rows() -> Vec<ChainRow> {
    vec![
        ChainRow {
            name: "confirm_pass with no pass requested",
            sc: sc_win,
            run: |t, chain| {
                post_honest(t, chain);
                chain.j.confirm(chain.clock + 5).err().and_then(|f| code_of(Err(f)))
            },
            program: Setl8Error::NoPendingPass,
        },
        ChainRow {
            name: "post_journal while a pass is confirmed (the journal is frozen)",
            sc: sc_win,
            run: |t, chain| {
                post_honest(t, chain);
                let w = World::base();
                assert!(chain.request_advance(&w, chain.clock + 5));
                chain.confirm_pass(&w, chain.clock + 1);
                // a heartbeat batch
                let last = t.batches.len() - 1;
                let time = chain.clock + 2;
                t.append(::evaluator::BatchKind::PostJournal, vec![], vec![], time, true, vec![]);
                t.fix();
                let _ = last;
                let b = t.batches.last().unwrap().clone();
                chain.try_post(&b, None).err().and_then(|e| e.code)
            },
            program: Setl8Error::PassPending,
        },
        ChainRow {
            name: "request_advance below the target (b < 1.08 S)",
            sc: sc_slow,
            run: |t, chain| {
                post_honest(t, chain);
                chain.j.request(chain.clock + 5).err().and_then(|f| code_of(Err(f)))
            },
            program: Setl8Error::TargetNotReached,
        },
        ChainRow {
            name: "finalize_pass before the delay has elapsed",
            sc: sc_win,
            run: |t, chain| {
                post_honest(t, chain);
                let w = World::base();
                assert!(chain.request_advance(&w, chain.clock + 5));
                chain.confirm_pass(&w, chain.clock + 1);
                let payer = chain.j.new_payer();
                chain.j.finalize(chain.clock + 60, &payer).err().and_then(|f| code_of(Err(f)))
            },
            program: Setl8Error::PassDelayNotElapsed,
        },
    ]
}

#[test]
fn trust_matrix_chain_rows() {
    for row in chain_rows() {
        let (mut t, mut chain) = honest(&(row.sc)());
        let got = (row.run)(&mut t, &mut chain);
        assert_eq!(got, Some(u32::from(row.program)), "{}", row.name);
    }
}

/// The table is documented: every NEITHER row appears under "Residual trust" in handover/trust-matrix.md, and the matrix
/// has at least thirty classes.
#[test]
fn trust_matrix_is_documented() {
    let all = rows();
    assert!(all.len() + chain_rows().len() >= 30, "at least 30 tamper classes");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/trust-matrix.md");
    let Ok(doc) = std::fs::read_to_string(&path) else { return };
    let residual = doc.split("## Residual trust").nth(1).expect("a Residual trust section");
    for r in all.iter().filter(|r| r.who == Who::Neither) {
        assert!(residual.contains(r.name), "NEITHER row not listed under Residual trust: {}", r.name);
    }
    // and every row is in the table
    for r in &all {
        assert!(doc.contains(r.name), "row missing from the document: {}", r.name);
    }
}

// ------------------------------------------------------------------------------------------------- findings against the real program
// (the evaluator-side halves are in crates/evaluator/tests/findings.rs; see handover/module-10.md, "Findings")

fn post_all_ok(t: &T, chain: &mut LiteChain) {
    for &bi in &t.order {
        chain.try_post(&t.batches[bi], None).unwrap_or_else(|e| panic!("an honest batch was rejected: {}", e.text));
    }
}

/// FINDING 2, program side: with a commission, `[Close, Fee, Fail]` is refused after a quick overshoot (I-8), so the evaluator
/// cannot have both the fee the verifier expects and a batch the program accepts.
#[test]
fn finding_2_the_program_refuses_a_fee_between_the_overshooting_close_and_the_fail() {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.post(T_1400 + 1);
    s.s(0, T_1400 + 30, 1_005_602_000_000);
    s.o(close_order(0, T_1400 + 30));
    s.s(0, T_1400 + 40, 1_005_602_000_000);
    s.end(T_1400 + 45);
    let spec = base_spec(vec![eur_spec(0, "EURUSD", 0, 60, vec![(0, 10_080)])], 20, 2, s.steps);
    let (mut t, mut chain) = honest(&spec);
    // the evaluator's own history: Fill, Fee | Close, Fail: accepted by the program
    let last = t.batches.len() - 1;
    assert!(matches!(t.batches[last].kind, ::evaluator::BatchKind::ReportFail(engine::FailReason::PhaseQuick)));
    // the variant the verifier wants: a commission Fee between the Close and the Fail
    let at = t.find(|e| matches!(e, JournalEvent::Close { .. }));
    let JournalEvent::Close { qty, exit_e12, .. } = t.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
    let fee = engine::fee_ceil_micro(engine::margin_micro(qty, exit_e12, 1).unwrap(), 20).unwrap();
    let fee_ev = JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: u64::try_from(fee).unwrap(), time: T_1400 + 30 };
    t.batches[at.0].batch.events.insert(at.1 + 1, fee_ev);
    t.batches[at.0].markets = vec![0];
    t.fix();
    for &bi in &t.order[..at.0] {
        chain.try_post(&t.batches[bi], None).unwrap();
    }
    let e = chain.try_post(&t.batches[at.0], None).expect_err("the program must refuse a Fee after the overshoot");
    assert_eq!(e.code, Some(u32::from(Setl8Error::QuickLimitExceeded)), "{}", e.text);
    // and since M9b (decision D2) the verifier flags that variant too: the overshooting slice carries no fee
    let o = run_case(&t, &mut LiteChain::new(&spec));
    assert!(o.classes.contains(&F::FeeMismatch), "the verifier must flag a fee after the overshoot: {}", o.text);
}

/// FINDING 4, program side: `Breach` with `b_after` at or below the static floor is refused (I-10); the same sample as a
/// `report_fail(StaticFloor)` is accepted.
#[test]
fn finding_4_the_program_wants_a_fail_when_the_breach_closes_end_below_the_static_floor() {
    let mut s = Script::new();
    let t0 = T_1400;
    s.s(0, t0, p4(10_000)).s(1, t0 + 1, p4(10_000));
    s.o(market_order(0, true, MEUR, t0 + 1));
    s.s(0, t0 + 100, p4(9_925)).s(1, t0 + 101, p4(10_000));
    s.o(close_order(0, t0 + 101));
    s.s(0, t0 + 110, p4(9_925)).s(1, t0 + 111, p4(10_000));
    s.post(t0 + 112);
    s.s(0, t0 + 113, p4(10_000)).s(1, t0 + 114, p4(10_000));
    s.o(market_order(0, true, MEUR, t0 + 114));
    s.o(market_order(1, true, MEUR, t0 + 114));
    s.s(0, t0 + 123, p4(10_015)).s(1, t0 + 124, p4(10_000));
    s.s(1, t0 + 153, p4(9_985));
    s.end(t0 + 160);
    let spec = base_spec(vec![eur_spec(0, "EURUSD", 0, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps);
    let (t, mut chain) = honest(&spec);
    let last = t.batches.len() - 1;
    assert!(matches!(t.batches[last].kind, ::evaluator::BatchKind::ReportFail(engine::FailReason::StaticFloor)));
    post_all_ok(&t, &mut chain);
    assert_eq!(chain.j.account().status, 1);
    // the Breach variant: the same closes, then a Breach instead of the Fail
    let (mut t2, mut chain2) = honest(&spec);
    let at = t2.find(|e| matches!(e, JournalEvent::Fail { .. }));
    let JournalEvent::Fail { time, b_after, proof, .. } = t2.batches[at.0].batch.events[at.1].clone() else { unreachable!() };
    let l = t2.account_after(at.0 - 1).l_level;
    t2.batches[at.0].batch.events[at.1] = JournalEvent::Breach { time, l_level: l, b_after, proof };
    t2.batches[at.0].kind = ::evaluator::BatchKind::PostJournal;
    t2.fix();
    for &bi in &t2.order[..at.0] {
        chain2.try_post(&t2.batches[bi], None).unwrap();
    }
    let e = chain2.try_post(&t2.batches[at.0], None).expect_err("the program must refuse the Breach");
    assert_eq!(e.code, Some(u32::from(Setl8Error::BreachBelowStaticFloor)), "{}", e.text);
}

/// FINDING 5, resolved by M9b decision D5. The program emits `PassFinalized` and then `PhaseAdvanced` for one finalization; the
/// verifier takes the `phase_advanced` that directly follows a `pass_finalized` with the same new phase as its confirmation.
#[test]
fn finding_5_phase_advanced_after_pass_finalized_is_an_idempotent_confirmation() {
    let (t, mut chain) = honest(&sc_win());
    post_all_ok(&t, &mut chain);
    let w = World::base();
    assert!(chain.request_advance(&w, chain.clock + 5));
    chain.confirm_pass(&w, chain.clock + 1);
    let tf = chain.clock + 172_800 + 5;
    let at = chain.j.account();
    let _ = at;
    chain.finalize_pass(&w, tf);
    // the lines as `LiteChain` writes them from the program's own events: `pass_finalized`, then `phase_advanced` of the same
    // transaction (M9b decision D5): VALID
    let mut items: Vec<Item> = t.order.iter().map(|&i| Item::Batch(t.batches[i].clone())).collect();
    for l in chain.lines.iter() {
        items.push(Item::Line(l.clone()));
    }
    assert!(chain.lines.iter().any(|l| l.contains("\"phase_advanced\"")) && chain.lines.iter().any(|l| l.contains("\"pass_finalized\"")));
    let ok = verify_events(&t, &items_text(&t.params, &items));
    assert!(ok.is_empty(), "{ok:?}");
    // an indexer that also mirrors the event a second time, or one to another phase, is not content
    let line = |from, to| Item::Line(common::evaluator::shared::runner::phase_advanced_line(&t.params.account, from, to, tf));
    items.push(line(2, 3));
    let bad = verify_events(&t, &items_text(&t.params, &items));
    assert!(bad.contains(&F::StateMismatch), "{bad:?}");
}

fn verify_events(t: &T, events: &str) -> Vec<F> {
    let rules = shared::rules_json(&t.rules, t.params.size_micro);
    let archive = t.archive_bytes();
    let intents = t.intents_text();
    setl8_verify::verify_account(&setl8_verify::Inputs { events: events.as_bytes(), rules: rules.as_bytes(), archive: &archive, intents: intents.as_bytes(), snapshot: None, second_archive: None, as_of_seq: None })
        .expect("inputs")
        .classes()
}
