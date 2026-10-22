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
use common::journal::TEST_HALF_SPREAD_E6;
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A flash crash: one sample breaches the tight floor, the next one is back up.
fn sc_flash() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.s(0, T_1400 + 10, p4(10_030)); // +3.0k: tight, L = 96k + 2k = 98k
    s.post(T_1400 + 11);
    // with spread 0 the crash was -2.0k and touched L exactly; the two half-spreads of 1 e6 move both sides by about 1 and 2 USD, so the
    // crash goes one tick further: -2.1k, E = 97.9k <= L = 97,997.997: breach
    s.s(0, T_1400 + 20, p4(9_979));
    s.s(0, T_1400 + 30, p4(10_031));
    s.s(0, T_1400 + 40, p4(10_032));
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// A market that closes: sessions Monday 00:00-14:30 only.
fn sc_session() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400 + 29 * 60, p4(10_000)); // 14:29
    s.o(market_order(0, true, MEUR / 10, T_1400 + 29 * 60));
    s.s(0, T_1400 + 29 * 60 + 20, p4(10_000));
    s.end(T_1400 + 29 * 60 + 25);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 14 * 60 + 30)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// D2. A long 1 M EUR with a commission of 20 ppm; +5,603 raw in 30 s; after the half-spread of 1 e6 on both fills the pnl is 5,600.994397 and the closing slice's credit 2,800.497198 is over 0.028 S.
fn sc_overshoot() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400));
    s.post(T_1400 + 1);
    s.s(0, T_1400 + 30, 1_005_603_000_000);
    s.o(close_order(0, T_1400 + 30));
    s.s(0, T_1400 + 40, 1_005_603_000_000);
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 20, 2, s.steps)
}

/// F-16 (decision 69). Two long 1 M EUR positions (markets 0 and 1), each +5,603 raw in 30 s, so each quick credit alone is over 0.028 S:
/// the trader closes market 0, which takes the quick tally over the limit; market 1 is closed with it, as `Close{Fail}`, then `Fail`.
fn sc_two_quick() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000)).s(1, T_1400 + 1, p4(10_000));
    s.o(market_order(0, true, MEUR, T_1400 + 1));
    s.o(market_order(1, true, MEUR, T_1400 + 1));
    s.post(T_1400 + 2);
    s.s(0, T_1400 + 30, 1_005_603_000_000).s(1, T_1400 + 31, 1_005_603_000_000);
    s.o(close_order(0, T_1400 + 31));
    s.s(0, T_1400 + 40, 1_005_603_000_000).s(1, T_1400 + 41, 1_005_603_000_000);
    s.end(T_1400 + 45);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 20, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
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
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Two flat markets and nothing else: a place to put a fill the evaluator should not have posted.
fn sc_flat_two() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000)).s(1, T_1400 + 1, p4(10_000));
    s.s(0, T_1400 + 10, p4(10_000)).s(1, T_1400 + 11, p4(10_000));
    s.end(T_1400 + 15);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Like `sc_flat_two`, but market 0 caps a position at $5,000 of notional (the program has no such rule).
fn sc_flat_capped() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000)).s(1, T_1400 + 1, p4(10_000));
    s.s(0, T_1400 + 10, p4(10_000)).s(1, T_1400 + 11, p4(10_000));
    s.end(T_1400 + 15);
    let mut m0 = eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]);
    m0.max_notional_micro = 5_000_000_000;
    base_spec(vec![m0, eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps)
}

/// Market 0 priced, market 1 NOT priced (the rule set has no half-spread for it, decision 64: a market added after the account's
/// rule set). Nothing is traded; the evaluator refuses every order that would open exposure in market 1.
fn sc_flat_unpriced() -> ::evaluator::testkit::PathSpec {
    let mut s = Script::new();
    s.s(0, T_1400, p4(10_000)).s(1, T_1400 + 1, p4(10_000));
    s.s(0, T_1400 + 10, p4(10_000)).s(1, T_1400 + 11, p4(10_000));
    s.end(T_1400 + 15);
    base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", 0, 900, vec![(0, 10_080)])], 0, 2, s.steps)
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
    // the `account_held` line of a hold the chain saw (M9c: the log carries it, dated with the block time)
    let mut hold_line: Option<(usize, String)> = None;
    for (pos, &bi) in t.order.iter().enumerate() {
        if t.hold_before == Some(pos) {
            let hash = [9u8; 32];
            chain.j.set_time(chain.clock);
            let ev = dup(&chain.j.v.evaluator);
            let ix = chain.j.hold_ix(ev.pubkey(), hash);
            assert_ok(chain.j.v.e.send(&[ix], &[&ev]));
            hold_line = Some((pos, ::setl8_verify::log::hold_line(&t.params.account, chain.clock, true)));
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
    let mut items: Vec<Item> = Vec::new();
    for (pos, &i) in t.order.iter().enumerate() {
        if let Some((at, line)) = &hold_line {
            if *at == pos {
                items.push(Item::Line(line.clone()));
            }
        }
        items.push(Item::Batch(t.batches[i].clone()));
    }
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

/// A limit buy in market 1 (unpriced) that the trader signed (client time +5) and that the +11 sample triggers. With `fill`, the
/// evaluator publishes its fill before the last heartbeat; without, it publishes nothing.
fn pending_unpriced(t: &mut T, fill: bool) {
    let qty = MEUR / 100;
    let intent = order(IntentKind::Limit, 1, 0, qty, p4(10_010), 0, 0, 0, T_1400 + 5);
    let line = t.trader_sign(99, &intent);
    t.intents.push(line);
    if !fill {
        let bi = t.batches.len() - 1;
        t.batches[bi].intents_committed_bytes = t.intents_text().len();
        t.fix();
        return;
    }
    let j = t.sample_at(1, T_1400 + 11);
    let mut ev = t.hand_fill(j, true, qty, T_1400 + 11, 99, 0);
    if let JournalEvent::Fill { order_kind, .. } = &mut ev {
        *order_kind = OrderKind::Limit;
    }
    let proof = t.proof_of_sample(j);
    let bi = t.batches.len() - 1;
    t.batches[bi].batch.events.insert(0, ev);
    t.batches[bi].batch.proofs.push(proof);
    t.batches[bi].markets.push(1);
    t.batches[bi].intents_committed_bytes = t.intents_text().len();
    t.set_meta_flat(bi, false);
    t.fix();
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
            who: Both,
            program: Some(Setl8Error::TimeInFuture),
            verifier: &[F::PriceSampleMismatch],
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
            who: Both,
            program: Some(Setl8Error::MarketClosed),
            verifier: &[F::FillNotAllowed],
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
            verifier: &[F::ForgedTrade, F::FillNotAllowed],
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
                // the fills of sc_two are long at ask(1.0, 1 e6) = ceil(1e12 x (1 + 1e-6)) = 1_000_001_000_000
                let entry = 1_000_001_000_000u64;
                let close_a = t.hand_close(a, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 0);
                let close_b = t.hand_close(b, true, MEUR, entry, time, T_1400 + 1, CloseReason::Breach, 1);
                let snap = t.account_after(1);
                // the dishonest breach must be internally consistent: its balance is the stored balance plus what its own two closes credit
                // (each pnl is 1_000_000 micro-USD below the spread-0 value because the entry is the ask 1_000_001_000_000)
                let credited = |e: &JournalEvent| if let JournalEvent::Close { credited, .. } = e { *credited } else { unreachable!() };
                let b_after = snap.b + credited(&close_a) + credited(&close_b);
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
            who: Both,
            program: Some(Setl8Error::AccountHeld),
            verifier: &[F::FillNotAllowed],
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
            who: Both,
            program: Some(Setl8Error::AccountLocked),
            verifier: &[F::FillNotAllowed],
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
            who: Verifier,
            program: None,
            verifier: &[F::PriceSampleMismatch],
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
            name: "forced close after the quick-limit overshoot relabelled as the trader's own close (reason Manual)",
            sc: sc_two_quick,
            tamper: |t| {
                let at = t.find(|e| matches!(e, JournalEvent::Close { reason: ::journal_types::CloseReason::Fail, .. }));
                if let JournalEvent::Close { reason, .. } = t.ev(at) {
                    *reason = ::journal_types::CloseReason::Manual;
                }
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::QuickLimitExceeded),
            verifier: &[F::ForgedTrade],
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
        // ---------------------------------------------------------------- the honest limit M9b made visible, closed by M9c (B1)
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
            who: Verifier,
            program: None,
            verifier: &[F::FillWithoutMargin],
        },
        Row {
            name: "a market fill above the market's notional cap",
            sc: sc_flat_capped,
            tamper: |t| {
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 20;
                // 10,000 EUR is $10,000 of notional on a market that caps a position at $5,000; the margin ($200) is no problem
                let qty = 10_000 * 1_000_000u64;
                let intent = market_order(0, true, qty, time);
                let line = t.trader_sign(99, &intent);
                t.intents.push(line);
                let j = t.sample_at(0, T_1400 + 10);
                let fill = t.hand_fill(j, true, qty, time, 99, 0);
                let proofs = vec![t.proof_of_sample(j)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill], proofs, time, false, vec![0]);
                t.fix();
            },
            who: Verifier,
            program: None,
            verifier: &[F::FillNotAllowed],
        },
        // ---------------------------------------------------------------- decision 64 (P21): unpriced markets are untradable
        Row {
            name: "a market fill in a market the account's rule set does not price",
            sc: sc_flat_unpriced,
            tamper: |t| {
                let i = t.samples.len() - 1;
                let time = t.samples[i].ts_sec() + 20;
                // 10,000 EUR ($200 of margin) in market 1, whose half-spread in the rule set is 0: the fill is at the mid itself
                let qty = MEUR / 100;
                let intent = market_order(1, true, qty, time);
                let line = t.trader_sign(99, &intent);
                t.intents.push(line);
                let j = t.sample_at(1, T_1400 + 11);
                let fill = t.hand_fill(j, true, qty, time, 99, 0);
                let proofs = vec![t.proof_of_sample(j)];
                t.append(::evaluator::BatchKind::PostJournal, vec![fill], proofs, time, false, vec![1]);
                t.fix();
            },
            who: Both,
            program: Some(Setl8Error::MarketUnpriced),
            verifier: &[F::FillNotAllowed],
        },
        Row {
            name: "the fill of a triggered pending order in a market the rule set does not price",
            sc: sc_flat_unpriced,
            tamper: |t| {
                pending_unpriced(t, true);
            },
            who: Both,
            program: Some(Setl8Error::MarketUnpriced),
            verifier: &[F::FillNotAllowed],
        },
        Row {
            // NOT an attack: this is the rule. The order is on file (the trader signed it; an honest evaluator would not even have
            // accepted it), it triggers, and the evaluator does not fill it. The verifier computes the same refusal and expects no
            // fill, so there is no TriggerMissed: both accept, which is exactly right. The same silence in a PRICED market is a miss.
            name: "a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself)",
            sc: sc_flat_unpriced,
            tamper: |t| {
                pending_unpriced(t, false);
            },
            who: Neither,
            program: None,
            verifier: &[],
        },
    ]
}

/// Runs ONE row through the machinery: honest history, tamper, program verdict, verifier verdict. Returns the markdown line of the
/// table and the problems found (empty = the row's classification holds).
fn check_row(row: &Row) -> (String, Vec<String>) {
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
    let line = format!("| {} | {:?} | {} | {} |", row.name, got, row.program.map(|e| format!("{e:?} ({})", u32::from(e))).unwrap_or_else(|| "-".into()), if row.verifier.is_empty() { "-".to_string() } else { format!("{:?}", row.verifier) });
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
    let report = if problems.is_empty() { Vec::new() } else { vec![format!("{}: {}\n   verifier says:\n{}", row.name, problems.join("; "), o.text)] };
    (line, report)
}

/// The aggregate test: every row, one report.
#[test]
fn trust_matrix() {
    let mut table = Vec::new();
    let mut bad = Vec::new();
    for row in rows() {
        let (line, problems) = check_row(&row);
        table.push(line);
        bad.extend(problems);
    }
    if std::env::var("TRUST_MATRIX_PRINT").is_ok() {
        println!("{}", table.join("\n"));
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n\n"));
}

/// One row by its exact name (exactly one row carries it), checked alone.
fn run_named_row(name: &str) {
    let all = rows();
    let hits: Vec<&Row> = all.iter().filter(|r| r.name == name).collect();
    assert_eq!(hits.len(), 1, "row name must exist exactly once in rows(): {name}");
    let (_, bad) = check_row(hits[0]);
    assert!(bad.is_empty(), "\n{}", bad.join("\n\n"));
}

/// One chain row by its exact name.
fn run_named_chain_row(name: &str) {
    let all = chain_rows();
    let hits: Vec<&ChainRow> = all.iter().filter(|r| r.name == name).collect();
    assert_eq!(hits.len(), 1, "chain row name must exist exactly once in chain_rows(): {name}");
    check_chain_row(hits[0]);
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

fn check_chain_row(row: &ChainRow) {
    let (mut t, mut chain) = honest(&(row.sc)());
    let got = (row.run)(&mut t, &mut chain);
    assert_eq!(got, Some(u32::from(row.program)), "{}", row.name);
}

#[test]
fn trust_matrix_chain_rows() {
    for row in chain_rows() {
        check_chain_row(&row);
    }
}

// ------------------------------------------------------------------------------------------------- one named test per row (M12, item 8)
//
// Every row of `handover/trust-matrix.md` has EXACTLY ONE test of its own, named after the row (`row_NNN_<slug>`, numbered in the order
// of `rows()`; `chain_row_NN_<slug>` for the chain rows). The macro writes the test function and the entry of the listing below from the
// same line, so a test cannot exist without a listing entry or the reverse. A test finds its row by the exact NAME (not by position), so
// reordering `rows()` cannot make a test check another row; a renamed or deleted row fails its test. `every_row_has_exactly_one_named_test`
// compares the listing with `rows()` and `chain_rows()` and with the table in the document.

macro_rules! row_tests {
    ($($f:ident => $name:expr),* $(,)?) => {
        $( #[test] fn $f() { run_named_row($name); } )*
        /// (test function name, row name), in the order of `rows()`.
        const ROW_TESTS: &[(&str, &str)] = &[ $( (stringify!($f), $name) ),* ];
    };
}

macro_rules! chain_row_tests {
    ($($f:ident => $name:expr),* $(,)?) => {
        $( #[test] fn $f() { run_named_chain_row($name); } )*
        /// (test function name, chain row name), in the order of `chain_rows()`.
        const CHAIN_ROW_TESTS: &[(&str, &str)] = &[ $( (stringify!($f), $name) ),* ];
    };
}

row_tests! {
    row_001_fill_price_altered_by_one_unit => "fill price altered by one unit",
    row_002_close_exit_price_altered_pnl_recomputed => "close exit price altered (pnl recomputed)",
    row_003_close_pnl_wrong_by_one_micro_usd => "close pnl wrong by one micro-USD",
    row_004_quick_flag_flipped_on_a_quick_slice => "quick flag flipped on a quick slice",
    row_005_quick_profit_credited_in_full => "quick profit credited in full",
    row_006_fee_amount_beyond_i64 => "fee amount beyond i64",
    row_007_tight_start_l_level_one_micro_off => "tight start: l_level one micro off",
    row_008_floor_raise_l_level_one_micro_off => "floor raise: l_level one micro off",
    row_009_floor_raise_dropped_the_breach_then_names_a_stale_level => "floor raise dropped (the breach then names a stale level)",
    row_010_tight_start_and_floor_raise_swapped => "tight start and floor raise swapped",
    row_011_meta_engine_version_wrong => "meta: engine version wrong",
    row_012_batch_replayed => "batch replayed",
    row_013_wrong_prev_hash => "wrong prev_hash",
    row_014_posted_by_a_key_that_is_not_the_evaluator => "posted by a key that is not the evaluator",
    row_015_event_time_goes_backwards => "event time goes backwards",
    row_016_event_dated_30_s_after_the_block_time => "event dated 30 s after the block time",
    row_017_proof_carries_another_market_s_feed => "proof carries another market's feed",
    row_018_proof_older_than_the_market_s_max_age => "proof older than the market's max_age",
    row_019_batch_of_nine_events => "batch of nine events",
    row_020_fill_in_a_closed_session => "fill in a closed session",
    row_021_fill_quantity_not_a_multiple_of_the_step => "fill quantity not a multiple of the step",
    row_022_commission_fee_missing_after_a_close => "commission fee missing after a close",
    row_023_commission_fee_one_micro_usd_too_high => "commission fee one micro-USD too high",
    row_024_commission_fee_placed_before_its_fill => "commission fee placed before its fill",
    row_025_holding_fee_omitted_at_the_rollover => "holding fee omitted at the rollover",
    row_026_holding_fee_one_micro_usd_too_low => "holding fee one micro-USD too low",
    row_027_meta_samples_hash_is_not_a_prefix_of_the_archive => "Meta.samples_hash is not a prefix of the archive",
    row_028_meta_intents_hash_is_not_a_prefix_of_the_intents_file => "Meta.intents_hash is not a prefix of the intents file",
    row_029_meta_flat_says_flat_while_a_position_is_open => "Meta.flat says flat while a position is open",
    row_030_breach_posted_late_with_the_old_sample_replayed_price => "breach posted late with the old sample (replayed price)",
    row_031_breach_built_from_the_worst_sample_of_each_market_never => "breach built from the worst sample of each market (never simultaneous)",
    row_032_breach_omitted_the_trader_keeps_the_position => "breach omitted (the trader keeps the position)",
    row_033_fail_omitted_at_the_static_floor => "fail omitted at the static floor",
    row_034_static_floor_fail_on_an_account_above_the_floor => "static-floor fail on an account above the floor",
    row_035_quick_limit_fail_on_an_account_below_the_limit => "quick-limit fail on an account below the limit",
    row_036_pass_requested_and_confirmed_on_an_account_that_is_not => "pass requested and confirmed on an account that is not flat",
    row_037_slow_profitable_slice_booked_as_quick_half_credited => "slow profitable slice booked as quick (half credited)",
    row_038_close_names_another_entry_price_pnl_consistent_with_it => "close names another entry price (pnl consistent with it)",
    row_039_fill_reports_a_wrong_average_entry => "fill reports a wrong average entry",
    row_040_fill_reports_a_wrong_margin => "fill reports a wrong margin",
    row_041_trade_with_no_signed_intent_forged_trade => "trade with no signed intent (forged trade)",
    row_042_intent_signed_by_a_key_that_is_not_the_trader_s => "intent signed by a key that is not the trader's",
    row_043_archive_the_price_of_the_proof_s_sample_altered => "archive: the price of the proof's sample altered",
    row_044_archive_the_sample_behind_a_fill_dropped => "archive: the sample behind a fill dropped",
    row_045_stop_loss_triggered_but_not_executed => "stop-loss triggered but not executed",
    row_046_limit_order_triggered_but_never_filled => "limit order triggered but never filled",
    row_047_stop_loss_close_with_no_trigger => "stop-loss close with no trigger",
    row_048_fill_on_a_held_account => "fill on a held account",
    row_049_fill_during_the_12_h_lock_after_a_breach => "fill during the 12 h lock after a breach",
    row_050_fill_on_an_account_that_already_failed => "fill on an account that already failed",
    row_051_a_sample_that_caused_a_breach_dropped_from_the_archive => "a sample that caused a breach dropped from the archive, breach omitted (no second recorder)",
    row_052_the_same_with_an_independent_second_recorder => "the same, with an independent second recorder",
    row_053_market_order_executed_at_an_older_sample_inside_max_age => "market order executed at an older sample inside max_age (worse price)",
    row_054_a_trader_s_market_order_ignored_no_fill_intent_on_file => "a trader's market order ignored (no fill, intent on file)",
    row_055_pending_order_that_only_reduces_the_close_that_executes => "pending order that only reduces: the Close that executes it is missing",
    row_056_pending_order_that_only_reduces_its_close_at_a_price => "pending order that only reduces: its Close at a price altered by one unit",
    row_057_close_under_the_nonce_of_a_pending_order_that_no_sample => "Close under the nonce of a pending order that no sample triggers",
    row_058_close_under_the_nonce_of_a_set_sl_tp => "Close under the nonce of a set_sl_tp",
    row_059_commission_fee_posted_after_the_slice_that_overshoots => "commission fee posted after the slice that overshoots the quick limit",
    row_073_forced_close_after_the_quick_limit_overshoot_relabelled_as_manual => "forced close after the quick-limit overshoot relabelled as the trader's own close (reason Manual)",
    row_060_flip_that_drops_its_opening_remainder_although_its => "flip that drops its opening remainder although its closing slice did not overshoot",
    row_061_pending_order_the_account_could_not_margin_is_filled => "pending order the account could not margin is filled anyway",
    row_062_breach_posted_where_its_closes_end_at_or_below_the => "breach posted where its closes end at or below the static floor",
    row_063_fail_omitted_a_tightend_instead_where_a_breach_s_closes => "Fail omitted (a TightEnd instead) where a breach's closes end at or below the static floor",
    row_064_breach_posted_where_its_closes_overshoot_the_quick_limit => "breach posted where its closes overshoot the quick limit",
    row_065_phase_advanced_to_another_phase_after_pass_finalized => "phase_advanced to another phase after pass_finalized",
    row_066_phase_advanced_repeated_after_pass_finalized => "phase_advanced repeated after pass_finalized",
    row_067_phase_advanced_with_no_pass_line_before_it => "phase_advanced with no pass line before it",
    row_068_a_market_fill_the_account_cannot_margin => "a market fill the account cannot margin",
    row_069_a_market_fill_above_the_market_s_notional_cap => "a market fill above the market's notional cap",
    row_070_a_market_fill_in_a_market_the_account_s_rule_set_does => "a market fill in a market the account's rule set does not price",
    row_071_the_fill_of_a_triggered_pending_order_in_a_market_the => "the fill of a triggered pending order in a market the rule set does not price",
    row_072_a_triggered_pending_order_in_an_unpriced_market_that => "a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself)",
}

chain_row_tests! {
    chain_row_01_confirm_pass_with_no_pass_requested => "confirm_pass with no pass requested",
    chain_row_02_post_journal_while_a_pass_is_confirmed_the_journal_is => "post_journal while a pass is confirmed (the journal is frozen)",
    chain_row_03_request_advance_below_the_target_b_1_08_s => "request_advance below the target (b < 1.08 S)",
    chain_row_04_finalize_pass_before_the_delay_has_elapsed => "finalize_pass before the delay has elapsed",
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
    s.s(0, T_1400 + 30, 1_005_603_000_000);
    s.o(close_order(0, T_1400 + 30));
    s.s(0, T_1400 + 40, 1_005_603_000_000);
    s.end(T_1400 + 45);
    let spec = base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 60, vec![(0, 10_080)])], 20, 2, s.steps);
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
    let spec = base_spec(vec![eur_spec(0, "EURUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)]), eur_spec(1, "GBPUSD", TEST_HALF_SPREAD_E6, 900, vec![(0, 10_080)])], 0, 2, s.steps);
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

// ------------------------------------------------------------------------------------------------- reconciliation with handover/trust-matrix.md

/// The 11 Reset (M8) rows of the document. They are asserted in `tests/trust_reset.rs`, which is NOT part of this file: there they run in
/// ONE table-driven test (`reset_trust_rows`), so none of them has a test of its own. `reset_rows_listing_matches_document_and_source`
/// pins the listing against the document and against that file; the audit fragment 08a reports them as "rows without an individually named test".
const RESET_ROWS: &[&str] = &[
    "reset: a price below the registry's tier price paid to the vault (a fake reset price)",
    "reset: the registry replaced by a copy with cheaper tiers at another address",
    "reset: an account that has not failed (an Active account of a high phase)",
    "reset: a second reset of the same failed record",
    "reset: a second reset of the same failed record with the local mark wiped (the vault is the backstop)",
    "reset: by a banned wallet",
    "reset: a violation-failed account after the wallet was unbanned (DESIGN 2.9: no reset)",
    "reset: while the sector is paused",
    "reset: while the vault pauses this product",
    "reset into a higher phase: the new account copies the failed phase (there is no phase argument)",
    "reset: an evaluator log that opens the new account in a higher phase than the chain's account",
];

/// The cells of every markdown table row (`| a | b |`) of `text`, without the header and the separator lines.
fn table_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| l.starts_with("| "))
        .map(|l| l.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect::<Vec<_>>())
        .filter(|cells| cells[0] != "class" && !cells[0].starts_with("---"))
        .collect()
}

fn trust_doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/trust-matrix.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("handover/trust-matrix.md must be readable: {e}"))
}

#[test]
fn every_row_has_exactly_one_named_test() {
    let all = rows();
    let chain = chain_rows();
    // the numbers the document states
    assert_eq!(all.len(), 73, "73 batch/log rows");
    assert_eq!(chain.len(), 4, "4 chain rows");
    assert_eq!(ROW_TESTS.len(), all.len(), "the number of per-row tests differs from rows().len()");
    assert_eq!(CHAIN_ROW_TESTS.len(), chain.len(), "the number of per-chain-row tests differs from chain_rows().len()");
    // row names are unique, every row has exactly one test entry, in the order of the table
    for (i, r) in all.iter().enumerate() {
        assert_eq!(all.iter().filter(|x| x.name == r.name).count(), 1, "duplicate row name: {}", r.name);
        assert_eq!(ROW_TESTS.iter().filter(|(_, n)| *n == r.name).count(), 1, "row without exactly one test: {}", r.name);
        assert_eq!(ROW_TESTS[i].1, r.name, "listing out of order at {i}");
    }
    for (i, r) in chain.iter().enumerate() {
        assert_eq!(chain.iter().filter(|x| x.name == r.name).count(), 1, "duplicate chain row name: {}", r.name);
        assert_eq!(CHAIN_ROW_TESTS.iter().filter(|(_, n)| *n == r.name).count(), 1, "chain row without exactly one test: {}", r.name);
        assert_eq!(CHAIN_ROW_TESTS[i].1, r.name, "chain listing out of order at {i}");
    }
    // two rows never share a test function, and a per-row test is not one of the aggregates
    let mut fns: Vec<&str> = ROW_TESTS.iter().chain(CHAIN_ROW_TESTS.iter()).map(|(f, _)| *f).collect();
    let n = fns.len();
    fns.sort_unstable();
    fns.dedup();
    assert_eq!(fns.len(), n, "two rows share a test function name");
    assert!(fns.iter().all(|f| f.starts_with("row_") || f.starts_with("chain_row_")));
}

/// The document and the table in this file say the same thing: same names, same WHO, same program error (name and code), same verifier
/// finding classes, for the 73 batch/log rows and the 4 chain rows; and the 11 Reset rows are the ones listed in `RESET_ROWS`.
#[test]
fn trust_matrix_document_matches_rows() {
    let doc = trust_doc();
    let (main, _) = doc.split_once("## Reset (M8)").expect("a Reset (M8) section");
    let (_, tables) = main.split_once("## Caught by the program, and also by the verifier (BOTH)").expect("the BOTH section");
    let (batch_part, chain_and_neither) = tables.split_once("## Chain-level rows").expect("the chain-level section");
    let (chain_part, neither_part) = chain_and_neither.split_once("## NEITHER").expect("the NEITHER section");
    let mut doc_rows = table_rows(batch_part);
    doc_rows.extend(table_rows(neither_part));
    let all = rows();
    assert_eq!(doc_rows.len(), all.len(), "the document lists {} batch/log rows, the test table has {}", doc_rows.len(), all.len());
    for r in &all {
        let cells = doc_rows.iter().find(|c| c[0] == r.name).unwrap_or_else(|| panic!("row missing from the document: {}", r.name));
        assert_eq!(cells[1], format!("{:?}", r.who), "WHO of '{}' in the document", r.name);
        let prog = r.program.map(|e| format!("{e:?} ({})", u32::from(e))).unwrap_or_else(|| "-".into());
        assert_eq!(cells[2], prog, "program column of '{}' in the document", r.name);
        let ver = if r.verifier.is_empty() { "-".to_string() } else { format!("{:?}", r.verifier) };
        assert_eq!(cells[3], ver, "verifier column of '{}' in the document", r.name);
    }
    let doc_chain = table_rows(chain_part);
    let chain = chain_rows();
    assert_eq!(doc_chain.len(), chain.len(), "chain rows in the document");
    for r in &chain {
        let cells = doc_chain.iter().find(|c| c[0] == r.name).unwrap_or_else(|| panic!("chain row missing from the document: {}", r.name));
        assert_eq!(cells[1], "Program");
        assert_eq!(cells[2], format!("{:?} ({})", r.program, u32::from(r.program)), "program column of chain row '{}'", r.name);
    }
}

#[test]
fn reset_rows_listing_matches_document_and_source() {
    let doc = trust_doc();
    let reset = doc.split_once("## Reset (M8)").expect("a Reset (M8) section").1.split_once("## Residual trust").expect("Residual trust").0;
    let doc_rows = table_rows(reset);
    assert_eq!(RESET_ROWS.len(), 11, "11 Reset rows");
    assert_eq!(doc_rows.len(), RESET_ROWS.len(), "Reset rows in the document");
    let src = include_str!("trust_reset.rs");
    for n in RESET_ROWS {
        assert_eq!(doc_rows.iter().filter(|c| c[0] == *n).count(), 1, "Reset row missing from the document: {n}");
        // asserted in trust_reset.rs: its row table carries exactly this name, once
        assert_eq!(src.matches(&format!("name: \"{n}\"")).count(), 1, "Reset row missing from tests/trust_reset.rs: {n}");
    }
    // and trust_reset.rs has no row the document does not list
    assert_eq!(src.matches("            name: \"").count(), RESET_ROWS.len(), "tests/trust_reset.rs has a row this listing does not");
}
