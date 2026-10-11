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
//! The walk runs up to the largest event time or `Meta.time` seen ("Meta.time = the time of the last sample the evaluator
//! processed for the batch"); an expectation of an earlier second that no event satisfied is a miss.
use std::collections::{BTreeMap, HashSet};

use engine::{ProtectiveKind, SampleAction};
use journal_types::{market_open, CloseReason, FailReason, OrderKind};

use crate::finding::FindingClass;
use crate::intents::{IntentKind, OrderIntent};
use crate::replay::{Ctx, FloorKind, Replay};

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
        Walker { cursor: 0, marks: BTreeMap::new(), last_change: BTreeMap::new(), last_value: BTreeMap::new(), exp: vec![], filled: HashSet::new(), pending, cancels, sltp, sltp_cursor: 0 }
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
        while let Some(s) = arch.samples.get(w.cursor) {
            if s.ts_sec() > upto {
                break;
            }
            let si = w.cursor;
            w.cursor += 1;
            self.walk_step(&mut w, si);
        }
        self.expire(&mut w, upto, prev_seq);
        self.walk = Some(w);
    }

    fn expire(&mut self, w: &mut Walker, upto: i64, prev_seq: u64) {
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
                    SampleAction::Breach { .. } => (Some(FloorKind::Breach), [0, 0, 0, 0, l_before]),
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
                w.push(Expect { sec, kind: ExpKind::Pending(m, nonce, price), market: m, sample: si });
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
        let mut list = Vec::new();
        let mut comm = 0i64;
        for (m, p) in &self.positions {
            let (bid, ask) = self.bid_ask_of(w, *m)?;
            list.push((p.p, bid, ask));
            let class = self.rules.class_of(*m)?;
            let rate = self.rules.commission_e6.get(class).copied().unwrap_or(0);
            let mark = if p.p.qty.is_positive() { bid } else { ask };
            let notional = engine::margin_micro(p.p.qty.unsigned_abs(), mark, 1).ok()?;
            comm = comm.checked_add(engine::fee_ceil_micro(notional, rate).ok()?)?;
        }
        let p = engine::open_profit_micro(&list).ok()?;
        Some((list.len(), p, comm))
    }

    // ------------------------------------------------------------------ consumption at journal events

    /// A `Fill` of a limit or stop order must be the fill of a pending order the samples trigger.
    pub fn walk_fill(&mut self, ctx: Ctx, market: u16, kind: OrderKind, intent: u32) {
        let Some(mut w) = self.walk.take() else { return };
        if matches!(kind, OrderKind::Limit | OrderKind::Stop) {
            let i = w.exp.iter().position(|e| e.sec == ctx.time && matches!(&e.kind, ExpKind::Pending(m, n, _) if *m == market && *n == intent));
            match i {
                Some(i) => {
                    w.exp.remove(i);
                }
                None => self.f.add(
                    FindingClass::TriggerFalse,
                    ctx,
                    "a pending order the samples trigger at this second",
                    format!("{kind:?} fill of intent {intent}"),
                    "the fill of a pending order that no sample of this second triggers",
                ),
            }
            w.filled.insert(intent);
        }
        self.walk = Some(w);
    }

    /// A stop-loss or take-profit `Close` must be a trigger the samples produce.
    pub fn walk_close(&mut self, ctx: Ctx, market: u16, reason: CloseReason) {
        let want = match reason {
            CloseReason::StopLoss => ProtectiveKind::StopLoss,
            CloseReason::TakeProfit => ProtectiveKind::TakeProfit,
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
            rp.walk_close(at(10), 1, CloseReason::StopLoss); // other market
            rp.walk_close(at(10), 0, CloseReason::TakeProfit); // other kind
            rp.walk_close(at(11), 0, CloseReason::StopLoss); // other second
            assert_eq!(classes(rp), vec![FindingClass::TriggerFalse; 3]);
            assert_eq!(left(rp), 1);
            // closes that are not protective are not judged here
            for r in [CloseReason::Manual, CloseReason::Flip, CloseReason::Breach, CloseReason::Fail] {
                rp.walk_close(at(10), 0, r);
            }
            assert_eq!((rp.f.list.len(), left(rp)), (3, 1));
            rp.walk_close(at(10), 0, CloseReason::StopLoss);
            assert_eq!((rp.f.list.len(), left(rp)), (3, 0));
            put(rp, trigger(10, 0, ProtectiveKind::TakeProfit));
            rp.walk_close(at(10), 0, CloseReason::TakeProfit);
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
