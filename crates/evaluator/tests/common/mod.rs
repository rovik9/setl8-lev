//! Test harness for the evaluator crate: a signed-order world, and the translation of the evaluator's outputs into the
//! five input files of the public verifier.
#![allow(dead_code)]

use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signer};
use ::evaluator::codec::{IntentKind, OrderIntent};
use ::evaluator::*;
use journal_types::{feed_id, JournalEvent, NativeSha256};
use setl8_verify::hexutil;
use setl8_verify::log::{self, OpenedRec};

pub const USD: i64 = 1_000_000;
pub const S100K: i64 = 100_000 * USD;
pub const E12: u64 = 1_000_000_000_000;
/// 2026-10-12 (a Monday) 14:00:00 UTC.
pub const T_1400: i64 = 1_791_813_600;
pub const HOUR: i64 = 3_600;

pub fn account_key() -> [u8; 32] {
    [0xA7; 32]
}

pub fn trader_keypair() -> Keypair {
    let secret = SecretKey::from_bytes(&[0x42; 32]).expect("seed");
    let public: PublicKey = (&secret).into();
    Keypair { secret, public }
}

pub struct Ed25519Check;
impl IntentVerifier for Ed25519Check {
    fn verify(&self, trader: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(pk) = PublicKey::from_bytes(trader) else { return false };
        let Ok(sig) = ed25519_dalek::Signature::from_bytes(sig) else { return false };
        pk.verify_strict(message, &sig).is_ok()
    }
}

pub fn eur_market(id: u16) -> MarketCfg {
    MarketCfg {
        id,
        class: 0,
        source: 2,
        feed_id: feed_id(&NativeSha256, 2, "EURUSD"),
        max_age_secs: 60,
        status: STATUS_ENABLED,
        qty_step: 1_000,
        qty_min: 1_000,
        max_notional_micro: 1_000_000_000_000_000,
        sessions: vec![(0, 10_080)],
        half_spread_e6: 30,
    }
}

pub fn market(id: u16, class: u8, symbol: &str, spread: u32) -> MarketCfg {
    MarketCfg { class, feed_id: feed_id(&NativeSha256, 2, symbol), half_spread_e6: spread, ..eur_market(id) }
}

pub fn base_rules(markets: Vec<MarketCfg>) -> RulesCfg {
    RulesCfg {
        ruleset_version: 1,
        leverage_x: [50, 20, 20, 20, 10, 5, 5, 2],
        holding_fee_e6: [100, 200, 400, 200, 400, 400, 1000, 1600],
        commission_e6: [0; 8],
        markets,
    }
}

pub fn params(size: i64, phase: u8, opened: i64) -> AccountParams {
    AccountParams {
        account: account_key(),
        trader: trader_keypair().public.to_bytes(),
        challenge_id: 0,
        size_micro: size,
        phase,
        tier_index: 8,
        opened_time: opened,
    }
}

pub struct World {
    pub ev: Evaluator,
    next_nonce: u32,
    pub rules: RulesCfg,
    pub opened: AccountParams,
}

impl World {
    pub fn new(rules: RulesCfg, size: i64, phase: u8, opened: i64) -> World {
        World::from_params(rules, params(size, phase, opened))
    }

    pub fn from_params(rules: RulesCfg, p: AccountParams) -> World {
        let ev = Evaluator::new(rules.clone(), p.clone(), Box::new(Ed25519Check)).expect("config");
        World { ev, next_nonce: 1, rules, opened: p }
    }

    pub fn base() -> World {
        World::new(base_rules(vec![eur_market(0)]), S100K, 2, T_1400 - HOUR)
    }

    pub fn sample(&mut self, market: u16, sec: i64, mid: u64) {
        self.ev.on_sample_mid(market, sec, mid).expect("sample");
    }

    /// Signs `intent` with the next nonce and submits it.
    pub fn order(&mut self, intent: OrderIntent) -> Result<u32, OrderReject> {
        let nonce = self.next_nonce;
        self.next_nonce += 1;
        let s = sign(&self.opened.account, nonce, &intent);
        self.ev.on_order(&s).map(|_| nonce)
    }

    pub fn drain(&mut self) -> Vec<Batch> {
        self.ev.drain_batches(&BatchPolicy::MAX)
    }
}

pub fn sign(account: &[u8; 32], nonce: u32, intent: &OrderIntent) -> SignedIntent {
    let order = intent.to_bytes();
    let kp = trader_keypair();
    let sig = kp.sign(&::evaluator::codec::signing_message(account, nonce, &order)).to_bytes();
    SignedIntent { nonce, order, sig }
}

#[allow(clippy::too_many_arguments)]
pub fn order(kind: IntentKind, market: u16, side: u8, qty: u64, price: u64, sl: u64, tp: u64, target: u32, t: i64) -> OrderIntent {
    OrderIntent { kind, market, side, qty, price_e12: price, sl_e12: sl, tp_e12: tp, target_nonce: target, client_time: t }
}

pub fn market_order(market: u16, long: bool, qty: u64, t: i64) -> OrderIntent {
    order(IntentKind::Market, market, if long { 0 } else { 1 }, qty, 0, 0, 0, 0, t)
}

pub fn close_order(market: u16, qty: u64, t: i64) -> OrderIntent {
    order(IntentKind::Close, market, 0, qty, 0, 0, 0, 0, t)
}

pub fn eur(p4: u64) -> u64 {
    p4 * 100_000_000
}

// ---------------------------------------------------------------------------- verifier inputs

pub fn rules_json(rules: &RulesCfg, size: i64) -> String {
    let markets: Vec<_> = rules
        .markets
        .iter()
        .map(|m| {
            serde_json::json!({"id": m.id, "class": m.class, "feed_id": hexutil::encode(&m.feed_id), "max_age_secs": m.max_age_secs, "status": m.status,
                "qty_step": m.qty_step, "qty_min": m.qty_min, "max_notional_usd": m.max_notional_micro,
                "sessions": m.sessions.iter().map(|s| serde_json::json!([s.0, s.1])).collect::<Vec<_>>()})
        })
        .collect();
    let spread: serde_json::Map<String, serde_json::Value> = rules.markets.iter().map(|m| (m.id.to_string(), serde_json::json!(m.half_spread_e6))).collect();
    serde_json::to_string(&serde_json::json!({"version": 1, "ruleset_version": rules.ruleset_version, "fee_bps": 40, "leverage_x": rules.leverage_x,
        "holding_fee_e6": rules.holding_fee_e6, "commission_e6": rules.commission_e6, "tier_sizes": [size], "spread_e6": spread, "markets": markets}))
    .expect("json")
}

pub fn opened_rec(p: &AccountParams) -> OpenedRec {
    OpenedRec {
        account: p.account,
        trader: p.trader,
        challenge_id: p.challenge_id,
        size_s: u64::try_from(p.size_micro).expect("size"),
        tier_index: p.tier_index,
        phase: p.phase,
        ruleset_version: 1,
        genesis_hash: journal_types::genesis_hash(&NativeSha256, &p.account),
        time: p.opened_time,
    }
}

/// events.jsonl from the batches, as the program's `EventLog`s would give it.
pub fn events_text(p: &AccountParams, batches: &[Batch]) -> String {
    let mut t = log::header_line();
    t.push_str(&log::opened_line(&opened_rec(p)));
    let mut slot = 1000u64;
    for b in batches {
        let mut seq = b.batch.first_seq;
        for ev in &b.batch.events {
            let proof = ev.proof_index().and_then(|i| b.batch.proofs.get(i)).map(|p| p.to_bytes().expect("proof")).unwrap_or_default();
            t.push_str(&log::event_line(&p.account, seq, &ev.to_bytes(), &proof, slot));
            seq += 1;
            slot += 1;
        }
    }
    t
}

pub fn snapshot_json(w: &Evaluator, batches: &[Batch], phase: u8, status: u8) -> String {
    let a = batches.last().map(|b| b.account_after);
    let (seq, hash) = (w.seq(), w.head());
    let st = w.state();
    serde_json::to_string(&serde_json::json!({"version": 1, "b": st.b, "q_tally": st.q_tally, "latch": u8::from(st.latch), "tight_on": u8::from(st.tight_on),
        "r": st.r, "g_peak": st.g_peak, "l_level": st.l_level, "lock_until": st.lock_until, "seq": seq, "journal_hash": hexutil::encode(&hash),
        "flat": u8::from(a.map(|x| x.flat).unwrap_or(true)), "phase": phase, "status": status, "trades_closed": w.trades_closed()}))
    .expect("json")
}

pub struct Files {
    pub events: String,
    pub rules: String,
    pub archive: Vec<u8>,
    pub intents: Vec<u8>,
    pub snapshot: String,
}

pub fn files(w: &World, batches: &[Batch]) -> Files {
    let failed = matches!(w.ev.status(), Status::Failed(_));
    Files {
        events: events_text(&w.opened, batches),
        rules: rules_json(&w.rules, w.opened.size_micro),
        archive: w.ev.archive_bytes().to_vec(),
        intents: w.ev.intents_bytes().to_vec(),
        snapshot: snapshot_json(&w.ev, batches, w.ev.phase(), u8::from(failed)),
    }
}

pub fn verify(w: &World, batches: &[Batch]) -> setl8_verify::Report {
    let f = files(w, batches);
    setl8_verify::verify_account(&setl8_verify::Inputs {
        events: f.events.as_bytes(),
        rules: f.rules.as_bytes(),
        archive: &f.archive,
        intents: &f.intents,
        snapshot: Some(f.snapshot.as_bytes()),
        second_archive: None,
        as_of_seq: None,
    })
    .expect("inputs parse")
}

pub fn events_of(batches: &[Batch]) -> Vec<JournalEvent> {
    batches.iter().flat_map(|b| b.batch.events.iter().cloned()).collect()
}

pub fn describe(r: &setl8_verify::Report) -> String {
    r.findings.iter().map(|f| format!("{:?} seq={:?} expected={} found={} ({})", f.class, f.seq, f.expected, f.found, f.message)).collect::<Vec<_>>().join("\n")
}

pub mod bytes_gate;
pub mod runner;
pub mod reference;

// ---------------------------------------------------------------------------- helpers for hand-written scenarios

/// The smallest half-spread a test market may have (decision 64: a market whose half-spread is 0 is untradable). At mid 1.0 the
/// ask is 1.000001 (`1_000_001_000_000`) and the bid 0.999999 (`999_999_000_000`).
pub const TEST_HALF_SPREAD_E6: u32 = 1;
/// Hand constants for mid 1.0 at `TEST_HALF_SPREAD_E6`: a long fills at the ask, a short at the bid, a close takes the other side.
pub const ASK_AT_ONE: u64 = 1_000_001_000_000;
pub const BID_AT_ONE: u64 = 999_999_000_000;

/// A one-market (EURUSD-like, class 0) world with an exact half-spread, S = $100,000, and phase 2.
pub fn fx_world(spread_e6: u32) -> World {
    let mut m = eur_market(0);
    m.half_spread_e6 = spread_e6;
    m.max_age_secs = 3_600;
    World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR)
}

pub const MEUR: u64 = 1_000_000_000_000;
pub const EUR_1: u64 = 1_000_000_000_000;

/// Mid price from a number with four decimals expressed in 1e-4 (`10_006` is 1.0006).
pub fn p4(x: u64) -> u64 {
    x * 100_000_000
}

/// Asserts that the public verifier calls the evaluator's output VALID.
pub fn assert_valid(w: &World, batches: &[Batch]) {
    let rep = verify(w, batches);
    assert!(rep.verdict == setl8_verify::Verdict::Valid, "the verifier disagrees:\n{}", describe(&rep));
}

pub fn close_events(evs: &[JournalEvent]) -> Vec<&JournalEvent> {
    evs.iter().filter(|e| matches!(e, JournalEvent::Close { .. })).collect()
}

/// A signed order for arbitrary order bytes (for inputs `OrderIntent` would never encode).
pub fn sign_raw(account: &[u8; 32], nonce: u32, order: [u8; 48]) -> SignedIntent {
    let kp = trader_keypair();
    let sig = kp.sign(&::evaluator::codec::signing_message(account, nonce, &order)).to_bytes();
    SignedIntent { nonce, order, sig }
}
