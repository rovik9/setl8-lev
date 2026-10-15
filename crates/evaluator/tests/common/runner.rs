//! Runs a generated history ([`evaluator::testkit::PathSpec`]) against an evaluator and a chain.
//! The same runner serves the engine-only campaign (a simulated chain) and the LiteSVM campaign (the real program).
#![allow(dead_code)]

use ::evaluator::codec::OrderIntent;
use ::evaluator::testkit::{PassPlan, PathSpec, Step};
use ::evaluator::*;
use journal_types::JournalEvent;
use setl8_verify::log;

use super::*;

pub const PASS_DELAY_SECS: i64 = 172_800;
pub const CANCEL_AFTER_SECS: i64 = 3_600;

/// The file order of the event log.
#[allow(clippy::large_enum_variant)]
pub enum Item {
    Batch(Batch),
    /// A ready-made JSON line (`pass_requested`, `pass_confirmed`, `phase_advanced`, `pass_finalized`).
    Line(String),
}

/// Seconds a pass flow takes after the button press (the generator and the runner agree on it).
pub fn pass_duration(plan: PassPlan, phase: u8) -> i64 {
    match plan {
        PassPlan::Confirm if phase >= 2 => 1 + PASS_DELAY_SECS + 5,
        PassPlan::Confirm => 6,
        PassPlan::Unanswered => CANCEL_AFTER_SECS + 5,
        PassPlan::Veto => 70,
        PassPlan::JournalBehind | PassPlan::PendingLeft => 6,
    }
}

/// The chain as the runner needs it. The simulated chain derives everything from the batches; the LiteSVM chain posts
/// them to the real program and compares the stored account after every transaction.
pub trait Chain {
    /// Posts a batch; the chain checks the account against `b.account_after`.
    fn post(&mut self, w: &World, b: &Batch);
    fn view(&mut self, w: &World) -> ChainView;
    /// `request_advance`; false when the program refuses it (target not reached, not flat, tight mode on).
    fn request_advance(&mut self, w: &World, t: i64) -> bool;
    fn confirm_pass(&mut self, w: &World, t: i64);
    fn reject_pass(&mut self, w: &World, t: i64, reason: PassReject);
    fn cancel_pass(&mut self, w: &World, t: i64);
    fn veto_pass(&mut self, w: &World, t: i64);
    fn finalize_pass(&mut self, w: &World, t: i64);
}

#[derive(Default, Clone, Debug)]
pub struct PassOutcome {
    pub confirmed: u32,
    pub finalized: u32,
    pub rejected: u32,
    pub cancelled: u32,
    pub vetoed: u32,
    pub graduated: bool,
    /// Button presses the program refused (the story did not reach 1.08 S, or a position was open).
    pub skipped: u32,
}

pub struct RunResult {
    pub world: World,
    pub items: Vec<Item>,
    pub pass: PassOutcome,
    /// Anything the runner did not expect (an honest history that did not go as planned).
    pub notes: Vec<String>,
    pub batches_posted: usize,
    pub samples_rejected: u32,
}

pub fn spec_rules(spec: &PathSpec) -> RulesCfg {
    let markets = spec
        .markets
        .iter()
        .map(|m| MarketCfg {
            id: m.id,
            class: m.class,
            source: m.source,
            feed_id: journal_types::feed_id(&journal_types::NativeSha256, m.source, &m.symbol),
            max_age_secs: m.max_age_secs,
            status: STATUS_ENABLED,
            qty_step: 1_000,
            qty_min: 1_000,
            max_notional_micro: 1_000_000_000_000_000,
            sessions: m.sessions.clone(),
            half_spread_e6: m.spread_e6,
        })
        .collect();
    RulesCfg {
        ruleset_version: 1,
        leverage_x: [50, 20, 20, 20, 10, 5, 5, 2],
        holding_fee_e6: [100, 200, 400, 200, 400, 400, 1000, 1600],
        commission_e6: spec.commission_e6,
        markets,
    }
}

pub fn world_for(spec: &PathSpec) -> World {
    World::new(spec_rules(spec), spec.size_micro, spec.phase, spec.opened_time)
}

fn kv_line(ty: &str, account: &[u8; 32], fields: &str) -> String {
    format!("{{\"type\":\"{ty}\",\"account\":\"{}\",{fields}}}\n", log::base58(account))
}

pub fn pass_requested_line(account: &[u8; 32], phase: u8, amount: u64, v: &ChainView, time: i64) -> String {
    kv_line("pass_requested", account, &format!("\"phase\":{phase},\"amount\":{amount},\"b\":{},\"seq\":{},\"journal_hash\":\"{}\",\"time\":{time}", v.b, v.seq, setl8_verify::hexutil::encode(&v.journal_hash)))
}

pub fn pass_confirmed_line(account: &[u8; 32], phase: u8, amount: u64, v: &ChainView, eligible_at: i64) -> String {
    kv_line("pass_confirmed", account, &format!("\"phase\":{phase},\"amount\":{amount},\"b\":{},\"seq\":{},\"journal_hash\":\"{}\",\"eligible_at\":{eligible_at}", v.b, v.seq, setl8_verify::hexutil::encode(&v.journal_hash)))
}

pub fn phase_advanced_line(account: &[u8; 32], from: u8, to: u8, time: i64) -> String {
    kv_line("phase_advanced", account, &format!("\"from_phase\":{from},\"to_phase\":{to},\"time\":{time}"))
}

pub fn pass_finalized_line(account: &[u8; 32], phase: u8, to_phase: u8, amount: u64, request_id: u64, time: i64) -> String {
    kv_line("pass_finalized", account, &format!("\"phase\":{phase},\"to_phase\":{to_phase},\"amount\":{amount},\"request_id\":{request_id},\"tally_count\":0,\"tally_total\":0,\"window_total\":0,\"time\":{time}"))
}

/// events.jsonl text from the items in file order.
pub fn items_text(p: &AccountParams, items: &[Item]) -> String {
    let mut t = log::header_line();
    t.push_str(&log::opened_line(&opened_rec(p)));
    let mut slot = 1000u64;
    for it in items {
        match it {
            Item::Line(l) => t.push_str(l),
            Item::Batch(b) => {
                let mut seq = b.batch.first_seq;
                for ev in &b.batch.events {
                    let proof = ev.proof_index().and_then(|i| b.batch.proofs.get(i)).map(|p| p.to_bytes().expect("proof")).unwrap_or_default();
                    t.push_str(&log::event_line(&p.account, seq, &ev.to_bytes(), &proof, slot));
                    seq += 1;
                    slot += 1;
                }
            }
        }
    }
    t
}

pub fn verify_items(w: &World, items: &[Item], snapshot: Option<&str>, as_of: Option<u64>) -> setl8_verify::Report {
    let events = items_text(&w.opened, items);
    let rules = rules_json(&w.rules, w.opened.size_micro);
    setl8_verify::verify_account(&setl8_verify::Inputs {
        events: events.as_bytes(),
        rules: rules.as_bytes(),
        archive: w.ev.archive_bytes(),
        intents: w.ev.intents_bytes(),
        snapshot: snapshot.map(|s| s.as_bytes()),
        second_archive: None,
        as_of_seq: as_of,
    })
    .expect("inputs parse")
}

fn policy_for(spec: &PathSpec, heartbeat: bool) -> BatchPolicy {
    BatchPolicy::MAX.with_max_events(spec.batch_max_events).with_heartbeat(heartbeat)
}

pub fn run_path<C: Chain>(spec: &PathSpec, w0: World, chain: &mut C) -> RunResult {
    let mut w = w0;
    let mut r = RunResult { world: World::base(), items: Vec::new(), pass: PassOutcome::default(), notes: Vec::new(), batches_posted: 0, samples_rejected: 0 };
    let account = w.opened.account;
    let size = w.opened.size_micro;
    let mut phase_now = spec.phase;
    for step in &spec.steps {
        match step {
            Step::Sample { market, sec, mid } => {
                let src = spec.markets.iter().find(|m| m.id == *market).map(|m| m.source).unwrap_or(2);
                let ts_ns = u64::try_from(*sec).unwrap_or(0) * 1_000_000_000;
                // a signed report (source 3) is archived by the sha256 of its blob; the stand-in blob is (market, time)
                let digest = if src == 3 { ::evaluator::sha256::sha256(&[market.to_le_bytes().as_slice(), ts_ns.to_le_bytes().as_slice()].concat()) } else { [0; 32] };
                if w.ev.on_sample(*market, ts_ns, i128::from(*mid) * 1_000_000, ts_ns + 1_000_000, digest).is_err() {
                    r.samples_rejected += 1;
                }
            }
            Step::Order { nonce, intent } => {
                let s = sign(&account, *nonce, intent);
                let _ = w.ev.on_order(&s);
            }
            Step::Tick { sec } => w.ev.tick(*sec),
            Step::Post { sec, heartbeat } => {
                w.ev.tick(*sec);
                post_all(&mut w, chain, &mut r, &policy_for(spec, *heartbeat));
            }
            Step::Pass { sec, plan } => {
                let t = *sec;
                w.ev.tick(t);
                if *plan != PassPlan::JournalBehind {
                    post_all(&mut w, chain, &mut r, &policy_for(spec, false));
                }
                let amount = if phase_now == 1 { 0 } else { u64::try_from(size).unwrap_or(0) * 8 / 100 };
                if !chain.request_advance(&w, t) {
                    r.pass.skipped += 1;
                    if *plan == PassPlan::JournalBehind {
                        post_all(&mut w, chain, &mut r, &policy_for(spec, false));
                    }
                    continue;
                }
                let v = chain.view(&w);
                r.items.push(Item::Line(pass_requested_line(&account, phase_now, amount, &v, t)));
                if *plan == PassPlan::Unanswered {
                    // the evaluator never answers: the trader cancels after an hour
                    let tc = t + CANCEL_AFTER_SECS;
                    chain.cancel_pass(&w, tc);
                    w.ev.tick(tc + 1);
                    r.pass.cancelled += 1;
                    continue;
                }
                let replay = {
                    let rep = verify_items(&w, &r.items, None, Some(v.seq));
                    if rep.verdict == setl8_verify::Verdict::Valid { ReplayCheck::Valid } else { ReplayCheck::Mismatch }
                };
                let dec = w.ev.request_pass(t, &v, replay);
                match (plan, dec) {
                    (PassPlan::JournalBehind, PassDecision::Reject(reason)) | (PassPlan::PendingLeft, PassDecision::Reject(reason)) => {
                        chain.reject_pass(&w, t + 1, reason);
                        w.ev.pass_resolved();
                        r.pass.rejected += 1;
                        w.ev.tick(t + 3);
                        post_all(&mut w, chain, &mut r, &policy_for(spec, false));
                    }
                    (PassPlan::Confirm, PassDecision::Confirm) | (PassPlan::Veto, PassDecision::Confirm) => {
                        chain.confirm_pass(&w, t + 1);
                        r.pass.confirmed += 1;
                        if phase_now == 1 {
                            r.items.push(Item::Line(phase_advanced_line(&account, 1, 2, t + 1)));
                            w.ev.phase_started(2);
                            phase_now = 2;
                        } else {
                            r.items.push(Item::Line(pass_confirmed_line(&account, phase_now, amount, &v, t + 1 + PASS_DELAY_SECS)));
                            if *plan == PassPlan::Veto {
                                chain.veto_pass(&w, t + 60);
                                w.ev.pass_resolved();
                                r.pass.vetoed += 1;
                                w.ev.tick(t + 62);
                            } else {
                                let tf = t + 1 + PASS_DELAY_SECS;
                                w.ev.tick(tf);
                                chain.finalize_pass(&w, tf);
                                r.pass.finalized += 1;
                                let next = if phase_now == 5 { 0 } else { phase_now + 1 };
                                r.items.push(Item::Line(pass_finalized_line(&account, phase_now, next, amount, 1, tf)));
                                if next == 0 {
                                    r.pass.graduated = true;
                                    w.ev.end_externally();
                                } else {
                                    w.ev.phase_started(next);
                                    phase_now = next;
                                }
                                w.ev.tick(tf + 1);
                            }
                        }
                    }
                    (p, d) => {
                        r.notes.push(format!("pass plan {p:?} met decision {d:?}"));
                        chain.reject_pass(&w, t + 1, PassReject::Other);
                        w.ev.pass_resolved();
                    }
                }
            }
        }
    }
    r.world = w;
    r
}

fn post_all<C: Chain>(w: &mut World, chain: &mut C, r: &mut RunResult, policy: &BatchPolicy) {
    for b in w.ev.drain_batches(policy) {
        chain.post(w, &b);
        r.batches_posted += 1;
        r.items.push(Item::Batch(b));
    }
}

/// Used by tests that only need a signed order.
pub fn sign_intent(account: &[u8; 32], nonce: u32, intent: &OrderIntent) -> SignedIntent {
    sign(account, nonce, intent)
}

pub fn events(items: &[Item]) -> Vec<JournalEvent> {
    items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.batch.events.clone()) } else { None }).flatten().collect()
}

/// A chain derived from the batches (no program): enough for the pass flow.
#[derive(Default)]
pub struct SimChain {
    seq: u64,
    hash: [u8; 32],
    b: i64,
    flat: bool,
    phase: u8,
}

impl Chain for SimChain {
    fn post(&mut self, _w: &World, b: &Batch) {
        let a = b.account_after;
        self.seq = a.seq;
        self.hash = a.journal_hash;
        self.b = a.b;
        self.flat = a.flat;
    }
    fn view(&mut self, w: &World) -> ChainView {
        if self.seq == 0 && self.hash == [0; 32] {
            self.hash = journal_types::genesis_hash(&journal_types::NativeSha256, &w.opened.account);
            self.b = w.opened.size_micro;
            self.flat = true;
            self.phase = w.opened.phase;
        }
        ChainView { seq: self.seq, journal_hash: self.hash, b: self.b, flat: self.flat, phase: w.ev.phase() }
    }
    fn request_advance(&mut self, w: &World, _t: i64) -> bool {
        let v = self.view(w);
        // the program's checks: flat, closed balance at the target, tight mode off
        v.flat && v.b >= w.opened.size_micro / 100 * 108 && !w.ev.state().tight_on
    }
    fn confirm_pass(&mut self, w: &World, _t: i64) {
        let _ = w;
    }
    fn reject_pass(&mut self, _w: &World, _t: i64, _r: PassReject) {}
    fn cancel_pass(&mut self, _w: &World, _t: i64) {}
    fn veto_pass(&mut self, _w: &World, _t: i64) {}
    fn finalize_pass(&mut self, _w: &World, _t: i64) {}
}

