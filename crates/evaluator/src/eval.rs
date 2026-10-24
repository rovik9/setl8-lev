//! The evaluator: state, inputs and clock. The rules that act on a second of samples live in `decide.rs`, orders in
//! `exec.rs`, batching in `drain.rs`, the pass flow in `pass.rs`.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use engine::{AccountState, Rules as EngineRules};
use journal_types::genesis_hash;

use crate::codec::{archive_header, intents_header_line, intents_line, signing_message, IntentKind, OrderIntent, SampleRec, ORDER_LEN};
use crate::config::*;
use crate::sha256::{chain_hasher, Sha256State};
use crate::types::*;

/// Checks the trader's ed25519 signature. The core has no crypto dependency, so the caller injects it.
pub trait IntentVerifier {
    /// `message` is `codec::signing_message(account, nonce, order)`; `trader` is the wallet that must have signed it.
    fn verify(&self, trader: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool;
}

/// Accepts every signature (tests of the book itself, or a service that verified the signature before calling).
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceptAll;

impl IntentVerifier for AcceptAll {
    fn verify(&self, _trader: &[u8; 32], _message: &[u8], _sig: &[u8; 64]) -> bool {
        true
    }
}

/// A trader order as received: the nonce the trader chose, the 48 order bytes and the signature over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedIntent {
    pub nonce: u32,
    pub order: [u8; ORDER_LEN],
    pub sig: [u8; 64],
}

/// Counters for the campaigns' coverage tables (they never influence a decision).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub breaches: u32,
    pub static_fails: u32,
    pub quick_fails: u32,
    pub quick_trades: u32,
    pub holding_fees: u32,
    pub limit_fills: u32,
    pub stop_fills: u32,
    pub sl_triggers: u32,
    pub tp_triggers: u32,
    pub flips: u32,
    pub tight_starts: u32,
    pub tight_ends: u32,
    pub floor_raises: u32,
    /// Orders refused because the market was closed or stale.
    pub closed_market_rejects: u32,
    /// Samples of a market that was closed while a position of the account sat in it.
    pub closed_with_position: u32,
    /// Triggered pending orders the evaluator could not fill (margin, cap, lock, hold): the verifier expects them filled.
    pub refused_fills: u32,
    /// Forced close-all groups the program cannot take in one batch (decision 69): the first close over the quick limit, the closes after
    /// it and the Fail need more than 8 events with the Meta, or more than 3 distinct price proofs. Must stay 0 over the campaigns.
    pub unpostable: u32,
    /// Forced closes (reason Fail) that follow the first close over the quick limit in one failing sequence (decision 69, audit F-16).
    pub forced_after_overshoot: u32,
    /// MARKET orders that passed the margin check with `equity - margin used == 0` exactly (M9c coverage).
    pub margin_exact: u32,
    /// Accepted market orders that could not execute when their second closed (margin, a market that closed, a lock).
    pub exec_refused: u32,
    pub orders_accepted: u32,
    pub orders_rejected: u32,
    /// Orders refused at arrival because they would open exposure in a market whose half-spread is 0 (decision 64).
    pub unpriced_rejects: u32,
    /// Price samples refused because their source second is more than `MAX_SAMPLE_FUTURE_SECS` ahead of the receive time (audit F-17).
    pub samples_future: u32,
    /// Price samples refused because they were older than the market's `max_age_secs` when received (audit F-17).
    pub samples_stale: u32,
    /// Price samples refused because they break the stream's order: the second is before the clock, or the market's time goes backwards.
    pub samples_out_of_order: u32,
}

/// An accepted order (or a triggered pending order) that could not execute when its second closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub time: i64,
    pub nonce: u32,
    pub reason: OrderReject,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Track {
    /// Indexes into `Evaluator::samples`, in stream order.
    pub idxs: Vec<usize>,
    pub last_value: Option<i128>,
    pub last_change: i64,
    pub last_ts_ns: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct QOrder {
    pub nonce: u32,
    pub intent: OrderIntent,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Bucket {
    pub samples: Vec<usize>,
    pub orders: Vec<QOrder>,
}

/// The reference evaluator of one account. See the crate documentation for the model.
pub struct Evaluator {
    pub(crate) rules: Arc<RulesCfg>,
    pub(crate) acct: AccountParams,
    pub(crate) verifier: Box<dyn IntentVerifier + Send>,
    pub(crate) erules: EngineRules,
    pub(crate) st: AccountState,
    pub(crate) phase: u8,
    pub(crate) positions: BTreeMap<u16, Pos>,
    pub(crate) pending: Vec<Pending>,
    pub(crate) trades_closed: u32,
    pub(crate) status: Status,
    pub(crate) frozen: bool,
    pub(crate) held: bool,
    pub(crate) fault: Option<EvalError>,
    /// The account ended outside the journal (a vault abandonment, graduation after the last payout): inputs are ignored.
    pub(crate) ended_externally: bool,
    // samples
    pub(crate) samples: Vec<Stored>,
    pub(crate) archive: Vec<u8>,
    pub(crate) tracks: BTreeMap<u16, Track>,
    pub(crate) marks: BTreeMap<u16, usize>,
    pub(crate) last_archive_day: Option<i64>,
    // time
    pub(crate) clock: i64,
    pub(crate) processed_through: i64,
    pub(crate) roll_cursor: i64,
    pub(crate) buckets: BTreeMap<i64, Bucket>,
    // output
    pub(crate) queue: VecDeque<Unit>,
    pub(crate) seq: u64,
    pub(crate) head: [u8; 32],
    pub(crate) last_meta_time: i64,
    // intents
    pub(crate) nonces: BTreeSet<u32>,
    pub(crate) intents: Vec<u8>,
    // commitments
    pub(crate) samples_hasher: Sha256State,
    pub(crate) samples_hashed: usize,
    pub(crate) intents_hasher: Sha256State,
    pub(crate) intents_hashed: usize,
    pub(crate) stats: Stats,
    pub(crate) refusals: Vec<Refusal>,
}

impl Evaluator {
    /// A new evaluator for a freshly opened account: balance S, phase `acct.phase`, nothing open, the hash chain at
    /// its genesis, an archive with its first segment header and an intents file with its header line.
    pub fn new(rules: RulesCfg, acct: AccountParams, verifier: Box<dyn IntentVerifier + Send>) -> Result<Self, ConfigError> {
        if acct.size_micro <= 0 || engine::frac_exact(acct.size_micro, engine::consts::BPS).is_err() {
            return Err(ConfigError::BadSize);
        }
        if acct.size_micro % 1_000_000 != 0 {
            return Err(ConfigError::BadSize);
        }
        if !(1..=5).contains(&acct.phase) {
            return Err(ConfigError::BadPhase);
        }
        if rules.leverage_x.contains(&0) {
            return Err(ConfigError::BadLeverage);
        }
        let mut seen = BTreeSet::new();
        for m in &rules.markets {
            if usize::from(m.class) >= CLASSES || m.sessions.len() > 6 || m.qty_step == 0 || m.max_age_secs == 0 || m.half_spread_e6 > 1_000_000 {
                return Err(ConfigError::BadMarket(m.id));
            }
            if !seen.insert(m.id) {
                return Err(ConfigError::DuplicateMarket(m.id));
            }
        }
        let head = genesis_hash(&chain_hasher(), &acct.account);
        let mut intents = Vec::new();
        intents.extend_from_slice(intents_header_line().as_bytes());
        let t0 = acct.opened_time;
        Ok(Evaluator {
            st: engine::reset_phase_state(acct.size_micro),
            phase: acct.phase,
            rules: Arc::new(rules),
            acct,
            verifier,
            erules: EngineRules::default(),
            positions: BTreeMap::new(),
            pending: Vec::new(),
            trades_closed: 0,
            status: Status::Active,
            frozen: false,
            held: false,
            fault: None,
            ended_externally: false,
            samples: Vec::new(),
            archive: archive_header().to_vec(),
            tracks: BTreeMap::new(),
            marks: BTreeMap::new(),
            last_archive_day: None,
            clock: t0,
            processed_through: t0,
            roll_cursor: t0,
            buckets: BTreeMap::new(),
            queue: VecDeque::new(),
            seq: 0,
            head,
            last_meta_time: t0,
            nonces: BTreeSet::new(),
            intents,
            samples_hasher: Sha256State::new(),
            samples_hashed: 0,
            intents_hasher: Sha256State::new(),
            intents_hashed: 0,
            stats: Stats::default(),
            refusals: Vec::new(),
        })
    }

    // ------------------------------------------------------------------ accessors

    pub fn account(&self) -> &AccountParams {
        &self.acct
    }
    pub fn rules(&self) -> &RulesCfg {
        &self.rules
    }
    /// The engine state as of everything processed (queued events included).
    pub fn state(&self) -> &AccountState {
        &self.st
    }
    pub fn phase(&self) -> u8 {
        self.phase
    }
    pub fn positions(&self) -> &BTreeMap<u16, Pos> {
        &self.positions
    }
    pub fn pending(&self) -> &[Pending] {
        &self.pending
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn trades_closed(&self) -> u32 {
        self.trades_closed
    }
    /// Sequence number and chain head of the batches built so far.
    pub fn seq(&self) -> u64 {
        self.seq
    }
    pub fn head(&self) -> [u8; 32] {
        self.head
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    /// The first 256 orders that were accepted but could not execute (margin, a lock, a market that closed ...), with the reason.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }
    /// The first internal failure, if any (a bug or an impossible configuration). After it the evaluator ignores input.
    pub fn fault(&self) -> Option<EvalError> {
        self.fault
    }
    /// The evaluator's second: every second before it is closed.
    pub fn clock(&self) -> i64 {
        self.clock
    }
    /// All seconds up to and including this one are processed; `Meta.time` is never later.
    pub fn processed_through(&self) -> i64 {
        self.processed_through
    }
    /// Events waiting for `drain_batches`.
    pub fn queued_events(&self) -> usize {
        self.queue.iter().map(|u| u.events.len()).sum()
    }
    /// The whole sample archive written so far (segment headers and records).
    pub fn archive_bytes(&self) -> &[u8] {
        &self.archive
    }
    /// The archive stream the sample hashes commit to: the records without headers.
    pub fn sample_stream(&self) -> Vec<u8> {
        self.samples.iter().flat_map(|s| s.rec.to_bytes()).collect()
    }
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
    pub fn sample(&self, idx: usize) -> Option<&Stored> {
        self.samples.get(idx)
    }
    /// The intents file written so far (header and one line per accepted order).
    pub fn intents_bytes(&self) -> &[u8] {
        &self.intents
    }
    /// Quick-tally headroom: how many more credited quick profit micro-USD fit before the phase fails.
    pub fn quick_headroom(&self) -> i64 {
        let limit = engine::frac_exact(self.st.size_micro, engine::consts::QUICK_LIMIT_BPS).unwrap_or(0);
        let room = limit.saturating_sub(self.st.q_tally);
        if self.erules.quick_strict {
            room
        } else {
            room.saturating_sub(1)
        }
    }
    /// `AccountHeld` (`held = true`) or `HoldReleased` (`held = false`): exposure-increasing orders are refused while held (DESIGN 2.9).
    /// The program lets `hold_account` act on an Active account only and `release_hold` on any status. Preconditions: not faulted or
    /// ended (`Ended`); to hold, status Active (`NotActive`) and not held (`AlreadyHeld`); to release, held (`NotHeld`). On any error
    /// NOTHING is changed.
    pub fn set_held(&mut self, held: bool) -> Result<(), LifecycleError> {
        if self.fault.is_some() || self.ended_externally {
            return Err(LifecycleError::Ended);
        }
        if held {
            if !matches!(self.status, Status::Active) {
                return Err(LifecycleError::NotActive);
            }
            if self.held {
                return Err(LifecycleError::AlreadyHeld);
            }
        } else if !self.held {
            return Err(LifecycleError::NotHeld);
        }
        self.held = held;
        Ok(())
    }
    pub fn is_ended(&self) -> bool {
        self.fault.is_some() || self.ended_externally || !matches!(self.status, Status::Active)
    }

    pub(crate) fn refuse(&mut self, time: i64, nonce: u32, reason: OrderReject) {
        self.stats.exec_refused = self.stats.exec_refused.saturating_add(1);
        if self.refusals.len() < 256 {
            self.refusals.push(Refusal { time, nonce, reason });
        }
    }

    pub(crate) fn fail_internal(&mut self, e: EvalError) {
        if self.fault.is_none() {
            self.fault = Some(e);
        }
    }

    pub(crate) fn market_cfg(&self, id: u16) -> Option<MarketCfg> {
        self.rules.market(id).cloned()
    }

    // ------------------------------------------------------------------ samples

    /// Records a price update: `value` has 18 decimals (the `PriceProof.quantized_value`), `ts_ns` is the source's own
    /// timestamp, `recv_ns` the evaluator's receive time, `digest` the sha256 of the signed blob for source 3 (zeros otherwise).
    /// Samples come in non-decreasing source-second order across all markets (decision 7 of module 9).
    pub fn on_sample(&mut self, market: u16, ts_ns: u64, value: i128, recv_ns: u64, digest: [u8; 32]) -> Result<(), SampleError> {
        let Some(m) = self.market_cfg(market) else { return Err(SampleError::UnknownMarket) };
        let mid = journal_types::price_e12_from_quantized(value).map_err(|_| SampleError::BadValue)?;
        let rec = SampleRec { market, source: m.source, recv_ns, ts_ns, value, digest };
        let sec = rec.sec();
        // audit F-17 (decision 68): a sample dated ahead of the evaluator's own receive time, or already older than the market's
        // max_age when it arrives, is counted and dropped; it is never journalled and never moves the clock or the market's time
        let recv_sec = i64::try_from(recv_ns / 1_000_000_000).unwrap_or(i64::MAX);
        if sec > recv_sec.saturating_add(MAX_SAMPLE_FUTURE_SECS) {
            self.stats.samples_future = self.stats.samples_future.saturating_add(1);
            return Err(SampleError::FutureSample);
        }
        if recv_sec.saturating_sub(sec) > i64::from(m.max_age_secs) {
            self.stats.samples_stale = self.stats.samples_stale.saturating_add(1);
            return Err(SampleError::StaleSample);
        }
        if sec < self.clock {
            self.stats.samples_out_of_order = self.stats.samples_out_of_order.saturating_add(1);
            return Err(SampleError::OutOfOrder);
        }
        let prev_ts = self.tracks.get(&market).map(|t| t.last_ts_ns).unwrap_or(0);
        if ts_ns < prev_ts {
            self.stats.samples_out_of_order = self.stats.samples_out_of_order.saturating_add(1);
            return Err(SampleError::MarketTimeBackwards);
        }
        let (Ok(bid), Ok(ask)) = (engine::bid_from_mid(mid, m.half_spread_e6), engine::ask_from_mid(mid, m.half_spread_e6)) else {
            return Err(SampleError::BadValue);
        };
        self.advance_clock(sec);
        let track = self.tracks.entry(market).or_default();
        if track.last_value != Some(value) {
            track.last_change = sec;
        }
        track.last_value = Some(value);
        track.last_ts_ns = ts_ns;
        let idx = self.samples.len();
        track.idxs.push(idx);
        let last_change = track.last_change;
        self.samples.push(Stored { rec, mid, bid, ask, last_change });
        // a new UTC day starts a new archive segment (headers are not part of the hashed stream)
        let day = sec.div_euclid(86_400);
        if self.last_archive_day.is_some_and(|d| d != day) {
            self.archive.extend_from_slice(&archive_header());
        }
        self.last_archive_day = Some(day);
        self.archive.extend_from_slice(&rec.to_bytes());
        if self.is_ended() {
            // the archive still records the stream; nothing acts on it
            return Ok(());
        }
        self.buckets.entry(sec).or_default().samples.push(idx);
        Ok(())
    }

    /// Convenience: a sample from a mid price in 1e-12 USD, stamped at whole second `sec`.
    pub fn on_sample_mid(&mut self, market: u16, sec: i64, mid_e12: u64) -> Result<(), SampleError> {
        let ts_ns = u64::try_from(sec).map_err(|_| SampleError::BadValue)?.saturating_mul(1_000_000_000);
        self.on_sample(market, ts_ns, i128::from(mid_e12) * 1_000_000, ts_ns.saturating_add(1_000_000), [0; 32])
    }

    // ------------------------------------------------------------------ clock

    /// Backend time moved to second `now`: every second before it is closed and processed (rollover fees, floors,
    /// triggers, pending orders, queued orders). Time never goes back; an earlier `now` is ignored.
    pub fn tick(&mut self, now: i64) {
        if now > self.clock {
            self.advance_clock(now);
        }
    }

    pub(crate) fn advance_clock(&mut self, to: i64) {
        if to <= self.clock {
            return;
        }
        self.close_before(to);
        self.clock = to;
    }

    // ------------------------------------------------------------------ orders

    /// A signed order. Accepted orders are written to the intents file (`Limit`, `Stop`, `Cancel`, `SetSlTp`, `Market`, `Close`)
    /// and executed when their second closes; a refused order leaves no trace. Market orders can still be refused at
    /// execution (margin, a market that closed): then the line stays in the intents file with no trade under it.
    pub fn on_order(&mut self, s: &SignedIntent) -> Result<(), OrderReject> {
        let r = self.accept_order(s);
        match r {
            Ok(()) => self.stats.orders_accepted = self.stats.orders_accepted.saturating_add(1),
            Err(why) => {
                self.stats.orders_rejected = self.stats.orders_rejected.saturating_add(1);
                if why == OrderReject::MarketUnpriced {
                    self.stats.unpriced_rejects = self.stats.unpriced_rejects.saturating_add(1);
                }
            }
        }
        r
    }

    fn accept_order(&mut self, s: &SignedIntent) -> Result<(), OrderReject> {
        if self.is_ended() {
            return Err(OrderReject::AccountEnded);
        }
        if self.frozen {
            return Err(OrderReject::AccountFrozen);
        }
        let msg = signing_message(&self.acct.account, s.nonce, &s.order);
        if !self.verifier.verify(&self.acct.trader, &msg, &s.sig) {
            return Err(OrderReject::BadSignature);
        }
        if s.nonce == 0 || self.nonces.contains(&s.nonce) {
            return Err(OrderReject::BadNonce);
        }
        let intent = OrderIntent::from_bytes(&s.order).ok_or(OrderReject::BadOrder)?;
        // audit F-18 (decision 68): the trader's time never moves the evaluator's clock. A market or close order is executed in the
        // evaluator's OWN second (the one it arrives in); its signed time only has to be within MAX_ORDER_SKEW_SECS of it. The other
        // kinds keep their signed time (the verifier replays them by it) and may be dated ahead, never in the past.
        let immediate = matches!(intent.kind, IntentKind::Market | IntentKind::Close);
        let skew = intent.client_time.saturating_sub(self.clock);
        if skew < if immediate { -MAX_ORDER_SKEW_SECS } else { 0 } {
            return Err(OrderReject::StaleOrder);
        }
        if skew > MAX_ORDER_SKEW_SECS {
            return Err(OrderReject::FutureOrder);
        }
        self.validate_arrival(&intent)?;
        // accepted: from here the order is part of the record
        self.nonces.insert(s.nonce);
        self.intents.extend_from_slice(intents_line(s.nonce, &s.order, &s.sig).as_bytes());
        match intent.kind {
            IntentKind::Limit | IntentKind::Stop => self.pending.push(Pending { nonce: s.nonce, intent }),
            IntentKind::Market | IntentKind::Close => self.buckets.entry(self.clock).or_default().orders.push(QOrder { nonce: s.nonce, intent }),
            IntentKind::Cancel | IntentKind::SetSlTp => self.buckets.entry(intent.client_time).or_default().orders.push(QOrder { nonce: s.nonce, intent }),
        }
        Ok(())
    }

    // ------------------------------------------------------------------ lifecycle
    //
    // The host contract (handover/runtime-hosting-design.md 7.3, handover/runtime-api.md): each call below follows ONE finalized chain
    // event, in chain order, and is checked against what the evaluator can verify itself. A violated precondition returns a
    // `LifecycleError` and changes nothing. What only the host can authenticate (that the event is finalized, not reorged, in order) stays
    // the host's duty.
    //
    //   PassRequested{phase}                      -> `request_pass(now, chain, replay)`   (freezes the account)
    //   PassRejected / PassCancelled / PassVetoed -> `pass_resolved()`
    //   PhaseAdvanced{1 -> 2} (from confirm_pass, no delay)
    //   PassFinalized{p -> p+1} + PhaseAdvanced   -> `phase_started(p + 1, chain)` once, with the chain's view AFTER the transition
    //   PassFinalized{5 -> 0} (graduation)        -> `end_externally()`
    //   AccountAbandoned / ViolationFail          -> `end_externally()` (also on a Failed account)
    //   AccountHeld / HoldReleased                -> `set_held(true / false)`

    fn live(&self) -> Result<(), LifecycleError> {
        if self.fault.is_some() || self.ended_externally {
            return Err(LifecycleError::Ended);
        }
        if !matches!(self.status, Status::Active) {
            return Err(LifecycleError::NotActive);
        }
        Ok(())
    }

    /// Whether an accepted order is still waiting in a second's bucket (market, close, cancel and set_sl_tp orders execute when their
    /// second closes).
    fn has_queued_orders(&self) -> bool {
        self.buckets.values().any(|b| !b.orders.is_empty())
    }

    /// A new phase starts: phase 2 at `confirm_pass` of phase 1, phases 3 to 5 at `finalize_pass` of phases 2 to 4. Call it once per
    /// transition (the program emits `PassFinalized` and `PhaseAdvanced` for one `finalize_pass`). `chain` is the chain's account AFTER
    /// the transition: `seq` and `journal_hash` are the journal head (a phase change does not touch the journal), `b` is S (a fresh
    /// phase), `flat` is true, `phase` is the new phase.
    ///
    /// The engine state becomes `reset_phase_state(S)` with an active 12 h breach lock KEPT (the program's `start_phase` and the
    /// verifier's `on_phase_change` keep it too); the phase advances; the account unfreezes. `seq`, `head`, the archive, the intents
    /// file, both hash states, the nonces and `trades_closed` continue untouched.
    ///
    /// Preconditions, checked in this order (the first violated one is returned): not faulted or ended (`Ended`), status Active
    /// (`NotActive`), a pass is pending (`NotFrozen`), `phase` is the next phase after the current one (`WrongPhase`; after phase 5 the
    /// expected phase is 0: call `end_externally`), no position (`NotFlat`), no pending order (`PendingOrders`), no queued journal event
    /// (`QueuedEvents`), no accepted order waiting for its second (`OrdersQueued`), and the chain view agrees (`ChainMismatch`: `phase`,
    /// `seq`, `journal_hash`, `b`, `flat`). On any error NOTHING is changed.
    pub fn phase_started(&mut self, phase: u8, chain: &ChainView) -> Result<(), LifecycleError> {
        self.live()?;
        if !self.frozen {
            return Err(LifecycleError::NotFrozen);
        }
        let want = match engine::next_phase(self.phase) {
            Ok(engine::NextPhase::Next { phase: p, .. }) => p,
            _ => 0,
        };
        if phase == 0 || phase != want {
            return Err(LifecycleError::WrongPhase { have: phase, want });
        }
        if !self.positions.is_empty() {
            return Err(LifecycleError::NotFlat);
        }
        if !self.pending.is_empty() {
            return Err(LifecycleError::PendingOrders);
        }
        if self.queued_events() > 0 {
            return Err(LifecycleError::QueuedEvents);
        }
        if self.has_queued_orders() {
            return Err(LifecycleError::OrdersQueued);
        }
        if chain.phase != phase {
            return Err(LifecycleError::ChainMismatch { what: "phase" });
        }
        if chain.seq != self.seq {
            return Err(LifecycleError::ChainMismatch { what: "seq" });
        }
        if chain.journal_hash != self.head {
            return Err(LifecycleError::ChainMismatch { what: "journal_hash" });
        }
        if chain.b != self.acct.size_micro {
            return Err(LifecycleError::ChainMismatch { what: "b" });
        }
        if !chain.flat {
            return Err(LifecycleError::ChainMismatch { what: "flat" });
        }
        let lock = self.st.lock_until;
        let mut fresh = engine::reset_phase_state(self.acct.size_micro);
        fresh.lock_until = lock;
        self.st = fresh;
        self.phase = phase;
        self.frozen = false;
        Ok(())
    }

    /// The pass was rejected (`PassRejected`), cancelled (`PassCancelled`) or vetoed (`PassVetoed`): trading resumes in the same phase.
    /// Preconditions: not faulted or ended (`Ended`), status Active (`NotActive`), a pass is pending (`NotFrozen`). On any error NOTHING
    /// is changed.
    pub fn pass_resolved(&mut self) -> Result<(), LifecycleError> {
        self.live()?;
        if !self.frozen {
            return Err(LifecycleError::NotFrozen);
        }
        self.frozen = false;
        Ok(())
    }

    /// Whether the account is frozen for trading (a pass is requested or waiting out its delay).
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// The account became terminal outside the journal (`AccountAbandoned`, `ViolationFail`, or `PassFinalized` to phase 0 after
    /// phase 5): the evaluator stops. Allowed on a Failed account (`fail_for_violation` ends a Failed + held account) and on a faulted
    /// evaluator; refused only when it was ended this way already (`AlreadyEnded`, nothing changed). Queued events and pending orders
    /// are left as they are (the host drains what the chain still accepts).
    pub fn end_externally(&mut self) -> Result<(), LifecycleError> {
        if self.ended_externally {
            return Err(LifecycleError::AlreadyEnded);
        }
        self.frozen = true;
        self.ended_externally = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev_for_unit_tests() -> Evaluator {
        let market = MarketCfg {
            id: 0,
            class: 0,
            source: 2,
            feed_id: [7; 32],
            max_age_secs: 60,
            status: 1,
            qty_step: 1_000,
            qty_min: 1_000,
            max_notional_micro: 1_000_000_000_000_000,
            sessions: vec![(0, 10_080)],
            half_spread_e6: 30,
        };
        let rules = RulesCfg {
            ruleset_version: 1,
            leverage_x: [50, 20, 20, 20, 10, 5, 5, 2],
            holding_fee_e6: [100, 200, 400, 200, 400, 400, 1000, 1600],
            commission_e6: [0; 8],
            markets: vec![market],
        };
        let acct = AccountParams { account: [1; 32], trader: [2; 32], challenge_id: 0, size_micro: 100_000_000_000, phase: 2, tier_index: 8, opened_time: 1_700_000_000 };
        Evaluator::new(rules, acct, Box::new(AcceptAll)).unwrap()
    }

    #[test]
    fn the_first_internal_failure_is_kept_and_ends_the_account() {
        let mut ev = ev_for_unit_tests();
        assert_eq!(ev.fault(), None);
        ev.fail_internal(EvalError::Overflow);
        ev.fail_internal(EvalError::Invariant("later"));
        assert_eq!(ev.fault(), Some(EvalError::Overflow));
        assert!(ev.is_ended());
    }

    #[test]
    fn a_faulted_evaluator_refuses_a_pass() {
        let mut ev = ev_for_unit_tests();
        ev.fail_internal(EvalError::Overflow);
        let chain = ChainView { seq: ev.seq(), journal_hash: ev.head(), b: ev.state().b, flat: true, phase: 2 };
        assert_eq!(ev.request_pass(1_700_000_001, &chain, ReplayCheck::Skipped), PassDecision::Reject(PassReject::Other));
    }

    #[test]
    fn only_the_first_256_refusals_are_kept() {
        let mut ev = ev_for_unit_tests();
        for n in 0..300 {
            ev.refuse(1, n, OrderReject::NoFreshPrice);
        }
        assert_eq!(ev.refusals().len(), 256);
        assert_eq!(ev.stats().exec_refused, 300);
    }

    // ---------------------------------------------------------------------------------------------- P29c: the typed lifecycle

    use crate::codec::{IntentKind, OrderIntent};
    use engine::Position;

    const T0: i64 = 1_700_000_100;

    /// Every private field, in a fixed order, as text: two equal fingerprints mean nothing changed.
    fn fingerprint(e: &Evaluator) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{}|{:?}|{}|{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}",
            e.rules, e.acct, e.erules, e.st, e.phase, e.positions, e.pending, e.trades_closed, e.status, e.frozen, e.held, e.fault, e.ended_externally,
            e.samples, e.archive, e.tracks, e.marks, e.last_archive_day, e.clock, e.processed_through, e.roll_cursor, e.buckets, e.queue, e.seq, e.head,
            e.last_meta_time, e.nonces, e.intents, e.samples_hasher, e.samples_hashed, e.intents_hasher, e.intents_hashed, e.stats, e.refusals
        )
    }

    fn chain_of(e: &Evaluator, phase: u8) -> ChainView {
        ChainView { seq: e.seq(), journal_hash: e.head(), b: e.acct.size_micro, flat: true, phase }
    }

    fn pos() -> Pos {
        Pos { p: Position { qty: 1_000, avg_entry_e12: 1_000_000_000_000, timer_start: T0, opened_at: T0, fee_day: 0 }, sl: 0, tp: 0 }
    }

    fn intent(kind: IntentKind, t: i64) -> OrderIntent {
        OrderIntent { kind, market: 0, side: 0, qty: 1_000, price_e12: 0, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: t }
    }

    fn signed(nonce: u32, it: &OrderIntent) -> SignedIntent {
        SignedIntent { nonce, order: it.to_bytes(), sig: [0; 64] }
    }

    /// A frozen, flat, idle account of phase 2: the state a pass leaves behind (what `phase_started` expects).
    fn frozen_idle() -> Evaluator {
        let mut e = ev_for_unit_tests();
        e.frozen = true;
        e
    }

    #[test]
    fn phase_started_accepts_the_transition_and_keeps_what_must_continue() {
        let mut e = ev_for_unit_tests();
        e.on_sample_mid(0, T0, 1_000_000_000_000).unwrap();
        e.on_order(&signed(7, &intent(IntentKind::Market, T0))).unwrap();
        e.tick(T0 + 2);
        // close it again so the account is flat but has history: a Fill and a Close in the queue, then drained
        e.frozen = false;
        e.on_order(&signed(8, &intent(IntentKind::Close, T0 + 2))).unwrap();
        e.on_sample_mid(0, T0 + 3, 1_000_000_000_000).unwrap();
        e.tick(T0 + 4);
        let _ = e.drain_batches(&BatchPolicy::MAX);
        assert!(e.positions().is_empty() && e.queued_events() == 0);
        e.st.lock_until = T0 + 40_000; // an active breach lock
        e.st.b += 10_000_000; // a phase that was won
        e.frozen = true;
        let (seq, head, archive, intents, nonces, trades, sh, ih) =
            (e.seq, e.head, e.archive.clone(), e.intents.clone(), e.nonces.clone(), e.trades_closed, e.samples_hasher.clone().finalize(), e.intents_hasher.clone().finalize());
        let c = chain_of(&e, 3);
        assert_eq!(e.phase_started(3, &c), Ok(()));
        assert_eq!((e.phase(), e.is_frozen()), (3, false));
        assert_eq!((e.st.b, e.st.q_tally, e.st.latch, e.st.tight_on, e.st.flat), (e.acct.size_micro, 0, false, false, true));
        assert_eq!(e.st.lock_until, T0 + 40_000, "the 12 h breach lock is kept across a phase change (start_phase keeps lock_until on chain)");
        assert_eq!((e.seq, e.head, &e.archive, &e.intents, &e.nonces, e.trades_closed), (seq, head, &archive, &intents, &nonces, trades));
        assert_eq!((e.samples_hasher.clone().finalize(), e.intents_hasher.clone().finalize()), (sh, ih));
    }

    #[test]
    fn phase_started_accepts_1_to_2_2_to_3_3_to_4_4_to_5_and_nothing_else() {
        for have in 1..=5u8 {
            for want in 0..=7u8 {
                let mut e = frozen_idle();
                e.phase = have;
                let c = chain_of(&e, want);
                let ok = matches!((have, want), (1, 2) | (2, 3) | (3, 4) | (4, 5));
                let r = e.phase_started(want, &c);
                if ok {
                    assert_eq!(r, Ok(()), "{have} -> {want}");
                    assert_eq!(e.phase(), want);
                } else {
                    let expected = if have == 5 { 0 } else { have + 1 };
                    assert_eq!(r, Err(LifecycleError::WrongPhase { have: want, want: expected }), "{have} -> {want}");
                    assert_eq!(e.phase(), have, "a refused call changes nothing");
                }
            }
        }
    }

    /// One refused call: the error, and a full fingerprint before and after.
    fn refused<F: FnOnce(&mut Evaluator) -> Result<(), LifecycleError>>(mut e: Evaluator, expected: LifecycleError, call: F) {
        let before = fingerprint(&e);
        assert_eq!(call(&mut e), Err(expected));
        assert_eq!(fingerprint(&e), before, "{expected:?} must change nothing");
    }

    #[test]
    fn phase_started_refuses_every_violated_precondition_and_changes_nothing() {
        let go = |e: &mut Evaluator| {
            let c = chain_of(e, 3);
            e.phase_started(3, &c)
        };
        // Ended: a fault, and an external end
        let mut e = frozen_idle();
        e.fail_internal(EvalError::Overflow);
        refused(e, LifecycleError::Ended, go);
        let mut e = frozen_idle();
        e.ended_externally = true;
        refused(e, LifecycleError::Ended, go);
        // NotActive: the journal ended with a Fail
        let mut e = frozen_idle();
        e.status = Status::Failed(engine::FailReason::StaticFloor);
        refused(e, LifecycleError::NotActive, go);
        // NotFrozen: no pass was requested
        refused(ev_for_unit_tests(), LifecycleError::NotFrozen, go);
        // WrongPhase
        refused(frozen_idle(), LifecycleError::WrongPhase { have: 4, want: 3 }, |e| {
            let c = chain_of(e, 4);
            e.phase_started(4, &c)
        });
        // NotFlat
        let mut e = frozen_idle();
        e.positions.insert(0, pos());
        refused(e, LifecycleError::NotFlat, go);
        // PendingOrders
        let mut e = frozen_idle();
        e.pending.push(Pending { nonce: 5, intent: intent(IntentKind::Limit, T0) });
        refused(e, LifecycleError::PendingOrders, go);
        // QueuedEvents
        let mut e = ev_for_unit_tests();
        e.on_sample_mid(0, T0, 1_000_000_000_000).unwrap();
        e.on_order(&signed(7, &intent(IntentKind::Market, T0))).unwrap();
        e.tick(T0 + 2);
        e.positions.clear();
        e.frozen = true;
        assert!(e.queued_events() > 0);
        refused(e, LifecycleError::QueuedEvents, go);
        // OrdersQueued: accepted in the second of the pass request, not executed yet (the hazard of design 7.4)
        let mut e = ev_for_unit_tests();
        e.on_sample_mid(0, T0, 1_000_000_000_000).unwrap();
        e.on_order(&signed(7, &intent(IntentKind::Market, T0))).unwrap();
        e.frozen = true;
        refused(e, LifecycleError::OrdersQueued, go);
        // ChainMismatch, one field at a time
        for (what, edit) in [
            ("seq", (|c: &mut ChainView| c.seq += 1) as fn(&mut ChainView)),
            ("journal_hash", |c| c.journal_hash[0] ^= 1),
            ("b", |c| c.b += 1),
            ("flat", |c| c.flat = false),
            ("phase", |c| c.phase = 2),
        ] {
            refused(frozen_idle(), LifecycleError::ChainMismatch { what }, |e| {
                let mut c = chain_of(e, 3);
                edit(&mut c);
                e.phase_started(3, &c)
            });
        }
    }

    #[test]
    fn pass_resolved_accepts_a_frozen_account_and_refuses_the_rest_without_a_change() {
        let mut e = frozen_idle();
        assert_eq!(e.pass_resolved(), Ok(()));
        assert!(!e.is_frozen() && e.phase() == 2);
        refused(ev_for_unit_tests(), LifecycleError::NotFrozen, |e| e.pass_resolved());
        let mut f = frozen_idle();
        f.fail_internal(EvalError::Overflow);
        refused(f, LifecycleError::Ended, |e| e.pass_resolved());
        let mut f = frozen_idle();
        f.ended_externally = true;
        refused(f, LifecycleError::Ended, |e| e.pass_resolved());
        let mut f = frozen_idle();
        f.status = Status::Failed(engine::FailReason::PhaseQuick);
        refused(f, LifecycleError::NotActive, |e| e.pass_resolved());
    }

    #[test]
    fn set_held_follows_the_programs_hold_rules() {
        let mut e = ev_for_unit_tests();
        assert_eq!(e.set_held(true), Ok(()));
        assert!(e.held);
        refused(e, LifecycleError::AlreadyHeld, |e| e.set_held(true));
        let mut e = ev_for_unit_tests();
        e.held = true;
        assert_eq!(e.set_held(false), Ok(()));
        assert!(!e.held);
        refused(ev_for_unit_tests(), LifecycleError::NotHeld, |e| e.set_held(false));
        // hold_account needs an Active account; release_hold works on any status (a Failed + held account is released or failed for violation)
        let mut f = ev_for_unit_tests();
        f.status = Status::Failed(engine::FailReason::StaticFloor);
        refused(f, LifecycleError::NotActive, |e| e.set_held(true));
        let mut f = ev_for_unit_tests();
        f.status = Status::Failed(engine::FailReason::StaticFloor);
        f.held = true;
        assert_eq!(f.set_held(false), Ok(()), "release_hold works on a failed account");
        // nothing after an end
        let mut f = ev_for_unit_tests();
        f.ended_externally = true;
        refused(f, LifecycleError::Ended, |e| e.set_held(true));
        let mut f = ev_for_unit_tests();
        f.fail_internal(EvalError::Overflow);
        refused(f, LifecycleError::Ended, |e| e.set_held(true));
    }

    #[test]
    fn end_externally_ends_once_and_also_a_failed_account() {
        let mut e = ev_for_unit_tests();
        assert_eq!(e.end_externally(), Ok(()));
        assert!(e.is_ended() && e.is_frozen());
        refused(e, LifecycleError::AlreadyEnded, |e| e.end_externally());
        // fail_for_violation on a Failed + held account: the evaluator is Failed already and is still ended externally
        let mut f = ev_for_unit_tests();
        f.status = Status::Failed(engine::FailReason::StaticFloor);
        assert_eq!(f.end_externally(), Ok(()));
        assert!(f.ended_externally);
        // a faulted evaluator can still be ended
        let mut g = ev_for_unit_tests();
        g.fail_internal(EvalError::Overflow);
        assert_eq!(g.end_externally(), Ok(()));
    }

    #[test]
    fn the_error_type_is_a_std_error_with_a_text() {
        let all = [
            LifecycleError::Ended, LifecycleError::NotFrozen, LifecycleError::NotActive, LifecycleError::NotFlat, LifecycleError::PendingOrders,
            LifecycleError::QueuedEvents, LifecycleError::OrdersQueued, LifecycleError::WrongPhase { have: 4, want: 3 },
            LifecycleError::ChainMismatch { what: "seq" }, LifecycleError::AlreadyHeld, LifecycleError::NotHeld, LifecycleError::AlreadyEnded,
        ];
        let texts: std::collections::BTreeSet<String> = all.iter().map(|e| e.to_string()).collect();
        assert_eq!(texts.len(), all.len(), "every variant has its own text");
        let boxed: Box<dyn std::error::Error> = Box::new(LifecycleError::WrongPhase { have: 4, want: 3 });
        assert!(boxed.to_string().contains('4') && boxed.to_string().contains('3'));
    }
}
