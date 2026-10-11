mod common;
use common::*;
use setl8_verify::{FindingClass, Verdict};

#[test]
fn generator_reproduces_design_4_9() {
    let b = design_4_9();
    let evs = b.events();
    use journal_types::JournalEvent as E;
    assert!(matches!(evs[0], E::Fill { price_e12: 1_080_032_400_000, margin: 21_600_648_000, .. }));
    assert!(matches!(evs[2], E::TightStart { p_open: 1_235_161_000, r: 100_000_000_000, l_start: 96_000_000_000, g_peak: 1_235_161_000, l_level: 96_235_161_000, .. }));
    let raised = evs.iter().find_map(|e| if let E::FloorRaised { g_peak, l_level, .. } = e { Some((*g_peak, *l_level)) } else { None }).unwrap();
    assert_eq!(raised, (3_035_107_000, 98_035_107_000));
    let close = evs.iter().find_map(|e| if let E::Close { pnl, exit_e12, reason, .. } = e { Some((*pnl, *exit_e12, *reason)) } else { None }).unwrap();
    assert_eq!((close.0, close.1), (-2_064_740_000, 1_077_967_660_000));
    assert_eq!(b.final_state.b, 97_935_260_000);
    assert_eq!(b.final_state.lock_until, T_1400 + 150 + 12 * 3600);
}

#[test]
fn design_4_9_verifies_valid_with_the_design_numbers() {
    let b = design_4_9();
    let r = verify(&b);
    assert_eq!(r.classes(), Vec::<FindingClass>::new(), "{:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Valid);
    assert_eq!(r.state.b, 97_935_260_000);
    assert_eq!((r.state.q_tally, r.state.tight_on, r.state.flat, r.state.trades_closed), (0, false, true, 1));
    assert_eq!(r.state.lock_until, T_1400 + 150 + 43_200);
}
