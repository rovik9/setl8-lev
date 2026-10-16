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
use std::collections::{BTreeMap, HashSet};

use engine::{AccountState, ProtectiveKind, SampleAction};
use journal_types::{market_open, CloseReason, FailReason, OrderKind};

use crate::finding::FindingClass;
use crate::intents::{IntentKind, OrderIntent};
use crate::replay::{Ctx, FloorKind, Pos, Replay};

/// The margin verdict for a triggered pending order.
enum Sim {
    /// The account cannot margin it: it stays pending and no event is expected.
    Refused,
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
    /// (second, market, nonce) of triggered pending orders refused for margin: the order stays pending, no event expected.
    refused: Vec<(i64, u16, u32)>,
    /// (second, nonce) of a pending flip whose `Close` was seen: its opening `Fill` is the other half, not a second execution.
    flip_open: HashSet<(i64, u32)>,
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
        Walker { cursor: 0, marks: BTreeMap::new(), last_change: BTreeMap::new(), last_value: BTreeMap::new(), exp: vec![], filled: HashSet::new(), pending, cancels, sltp, sltp_cursor: 0, cand: BTreeMap::new(), refused: vec![], flip_open: HashSet::new() }
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
        w.refused.retain(|(s, _, _)| *s >= upto);
        w.flip_open.retain(|(s, _)| *s >= upto);
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
                    w.push(Expect { sec, kind: ExpKind::Floor(k, vals), market: 0, sample: si });
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
                Sim::Refused => w.refused.push((sec, m, nonce)),
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
            let mut b_after = st.b;
            let mut base = existing.map(|p| p.p);
            if close_qty > 0 {
                let Ok(o) = engine::apply_fill(base, side, close_qty, price, sec, day) else { return Sim::Unknown };
                let Some(b) = b_after.checked_add(o.balance_delta) else { return Sim::Unknown };
                b_after = b;
                base = o.position;
            }
            let Ok(o) = engine::apply_fill(base, side, open_qty, price, sec, day) else { return Sim::Unknown };
            let Some(np) = o.position else { return Sim::Unknown };
            let mut after = pos.clone();
            after.insert(market, Pos { p: np, sl: 0, tp: 0 });
            let Some((p_open, comm)) = self.profit_for(w, &after, Some((market, bid, ask))) else { return Sim::Unknown };
            let Some(fill_comm) = self.commission_of(market, open_qty, price) else { return Sim::Unknown };
            let equity = i128::from(b_after) + i128::from(p_open) - i128::from(comm) - i128::from(fill_comm);
            let mut used: i128 = 0;
            for (m, p) in &after {
                let Some(lev) = self.rules.class_of(*m).and_then(|c| self.rules.leverage_x.get(c).copied()) else { return Sim::Unknown };
                let Ok(mg) = engine::margin_micro(p.p.qty.unsigned_abs(), p.p.avg_entry_e12, lev) else { return Sim::Unknown };
                used += i128::from(mg);
            }
            if equity - used < 0 {
                return Sim::Refused;
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

    // ------------------------------------------------------------------ consumption at journal events

    /// A `Fill` of a limit or stop order must be the fill of a pending order the samples trigger.
    pub fn walk_fill(&mut self, ctx: Ctx, market: u16, kind: OrderKind, intent: u32) {
        let Some(mut w) = self.walk.take() else { return };
        if matches!(kind, OrderKind::Limit | OrderKind::Stop) {
            // the opening half of a pending flip whose `Close` was already taken as the execution
            if !w.flip_open.remove(&(ctx.time, intent)) {
                self.consume_pending(&mut w, ctx, market, intent, &format!("{kind:?} fill of intent {intent}"));
            }
        }
        self.walk = Some(w);
    }

    /// The execution of a pending order at this second: its `Pending` expectation is consumed; an execution the margin check
    /// refused is a `PendingFillWithoutMargin`; one nothing triggers is a `TriggerFalse`.
    fn consume_pending(&mut self, w: &mut Walker, ctx: Ctx, market: u16, intent: u32, what: &str) {
        let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Pending(m, n, _) if *m == market && *n == intent));
        match i {
            Some(i) => {
                w.exp.remove(i);
            }
            None => {
                if let Some(r) = w.refused.iter().position(|(s, m, n)| *s == ctx.time && *m == market && *n == intent) {
                    w.refused.remove(r);
                    self.f.add(
                        FindingClass::PendingFillWithoutMargin,
                        ctx,
                        "no execution: the account cannot margin this pending order, it stays pending",
                        what.to_string(),
                        "a pending order was executed although `b + P - commissions` does not cover the margin of the position it leaves",
                    );
                } else {
                    self.f.add(FindingClass::TriggerFalse, ctx, "a pending order the samples trigger at this second", what.to_string(), "the execution of a pending order that no sample of this second triggers");
                }
            }
        }
        w.filled.insert(intent);
    }

    /// A stop-loss or take-profit `Close` must be a trigger the samples produce; a `Manual` (or `Flip`) `Close` under the nonce
    /// of a pending order of that market is the execution of that order (decision D1: a pending order that only reduces a
    /// position has no `Fill`).
    pub fn walk_close(&mut self, ctx: Ctx, market: u16, reason: CloseReason, intent: u32, _qty: u64) {
        let want = match reason {
            CloseReason::StopLoss => ProtectiveKind::StopLoss,
            CloseReason::TakeProfit => ProtectiveKind::TakeProfit,
            CloseReason::Manual | CloseReason::Flip => {
                if intent == 0 {
                    return;
                }
                let Some(mut w) = self.walk.take() else { return };
                if w.pending.iter().any(|(n, it)| *n == intent && it.market == market) {
                    self.consume_pending(&mut w, ctx, market, intent, &format!("{reason:?} close under pending order {intent}"));
                    if reason == CloseReason::Flip {
                        w.flip_open.insert((ctx.time, intent));
                    }
                }
                self.walk = Some(w);
                return;
            }
            _ => return,
        };
        let Some(mut w) = self.walk.take() else { return };
        let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Trigger(m, k, _) if *m == market && *k == want));
        match i {
            Some(i) => {
                w.exp.remove(i);
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
            // closes that are not protective are not judged here
            for r in [CloseReason::Manual, CloseReason::Flip, CloseReason::Breach, CloseReason::Fail] {
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
/// the lookup of refused executions. Numbers: S = $100,000, class 0 leverage 50, spread 0; 1 EUR is 1e6 qty units.
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

    fn rules_json(commission_e6: u32) -> String {
        let feed = crate::hexutil::encode(&[7u8; 32]);
        let feed1 = crate::hexutil::encode(&[8u8; 32]);
        format!(
            r#"{{"version":1,"ruleset_version":1,"fee_bps":40,"leverage_x":[50,20,20,20,10,5,5,2],"holding_fee_e6":[0,0,0,0,0,0,0,0],
"commission_e6":[{commission_e6},0,0,0,0,0,0,0],"tier_sizes":[100000000000],"spread_e6":{{"0":0,"1":0}},
"markets":[{{"id":0,"class":0,"feed_id":"{feed}","max_age_secs":60,"status":1,"qty_step":1000,"qty_min":1000000,"max_notional_usd":5,"sessions":[[0,10080]]}},
{{"id":1,"class":0,"feed_id":"{feed1}","max_age_secs":60,"status":1,"qty_step":1000,"qty_min":1000000,"max_notional_usd":5,"sessions":[[0,10080]]}}]}}"#
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
        // 5 M EUR at 1.0 needs $100,000 of margin on $100,000: equity - used == 0 is enough
        with(0, &[smp(0, ONE)], |rp| {
            let w = resolve(rp, vec![pending(1, 0, 0, 5_000_000)], &[(0, 0)], &[(0, 1, ONE, 0)], vec![]);
            assert_eq!((expected(&w), refused(&w)), (vec![1], vec![]));
        });
        with(0, &[smp(0, ONE)], |rp| {
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
            w.refused.push((T0, 0, 1));
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
