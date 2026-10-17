//! Orders and closes: arrival checks, execution of market / close / pending orders, netting into `Close` and `Fill`
//! events with their commissions, forced closes for a breach or a fail, and the quick-limit ordering rule.
use engine::StopKind;
use journal_types::{CloseReason, FeeKind, JournalEvent, OrderKind, NO_PROOF};

use crate::codec::{IntentKind, OrderIntent};
use crate::config::*;
use crate::decide::{eside, side_of};
use crate::eval::{Evaluator, QOrder};
use crate::types::*;

type Reject = Result<(), OrderReject>;

impl Evaluator {
    /// The newest sample of `market` recorded before the order arrived (`arrival` samples existed), no later than second `t`.
    fn order_sample(&self, market: u16, arrival: usize, t: i64) -> Option<usize> {
        let tr = self.tracks.get(&market)?;
        tr.idxs.iter().rev().copied().find(|&i| i < arrival && self.samples.get(i).is_some_and(|s| s.sec() <= t))
    }

    /// Price freshness and market state for an order at second `t` against sample `j`: the newest sample is no older than
    /// `max_age_secs`, and the market is inside its session windows with a price that changed within `max_age_secs`.
    fn market_state_ok(&self, cfg: &MarketCfg, j: usize, t: i64) -> Reject {
        let s = self.samples.get(j).ok_or(OrderReject::NoFreshPrice)?;
        if t.saturating_sub(s.sec()) > i64::from(cfg.max_age_secs) {
            return Err(OrderReject::NoFreshPrice);
        }
        if !Self::market_is_open(cfg, t, s.last_change) {
            return Err(OrderReject::MarketClosed);
        }
        Ok(())
    }

    fn count_closed(&mut self, r: &Reject) {
        if matches!(r, Err(OrderReject::MarketClosed) | Err(OrderReject::NoFreshPrice)) {
            self.stats.closed_market_rejects = self.stats.closed_market_rejects.saturating_add(1);
        }
    }

    // ------------------------------------------------------------------ arrival

    /// Everything about an order that can be decided when it arrives. A refused order is not recorded anywhere.
    pub(crate) fn validate_arrival(&mut self, it: &OrderIntent) -> Reject {
        let r = self.validate_arrival_inner(it);
        self.count_closed(&r);
        r
    }

    fn validate_arrival_inner(&self, it: &OrderIntent) -> Reject {
        if it.kind == IntentKind::Cancel {
            return if self.pending.iter().any(|p| p.nonce == it.target_nonce) { Ok(()) } else { Err(OrderReject::NoSuchOrder) };
        }
        let cfg = self.market_cfg(it.market).ok_or(OrderReject::UnknownMarket)?;
        let latest = self.tracks.get(&it.market).and_then(|t| t.idxs.last().copied());
        let pos = self.positions.get(&it.market);
        match it.kind {
            IntentKind::Market | IntentKind::Limit | IntentKind::Stop => {
                engine::validate_quantity(it.qty, cfg.qty_step, cfg.qty_min).map_err(|_| OrderReject::InvalidQuantity)?;
                if self.held {
                    // a held account may still reduce, but an order of unknown netting is refused up front only for pending kinds
                    if it.kind != IntentKind::Market {
                        return Err(OrderReject::AccountHeld);
                    }
                }
                let j = latest.ok_or(OrderReject::NoFreshPrice)?;
                self.market_state_ok(&cfg, j, it.client_time)?;
                if it.kind != IntentKind::Market && cfg.status != STATUS_ENABLED {
                    return Err(OrderReject::MarketNotTradable);
                }
                // decision 64: an order that opens or adds in a market the account's rule set does not price is refused at
                // placement. The rule set is immutable, so the half-spread never becomes non-zero for this account: a pending
                // order refused here would never fill, which is why it is not recorded (a refusal at the trigger, decision 27,
                // would stay pending forever and be the same refusal every second).
                if cfg.half_spread_e6 == 0 && would_open(pos, it) {
                    return Err(OrderReject::MarketUnpriced);
                }
                if it.kind != IntentKind::Market {
                    if it.price_e12 == 0 {
                        return Err(OrderReject::InvalidLevel);
                    }
                    if self.pending.len() >= MAX_PENDING {
                        return Err(OrderReject::TooManyPending);
                    }
                    let long = it.side == 0;
                    let (sl_ok, tp_ok) = if long {
                        (it.sl_e12 == 0 || it.sl_e12 < it.price_e12, it.tp_e12 == 0 || it.tp_e12 > it.price_e12)
                    } else {
                        (it.sl_e12 == 0 || it.sl_e12 > it.price_e12, it.tp_e12 == 0 || it.tp_e12 < it.price_e12)
                    };
                    if !sl_ok || !tp_ok {
                        return Err(OrderReject::InvalidLevel);
                    }
                    let notional = engine::units::notional_micro(it.qty, it.price_e12).map_err(|_| OrderReject::Engine)?;
                    if notional > i128::from(cfg.max_notional_micro) {
                        return Err(OrderReject::NotionalCap);
                    }
                }
                Ok(())
            }
            IntentKind::Close => {
                let p = pos.ok_or(OrderReject::NoPosition)?;
                if it.qty != 0 {
                    engine::validate_quantity(it.qty, cfg.qty_step, cfg.qty_min).map_err(|_| OrderReject::InvalidQuantity)?;
                    if it.qty > p.p.qty.unsigned_abs() {
                        return Err(OrderReject::InvalidQuantity);
                    }
                }
                let j = latest.ok_or(OrderReject::NoFreshPrice)?;
                self.market_state_ok(&cfg, j, it.client_time)
            }
            IntentKind::SetSlTp => {
                let p = pos.ok_or(OrderReject::NoPosition)?;
                let j = latest.ok_or(OrderReject::NoFreshPrice)?;
                let s = self.samples.get(j).ok_or(OrderReject::NoFreshPrice)?;
                let side = if p.p.qty.is_positive() { engine::Side::Buy } else { engine::Side::Sell };
                for (level, kind) in [(it.sl_e12, StopKind::StopLoss), (it.tp_e12, StopKind::TakeProfit)] {
                    if level != 0 {
                        engine::validate_stop_level(side, kind, level, s.bid, s.ask).map_err(|_| OrderReject::InvalidLevel)?;
                    }
                }
                Ok(())
            }
            IntentKind::Cancel => Ok(()),
        }
    }

    // ------------------------------------------------------------------ execution of queued orders

    /// Executes one queued order at second `t` (after the second's decision pass). A refusal is silent: no event.
    pub(crate) fn exec_order(&mut self, q: QOrder, t: i64) -> Result<(), EvalError> {
        match q.intent.kind {
            IntentKind::Market => {
                let Some(cfg) = self.market_cfg(q.intent.market) else { return Ok(()) };
                let Some(j) = self.order_sample(q.intent.market, q.arrival, t) else {
                    self.refuse(t, q.nonce, OrderReject::NoFreshPrice);
                    return Ok(());
                };
                let r = self.market_state_ok(&cfg, j, t);
                self.count_closed(&r);
                if let Err(reason) = r {
                    self.refuse(t, q.nonce, reason);
                    return Ok(());
                }
                let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
                let price = if q.intent.side == 0 { s.ask } else { s.bid };
                if let Err(reason) = self.exec_trade(q.nonce, &q.intent, j, OrderKind::Market, t, price)? {
                    self.refuse(t, q.nonce, reason);
                }
            }
            IntentKind::Close => self.exec_close_order(q, t)?,
            IntentKind::Cancel => self.pending.retain(|p| p.nonce != q.intent.target_nonce),
            IntentKind::Limit | IntentKind::Stop | IntentKind::SetSlTp => {}
        }
        Ok(())
    }

    /// `SetSlTp`: the new levels replace the position's if it was already open at the order's `client_time`.
    pub(crate) fn apply_sl_tp(&mut self, q: QOrder, _t: i64) {
        if let Some(p) = self.positions.get_mut(&q.intent.market) {
            if p.p.opened_at <= q.intent.client_time {
                p.sl = q.intent.sl_e12;
                p.tp = q.intent.tp_e12;
            }
        }
    }

    fn exec_close_order(&mut self, q: QOrder, t: i64) -> Result<(), EvalError> {
        let it = q.intent;
        let Some(cfg) = self.market_cfg(it.market) else { return Ok(()) };
        let Some(pos) = self.positions.get(&it.market).copied() else {
            self.refuse(t, q.nonce, OrderReject::NoPosition);
            return Ok(());
        };
        let Some(j) = self.order_sample(it.market, q.arrival, t) else {
            self.refuse(t, q.nonce, OrderReject::NoFreshPrice);
            return Ok(());
        };
        let r = self.market_state_ok(&cfg, j, t);
        self.count_closed(&r);
        if let Err(reason) = r {
            self.refuse(t, q.nonce, reason);
            return Ok(());
        }
        let held_qty = pos.p.qty.unsigned_abs();
        let qty = if it.qty == 0 { held_qty } else { it.qty };
        if qty > held_qty {
            self.refuse(t, q.nonce, OrderReject::InvalidQuantity);
            return Ok(());
        }
        let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
        let price = if pos.p.qty.is_positive() { s.bid } else { s.ask };
        self.close_checked(t, it.market, qty, price, j, CloseReason::Manual, q.nonce)?;
        Ok(())
    }

    /// A triggered limit or stop order, filled at the sample's price. Refused (and kept pending) when the account cannot
    /// margin it; the verifier expects no event for such an order (M9b decision D3).
    pub(crate) fn execute_pending_fill(&mut self, sec: i64, market: u16, nonce: u32, price: u64, j: usize) -> Result<(), EvalError> {
        let Some(idx) = self.pending.iter().position(|p| p.nonce == nonce && p.intent.market == market) else { return Ok(()) };
        let it = self.pending[idx].intent;
        let kind = if it.kind == IntentKind::Limit { OrderKind::Limit } else { OrderKind::Stop };
        match self.exec_trade(nonce, &it, j, kind, sec, price)? {
            Ok(()) => {
                if let Some(i) = self.pending.iter().position(|p| p.nonce == nonce) {
                    self.pending.remove(i);
                }
                if kind == OrderKind::Limit {
                    self.stats.limit_fills = self.stats.limit_fills.saturating_add(1);
                } else {
                    self.stats.stop_fills = self.stats.stop_fills.saturating_add(1);
                }
            }
            Err(reason) => {
                self.stats.refused_fills = self.stats.refused_fills.saturating_add(1);
                self.refuse(sec, nonce, reason);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ the trade itself

    /// Nets an order of `it.qty` at `price` (the sample `j`'s ask for a buy, bid for a sell) into the position of its
    /// market: a closing slice (`Close`) and/or an opening part (`Fill`), each with its commission. Returns the order's
    /// refusal, if any; nothing is changed then.
    pub(crate) fn exec_trade(&mut self, nonce: u32, it: &OrderIntent, j: usize, kind: OrderKind, t: i64, price: u64) -> Result<Reject, EvalError> {
        let cfg = self.market_cfg(it.market).ok_or(EvalError::Invariant("order in an unknown market"))?;
        let long = it.side == 0;
        let existing = self.positions.get(&it.market).copied();
        let (close_qty, open_qty) = match existing {
            Some(p) if (p.p.qty.is_positive()) != long => {
                let c = it.qty.min(p.p.qty.unsigned_abs());
                (c, it.qty.saturating_sub(c))
            }
            _ => (0, it.qty),
        };
        let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
        if open_qty > 0 {
            if self.held {
                return Ok(Err(OrderReject::AccountHeld));
            }
            if !engine::can_increase_exposure(&self.st, t) {
                return Ok(Err(OrderReject::AccountLocked));
            }
            if cfg.status != STATUS_ENABLED {
                return Ok(Err(OrderReject::MarketNotTradable));
            }
            // decision 64 (fill time, the order the verifier mirrors: held, lock, status, UNPRICED, then the quantity of a flip's
            // remainder, positions, levels, cap, margin): the same refusal for a triggered pending order and a market order
            if cfg.half_spread_e6 == 0 {
                return Ok(Err(OrderReject::MarketUnpriced));
            }
            if close_qty > 0 && engine::validate_quantity(open_qty, cfg.qty_step, cfg.qty_min).is_err() {
                return Ok(Err(OrderReject::InvalidRemainder));
            }
            if existing.is_none() && self.positions.len() >= MAX_POSITIONS {
                return Ok(Err(OrderReject::TooManyPositions));
            }
            if kind == OrderKind::Market {
                let pside = if long { engine::Side::Buy } else { engine::Side::Sell };
                for (level, k) in [(it.sl_e12, StopKind::StopLoss), (it.tp_e12, StopKind::TakeProfit)] {
                    if level != 0 && engine::validate_stop_level(pside, k, level, s.bid, s.ask).is_err() {
                        return Ok(Err(OrderReject::InvalidLevel));
                    }
                }
            }
            // risk check on the position the order leaves behind
            let day = engine::day_index(t);
            let mut pos_after = self.positions.clone();
            let mut b_after = self.st.b;
            let mut base = existing.map(|p| p.p);
            let mut keep_levels = existing.map(|p| (p.sl, p.tp)).unwrap_or((0, 0));
            if close_qty > 0 {
                let o = engine::apply_fill(base, eside(long), close_qty, price, t, day)?;
                b_after = b_after.checked_add(o.balance_delta).ok_or(EvalError::Overflow)?;
                base = o.position;
                keep_levels = (0, 0);
            }
            let o = engine::apply_fill(base, eside(long), open_qty, price, t, day)?;
            let np = o.position.ok_or(EvalError::Invariant("an opening fill leaves a position"))?;
            let new_sl = if it.sl_e12 != 0 { it.sl_e12 } else { keep_levels.0 };
            let new_tp = if it.tp_e12 != 0 { it.tp_e12 } else { keep_levels.1 };
            pos_after.insert(it.market, Pos { p: np, sl: new_sl, tp: new_tp });
            let notional = engine::units::notional_micro(np.qty.unsigned_abs(), np.avg_entry_e12)?;
            if notional > i128::from(cfg.max_notional_micro) {
                return Ok(Err(OrderReject::NotionalCap));
            }
            let mut marks = self.marks.clone();
            marks.insert(it.market, j);
            let (p_open, comm) = self.profit_and_commission(&pos_after, &marks)?;
            let fill_comm = self.commission_micro(cfg.class, open_qty, price)?;
            let equity = i128::from(b_after) + i128::from(p_open) - i128::from(comm) - i128::from(fill_comm);
            let mut used: i128 = 0;
            for (m, p) in &pos_after {
                let class = self.market_cfg(*m).map(|c| c.class).ok_or(EvalError::Invariant("position in an unknown market"))?;
                let lev = self.rules.leverage(class).ok_or(EvalError::Invariant("class without leverage"))?;
                used += i128::from(engine::margin_micro(p.p.qty.unsigned_abs(), p.p.avg_entry_e12, lev)?);
            }
            if equity - used < 0 {
                return Ok(Err(OrderReject::InsufficientMargin));
            }
            if equity == used && kind == OrderKind::Market {
                self.stats.margin_exact = self.stats.margin_exact.saturating_add(1);
            }
        }
        // ---- execution
        if close_qty > 0 && open_qty == 0 {
            // a pure reduce or close
            self.close_checked(t, it.market, close_qty, price, j, CloseReason::Manual, nonce)?;
            return Ok(Ok(()));
        }
        let mut events: Vec<QEvent> = Vec::new();
        if close_qty > 0 {
            // a flip: if the closing slice alone would overshoot the quick limit the account fails and the rest is void
            let pos = existing.ok_or(EvalError::Invariant("flip without a position"))?;
            let prev = self.preview_close(&pos, close_qty, price, t)?;
            if self.would_exceed_quick(prev.0, prev.1) {
                self.close_checked(t, it.market, close_qty, price, j, CloseReason::Flip, nonce)?;
                return Ok(Ok(()));
            }
            let (evs, _over) = self.do_close_event(t, it.market, close_qty, price, j, CloseReason::Flip, nonce, false)?;
            events.extend(evs);
            self.stats.flips = self.stats.flips.saturating_add(1);
        }
        events.extend(self.do_fill_event(t, it, open_qty, price, j, kind, nonce, close_qty > 0)?);
        self.push_unit(events);
        Ok(Ok(()))
    }

    /// The `Fill` for an opening part and its commission; updates the position and the book.
    #[allow(clippy::too_many_arguments)]
    fn do_fill_event(&mut self, t: i64, it: &OrderIntent, qty: u64, price: u64, j: usize, kind: OrderKind, nonce: u32, after_flip: bool) -> Result<Vec<QEvent>, EvalError> {
        let cfg = self.market_cfg(it.market).ok_or(EvalError::Invariant("order in an unknown market"))?;
        let long = it.side == 0;
        let existing = if after_flip { None } else { self.positions.get(&it.market).copied() };
        let out = engine::apply_fill(existing.map(|p| p.p), eside(long), qty, price, t, engine::day_index(t))?;
        let np = out.position.ok_or(EvalError::Invariant("an opening fill leaves a position"))?;
        let (osl, otp) = existing.map(|p| (p.sl, p.tp)).unwrap_or((0, 0));
        self.positions.insert(
            it.market,
            Pos { p: np, sl: if it.sl_e12 != 0 { it.sl_e12 } else { osl }, tp: if it.tp_e12 != 0 { it.tp_e12 } else { otp } },
        );
        let lev = self.rules.leverage(cfg.class).ok_or(EvalError::Invariant("class without leverage"))?;
        let margin = u64::try_from(engine::margin_micro(np.qty.unsigned_abs(), np.avg_entry_e12, lev)?).map_err(|_| EvalError::Overflow)?;
        let ev = JournalEvent::Fill {
            market: it.market,
            side: side_of(long),
            order_kind: kind,
            qty,
            price_e12: price,
            time: t,
            avg_entry_after: np.avg_entry_e12,
            margin,
            sl: it.sl_e12,
            tp: it.tp_e12,
            intent_id: nonce,
            proof: NO_PROOF,
        };
        let mut evs = vec![self.qevent(ev, Some(j), t)];
        let fee = self.commission_micro(cfg.class, qty, price)?;
        if fee > 0 {
            self.debit(fee)?;
            let amount = u64::try_from(fee).map_err(|_| EvalError::Overflow)?;
            evs.push(self.qevent(JournalEvent::Fee { kind: FeeKind::Commission, market: it.market, amount, time: t }, None, t));
        }
        Ok(evs)
    }

    // ------------------------------------------------------------------ closing

    /// (credit, quick-tally delta) of closing `qty` of `pos` at `price` at `t`, without changing anything.
    pub(crate) fn preview_close(&self, pos: &Pos, qty: u64, price: u64, t: i64) -> Result<(i64, i64), EvalError> {
        let long = pos.p.qty.is_positive();
        let out = engine::apply_fill(Some(pos.p), eside(!long), qty, price, t, engine::day_index(t))?;
        Ok((out.balance_delta, out.quick_tally_delta))
    }

    /// Whether booking a slice with this credit and tally delta pushes the quick tally over the limit.
    pub(crate) fn would_exceed_quick(&self, credit: i64, tally: i64) -> bool {
        engine::apply_closed_slice(&self.st, &self.erules, credit, tally).map(|d| d.fail.is_some()).unwrap_or(false)
    }

    /// Closes `qty` of the position in `market` at `price`: the `Close` event, its commission, and a `TightEnd` when this
    /// leaves the account flat in tight mode. Returns the events and whether the quick tally went over its limit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn do_close_event(
        &mut self,
        t: i64,
        market: u16,
        qty: u64,
        price: u64,
        j: usize,
        reason: CloseReason,
        intent_id: u32,
        allow_tight_end: bool,
    ) -> Result<(Vec<QEvent>, bool), EvalError> {
        let pos = self.positions.get(&market).copied().ok_or(EvalError::Invariant("close without a position"))?;
        let cfg = self.market_cfg(market).ok_or(EvalError::Invariant("close in an unknown market"))?;
        let long = pos.p.qty.is_positive();
        let out = engine::apply_fill(Some(pos.p), eside(!long), qty, price, t, engine::day_index(t))?;
        let slice = out.closed.ok_or(EvalError::Invariant("a closing fill has a slice"))?;
        engine::check_close(&engine::CloseClaim {
            long,
            qty,
            entry_e12: slice.entry_e12,
            exit_e12: price,
            time: t,
            timer_start: pos.p.timer_start,
            pnl: slice.pnl,
            quick: slice.quick,
            credited: slice.credit,
        })
        .map_err(|_| EvalError::Invariant("close claim"))?;
        let d = engine::apply_closed_slice(&self.st, &self.erules, slice.credit, out.quick_tally_delta)?;
        self.st = d.state;
        self.trades_closed = self.trades_closed.saturating_add(1);
        if slice.quick {
            self.stats.quick_trades = self.stats.quick_trades.saturating_add(1);
        }
        match out.position {
            Some(np) => {
                self.positions.insert(market, Pos { p: np, ..pos });
            }
            None => {
                self.positions.remove(&market);
            }
        }
        let ev = JournalEvent::Close {
            market,
            side: side_of(long),
            qty,
            entry_e12: slice.entry_e12,
            exit_e12: price,
            time: t,
            timer_start: pos.p.timer_start,
            pnl: slice.pnl,
            quick: slice.quick,
            credited: slice.credit,
            reason,
            intent_id,
            proof: NO_PROOF,
        };
        let mut evs = vec![self.qevent(ev, Some(j), t)];
        let over = d.fail.is_some();
        if !over {
            let fee = self.commission_micro(cfg.class, qty, price)?;
            if fee > 0 {
                self.debit(fee)?;
                let amount = u64::try_from(fee).map_err(|_| EvalError::Overflow)?;
                evs.push(self.qevent(JournalEvent::Fee { kind: FeeKind::Commission, market, amount, time: t }, None, t));
            }
            if allow_tight_end && self.positions.is_empty() && self.st.tight_on {
                self.st = engine::check_tight_end(&self.st).map_err(|_| EvalError::Invariant("tight end"))?;
                self.stats.tight_ends = self.stats.tight_ends.saturating_add(1);
                evs.push(self.qevent(JournalEvent::TightEnd { time: t }, None, t));
            }
        }
        Ok((evs, over))
    }

    /// One closing slice of the trader's (or a trigger's) own, with the rules of the quick limit and the static floor:
    /// if booking the slice would push the quick tally over its limit, every other position (and the rest of this one) is
    /// closed FIRST, then the slice, then `Fail{PhaseQuick}`: the program accepts nothing but the `Fail` after an overshoot.
    /// A slice that leaves the account flat at or below the static floor is followed by `Fail{StaticFloor}`.
    /// Returns whether the account failed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn close_checked(&mut self, t: i64, market: u16, qty: u64, price: u64, j: usize, reason: CloseReason, intent_id: u32) -> Result<bool, EvalError> {
        let pos = self.positions.get(&market).copied().ok_or(EvalError::Invariant("close without a position"))?;
        let prev = self.preview_close(&pos, qty, price, t)?;
        let would_over = self.would_exceed_quick(prev.0, prev.1);
        let mut units: Vec<Vec<QEvent>> = Vec::new();
        if would_over {
            units.extend(self.force_close_all(t, CloseReason::Fail, Some((market, qty)))?);
        }
        // after the forced closes the slice may no longer exceed (it always does: Q only grows) - book it
        let (evs, over) = self.do_close_event(t, market, qty, price, j, reason, intent_id, !would_over)?;
        units.push(evs);
        let tail = if over || would_over {
            Some(self.fail_event(t, j, journal_types::FailReason::PhaseQuick))
        } else if self.positions.is_empty() && self.st.b <= engine::static_floor_micro(&self.st)? {
            Some(self.fail_event(t, j, journal_types::FailReason::StaticFloor))
        } else {
            None
        };
        let failed = tail.is_some();
        self.finish_units(&mut units, tail);
        for u in units {
            self.push_unit(u);
        }
        Ok(failed)
    }

    /// Closes every position at the current marks with `reason` (intent 0): the ones whose slice adds least to the quick
    /// tally first, so that a slice that crosses the limit is the last one. `keep` names a position that stays open for
    /// `qty` units (the slice the caller is about to book). Returns one unit per close (a close with its commission).
    pub(crate) fn force_close_all(&mut self, t: i64, reason: CloseReason, keep: Option<(u16, u64)>) -> Result<Vec<Vec<QEvent>>, EvalError> {
        let mut todo: Vec<(i64, u16, u64, u64, usize)> = Vec::new();
        let list: Vec<(u16, Pos)> = self.positions.iter().map(|(k, v)| (*k, *v)).collect();
        for (m, p) in list {
            let held_qty = p.p.qty.unsigned_abs();
            let qty = match keep {
                Some((km, keep_qty)) if km == m => held_qty.saturating_sub(keep_qty),
                _ => held_qty,
            };
            if qty == 0 {
                continue;
            }
            let j = *self.marks.get(&m).ok_or(EvalError::Invariant("a position without a mark"))?;
            let s = *self.samples.get(j).ok_or(EvalError::Invariant("sample index"))?;
            let price = if p.p.qty.is_positive() { s.bid } else { s.ask };
            let (_, tally) = self.preview_close(&p, qty, price, t)?;
            todo.push((tally, m, qty, price, j));
        }
        todo.sort_by_key(|x| (x.0, x.1));
        let n = todo.len();
        let mut units = Vec::new();
        for (i, (_, m, qty, price, j)) in todo.into_iter().enumerate() {
            let (evs, over) = self.do_close_event(t, m, qty, price, j, reason, 0, false)?;
            if over && i + 1 < n {
                self.stats.unpostable = self.stats.unpostable.saturating_add(1);
            }
            units.push(evs);
        }
        Ok(units)
    }
}

/// Whether an order of `it` would open or add exposure against the position `pos` of its market: no position, a position of
/// the same side, or more than the opposite position holds (a flip's opening half).
fn would_open(pos: Option<&Pos>, it: &OrderIntent) -> bool {
    match pos {
        None => true,
        Some(p) => (p.p.qty.is_positive()) == (it.side == 0) || it.qty > p.p.qty.unsigned_abs(),
    }
}

#[cfg(test)]
mod unpriced_tests {
    //! Decision 64 at fill time. Through `on_order` an order that opens in an unpriced market never gets this far (it is refused at
    //! arrival), so the fill-time check is exercised by calling `exec_trade` on an evaluator that holds a position in an
    //! unpriced market (state an honest account cannot reach): the opening part is refused, a pure reduce is not.
    use super::*;
    use engine::Position;

    const ONE: u64 = 1_000_000_000_000;
    const T: i64 = 1_700_000_100;

    fn ev(half_spread_e6: u32) -> Evaluator {
        ev_with(half_spread_e6, |_| {})
    }

    fn ev_with(half_spread_e6: u32, edit: impl FnOnce(&mut MarketCfg)) -> Evaluator {
        let mut market = MarketCfg {
            id: 0,
            class: 0,
            source: 2,
            feed_id: [7; 32],
            max_age_secs: 600,
            status: 1,
            qty_step: 1_000,
            qty_min: 1_000,
            max_notional_micro: 1_000_000_000_000_000,
            sessions: vec![(0, 10_080)],
            half_spread_e6,
        };
        edit(&mut market);
        let rules = RulesCfg { ruleset_version: 1, leverage_x: [50, 20, 20, 20, 10, 5, 5, 2], holding_fee_e6: [100, 200, 400, 200, 400, 400, 1000, 1600], commission_e6: [0; 8], markets: vec![market] };
        let acct = AccountParams { account: [1; 32], trader: [2; 32], challenge_id: 0, size_micro: 100_000_000_000, phase: 2, tier_index: 8, opened_time: T - 3_600 };
        let mut ev = Evaluator::new(rules, acct, Box::new(crate::eval::AcceptAll)).unwrap();
        ev.on_sample_mid(0, T, ONE).unwrap();
        ev.tick(T + 1);
        ev
    }

    fn long_position(qty: i64) -> Pos {
        Pos { p: Position { qty, avg_entry_e12: ONE, timer_start: T - 600, opened_at: T - 600, fee_day: engine::day_index(T) }, sl: 0, tp: 0 }
    }

    fn intent(side: u8, qty: u64) -> OrderIntent {
        OrderIntent { kind: IntentKind::Market, market: 0, side, qty, price_e12: 0, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: T }
    }

    fn run(ev: &mut Evaluator, it: &OrderIntent, kind: OrderKind) -> Reject {
        ev.exec_trade(9, it, 0, kind, T + 1, ONE).expect("no internal error")
    }

    #[test]
    fn would_open_is_true_for_no_position_the_same_side_and_the_part_beyond_the_opposite_position() {
        let long = long_position(10_000);
        assert!(would_open(None, &intent(0, 1_000)));
        assert!(would_open(None, &intent(1, 1_000)));
        assert!(would_open(Some(&long), &intent(0, 1_000)), "an add");
        assert!(!would_open(Some(&long), &intent(1, 1_000)), "a reduce");
        assert!(!would_open(Some(&long), &intent(1, 10_000)), "exactly the position closes it");
        assert!(would_open(Some(&long), &intent(1, 10_001)), "a flip opens the remainder");
    }

    #[test]
    fn an_opening_fill_of_a_market_or_a_triggered_pending_order_is_refused_when_the_market_is_unpriced() {
        for kind in [OrderKind::Market, OrderKind::Limit, OrderKind::Stop] {
            let mut e = ev(0);
            assert_eq!(run(&mut e, &intent(0, 10_000), kind), Err(OrderReject::MarketUnpriced), "{kind:?} open");
            // an add and the opening half of a flip, with a position that cannot exist for an honest account
            let mut e = ev(0);
            e.positions.insert(0, long_position(10_000));
            assert_eq!(run(&mut e, &intent(0, 1_000), kind), Err(OrderReject::MarketUnpriced), "{kind:?} add");
            let mut e = ev(0);
            e.positions.insert(0, long_position(10_000));
            assert_eq!(run(&mut e, &intent(1, 12_000), kind), Err(OrderReject::MarketUnpriced), "{kind:?} flip");
            // nothing changed
            assert_eq!(e.positions.get(&0).map(|p| p.p.qty), Some(10_000));
            assert!(e.queue.is_empty());
        }
        // the same orders in a priced market are not refused for it
        let mut e = ev(1);
        assert_eq!(run(&mut e, &intent(0, 10_000), OrderKind::Market), Ok(()));
    }

    #[test]
    fn a_reduce_or_a_close_in_an_unpriced_market_is_never_refused() {
        let mut e = ev(0);
        e.positions.insert(0, long_position(10_000));
        assert_eq!(run(&mut e, &intent(1, 4_000), OrderKind::Market), Ok(()), "a partial reduce");
        assert_eq!(e.positions.get(&0).map(|p| p.p.qty), Some(6_000));
        assert_eq!(run(&mut e, &intent(1, 6_000), OrderKind::Limit), Ok(()), "a pending order that closes the rest");
        assert!(!e.positions.contains_key(&0));
    }

    #[test]
    fn the_unpriced_refusal_comes_after_hold_lock_and_status_and_before_the_flip_remainder() {
        // held first
        let mut e = ev(0);
        e.set_held(true);
        assert_eq!(run(&mut e, &intent(0, 10_000), OrderKind::Market), Err(OrderReject::AccountHeld));
        // status next
        let mut e = ev_with(0, |m| m.status = STATUS_REDUCE_ONLY);
        assert_eq!(run(&mut e, &intent(0, 10_000), OrderKind::Market), Err(OrderReject::MarketNotTradable));
        // before the remainder of a flip: a flip whose remainder (1,000 units) is under the minimum (1,000,000) is `InvalidRemainder` in a priced
        // market and `MarketUnpriced` in an unpriced one
        let mut e = ev_with(1, |m| m.qty_min = 1_000_000);
        e.positions.insert(0, long_position(10_000));
        assert_eq!(run(&mut e, &intent(1, 11_000), OrderKind::Limit), Err(OrderReject::InvalidRemainder));
        let mut e = ev_with(0, |m| m.qty_min = 1_000_000);
        e.positions.insert(0, long_position(10_000));
        assert_eq!(run(&mut e, &intent(1, 11_000), OrderKind::Limit), Err(OrderReject::MarketUnpriced));
    }
}
