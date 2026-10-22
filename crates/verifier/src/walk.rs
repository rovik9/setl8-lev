//! Stage 2: the archive walk. The samples are replayed in stream order against the account as the JOURNAL says it is, and
//! whatever the engine says must have happened at a sample becomes an expectation the journal has to satisfy.
//!
//! Which sample is "the sample after which the evaluator must have acted": every sample of a market that is OPEN
//! (inside a session window and with a price change within `max_age_secs`, DESIGN 3.4 and decision 49). The decision for a
//! sample uses:
//!
//!  * the account state as the journal left it BEFORE the first event of that sample's second (an evaluator acts on the
//!    state it had when the second began; an action changes the state for the NEXT second),
//!  * the marks = the latest sample of every market that has an open position (longs at bid, shorts at ask),
//!  * `engine::evaluate_sample` with equity `E = b + P - estimated closing commission`, then, if the floors allow it,
//!    `check_protective` for the positions of the sample's market and `check_pending` for the pending orders of the market.
//!
//! The tolerance is NONE: touching counts (`E <= F_s`, `E <= L`, `bid <= sl`, ...), and one micro-USD decides.
//! An intent (pending order, cancel, set_sl_tp) is effective from the first sample whose second is STRICTLY greater than
//! its `client_time`. An evaluator acts at most once per second and kind: when several samples of one second produce the
//! same kind of action, the LAST one's numbers are the ones expected.
//!
//! M9b (decisions D1 to D4 of handover/design-decisions.md 53): a pending order that only reduces is executed by a `Close`
//! under its nonce; a triggered pending order whose margin is insufficient (computed here with the evaluator's inputs) is
//! expected NOT to fill and stays pending; a breach whose forced closes end at or below the static floor, or over the quick
//! limit, is expected as `Fail` instead of `Breach`.
//!
//! The walk runs up to the largest event time or `Meta.time` seen ("Meta.time = the time of the last sample the evaluator
//! processed for the batch"); an expectation of an earlier second that no event satisfied is a miss.
use std::collections::{BTreeMap, HashMap, HashSet};

use engine::{AccountState, ProtectiveKind, SampleAction};
use journal_types::{market_open, CloseReason, FailReason, OrderKind};

use crate::finding::FindingClass;
use crate::intents::{IntentKind, OrderIntent};
use crate::replay::{Ctx, FloorKind, Pos, Replay};

/// Why the evaluator refuses an order that opens or adds exposure, as far as the verifier can reproduce it (M9c, B1 and B2).
/// The order is the evaluator's own (`exec_trade`): lock, market status, the quantity of a flip's remainder, the number of
/// open positions, the notional cap of the position the order leaves behind, and the margin. The hold comes from the
/// `account_held` / `hold_released` lines of the log (an indexer's lines, dated with the block time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The account is held (`account_held` line without a `hold_released` before the second).
    Held,
    /// `time < lock_until` (the 12 h breach lock).
    Locked,
    /// The market is disabled or reduce-only (`status != 1`).
    NotTradable,
    /// The market has no half-spread in the account's rule set (`spread_e6 == 0`, decision 64): an exposure increase is refused.
    Unpriced,
    /// The quantity opened is not a multiple of the market's step or below its minimum (the whole order of a market fill, the
    /// remainder of a flip).
    Quantity,
    /// Eight positions are open already and the order would open a ninth market.
    TooManyPositions,
    /// The position the order leaves behind is above the market's `max_notional_usd` (compared in micro-USD, as the evaluator does).
    NotionalCap,
    /// `b' + P' - closing commission - this fill's commission` is below the margin of the positions it leaves.
    Margin,
}

impl Refusal {
    pub fn describe(self) -> &'static str {
        match self {
            Refusal::Held => "an exposure increase on a held account",
            Refusal::Locked => "an exposure increase during the breach lock",
            Refusal::NotTradable => "an exposure increase in a market that is disabled or reduce-only",
            Refusal::Unpriced => "an exposure increase in an unpriced market",
            Refusal::Quantity => "a quantity off the market's step or below its minimum",
            Refusal::TooManyPositions => "a ninth open position",
            Refusal::NotionalCap => "a position above the market's notional cap",
            Refusal::Margin => "a position the account cannot margin",
        }
    }
}

/// Which of the evaluator's checks `open_refusal` runs.
#[derive(Clone, Copy, Debug)]
struct Check {
    /// The breach lock (a pending order is not triggered while locked, the walk has already left it out).
    lock: bool,
    /// The quantity of the order itself (a pending order was validated when it was placed; a market order is judged here).
    whole_qty: bool,
}

impl Check {
    const PENDING: Check = Check { lock: false, whole_qty: false };
    const MARKET: Check = Check { lock: true, whole_qty: true };
}

/// The evaluator's cap on simultaneously open positions (`evaluator::config::MAX_POSITIONS`); the program does not enforce it.
pub const MAX_POSITIONS: usize = 8;

/// The verdict for a triggered pending order.
enum Sim {
    /// The evaluator refuses it for a reason the verifier reproduces: it stays pending and no event is expected.
    Refused(Refusal),
    /// It fills; the simulated account afterwards, and whether the phase ended (the quick limit) on the way.
    Accepted(AccountState, BTreeMap<u16, Pos>, bool),
    /// Something did not compute: fall back to "it fills" (the pre-M9b expectation).
    Unknown,
}

/// What a breach sample leads to once the forced closes are replayed with the engine (decision D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreachShape {
    /// `Breach`: the closes end above the static floor and within the quick limit.
    Breach,
    /// `Fail{StaticFloor}`: the closes end at or below the static floor (the program refuses the `Breach`).
    StaticFloor,
    /// `Fail{PhaseQuick}`: the closes push the quick tally over its limit (nothing but the `Fail` may follow).
    PhaseQuick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpKind {
    Floor(FloorKind, [i64; 5]),
    Trigger(u16, ProtectiveKind, u64),
    Pending(u16, u32, u64),
}

#[derive(Clone, Debug)]
pub struct Expect {
    pub sec: i64,
    pub kind: ExpKind,
    pub market: u16,
    pub sample: usize,
}

#[derive(Clone, Debug)]
pub struct Walker {
    pub cursor: usize,
    marks: BTreeMap<u16, usize>,
    last_change: BTreeMap<u16, i64>,
    last_value: BTreeMap<u16, i128>,
    pub exp: Vec<Expect>,
    filled: HashSet<u32>,
    pending: Vec<(u32, OrderIntent)>,
    cancels: Vec<(i64, u32)>,
    sltp: Vec<(i64, OrderIntent)>,
    sltp_cursor: usize,
    /// Pending orders the samples of the second being walked trigger: (market, nonce) -> (price, sample index). Judged for
    /// margin when the second is complete (`resolve_second`).
    cand: BTreeMap<(u16, u32), (u64, usize)>,
    /// (second, market, nonce, reason) of triggered pending orders the evaluator refuses: the order stays pending, no event expected.
    refused: Vec<(i64, u16, u32, Refusal)>,
    /// For every sample walked so far (by archive index): the second its market's price last changed, as of that sample.
    lc_at: Vec<i64>,
    /// (second, nonce) of a pending flip whose `Close` was seen: its opening `Fill` is the other half, not a second execution.
    flip_open: HashMap<(i64, u32), Option<usize>>,
    /// For the seconds with a Breach or Fail decision: the marks (market -> archive sample) the decision used for the markets that
    /// had a position; the forced closes of that second must be priced with exactly these samples (D-F12, D-F21).
    floor_marks: BTreeMap<i64, Vec<(u16, usize)>>,
}

impl Walker {
    /// Adds an expectation; one per (second, kind, market, order): the last sample of the second wins.
    pub fn push(&mut self, e: Expect) {
        let same = |x: &Expect| {
            x.sec == e.sec
                && x.market == e.market
                && match (&x.kind, &e.kind) {
                    (ExpKind::Floor(a, _), ExpKind::Floor(b, _)) => a == b,
                    (ExpKind::Trigger(_, a, _), ExpKind::Trigger(_, b, _)) => a == b,
                    (ExpKind::Pending(_, a, _), ExpKind::Pending(_, b, _)) => a == b,
                    _ => false,
                }
        };
        if let Some(i) = self.exp.iter().position(same) {
            self.exp[i] = e;
        } else {
            self.exp.push(e);
        }
    }

    pub fn new(pending: Vec<(u32, OrderIntent)>, cancels: Vec<(i64, u32)>, mut sltp: Vec<(i64, OrderIntent)>) -> Walker {
        sltp.sort_by_key(|(t, _)| *t);
        Walker { cursor: 0, marks: BTreeMap::new(), last_change: BTreeMap::new(), last_value: BTreeMap::new(), exp: vec![], filled: HashSet::new(), pending, cancels, sltp, sltp_cursor: 0, cand: BTreeMap::new(), refused: vec![], lc_at: Vec::new(), flip_open: HashMap::new(), floor_marks: BTreeMap::new() }
    }
}

fn class_of_miss(k: &ExpKind) -> FindingClass {
    match k {
        ExpKind::Floor(FloorKind::TightStart, _) => FindingClass::MissedTightStart,
        ExpKind::Floor(FloorKind::FloorRaised, _) => FindingClass::MissedFloorRaise,
        ExpKind::Floor(FloorKind::Breach, _) => FindingClass::MissedBreach,
        ExpKind::Floor(FloorKind::Fail(_), _) => FindingClass::MissedFail,
        ExpKind::Trigger(..) | ExpKind::Pending(..) => FindingClass::TriggerMissed,
    }
}

impl<'a> Replay<'a> {
    /// Builds the walker from the committed, correctly signed intents.
    pub fn init_walk(&mut self) {
        let limit = if self.intent_prefix.is_empty() {
            self.intents.lines.len()
        } else {
            let bytes = self.intent_prefix.values().copied().max().unwrap_or(0);
            self.intents.lines_in_prefix(bytes)
        };
        let (mut pending, mut cancels, mut sltp) = (vec![], vec![], vec![]);
        for (i, l) in self.intents.lines.iter().enumerate().take(limit) {
            if self.bad_sig_lines.contains(&i) {
                continue;
            }
            let Ok(it) = l.intent else { continue };
            match it.kind {
                IntentKind::Limit | IntentKind::Stop => pending.push((l.nonce, it)),
                IntentKind::Cancel => cancels.push((it.client_time, it.target_nonce)),
                IntentKind::SetSlTp => sltp.push((it.client_time, it)),
                _ => {}
            }
        }
        self.walk = Some(Walker::new(pending, cancels, sltp));
    }

    /// Runs the walk through every sample whose second is `<= upto`, then reports the expectations of earlier seconds that
    /// nothing satisfied. `prev_seq` is the chain sequence the state is at (findings are attributed to it).
    pub fn walk_advance(&mut self, upto: i64, prev_seq: u64) {
        let Some(mut w) = self.walk.take() else { return };
        let arch = self.arch;
        let mut cur: Option<i64> = None;
        while let Some(s) = arch.samples.get(w.cursor) {
            if arch.is_bad(w.cursor) {
                w.cursor += 1; // a poison record (D-F10/F11) is reported by `check_archive` and skipped
                continue;
            }
            if s.ts_sec() > upto {
                break;
            }
            if let Some(c) = cur.filter(|c| *c != s.ts_sec()) {
                self.resolve_second(&mut w, c);
            }
            cur = Some(s.ts_sec());
            let si = w.cursor;
            w.cursor += 1;
            self.walk_step(&mut w, si);
        }
        if let Some(c) = cur {
            self.resolve_second(&mut w, c);
        }
        self.expire(&mut w, upto, prev_seq);
        self.walk = Some(w);
    }

    fn expire(&mut self, w: &mut Walker, upto: i64, prev_seq: u64) {
        w.refused.retain(|(s, _, _, _)| *s >= upto);
        w.flip_open.retain(|(s, _), _| *s >= upto);
        w.floor_marks.retain(|s, _| *s >= upto);
        let old = std::mem::take(&mut w.exp);
        for e in old {
            if e.sec < upto {
                self.report_miss(&e, prev_seq);
            } else {
                w.exp.push(e);
            }
        }
    }

    fn report_miss(&mut self, e: &Expect, prev_seq: u64) {
        let ctx = Ctx { seq: if prev_seq == 0 { None } else { Some(prev_seq) }, idx: None, time: e.sec };
        let class = class_of_miss(&e.kind);
        let what = match &e.kind {
            ExpKind::Floor(k, v) => format!("{k:?} with numbers {v:?}"),
            ExpKind::Trigger(m, k, p) => format!("{k:?} of market {m} at {p}"),
            ExpKind::Pending(m, n, p) => format!("pending order {n} of market {m} at {p}"),
        };
        self.f.add_once(class, ctx, format!("{what} in the journal at second {}", e.sec), "nothing", format!("the samples (record {}, second {}) require it but the journal has no such event", e.sample, e.sec));
    }

    fn walk_step(&mut self, w: &mut Walker, si: usize) {
        let Some(s) = self.arch.samples.get(si).copied() else { return };
        let (m, sec) = (s.market, s.ts_sec());
        w.marks.insert(m, si);
        if w.last_value.get(&m) != Some(&s.value) {
            w.last_change.insert(m, sec);
        }
        w.last_value.insert(m, s.value);
        w.lc_at.push(w.last_change.get(&m).copied().unwrap_or(sec));
        if self.terminal {
            return;
        }
        let Some(mk) = self.rules.markets.get(&m) else { return };
        let last = w.last_change.get(&m).copied().unwrap_or(sec);
        let count = u8::try_from(mk.sessions.len()).unwrap_or(u8::MAX);
        if !market_open(&mk.sessions, count, sec, last, u32::from(mk.max_age_secs)) {
            return; // frozen mark: nothing triggers on a closed market
        }
        // set_sl_tp intents that became effective
        while let Some((t, it)) = w.sltp.get(w.sltp_cursor) {
            if *t >= sec {
                break;
            }
            if let Some(p) = self.positions.get_mut(&it.market) {
                if p.p.opened_at <= *t {
                    p.sl = it.sl_e12;
                    p.tp = it.tp_e12;
                }
            }
            w.sltp_cursor += 1;
        }
        let mut floors_allow_orders = true;
        if !self.positions.is_empty() {
            let Some((marks, p_open, comm)) = self.marks_and_profit(w) else { return };
            let _ = marks;
            if let Ok(d) = engine::evaluate_sample(&self.st, &self.engine_rules, p_open, comm, sec) {
                let l_before = self.st.l_level;
                let (kind, vals) = match d.action {
                    SampleAction::None => (None, [0; 5]),
                    SampleAction::TightStart { r, l_start, g_peak, l_level } => (Some(FloorKind::TightStart), [p_open, r, l_start, g_peak, l_level]),
                    SampleAction::FloorRaised { g_peak, l_level } => (Some(FloorKind::FloorRaised), [0, 0, 0, g_peak, l_level]),
                    SampleAction::Breach { .. } => match self.breach_shape(w, sec) {
                        BreachShape::Breach => (Some(FloorKind::Breach), [0, 0, 0, 0, l_before]),
                        BreachShape::StaticFloor => (Some(FloorKind::Fail(FailReason::StaticFloor)), [0; 5]),
                        BreachShape::PhaseQuick => (Some(FloorKind::Fail(FailReason::PhaseQuick)), [0; 5]),
                    },
                    SampleAction::Fail(r) => (
                        Some(FloorKind::Fail(match r {
                            engine::FailReason::StaticFloor => FailReason::StaticFloor,
                            engine::FailReason::PhaseQuick => FailReason::PhaseQuick,
                        })),
                        [0; 5],
                    ),
                };
                if let Some(k) = kind {
                    // D-F15 (decision 68): a breach-shaped and a static-fail sample in ONE second: the more severe outcome, the Fail,
                    // wins; the evaluator acts once per second and posts the Fail
                    let fail_here = |w: &Walker| w.exp.iter().any(|e| e.sec == sec && matches!(&e.kind, ExpKind::Floor(FloorKind::Fail(_), _)));
                    if matches!(k, FloorKind::Fail(_)) {
                        w.exp.retain(|e| !(e.sec == sec && matches!(&e.kind, ExpKind::Floor(FloorKind::Breach, _))));
                    }
                    if !(k == FloorKind::Breach && fail_here(w)) {
                        if matches!(k, FloorKind::Breach | FloorKind::Fail(_)) {
                            let snap: Vec<(u16, usize)> = self.positions.keys().filter_map(|m| w.marks.get(m).map(|i| (*m, *i))).collect();
                            w.floor_marks.insert(sec, snap);
                        }
                        w.push(Expect { sec, kind: ExpKind::Floor(k, vals), market: 0, sample: si });
                    }
                }
                floors_allow_orders = engine::sl_tp_allowed_after(&d.action);
            }
        }
        if !floors_allow_orders {
            return;
        }
        // SL / TP of this market's position
        if let Some(p) = self.positions.get(&m).copied() {
            let (bid, ask) = self.bid_ask_of(w, m).unwrap_or((0, 0));
            let side = if p.p.qty.is_positive() { engine::Side::Buy } else { engine::Side::Sell };
            if let Some(t) = engine::check_protective(side, p.sl, p.tp, bid, ask, true) {
                w.push(Expect { sec, kind: ExpKind::Trigger(m, t.kind, t.price_e12), market: m, sample: si });
            }
        }
        // pending orders of this market
        let locked = !engine::can_increase_exposure(&self.st, sec);
        let (bid, ask) = self.bid_ask_of(w, m).unwrap_or((0, 0));
        let live: Vec<(u32, OrderIntent)> = w
            .pending
            .iter()
            .filter(|(n, it)| {
                it.market == m
                    && it.client_time < sec
                    && !w.filled.contains(n)
                    && !w.cancels.iter().any(|(ct, target)| target == n && *ct < sec)
            })
            .copied()
            .collect();
        for (nonce, it) in live {
            let kind = if it.kind == IntentKind::Limit { engine::PendingKind::Limit } else { engine::PendingKind::Stop };
            let side = if it.side == 0 { engine::Side::Buy } else { engine::Side::Sell };
            if let Some(price) = engine::check_pending(side, kind, it.price_e12, bid, ask, true, locked) {
                // the last sample of the second wins (as for every other expectation); margin is judged at the end of the second
                w.cand.insert((m, nonce), (price, si));
            }
        }
    }

    fn bid_ask_of(&self, w: &Walker, market: u16) -> Option<(u64, u64)> {
        let si = *w.marks.get(&market)?;
        let mid = self.arch.samples.get(si)?.mid_e12()?;
        crate::replay::mid_pair(mid, self.rules.spread(market))
    }

    /// Open profit of all positions and the estimated closing commission at the current marks.
    fn marks_and_profit(&self, w: &Walker) -> Option<(usize, i64, i64)> {
        let (p, comm) = self.profit_for(w, &self.positions, None)?;
        Some((self.positions.len(), p, comm))
    }

    /// Open profit and estimated closing commission of `positions` at the walk's marks; `mark` replaces the bid and ask of
    /// one market (the trigger sample of a pending order).
    fn profit_for(&self, w: &Walker, positions: &BTreeMap<u16, Pos>, mark: Option<(u16, u64, u64)>) -> Option<(i64, i64)> {
        let mut list = Vec::new();
        let mut comm = 0i64;
        for (m, p) in positions {
            let (bid, ask) = match mark {
                Some((mm, b, a)) if mm == *m => (b, a),
                _ => self.bid_ask_of(w, *m)?,
            };
            list.push((p.p, bid, ask));
            let price = if p.p.qty.is_positive() { bid } else { ask };
            comm = comm.checked_add(self.commission_of(*m, p.p.qty.unsigned_abs(), price)?)?;
        }
        Some((engine::open_profit_micro(&list).ok()?, comm))
    }

    /// `ceil(rate x margin_micro(qty, price, 1))` of the market's class (0 without a rate).
    fn commission_of(&self, market: u16, qty: u64, price: u64) -> Option<i64> {
        let class = self.rules.class_of(market)?;
        let rate = self.rules.commission_e6.get(class).copied().unwrap_or(0);
        if rate == 0 {
            return Some(0);
        }
        let notional = engine::margin_micro(qty, price, 1).ok()?;
        engine::fee_ceil_micro(notional, rate).ok()
    }

    fn bid_ask_of_sample(&self, si: usize) -> Option<(u64, u64)> {
        let s = self.arch.samples.get(si)?;
        crate::replay::mid_pair(s.mid_e12()?, self.rules.spread(s.market))
    }

    // ------------------------------------------------------------------ D4: what a breach sample leads to

    /// Replays the forced closes of a breach at the current marks the way the evaluator books them (the slice that adds least
    /// to the quick tally first, a commission after every slice that does not overshoot the limit) and says whether the
    /// result is a `Breach`, a `Fail{StaticFloor}` (B at or below the static floor) or a `Fail{PhaseQuick}` (tally over the
    /// limit). The program accepts either `Fail` reason when both conditions hold (`report_fail` checks the reason it is given
    /// against its own condition); the static floor is named first, as in `check_breach`.
    fn breach_shape(&self, w: &Walker, sec: i64) -> BreachShape {
        let mut st = self.st;
        let mut todo: Vec<(i64, u16, u64, u64)> = Vec::new();
        for (m, p) in &self.positions {
            let Some((bid, ask)) = self.bid_ask_of(w, *m) else { return BreachShape::Breach };
            let long = p.p.qty.is_positive();
            let price = if long { bid } else { ask };
            let qty = p.p.qty.unsigned_abs();
            let side = if long { engine::Side::Sell } else { engine::Side::Buy };
            let Ok(out) = engine::apply_fill(Some(p.p), side, qty, price, sec, engine::day_index(sec)) else { return BreachShape::Breach };
            todo.push((out.quick_tally_delta, *m, qty, price));
        }
        todo.sort_by_key(|x| (x.0, x.1));
        let mut over = false;
        for (_, m, qty, price) in todo {
            let Some(p) = self.positions.get(&m) else { return BreachShape::Breach };
            let long = p.p.qty.is_positive();
            let side = if long { engine::Side::Sell } else { engine::Side::Buy };
            let Ok(out) = engine::apply_fill(Some(p.p), side, qty, price, sec, engine::day_index(sec)) else { return BreachShape::Breach };
            let Some(slice) = out.closed else { return BreachShape::Breach };
            let Ok(d) = engine::apply_closed_slice(&st, &self.engine_rules, slice.credit, out.quick_tally_delta) else { return BreachShape::Breach };
            st = d.state;
            if d.fail.is_some() {
                over = true;
            } else {
                let Some(fee) = self.commission_of(m, qty, price) else { return BreachShape::Breach };
                let Ok(d2) = engine::apply_closed_slice(&st, &self.engine_rules, fee.saturating_neg(), 0) else { return BreachShape::Breach };
                st = d2.state;
            }
        }
        match engine::static_floor_micro(&st) {
            Ok(floor) if st.b <= floor => BreachShape::StaticFloor,
            _ if over => BreachShape::PhaseQuick,
            _ => BreachShape::Breach,
        }
    }

    // ------------------------------------------------------------------ D3: margin of a triggered pending order

    /// The end of a second: the pending orders its samples triggered are judged for margin, in the evaluator's order (the
    /// stop-loss and take-profit closes of the second first, then the orders by (market, nonce)), each against the account
    /// as the earlier ones left it. An order the account cannot margin stays pending and nothing is expected for it; the
    /// others become `Pending` expectations.
    fn resolve_second(&mut self, w: &mut Walker, sec: i64) {
        let cands = std::mem::take(&mut w.cand);
        if cands.is_empty() {
            return;
        }
        let mut st = self.st;
        let mut pos = self.positions.clone();
        let mut ended = false;
        let mut trig: Vec<(u16, u8, u64)> = w
            .exp
            .iter()
            .filter(|e| e.sec == sec)
            .filter_map(|e| match &e.kind {
                ExpKind::Trigger(m, k, p) => Some((*m, u8::from(*k == ProtectiveKind::TakeProfit), *p)),
                _ => None,
            })
            .collect();
        trig.sort();
        for (m, _, price) in trig {
            let Some(p) = pos.get(&m).copied() else { continue };
            match self.sim_close(&mut st, &mut pos, m, p.p.qty.unsigned_abs(), price, sec) {
                Some(false) => {}
                // the close pushed the quick tally over its limit (the account fails), or the arithmetic did not fit
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        for ((m, nonce), (price, si)) in cands {
            if ended {
                break;
            }
            match self.sim_pending(w, &st, &pos, m, nonce, price, si, sec) {
                Sim::Refused(why) => w.refused.push((sec, m, nonce, why)),
                Sim::Accepted(st2, pos2, over) => {
                    w.push(Expect { sec, kind: ExpKind::Pending(m, nonce, price), market: m, sample: si });
                    st = st2;
                    pos = pos2;
                    ended = over;
                }
                Sim::Unknown => w.push(Expect { sec, kind: ExpKind::Pending(m, nonce, price), market: m, sample: si }),
            }
        }
    }

    /// Books a closing slice of `qty` at `price` on the simulated account; `Some(true)` when it overshoots the quick limit.
    fn sim_close(&self, st: &mut AccountState, pos: &mut BTreeMap<u16, Pos>, m: u16, qty: u64, price: u64, sec: i64) -> Option<bool> {
        let p = pos.get(&m).copied()?;
        let side = if p.p.qty.is_positive() { engine::Side::Sell } else { engine::Side::Buy };
        let out = engine::apply_fill(Some(p.p), side, qty, price, sec, engine::day_index(sec)).ok()?;
        let slice = out.closed?;
        let d = engine::apply_closed_slice(st, &self.engine_rules, slice.credit, out.quick_tally_delta).ok()?;
        *st = d.state;
        match out.position {
            Some(np) => {
                pos.insert(m, Pos { p: np, ..p });
            }
            None => {
                pos.remove(&m);
            }
        }
        if d.fail.is_some() {
            return Some(true);
        }
        let fee = self.commission_of(m, qty, price)?;
        *st = engine::apply_closed_slice(st, &self.engine_rules, fee.checked_neg()?, 0).ok()?.state;
        Some(false)
    }

    /// The evaluator's checks for an order that opens or adds exposure (`exec_trade`, in its order), as far as the verifier
    /// can reproduce them. `order_qty` is the whole order; if it is against an open position of this market the first
    /// `|position|` of it only closes (a flip), the rest opens. `Err(())`: something did not compute (no mark, overflow).
    ///
    /// 0. `Held`: the log holds the account at this second.
    /// 1. `Locked` (`check.lock`): `time < lock_until`. A pending order never gets here locked (the walk does not trigger it).
    /// 2. `NotTradable`: the market is not enabled (disabled or reduce-only).
    /// 3. `Unpriced`: the market's half-spread in the account's rule set is 0 (decision 64, "unpriced market").
    /// 4. `Quantity`: the quantity opened (the whole order of a plain open, the remainder of a flip) or, for a market order
    ///    (`check.whole_qty`), the whole order is off the step or below the minimum.
    /// 5. `TooManyPositions`: a new market while `MAX_POSITIONS` are open.
    /// 6. `NotionalCap`: the position left behind is above `max_notional_usd`.
    /// 7. `Margin`: `b' + P' - closing commission - this fill's commission >= sum of margins` over the positions it leaves,
    ///    with the order's own sample as its market's mark and the walk's marks for the others (DESIGN 7.3, decision 27).
    #[allow(clippy::too_many_arguments)]
    fn open_refusal(
        &self,
        w: &Walker,
        st: &AccountState,
        pos: &BTreeMap<u16, Pos>,
        market: u16,
        long: bool,
        order_qty: u64,
        price: u64,
        (bid, ask): (u64, u64),
        sec: i64,
        check: Check,
    ) -> Result<Option<Refusal>, ()> {
        let side = if long { engine::Side::Buy } else { engine::Side::Sell };
        let existing = pos.get(&market).copied();
        let (close_qty, open_qty) = match existing {
            Some(p) if p.p.qty.is_positive() != long => {
                let c = order_qty.min(p.p.qty.unsigned_abs());
                (c, order_qty.saturating_sub(c))
            }
            _ => (0, order_qty),
        };
        if open_qty == 0 {
            return Ok(None);
        }
        let mk = self.rules.markets.get(&market).ok_or(())?;
        if self.held_at(sec) {
            return Ok(Some(Refusal::Held));
        }
        if check.lock && !engine::can_increase_exposure(st, sec) {
            return Ok(Some(Refusal::Locked));
        }
        if mk.status != 1 {
            return Ok(Some(Refusal::NotTradable));
        }
        if self.rules.spread(market) == 0 {
            return Ok(Some(Refusal::Unpriced));
        }
        let bad = |q: u64| engine::validate_quantity(q, mk.qty_step, mk.qty_min).is_err();
        if bad(open_qty) || (check.whole_qty && bad(order_qty)) {
            return Ok(Some(Refusal::Quantity));
        }
        if existing.is_none() && pos.len() >= MAX_POSITIONS {
            return Ok(Some(Refusal::TooManyPositions));
        }
        let day = engine::day_index(sec);
        let mut b_after = st.b;
        let mut base = existing.map(|p| p.p);
        if close_qty > 0 {
            let o = engine::apply_fill(base, side, close_qty, price, sec, day).map_err(|_| ())?;
            b_after = b_after.checked_add(o.balance_delta).ok_or(())?;
            base = o.position;
        }
        let o = engine::apply_fill(base, side, open_qty, price, sec, day).map_err(|_| ())?;
        let np = o.position.ok_or(())?;
        let mut after = pos.clone();
        after.insert(market, Pos { p: np, sl: 0, tp: 0 });
        let notional = engine::units::notional_micro(np.qty.unsigned_abs(), np.avg_entry_e12).map_err(|_| ())?;
        if notional > i128::from(mk.max_notional_usd) {
            return Ok(Some(Refusal::NotionalCap));
        }
        let (p_open, comm) = self.profit_for(w, &after, Some((market, bid, ask))).ok_or(())?;
        let fill_comm = self.commission_of(market, open_qty, price).ok_or(())?;
        let equity = i128::from(b_after) + i128::from(p_open) - i128::from(comm) - i128::from(fill_comm);
        let mut used: i128 = 0;
        for (m, p) in &after {
            let lev = self.rules.class_of(*m).and_then(|c| self.rules.leverage_x.get(c).copied()).ok_or(())?;
            used += i128::from(engine::margin_micro(p.p.qty.unsigned_abs(), p.p.avg_entry_e12, lev).map_err(|_| ())?);
        }
        if equity - used < 0 {
            return Ok(Some(Refusal::Margin));
        }
        Ok(None)
    }

    /// The evaluator's check for a triggered limit or stop order (DESIGN 7.3, decision 27): net the order into the position,
    /// then `b' + P' - closing commission - this fill's commission >= sum of margins` over the positions it leaves behind,
    /// with the trigger sample as the order's market's mark. Pure reduces skip the check.
    #[allow(clippy::too_many_arguments)]
    fn sim_pending(&self, w: &Walker, st: &AccountState, pos: &BTreeMap<u16, Pos>, market: u16, nonce: u32, price: u64, si: usize, sec: i64) -> Sim {
        let Some((_, it)) = w.pending.iter().find(|(n, _)| *n == nonce) else { return Sim::Unknown };
        let long = it.side == 0;
        let side = if long { engine::Side::Buy } else { engine::Side::Sell };
        let existing = pos.get(&market).copied();
        let (close_qty, open_qty) = match existing {
            Some(p) if p.p.qty.is_positive() != long => {
                let c = it.qty.min(p.p.qty.unsigned_abs());
                (c, it.qty.saturating_sub(c))
            }
            _ => (0, it.qty),
        };
        let day = engine::day_index(sec);
        if open_qty > 0 {
            let Some((bid, ask)) = self.bid_ask_of_sample(si) else { return Sim::Unknown };
            match self.open_refusal(w, st, pos, market, long, it.qty, price, (bid, ask), sec, Check::PENDING) {
                Ok(Some(why)) => return Sim::Refused(why),
                Ok(None) => {}
                Err(()) => return Sim::Unknown,
            }
        }
        // execution on the simulated account
        let mut st2 = *st;
        let mut pos2 = pos.clone();
        if close_qty > 0 {
            match self.sim_close(&mut st2, &mut pos2, market, close_qty, price, sec) {
                Some(false) => {}
                Some(true) => return Sim::Accepted(st2, pos2, true),
                None => return Sim::Unknown,
            }
        }
        if open_qty > 0 {
            let Ok(o) = engine::apply_fill(pos2.get(&market).map(|p| p.p), side, open_qty, price, sec, day) else { return Sim::Unknown };
            let Some(np) = o.position else { return Sim::Unknown };
            pos2.insert(market, Pos { p: np, sl: 0, tp: 0 });
            let Some(fee) = self.commission_of(market, open_qty, price) else { return Sim::Unknown };
            let Some(neg) = fee.checked_neg() else { return Sim::Unknown };
            let Ok(d) = engine::apply_closed_slice(&st2, &self.engine_rules, neg, 0) else { return Sim::Unknown };
            st2 = d.state;
        }
        Sim::Accepted(st2, pos2, false)
    }

    // ------------------------------------------------------------------ M9c: the execution of a MARKET order

    /// The trader's signed intent behind `nonce`, if it decodes.
    fn intent_of(&self, nonce: u32) -> Option<OrderIntent> {
        self.intents.get(nonce).and_then(|(_, l)| l.intent.clone().ok())
    }

    /// Whether `market` is closed at second `sec` as the sample `si` (the order's own sample) saw it: outside its session
    /// windows, or no price change within `max_age_secs` (decision 49). `None` when the walk has not reached the sample.
    fn market_closed(&self, w: &Walker, market: u16, sec: i64, si: usize) -> Option<bool> {
        let mk = self.rules.markets.get(&market)?;
        let last = *w.lc_at.get(si)?;
        let count = u8::try_from(mk.sessions.len()).unwrap_or(u8::MAX);
        Some(!market_open(&mk.sessions, count, sec, last, u32::from(mk.max_age_secs)))
    }

    /// B1 and B2 for the events of a market order that opens or adds exposure (a plain `Fill`, or the `Close{Flip}` that starts
    /// a flip, judged before the close because the evaluator judges the whole order first): the market is open, and
    /// `open_refusal` finds nothing. The account is the one the journal has left before this event, which is the state the
    /// evaluator had when it ran the order (every change of state is an event). A margin shortfall is `FillWithoutMargin`,
    /// the other reasons `FillNotAllowed`.
    #[allow(clippy::too_many_arguments)]
    pub fn check_market_order(&mut self, ctx: Ctx, market: u16, long: bool, order_qty: u64, price: u64, time: i64) {
        let Some(w) = self.walk.take() else { return };
        let verdict = (|| {
            let si = self.last_proof_si?;
            let s = self.arch.samples.get(si)?;
            let bid_ask = crate::replay::mid_pair(s.mid_e12()?, self.rules.spread(market))?;
            if self.market_closed(&w, market, time, si) == Some(true) {
                return Some(Some(None));
            }
            match self.open_refusal(&w, &self.st, &self.positions, market, long, order_qty, price, bid_ask, time, Check::MARKET) {
                Ok(Some(why)) => Some(Some(Some(why))),
                _ => Some(None),
            }
        })();
        self.walk = Some(w);
        match verdict {
            Some(Some(None)) => self.f.add(
                FindingClass::FillNotAllowed,
                ctx,
                "no execution: the market is closed (outside its sessions, or no price change within max_age_secs)",
                format!("a market order executed in market {market}"),
                "the evaluator refuses a market order in a closed market",
            ),
            Some(Some(Some(Refusal::Margin))) => self.f.add(
                FindingClass::FillWithoutMargin,
                ctx,
                "no execution: the account cannot margin this order",
                format!("a market order of {order_qty} executed in market {market}"),
                "a market order was executed although `b + P - commissions` does not cover the margin of the position it leaves",
            ),
            Some(Some(Some(why))) => self.f.add(
                FindingClass::FillNotAllowed,
                ctx,
                format!("no execution: {} is refused", why.describe()),
                format!("a market order of {order_qty} executed in market {market}"),
                format!("a market order was executed although the evaluator's own rules refuse it ({})", why.describe()),
            ),
            _ => {}
        }
    }

    /// B2 for a market order that only reduces (a `Close{Manual}` under a market or close intent): the market must be open, and
    /// a quantity the trader named (`order_qty`, 0 = all) must be on the step and at least the minimum.
    pub fn check_market_close(&mut self, ctx: Ctx, market: u16, time: i64, order_qty: u64) {
        let Some(w) = self.walk.take() else { return };
        let closed = self.last_proof_si.and_then(|si| self.market_closed(&w, market, time, si));
        self.walk = Some(w);
        if closed == Some(true) {
            self.f.add(
                FindingClass::FillNotAllowed,
                ctx,
                "no execution: the market is closed (outside its sessions, or no price change within max_age_secs)",
                format!("a market close executed in market {market}"),
                "the evaluator refuses a market order in a closed market",
            );
            return;
        }
        let off = order_qty != 0 && self.rules.markets.get(&market).is_some_and(|mk| engine::validate_quantity(order_qty, mk.qty_step, mk.qty_min).is_err());
        if off {
            self.f.add(
                FindingClass::FillNotAllowed,
                ctx,
                format!("no execution: {} is refused", Refusal::Quantity.describe()),
                format!("a market close of {order_qty} units in market {market}"),
                format!("an order was executed although the evaluator's own rules refuse it ({})", Refusal::Quantity.describe()),
            );
        }
    }

    /// The nonce of a market order and its intent, if `intent` is the signed intent of a MARKET or CLOSE order of `market`.
    pub fn market_intent(&self, intent: u32, market: u16) -> Option<OrderIntent> {
        if intent == 0 {
            return None;
        }
        let it = self.intent_of(intent)?;
        (it.market == market && matches!(it.kind, IntentKind::Market | IntentKind::Close)).then_some(it)
    }

    // ------------------------------------------------------------------ consumption at journal events

    /// A `Fill` of a limit or stop order must be the fill of a pending order the samples trigger. Returns the archive sample that
    /// triggered it (the one the fill must be priced with, D-F12), if known.
    pub fn walk_fill(&mut self, ctx: Ctx, market: u16, kind: OrderKind, intent: u32) -> Option<usize> {
        let mut w = self.walk.take()?;
        let mut sample = None;
        if matches!(kind, OrderKind::Limit | OrderKind::Stop) {
            // the opening half of a pending flip whose `Close` was already taken as the execution
            sample = match w.flip_open.remove(&(ctx.time, intent)) {
                Some(si) => si,
                None => self.consume_pending(&mut w, ctx, market, intent, &format!("{kind:?} fill of intent {intent}")),
            };
        }
        self.walk = Some(w);
        sample
    }

    /// The execution of a pending order at this second: its `Pending` expectation is consumed; an execution the margin check
    /// refused is a `PendingFillWithoutMargin`; one nothing triggers is a `TriggerFalse`. Returns the trigger sample.
    fn consume_pending(&mut self, w: &mut Walker, ctx: Ctx, market: u16, intent: u32, what: &str) -> Option<usize> {
        let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Pending(m, n, _) if *m == market && *n == intent));
        let mut sample = None;
        match i {
            Some(i) => {
                sample = Some(w.exp.remove(i).sample);
            }
            None => {
                if let Some(r) = w.refused.iter().position(|(s, m, n, _)| *s == ctx.time && *m == market && *n == intent) {
                    let (_, _, _, why) = w.refused.remove(r);
                    if why == Refusal::Margin {
                        self.f.add(
                            FindingClass::PendingFillWithoutMargin,
                            ctx,
                            "no execution: the account cannot margin this pending order, it stays pending",
                            what.to_string(),
                            "a pending order was executed although `b + P - commissions` does not cover the margin of the position it leaves",
                        );
                    } else {
                        self.f.add(
                            FindingClass::FillNotAllowed,
                            ctx,
                            format!("no execution: {} is refused, the order stays pending", why.describe()),
                            what.to_string(),
                            format!("a pending order was executed although the evaluator's own rules refuse it ({})", why.describe()),
                        );
                    }
                } else {
                    self.f.add(FindingClass::TriggerFalse, ctx, "a pending order the samples trigger at this second", what.to_string(), "the execution of a pending order that no sample of this second triggers");
                }
            }
        }
        w.filled.insert(intent);
        sample
    }

    /// A stop-loss or take-profit `Close` must be a trigger the samples produce; a `Manual` (or `Flip`) `Close` under the nonce
    /// of a pending order of that market is the execution of that order (decision D1: a pending order that only reduces a
    /// position has no `Fill`); a `Breach` or `Fail` close must belong to a floor decision of its second (D-F13).
    /// Returns the archive sample the close must be priced with, when the walk knows it (the trigger sample, the pending
    /// order's trigger sample, the breach sample's mark of this market); a market order's sample is the caller's rule.
    pub fn walk_close(&mut self, ctx: Ctx, market: u16, reason: CloseReason, intent: u32, _qty: u64) -> Option<usize> {
        let want = match reason {
            CloseReason::StopLoss => ProtectiveKind::StopLoss,
            CloseReason::TakeProfit => ProtectiveKind::TakeProfit,
            CloseReason::Manual | CloseReason::Flip => {
                if intent == 0 {
                    return None;
                }
                let mut w = self.walk.take()?;
                let mut sample = None;
                if w.pending.iter().any(|(n, it)| *n == intent && it.market == market) {
                    sample = self.consume_pending(&mut w, ctx, market, intent, &format!("{reason:?} close under pending order {intent}"));
                    if reason == CloseReason::Flip {
                        w.flip_open.insert((ctx.time, intent), sample);
                    }
                }
                self.walk = Some(w);
                return sample;
            }
            CloseReason::Breach | CloseReason::Fail => {
                let w = self.walk.take()?;
                let has = w.exp.iter().any(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Floor(FloorKind::Breach | FloorKind::Fail(_), _)));
                let sample = if has { w.floor_marks.get(&ctx.time).and_then(|v| v.iter().find(|(m, _)| *m == market)).map(|(_, i)| *i) } else { None };
                self.walk = Some(w);
                if !has {
                    self.unjustified_forced_close(ctx, market, reason);
                }
                return sample;
            }
        };
        let mut w = self.walk.take()?;
        let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Trigger(m, k, _) if *m == market && *k == want));
        let mut sample = None;
        match i {
            Some(i) => {
                sample = Some(w.exp.remove(i).sample);
            }
            None => self.f.add(
                FindingClass::TriggerFalse,
                ctx,
                format!("a {want:?} trigger of market {market} at this second"),
                format!("{reason:?} close"),
                "a protective close that no sample of this second triggers",
            ),
        }
        self.walk = Some(w);
        sample
    }

    /// A TightStart / FloorRaised / Breach must be the decision the samples produce; its numbers must be the archive's.
    pub fn walk_floor(&mut self, ctx: Ctx, kind: FloorKind, vals: [i64; 5], claims_ok: bool) -> Option<[i64; 5]> {
        let mut found = None;
        if matches!(kind, FloorKind::Fail(_)) {
            // a Fail is justified by the replayed state (stage 1); the walk only reports a MISSED one
            if let Some(w) = self.walk.as_mut() {
                w.exp.retain(|e| !(e.sec == ctx.time && matches!(e.kind, ExpKind::Floor(FloorKind::Fail(_), _))));
            }
            return None;
        }
        let mut w = self.walk.take()?;
        let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Floor(k, _) if *k == kind));
        match i {
            Some(i) => {
                let e = w.exp.remove(i);
                if let ExpKind::Floor(_, exp) = e.kind {
                    found = Some(exp);
                    if claims_ok {
                        let differs = match kind {
                            FloorKind::TightStart => vals[0] != exp[0] || vals[3] != exp[3] || vals[4] != exp[4],
                            FloorKind::FloorRaised => vals[3] != exp[3] || vals[4] != exp[4],
                            _ => false,
                        };
                        if differs {
                            self.f.add(
                                FindingClass::WrongLLevel,
                                ctx,
                                format!("p_open/g_peak/l_level {} / {} / {}", exp[0], exp[3], exp[4]),
                                format!("{} / {} / {}", vals[0], vals[3], vals[4]),
                                format!("{kind:?} numbers differ from the ones the samples give (record {})", e.sample),
                            );
                        }
                    }
                }
            }
            None => {
                let class = match kind {
                    FloorKind::TightStart => FindingClass::FalseTightStart,
                    FloorKind::FloorRaised => FindingClass::FalseFloorRaise,
                    _ => FindingClass::FalseBreach,
                };
                self.f.add(class, ctx, format!("no {kind:?} at second {}", ctx.time), format!("{kind:?}"), "the samples of this second do not call for this event");
            }
        }
        self.walk = Some(w);
        found
    }

    /// End of the replay: walk through the last time seen and report everything still expected.
    pub fn walk_finish(&mut self) {
        let upto = self.max_time;
        let prev = self.last_seq;
        self.walk_advance(upto, prev);
        if let Some(mut w) = self.walk.take() {
            self.expire(&mut w, upto.saturating_add(1), prev);
            self.walk = Some(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walker() -> Walker {
        Walker::new(vec![], vec![], vec![])
    }

    fn floor(sec: i64, k: FloorKind, v: i64) -> Expect {
        Expect { sec, kind: ExpKind::Floor(k, [v; 5]), market: 0, sample: 0 }
    }

    #[test]
    fn the_last_expectation_of_a_second_and_kind_wins() {
        let mut w = walker();
        w.push(floor(10, FloorKind::TightStart, 1));
        w.push(floor(10, FloorKind::TightStart, 2));
        assert_eq!(w.exp.len(), 1);
        assert_eq!(w.exp[0].kind, ExpKind::Floor(FloorKind::TightStart, [2; 5]));
        // another kind, another second, another market: separate expectations
        w.push(floor(10, FloorKind::FloorRaised, 3));
        w.push(floor(11, FloorKind::TightStart, 4));
        let mut other = floor(10, FloorKind::TightStart, 5);
        other.market = 1;
        w.push(other);
        assert_eq!(w.exp.len(), 4);
    }

    #[test]
    fn triggers_and_pending_orders_are_keyed_by_their_own_kind() {
        let mut w = walker();
        let trig = |sec: i64, k: ProtectiveKind, p: u64| Expect { sec, kind: ExpKind::Trigger(0, k, p), market: 0, sample: 0 };
        w.push(trig(5, ProtectiveKind::StopLoss, 1));
        w.push(trig(5, ProtectiveKind::StopLoss, 2));
        w.push(trig(5, ProtectiveKind::TakeProfit, 3));
        assert_eq!(w.exp.len(), 2);
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Trigger(0, ProtectiveKind::StopLoss, 2)));
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Trigger(0, ProtectiveKind::TakeProfit, 3)));
        w.push(trig(5, ProtectiveKind::StopLoss, 4));
        let count = |k: ProtectiveKind| w.exp.iter().filter(|e| matches!(e.kind, ExpKind::Trigger(_, kk, _) if kk == k)).count();
        assert_eq!((w.exp.len(), count(ProtectiveKind::StopLoss), count(ProtectiveKind::TakeProfit)), (2, 1, 1));
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Trigger(0, ProtectiveKind::StopLoss, 4)));
        let pend = |n: u32, p: u64| Expect { sec: 5, kind: ExpKind::Pending(0, n, p), market: 0, sample: 0 };
        w.push(pend(1, 10));
        w.push(pend(1, 11));
        w.push(pend(2, 12));
        assert_eq!(w.exp.len(), 4);
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Pending(0, 1, 11)));
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Pending(0, 2, 12)));
        assert!(!w.exp.iter().any(|e| e.kind == ExpKind::Pending(0, 1, 10)));
        w.push(pend(2, 13));
        let per = |n: u32| w.exp.iter().filter(|e| matches!(e.kind, ExpKind::Pending(_, nn, _) if nn == n)).count();
        assert_eq!((per(1), per(2)), (1, 1));
        assert!(w.exp.iter().any(|e| e.kind == ExpKind::Pending(0, 2, 13)) && w.exp.iter().any(|e| e.kind == ExpKind::Pending(0, 1, 11)));
        // a floor expectation is never replaced by a trigger of the same second
        w.push(floor(5, FloorKind::Breach, 9));
        assert_eq!(w.exp.len(), 5);
    }

    #[test]
    fn misses_map_to_their_classes() {
        assert_eq!(class_of_miss(&ExpKind::Floor(FloorKind::TightStart, [0; 5])), FindingClass::MissedTightStart);
        assert_eq!(class_of_miss(&ExpKind::Floor(FloorKind::FloorRaised, [0; 5])), FindingClass::MissedFloorRaise);
        assert_eq!(class_of_miss(&ExpKind::Floor(FloorKind::Breach, [0; 5])), FindingClass::MissedBreach);
        assert_eq!(class_of_miss(&ExpKind::Floor(FloorKind::Fail(FailReason::StaticFloor), [0; 5])), FindingClass::MissedFail);
        assert_eq!(class_of_miss(&ExpKind::Trigger(0, ProtectiveKind::StopLoss, 1)), FindingClass::TriggerMissed);
        assert_eq!(class_of_miss(&ExpKind::Pending(0, 1, 1)), FindingClass::TriggerMissed);
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;
    use crate::replay::tests::{ctx, with_replay, T0};

    fn at(sec: i64) -> Ctx {
        Ctx { seq: Some(3), idx: Some(2), time: sec }
    }

    fn put(rp: &mut Replay, e: Expect) {
        let mut w = rp.walk.take().unwrap_or_else(|| Walker::new(vec![], vec![], vec![]));
        w.exp.push(e);
        rp.walk = Some(w);
    }

    fn left(rp: &Replay) -> usize {
        rp.walk.as_ref().map_or(0, |w| w.exp.len())
    }

    fn classes(rp: &Replay) -> Vec<FindingClass> {
        rp.f.list.iter().map(|f| f.class).collect()
    }

    fn pending(sec: i64, market: u16, nonce: u32) -> Expect {
        Expect { sec, kind: ExpKind::Pending(market, nonce, 5), market, sample: 0 }
    }

    fn trigger(sec: i64, market: u16, k: ProtectiveKind) -> Expect {
        Expect { sec, kind: ExpKind::Trigger(market, k, 5), market, sample: 0 }
    }

    fn floor(sec: i64, k: FloorKind, v: [i64; 5]) -> Expect {
        Expect { sec, kind: ExpKind::Floor(k, v), market: 0, sample: 0 }
    }

    #[test]
    fn a_pending_fill_consumes_exactly_its_own_expectation() {
        with_replay(&[], |rp| {
            put(rp, pending(10, 0, 1));
            // other nonce, other market, other second: none of them is the expectation, each is a false trigger
            rp.walk_fill(at(10), 0, OrderKind::Limit, 2);
            rp.walk_fill(at(10), 1, OrderKind::Limit, 1);
            rp.walk_fill(at(11), 0, OrderKind::Limit, 1);
            assert_eq!(classes(rp), vec![FindingClass::TriggerFalse; 3]);
            assert_eq!(left(rp), 1);
            // a market fill needs no expectation and touches nothing
            rp.walk_fill(at(10), 0, OrderKind::Market, 7);
            assert_eq!((rp.f.list.len(), left(rp)), (3, 1));
            // the right one is consumed silently
            rp.walk_fill(at(10), 0, OrderKind::Limit, 1);
            assert_eq!((rp.f.list.len(), left(rp)), (3, 0));
            // a stop fill is judged like a limit fill
            rp.walk_fill(at(10), 0, OrderKind::Stop, 9);
            assert_eq!(rp.f.list.len(), 4);
        });
    }

    #[test]
    fn a_protective_close_consumes_exactly_its_own_trigger() {
        with_replay(&[], |rp| {
            put(rp, trigger(10, 0, ProtectiveKind::StopLoss));
            rp.walk_close(at(10), 1, CloseReason::StopLoss, 0, 0); // other market
            rp.walk_close(at(10), 0, CloseReason::TakeProfit, 0, 0); // other kind
            rp.walk_close(at(11), 0, CloseReason::StopLoss, 0, 0); // other second
            assert_eq!(classes(rp), vec![FindingClass::TriggerFalse; 3]);
            assert_eq!(left(rp), 1);
            // trader closes without a pending order are not judged here (a Breach or Fail close is, see the tests below)
            for r in [CloseReason::Manual, CloseReason::Flip] {
                rp.walk_close(at(10), 0, r, 0, 0);
            }
            assert_eq!((rp.f.list.len(), left(rp)), (3, 1));
            rp.walk_close(at(10), 0, CloseReason::StopLoss, 0, 0);
            assert_eq!((rp.f.list.len(), left(rp)), (3, 0));
            put(rp, trigger(10, 0, ProtectiveKind::TakeProfit));
            rp.walk_close(at(10), 0, CloseReason::TakeProfit, 0, 0);
            assert_eq!((rp.f.list.len(), left(rp)), (3, 0));
        });
    }

    #[test]
    fn a_forced_close_needs_a_floor_decision_of_its_second_and_is_priced_with_the_decisions_marks() {
        with_replay(&[], |rp| {
            // no decision (a decision of another second exists): a Breach close is a finding at once, a Fail close waits for a quick-limit Fail
            put(rp, floor(99, FloorKind::Breach, [0; 5]));
            assert_eq!(rp.walk_close(at(10), 0, CloseReason::Breach, 0, 0), None);
            assert_eq!(classes(rp), vec![FindingClass::ForcedCloseUnjustified]);
            assert_eq!(rp.walk_close(at(10), 0, CloseReason::Fail, 0, 0), None);
            assert_eq!(classes(rp), vec![FindingClass::ForcedCloseUnjustified], "the Fail close is pending, not yet a finding");
        });
        with_replay(&[], |rp| {
            // a decision of second 10 with marks: market 0 -> record 7, market 1 -> record 9
            put(rp, floor(10, FloorKind::Breach, [0; 5]));
            if let Some(w) = rp.walk.as_mut() {
                w.floor_marks.insert(10, vec![(0, 7), (1, 9)]);
            }
            assert_eq!(rp.walk_close(at(10), 0, CloseReason::Breach, 0, 0), Some(7));
            assert_eq!(rp.walk_close(at(10), 1, CloseReason::Fail, 0, 0), Some(9));
            assert_eq!(rp.walk_close(at(10), 2, CloseReason::Breach, 0, 0), None, "a market with no mark in the decision");
            assert!(rp.f.list.is_empty());
            // another second has no decision
            assert_eq!(rp.walk_close(at(11), 0, CloseReason::Breach, 0, 0), None);
            assert_eq!(classes(rp), vec![FindingClass::ForcedCloseUnjustified]);
            // a floor decision of kind Fail supports the closes as well
            put(rp, floor(12, FloorKind::Fail(FailReason::StaticFloor), [0; 5]));
            assert_eq!(rp.walk_close(at(12), 0, CloseReason::Fail, 0, 0), None);
            assert_eq!(rp.f.list.len(), 1);
        });
    }

    #[test]
    fn a_floor_event_consumes_exactly_the_matching_decision_and_compares_its_numbers() {
        with_replay(&[], |rp| {
            let ts = [1, 2, 3, 4, 5];
            put(rp, floor(10, FloorKind::TightStart, ts));
            // wrong kind and wrong second are unsupported events
            assert_eq!(rp.walk_floor(at(10), FloorKind::FloorRaised, [0; 5], true), None);
            assert_eq!(rp.walk_floor(at(10), FloorKind::Breach, [0; 5], true), None);
            assert_eq!(rp.walk_floor(at(11), FloorKind::TightStart, ts, true), None);
            assert_eq!(classes(rp), vec![FindingClass::FalseFloorRaise, FindingClass::FalseBreach, FindingClass::FalseTightStart]);
            assert_eq!(left(rp), 1);
            // same numbers: consumed, silent, and the expected numbers come back
            assert_eq!(rp.walk_floor(at(10), FloorKind::TightStart, ts, true), Some(ts));
            assert_eq!((rp.f.list.len(), left(rp)), (3, 0));
        });
    }

    #[test]
    fn each_compared_number_of_a_tight_start_and_a_raise_matters_alone() {
        let base = [100, 7, 8, 200, 300];
        for (idx, differs) in [(0usize, true), (1, false), (2, false), (3, true), (4, true)] {
            with_replay(&[], |rp| {
                put(rp, floor(10, FloorKind::TightStart, base));
                let mut v = base;
                v[idx] += 1;
                rp.walk_floor(at(10), FloorKind::TightStart, v, true);
                assert_eq!(classes(rp), if differs { vec![FindingClass::WrongLLevel] } else { vec![] }, "tight start field {idx}");
            });
        }
        for (idx, differs) in [(0usize, false), (3, true), (4, true)] {
            with_replay(&[], |rp| {
                put(rp, floor(10, FloorKind::FloorRaised, base));
                let mut v = base;
                v[idx] += 1;
                rp.walk_floor(at(10), FloorKind::FloorRaised, v, true);
                assert_eq!(classes(rp), if differs { vec![FindingClass::WrongLLevel] } else { vec![] }, "raise field {idx}");
            });
        }
        // a claim the formulas already refused is not compared again
        with_replay(&[], |rp| {
            put(rp, floor(10, FloorKind::TightStart, base));
            rp.walk_floor(at(10), FloorKind::TightStart, [0; 5], false);
            assert!(rp.f.list.is_empty());
        });
        // a Breach carries no numbers of its own to compare
        with_replay(&[], |rp| {
            put(rp, floor(10, FloorKind::Breach, base));
            assert_eq!(rp.walk_floor(at(10), FloorKind::Breach, [9; 5], true), Some(base));
            assert!(rp.f.list.is_empty());
        });
    }

    #[test]
    fn a_fail_in_the_journal_clears_the_fail_expectation_of_its_second_only() {
        with_replay(&[], |rp| {
            put(rp, floor(10, FloorKind::Fail(FailReason::StaticFloor), [0; 5]));
            put(rp, floor(11, FloorKind::Fail(FailReason::StaticFloor), [0; 5]));
            put(rp, floor(10, FloorKind::Breach, [0; 5]));
            assert_eq!(rp.walk_floor(at(10), FloorKind::Fail(FailReason::StaticFloor), [0; 5], true), None);
            assert_eq!(left(rp), 2, "the Fail of second 11 and the Breach of second 10 stay");
        });
    }

    #[test]
    fn expectations_of_earlier_seconds_are_misses_and_later_ones_wait() {
        with_replay(&[], |rp| {
            put(rp, floor(10, FloorKind::Breach, [0; 5]));
            put(rp, floor(20, FloorKind::TightStart, [0; 5]));
            rp.walk_advance(10, 4);
            assert_eq!((rp.f.list.len(), left(rp)), (0, 2), "second 10 is still being judged");
            rp.walk_advance(15, 4);
            assert_eq!(classes(rp), vec![FindingClass::MissedBreach]);
            assert_eq!((rp.f.list[0].seq, left(rp)), (Some(4), 1));
            rp.max_time = 25;
            rp.walk_finish();
            assert_eq!(classes(rp), vec![FindingClass::MissedBreach, FindingClass::MissedTightStart], "the end of the log judges everything left");
            let _ = (ctx(), T0);
        });
    }

    #[test]
    fn a_miss_before_any_event_has_no_sequence() {
        with_replay(&[], |rp| {
            put(rp, pending(10, 0, 1));
            rp.walk_advance(20, 0);
            assert_eq!((rp.f.list.len(), rp.f.list[0].seq), (1, None));
        });
    }
}

/// M9b (D3): the margin check of triggered pending orders, the stop-loss and take-profit closes that come first in a second, and
/// the lookup of refused executions. Numbers: S = $100,000, class 0 leverage 50, half-spread 1 e6; 1 EUR is 1e6 qty units.
#[cfg(test)]
mod margin_tests {
    use super::*;
    use crate::archive::{Archive, ArchiveWriter, Sample};
    use crate::intents::{header_line, IntentFile};
    use crate::log::OpenedRec;
    use crate::replay::tests::T0;
    use crate::rules::Rules;
    use engine::Position;
    use journal_types::{genesis_hash, NativeSha256};

    const EUR: u64 = 1_000_000;
    const ONE: u64 = 1_000_000_000_000;
    /// The half-spread of both markets (decision 64: a market with 0 is untradable, so the fixtures cannot use 0).
    const TEST_HALF_SPREAD_E6: u32 = 1;

    fn rules_json(commission_e6: u32) -> String {
        let feed = crate::hexutil::encode(&[7u8; 32]);
        let feed1 = crate::hexutil::encode(&[8u8; 32]);
        format!(
            r#"{{"version":1,"ruleset_version":1,"fee_bps":40,"leverage_x":[50,20,20,20,10,5,5,2],"holding_fee_e6":[0,0,0,0,0,0,0,0],
"commission_e6":[{commission_e6},0,0,0,0,0,0,0],"tier_sizes":[100000000000],"spread_e6":{{"0":{TEST_HALF_SPREAD_E6},"1":{TEST_HALF_SPREAD_E6}}},
"markets":[{{"id":0,"class":0,"feed_id":"{feed}","max_age_secs":60,"status":1,"qty_step":1000,"qty_min":1000000,"max_notional_usd":1000000000000000,"sessions":[[0,10080]]}},
{{"id":1,"class":0,"feed_id":"{feed1}","max_age_secs":60,"status":1,"qty_step":1000,"qty_min":1000000,"max_notional_usd":1000000000000000,"sessions":[[0,10080]]}}]}}"#
        )
    }

    fn smp(market: u16, mid: u64) -> Sample {
        let ts_ns = u64::try_from(T0).unwrap() * 1_000_000_000;
        Sample { market, source: 2, recv_ns: ts_ns, ts_ns, value: i128::from(mid) * 1_000_000, digest: [0; 32] }
    }

    fn with(commission_e6: u32, samples: &[Sample], f: impl FnOnce(&mut Replay)) {
        let rules = Rules::parse(rules_json(commission_e6).as_bytes()).unwrap();
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
            phase: 2,
            ruleset_version: 1,
            genesis_hash: genesis_hash(&NativeSha256, &account),
            time: T0 - 3_600,
        };
        let mut rp = Replay::new(&rules, &arch, &intents, opened, "primary", Default::default(), Default::default()).unwrap();
        f(&mut rp);
    }

    fn long(qty_eur: u64, avg: u64, quick_for: i64) -> Pos {
        Pos { p: Position { qty: i64::try_from(qty_eur * EUR).unwrap(), avg_entry_e12: avg, timer_start: T0 - quick_for, opened_at: T0 - quick_for, fee_day: engine::day_index(T0) }, sl: 0, tp: 0 }
    }

    fn pending(nonce: u32, market: u16, side: u8, qty_eur: u64) -> (u32, OrderIntent) {
        (nonce, OrderIntent { kind: IntentKind::Limit, market, side, qty: qty_eur * EUR, price_e12: ONE, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: T0 - 10 })
    }

    /// Runs `resolve_second` for the candidates `(market, nonce, price, sample)` with the marks `(market, sample)` and the extra
    /// expectations; returns the walker.
    fn resolve(rp: &mut Replay, orders: Vec<(u32, OrderIntent)>, marks: &[(u16, usize)], cands: &[(u16, u32, u64, usize)], extra: Vec<Expect>) -> Walker {
        let mut w = Walker::new(orders, vec![], vec![]);
        for (m, si) in marks {
            w.marks.insert(*m, *si);
        }
        for (m, n, price, si) in cands {
            w.cand.insert((*m, *n), (*price, *si));
        }
        for e in extra {
            w.exp.push(e);
        }
        rp.resolve_second(&mut w, T0);
        w
    }

    fn expected(w: &Walker) -> Vec<u32> {
        let mut v: Vec<u32> = w.exp.iter().filter_map(|e| if let ExpKind::Pending(_, n, _) = e.kind { Some(n) } else { None }).collect();
        v.sort_unstable();
        v
    }

    fn refused(w: &Walker) -> Vec<u32> {
        let mut v: Vec<u32> = w.refused.iter().map(|r| r.2).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn an_order_that_uses_exactly_the_whole_margin_is_expected_and_one_step_more_is_not() {
        // 5 M EUR at 1.0 needs $100,000 of margin on $100,000: equity - used == 0 is enough. The long is marked at the bid,
        // 1 e6 under the mid: 5e12 units x 1e6 / 1e12 = $5 of open loss, which the balance covers (S + 5,000,000 micro-USD).
        with(0, &[smp(0, ONE)], |rp| {
            rp.st.b = 100_000_000_000 + 5_000_000;
            let w = resolve(rp, vec![pending(1, 0, 0, 5_000_000)], &[(0, 0)], &[(0, 1, ONE, 0)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![]));
        });
        with(0, &[smp(0, ONE)], |rp| {
            rp.st.b = 100_000_000_000 + 5_000_000;
            let mut o = pending(1, 0, 0, 5_000_000);
            o.1.qty += 1_000;
            let w = resolve(rp, vec![o], &[(0, 0)], &[(0, 1, ONE, 0)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![], vec![1]));
        });
    }

    #[test]
    fn both_commissions_are_charged_in_the_margin_check() {
        // 0.1%: equity = 100,000 - 2 x 0.001 N, margin N / 50; N may be at most 4,545,454 EUR
        for (n, ok) in [(4_545_000u64, true), (4_546_000, false)] {
            with(1_000, &[smp(0, ONE)], |rp| {
                let w = resolve(rp, vec![pending(1, 0, 0, n)], &[(0, 0)], &[(0, 1, ONE, 0)], vec![]);
                assert_eq!(expected(&w) == vec![1], ok, "{n} EUR");
                assert_eq!(refused(&w) == vec![1], !ok, "{n} EUR");
            });
        }
    }

    #[test]
    fn open_profit_counts_and_the_trigger_sample_is_the_mark_of_the_orders_market() {
        // A: long 1 M EUR at 1.0, now 1.01 (+$10,000). Market 1's trigger sample is 1.0000 (sample 1); a later sample of market 1
        // at 0.99 (sample 2) is its latest mark but must not be used for the new order.
        let samples = [smp(0, 1_010_000_000_000), smp(1, ONE), smp(1, 990_000_000_000)];
        // equity 110,000 - used 20,000 - N / 50: N up to 4.5 M EUR
        for (n, ok) in [(4_400_000u64, true), (4_600_000, false)] {
            with(0, &samples, |rp| {
                rp.positions.insert(0, long(1_000_000, ONE, 1_000));
                let w = resolve(rp, vec![pending(1, 1, 0, n)], &[(0, 0), (1, 2)], &[(1, 1, ONE, 1)], vec![]);
                assert_eq!(expected(&w) == vec![1], ok, "{n} EUR: expected {:?} refused {:?}", expected(&w), refused(&w));
            });
        }
    }

    #[test]
    fn open_profit_and_commissions_together() {
        // as above with 0.1%: closing commission of A = 1,010, then 2 x 0.001 N for the new position
        // 110,000 - 1,010 - 20,000 - 0.022 N >= 0: N up to 4,045,000 EUR
        let samples = [smp(0, 1_010_000_000_000), smp(1, ONE)];
        for (n, ok) in [(4_000_000u64, true), (4_100_000, false)] {
            with(1_000, &samples, |rp| {
                rp.positions.insert(0, long(1_000_000, ONE, 1_000));
                let w = resolve(rp, vec![pending(1, 1, 0, n)], &[(0, 0), (1, 1)], &[(1, 1, ONE, 1)], vec![]);
                assert_eq!(expected(&w) == vec![1], ok, "{n} EUR");
            });
        }
    }

    #[test]
    fn a_pure_reduce_needs_no_margin() {
        // A: long 4 M EUR with a $200,000 loss: the account is far below its margin, a sell of 1 M only reduces
        let samples = [smp(0, 950_000_000_000)];
        with(0, &samples, |rp| {
            rp.positions.insert(0, long(4_000_000, ONE, 1_000));
            let w = resolve(rp, vec![pending(1, 0, 1, 1_000_000)], &[(0, 0)], &[(0, 1, 950_000_000_000, 0)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![]));
        });
    }

    #[test]
    fn a_flip_credits_its_closing_slice_before_the_margin_check() {
        // A: long 4 M EUR at 1.0, now 1.02 (+$80,000, not quick); the balance is -$20,000. A sell of 5 M flips: the closing credit
        // makes the balance $60,000, the remaining short of 1 M needs $20,400
        let samples = [smp(0, 1_020_000_000_000)];
        with(0, &samples, |rp| {
            rp.st.b = -20_000_000_000;
            rp.positions.insert(0, long(4_000_000, ONE, 1_000));
            let w = resolve(rp, vec![pending(1, 0, 1, 5_000_000)], &[(0, 0)], &[(0, 1, 1_020_000_000_000, 0)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![]));
        });
    }

    #[test]
    fn the_orders_of_one_second_see_each_other() {
        let samples = [smp(0, ONE), smp(1, ONE)];
        // a pure partial reduce of A (4 M -> 3 M EUR) frees $20,000: the next order of 1.5 M EUR (margin $30,000 on a free
        // $40,000) fits only because the reduce is booked first (1.0 M would fit without it)
        with(0, &samples, |rp| {
            rp.positions.insert(0, long(4_000_000, ONE, 1_000));
            let w = resolve(rp, vec![pending(1, 0, 1, 1_000_000), pending(2, 1, 0, 1_500_000)], &[(0, 0), (1, 1)], &[(0, 1, ONE, 0), (1, 2, ONE, 1)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1, 2], vec![]));
        });
        // an opening order uses margin that the next one then does not have: 3 M EUR (margin $60,000) then 2.5 M ($50,000)
        with(0, &samples, |rp| {
            let w = resolve(rp, vec![pending(1, 0, 0, 3_000_000), pending(2, 1, 0, 2_500_000)], &[(0, 0), (1, 1)], &[(0, 1, ONE, 0), (1, 2, ONE, 1)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![2]));
        });
    }

    fn sl(sec: i64, market: u16, price: u64) -> Expect {
        Expect { sec, kind: ExpKind::Trigger(market, ProtectiveKind::StopLoss, price), market, sample: 0 }
    }

    #[test]
    fn the_stop_loss_closes_of_the_second_come_before_its_pending_orders() {
        // A: long 1 M EUR at 1.0, now 0.97 (-$30,000): equity 70,000 either way, but once the stop-loss has closed A the account
        // holds no margin for it: a buy of 3 M EUR (margin $60,000) fits only then
        let samples = [smp(0, 970_000_000_000), smp(1, ONE)];
        let run = |extra: Vec<Expect>| {
            let mut out = (vec![], vec![]);
            with(0, &samples, |rp| {
                rp.positions.insert(0, long(1_000_000, ONE, 1_000));
                let w = resolve(rp, vec![pending(1, 1, 0, 3_000_000)], &[(0, 0), (1, 1)], &[(1, 1, ONE, 1)], extra);
                out = (expected(&w), refused(&w));
            });
            out
        };
        assert_eq!(run(vec![sl(T0, 0, 970_000_000_000)]), (vec![1], vec![]));
        assert_eq!(run(vec![]), (vec![], vec![1]), "without the close the margin of A still counts");
        assert_eq!(run(vec![sl(T0 + 5, 0, 970_000_000_000)]), (vec![], vec![1]), "a trigger of another second is not part of this one");
        assert_eq!(run(vec![sl(T0, 1, 970_000_000_000)]), (vec![], vec![1]), "nor is one of a market without a position");
    }

    #[test]
    fn when_a_stop_loss_and_a_take_profit_of_one_market_both_trigger_the_stop_loss_closes_first() {
        // the evaluator executes the triggers of a second by (market, kind), stop-loss first; the take-profit then finds no position
        let samples = [smp(0, ONE), smp(1, ONE)];
        with(0, &samples, |rp| {
            rp.positions.insert(0, long(1_000_000, ONE, 1_000));
            let tp = Expect { sec: T0, kind: ExpKind::Trigger(0, ProtectiveKind::TakeProfit, 1_030_000_000_000), market: 0, sample: 0 };
            // closed at the stop (-$30,000): equity 70,000, an order with a margin of $100,000 is refused; closed at the take-profit
            // (+$30,000) it would be accepted
            let w = resolve(rp, vec![pending(1, 1, 0, 5_000_000)], &[(0, 0), (1, 1)], &[(1, 1, ONE, 1)], vec![tp, sl(T0, 0, 970_000_000_000)]);
            assert_eq!((expected(&w), refused(&w)), (vec![], vec![1]));
        });
    }

    #[test]
    fn a_stop_loss_that_pushes_the_quick_tally_over_its_limit_ends_the_phase_before_the_orders() {
        // A: long 1 M EUR opened 10 s ago at 1.0, now 1.06: the close credits $30,000 of a quick profit, over 0.028 S
        let samples = [smp(0, 1_060_000_000_000), smp(1, ONE)];
        with(0, &samples, |rp| {
            rp.positions.insert(0, long(1_000_000, ONE, 10));
            let w = resolve(rp, vec![pending(1, 1, 0, 1_000_000)], &[(0, 0), (1, 1)], &[(1, 1, ONE, 1)], vec![sl(T0, 0, 1_060_000_000_000)]);
            assert_eq!((expected(&w), refused(&w)), (vec![], vec![]), "the account failed: no order is expected, none is refused");
        });
    }

    #[test]
    fn an_order_nobody_can_price_is_expected_to_fill_as_before() {
        // no sample behind the candidate: the walk cannot judge, it keeps the pre-M9b expectation
        with(0, &[smp(0, ONE)], |rp| {
            let w = resolve(rp, vec![pending(1, 0, 0, 1_000_000)], &[(0, 0)], &[(0, 1, ONE, 9)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![]));
        });
    }

    #[test]
    fn a_refused_execution_is_matched_on_second_market_and_nonce() {
        with(0, &[smp(0, ONE)], |rp| {
            let mut w = Walker::new(vec![pending(1, 0, 0, 1)], vec![], vec![]);
            w.refused.push((T0, 0, 1, Refusal::Margin));
            rp.walk = Some(w);
            let at = |sec: i64| Ctx { seq: Some(3), idx: Some(2), time: sec };
            // other nonce, other market, other second: each is a TriggerFalse and the refusal stays
            rp.walk_fill(at(T0), 0, OrderKind::Limit, 2);
            rp.walk_fill(at(T0), 1, OrderKind::Limit, 1);
            rp.walk_fill(at(T0 + 1), 0, OrderKind::Limit, 1);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), vec![FindingClass::TriggerFalse; 3]);
            // the right one is the margin finding
            rp.walk_fill(at(T0), 0, OrderKind::Limit, 1);
            assert_eq!(rp.f.list.last().map(|f| f.class), Some(FindingClass::PendingFillWithoutMargin));
            assert_eq!(rp.f.list.len(), 4);
        });
    }
}

/// M9c (B1, B2, B3): the evaluator's own refusals, recomputed for the execution of MARKET orders and for triggered pending
/// orders. Numbers: S = $100,000, class 0 leverage 50, half-spread 1 e6, price 1.0 (1 EUR is 1e6 qty units, so a quantity in units is
/// also its notional in micro-USD). Every check has its boundary pair.
#[cfg(test)]
mod refusal_tests {
    use super::*;
    use crate::archive::{Archive, ArchiveWriter, Sample};
    use crate::intents::{header_line, IntentFile};
    use crate::log::OpenedRec;
    use crate::replay::tests::T0;
    use crate::rules::Rules;
    use engine::Position;
    use journal_types::{genesis_hash, NativeSha256};

    const EUR: u64 = 1_000_000;
    const ONE: u64 = 1_000_000_000_000;
    const S: i64 = 100_000_000_000;
    /// The half-spread of every market (decision 64: a market with 0 is untradable, so the fixtures cannot use 0).
    const TEST_HALF_SPREAD_E6: u32 = 1;

    /// Rules with `p.n` markets (ids 0 to n-1, class 0, half-spread `TEST_HALF_SPREAD_E6` except market 0's `p.spread`): `status` /
    /// `qty_step` / `qty_min` / `cap` / `sessions` of the first one are the parameters, the others are ordinary.
    fn rules_json(p: &P) -> String {
        let mut markets = Vec::new();
        for id in 0..p.n {
            let feed = crate::hexutil::encode(&[7 + id as u8; 32]);
            let (st, step, min, cp, ses) = if id == 0 { (p.status, p.qty_step, p.qty_min, p.cap, p.sessions.to_string()) } else { (1, 1_000, 1_000_000, 1_000_000_000_000_000, "[[0,10080]]".to_string()) };
            markets.push(format!(r#"{{"id":{id},"class":0,"feed_id":"{feed}","max_age_secs":60,"status":{st},"qty_step":{step},"qty_min":{min},"max_notional_usd":{cp},"sessions":{ses}}}"#));
        }
        let spreads: Vec<String> = (0..p.n).map(|id| format!(r#""{id}":{}"#, if id == 0 { p.spread } else { TEST_HALF_SPREAD_E6 })).collect();
        format!(
            r#"{{"version":1,"ruleset_version":1,"fee_bps":40,"leverage_x":[50,20,20,20,10,5,5,2],"holding_fee_e6":[0,0,0,0,0,0,0,0],
"commission_e6":[{}],"tier_sizes":[100000000000],"spread_e6":{{{}}},"markets":[{}]}}"#,
            format_args!("{},0,0,0,0,0,0,0", p.commission_e6),
            spreads.join(","),
            markets.join(",")
        )
    }

    fn smp(market: u16, mid: u64) -> Sample {
        let ts_ns = u64::try_from(T0).unwrap() * 1_000_000_000;
        Sample { market, source: 2, recv_ns: ts_ns, ts_ns, value: i128::from(mid) * 1_000_000, digest: [0; 32] }
    }

    #[derive(Clone, Copy)]
    struct P {
        commission_e6: u32,
        n: u16,
        status: u8,
        qty_step: u64,
        qty_min: u64,
        cap: u64,
        sessions: &'static str,
        /// The half-spread of market 0 in e6 (0 = the rule set does not price it, decision 64).
        spread: u32,
    }

    impl P {
        const NEW: P = P { commission_e6: 0, n: 2, status: 1, qty_step: 1_000, qty_min: 1_000_000, cap: 1_000_000_000_000_000, sessions: "[[0,10080]]", spread: TEST_HALF_SPREAD_E6 };
        fn new() -> P {
            P::NEW
        }
    }

    /// A replay over one sample per market (at price 1.0, second T0) with a walker that has walked them all.
    fn with(p: &P, f: impl FnOnce(&mut Replay)) {
        let rules = Rules::parse(rules_json(p).as_bytes()).unwrap();
        let mut aw = ArchiveWriter::new();
        for m in 0..p.n {
            aw.push(&smp(m, ONE));
        }
        let arch = Archive::parse(&aw.into_bytes()).unwrap();
        let intents = IntentFile::parse(header_line().as_bytes()).unwrap();
        let account = [1u8; 32];
        let opened = OpenedRec {
            account,
            trader: [2; 32],
            challenge_id: 0,
            size_s: 100_000_000_000,
            tier_index: 8,
            phase: 2,
            ruleset_version: 1,
            genesis_hash: genesis_hash(&NativeSha256, &account),
            time: T0 - 3_600,
        };
        let mut rp = Replay::new(&rules, &arch, &intents, opened, "primary", Default::default(), Default::default()).unwrap();
        let mut w = Walker::new(vec![], vec![], vec![]);
        for m in 0..p.n {
            w.marks.insert(m, usize::from(m));
            w.lc_at.push(T0);
        }
        rp.walk = Some(w);
        f(&mut rp);
    }

    fn ctx() -> Ctx {
        Ctx { seq: Some(5), idx: Some(1), time: T0 }
    }

    fn long(market_qty_eur: u64, avg: u64) -> Pos {
        Pos { p: Position { qty: i64::try_from(market_qty_eur * EUR).unwrap(), avg_entry_e12: avg, timer_start: T0 - 1_000, opened_at: T0 - 1_000, fee_day: engine::day_index(T0) }, sl: 0, tp: 0 }
    }

    /// Judges a market order of `qty` units on market 0 at price 1.0 with the proof sample 0; returns the classes it reports.
    fn judge(rp: &mut Replay, long: bool, qty: u64) -> Vec<FindingClass> {
        rp.last_proof_si = Some(0);
        rp.check_market_order(ctx(), 0, long, qty, ONE, T0);
        rp.f.list.iter().map(|f| f.class).collect()
    }

    #[test]
    fn the_whole_margin_is_enough_one_micro_usd_less_is_not() {
        // 5,000,000 EUR is $5M of notional: margin exactly $100,000 = S. The long is marked at the bid, 1 e6 under the mid (half-spread
        // 1 e6): 5e12 units x 1e6 / 1e12 = 5,000,000 micro-USD of open loss, so the balance S + LOSS is exactly enough.
        const LOSS: i64 = 5_000_000;
        with(&P::new(), |rp| {
            rp.st.b = S + LOSS;
            assert_eq!(judge(rp, true, 5_000_000 * EUR), vec![]);
        });
        // one micro-USD short: the closed balance is S + LOSS - 1
        with(&P::new(), |rp| {
            rp.st.b = S + LOSS - 1;
            assert_eq!(judge(rp, true, 5_000_000 * EUR), vec![FindingClass::FillWithoutMargin]);
        });
        // one step (1,000 units = 20 micro-USD of margin, and 1,000 x 1e6 / 1e12 = 0.001 micro-USD of loss, rounded up to 1) more
        // needs 21 micro-USD more balance
        with(&P::new(), |rp| {
            rp.st.b = S + LOSS + 21;
            assert_eq!(judge(rp, true, 5_000_000 * EUR + 1_000), vec![]);
        });
        with(&P::new(), |rp| {
            rp.st.b = S + LOSS + 20;
            assert_eq!(judge(rp, true, 5_000_000 * EUR + 1_000), vec![FindingClass::FillWithoutMargin]);
        });
        // the finding names the order
        with(&P::new(), |rp| {
            judge(rp, false, 5_001_000 * EUR);
            assert_eq!(rp.f.list.len(), 1);
            assert_eq!((rp.f.list[0].seq, rp.f.list[0].event_index), (Some(5), Some(1)));
        });
    }

    #[test]
    fn both_commissions_are_charged_for_a_market_order() {
        // 0.1%: equity = S - 2 x 0.001 N, margin N / 50: N at most 4,545,454.5 EUR; steps of 1,000 units
        for (n, ok) in [(4_545_000u64, true), (4_546_000, false)] {
            with(&P { commission_e6: 1_000, ..P::new() }, |rp| {
                let c = judge(rp, true, n * EUR);
                assert_eq!(c.is_empty(), ok, "{n} EUR: {c:?}");
            });
        }
    }

    #[test]
    fn open_profit_of_the_other_markets_counts_at_the_walks_marks() {
        // long 1 M EUR in market 0 at 1.0 now marked at the bid of 1.01, 1 e6 under it (+$10,000 - $1.01 = $9,998.99, margin
        // $20,000); a market order in market 1 (sample 1 at 1.0, bid 1 e6 under it: N micro-USD of open loss) of N EUR:
        // 109,998.99 - 20,000 - N / 50 - N e-6 >= 0, N at most 4,499,724 EUR, so 4,499,000 fits and 4,500,000 does not
        for (n, ok) in [(4_499_000u64, true), (4_500_000, false)] {
            let rules = Rules::parse(rules_json(&P::new()).as_bytes()).unwrap();
            let mut aw = ArchiveWriter::new();
            aw.push(&smp(0, 1_010_000_000_000));
            aw.push(&smp(1, ONE));
            let arch = Archive::parse(&aw.into_bytes()).unwrap();
            let intents = IntentFile::parse(header_line().as_bytes()).unwrap();
            let account = [1u8; 32];
            let opened = OpenedRec { account, trader: [2; 32], challenge_id: 0, size_s: 100_000_000_000, tier_index: 8, phase: 2, ruleset_version: 1, genesis_hash: genesis_hash(&NativeSha256, &account), time: T0 - 3_600 };
            let mut rp = Replay::new(&rules, &arch, &intents, opened, "primary", Default::default(), Default::default()).unwrap();
            let mut w = Walker::new(vec![], vec![], vec![]);
            w.marks.insert(0, 0);
            w.marks.insert(1, 1);
            w.lc_at = vec![T0, T0];
            rp.walk = Some(w);
            rp.positions.insert(0, long(1_000_000, ONE));
            rp.last_proof_si = Some(1);
            rp.check_market_order(ctx(), 1, true, n * EUR, ONE, T0);
            assert_eq!(rp.f.list.is_empty(), ok, "{n} EUR");
        }
    }

    #[test]
    fn a_flip_is_judged_as_a_whole_before_its_closing_slice() {
        // long 4 M EUR at 1.0, mark 1.0: a market sell of 9 M EUR closes the 4 M and opens a short of 5 M ($100,000 of margin),
        // marked at the ask 1 e6 over the mid ($5 of open loss, so the balance S + 5,000,000 micro-USD): exactly enough. One step
        // more is short.
        with(&P::new(), |rp| {
            rp.st.b = S + 5_000_000;
            rp.positions.insert(0, long(4_000_000, ONE));
            assert_eq!(judge(rp, false, 9_000_000 * EUR), vec![]);
        });
        with(&P::new(), |rp| {
            rp.st.b = S + 5_000_000;
            rp.positions.insert(0, long(4_000_000, ONE));
            assert_eq!(judge(rp, false, 9_000_000 * EUR + 1_000), vec![FindingClass::FillWithoutMargin]);
        });
    }

    #[test]
    fn an_order_that_only_reduces_needs_nothing() {
        // the account is far under water and the market is reduce-only: a sell of part of the long position is fine
        with(&P { status: 2, ..P::new() }, |rp| {
            rp.positions.insert(0, long(4_000_000, ONE));
            rp.st.b = 1_000_000;
            assert_eq!(judge(rp, false, 1_000_000 * EUR), vec![]);
        });
    }

    #[test]
    fn the_lock_blocks_an_exposure_increase_exactly_until_lock_until() {
        with(&P::new(), |rp| {
            rp.st.lock_until = T0;
            assert_eq!(judge(rp, true, 10 * EUR), vec![], "the lock ends at lock_until");
        });
        with(&P::new(), |rp| {
            rp.st.lock_until = T0 + 1;
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("breach lock"), "{:?}", rp.f.list[0]);
        });
    }

    #[test]
    fn a_disabled_or_reduce_only_market_takes_no_new_exposure() {
        for (status, ok) in [(1u8, true), (2, false), (0, false)] {
            with(&P { status, ..P::new() }, |rp| {
                let c = judge(rp, true, 10 * EUR);
                assert_eq!(c.is_empty(), ok, "status {status}: {c:?}");
                if !ok {
                    assert!(rp.f.list[0].expected.contains("disabled or reduce-only"));
                }
            });
        }
    }

    #[test]
    fn the_quantity_must_be_on_the_step_and_at_least_the_minimum() {
        // step 1,000 units, minimum 1,000,000 units
        for (qty, ok) in [(1_000_000u64, true), (1_001_000, true), (999_000, false), (1_000_500, false), (1_000_001, false)] {
            with(&P::new(), |rp| {
                let c = judge(rp, true, qty);
                assert_eq!(c.is_empty(), ok, "{qty} units: {c:?}");
            });
        }
    }

    #[test]
    fn the_remainder_of_a_flip_is_judged_on_step_and_minimum() {
        // long 1 M EUR; a market sell of 1,000,000 + r units flips with a remainder of r units
        for (r, ok) in [(1_000_000u64, true), (999_000, false), (1_000, false), (1_000_500, false)] {
            with(&P::new(), |rp| {
                rp.positions.insert(0, long(1_000_000, ONE));
                let c = judge(rp, false, 1_000_000 * EUR + r);
                assert_eq!(c.is_empty(), ok, "remainder {r}: {c:?}");
            });
        }
    }

    #[test]
    fn a_position_may_not_exceed_the_markets_notional_cap() {
        // cap $5,000 = 5,000,000,000 micro-USD: a notional of exactly the cap is fine
        let p = P { cap: 5_000_000_000, ..P::new() };
        with(&p, |rp| assert_eq!(judge(rp, true, 5_000_000_000), vec![]));
        with(&p, |rp| assert_eq!(judge(rp, true, 5_000_001_000), vec![FindingClass::FillNotAllowed]));
        // the position left behind counts, not the order: 4,000 EUR held + 1,001 EUR bought is 5,001 EUR
        with(&p, |rp| {
            rp.positions.insert(0, long(4_000, ONE));
            assert_eq!(judge(rp, true, 1_000 * EUR), vec![]);
        });
        with(&p, |rp| {
            rp.positions.insert(0, long(4_000, ONE));
            assert_eq!(judge(rp, true, 1_001 * EUR), vec![FindingClass::FillNotAllowed]);
        });
    }

    #[test]
    fn the_ninth_position_is_refused_and_an_add_to_an_open_market_is_not() {
        // markets 0 to 9; positions of 1 EUR in markets 1 to 8 (eight), a buy in market 0 would be the ninth
        let p = P { n: 10, ..P::new() };
        with(&p, |rp| {
            for m in 1..=8u16 {
                rp.positions.insert(m, long(1, ONE));
            }
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
        });
        // seven positions: the eighth is fine
        with(&p, |rp| {
            for m in 1..=7u16 {
                rp.positions.insert(m, long(1, ONE));
            }
            assert_eq!(judge(rp, true, 10 * EUR), vec![]);
        });
        // eight positions and an add to one of them
        with(&p, |rp| {
            for m in 0..=7u16 {
                rp.positions.insert(m, long(1, ONE));
            }
            assert_eq!(judge(rp, true, 10 * EUR), vec![]);
        });
    }

    #[test]
    fn a_market_order_in_a_closed_market_is_refused_for_both_reasons_a_market_is_closed() {
        // outside the session windows (Monday 00:00-00:10 only; T0 is later in the week)
        with(&P { sessions: "[[0,10]]", ..P::new() }, |rp| {
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("closed"), "{:?}", rp.f.list[0]);
        });
        // no price change within max_age_secs (60): the price last changed at T0 - 61 / T0 - 60 as of the order's sample
        for (last, ok) in [(T0 - 60, true), (T0 - 61, false)] {
            with(&P::new(), |rp| {
                rp.walk.as_mut().unwrap().lc_at[0] = last;
                let c = judge(rp, true, 10 * EUR);
                assert_eq!(c.is_empty(), ok, "last change {last}: {c:?}");
            });
        }
        // a close of a position in a closed market is refused too, an open one is fine
        with(&P { sessions: "[[0,10]]", ..P::new() }, |rp| {
            rp.last_proof_si = Some(0);
            rp.check_market_close(ctx(), 0, T0, 0);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), vec![FindingClass::FillNotAllowed]);
        });
        with(&P::new(), |rp| {
            rp.last_proof_si = Some(0);
            rp.check_market_close(ctx(), 0, T0, 0);
            assert!(rp.f.list.is_empty());
        });
        // the quantity the trader named: on the step and at least the minimum (0 = all)
        for (qty, ok) in [(0u64, true), (1_000_000, true), (1_001_000, true), (999_000, false), (1_000_500, false)] {
            with(&P::new(), |rp| {
                rp.last_proof_si = Some(0);
                rp.check_market_close(ctx(), 0, T0, qty);
                assert_eq!(rp.f.list.is_empty(), ok, "close of {qty} units");
            });
        }
        with(&P::new(), |rp| {
            // a flip's total quantity is validated as well as its remainder: 1.5 M EUR + 500 units against a 1 M EUR long
            rp.positions.insert(0, long(1_000_000, ONE));
            assert_eq!(judge(rp, false, 1_000_000 * EUR + 1_500_500), vec![FindingClass::FillNotAllowed]);
        });
    }

    #[test]
    fn nothing_is_judged_without_a_walker_a_proof_sample_or_a_mark() {
        // no walker (the archive walk did not run): silent
        with(&P::new(), |rp| {
            rp.walk = None;
            assert_eq!(judge(rp, true, 5_001_000 * EUR), vec![]);
        });
        // the proof did not resolve to a sample
        with(&P::new(), |rp| {
            rp.last_proof_si = None;
            rp.check_market_order(ctx(), 0, true, 5_001_000 * EUR, ONE, T0);
            assert!(rp.f.list.is_empty());
        });
        // a position in a market nobody can price: the check cannot compute and says nothing
        with(&P::new(), |rp| {
            rp.positions.insert(1, long(1, ONE));
            rp.walk.as_mut().unwrap().marks.remove(&1);
            assert_eq!(judge(rp, true, 5_001_000 * EUR), vec![]);
        });
    }

    // ---- the same reasons for a triggered pending order: it stays pending and no event is expected

    fn pending(nonce: u32, qty_units: u64, side: u8) -> (u32, OrderIntent) {
        (nonce, OrderIntent { kind: IntentKind::Limit, market: 0, side, qty: qty_units, price_e12: ONE, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: T0 - 10 })
    }

    fn resolve(rp: &mut Replay, order: (u32, OrderIntent)) -> Walker {
        let mut w = Walker::new(vec![order], vec![], vec![]);
        for m in 0..2u16 {
            w.marks.insert(m, usize::from(m));
            w.lc_at.push(T0);
        }
        w.cand.insert((0, order.0), (ONE, 0));
        rp.resolve_second(&mut w, T0);
        w
    }

    fn outcome(w: &Walker) -> (Vec<u32>, Vec<(u32, Refusal)>) {
        let exp = w.exp.iter().filter_map(|e| if let ExpKind::Pending(_, n, _) = e.kind { Some(n) } else { None }).collect();
        (exp, w.refused.iter().map(|r| (r.2, r.3)).collect())
    }

    #[test]
    fn a_triggered_pending_order_the_evaluator_refuses_for_a_reproducible_reason_is_expected_not_to_fill() {
        // notional cap: the position left behind
        let capped = P { cap: 5_000_000_000, ..P::new() };
        with(&capped, |rp| {
            rp.positions.insert(0, long(4_000, ONE));
            let w = resolve(rp, pending(1, 1_000 * EUR, 0));
            assert_eq!(outcome(&w), (vec![1], vec![]), "5,000 EUR is the cap exactly");
        });
        with(&capped, |rp| {
            rp.positions.insert(0, long(4_000, ONE));
            let w = resolve(rp, pending(1, 1_001 * EUR, 0));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::NotionalCap)]));
        });
        // the quantity left over by a flip: minimum 1,000,000 units
        with(&P::new(), |rp| {
            rp.positions.insert(0, long(1_000_000, ONE));
            let w = resolve(rp, pending(1, 1_000_000 * EUR + 1_000_000, 1));
            assert_eq!(outcome(&w), (vec![1], vec![]), "a remainder of exactly the minimum fills");
        });
        with(&P::new(), |rp| {
            rp.positions.insert(0, long(1_000_000, ONE));
            let w = resolve(rp, pending(1, 1_000_000 * EUR + 999_000, 1));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::Quantity)]));
        });
        // a disabled or reduce-only market
        with(&P { status: 2, ..P::new() }, |rp| {
            let w = resolve(rp, pending(1, 10 * EUR, 0));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::NotTradable)]));
        });
        // ... but a reduce in a reduce-only market is fine
        with(&P { status: 2, ..P::new() }, |rp| {
            rp.positions.insert(0, long(1_000_000, ONE));
            let w = resolve(rp, pending(1, 500_000 * EUR, 1));
            assert_eq!(outcome(&w), (vec![1], vec![]));
        });
        // the ninth position
        let nine = P { n: 10, ..P::new() };
        with(&nine, |rp| {
            for m in 1..=8u16 {
                rp.positions.insert(m, long(1, ONE));
            }
            let mut w = Walker::new(vec![pending(1, 10 * EUR, 0)], vec![], vec![]);
            for m in 0..10u16 {
                w.marks.insert(m, usize::from(m));
                w.lc_at.push(T0);
            }
            w.cand.insert((0, 1), (ONE, 0));
            rp.resolve_second(&mut w, T0);
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::TooManyPositions)]));
        });
    }

    #[test]
    fn an_exposure_increase_of_an_invalid_quantity_is_refused_for_a_pending_order_too_but_a_pure_reduce_is_not_judged() {
        // a pending order the evaluator would never have accepted (1,000,500 units on a step of 1,000): not expected to fill
        with(&P::new(), |rp| {
            let w = resolve(rp, pending(1, 1_000_000 + 500, 0));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::Quantity)]));
        });
        // ... while a pending order that only reduces is not a quantity check (its quantity is not an exposure increase)
        with(&P::new(), |rp| {
            rp.positions.insert(0, long(1_000_000, ONE));
            let w = resolve(rp, pending(1, 500_500_000, 1));
            assert_eq!(outcome(&w), (vec![1], vec![]));
        });
    }

    #[test]
    fn a_flip_the_first_order_of_a_second_makes_changes_the_account_the_second_order_sees() {
        // long 4 M EUR at 1.0, market 0 marked at 1.01 (+$40,000 open profit). Order 1 sells 5 M EUR at 1.01: it closes the long
        // (+$40,000 realized, B = 140,000) and opens a short of 1 M EUR (margin 20,200). Order 2 buys N EUR in market 1 at 1.0:
        // 140,000 - 20,200 - N / 50 >= 0 means N up to 5,990,000; without the credit it would be 4,000,000 at most.
        let sample = |m: u16, mid: u64| smp(m, mid);
        for (n, ok) in [(5_900_000u64, true), (6_100_000, false)] {
            let rules = Rules::parse(rules_json(&P::new()).as_bytes()).unwrap();
            let mut aw = ArchiveWriter::new();
            aw.push(&sample(0, 1_010_000_000_000));
            aw.push(&sample(1, ONE));
            let arch = Archive::parse(&aw.into_bytes()).unwrap();
            let intents = IntentFile::parse(header_line().as_bytes()).unwrap();
            let account = [1u8; 32];
            let opened = OpenedRec { account, trader: [2; 32], challenge_id: 0, size_s: 100_000_000_000, tier_index: 8, phase: 2, ruleset_version: 1, genesis_hash: genesis_hash(&NativeSha256, &account), time: T0 - 3_600 };
            let mut rp = Replay::new(&rules, &arch, &intents, opened, "primary", Default::default(), Default::default()).unwrap();
            rp.positions.insert(0, long(4_000_000, ONE));
            let o1 = (1u32, OrderIntent { kind: IntentKind::Limit, market: 0, side: 1, qty: 5_000_000 * EUR, price_e12: 1_010_000_000_000, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: T0 - 10 });
            let o2 = (2u32, OrderIntent { kind: IntentKind::Limit, market: 1, side: 0, qty: n * EUR, price_e12: ONE, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: T0 - 10 });
            let mut w = Walker::new(vec![o1, o2], vec![], vec![]);
            w.marks.insert(0, 0);
            w.marks.insert(1, 1);
            w.lc_at = vec![T0, T0];
            w.cand.insert((0, 1), (1_010_000_000_000, 0));
            w.cand.insert((1, 2), (ONE, 1));
            rp.resolve_second(&mut w, T0);
            let (exp, refused) = outcome(&w);
            assert!(exp.contains(&1), "the flip itself fits");
            assert_eq!(exp.contains(&2), ok, "{n} EUR: expected {exp:?} refused {refused:?}");
        }
    }

    #[test]
    fn the_fill_of_a_refused_pending_order_is_a_fill_not_allowed_and_a_margin_one_stays_pending_without_margin() {
        let capped = P { cap: 5_000_000_000, ..P::new() };
        with(&capped, |rp| {
            rp.positions.insert(0, long(4_000, ONE));
            let w = resolve(rp, pending(1, 1_001 * EUR, 0));
            rp.walk = Some(w);
            rp.walk_fill(Ctx { seq: Some(3), idx: Some(2), time: T0 }, 0, OrderKind::Limit, 1);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), vec![FindingClass::FillNotAllowed]);
        });
        with(&P::new(), |rp| {
            let w = resolve(rp, pending(1, 5_001_000 * EUR, 0));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::Margin)]));
            rp.walk = Some(w);
            rp.walk_fill(Ctx { seq: Some(3), idx: Some(2), time: T0 }, 0, OrderKind::Limit, 1);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), vec![FindingClass::PendingFillWithoutMargin]);
        });
    }

    // ---- decision 64: a market the account's rule set does not price (half-spread 0) takes no new exposure

    const UNPRICED: P = P { spread: 0, ..P::NEW };

    #[test]
    fn a_market_order_in_an_unpriced_market_is_a_fill_not_allowed_and_a_half_spread_of_one_is_not() {
        // the boundary pair: 1 e6 is priced, 0 is not (the first fill of the history is refused, the reason names the market)
        with(&P { spread: 1, ..P::new() }, |rp| assert_eq!(judge(rp, true, 10 * EUR), vec![]));
        with(&UNPRICED, |rp| {
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("unpriced market"), "{:?}", rp.f.list[0]);
            assert_eq!((rp.f.list[0].seq, rp.f.list[0].event_index), (Some(5), Some(1)));
        });
        // a short is refused the same way, and the other market (priced) is not affected
        with(&UNPRICED, |rp| assert_eq!(judge(rp, false, 10 * EUR), vec![FindingClass::FillNotAllowed]));
        with(&UNPRICED, |rp| {
            rp.last_proof_si = Some(1);
            rp.check_market_order(ctx(), 1, true, 10 * EUR, ONE, T0);
            assert!(rp.f.list.is_empty(), "market 1 is priced: {:?}", rp.f.list);
        });
    }

    #[test]
    fn a_close_in_an_unpriced_market_is_never_refused_but_the_opening_half_of_a_flip_is() {
        // a position that cannot exist for an honest account (it could not be opened) is still closed without a finding
        with(&UNPRICED, |rp| {
            rp.positions.insert(0, long(1_000, ONE));
            assert_eq!(judge(rp, false, 1_000 * EUR), vec![], "a sell of exactly the long");
            rp.check_market_close(ctx(), 0, T0, 400 * EUR);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), Vec::<FindingClass>::new(), "a named quantity of a close");
        });
        // a sell of one step more than the long opens a short: refused
        with(&UNPRICED, |rp| {
            rp.positions.insert(0, long(1_000, ONE));
            assert_eq!(judge(rp, false, 1_000 * EUR + 1_000_000), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("unpriced market"), "{:?}", rp.f.list[0]);
        });
        // an add to a long is an exposure increase
        with(&UNPRICED, |rp| {
            rp.positions.insert(0, long(1_000, ONE));
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
        });
    }

    #[test]
    fn the_unpriced_check_sits_after_the_status_and_before_the_quantity() {
        // held, locked and not-tradable answer first; the quantity (off the step) and everything after answer second
        with(&UNPRICED, |rp| {
            rp.st.lock_until = T0 + 1;
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("breach lock"), "{:?}", rp.f.list[0]);
        });
        with(&P { spread: 0, status: 2, ..P::new() }, |rp| {
            assert_eq!(judge(rp, true, 10 * EUR), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("disabled or reduce-only"), "{:?}", rp.f.list[0]);
        });
        with(&UNPRICED, |rp| {
            // 1,000,500 units: off the 1,000-unit step, and unpriced: the unpriced reason is the one reported
            assert_eq!(judge(rp, true, 1_000_000 + 500), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("unpriced market"), "{:?}", rp.f.list[0]);
        });
        // a priced market with the same off-step quantity: the quantity is the reason
        with(&P::new(), |rp| {
            assert_eq!(judge(rp, true, 1_000_000 + 500), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("step"), "{:?}", rp.f.list[0]);
        });
    }

    #[test]
    fn a_triggered_pending_order_in_an_unpriced_market_is_expected_not_to_fill_and_a_fill_is_not_allowed() {
        with(&UNPRICED, |rp| {
            let w = resolve(rp, pending(1, 10 * EUR, 0));
            assert_eq!(outcome(&w), (vec![], vec![(1, Refusal::Unpriced)]), "no fill is expected, so no TriggerMissed either");
            // the dishonest history executes it: FillNotAllowed (not a TriggerFalse, not a margin finding)
            rp.walk = Some(w);
            rp.walk_fill(Ctx { seq: Some(3), idx: Some(2), time: T0 }, 0, OrderKind::Limit, 1);
            assert_eq!(rp.f.list.iter().map(|f| f.class).collect::<Vec<_>>(), vec![FindingClass::FillNotAllowed]);
            assert!(rp.f.list[0].expected.contains("unpriced market"), "{:?}", rp.f.list[0]);
        });
        // the same order in a PRICED market with every other condition equal is expected to fill (so its absence is a TriggerMissed)
        with(&P::new(), |rp| {
            let w = resolve(rp, pending(1, 10 * EUR, 0));
            assert_eq!(outcome(&w), (vec![1], vec![]));
        });
        // a pending reduce in an unpriced market is not judged (the position cannot be opened, but a reduce is never refused)
        with(&UNPRICED, |rp| {
            rp.positions.insert(0, long(1_000_000, ONE));
            let w = resolve(rp, pending(1, 500_000 * EUR, 1));
            assert_eq!(outcome(&w), (vec![1], vec![]));
        });
    }
}
