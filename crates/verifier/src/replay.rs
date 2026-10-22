//! The replay core (pure). It rebuilds the account event by event with the same engine the program and the evaluator use
//! and records every disagreement as a [`Finding`]. Nothing the evaluator wrote is trusted: prices, pnl, credits, fees,
//! margins, average entries and floor levels are recomputed from the archive, the rules and the signed intents.
use std::collections::{BTreeMap, HashMap, HashSet};

use engine::{AccountState, Position, Rules as EngineRules, Side as ESide};
use journal_types::{
    check_proof_without_price, check_signed_update_shape, genesis_hash, next_hash, CloseReason, FailReason, FeeKind, JournalEvent,
    NativeSha256, OrderKind, PriceProof, ProofError, Side,
};

use crate::archive::Archive;
use crate::finding::*;
use crate::intents::{verify_signature, IntentFile, IntentKind};
use crate::log::{OpenedRec, PassKind};
use crate::rules::Rules;
use crate::snapshot::Snapshot;

/// One open position and its protective levels.
#[derive(Clone, Copy, Debug)]
pub struct Pos {
    pub p: Position,
    pub sl: u64,
    pub tp: u64,
}

/// Where a finding is attributed.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub seq: Option<u64>,
    pub idx: Option<usize>,
    pub time: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeeExpKind {
    Holding,
    Commission,
}

/// A fee the replay expects the journal to contain.
#[derive(Clone, Debug)]
pub struct FeeExp {
    pub kind: FeeExpKind,
    pub market: u16,
    pub time: i64,
    pub amount: i64,
    pub why: String,
}

#[derive(Default)]
struct Batch {
    /// (archive sample index, seq, event index) of every proof used in the batch.
    proofs: Vec<(usize, u64, usize)>,
    /// (intent line index, seq, event index) of every intent used in the batch.
    intents: Vec<(usize, u64, usize)>,
    /// nonce -> (quantity executed so far, quantity the intent asks for or None when "all").
    nonce_qty: BTreeMap<u32, (u64, Option<u64>)>,
}

pub struct PhaseRec {
    pub phase: u8,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub events: usize,
}

pub struct Replay<'a> {
    pub rules: &'a Rules,
    pub arch: &'a Archive,
    pub intents: &'a IntentFile,
    pub opened: OpenedRec,
    pub size: i64,
    pub st: AccountState,
    pub engine_rules: EngineRules,
    pub positions: BTreeMap<u16, Pos>,
    pub phase: u8,
    pub terminal: bool,
    pub trades_closed: u32,
    pub expect_seq: u64,
    pub last_seq: u64,
    pub head: [u8; 32],
    pub last_time: i64,
    pub event_count: usize,
    pub f: Findings,
    batch: Batch,
    pub fee_exp: Vec<FeeExp>,
    roll_cursor: Option<i64>,
    pub(crate) sample_prefix: HashMap<[u8; 32], usize>,
    pub(crate) intent_prefix: HashMap<[u8; 32], usize>,
    last_sample_n: usize,
    last_intent_n: usize,
    pub(crate) bad_sig_lines: HashSet<usize>,
    pub phases: Vec<PhaseRec>,
    pub passes: Vec<PassVerdict>,
    in_batch_events: usize,
    pub quick_fail_due: bool,
    /// Archive-walk state (stage 2).
    pub walk: Option<crate::walk::Walker>,
    /// The largest event or Meta time seen (the walk runs up to it).
    pub max_time: i64,
    /// M9c: the intervals `[from, to)` in which the account was held (`to` is None while it still is).
    holds: Vec<(i64, Option<i64>)>,
    /// The archive index of the sample the last accepted price proof refers to (M9c: the market state a market order saw).
    pub(crate) last_proof_si: Option<usize>,
    /// (second, nonce) of a market flip whose margin was judged at its `Close{Flip}`: its opening `Fill` is the other half.
    flip_checked: HashSet<(i64, u32)>,
    /// D-F13: `Close{Fail}` events no floor decision of their second supports (yet): the quick-limit forced closes of the other
    /// positions come BEFORE the overshooting slice and the `Fail{PhaseQuick}`, so they are judged when that `Fail` arrives.
    forced_pending: Vec<(Ctx, u16)>,
    /// D-F14: the nonces of the intents the trades of EARLIER batches used up (one intent is executed in one batch, once).
    closed_nonces: HashSet<u32>,
    /// D-F09: the seconds in which a forced close was flagged; a `Fail` of such a second is a `FalseFailMarks`.
    flagged_forced: HashSet<i64>,
    /// P25b (F-19): the feed and the status of the markets the log CHANGES, as they stand at the log position being replayed. The rules
    /// file shows the state at the END of the log; before the first change a market had the first change's old value.
    feed_now: BTreeMap<u16, [u8; 32]>,
    status_now: BTreeMap<u16, u8>,
}

/// Collects findings with the cap and the miss de-duplication.
#[derive(Default)]
pub struct Findings {
    pub list: Vec<Finding>,
    pub truncated: bool,
    pub source: &'static str,
    seen_miss: HashSet<(FindingClass, u64)>,
    pub epoch: u64,
}

impl Findings {
    pub fn new(source: &'static str) -> Self {
        Findings { source, ..Default::default() }
    }

    pub fn add(&mut self, class: FindingClass, ctx: Ctx, expected: impl Into<String>, found: impl Into<String>, message: impl Into<String>) {
        if self.list.len() >= MAX_FINDINGS {
            self.truncated = true;
            return;
        }
        self.list.push(Finding { class, seq: ctx.seq, event_index: ctx.idx, expected: expected.into(), found: found.into(), message: message.into(), source: self.source });
    }

    /// Like `add`, but a repeat of the same class in the same account-state epoch is dropped (a miss that persists over many
    /// samples is reported once).
    /// The account state changed: a miss of a class already reported may be reported again.
    pub fn bump(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    pub fn add_once(&mut self, class: FindingClass, ctx: Ctx, expected: impl Into<String>, found: impl Into<String>, message: impl Into<String>) {
        if self.seen_miss.insert((class, self.epoch)) {
            self.add(class, ctx, expected, found, message);
        }
    }
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Long => "long",
        Side::Short => "short",
    }
}

fn eside(s: Side) -> ESide {
    match s {
        Side::Long => ESide::Buy,
        Side::Short => ESide::Sell,
    }
}

fn opposite(s: Side) -> ESide {
    match s {
        Side::Long => ESide::Sell,
        Side::Short => ESide::Buy,
    }
}

/// An order that executes at once (market, close) is executed in the second the evaluator received it; the trader's signed `client_time`
/// is within this many seconds of it (D-F18, mirrored by `evaluator::config::MAX_ORDER_SKEW_SECS`).
pub const ORDER_WINDOW_SECS: i64 = 5;

/// Longest stretch between two events over which holding-fee nights are checked one by one (about ten years).
pub const MAX_ROLLOVER_GAP: i64 = 3_660;

pub fn mid_pair(mid: u64, h: u32) -> Option<(u64, u64)> {
    Some((engine::bid_from_mid(mid, h).ok()?, engine::ask_from_mid(mid, h).ok()?))
}

/// First rollover (22:00 UTC) instant strictly after `t`.
pub fn next_rollover_after(t: i64) -> i64 {
    // day_index changes at 22:00 UTC; the instant of index k is k * 86400 + 22 * 3600
    let k = engine::day_index(t).saturating_add(1);
    k.saturating_mul(86_400).saturating_add(22 * 3_600)
}

impl<'a> Replay<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rules: &'a Rules,
        arch: &'a Archive,
        intents: &'a IntentFile,
        opened: OpenedRec,
        source: &'static str,
        sample_prefix: HashMap<[u8; 32], usize>,
        intent_prefix: HashMap<[u8; 32], usize>,
    ) -> Option<Replay<'a>> {
        let size = i64::try_from(opened.size_s).ok()?;
        let st = engine::reset_phase_state(size);
        let head = genesis_hash(&NativeSha256, &opened.account);
        let phase = opened.phase;
        Some(Replay {
            rules,
            arch,
            intents,
            size,
            st,
            engine_rules: EngineRules::default(),
            positions: BTreeMap::new(),
            phase,
            terminal: false,
            trades_closed: 0,
            expect_seq: 1,
            last_seq: 0,
            head,
            last_time: opened.time,
            event_count: 0,
            f: Findings::new(source),
            batch: Batch::default(),
            fee_exp: Vec::new(),
            roll_cursor: None,
            sample_prefix,
            intent_prefix,
            last_sample_n: 0,
            last_intent_n: 0,
            bad_sig_lines: HashSet::new(),
            phases: vec![PhaseRec { phase, first_seq: None, last_seq: None, events: 0 }],
            passes: Vec::new(),
            in_batch_events: 0,
            quick_fail_due: false,
            walk: None,
            max_time: opened.time,
            holds: Vec::new(),
            last_proof_si: None,
            flip_checked: HashSet::new(),
            forced_pending: Vec::new(),
            closed_nonces: HashSet::new(),
            flagged_forced: HashSet::new(),
            feed_now: BTreeMap::new(),
            status_now: BTreeMap::new(),
            opened,
        })
    }

    // ------------------------------------------------------------------ helpers

    pub(crate) fn bump_epoch(&mut self) {
        self.f.bump();
    }

    pub fn is_flat(&self) -> bool {
        self.positions.is_empty()
    }

    /// D-F20 (decision 68): the account's size must be one of the rule set's tiers (the chain takes it from `tier_size[tier_index]`).
    pub fn check_opened(&mut self) {
        let ctx = Ctx { seq: None, idx: None, time: self.opened.time };
        if !self.rules.tier_sizes.contains(&self.opened.size_s) {
            self.f.add(FindingClass::StateMismatch, ctx, format!("one of the tier sizes {:?}", self.rules.tier_sizes), self.opened.size_s.to_string(), "opened.size_s is not a tier size of the rules file");
        }
    }

    /// D-F10/F11: every archive record the verifier will not use (a value that is not a positive price, a record out of the stream's
    /// order) is a finding of its own; it is skipped everywhere else, so it cannot hide anything behind it.
    pub fn check_archive(&mut self) {
        let ctx = Ctx { seq: None, idx: None, time: self.opened.time };
        let bad = self.arch.bad.clone();
        for (i, why) in bad {
            let (market, ts) = self.arch.samples.get(i).map(|s| (s.market, s.ts_sec())).unwrap_or((0, 0));
            self.f.add(FindingClass::ArchiveRecordInvalid, ctx, "a record in the stream's order with a positive price", format!("record {i} (market {market}, second {ts})"), format!("archive record {i} is skipped: {why}"));
        }
    }

    /// Validates the signatures and decoding of every intent line that the committed prefixes cover (all lines when no
    /// `Meta` commits a prefix).
    pub fn check_intents(&mut self, trader: &[u8; 32]) {
        let limit = if self.intent_prefix.is_empty() {
            self.intents.lines.len()
        } else {
            let max_bytes = self.intent_prefix.values().copied().max().unwrap_or(0);
            self.intents.lines_in_prefix(max_bytes)
        };
        let ctx = Ctx { seq: None, idx: None, time: self.opened.time };
        for (i, line) in self.intents.lines.iter().enumerate().take(limit) {
            if let Err(why) = &line.intent {
                self.f.add(FindingClass::IntentDecode, ctx, "a valid intent line", why.clone(), format!("intent line {} is unusable", i + 1));
                self.bad_sig_lines.insert(i);
                continue;
            }
            if line.nonce == 0 {
                self.f.add(FindingClass::IntentDecode, ctx, "nonce > 0", "0", format!("intent line {} uses the reserved nonce 0", i + 1));
                self.bad_sig_lines.insert(i);
                continue;
            }
            if !verify_signature(trader, &self.opened.account, line.nonce, &line.order, &line.sig) {
                self.f.add(FindingClass::BadIntentSignature, ctx, "a valid ed25519 signature by the trader", "invalid", format!("intent nonce {} is not signed by the trader", line.nonce));
                self.bad_sig_lines.insert(i);
            }
        }
        for &d in &self.intents.duplicates {
            if d < limit {
                let nonce = self.intents.lines.get(d).map(|l| l.nonce).unwrap_or(0);
                self.f.add(FindingClass::IntentDecode, ctx, "unique nonces", nonce.to_string(), format!("nonce {nonce} appears twice"));
            }
        }
    }

    /// P25b (F-24): the vault flipped the record to Abandoned: the account is over (no more events), whatever was open or pending.
    pub fn on_account_abandoned(&mut self) {
        self.terminal = true;
        self.quick_fail_due = false;
        self.bump_epoch();
    }

    // ------------------------------------------------------------------ admin market changes (P25b, F-19)

    /// The feed of `market` at the log position being replayed.
    pub(crate) fn feed_of(&self, market: u16) -> [u8; 32] {
        self.feed_now.get(&market).copied().or_else(|| self.rules.markets.get(&market).map(|m| m.feed_id)).unwrap_or([0; 32])
    }

    /// The status of `market` at the log position being replayed.
    pub(crate) fn status_of(&self, market: u16) -> u8 {
        self.status_now.get(&market).copied().or_else(|| self.rules.markets.get(&market).map(|m| m.status)).unwrap_or(0)
    }

    /// Before the replay: a market the log changes started with the OLD value of its first change (the rules file shows the end state).
    pub fn init_markets(&mut self, lines: &[crate::log::LineAt]) {
        for l in lines {
            match &l.line {
                crate::log::Line::MarketFeedChanged { market, old_feed, .. } => {
                    self.feed_now.entry(*market).or_insert(*old_feed);
                }
                crate::log::Line::MarketStatusChanged { market, old_status, .. } => {
                    self.status_now.entry(*market).or_insert(*old_status);
                }
                _ => {}
            }
        }
    }

    /// A `market_feed_changed` line: the change must follow the feed the market had; from here on proofs are judged against the new one.
    pub fn on_market_feed(&mut self, ctx: Ctx, market: u16, old: [u8; 32], new: [u8; 32]) {
        if !self.rules.markets.contains_key(&market) {
            self.f.add(FindingClass::UnknownMarket, ctx, "a market of the rules file", market.to_string(), "a feed change names a market the rules do not list");
            return;
        }
        let have = self.feed_of(market);
        if have != old {
            self.f.add(FindingClass::StateMismatch, ctx, crate::hexutil::encode(&have), crate::hexutil::encode(&old), format!("the feed change of market {market} does not start from the feed the market had"));
        }
        self.feed_now.insert(market, new);
    }

    /// A `market_status_changed` line (same chain rule as the feed).
    pub fn on_market_status(&mut self, ctx: Ctx, market: u16, old: u8, new: u8) {
        if !self.rules.markets.contains_key(&market) {
            self.f.add(FindingClass::UnknownMarket, ctx, "a market of the rules file", market.to_string(), "a status change names a market the rules do not list");
            return;
        }
        let have = self.status_of(market);
        if have != old {
            self.f.add(FindingClass::StateMismatch, ctx, have.to_string(), old.to_string(), format!("the status change of market {market} does not start from the status the market had"));
        }
        self.status_now.insert(market, new);
    }

    /// After the whole log: the rules file is the state at the END of the log, so it must show the last change of every market.
    pub fn check_markets_end_state(&mut self) {
        let ctx = Ctx { seq: None, idx: None, time: self.last_time };
        let feeds: Vec<(u16, [u8; 32])> = self.feed_now.iter().map(|(k, v)| (*k, *v)).collect();
        for (m, f) in feeds {
            let shown = self.rules.markets.get(&m).map(|x| x.feed_id);
            if shown != Some(f) {
                self.f.add(FindingClass::StateMismatch, ctx, crate::hexutil::encode(&f), shown.map(|x| crate::hexutil::encode(&x)).unwrap_or_default(), format!("the rules file does not show the last feed of market {m}"));
            }
        }
        let statuses: Vec<(u16, u8)> = self.status_now.iter().map(|(k, v)| (*k, *v)).collect();
        for (m, st) in statuses {
            let shown = self.rules.markets.get(&m).map(|x| x.status);
            if shown != Some(st) {
                self.f.add(FindingClass::StateMismatch, ctx, st.to_string(), shown.map(|x| x.to_string()).unwrap_or_default(), format!("the rules file does not show the last status of market {m}"));
            }
        }
    }

    // ------------------------------------------------------------------ proofs

    /// Checks a proof against the rules and the archive. `market` is the event's market, or `None` for floating events
    /// (then the market is the one whose feed the proof carries). Returns `(market, mid_e12)` when the proof is usable.
    fn check_proof(&mut self, ctx: Ctx, proof: &PriceProof, market: Option<u16>, ev_time: i64) -> Option<(u16, u64)> {
        if let Err(e) = check_signed_update_shape(proof) {
            self.f.add(FindingClass::ProofShape, ctx, "a well-formed signed_update", format!("{e:?}"), "proof shape");
            return None;
        }
        let m = match market {
            Some(id) => match self.rules.markets.get(&id) {
                Some(m) => m,
                None => {
                    self.f.add(FindingClass::UnknownMarket, ctx, "a market of the rules file", id.to_string(), "the event names a market the rules do not list");
                    return None;
                }
            },
            None => match self.rules.markets.values().find(|m| self.feed_of(m.id) == proof.feed_id) {
                Some(m) => m,
                None => {
                    self.f.add(FindingClass::ProofFeed, ctx, "the feed of a market of the rules file", crate::hexutil::encode(&proof.feed_id), "the proof's feed belongs to no market");
                    return None;
                }
            },
        };
        let (mid, mut mk) = (m.id, m.clone());
        mk.feed_id = self.feed_of(mid); // P25b (F-19): the feed as it stands at this log position
        match check_proof_without_price(proof, &mk.feed_id, u32::from(mk.max_age_secs), ev_time, false) {
            Ok(()) => {}
            Err(ProofError::WrongFeed) => {
                self.f.add(FindingClass::ProofFeed, ctx, crate::hexutil::encode(&mk.feed_id), crate::hexutil::encode(&proof.feed_id), format!("proof feed is not market {mid}'s feed"));
                return None;
            }
            Err(ProofError::TooOld) | Err(ProofError::InFuture) => {
                let pt = journal_types::proof_time_secs(proof);
                self.f.add(
                    FindingClass::ProofAge,
                    ctx,
                    format!("{}..={}", ev_time.saturating_sub(i64::from(mk.max_age_secs)), ev_time.saturating_add(2)),
                    pt.to_string(),
                    format!("proof time is outside the {} s window of market {mid}", mk.max_age_secs),
                );
                return None;
            }
            Err(e) => {
                self.f.add(FindingClass::ProofShape, ctx, "an accepted proof", format!("{e:?}"), "proof rejected by the shared proof check");
                return None;
            }
        }
        let mid_e12 = match journal_types::price_e12_from_quantized(proof.quantized_value) {
            Ok(v) => v,
            Err(e) => {
                self.f.add(FindingClass::ProofShape, ctx, "a positive price", format!("{e:?}"), "proof value");
                return None;
            }
        };
        // the proof must be a recorded sample
        match self.arch.find(mid, proof.timestamp_ns, proof.quantized_value) {
            Some(i) => {
                self.last_proof_si = Some(i);
                let s = self.arch.samples.get(i);
                let digest_ok = match s {
                    Some(s) => s.source == proof.source && (proof.source != 3 || proof.signed_update.as_slice() == s.digest.as_slice()),
                    None => false,
                };
                if !digest_ok {
                    self.f.add(FindingClass::ArchiveMissing, ctx, "an archive record with the proof's source and blob digest", "different source or digest", "the matching archive record disagrees with the proof");
                    return None;
                }
                if let (Some(seq), Some(idx)) = (ctx.seq, ctx.idx) {
                    self.batch.proofs.push((i, seq, idx));
                }
            }
            None => {
                self.f.add(
                    FindingClass::ArchiveMissing,
                    ctx,
                    format!("a record (market {mid}, ts {} ns, value {})", proof.timestamp_ns, proof.quantized_value),
                    "none",
                    "the proof is not in the sample archive",
                );
                return None;
            }
        }
        Some((mid, mid_e12))
    }

    fn proof_for(&mut self, ctx: Ctx, ev: &JournalEvent, proof: &Option<PriceProof>, market: Option<u16>, time: i64) -> Option<(u16, u64)> {
        self.last_proof_si = None;
        match (ev.proof_ref(), proof) {
            (Some(p), Some(pr)) if p != journal_types::NO_PROOF => self.check_proof(ctx, pr, market, time),
            _ => {
                self.f.add(FindingClass::ProofShape, ctx, "a price proof", "none", "this event must carry a proof");
                None
            }
        }
    }

    // ------------------------------------------------------------------ fees

    /// Called before every event with the event's time: generates the holding-fee expectations of the rollovers that
    /// passed, then expires the expectations older than `time`.
    pub fn before_event(&mut self, ctx: Ctx, time: i64) {
        let cursor = self.roll_cursor.unwrap_or(self.opened.time);
        if !self.positions.is_empty() {
            let nights = engine::day_index(time).saturating_sub(engine::day_index(cursor));
            if nights > MAX_ROLLOVER_GAP {
                // an account that holds positions across more nights than any journal covers: do not loop over them
                self.f.add(FindingClass::FeeMismatch, ctx, format!("at most {MAX_ROLLOVER_GAP} nights between two events"), nights.to_string(), "too many holding-fee nights between two events to check");
                for p in self.positions.values_mut() {
                    p.p.fee_day = engine::day_index(time);
                }
            } else {
                let mut r = next_rollover_after(cursor);
                // `r > cursor` is the progress test: near i64::MAX `next_rollover_after` saturates and would return the same
                // instant for ever (M12 finding V-12)
                while r > cursor && r <= time && !self.positions.is_empty() {
                    self.holding_expectations(ctx, r);
                    let next = next_rollover_after(r);
                    if next <= r {
                        break;
                    }
                    r = next;
                }
            }
        }
        self.roll_cursor = Some(cursor.max(time));
        self.expire_fees(ctx, time);
    }

    fn holding_expectations(&mut self, ctx: Ctx, r: i64) {
        let today = engine::day_index(r);
        let list: Vec<(u16, Pos)> = self.positions.iter().map(|(k, v)| (*k, *v)).collect();
        for (market, pos) in list {
            if pos.p.fee_day >= today {
                continue;
            }
            let Some(class) = self.rules.class_of(market) else { continue };
            let rate = self.rules.holding_fee_e6.get(class).copied().unwrap_or(0);
            let Some(si) = self.arch.last_at_or_before(market, r) else {
                self.f.add(FindingClass::ArchiveMissing, ctx, format!("a market {market} sample at or before {r}"), "none", "no mark for the holding fee");
                continue;
            };
            let Some(mid) = self.arch.samples.get(si).and_then(|s| s.mid_e12()) else { continue };
            let Some((bid, ask)) = mid_pair(mid, self.rules.spread(market)) else { continue };
            let mark = if pos.p.qty.is_positive() { bid } else { ask };
            match engine::holding_fee_micro(pos.p.qty.unsigned_abs(), mark, rate, 1) {
                Ok(amount) => {
                    if rate > 0 {
                        self.fee_exp.push(FeeExp { kind: FeeExpKind::Holding, market, time: r, amount, why: format!("holding fee, night of index {today}") });
                    }
                }
                Err(_) => {
                    self.f.add(FindingClass::FeeMismatch, ctx, "a computable holding fee", "overflow", "holding fee overflow");
                }
            }
            if let Some(p) = self.positions.get_mut(&market) {
                p.p.fee_day = today;
            }
        }
    }

    fn expire_fees(&mut self, ctx: Ctx, time: i64) {
        let mut keep = Vec::new();
        let old = std::mem::take(&mut self.fee_exp);
        for e in old {
            if e.time < time {
                self.f.add(
                    FindingClass::FeeMismatch,
                    Ctx { seq: ctx.seq, idx: ctx.idx, time },
                    format!("{:?} fee of {} on market {} at {}", e.kind, e.amount, e.market, e.time),
                    "missing",
                    format!("{} is missing from the journal", e.why),
                );
            } else {
                keep.push(e);
            }
        }
        self.fee_exp = keep;
    }

    /// End of the replay: everything still expected is missing.
    pub fn flush_fees(&mut self) {
        let ctx = Ctx { seq: Some(self.last_seq), idx: None, time: self.last_time };
        let old = std::mem::take(&mut self.fee_exp);
        for e in old {
            self.f.add(
                FindingClass::FeeMismatch,
                ctx,
                format!("{:?} fee of {} on market {} at {}", e.kind, e.amount, e.market, e.time),
                "missing",
                format!("{} is missing from the journal", e.why),
            );
        }
    }

    fn commission_expectation(&mut self, market: u16, qty: u64, price: u64, time: i64, what: &str) {
        let Some(class) = self.rules.class_of(market) else { return };
        let rate = self.rules.commission_e6.get(class).copied().unwrap_or(0);
        if rate == 0 {
            return;
        }
        let Ok(notional) = engine::margin_micro(qty, price, 1) else { return };
        if let Ok(amount) = engine::fee_ceil_micro(notional, rate) {
            self.fee_exp.push(FeeExp { kind: FeeExpKind::Commission, market, time, amount, why: format!("commission of the {what}") });
        }
    }

    fn debit(&mut self, ctx: Ctx, amount: i64) {
        match engine::apply_closed_slice(&self.st, &self.engine_rules, amount.saturating_neg(), 0) {
            Ok(d) => self.st = d.state,
            Err(_) => self.f.add(FindingClass::StateMismatch, ctx, "a balance that fits i64", "overflow", "fee overflow"),
        }
    }

    // ------------------------------------------------------------------ intents

    #[allow(clippy::too_many_arguments)]
    fn check_intent(&mut self, ctx: Ctx, nonce: u32, market: u16, qty: u64, time: i64, what: IntentUse) {
        match what {
            IntentUse::Close { reason, .. } => {
                let evaluator = matches!(reason, CloseReason::StopLoss | CloseReason::TakeProfit | CloseReason::Breach | CloseReason::Fail);
                if evaluator && nonce != 0 {
                    self.f.add(FindingClass::ForgedTrade, ctx, "intent_id 0 (an evaluator-originated close)", nonce.to_string(), format!("a {reason:?} close must not claim a trader intent"));
                    return;
                }
                if !evaluator && nonce == 0 {
                    self.f.add(FindingClass::ForgedTrade, ctx, "a signed intent", "intent_id 0", format!("a {reason:?} close needs the trader's signed intent"));
                    return;
                }
                if nonce == 0 {
                    return;
                }
            }
            IntentUse::Fill { .. } => {
                if nonce == 0 {
                    self.f.add(FindingClass::ForgedTrade, ctx, "a signed intent", "intent_id 0", "a fill needs the trader's signed intent");
                    return;
                }
            }
        }
        let intents = self.intents;
        let Some((line_idx, line)) = intents.get(nonce) else {
            self.f.add(FindingClass::ForgedTrade, ctx, format!("intent nonce {nonce} in the intents file"), "absent", "no signed intent with this nonce");
            return;
        };
        if self.bad_sig_lines.contains(&line_idx) {
            return; // already reported as BadIntentSignature / IntentDecode
        }
        let Ok(intent) = line.intent.clone() else { return };
        self.batch.intents.push((line_idx, ctx.seq.unwrap_or(0), ctx.idx.unwrap_or(0)));
        let mut bad = |why: String, exp: String, found: String| self.f.add(FindingClass::ForgedTrade, ctx, exp, found, why);
        if intent.market != market {
            bad(format!("intent {nonce} is for market {}", intent.market), market.to_string(), intent.market.to_string());
            return;
        }
        if self.closed_nonces.contains(&nonce) {
            bad(format!("intent {nonce} was already executed by an earlier batch"), "an intent used once".into(), "a second batch under the same nonce".into());
            return;
        }
        // D-F14/D-F18: an order that executes at once (market, close) is executed in the second the evaluator received it, which is
        // within ORDER_WINDOW_SECS of the time the trader signed; a pending order (limit, stop) is placed before it fills
        let immediate = matches!(intent.kind, IntentKind::Market | IntentKind::Close);
        if immediate && (time < intent.client_time.saturating_sub(ORDER_WINDOW_SECS) || time > intent.client_time.saturating_add(ORDER_WINDOW_SECS)) {
            bad(
                format!("intent {nonce} was signed at {} and the trade is at {time}: outside its {ORDER_WINDOW_SECS} s window", intent.client_time),
                format!("{}..={}", intent.client_time.saturating_sub(ORDER_WINDOW_SECS), intent.client_time.saturating_add(ORDER_WINDOW_SECS)),
                time.to_string(),
            );
            return;
        }
        if !immediate && intent.client_time > time {
            bad(format!("intent {nonce} is dated after the trade"), format!("<= {time}"), intent.client_time.to_string());
            return;
        }
        let mut total: Option<u64> = Some(intent.qty);
        match what {
            IntentUse::Fill { side, order_kind, sl, tp } => {
                let kind_ok = matches!(
                    (order_kind, intent.kind),
                    (OrderKind::Market, IntentKind::Market) | (OrderKind::Limit, IntentKind::Limit) | (OrderKind::Stop, IntentKind::Stop)
                );
                if !kind_ok {
                    bad(format!("intent {nonce} is a {:?} order, the fill says {order_kind:?}", intent.kind), format!("{order_kind:?}"), format!("{:?}", intent.kind));
                    return;
                }
                if intent.side != side.as_u8() {
                    bad(format!("intent {nonce} is the other side"), side_name(side).to_string(), if intent.side == 0 { "long".into() } else { "short".into() });
                    return;
                }
                if intent.sl_e12 != sl || intent.tp_e12 != tp {
                    bad(format!("intent {nonce} carries other protective levels"), format!("sl {} tp {}", intent.sl_e12, intent.tp_e12), format!("sl {sl} tp {tp}"));
                    return;
                }
            }
            IntentUse::Close { position_side, .. } => match intent.kind {
                IntentKind::Market | IntentKind::Limit | IntentKind::Stop => {
                    if intent.side == position_side.as_u8() {
                        bad(format!("intent {nonce} adds to the position instead of closing it"), "the opposite side".into(), "the same side".into());
                        return;
                    }
                }
                IntentKind::Close => {
                    if intent.qty == 0 {
                        total = None;
                    }
                }
                IntentKind::Cancel | IntentKind::SetSlTp => {
                    bad(format!("intent {nonce} is a {:?}, not an order", intent.kind), "an order or close intent".into(), format!("{:?}", intent.kind));
                    return;
                }
            },
        }
        let entry = self.batch.nonce_qty.entry(nonce).or_insert((0, total));
        entry.0 = entry.0.saturating_add(qty);
        if let Some(t) = entry.1 {
            if entry.0 > t {
                let (got, want) = (entry.0, t);
                self.f.add(FindingClass::ForgedTrade, ctx, format!("at most {want}"), got.to_string(), format!("trades under intent {nonce} exceed its quantity"));
            }
        }
    }

    // ------------------------------------------------------------------ events

    /// Applies one log event line. `proof` is the decoded proof of the line, if any.
    pub fn on_event_line(&mut self, seq: u64, ev_bytes: &[u8], proof_bytes: &[u8]) {
        self.event_count += 1;
        let idx = self.event_count - 1;
        let ctx0 = Ctx { seq: Some(seq), idx: Some(idx), time: self.last_time };
        let prev_seq = self.last_seq;
        if seq != self.expect_seq {
            self.f.add(FindingClass::SequenceGap, ctx0, self.expect_seq.to_string(), seq.to_string(), "the chain sequence is not contiguous (dropped, repeated or reordered event)");
        }
        self.expect_seq = seq.saturating_add(1);
        let ev = match JournalEvent::decode(ev_bytes) {
            Ok(e) => e,
            Err(e) => {
                self.f.add(FindingClass::EventDecode, ctx0, "a canonical journal event", format!("{e:?}"), "event bytes do not decode");
                return;
            }
        };
        let proof = if proof_bytes.is_empty() {
            None
        } else {
            match PriceProof::decode(proof_bytes) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.f.add(FindingClass::EventDecode, ctx0, "a canonical price proof", format!("{e:?}"), "proof bytes do not decode");
                    return;
                }
            }
        };
        // hash chain (events whose proof reference and bytes disagree cannot be hashed)
        match next_hash(&NativeSha256, &self.head, seq, &ev, proof.as_ref()) {
            Ok(h) => self.head = h,
            Err(_) => self.f.add(FindingClass::ProofShape, ctx0, "a proof exactly when the event references one", if proof.is_some() { "an unreferenced proof" } else { "no proof" }, "proof presence disagrees with the event"),
        }
        self.last_seq = seq;
        let time = ev.time();
        let ctx = Ctx { seq: Some(seq), idx: Some(idx), time };
        if self.terminal && !matches!(ev, JournalEvent::Meta { .. }) {
            self.f.add(FindingClass::StateMismatch, ctx, "no events after the account failed", "an event", "the account is already terminal");
        }
        // P25b (F-26): the program checks every event of a batch against the previous one, `Meta` included, and stores the Meta time
        // as `last_batch_time`, so the verifier's clock follows the Meta times too
        if time < self.last_time {
            self.f.add(FindingClass::StateMismatch, ctx, format!(">= {}", self.last_time), time.to_string(), "event time goes backwards");
        }
        if let Some(ph) = self.phases.last_mut() {
            ph.first_seq.get_or_insert(seq);
            ph.last_seq = Some(seq);
            ph.events += 1;
        }
        self.in_batch_events += 1;
        self.before_event(ctx, time);
        // the archive walk: a holding fee is dated at the rollover and the samples of that second are judged AFTER it
        let walk_to = match &ev {
            JournalEvent::Fee { kind: FeeKind::Holding, time, .. } => time.saturating_sub(1),
            _ => time,
        };
        self.walk_advance(walk_to, prev_seq);
        self.max_time = self.max_time.max(time);
        if self.quick_fail_due && !self.terminal && !matches!(ev, JournalEvent::Fail { .. } | JournalEvent::Meta { .. } | JournalEvent::Fee { kind: FeeKind::Commission, .. }) {
            self.f.add_once(FindingClass::MissedFail, ctx, "a Fail{PhaseQuick} right after the closing slice", "another event", "the quick tally is above 0.028 S and the account was not failed");
            self.quick_fail_due = false;
        }
        self.apply(ctx, &ev, &proof);
        self.last_time = self.last_time.max(time);
    }

    fn apply(&mut self, ctx: Ctx, ev: &JournalEvent, proof: &Option<PriceProof>) {
        match ev {
            JournalEvent::Fill { market, side, order_kind, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, .. } => {
                self.on_fill(ctx, ev, proof, *market, *side, *order_kind, *qty, *price_e12, *time, *avg_entry_after, *margin, *sl, *tp, *intent_id)
            }
            JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason, intent_id, .. } => {
                self.on_close(ctx, ev, proof, *market, *side, *qty, *entry_e12, *exit_e12, *time, *timer_start, *pnl, *quick, *credited, *reason, *intent_id)
            }
            JournalEvent::Fee { kind, market, amount, time } => self.on_fee(ctx, *kind, *market, *amount, *time),
            JournalEvent::TightStart { time, p_open, r, l_start, g_peak, l_level, .. } => {
                self.floating_proof(ctx, ev, proof, *time);
                self.on_tight_start(ctx, *p_open, *r, *l_start, *g_peak, *l_level);
            }
            JournalEvent::FloorRaised { time, g_peak, l_level, .. } => {
                self.floating_proof(ctx, ev, proof, *time);
                self.on_floor_raised(ctx, *g_peak, *l_level);
            }
            JournalEvent::TightEnd { .. } => self.on_tight_end(ctx),
            JournalEvent::Breach { time, l_level, b_after, .. } => {
                self.floating_proof(ctx, ev, proof, *time);
                self.on_breach(ctx, *time, *l_level, *b_after);
            }
            JournalEvent::Fail { time, reason, b_after, .. } => {
                self.floating_proof(ctx, ev, proof, *time);
                self.on_fail(ctx, *reason, *b_after);
            }
            JournalEvent::Meta { flat, samples_hash, intents_hash, engine_version, .. } => self.on_meta(ctx, *flat, samples_hash, intents_hash, *engine_version),
        }
    }

    /// D-F08: the archive sample a market order of `market` executed in second `time` must be priced with: the newest sample of the
    /// market at or before that second (the rule of DESIGN 4.5; the evaluator follows it).
    fn market_sample(&self, market: u16, time: i64) -> Option<usize> {
        self.arch.last_at_or_before(market, time)
    }

    /// D-F08/D-F12: the sample the event's proof used must be the one the rule names. `false` when it is not.
    fn check_sample(&mut self, ctx: Ctx, market: u16, want: Option<usize>, what: &str) -> bool {
        let (Some(want), Some(got)) = (want, self.last_proof_si) else { return true };
        if want == got {
            return true;
        }
        // a proof carries no record index: two records with the same market, source timestamp, value and digest are the same price
        // (a duplicate tick), whichever of them `Archive::find` returns
        if let (Some(w), Some(g)) = (self.arch.samples.get(want), self.arch.samples.get(got)) {
            if (w.market, w.source, w.ts_ns, w.value, w.digest) == (g.market, g.source, g.ts_ns, g.value, g.digest) {
                return true;
            }
        }
        let describe = |i: usize| self.arch.samples.get(i).map(|s| format!("record {i} (second {}, ts {} ns)", s.ts_sec(), s.ts_ns)).unwrap_or_else(|| format!("record {i}"));
        // the order of two records: by source timestamp, then by place in the stream (`got != want`, so never equal)
        let how = match (self.arch.samples.get(got), self.arch.samples.get(want)) {
            (Some(g), Some(w)) => match (g.ts_ns, got).cmp(&(w.ts_ns, want)) {
                std::cmp::Ordering::Less => "an OLDER sample than the rule names",
                _ => "a NEWER sample than the rule names",
            },
            _ => "another sample than the rule names",
        };
        self.f.add(FindingClass::PriceSampleMismatch, ctx, describe(want), describe(got), format!("the {what} in market {market} is priced with {how}"));
        false
    }

    /// D-F13: a forced close no floor decision of the archive supports. A `Breach` close is flagged at once; a `Fail` close may be one
    /// of the others closed before a quick-limit overshoot, which only the `Fail{PhaseQuick}` that follows can justify.
    pub(crate) fn unjustified_forced_close(&mut self, ctx: Ctx, market: u16, reason: CloseReason) {
        if reason == CloseReason::Fail {
            self.forced_pending.push((ctx, market));
        } else {
            self.flagged_forced.insert(ctx.time);
            self.f.add(FindingClass::ForcedCloseUnjustified, ctx, "a Breach decision of the samples of this second", format!("Close{{{reason:?}}} in market {market}"), "a forced close that no sample of this second calls for");
        }
    }

    /// The pending `Close{Fail}` events are justified by a `Fail{PhaseQuick}` of their own second; any others are findings.
    fn settle_forced_closes(&mut self, fail_time: Option<i64>) {
        let list = std::mem::take(&mut self.forced_pending);
        for (c, market) in list {
            if fail_time == Some(c.time) {
                continue;
            }
            self.flagged_forced.insert(c.time);
            self.f.add(FindingClass::ForcedCloseUnjustified, c, "a Fail decision of the samples of this second, or a quick-limit Fail right after", format!("Close{{Fail}} in market {market}"), "a forced close that no sample of this second calls for and no quick-limit Fail followed");
        }
    }

    fn floating_proof(&mut self, ctx: Ctx, ev: &JournalEvent, proof: &Option<PriceProof>, time: i64) {
        let _ = self.proof_for(ctx, ev, proof, None, time);
    }

    #[allow(clippy::too_many_arguments)]
    fn on_fill(&mut self, ctx: Ctx, ev: &JournalEvent, proof: &Option<PriceProof>, market: u16, side: Side, kind: OrderKind, qty: u64, price: u64, time: i64, avg_after: u64, margin: u64, sl: u64, tp: u64, intent: u32) {
        if !self.rules.markets.contains_key(&market) {
            self.f.add(FindingClass::UnknownMarket, ctx, "a market of the rules file", market.to_string(), "the Fill names a market the rules do not list");
            return;
        }
        let priced = self.proof_for(ctx, ev, proof, Some(market), time);
        if let Some((_, mid)) = priced {
            let long = side == Side::Long;
            match engine::fill_price_e12(mid, self.rules.spread(market), long) {
                Ok(expected) if expected != price => {
                    self.f.add(FindingClass::PriceMismatch, ctx, expected.to_string(), price.to_string(), format!("fill price vs proof mid {mid} and the {} half-spread", self.rules.spread(market)));
                }
                Ok(_) => {}
                Err(_) => self.f.add(FindingClass::PriceMismatch, ctx, "a computable price", "overflow", "fill price overflow"),
            }
        }
        self.check_intent(ctx, intent, market, qty, time, IntentUse::Fill { side, order_kind: kind, sl, tp });
        let trigger = self.walk_fill(ctx, market, kind, intent);
        let want = if kind == OrderKind::Market { self.market_sample(market, time) } else { trigger };
        self.check_sample(ctx, market, want, "fill");
        // M9c: a market order the evaluator's own rules refuse (margin, caps, status, lock, closed market). The opening half of
        // a flip was judged with its `Close{Flip}`.
        if kind == OrderKind::Market && priced.is_some() && !self.flip_checked.remove(&(time, intent)) {
            self.check_market_order(ctx, market, side == Side::Long, qty, price, time);
        }
        let existing = self.positions.get(&market).copied();
        let out = match engine::apply_fill(existing.map(|p| p.p), eside(side), qty, price, time, engine::day_index(time)) {
            Ok(o) => o,
            Err(e) => {
                self.f.add(FindingClass::EventDecode, ctx, "a valid fill", format!("{e:?}"), "the engine refuses this fill");
                return;
            }
        };
        if out.closed.is_some() {
            self.f.add(FindingClass::PositionMismatch, ctx, "a Fill that only opens or adds", "a Fill against an opposite position", "a reducing fill must be a Close event");
        }
        if let Some(np) = out.position {
            if np.avg_entry_e12 != avg_after {
                self.f.add(FindingClass::AvgEntryMismatch, ctx, np.avg_entry_e12.to_string(), avg_after.to_string(), "average entry after the fill");
            }
            let lev = self.rules.class_of(market).and_then(|c| self.rules.leverage_x.get(c).copied()).unwrap_or(1);
            match engine::margin_micro(np.qty.unsigned_abs(), np.avg_entry_e12, lev) {
                Ok(m) => {
                    if u64::try_from(m).ok() != Some(margin) {
                        self.f.add(FindingClass::MarginMismatch, ctx, m.to_string(), margin.to_string(), "margin of the whole position after the fill");
                    }
                }
                Err(_) => self.f.add(FindingClass::MarginMismatch, ctx, "a computable margin", "overflow", "margin overflow"),
            }
            let (osl, otp) = existing.map(|p| (p.sl, p.tp)).filter(|_| out.closed.is_none()).unwrap_or((0, 0));
            self.positions.insert(market, Pos { p: np, sl: if sl != 0 { sl } else { osl }, tp: if tp != 0 { tp } else { otp } });
        } else {
            self.positions.remove(&market);
        }
        self.commission_expectation(market, qty, price, time, "fill");
        self.bump_epoch();
    }

    #[allow(clippy::too_many_arguments)]
    fn on_close(&mut self, ctx: Ctx, ev: &JournalEvent, proof: &Option<PriceProof>, market: u16, side: Side, qty: u64, entry: u64, exit: u64, time: i64, timer_start: i64, pnl: i64, quick: bool, credited: i64, reason: CloseReason, intent: u32) {
        if !self.rules.markets.contains_key(&market) {
            self.f.add(FindingClass::UnknownMarket, ctx, "a market of the rules file", market.to_string(), "the Close names a market the rules do not list");
            return;
        }
        let priced = self.proof_for(ctx, ev, proof, Some(market), time);
        if let Some((_, mid)) = priced {
            match engine::close_price_e12(mid, self.rules.spread(market), side == Side::Long) {
                Ok(expected) if expected != exit => {
                    self.f.add(FindingClass::PriceMismatch, ctx, expected.to_string(), exit.to_string(), format!("close price vs proof mid {mid} and the {} half-spread", self.rules.spread(market)));
                }
                Ok(_) => {}
                Err(_) => self.f.add(FindingClass::PriceMismatch, ctx, "a computable price", "overflow", "close price overflow"),
            }
        }
        self.check_intent(ctx, intent, market, qty, time, IntentUse::Close { reason, position_side: side });
        let walked = self.walk_close(ctx, market, reason, intent, qty);
        // a Manual or Flip close is either the execution of a pending order (the walk named its trigger sample) or of a market/close
        // intent (the newest sample at or before the second); the other reasons are priced by what the walk named
        let want = match reason {
            CloseReason::Manual | CloseReason::Flip => walked.or_else(|| self.market_intent(intent, market).and_then(|_| self.market_sample(market, time))),
            _ => walked,
        };
        if !self.check_sample(ctx, market, want, "close") && matches!(reason, CloseReason::Breach | CloseReason::Fail) {
            self.flagged_forced.insert(time);
        }
        // M9c: the execution of a MARKET order that reduces or flips: the market must be open; a flip is judged as a whole
        // here (the evaluator checks margin, caps and lock before it books the closing slice)
        if priced.is_some() && matches!(reason, CloseReason::Manual | CloseReason::Flip) {
            if let Some(it) = self.market_intent(intent, market) {
                if reason == CloseReason::Flip {
                    self.flip_checked.retain(|(t, _)| *t >= time);
                    self.flip_checked.insert((time, intent));
                    self.check_market_order(ctx, market, it.side == 0, it.qty, exit, time);
                } else {
                    self.check_market_close(ctx, market, time, it.qty);
                }
            }
        }
        let Some(pos) = self.positions.get(&market).copied() else {
            self.f.add(FindingClass::PositionMismatch, ctx, "an open position in this market", "none", "Close without a position");
            return;
        };
        let pos_side = if pos.p.qty.is_positive() { Side::Long } else { Side::Short };
        if pos_side != side {
            self.f.add(FindingClass::PositionMismatch, ctx, side_name(pos_side), side_name(side), "Close names the other side than the open position");
            return;
        }
        if qty == 0 || qty > pos.p.qty.unsigned_abs() {
            self.f.add(FindingClass::PositionMismatch, ctx, format!("1..={}", pos.p.qty.unsigned_abs()), qty.to_string(), "Close quantity exceeds the position");
            return;
        }
        if entry != pos.p.avg_entry_e12 {
            self.f.add(FindingClass::AvgEntryMismatch, ctx, pos.p.avg_entry_e12.to_string(), entry.to_string(), "entry price of the closed slice");
        }
        if timer_start != pos.p.timer_start {
            self.f.add(FindingClass::QuickMismatch, ctx, pos.p.timer_start.to_string(), timer_start.to_string(), "timer start of the closed position");
        }
        let out = match engine::apply_fill(Some(pos.p), opposite(side), qty, exit, time, engine::day_index(time)) {
            Ok(o) => o,
            Err(e) => {
                self.f.add(FindingClass::EventDecode, ctx, "a valid close", format!("{e:?}"), "the engine refuses this close");
                return;
            }
        };
        let Some(slice) = out.closed else { return };
        // the claims the program checks, against the REPLAYED position
        let claim = engine::CloseClaim { long: side == Side::Long, qty, entry_e12: pos.p.avg_entry_e12, exit_e12: exit, time, timer_start: pos.p.timer_start, pnl, quick, credited };
        if let Err(e) = engine::check_close(&claim) {
            let class = match e {
                engine::ClaimError::PnlMismatch => FindingClass::PnlMismatch,
                engine::ClaimError::CreditMismatch => FindingClass::CreditMismatch,
                engine::ClaimError::QuickMismatch | engine::ClaimError::TimerAfterTime => FindingClass::QuickMismatch,
                _ => FindingClass::EventDecode,
            };
            let (exp, found) = match class {
                FindingClass::PnlMismatch => (slice.pnl.to_string(), pnl.to_string()),
                FindingClass::CreditMismatch => (slice.credit.to_string(), credited.to_string()),
                FindingClass::QuickMismatch => (slice.quick.to_string(), quick.to_string()),
                _ => ("a valid close".to_string(), format!("{e:?}")),
            };
            self.f.add(class, ctx, exp, found, format!("close claim: {e:?}"));
        }
        // the balance moves by what the ENGINE computed
        let mut over = false;
        match engine::apply_closed_slice(&self.st, &self.engine_rules, slice.credit, out.quick_tally_delta) {
            Ok(d) => {
                self.st = d.state;
                if d.fail.is_some() {
                    self.quick_fail_due = true;
                    over = true;
                }
            }
            Err(_) => self.f.add(FindingClass::StateMismatch, ctx, "a balance that fits i64", "overflow", "credit overflow"),
        }
        if over && intent != 0 {
            // M9b decision D2: the slice that pushes the quick tally over its limit ends the phase. Nothing but the `Fail` may
            // follow it (DESIGN I-8), so a flip's opening remainder is never executed: the trades under this intent end here.
            if let Some(e) = self.batch.nonce_qty.get_mut(&intent) {
                e.1 = None;
            }
        }
        self.trades_closed = self.trades_closed.saturating_add(1);
        match out.position {
            Some(np) => {
                self.positions.insert(market, Pos { p: np, sl: pos.sl, tp: pos.tp });
            }
            None => {
                self.positions.remove(&market);
            }
        }
        // D2: the overshooting slice carries no commission `Fee` (the program allows only the `Fail` after it)
        if !over {
            self.commission_expectation(market, qty, exit, time, "close");
        }
        self.bump_epoch();
    }

    /// M9c: `hold_account` / `release_hold`. A hold on a held account and a release of one that is not held are refused by the
    /// program, so they are findings; a hold line after the account ended changes nothing.
    pub fn on_hold(&mut self, ctx: Ctx, time: i64, held: bool) {
        let open = self.holds.last().is_some_and(|(_, to)| to.is_none());
        if held {
            if open {
                self.f.add(FindingClass::StateMismatch, ctx, "an account that is not held", "a second account_held line", "hold_account on an account that is already held");
                return;
            }
            self.holds.push((time, None));
        } else {
            let Some(last) = self.holds.last_mut().filter(|(_, to)| to.is_none()) else {
                self.f.add(FindingClass::StateMismatch, ctx, "a held account", "a hold_released line", "release_hold on an account that is not held");
                return;
            };
            last.1 = Some(time);
        }
    }

    /// Whether the account is held during second `sec`: from the second of the `account_held` line on, up to (excluding) the
    /// second of the `hold_released` line.
    pub fn held_at(&self, sec: i64) -> bool {
        self.holds.iter().any(|(from, to)| sec >= *from && to.is_none_or(|t| sec < t))
    }

    fn on_fee(&mut self, ctx: Ctx, kind: FeeKind, market: u16, amount: u64, time: i64) {
        let want = match kind {
            FeeKind::Holding => FeeExpKind::Holding,
            FeeKind::Commission => FeeExpKind::Commission,
        };
        let pos = self.fee_exp.iter().position(|e| e.kind == want && e.market == market && e.time == time);
        match pos {
            Some(i) => {
                let e = self.fee_exp.remove(i);
                if i64::try_from(amount).ok() != Some(e.amount) {
                    self.f.add(FindingClass::FeeMismatch, ctx, e.amount.to_string(), amount.to_string(), format!("{} has the wrong amount", e.why));
                }
            }
            None => {
                self.f.add(
                    FindingClass::FeeMismatch,
                    ctx,
                    "no such fee",
                    format!("{kind:?} fee of {amount} on market {market} at {time}"),
                    "a fee the rules do not call for (flat day, wrong time or wrong market)",
                );
            }
        }
        match i64::try_from(amount) {
            Ok(a) => self.debit(ctx, a),
            Err(_) => self.f.add(FindingClass::FeeMismatch, ctx, "an amount that fits i64", amount.to_string(), "fee amount"),
        }
        self.bump_epoch();
    }

    fn on_tight_start(&mut self, ctx: Ctx, p_open: i64, r: i64, l_start: i64, g_peak: i64, l_level: i64) {
        let claims = engine::check_tight_start(&self.st, p_open, r, l_start, g_peak, l_level);
        let exp = self.walk_floor(ctx, FloorKind::TightStart, [p_open, r, l_start, g_peak, l_level], claims.is_ok());
        match claims {
            Ok(ns) => self.st = ns,
            Err(e) => {
                self.f.add(FindingClass::FloorMismatch, ctx, "a TightStart that follows the formulas", format!("{e:?}"), "tight start claim");
                // a wrong claim must not poison what follows: continue from the numbers the samples give, if there are any
                let v = exp.unwrap_or([p_open, r, l_start, g_peak, l_level]);
                self.st = AccountState { tight_on: true, r: v[1], l_start: v[2], g_peak: v[3], l_level: v[4], ..self.st };
            }
        }
        self.bump_epoch();
    }

    fn on_floor_raised(&mut self, ctx: Ctx, g_peak: i64, l_level: i64) {
        let claims = engine::check_floor_raised(&self.st, g_peak, l_level);
        let exp = self.walk_floor(ctx, FloorKind::FloorRaised, [0, 0, 0, g_peak, l_level], claims.is_ok());
        match claims {
            Ok(ns) => self.st = ns,
            Err(e) => {
                self.f.add(FindingClass::FloorMismatch, ctx, "a FloorRaised that follows the formulas", format!("{e:?}"), "floor raise claim");
                let v = exp.unwrap_or([0, 0, 0, g_peak, l_level]);
                self.st = AccountState { g_peak: v[3], l_level: v[4], ..self.st };
            }
        }
        self.bump_epoch();
    }

    fn on_tight_end(&mut self, ctx: Ctx) {
        if !self.is_flat() {
            self.f.add(FindingClass::FloorMismatch, ctx, "flat", format!("{} open positions", self.positions.len()), "TightEnd with open positions");
        }
        match engine::check_tight_end(&self.st) {
            Ok(ns) => self.st = ns,
            Err(e) => self.f.add(FindingClass::FloorMismatch, ctx, "tight mode on", format!("{e:?}"), "TightEnd claim"),
        }
        self.bump_epoch();
    }

    fn on_breach(&mut self, ctx: Ctx, time: i64, l_level: i64, b_after: i64) {
        let _ = self.walk_floor(ctx, FloorKind::Breach, [0, 0, 0, 0, l_level], true);
        if !self.is_flat() {
            self.f.add(FindingClass::FloorMismatch, ctx, "every position closed before the Breach", format!("{} open positions", self.positions.len()), "Breach with open positions");
        }
        match engine::check_breach(&self.st, time, l_level, b_after) {
            Ok(ns) => self.st = ns,
            Err(e) => {
                self.f.add(FindingClass::FloorMismatch, ctx, "a Breach that follows the formulas", format!("{e:?}"), "breach claim");
                self.st = engine::end_tight_if_flat(&self.st, true);
                self.st.lock_until = time.saturating_add(engine::consts::LOCK_SECS);
            }
        }
        self.bump_epoch();
    }

    pub(crate) fn on_fail(&mut self, ctx: Ctx, reason: FailReason, b_after: i64) {
        let _ = self.walk_floor(ctx, FloorKind::Fail(reason), [0, 0, 0, 0, 0], true);
        if b_after != self.st.b {
            self.f.add(FindingClass::FloorMismatch, ctx, self.st.b.to_string(), b_after.to_string(), "Fail.b_after is not the closed balance");
        }
        let justified = match reason {
            FailReason::StaticFloor => self.is_flat() && engine::static_floor_micro(&self.st).map(|f| self.st.b <= f).unwrap_or(false),
            FailReason::PhaseQuick => engine::apply_closed_slice(&self.st, &self.engine_rules, 0, 0).map(|d| d.fail.is_some()).unwrap_or(false),
        };
        if !justified {
            self.f.add(FindingClass::FalseFail, ctx, format!("{reason:?} conditions"), format!("b {} q {}", self.st.b, self.st.q_tally), "the account state does not justify this Fail");
        }
        // D-F13/D-F09: forced closes of this second that a quick-limit Fail justifies are settled; the others were flagged
        let quick_fail = (reason == FailReason::PhaseQuick).then_some(ctx.time);
        self.settle_forced_closes(quick_fail);
        if self.flagged_forced.contains(&ctx.time) {
            self.f.add(FindingClass::FalseFailMarks, ctx, "closes priced with the simultaneous samples of one decision", "forced closes that no decision of the archive supports", "the Fail rests on marks that are not simultaneous archive samples");
        }
        self.terminal = true;
        self.quick_fail_due = false;
        self.bump_epoch();
    }

    fn on_meta(&mut self, ctx: Ctx, flat: bool, samples_hash: &[u8; 32], intents_hash: &[u8; 32], engine_version: u16) {
        if engine_version != engine::ENGINE_VERSION {
            self.f.add(FindingClass::EngineVersion, ctx, engine::ENGINE_VERSION.to_string(), engine_version.to_string(), "Meta.engine_version");
        }
        if self.quick_fail_due && !self.terminal {
            self.f.add_once(FindingClass::MissedFail, ctx, "a Fail{PhaseQuick} right after the closing slice", "none before the batch ended", "the quick tally is above 0.028 S and the account was not failed");
            self.quick_fail_due = false;
        }
        self.settle_forced_closes(None);
        if flat != self.is_flat() {
            self.f.add(FindingClass::StateMismatch, ctx, self.is_flat().to_string(), flat.to_string(), "Meta.flat disagrees with the replayed positions");
        }
        if self.st.tight_on && self.is_flat() && !self.terminal {
            self.f.add(FindingClass::FloorMismatch, ctx, "TightEnd before a flat Meta", "tight mode still on", "missing TightEnd");
        }
        self.st.flat = flat;
        // commitments
        let batch = std::mem::take(&mut self.batch);
        let n_samp = match self.sample_prefix.get(samples_hash) {
            Some(n) if *n >= self.last_sample_n => {
                self.last_sample_n = *n;
                Some(*n)
            }
            Some(n) => {
                self.f.add(FindingClass::SamplesHashMismatch, ctx, format!(">= {} records", self.last_sample_n), format!("{n} records"), "the committed archive prefix shrank");
                None
            }
            None => {
                self.f.add(FindingClass::SamplesHashMismatch, ctx, "the sha256 of a prefix of the archive stream", crate::hexutil::encode(samples_hash), "Meta.samples_hash matches no record boundary");
                None
            }
        };
        if let Some(n) = n_samp {
            for (si, seq, idx) in &batch.proofs {
                if *si >= n {
                    self.f.add(FindingClass::ArchiveMissing, Ctx { seq: Some(*seq), idx: Some(*idx), time: ctx.time }, format!("a sample inside the first {n} records"), format!("record {si}"), "the proof's sample is not covered by the batch's samples_hash");
                }
            }
        }
        let n_int = match self.intent_prefix.get(intents_hash) {
            Some(bytes) if *bytes >= self.last_intent_n => {
                self.last_intent_n = *bytes;
                Some(self.intents.lines_in_prefix(*bytes))
            }
            Some(bytes) => {
                self.f.add(FindingClass::IntentsHashMismatch, ctx, format!(">= {} bytes", self.last_intent_n), format!("{bytes} bytes"), "the committed intents prefix shrank");
                None
            }
            None => {
                self.f.add(FindingClass::IntentsHashMismatch, ctx, "the sha256 of a prefix of the intents file", crate::hexutil::encode(intents_hash), "Meta.intents_hash matches no line boundary");
                None
            }
        };
        if let Some(n) = n_int {
            for (li, seq, idx) in &batch.intents {
                if *li >= n {
                    self.f.add(FindingClass::ForgedTrade, Ctx { seq: Some(*seq), idx: Some(*idx), time: ctx.time }, format!("an intent inside the first {n} lines"), format!("line {li}"), "the intent is not covered by the batch's intents_hash");
                }
            }
        }
        for (nonce, (got, want)) in &batch.nonce_qty {
            if let Some(w) = want {
                if got != w {
                    self.f.add(FindingClass::ForgedTrade, ctx, w.to_string(), got.to_string(), format!("the trades under intent {nonce} do not add up to its quantity"));
                }
            }
            self.closed_nonces.insert(*nonce);
        }
        self.in_batch_events = 0;
    }

    // ------------------------------------------------------------------ pass lines and phases

    fn state_summary_flat(&self) -> bool {
        self.is_flat() && self.st.flat
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_pass_line(&mut self, kind: &PassKind, phase: u8, amount: u64, b: i64, seq: u64, hash: &[u8; 32], time: i64) {
        if *kind == PassKind::Requested {
            let prev = self.last_seq;
            self.walk_advance(time, prev);
            self.max_time = self.max_time.max(time);
        }
        let mut reasons: Vec<String> = Vec::new();
        if seq > self.last_seq {
            reasons.push("journal_behind".into());
        } else if seq < self.last_seq {
            reasons.push("seq_mismatch".into());
        } else if hash != &self.head {
            reasons.push("hash_mismatch".into());
        }
        if phase != self.phase {
            reasons.push("phase_mismatch".into());
        }
        if b != self.st.b {
            reasons.push("b_mismatch".into());
        }
        let failed = self.phase_failed();
        match engine::pass_check(&self.st, self.state_summary_flat(), failed) {
            Ok(()) => {}
            Err(engine::PassError::NotFlat) => reasons.push("not_flat".into()),
            Err(engine::PassError::TargetNotReached) => reasons.push("target_not_reached".into()),
            Err(engine::PassError::PhaseFailed) => reasons.push("phase_failed".into()),
        }
        let want_amount = match engine::next_phase(self.phase) {
            Ok(n) if n.pays() => engine::payout_micro(self.size).ok().and_then(|a| u64::try_from(a).ok()),
            Ok(_) => Some(0),
            Err(_) => None,
        };
        if want_amount != Some(amount) {
            reasons.push("amount_mismatch".into());
        }
        if self.f.list.iter().any(|f| f.seq.is_some_and(|s| s <= self.last_seq)) {
            reasons.push("history_invalid".into());
        }
        let valid = reasons.is_empty();
        let ctx = Ctx { seq: Some(seq), idx: None, time: self.last_time };
        if !valid {
            self.f.add(FindingClass::PassInvalid, ctx, "a pass request that passes pass_check against the replay", reasons.join(","), format!("{kind:?} pass of phase {phase}"));
        }
        self.passes.push(PassVerdict { phase, kind: if *kind == PassKind::Requested { "requested" } else { "confirmed" }, requested_seq: seq, valid, reasons });
    }

    pub fn phase_failed(&self) -> bool {
        let floor = engine::static_floor_micro(&self.st).unwrap_or(i64::MAX);
        if self.st.b <= floor {
            return true;
        }
        engine::apply_closed_slice(&self.st, &self.engine_rules, 0, 0).map(|d| d.fail.is_some()).unwrap_or(true)
    }

    /// `PhaseAdvanced` / `PassFinalized`: a fresh phase starts (the program's reset, keeping an active lock).
    pub fn on_phase_change(&mut self, from_phase: u8, to_phase: u8, paid: Option<u64>) {
        let ctx = Ctx { seq: Some(self.last_seq), idx: None, time: self.last_time };
        if from_phase != self.phase {
            self.f.add(FindingClass::StateMismatch, ctx, self.phase.to_string(), from_phase.to_string(), "phase change from another phase");
        }
        let failed = self.phase_failed();
        if let Err(e) = engine::pass_check(&self.st, self.state_summary_flat(), failed) {
            self.f.add(FindingClass::PassInvalid, ctx, "the pass condition at the phase change", format!("{e:?}"), "the phase advanced without a passing state");
        }
        if let Some(a) = paid {
            let want = engine::payout_micro(self.size).ok().and_then(|x| u64::try_from(x).ok());
            if want != Some(a) {
                self.f.add(FindingClass::PassInvalid, ctx, format!("{want:?}"), a.to_string(), "payout amount");
            }
        }
        match engine::next_phase(from_phase) {
            Ok(engine::NextPhase::Next { phase, .. }) if phase == to_phase => {}
            Ok(engine::NextPhase::Graduated) if to_phase == 0 => {}
            other => self.f.add(FindingClass::PassInvalid, ctx, format!("{other:?}"), to_phase.to_string(), "unexpected next phase"),
        }
        if to_phase == 0 {
            self.terminal = true;
        } else {
            let mut ns = engine::reset_phase_state(self.size);
            ns.lock_until = self.st.lock_until;
            self.st = ns;
            self.phase = to_phase;
            self.phases.push(PhaseRec { phase: to_phase, first_seq: None, last_seq: None, events: 0 });
        }
        self.bump_epoch();
    }

    // ------------------------------------------------------------------ end

    pub fn finish(&mut self, as_of: bool) {
        self.walk_finish();
        self.flush_fees();
        if self.in_batch_events > 0 && !as_of {
            let ctx = Ctx { seq: Some(self.last_seq), idx: None, time: self.last_time };
            self.f.add(FindingClass::StateMismatch, ctx, "a log that ends with a Meta", format!("{} events after the last Meta", self.in_batch_events), "the log ends inside a batch");
        }
    }

    pub fn compare_snapshot(&mut self, snap: &Snapshot) {
        let ctx = Ctx { seq: Some(self.last_seq), idx: None, time: self.last_time };
        if snap.seq != self.last_seq {
            self.f.add(FindingClass::SequenceGap, ctx, snap.seq.to_string(), self.last_seq.to_string(), "the replayed log ends at another sequence than the stored account");
        } else if snap.hash() != self.head {
            self.f.add(FindingClass::HashChain, ctx, crate::hexutil::encode(&snap.hash()), crate::hexutil::encode(&self.head), "the replayed chain head differs from the stored journal_hash");
        }
        let status = if self.terminal { None } else { Some(0u8) };
        let checks: Vec<(&str, String, String)> = vec![
            ("b", snap.b.to_string(), self.st.b.to_string()),
            ("q_tally", snap.q_tally.to_string(), self.st.q_tally.to_string()),
            ("latch", snap.latch.to_string(), u8::from(self.st.latch).to_string()),
            ("tight_on", snap.tight_on.to_string(), u8::from(self.st.tight_on).to_string()),
            ("r", snap.r.to_string(), self.st.r.to_string()),
            ("g_peak", snap.g_peak.to_string(), self.st.g_peak.to_string()),
            ("l_level", snap.l_level.to_string(), self.st.l_level.to_string()),
            ("lock_until", snap.lock_until.to_string(), self.st.lock_until.to_string()),
            ("flat", snap.flat.to_string(), u8::from(self.st.flat).to_string()),
            ("phase", snap.phase.to_string(), self.phase.to_string()),
            ("trades_closed", snap.trades_closed.to_string(), self.trades_closed.to_string()),
        ];
        for (name, want, got) in checks {
            if want != got {
                self.f.add(FindingClass::StateMismatch, ctx, got, want, format!("snapshot field {name}"));
            }
        }
        if let Some(s) = status {
            if snap.status != s {
                self.f.add(FindingClass::StateMismatch, ctx, s.to_string(), snap.status.to_string(), "snapshot field status");
            }
        }
    }

    pub fn summary(&self) -> StateSummary {
        StateSummary {
            b: self.st.b,
            q_tally: self.st.q_tally,
            latch: self.st.latch,
            tight_on: self.st.tight_on,
            r: self.st.r,
            g_peak: self.st.g_peak,
            l_level: self.st.l_level,
            lock_until: self.st.lock_until,
            flat: self.is_flat(),
            phase: self.phase,
            terminal: self.terminal,
            trades_closed: self.trades_closed,
            seq: self.last_seq,
            journal_hash: crate::hexutil::encode(&self.head),
            open_positions: self.positions.len(),
        }
    }

    pub fn phase_verdicts(&self) -> Vec<PhaseVerdict> {
        self.phases
            .iter()
            .map(|p| {
                let (first, last) = (p.first_seq.unwrap_or(0), p.last_seq.unwrap_or(0));
                let bad_find = self.f.list.iter().any(|f| f.seq.is_some_and(|s| p.first_seq.is_some() && s >= first && s <= last));
                let bad_pass = self.passes.iter().any(|v| v.phase == p.phase && !v.valid);
                PhaseVerdict { phase: p.phase, first_seq: first, last_seq: last, events: p.events, valid: !bad_find && !bad_pass }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum IntentUse {
    Fill { side: Side, order_kind: OrderKind, sl: u64, tp: u64 },
    Close { reason: CloseReason, position_side: Side },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorKind {
    TightStart,
    FloorRaised,
    Breach,
    Fail(FailReason),
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::archive::{ArchiveWriter, Sample};
    use crate::intents::{header_line, IntentFile};
    use crate::log::OpenedRec;

    pub(crate) const T0: i64 = 1_791_813_600; // 2026-10-12 14:00:00 UTC

    pub(crate) fn with_replay(samples: &[Sample], f: impl FnOnce(&mut Replay)) {
        let rules = Rules::parse(crate::rules::sample_json().as_bytes()).unwrap();
        let mut w = ArchiveWriter::new();
        for s in samples {
            w.push(s);
        }
        let arch = Archive::parse(&w.into_bytes()).unwrap();
        let intents = IntentFile::parse(header_line().as_bytes()).unwrap();
        let account = [1u8; 32];
        let opened = OpenedRec {
            account,
            trader: [2; 32],
            challenge_id: 0,
            size_s: 100_000_000_000,
            tier_index: 8,
            phase: 1,
            ruleset_version: 1,
            genesis_hash: genesis_hash(&NativeSha256, &account),
            time: T0,
        };
        let mut rp = Replay::new(&rules, &arch, &intents, opened, "primary", Default::default(), Default::default()).unwrap();
        f(&mut rp);
    }

    pub(crate) fn sample(t: i64, mid: u64) -> Sample {
        let ts_ns = u64::try_from(t).unwrap() * 1_000_000_000;
        Sample { market: 0, source: 2, recv_ns: ts_ns, ts_ns, value: i128::from(mid) * 1_000_000, digest: [0; 32] }
    }

    pub(crate) fn long_position(fee_day: i64) -> Pos {
        Pos { p: Position { qty: 1_000_000_000_000, avg_entry_e12: 1_080_000_000_000, timer_start: T0, opened_at: T0, fee_day }, sl: 0, tp: 0 }
    }

    pub(crate) fn ctx() -> Ctx {
        Ctx { seq: Some(1), idx: Some(0), time: T0 }
    }

    #[test]
    fn a_miss_is_reported_once_per_epoch_and_again_after_the_state_changed() {
        with_replay(&[], |rp| {
            rp.f.add_once(FindingClass::MissedBreach, ctx(), "a", "b", "m");
            rp.f.add_once(FindingClass::MissedBreach, ctx(), "a", "b", "m");
            assert_eq!(rp.f.list.len(), 1);
            rp.f.add_once(FindingClass::MissedFail, ctx(), "a", "b", "m");
            assert_eq!(rp.f.list.len(), 2, "another class is reported");
            rp.bump_epoch();
            rp.f.add_once(FindingClass::MissedBreach, ctx(), "a", "b", "m");
            assert_eq!(rp.f.list.len(), 3, "the account changed in between");
        });
    }

    #[test]
    fn findings_are_capped() {
        with_replay(&[], |rp| {
            for _ in 0..(MAX_FINDINGS + 5) {
                rp.f.add(FindingClass::StateMismatch, ctx(), "a", "b", "m");
            }
            assert_eq!(rp.f.list.len(), MAX_FINDINGS);
            assert!(rp.f.truncated);
        });
    }

    #[test]
    fn a_night_already_charged_is_not_expected_again() {
        let r = T0 + 8 * 3_600; // 22:00:00
        let today = engine::day_index(r);
        with_replay(&[sample(T0, 1_080_000_000_000)], |rp| {
            rp.positions.insert(0, long_position(today));
            rp.holding_expectations(ctx(), r);
            assert!(rp.fee_exp.is_empty() && rp.f.list.is_empty());
            rp.positions.insert(0, long_position(today - 1));
            rp.holding_expectations(ctx(), r);
            assert_eq!(rp.fee_exp.len(), 1);
            assert_eq!((rp.fee_exp[0].time, rp.fee_exp[0].market), (r, 0));
            assert_eq!(rp.positions[&0].p.fee_day, today, "charged once");
            // the same night again: nothing new
            rp.holding_expectations(ctx(), r);
            assert_eq!(rp.fee_exp.len(), 1);
        });
    }

    #[test]
    fn a_rate_of_zero_expects_no_fee() {
        let r = T0 + 8 * 3_600;
        with_replay(&[sample(T0, 1_080_000_000_000)], |rp| {
            let mut rules = rp.rules.clone();
            rules.holding_fee_e6 = [0; 8];
            let rules = Box::leak(Box::new(rules));
            rp.rules = rules;
            rp.positions.insert(0, long_position(engine::day_index(r) - 1));
            rp.holding_expectations(ctx(), r);
            assert!(rp.fee_exp.is_empty());
        });
    }

    #[test]
    fn no_mark_before_the_rollover_is_a_finding() {
        let r = T0 + 8 * 3_600;
        with_replay(&[sample(r + 5, 1_080_000_000_000)], |rp| {
            rp.positions.insert(0, long_position(engine::day_index(r) - 1));
            rp.holding_expectations(ctx(), r);
            assert!(rp.fee_exp.is_empty());
            assert_eq!(rp.f.list.len(), 1);
            assert_eq!(rp.f.list[0].class, FindingClass::ArchiveMissing);
        });
    }

    #[test]
    fn the_gap_between_two_events_is_capped() {
        with_replay(&[sample(T0, 1_080_000_000_000)], |rp| {
            rp.positions.insert(0, long_position(engine::day_index(T0)));
            rp.before_event(ctx(), T0 + MAX_ROLLOVER_GAP * 86_400 + 5 * 86_400);
            let capped: Vec<_> = rp.f.list.iter().filter(|f| f.message.contains("too many holding-fee nights")).collect();
            assert_eq!(capped.len(), 1);
            assert!(rp.fee_exp.is_empty());
        });
        // exactly the cap is checked night by night
        with_replay(&[sample(T0, 1_080_000_000_000)], |rp| {
            rp.positions.insert(0, long_position(engine::day_index(T0)));
            let t = T0 + 8 * 3_600 + (MAX_ROLLOVER_GAP - 1) * 86_400;
            assert_eq!(engine::day_index(t) - engine::day_index(T0), MAX_ROLLOVER_GAP);
            rp.before_event(ctx(), t);
            assert!(!rp.f.list.iter().any(|f| f.message.contains("too many holding-fee nights")));
        });
    }

    #[test]
    fn expired_and_flushed_fees_are_reported_with_their_attribution() {
        with_replay(&[], |rp| {
            let exp = |time: i64| FeeExp { kind: FeeExpKind::Commission, market: 0, time, amount: 5, why: "w".into() };
            rp.fee_exp.push(exp(T0));
            rp.fee_exp.push(exp(T0 + 10));
            rp.expire_fees(Ctx { seq: Some(9), idx: None, time: T0 + 10 }, T0 + 10);
            assert_eq!(rp.f.list.len(), 1, "only the older one expires");
            assert_eq!(rp.f.list[0].seq, Some(9));
            assert_eq!(rp.fee_exp.len(), 1);
            rp.last_seq = 12;
            rp.flush_fees();
            assert_eq!(rp.f.list.len(), 2);
            assert_eq!(rp.f.list[1].seq, Some(12));
            assert!(rp.fee_exp.is_empty());
        });
    }

    #[test]
    fn phase_failed_and_the_flat_flag() {
        with_replay(&[], |rp| {
            assert!(!rp.phase_failed());
            rp.st.b = 92_000_000_000;
            assert!(rp.phase_failed(), "at the static floor");
            rp.st.b = 92_000_000_001;
            assert!(!rp.phase_failed());
            rp.st.q_tally = 2_800_000_000;
            assert!(!rp.phase_failed());
            rp.st.q_tally = 2_800_000_001;
            assert!(rp.phase_failed());
            rp.st.q_tally = 0;
            rp.st.latch = true;
            rp.st.b = 96_000_000_000;
            assert!(rp.phase_failed(), "the latched floor is 0.96 S");
            // state_summary_flat needs both the replayed positions and the stored flag
            rp.st.flat = true;
            assert!(rp.state_summary_flat());
            rp.st.flat = false;
            assert!(!rp.state_summary_flat());
            rp.st.flat = true;
            rp.positions.insert(0, long_position(0));
            assert!(!rp.state_summary_flat());
        });
    }

    #[test]
    fn phase_verdicts_attribute_findings_to_the_phase_that_holds_their_sequence() {
        with_replay(&[], |rp| {
            rp.phases = vec![
                PhaseRec { phase: 2, first_seq: Some(1), last_seq: Some(4), events: 4 },
                PhaseRec { phase: 3, first_seq: Some(5), last_seq: Some(8), events: 4 },
                PhaseRec { phase: 4, first_seq: None, last_seq: None, events: 0 },
            ];
            rp.f.add(FindingClass::StateMismatch, Ctx { seq: Some(5), idx: None, time: 0 }, "a", "b", "m");
            let v = rp.phase_verdicts();
            assert_eq!((v[0].valid, v[1].valid, v[2].valid), (true, false, true));
            assert_eq!((v[0].first_seq, v[0].last_seq, v[0].events), (1, 4, 4));
            // a finding with the boundary sequence belongs to the phase that ends there
            rp.f.list.clear();
            rp.f.add(FindingClass::StateMismatch, Ctx { seq: Some(4), idx: None, time: 0 }, "a", "b", "m");
            let v = rp.phase_verdicts();
            assert_eq!((v[0].valid, v[1].valid), (false, true));
            rp.f.list.clear();
            rp.f.add(FindingClass::StateMismatch, Ctx { seq: Some(8), idx: None, time: 0 }, "a", "b", "m");
            assert_eq!((rp.phase_verdicts()[1].valid, rp.phase_verdicts()[0].valid), (false, true));
            rp.f.list.clear();
            rp.f.add(FindingClass::StateMismatch, Ctx { seq: Some(9), idx: None, time: 0 }, "a", "b", "m");
            assert!(rp.phase_verdicts().iter().all(|p| p.valid));
            // an invalid pass belongs to its phase only
            rp.f.list.clear();
            rp.passes.push(PassVerdict { phase: 3, kind: "requested", requested_seq: 6, valid: false, reasons: vec![] });
            let v = rp.phase_verdicts();
            assert_eq!((v[0].valid, v[1].valid, v[2].valid), (true, false, true));
        });
    }
}
