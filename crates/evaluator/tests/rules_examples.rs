//! The worked examples of the DESIGN, with their exact numbers, run end to end through the evaluator (and the public
//! verifier): 2.6 R1 to R3, 7.5 netting, 9.3 holding fee, decision 31.
mod common;
use common::*;
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent};

fn drain_all(w: &mut World) -> Vec<evaluator::Batch> {
    w.ev.drain_batches(&evaluator::BatchPolicy::MAX.with_heartbeat(true))
}

/// DESIGN 2.6 R1: S = $100,000, spread 0, 1,000,000 EUR bought at 1.0000.
#[test]
fn r1_tight_floor_example() {
    let mut w = fx_world(0);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    for (dt, p) in [(30, 10_006), (60, 10_012), (90, 10_030), (120, 10_010), (150, 9_980)] {
        w.sample(0, T_1400 + dt, p4(p));
    }
    w.ev.tick(T_1400 + 151);
    let batches = drain_all(&mut w);
    let evs = events_of(&batches);
    // t1: P = +0.6k, nothing. t2: tight mode on, R = 100k, L_start = 96k, G = 1.2k, L = 96k + 0.2k
    let ts: Vec<_> = evs.iter().filter_map(|e| if let JournalEvent::TightStart { time, p_open, r, l_start, g_peak, l_level, .. } = e { Some((*time, *p_open, *r, *l_start, *g_peak, *l_level)) } else { None }).collect();
    assert_eq!(ts, vec![(T_1400 + 60, 1_200_000_000, 100_000_000_000, 96_000_000_000, 1_200_000_000, 96_200_000_000)]);
    // t3: G = 3.0k, L = 96k + (3.0k - 1k) = 98.0k
    let fr: Vec<_> = evs.iter().filter_map(|e| if let JournalEvent::FloorRaised { time, g_peak, l_level, .. } = e { Some((*time, *g_peak, *l_level)) } else { None }).collect();
    assert_eq!(fr, vec![(T_1400 + 90, 3_000_000_000, 98_000_000_000)]);
    // t4: P = +1.0k: below 1% but L stays 98.0k (no event). t5: E = 98.0k <= L: breach, B = 98.0k, locked 12 h
    let close = evs.iter().find_map(|e| if let JournalEvent::Close { pnl, credited, quick, reason, .. } = e { Some((*pnl, *credited, *quick, *reason)) } else { None }).unwrap();
    assert_eq!(close, (-2_000_000_000, -2_000_000_000, false, CloseReason::Breach));
    let breach = evs.iter().find_map(|e| if let JournalEvent::Breach { time, l_level, b_after, .. } = e { Some((*time, *l_level, *b_after)) } else { None }).unwrap();
    assert_eq!(breach, (T_1400 + 150, 98_000_000_000, 98_000_000_000));
    let st = w.ev.state();
    assert_eq!((st.b, st.tight_on, st.flat, st.lock_until), (98_000_000_000, false, true, T_1400 + 150 + 43_200));
    assert_eq!(w.ev.stats().breaches, 1);
    assert_valid(&w, &batches);
}

/// DESIGN 2.7 R2: the static floor moves to 0.96 S for good once B >= 1.04 S.
#[test]
fn r2_permanent_shift_of_the_static_floor() {
    let mut w = fx_world(0);
    let mut t = T_1400;
    // a normal slice of +4,000 brings B to exactly 104,000: the latch sets at equality
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 100;
    w.sample(0, t, p4(10_040));
    w.order(close_order(0, 0, t)).unwrap();
    t += 10;
    w.sample(0, t, p4(10_040));
    w.ev.tick(t + 1);
    assert_eq!((w.ev.state().b, w.ev.state().latch), (104_000_000_000, true));
    t += 1;
    // a loss of 3,000 brings B back to 101,000; the latch stays
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 100;
    w.sample(0, t, p4(10_010));
    w.order(close_order(0, 0, t)).unwrap();
    t += 10;
    w.sample(0, t, p4(10_010));
    w.ev.tick(t + 1);
    assert_eq!((w.ev.state().b, w.ev.state().latch), (101_000_000_000, true));
    t += 1;
    // E = 101,000 - 5,000 = 96,000 touches the latched floor (0.96 S): the account fails
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 20;
    w.sample(0, t, p4(10_010));
    t += 20;
    w.sample(0, t, p4(10_010 - 50));
    w.ev.tick(t + 1);
    let batches = drain_all(&mut w);
    let evs = events_of(&batches);
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Fail { reason: FailReason::StaticFloor, b_after: 96_000_000_000, .. })));
    assert!(matches!(w.ev.status(), evaluator::Status::Failed(engine::FailReason::StaticFloor)));
    assert_valid(&w, &batches);
}

/// DESIGN 2.8 R3: quick credit, quick loss, and the quick-tally limit 0.028 S (strictly greater fails).
#[test]
fn r3_quick_credit_and_limit() {
    let mut w = fx_world(0);
    let mut t = T_1400;
    // +1,000 raw within 30 s: B += 500, Q += 500
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 30;
    w.sample(0, t, p4(10_010));
    w.order(close_order(0, 0, t)).unwrap();
    t += 5;
    w.sample(0, t, p4(10_010));
    w.ev.tick(t + 1);
    assert_eq!((w.ev.state().b, w.ev.state().q_tally), (100_500_000_000, 500_000_000));
    t += 1;
    // -400 in 20 s: credited in full, Q unchanged
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 20;
    w.sample(0, t, p4(10_010 - 4));
    w.order(close_order(0, 0, t)).unwrap();
    t += 5;
    w.sample(0, t, p4(10_006));
    w.ev.tick(t + 1);
    assert_eq!((w.ev.state().b, w.ev.state().q_tally), (100_100_000_000, 500_000_000));
    t += 1;
    // +4,600 raw quick: Q = 500 + 2,300 = 2,800 exactly: allowed (strictly greater fails)
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 30;
    w.sample(0, t, p4(10_006 + 46));
    w.order(close_order(0, 0, t)).unwrap();
    t += 5;
    w.sample(0, t, p4(10_006 + 46));
    w.ev.tick(t + 1);
    assert_eq!(w.ev.state().q_tally, 2_800_000_000);
    assert!(matches!(w.ev.status(), evaluator::Status::Active));
    t += 1;
    // +2 micro-USD raw quick: credited 1, Q = 2,800.000001 > limit: the phase fails
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 30;
    w.sample(0, t, (10_006 + 46) * 100_000_000 + 2);
    w.order(close_order(0, 0, t)).unwrap();
    t += 5;
    w.sample(0, t, (10_006 + 46) * 100_000_000 + 2);
    w.ev.tick(t + 1);
    assert!(matches!(w.ev.status(), evaluator::Status::Failed(engine::FailReason::PhaseQuick)));
    let batches = drain_all(&mut w);
    assert!(events_of(&batches).iter().any(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })));
    assert_valid(&w, &batches);
}

/// DESIGN 7.5 merge: long 3 @ 1.10 then buy 4 @ 1.13 (units of 10,000 EUR): the average entry rounds UP for a long, and the
/// timer restarts at the add.
#[test]
fn merge_example_of_7_5() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, 1_100_000_000_000);
    w.order(market_order(0, true, 30_000_000_000, t)).unwrap();
    w.sample(0, t + 100, 1_130_000_000_000);
    w.order(market_order(0, true, 40_000_000_000, t + 100)).unwrap();
    w.sample(0, t + 110, 1_130_000_000_000);
    // (3e11 x 1.10 + 4e11 x 1.13) / 7e11 = 1.117142857142857...: a long rounds up
    let p = w.ev.positions()[&0].p;
    assert_eq!((p.qty, p.avg_entry_e12, p.timer_start, p.opened_at), (70_000_000_000, 1_117_142_857_143, t + 100, t));
    // a short rounds the average DOWN
    let mut w = fx_world(0);
    w.sample(0, t, 1_100_000_000_000);
    w.order(market_order(0, false, 30_000_000_000, t)).unwrap();
    w.sample(0, t + 100, 1_130_000_000_000);
    w.order(market_order(0, false, 40_000_000_000, t + 100)).unwrap();
    w.sample(0, t + 110, 1_130_000_000_000);
    let p = w.ev.positions()[&0].p;
    assert_eq!((p.qty, p.avg_entry_e12), (-70_000_000_000, 1_117_142_857_142));
    w.ev.tick(t + 111);
    let batches = drain_all(&mut w);
    assert_valid(&w, &batches);
}

#[test]
fn partial_reduce_pnl_is_857_142857() {
    let mut w = fx_world(0);
    let t = T_1400;
    // build the position of the example in one go: 700,000 EUR at an average of 1.117142857143 is the merge above; here the
    // same pnl is checked on a position bought at 1.117142857143 directly
    w.sample(0, t, 1_117_142_857_143);
    w.order(market_order(0, true, 700_000_000_000, t)).unwrap();
    w.sample(0, t + 100, 1_120_000_000_000);
    w.order(market_order(0, false, 300_000_000_000, t + 100)).unwrap();
    w.ev.tick(t + 101);
    let batches = drain_all(&mut w);
    let evs = events_of(&batches);
    let c = close_events(&evs);
    match c[0] {
        JournalEvent::Close { qty, pnl, quick, reason, .. } => {
            assert_eq!((*qty, *pnl, *quick, *reason), (300_000_000_000, 857_142_857, false, CloseReason::Manual));
        }
        _ => unreachable!(),
    }
    let p = w.ev.positions()[&0].p;
    assert_eq!((p.qty, p.avg_entry_e12, p.timer_start), (400_000_000_000, 1_117_142_857_143, t));
    assert_valid(&w, &batches);
}

#[test]
fn flip_example_and_its_quick_variant() {
    for (age, quick, credit, tally) in [(300i64, false, 1_967_600_000i64, 0i64), (30, true, 983_800_000, 983_800_000)] {
        let mut w = fx_world(0);
        let t = T_1400;
        w.sample(0, t, 1_080_032_400_000);
        w.order(market_order(0, true, 1_000_000_000_000, t)).unwrap();
        w.sample(0, t + age, 1_082_000_000_000);
        w.order(market_order(0, false, 1_500_000_000_000, t + age)).unwrap();
        w.ev.tick(t + age + 1);
        let batches = drain_all(&mut w);
        let evs = events_of(&batches);
        let close = evs.iter().find_map(|e| if let JournalEvent::Close { pnl, quick, credited, reason, .. } = e { Some((*pnl, *quick, *credited, *reason)) } else { None }).unwrap();
        assert_eq!(close, (1_967_600_000, quick, credit, CloseReason::Flip));
        // the remainder: short 500,000 EUR at 1.0820000, fresh timer, margin 500,000 x 1.082 / 50 = $10,820
        let fill = evs.iter().filter_map(|e| if let JournalEvent::Fill { side, qty, price_e12, margin, avg_entry_after, .. } = e { Some((*side, *qty, *price_e12, *margin, *avg_entry_after)) } else { None }).next_back().unwrap();
        assert_eq!(fill, (journal_types::Side::Short, 500_000_000_000, 1_082_000_000_000, 10_820_000_000, 1_082_000_000_000));
        let p = w.ev.positions()[&0].p;
        assert_eq!((p.qty, p.timer_start), (-500_000_000_000, t + age));
        assert_eq!(w.ev.state().q_tally, tally);
        assert_eq!(w.ev.state().b, 100_000_000_000 + credit);
        assert_eq!(w.ev.stats().flips, 1);
        assert_valid(&w, &batches);
    }
}

/// DESIGN 9.3: 0.01% per night on the notional at the mark, charged at 22:00 UTC; a weekend is three nights.
#[test]
fn holding_fee_example_and_a_weekend() {
    let mut w = fx_world(0);
    let t_open = T_1400 + 7 * HOUR; // 21:00 on Monday 2026-10-12
    let r = T_1400 + 8 * HOUR; // 22:00:00
    w.sample(0, t_open, p4(10_800));
    w.order(market_order(0, true, MEUR, t_open)).unwrap();
    w.sample(0, r - 600, p4(10_805));
    w.sample(0, r - 1, p4(10_805));
    w.sample(0, r + 5, p4(10_812));
    w.ev.tick(r + 6);
    let batches = drain_all(&mut w);
    let fees: Vec<_> = events_of(&batches).iter().filter_map(|e| if let JournalEvent::Fee { kind: FeeKind::Holding, amount, time, market } = e { Some((*market, *amount, *time)) } else { None }).collect();
    // the mark is the LAST sample at or before 22:00:00 (1.0805): 1,000,000 EUR x 1.0805 x 0.0001 = $108.05
    assert_eq!(fees, vec![(0, 108_050_000, r)]);
    assert_eq!(w.ev.state().b, 100_000_000_000 - 108_050_000);
    assert_valid(&w, &batches);
    // the same position held through Friday, Saturday and Sunday nights: three fees
    let mut w = fx_world(0);
    let friday_2100 = T_1400 + 4 * 86_400 + 7 * HOUR;
    w.sample(0, friday_2100, p4(10_800));
    w.order(market_order(0, true, MEUR, friday_2100)).unwrap();
    for k in 1..=60 {
        w.sample(0, friday_2100 + k * 3_000, p4(10_800 + (k as u64 % 7)));
    }
    w.ev.tick(friday_2100 + 61 * 3_000);
    let batches = drain_all(&mut w);
    let nights = events_of(&batches).iter().filter(|e| matches!(e, JournalEvent::Fee { kind: FeeKind::Holding, .. })).count();
    assert_eq!(nights, 3, "60 steps of 50 minutes after Friday 21:00 end Sunday 23:00: Friday, Saturday and Sunday nights");
    assert_valid(&w, &batches);
}

/// Decision 31: L_start is capped at the closed balance. B = $93,000, F_s = $92,000, +$1,100 open: L = $93.1K, not $96K.
#[test]
fn decision_31_tight_floor_is_capped_at_the_closed_balance() {
    let mut w = fx_world(0);
    let mut t = T_1400;
    // bring B down to $93,000 with a -7,000 slice (a normal one)
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 100;
    w.sample(0, t, p4(9_930));
    w.order(close_order(0, 0, t)).unwrap();
    t += 10;
    w.sample(0, t, p4(9_930));
    w.ev.tick(t + 1);
    assert_eq!(w.ev.state().b, 93_000_000_000);
    t += 1;
    w.order(market_order(0, true, MEUR, t)).unwrap();
    t += 10;
    w.sample(0, t, p4(9_930));
    t += 10;
    w.sample(0, t, p4(9_930 + 11));
    w.ev.tick(t + 1);
    let st = w.ev.state();
    assert!(st.tight_on);
    assert_eq!((st.r, st.l_start, st.g_peak, st.l_level), (93_000_000_000, 93_000_000_000, 1_100_000_000, 93_100_000_000));
    let batches = drain_all(&mut w);
    assert_valid(&w, &batches);
}
