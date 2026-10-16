//! What a closed second does: holding fees at the 22:00 UTC rollovers, the decision pass over its samples (floors, stop-loss
//! and take-profit, pending orders), the events that follow from it, then the queued orders of the second.
//!
//! The decision for a sample uses the account as it stood at the START of its second (after the rollover fees), the latest
//! sample of every market as the marks, and the same engine functions the public verifier's archive walk uses; when several
//! samples of one second give the same kind of decision, the LAST one's numbers are acted on (handover/verifier-formats.md 7).
use std::collections::BTreeMap;

use engine::{AccountState, FailReason, ProtectiveKind, SampleAction, Side as ESide};
use journal_types::{market_open, CloseReason, FeeKind, JournalEvent, Side, NO_PROOF};

use crate::config::*;
use crate::eval::{Bucket, Evaluator};
use crate::types::*;

/// What one second's decision pass asks for.
#[derive(Default)]
struct Plan {
    /// (sample, p_open, r, l_start, g_peak, l_level)
    tight_start: Option<(usize, i64, i64, i64, i64, i64)>,
    /// (sample, g_peak, l_level)
    raised: Option<(usize, i64, i64)>,
    breach: Option<usize>,
    fail: Option<usize>,
    /// (market, 0 stop-loss / 1 take-profit) -> sample
    triggers: BTreeMap<(u16, u8), usize>,
    /// (market, nonce) -> (price, sample)
    fills: BTreeMap<(u16, u32), (u64, usize)>,
}

pub(crate) fn side_of(long: bool) -> Side {
    if long {
        Side::Long
    } else {
        Side::Short
    }
}

pub(crate) fn eside(long: bool) -> ESide {
    if long {
        ESide::Buy
    } else {
        ESide::Sell
    }
}

impl Evaluator {
    /// Whether a market is open at `sec` given the second of its last price change (sessions AND not stale).
    pub(crate) fn market_is_open(m: &MarketCfg, sec: i64, last_change: i64) -> bool {
        let count = u8::try_from(m.sessions.len()).unwrap_or(u8::MAX);
        market_open(&m.sessions, count, sec, last_change, u32::from(m.max_age_secs))
    }

    /// Closes every second before `c`: processes the buckets in order, then the rollovers up to `c - 1`.
    pub(crate) fn close_before(&mut self, c: i64) {
        while let Some((&x, _)) = self.buckets.iter().next() {
            if x >= c {
                break;
            }
            let Some(bucket) = self.buckets.remove(&x) else { break };
            if let Err(e) = self.process_second(x, bucket) {
                self.fail_internal(e);
                return;
            }
        }
        let upto = c.saturating_sub(1);
        if !self.is_ended() {
            if let Err(e) = self.apply_rollovers(upto) {
                self.fail_internal(e);
                return;
            }
        }
        self.processed_through = self.processed_through.max(upto);
    }

    pub(crate) fn push_unit(&mut self, events: Vec<QEvent>) {
        if !events.is_empty() {
            self.queue.push_back(Unit { events });
        }
    }

    /// Snapshot of the account after an event at `time`.
    pub(crate) fn snap(&self, time: i64) -> Snap {
        Snap { st: self.st, trades_closed: self.trades_closed, last_time: time, flat: self.positions.is_empty() }
    }

    /// An event with its state-after, justified by `proof` (an index into the sample list).
    pub(crate) fn qevent(&self, ev: JournalEvent, proof: Option<usize>, time: i64) -> QEvent {
        QEvent { ev, proof, after: self.snap(time) }
    }

    // ------------------------------------------------------------------ rollovers

    /// First rollover instant (22:00:00 UTC) strictly after `t`.
    pub(crate) fn next_rollover_after(t: i64) -> i64 {
        engine::day_index(t).saturating_add(1).saturating_mul(86_400).saturating_add(22 * 3_600)
    }

    /// One `Fee{Holding}` per night per position held at each rollover instant `R <= upto` that has not been charged:
    /// `holding_fee_micro(|q|, mark, rate, 1)` with the mark the bid (long) or ask (short) of the LAST sample of the market
    /// at or before `R`, dated `R`. A position opened at or after `R` is not charged for it.
    pub(crate) fn apply_rollovers(&mut self, upto: i64) -> Result<(), EvalError> {
        if upto <= self.roll_cursor {
            return Ok(());
        }
        let mut r = Self::next_rollover_after(self.roll_cursor);
        while r <= upto && !self.positions.is_empty() {
            let unit = self.charge_night(r)?;
            // the fees of one night form one unit (a batch cut between two of them would let the verifier judge the
            // samples of second R after the first fee but before the second); with more than seven positions the unit
            // is chunked so that every chunk still fits a batch with its `Meta`
            for chunk in unit.chunks(7) {
                self.push_unit(chunk.to_vec());
            }
            r = Self::next_rollover_after(r);
        }
        self.roll_cursor = upto;
        Ok(())
    }

    fn charge_night(&mut self, r: i64) -> Result<Vec<QEvent>, EvalError> {
        let today = engine::day_index(r);
        let mut unit = Vec::new();
        let list: Vec<(u16, Pos)> = self.positions.iter().map(|(k, v)| (*k, *v)).collect();
        for (market, pos) in list {
            if pos.p.fee_day >= today {
                continue;
            }
            let class = self.market_cfg(market).map(|m| m.class).ok_or(EvalError::Invariant("position in an unknown market"))?;
            let rate = self.rules.holding(class);
            if rate > 0 {
                let s = self.mark_at_or_before(market, r).ok_or(EvalError::Invariant("no mark for a holding fee"))?;
                let mark = if pos.p.qty.is_positive() { s.bid } else { s.ask };
                let fee = engine::holding_fee_micro(pos.p.qty.unsigned_abs(), mark, rate, 1)?;
                let amount = u64::try_from(fee).map_err(|_| EvalError::Overflow)?;
                self.debit(fee)?;
                self.stats.holding_fees = self.stats.holding_fees.saturating_add(1);
                let ev = JournalEvent::Fee { kind: FeeKind::Holding, market, amount, time: r };
                let q = self.qevent(ev, None, r);
                unit.push(q);
            }
            if let Some(p) = self.positions.get_mut(&market) {
                p.p.fee_day = today;
            }
        }
        Ok(unit)
    }

    /// The last sample of `market` whose source second is `<= sec`.
    pub(crate) fn mark_at_or_before(&self, market: u16, sec: i64) -> Option<Stored> {
        let t = self.tracks.get(&market)?;
        let n = t.idxs.partition_point(|&i| self.samples.get(i).is_some_and(|s| s.sec() <= sec));
        let i = n.checked_sub(1).and_then(|k| t.idxs.get(k)).copied()?;
        self.samples.get(i).copied()
    }

    /// Debits a fee from the closed balance with the engine's own function (a fee can never latch or fail anything).
    pub(crate) fn debit(&mut self, amount: i64) -> Result<(), EvalError> {
        let neg = amount.checked_neg().ok_or(EvalError::Overflow)?;
        self.st = engine::apply_closed_slice(&self.st, &self.erules, neg, 0)?.state;
        Ok(())
    }

    // ------------------------------------------------------------------ one second

    pub(crate) fn process_second(&mut self, x: i64, bucket: Bucket) -> Result<(), EvalError> {
        if !self.is_ended() {
            self.apply_rollovers(x)?;
        }
        if self.is_ended() {
            for &i in &bucket.samples {
                if let Some(s) = self.samples.get(i) {
                    self.marks.insert(s.rec.market, i);
                }
            }
            return Ok(());
        }
        let plan = self.decide(x, &bucket.samples)?;
        self.act(x, plan)?;
        let mut sltp = Vec::new();
        for q in bucket.orders {
            if self.is_ended() {
                break;
            }
            if q.intent.kind == crate::codec::IntentKind::SetSlTp {
                sltp.push(q);
            } else {
                self.exec_order(q, x)?;
            }
        }
        if !self.is_ended() {
            for q in sltp {
                self.apply_sl_tp(q, x);
            }
        }
        Ok(())
    }

    /// Open profit and estimated closing commission of `positions` at the current `marks`.
    pub(crate) fn profit_and_commission(&self, positions: &BTreeMap<u16, Pos>, marks: &BTreeMap<u16, usize>) -> Result<(i64, i64), EvalError> {
        let mut list = Vec::new();
        let mut comm = 0i64;
        for (m, p) in positions {
            let s = marks.get(m).and_then(|i| self.samples.get(*i)).ok_or(EvalError::Invariant("a position without a mark"))?;
            list.push((p.p, s.bid, s.ask));
            let class = self.market_cfg(*m).map(|c| c.class).ok_or(EvalError::Invariant("position in an unknown market"))?;
            let mark = if p.p.qty.is_positive() { s.bid } else { s.ask };
            comm = comm.checked_add(self.commission_micro(class, p.p.qty.unsigned_abs(), mark)?).ok_or(EvalError::Overflow)?;
        }
        Ok((engine::open_profit_micro(&list)?, comm))
    }

    /// `ceil(rate x margin_micro(qty, price, 1))`: the commission of a fill or a closing slice (0 when the class has no rate).
    pub(crate) fn commission_micro(&self, class: u8, qty: u64, price: u64) -> Result<i64, EvalError> {
        let rate = self.rules.commission(class);
        if rate == 0 {
            return Ok(0);
        }
        let notional = engine::margin_micro(qty, price, 1)?;
        Ok(engine::fee_ceil_micro(notional, rate)?)
    }

    fn decide(&mut self, sec: i64, samples: &[usize]) -> Result<Plan, EvalError> {
        let st0: AccountState = self.st;
        let pos0 = self.positions.clone();
        let locked = !engine::can_increase_exposure(&st0, sec);
        let mut plan = Plan::default();
        for &j in samples {
            let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
            let m = s.rec.market;
            self.marks.insert(m, j);
            let cfg = self.market_cfg(m).ok_or(EvalError::Invariant("sample of an unknown market"))?;
            let open = Self::market_is_open(&cfg, sec, s.last_change);
            if !open {
                if pos0.contains_key(&m) {
                    self.stats.closed_with_position = self.stats.closed_with_position.saturating_add(1);
                }
                continue;
            }
            let mut floors_allow = true;
            if !pos0.is_empty() {
                let (p_open, comm) = self.profit_and_commission(&pos0, &self.marks)?;
                let d = engine::evaluate_sample(&st0, &self.erules, p_open, comm, sec)?;
                match d.action {
                    SampleAction::None => {}
                    SampleAction::TightStart { r, l_start, g_peak, l_level } => plan.tight_start = Some((j, p_open, r, l_start, g_peak, l_level)),
                    SampleAction::FloorRaised { g_peak, l_level } => plan.raised = Some((j, g_peak, l_level)),
                    SampleAction::Breach { .. } => plan.breach = Some(j),
                    SampleAction::Fail(FailReason::StaticFloor) => plan.fail = Some(j),
                    SampleAction::Fail(FailReason::PhaseQuick) => {}
                }
                floors_allow = engine::sl_tp_allowed_after(&d.action);
            }
            if !floors_allow {
                continue;
            }
            if let Some(p) = pos0.get(&m) {
                let side = if p.p.qty.is_positive() { ESide::Buy } else { ESide::Sell };
                if let Some(t) = engine::check_protective(side, p.sl, p.tp, s.bid, s.ask, true) {
                    let k = match t.kind {
                        ProtectiveKind::StopLoss => 0u8,
                        ProtectiveKind::TakeProfit => 1u8,
                    };
                    plan.triggers.insert((m, k), j);
                }
            }
            for o in self.pending.iter().filter(|o| o.intent.market == m && o.intent.client_time < sec) {
                let kind = if o.intent.kind == crate::codec::IntentKind::Limit { engine::PendingKind::Limit } else { engine::PendingKind::Stop };
                let side = if o.intent.side == 0 { ESide::Buy } else { ESide::Sell };
                if let Some(price) = engine::check_pending(side, kind, o.intent.price_e12, s.bid, s.ask, true, locked) {
                    plan.fills.insert((m, o.nonce), (price, j));
                }
            }
        }
        Ok(plan)
    }

    // ------------------------------------------------------------------ acting on a plan

    fn act(&mut self, sec: i64, plan: Plan) -> Result<(), EvalError> {
        if let Some((j, p_open, r, l_start, g_peak, l_level)) = plan.tight_start {
            let next = engine::check_tight_start(&self.st, p_open, r, l_start, g_peak, l_level).map_err(|_| EvalError::Invariant("tight start claim"))?;
            self.st = next;
            self.stats.tight_starts = self.stats.tight_starts.saturating_add(1);
            let ev = JournalEvent::TightStart { time: sec, p_open, r, l_start, g_peak, l_level, proof: NO_PROOF };
            let q = self.qevent(ev, Some(j), sec);
            self.push_unit(vec![q]);
        }
        if let Some((j, g_peak, l_level)) = plan.raised {
            let next = engine::check_floor_raised(&self.st, g_peak, l_level).map_err(|_| EvalError::Invariant("floor raise claim"))?;
            self.st = next;
            self.stats.floor_raises = self.stats.floor_raises.saturating_add(1);
            let ev = JournalEvent::FloorRaised { time: sec, g_peak, l_level, proof: NO_PROOF };
            let q = self.qevent(ev, Some(j), sec);
            self.push_unit(vec![q]);
        }
        for ((market, kind), j) in plan.triggers {
            if self.is_ended() {
                break;
            }
            self.execute_trigger(sec, market, kind, j)?;
        }
        for ((market, nonce), (price, j)) in plan.fills {
            if self.is_ended() {
                break;
            }
            self.execute_pending_fill(sec, market, nonce, price, j)?;
        }
        if self.is_ended() {
            return Ok(());
        }
        if let Some(j) = plan.breach {
            self.do_breach(sec, j)?;
        } else if let Some(j) = plan.fail {
            self.do_static_fail(sec, j)?;
        }
        Ok(())
    }

    fn execute_trigger(&mut self, sec: i64, market: u16, kind: u8, j: usize) -> Result<(), EvalError> {
        let Some(pos) = self.positions.get(&market).copied() else { return Ok(()) };
        let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
        let long = pos.p.qty.is_positive();
        let price = if long { s.bid } else { s.ask };
        let (reason, counter) = if kind == 0 { (CloseReason::StopLoss, &mut self.stats.sl_triggers) } else { (CloseReason::TakeProfit, &mut self.stats.tp_triggers) };
        *counter = counter.saturating_add(1);
        self.close_checked(sec, market, pos.p.qty.unsigned_abs(), price, j, reason, 0)?;
        Ok(())
    }

    fn do_breach(&mut self, sec: i64, j: usize) -> Result<(), EvalError> {
        let mut units = self.force_close_all(sec, CloseReason::Breach, None)?;
        let over = self.quick_over();
        let tail = if over {
            Some(self.fail_event(sec, j, journal_types::FailReason::PhaseQuick))
        } else if self.st.b <= engine::static_floor_micro(&self.st)? {
            Some(self.fail_event(sec, j, journal_types::FailReason::StaticFloor))
        } else {
            let (l_level, b_after) = (self.st.l_level, self.st.b);
            match engine::check_breach(&self.st, sec, l_level, b_after) {
                Ok(next) => {
                    self.st = next;
                    self.stats.breaches = self.stats.breaches.saturating_add(1);
                    let ev = JournalEvent::Breach { time: sec, l_level, b_after, proof: NO_PROOF };
                    Some(self.qevent(ev, Some(j), sec))
                }
                Err(_) => {
                    // the closes left the balance above the tight floor (another second's gain): the mode just ends
                    if self.st.tight_on {
                        self.st = engine::check_tight_end(&self.st).map_err(|_| EvalError::Invariant("tight end"))?;
                        self.stats.tight_ends = self.stats.tight_ends.saturating_add(1);
                        Some(self.qevent(JournalEvent::TightEnd { time: sec }, None, sec))
                    } else {
                        None
                    }
                }
            }
        };
        self.finish_units(&mut units, tail);
        for u in units {
            self.push_unit(u);
        }
        Ok(())
    }

    fn do_static_fail(&mut self, sec: i64, j: usize) -> Result<(), EvalError> {
        let mut units = self.force_close_all(sec, CloseReason::Fail, None)?;
        let tail = if self.quick_over() {
            Some(self.fail_event(sec, j, journal_types::FailReason::PhaseQuick))
        } else if self.st.b <= engine::static_floor_micro(&self.st)? {
            Some(self.fail_event(sec, j, journal_types::FailReason::StaticFloor))
        } else if self.st.tight_on {
            self.st = engine::check_tight_end(&self.st).map_err(|_| EvalError::Invariant("tight end"))?;
            self.stats.tight_ends = self.stats.tight_ends.saturating_add(1);
            Some(self.qevent(JournalEvent::TightEnd { time: sec }, None, sec))
        } else {
            None
        };
        self.finish_units(&mut units, tail);
        for u in units {
            self.push_unit(u);
        }
        Ok(())
    }

    /// Appends the tail event (a `Breach`, `Fail` or `TightEnd`) to the last unit, or makes it a unit of its own.
    pub(crate) fn finish_units(&mut self, units: &mut Vec<Vec<QEvent>>, tail: Option<QEvent>) {
        let Some(t) = tail else { return };
        if let JournalEvent::Fail { reason, .. } = &t.ev {
            let fr = match reason {
                journal_types::FailReason::StaticFloor => FailReason::StaticFloor,
                journal_types::FailReason::PhaseQuick => FailReason::PhaseQuick,
            };
            match fr {
                FailReason::StaticFloor => self.stats.static_fails = self.stats.static_fails.saturating_add(1),
                FailReason::PhaseQuick => self.stats.quick_fails = self.stats.quick_fails.saturating_add(1),
            }
            self.status = Status::Failed(fr);
            self.pending.clear();
        }
        match units.last_mut() {
            Some(u) => u.push(t),
            None => units.push(vec![t]),
        }
    }

    pub(crate) fn fail_event(&mut self, sec: i64, j: usize, reason: journal_types::FailReason) -> QEvent {
        let b_after = self.st.b;
        let ev = JournalEvent::Fail { time: sec, reason, b_after, proof: NO_PROOF };
        self.qevent(ev, Some(j), sec)
    }

    /// Whether the quick tally is over the limit (the phase has failed).
    pub(crate) fn quick_over(&self) -> bool {
        engine::apply_closed_slice(&self.st, &self.erules, 0, 0).map(|d| d.fail.is_some()).unwrap_or(false)
    }
}
