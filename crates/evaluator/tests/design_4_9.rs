//! DESIGN 4.9 end to end: the evaluator produces the three-step story of the worked example and the public verifier agrees.
mod common;
use common::*;
use journal_types::{CloseReason, JournalEvent};

#[test]
fn design_4_9_numbers_and_verifier_verdict() {
    let mut w = World::base();
    let mid = |p: u64| eur(p);
    w.sample(0, T_1400, mid(10_800));
    w.order(market_order(0, true, 1_000_000_000_000, T_1400)).expect("order");
    for (dt, p) in [(30, 10_806), (60, 10_813), (90, 10_831), (120, 10_811), (150, 10_780)] {
        w.sample(0, T_1400 + dt, mid(p));
    }
    w.ev.tick(T_1400 + 151);
    let batches = w.ev.drain_batches(&evaluator::BatchPolicy::MAX.with_heartbeat(true));
    let evs = events_of(&batches);
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            JournalEvent::Fill { .. } => "Fill",
            JournalEvent::Close { .. } => "Close",
            JournalEvent::Fee { .. } => "Fee",
            JournalEvent::TightStart { .. } => "TightStart",
            JournalEvent::FloorRaised { .. } => "FloorRaised",
            JournalEvent::TightEnd { .. } => "TightEnd",
            JournalEvent::Breach { .. } => "Breach",
            JournalEvent::Fail { .. } => "Fail",
            JournalEvent::Meta { .. } => "Meta",
        })
        .collect();
    println!("{kinds:?}");
    // the Fill: ask 1.0800324
    assert!(matches!(evs[0], JournalEvent::Fill { price_e12: 1_080_032_400_000, avg_entry_after: 1_080_032_400_000, margin: 21_600_648_000, .. }));
    let ts = evs.iter().find_map(|e| if let JournalEvent::TightStart { p_open, r, l_start, g_peak, l_level, .. } = e { Some((*p_open, *r, *l_start, *g_peak, *l_level)) } else { None }).expect("tight start");
    assert_eq!(ts, (1_235_161_000, 100_000_000_000, 96_000_000_000, 1_235_161_000, 96_235_161_000));
    let fr = evs.iter().find_map(|e| if let JournalEvent::FloorRaised { g_peak, l_level, .. } = e { Some((*g_peak, *l_level)) } else { None }).expect("raise");
    assert_eq!(fr, (3_035_107_000, 98_035_107_000));
    let close = evs.iter().find_map(|e| if let JournalEvent::Close { exit_e12, pnl, quick, credited, reason, .. } = e { Some((*exit_e12, *pnl, *quick, *credited, *reason)) } else { None }).expect("close");
    assert_eq!(close, (1_077_967_660_000, -2_064_740_000, false, -2_064_740_000, CloseReason::Breach));
    assert!(evs.iter().any(|e| matches!(e, JournalEvent::Breach { l_level: 98_035_107_000, b_after: 97_935_260_000, .. })));
    let rep = verify(&w, &batches);
    assert!(rep.verdict == setl8_verify::Verdict::Valid, "{}", describe(&rep));
}
