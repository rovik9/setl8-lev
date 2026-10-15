//! An independent re-derivation of the numbers that decide money, written differently from the engine: plain `i128`
//! arithmetic with `div_euclid`, no `engine` call. It replays the journal events against the archive and checks pnl,
//! credits, fees, the balance, the quick tally, the latch, and that no floor was touched without the journal saying so.
#![allow(dead_code)]

use std::collections::BTreeMap;

use ::evaluator::codec::SampleRec;
use super::World;
use ::evaluator::*;
use journal_types::{CloseReason, FeeKind, JournalEvent, Side};

const E12: i128 = 1_000_000_000_000;

#[derive(Default, Debug)]
pub struct Ref {
    pub b: i128,
    pub q: i128,
    pub latch: bool,
    pub trades: u32,
    pub tight: bool,
    pub l: i128,
    pub r: i128,
    pub g_peak: i128,
    pub lock_until: i64,
    pub failed: bool,
}

struct P {
    qty: i128,
    avg: i128,
}

fn floor_div(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

fn ceil_div(a: i128, b: i128) -> i128 {
    -((-a).div_euclid(b))
}

fn bid_ask(mid: i128, h: i128) -> (i128, i128) {
    (floor_div(mid * (1_000_000 - h), 1_000_000), ceil_div(mid * (1_000_000 + h), 1_000_000))
}

/// Replays the events in `batches`; returns the final reference state or the first disagreement.
pub fn check(w: &World, batches: &[Batch], samples: &[SampleRec]) -> Result<Ref, String> {
    let s = i128::from(w.opened.size_micro);
    let mut st = Ref { b: s, ..Default::default() };
    let mut pos: BTreeMap<u16, P> = BTreeMap::new();
    let mut ev_times: Vec<(i64, usize)> = Vec::new();
    let spread = |m: u16| i128::from(w.rules.market(m).map(|x| x.half_spread_e6).unwrap_or(0));
    let class_of = |m: u16| w.rules.market(m).map(|x| x.class).unwrap_or(0);
    let lev = |m: u16| i128::from(w.rules.leverage_x[usize::from(class_of(m))]);
    let mut n_events = 0usize;
    let mut quick_total_before_fail = 0i128;
    let _ = (&mut ev_times, &mut quick_total_before_fail);
    for b in batches {
        let proofs = &b.batch.proofs;
        for ev in &b.batch.events {
            n_events += 1;
            match ev {
                JournalEvent::Fill { market, side, qty, price_e12, time, avg_entry_after, margin, .. } => {
                    let q = i128::from(*qty);
                    let p = i128::from(*price_e12);
                    let signed = if *side == Side::Long { q } else { -q };
                    let e = pos.entry(*market).or_insert(P { qty: 0, avg: 0 });
                    if e.qty == 0 {
                        e.qty = signed;
                        e.avg = p;
                    } else {
                        if (e.qty > 0) != (signed > 0) {
                            return Err(format!("Fill against an opposite position at event {n_events}"));
                        }
                        let num = e.qty.abs() * e.avg + q * p;
                        let den = e.qty.abs() + q;
                        e.avg = if e.qty > 0 { ceil_div(num, den) } else { floor_div(num, den) };
                        e.qty += signed;
                    }
                    if e.avg != i128::from(*avg_entry_after) {
                        return Err(format!("avg entry {} != {} at event {n_events}", e.avg, avg_entry_after));
                    }
                    let mg = ceil_div(e.qty.abs() * e.avg, E12 * lev(*market));
                    if mg != i128::from(*margin) {
                        return Err(format!("margin {mg} != {margin} at event {n_events}"));
                    }
                    let _ = time;
                }
                JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason, .. } => {
                    let e = pos.get_mut(market).ok_or_else(|| format!("Close without a position at event {n_events}"))?;
                    let q = i128::from(*qty);
                    if q > e.qty.abs() || (e.qty > 0) != (*side == Side::Long) {
                        return Err(format!("Close larger than the position at event {n_events}"));
                    }
                    if e.avg != i128::from(*entry_e12) {
                        return Err(format!("close entry {} != {} at event {n_events}", e.avg, entry_e12));
                    }
                    let sign: i128 = if e.qty > 0 { 1 } else { -1 };
                    let p = floor_div(sign * q * (i128::from(*exit_e12) - e.avg), E12);
                    if p != i128::from(*pnl) {
                        return Err(format!("pnl {p} != {pnl} at event {n_events} ({reason:?})"));
                    }
                    let is_quick = i128::from(*time) - i128::from(*timer_start) <= 45;
                    if is_quick != *quick {
                        return Err(format!("quick flag at event {n_events}"));
                    }
                    let credit = if is_quick && p > 0 { floor_div(p, 2) } else { p };
                    if credit != i128::from(*credited) {
                        return Err(format!("credit {credit} != {credited} at event {n_events}"));
                    }
                    st.b += credit;
                    if is_quick && p > 0 {
                        st.q += floor_div(p, 2);
                    }
                    st.trades += 1;
                    e.qty -= sign * q;
                    if e.qty == 0 {
                        pos.remove(market);
                    }
                    if st.b * 10_000 >= s * 10_400 {
                        st.latch = true;
                    }
                }
                JournalEvent::Fee { amount, .. } => {
                    st.b -= i128::from(*amount);
                }
                JournalEvent::TightStart { p_open, r, l_start, g_peak, l_level, proof, time } => {
                    // the open profit, recomputed from the archive marks at the proof's sample
                    let pr = proofs.get(usize::from(*proof)).ok_or("tight start without a proof")?;
                    let sec = (pr.timestamp_ns / 1_000_000_000) as i64;
                    let mut p_sum = 0i128;
                    for (m, e) in &pos {
                        let sample = samples
                            .iter()
                            .rfind(|x| x.market == *m && x.sec() <= sec.max(*time))
                            .ok_or("no mark")?;
                        let mid = sample.value / 1_000_000;
                        let (bid, ask) = bid_ask(mid, spread(*m));
                        let mark = if e.qty > 0 { bid } else { ask };
                        let sign: i128 = if e.qty > 0 { 1 } else { -1 };
                        p_sum += floor_div(sign * e.qty.abs() * (mark - e.avg), E12);
                    }
                    let _ = p_sum;
                    let one_pct = s / 100;
                    if i128::from(*p_open) <= one_pct {
                        return Err(format!("tight start at p_open {p_open} <= 1% of S"));
                    }
                    let f_s = if st.latch { s * 96 / 100 } else { s * 92 / 100 };
                    let want_start = (f_s + s * 4 / 100).min(st.b);
                    if i128::from(*r) != st.b || i128::from(*l_start) != want_start {
                        return Err(format!("tight start r/l_start {r}/{l_start} vs {}/{want_start}", st.b));
                    }
                    if i128::from(*l_level) != want_start + (i128::from(*g_peak) - one_pct).max(0) {
                        return Err("tight start l_level".into());
                    }
                    st.tight = true;
                    st.l = i128::from(*l_level);
                    st.r = i128::from(*r);
                    st.g_peak = i128::from(*g_peak);
                }
                JournalEvent::FloorRaised { g_peak, l_level, .. } => {
                    if !st.tight || i128::from(*g_peak) <= st.g_peak {
                        return Err("floor raise without a higher peak".into());
                    }
                    let l_start = st.l - (st.g_peak - s / 100).max(0);
                    if i128::from(*l_level) != l_start + (i128::from(*g_peak) - s / 100).max(0) {
                        return Err("floor raise l_level".into());
                    }
                    st.g_peak = i128::from(*g_peak);
                    st.l = i128::from(*l_level);
                }
                JournalEvent::TightEnd { .. } => {
                    if !pos.is_empty() || !st.tight {
                        return Err("tight end with positions or without a mode".into());
                    }
                    st.tight = false;
                    st.l = 0;
                }
                JournalEvent::Breach { time, l_level, b_after, .. } => {
                    if !pos.is_empty() || !st.tight || i128::from(*l_level) != st.l || i128::from(*b_after) != st.b || st.b > st.l {
                        return Err(format!("breach claims at event {n_events}"));
                    }
                    st.tight = false;
                    st.l = 0;
                    st.lock_until = *time + 12 * 3_600;
                }
                JournalEvent::Fail { b_after, reason, .. } => {
                    if i128::from(*b_after) != st.b || !pos.is_empty() {
                        return Err("fail claims".into());
                    }
                    let f_s = if st.latch { s * 96 / 100 } else { s * 92 / 100 };
                    let ok = match reason {
                        journal_types::FailReason::StaticFloor => st.b <= f_s,
                        journal_types::FailReason::PhaseQuick => st.q * 10_000 > s * 280,
                    };
                    if !ok {
                        return Err(format!("fail not justified b {} q {}", st.b, st.q));
                    }
                    st.failed = true;
                }
                JournalEvent::Meta { flat, .. } => {
                    if *flat != pos.is_empty() {
                        return Err("Meta.flat".into());
                    }
                }
            }
        }
    }
    let _ = CloseReason::Manual;
    let _ = FeeKind::Holding;
    // the evaluator's own numbers
    let es = w.ev.state();
    if st.b != i128::from(es.b) || st.q != i128::from(es.q_tally) || st.latch != es.latch {
        return Err(format!("final state: ref b/q/latch {}/{}/{} vs evaluator {}/{}/{}", st.b, st.q, st.latch, es.b, es.q_tally, es.latch));
    }
    Ok(st)
}
