//! Stage 2: archive replay of floors and triggers, the second recorder, pass verdicts.
mod common;
use common::*;
use journal_types::{CloseReason, JournalEvent as E};
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

#[test]
fn honest_long_path_verifies_valid_and_has_every_feature() {
    let b = honest_long();
    let evs = b.events();
    let has = |f: &dyn Fn(&E) -> bool| evs.iter().any(|e| f(e));
    assert!(has(&|e| matches!(e, E::TightStart { .. })), "tight start");
    assert!(has(&|e| matches!(e, E::FloorRaised { .. })), "raise");
    assert!(has(&|e| matches!(e, E::Breach { .. })), "breach");
    assert!(has(&|e| matches!(e, E::TightEnd { .. })), "tight end");
    assert!(has(&|e| matches!(e, E::Close { reason: CloseReason::StopLoss, .. })), "stop loss");
    assert!(has(&|e| matches!(e, E::Close { reason: CloseReason::TakeProfit, .. })), "take profit");
    assert!(has(&|e| matches!(e, E::Close { quick: true, .. })), "quick trade");
    assert!(has(&|e| matches!(e, E::Fee { kind: journal_types::FeeKind::Holding, .. })), "holding fee night");
    assert!(has(&|e| matches!(e, E::Fee { kind: journal_types::FeeKind::Commission, .. })), "commission");
    assert!(has(&|e| matches!(e, E::Fill { order_kind: journal_types::OrderKind::Limit, .. })), "limit fill");
    let r = verify(&b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Valid);
    assert!(b.samples.len() > 2_500);
}

fn only(r: &setl8_verify::Report, c: C) {
    assert_eq!(r.classes(), vec![c], "findings: {:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Mismatch);
}

fn valid(b: &Bundle) {
    let r = verify(b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Valid);
}

/// Sets `flat = false` in every Meta from line `from` on (a coherent liar: the position it hides stays open).
fn metas_not_flat_from(b: &mut Bundle, from: usize) {
    for i in from..b.lines.len() {
        if let GLine::Event { ev: E::Meta { flat, .. }, .. } = &mut b.lines[i] {
            *flat = false;
        }
    }
}

fn kinds(b: &Bundle) -> Vec<&'static str> {
    b.events()
        .iter()
        .map(|e| match e {
            E::Fill { .. } => "Fill",
            E::Close { .. } => "Close",
            E::Fee { .. } => "Fee",
            E::TightStart { .. } => "TightStart",
            E::FloorRaised { .. } => "FloorRaised",
            E::TightEnd { .. } => "TightEnd",
            E::Breach { .. } => "Breach",
            E::Fail { .. } => "Fail",
            E::Meta { .. } => "Meta",
        })
        .collect()
}

// ---------------------------------------------------------------- breach

#[test]
fn breach_then_recovery_is_valid_and_has_a_breach() {
    let b = breach_then_recovery();
    assert_eq!(kinds(&b), ["Fill", "Meta", "TightStart", "Meta", "FloorRaised", "Meta", "Close", "Breach", "Meta", "Meta"]);
    valid(&b);
}

#[test]
fn missed_breach_the_breach_is_removed_from_the_journal() {
    let mut b = breach_then_recovery();
    let close = b.find(|e| matches!(e, E::Close { .. }));
    let breach = b.find(|e| matches!(e, E::Breach { .. }));
    b.remove_lines(&[close, breach]);
    let first_meta_after = b.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut b, first_meta_after - 1);
    let r = verify_no_snapshot(&b);
    only(&r, C::MissedBreach);
    assert_eq!(r.findings.len(), 1, "a persisting miss is reported once");
}

#[test]
fn missed_breach_found_only_by_the_second_archive() {
    let honest = breach_then_recovery();
    let second = honest.archive_bytes();
    // the primary archive lost the breach sample, and the journal (a coherent liar) has no breach
    let mut a = honest.clone();
    let close = a.find(|e| matches!(e, E::Close { .. }));
    let breach = a.find(|e| matches!(e, E::Breach { .. }));
    a.remove_lines(&[close, breach]);
    let first_meta_after = a.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut a, first_meta_after - 1);
    let drop_idx = a.samples.iter().position(|s| s.ts_sec() == T_1400 + 150).unwrap();
    a.samples.remove(drop_idx);
    for m in &mut a.metas {
        if m.n_samples > drop_idx {
            m.n_samples -= 1;
        }
    }
    a.refresh_commitments();
    // alone, the primary archive explains everything
    let alone = verify_no_snapshot(&a);
    assert_eq!(alone.classes(), Vec::<C>::new(), "{:#?}", alone.findings);
    // with the second recorder the dropped sample shows
    let r = verify_second(&a, second);
    only(&r, C::MissedBreach);
    assert_eq!(r.findings[0].source, "second");
}

#[test]
fn false_breach_when_the_primary_archive_was_manipulated() {
    // the primary archive (and the journal built on it) has a price dip that the honest recorder never saw
    let quiet = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_805), eur(10_830)]);
    let manipulated = breach_then_recovery();
    let r = verify_second(&manipulated, quiet.archive_bytes());
    // P25b (D-F13): the manipulated journal's forced closes are not supported by the honest recorder's samples either
    assert_eq!(r.classes(), vec![C::FalseBreach, C::ArchiveDisagreement, C::ForcedCloseUnjustified], "{:#?}", r.findings);
    assert!(r.findings.iter().all(|f| f.source == "second"));
}

#[test]
fn false_tight_start_when_the_primary_archive_was_manipulated() {
    let quiet = eur_run(Spec::base(), &[eur(10_806), eur(10_801), eur(10_806), eur(10_806)]);
    let manipulated = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_806), eur(10_806)]);
    assert!(manipulated.events().iter().any(|e| matches!(e, E::TightStart { .. })));
    let r = verify_second(&manipulated, quiet.archive_bytes());
    assert_eq!(r.classes(), vec![C::FalseTightStart, C::ArchiveDisagreement], "{:#?}", r.findings);
}

#[test]
fn false_floor_raise_when_the_primary_archive_was_manipulated() {
    let quiet = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_812), eur(10_811), eur(10_811)]);
    let manipulated = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_811)]);
    assert!(manipulated.events().iter().any(|e| matches!(e, E::FloorRaised { .. })));
    let r = verify_second(&manipulated, quiet.archive_bytes());
    assert_eq!(r.classes(), vec![C::FalseFloorRaise, C::ArchiveDisagreement], "{:#?}", r.findings);
}

#[test]
fn archive_disagreement_alone_and_no_disagreement_for_identical_archives() {
    let b = breach_then_recovery();
    let r = verify_second(&b, b.archive_bytes());
    assert_eq!(r.classes(), Vec::<C>::new());
    // a second archive that differs by one e12 unit at a quiet sample
    let mut other = b.clone();
    other.samples[1].value += 1_000_000; // +1 e12 unit of the mid (30 s after the fill; nothing acts on it)
    let r = verify_second(&b, other.archive_bytes());
    only(&r, C::ArchiveDisagreement);
    // below the e12 floor (value + 1 of 1e-18) is no disagreement
    let mut tiny = b.clone();
    tiny.samples[1].value += 1;
    assert_eq!(verify_second(&b, tiny.archive_bytes()).classes(), Vec::<C>::new());
    // samples only the second archive has are not disagreements by themselves
    let mut extra = b.clone();
    let mut s = extra.samples[1];
    s.ts_ns += 5_000_000_000;
    extra.samples.insert(2, s);
    assert_eq!(verify_second(&b, extra.archive_bytes()).classes(), Vec::<C>::new());
}

// ---------------------------------------------------------------- tight mode and floor levels

#[test]
fn wrong_l_level_numbers_consistent_with_the_formulas_but_not_with_the_samples() {
    let mut b = breach_then_recovery();
    // g_peak and l_level one micro higher: check_floor_raised still passes, the samples say otherwise
    let i = b.find(|e| matches!(e, E::FloorRaised { .. }));
    if let E::FloorRaised { g_peak, l_level, .. } = b.ev_mut(i) {
        *g_peak += 1;
        *l_level += 1;
    }
    let j = b.find(|e| matches!(e, E::Breach { .. }));
    if let E::Breach { l_level, .. } = b.ev_mut(j) {
        *l_level += 1; // a coherent liar keeps the later claims consistent
    }
    only(&verify_no_snapshot(&b), C::WrongLLevel);
}

#[test]
fn missed_floor_raise() {
    let mut b = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_811)]);
    assert_eq!(kinds(&b)[..6], ["Fill", "Meta", "TightStart", "Meta", "FloorRaised", "Meta"]);
    valid(&b);
    let i = b.find(|e| matches!(e, E::FloorRaised { .. }));
    b.remove_lines(&[i]);
    only(&verify_no_snapshot(&b), C::MissedFloorRaise);
}

#[test]
fn tight_start_boundary_p_open_equals_one_percent() {
    // P == 0.01 S: no tight mode; P == 0.01 S + 1 micro: tight mode
    let at = eur_run(Spec::base(), &[mid_for_pnl(1_000_000_000), eur(10_800)]);
    assert!(!at.events().iter().any(|e| matches!(e, E::TightStart { .. })));
    valid(&at);
    let over = eur_run(Spec::base(), &[mid_for_pnl(1_000_000_001), eur(10_800)]);
    let ts = over.events().into_iter().find_map(|e| if let E::TightStart { p_open, .. } = e { Some(*p_open) } else { None }).unwrap();
    assert_eq!(ts, 1_000_000_001);
    valid(&over);
    // the verifier expects exactly that difference
    let mut dropped = over.clone();
    let i = dropped.find(|e| matches!(e, E::TightStart { .. }));
    dropped.remove_lines(&[i]);
    only(&verify_no_snapshot(&dropped), C::MissedTightStart);
    // and refuses a tight start at exactly 1 %
    let mut forged = at.clone();
    let proof = forged.lines.iter().find_map(|l| if let GLine::Event { proof: Some(p), .. } = l { Some(p.clone()) } else { None }).unwrap();
    let samp = forged.samples.iter().position(|s| s.mid_e12() == Some(mid_for_pnl(1_000_000_000))).unwrap();
    let p = proof_of(&forged.spec, &forged.samples[samp]);
    let t = forged.samples[samp].ts_sec();
    let at_meta = forged.metas[0].line;
    let ev = E::TightStart { time: t, p_open: 1_000_000_000, r: 100_000_000_000, l_start: 96_000_000_000, g_peak: 1_000_000_000, l_level: 96_000_000_000, proof: 0 };
    let _ = proof;
    forged.lines.insert(at_meta + 1, GLine::Event { seq: 0, ev, proof: Some(p) });
    forged.renumber();
    let r = verify_no_snapshot(&forged);
    assert!(r.classes().contains(&C::FalseTightStart), "{:#?}", r.findings);
}

#[test]
fn breach_boundary_equity_equals_the_tight_floor() {
    // after the raise L = 98,035.107000; E == L breaches, E == L + 1 micro does not
    let l = 98_035_107_000i64;
    let pnl_at_l = l - 100_000_000_000;
    let prefix = [eur(10_806), eur(10_813), eur(10_831), eur(10_811)];
    let mk = |pnl: i64| {
        let mut mids = prefix.to_vec();
        mids.push(mid_for_pnl(pnl));
        mids.push(eur(10_830));
        eur_run(Spec::base(), &mids)
    };
    let hit = mk(pnl_at_l);
    assert!(hit.events().iter().any(|e| matches!(e, E::Breach { .. })), "E == L is a breach");
    valid(&hit);
    let miss = mk(pnl_at_l + 1);
    assert!(!miss.events().iter().any(|e| matches!(e, E::Breach { .. })), "E == L + 1 is not");
    valid(&miss);
    // at the verifier level: the same journal without the breach is a MissedBreach exactly at E == L, and fine at L + 1
    let mut dropped = hit.clone();
    let c = dropped.find(|e| matches!(e, E::Close { .. }));
    let br = dropped.find(|e| matches!(e, E::Breach { .. }));
    dropped.remove_lines(&[c, br]);
    let idx = dropped.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut dropped, idx);
    only(&verify_no_snapshot(&dropped), C::MissedBreach);
}

#[test]
fn static_floor_boundary_equity_equals_the_floor() {
    // E == F_s (92,000) fails the account; E == F_s + 1 micro does not
    let mk = |pnl: i64| eur_run(Spec::base(), &[mid_for_pnl(pnl), eur(10_800)]);
    let hit = mk(-8_000_000_000);
    assert!(hit.events().iter().any(|e| matches!(e, E::Fail { .. })), "{:?}", kinds(&hit));
    valid(&hit);
    let miss = mk(-7_999_999_999);
    assert!(!miss.events().iter().any(|e| matches!(e, E::Fail { .. })));
    valid(&miss);
    let mut dropped = hit.clone();
    let c = dropped.find(|e| matches!(e, E::Close { .. }));
    let f = dropped.find(|e| matches!(e, E::Fail { .. }));
    dropped.remove_lines(&[c, f]);
    let idx = dropped.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut dropped, idx);
    only(&verify_no_snapshot(&dropped), C::MissedFail);
}

// ---------------------------------------------------------------- protective and pending orders

fn sl_case() -> Bundle {
    // 300,000 EUR bought at 14:00:00; a set_sl_tp at 14:00:10 puts the stop at 1.0750; the price falls through it at 14:01:00
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 300_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_795) },
        Script::Order { t: T_1400 + 10, intent: order(IntentKind::SetSlTp, 0, 0, 0, 0, eur(10_750), 0, 0, T_1400 + 10) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_770) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_745) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_740) },
        Script::Heartbeat { t: T_1400 + 90 },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn stop_loss_path_is_valid_and_has_a_stop_loss_close() {
    let b = sl_case();
    assert!(b.events().iter().any(|e| matches!(e, E::Close { reason: CloseReason::StopLoss, .. })));
    valid(&b);
}

#[test]
fn trigger_missed_stop_loss() {
    let mut b = sl_case();
    let c = b.find(|e| matches!(e, E::Close { reason: CloseReason::StopLoss, .. }));
    b.remove_lines(&[c]);
    let idx = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    // the Meta of the SL batch and the heartbeat now claim the position is still open
    let from = b.lines.iter().enumerate().skip(idx + 1).find(|(_, l)| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).map(|(i, _)| i).unwrap();
    metas_not_flat_from(&mut b, from);
    only(&verify_no_snapshot(&b), C::TriggerMissed);
}

#[test]
fn trigger_false_stop_loss_the_signed_level_is_lower() {
    let mut b = sl_case();
    let mut it = b.intents[1].intent;
    it.sl_e12 = eur(10_700);
    b.intents[1] = sign(2, &it);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::TriggerFalse);
}

fn limit_case() -> Bundle {
    // a limit buy of 200,000 EUR at 1.0795 placed at 14:00:05; the price dips to 1.0790 at 14:01:00 and fills it
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_798) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_810) },
        Script::Heartbeat { t: T_1400 + 90 },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn limit_order_path_is_valid_and_fills_at_the_sample() {
    let b = limit_case();
    let fill = b.events().into_iter().find_map(|e| if let E::Fill { order_kind, price_e12, time, .. } = e { Some((*order_kind, *price_e12, *time)) } else { None }).unwrap();
    assert_eq!(fill.0, journal_types::OrderKind::Limit);
    assert_eq!(fill.2, T_1400 + 60);
    assert_eq!(fill.1, engine::ask_from_mid(eur(10_790), 30).unwrap(), "the price improvement of a limit is kept");
    valid(&b);
}

#[test]
fn trigger_missed_limit_fill() {
    let mut b = limit_case();
    let f = b.find(|e| matches!(e, E::Fill { .. }));
    b.remove_lines(&[f]);
    // flat again: every Meta says so
    for l in &mut b.lines {
        if let GLine::Event { ev: E::Meta { flat, .. }, .. } = l {
            *flat = true;
        }
    }
    only(&verify_no_snapshot(&b), C::TriggerMissed);
}

#[test]
fn trigger_false_limit_fill_that_no_sample_triggers() {
    let mut b = limit_case();
    let mut it = b.intents[0].intent;
    it.price_e12 = eur(10_780); // the signed limit is lower: 1.0790 does not reach it
    b.intents[0] = sign(1, &it);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::TriggerFalse);
}

#[test]
fn a_cancelled_order_does_not_fill() {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Order { t: T_1400 + 20, intent: order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, 1, T_1400 + 20) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Heartbeat { t: T_1400 + 60 },
    ];
    let b = Gen::new(spec).run(&script);
    assert!(!b.events().iter().any(|e| matches!(e, E::Fill { .. })));
    valid(&b);
}

// ---------------------------------------------------------------- closed markets

#[test]
fn a_closed_market_triggers_nothing_and_the_verifier_honours_the_sessions() {
    // Monday 14:00 is minute 840 of the week; the window ends at minute 845, so the market closes at 14:05
    let mut spec = Spec::base();
    spec.markets[0].sessions = vec![(0, 845)];
    let mids = [eur(10_800) + 1_000_000, eur(10_800) + 2_000_000, eur(10_800) + 3_000_000, eur(10_800) + 4_000_000, eur(10_800) + 5_000_000, eur(10_800) + 6_000_000, eur(10_800) + 7_000_000, eur(10_800) + 8_000_000, eur(10_800) + 9_000_000, eur(10_800) + 10_000_000, eur(10_500)];
    let mut script = vec![Script::Sample { market: 0, t: T_1400, mid: eur(10_800) }, Script::Order { t: T_1400, intent: market_order(0, true, 1_000_000_000_000, T_1400) }];
    let mut t = T_1400;
    for m in mids {
        t += 30;
        script.push(Script::Sample { market: 0, t, mid: m });
    }
    script.push(Script::Heartbeat { t });
    let closed = Gen::new(spec.clone()).run(&script);
    // 1.0500 is 3 % below: with an open market this is a static-floor fail, with a closed one nothing happens
    assert!(!closed.events().iter().any(|e| matches!(e, E::Fail { .. } | E::Breach { .. })), "{:?}", kinds(&closed));
    valid(&closed);
    let mut open_spec = spec;
    open_spec.markets[0].sessions = vec![(0, 10_080)];
    let open = Gen::new(open_spec.clone()).run(&script);
    assert!(open.events().iter().any(|e| matches!(e, E::Fail { .. })), "{:?}", kinds(&open));
    valid(&open);
    // the journal of the closed run, judged against rules that keep the market open all week, misses the fail
    let mut f = closed.files();
    f.rules = open_spec.rules_json().into_bytes();
    f.snapshot = None;
    let r = setl8_verify::verify_account(&f.inputs()).unwrap();
    assert!(r.classes().contains(&C::MissedFail), "{:#?}", r.findings);
}

// ---------------------------------------------------------------- pass verdicts

#[test]
fn pass_requested_at_exactly_one_point_zero_eight_s_is_valid() {
    let mut b = pass_case(0);
    assert_eq!(b.final_state.b, 108_000_000_000);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    b.push_pass_confirmed(2, 8_000_000_000, 108_000_000_000, seq, t + 172_800);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert_eq!(r.per_pass.len(), 2);
    assert!(r.per_pass.iter().all(|p| p.valid && p.reasons.is_empty()));
    assert_eq!((r.per_pass[0].kind, r.per_pass[0].phase, r.per_pass[0].requested_seq), ("requested", 2, seq));
    assert_eq!(r.per_pass[1].kind, "confirmed");
}

#[test]
fn pass_requested_one_micro_short_is_invalid() {
    let mut b = pass_case(-1);
    assert_eq!(b.final_state.b, 108_000_000_000 - 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000 - 1, seq, t);
    let r = verify_no_snapshot(&b);
    only(&r, C::PassInvalid);
    assert_eq!(r.per_pass[0].reasons, vec!["target_not_reached".to_string()]);
    assert!(!r.per_pass[0].valid);
}

#[test]
fn pass_requested_with_the_journal_behind() {
    let mut b = pass_case(0);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq + 3, t);
    let r = verify_no_snapshot(&b);
    only(&r, C::PassInvalid);
    assert!(r.per_pass[0].reasons.contains(&"journal_behind".to_string()), "{:?}", r.per_pass);
}

#[test]
fn pass_request_claims_are_compared_with_the_replay() {
    let base = pass_case(0);
    let (seq, t) = (base.last_seq(), last_event_time(&base));
    let try_with = |amount: u64, b: i64, phase: u8, sq: u64, flip_hash: bool| {
        let mut x = base.clone();
        x.push_pass_requested(phase, amount, b, sq, t);
        if flip_hash {
            let h = x.head_at(sq);
            let mut y = h;
            y[0] ^= 1;
            if let Some(GLine::Raw(s)) = x.lines.last_mut() {
                *s = s.replace(&hexutil_enc(&h), &hexutil_enc(&y));
            }
        }
        verify_no_snapshot(&x).per_pass[0].reasons.clone()
    };
    assert_eq!(try_with(8_000_000_000, 108_000_000_000, 2, seq, false), Vec::<String>::new());
    assert_eq!(try_with(8_000_000_001, 108_000_000_000, 2, seq, false), vec!["amount_mismatch"]);
    assert_eq!(try_with(8_000_000_000, 108_000_000_001, 2, seq, false), vec!["b_mismatch"]);
    assert_eq!(try_with(8_000_000_000, 108_000_000_000, 3, seq, false), vec!["phase_mismatch"]);
    assert_eq!(try_with(8_000_000_000, 108_000_000_000, 2, seq, true), vec!["hash_mismatch"]);
    assert_eq!(try_with(8_000_000_000, 108_000_000_000, 2, seq - 1, false), vec!["seq_mismatch"]);
}

fn hexutil_enc(b: &[u8; 32]) -> String {
    setl8_verify::hexutil::encode(b)
}

#[test]
fn pass_requested_while_a_position_is_open_is_not_flat() {
    let mut b = pass_case(0);
    // right after the Fill batch (seq 2): a position is open and b is still S
    let line = b.metas[0].line;
    let h = b.head_at(2);
    b.lines.insert(
        line + 1,
        GLine::Raw(format!(
            "{{\"type\":\"pass_requested\",\"account\":\"{}\",\"phase\":2,\"amount\":8000000000,\"b\":100000000000,\"seq\":2,\"journal_hash\":\"{}\",\"time\":{}}}",
            setl8_verify::log::base58(&account_key()),
            hexutil_enc(&h),
            T_1400
        )),
    );
    let r = verify_no_snapshot(&b);
    only(&r, C::PassInvalid);
    let reasons = &r.per_pass[0].reasons;
    assert!(reasons.contains(&"not_flat".to_string()) || reasons.contains(&"target_not_reached".to_string()), "{reasons:?}");
}

#[test]
fn a_pass_after_an_invalid_history_carries_history_invalid() {
    let mut b = pass_case(0);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { pnl, .. } = b.ev_mut(i) {
        *pnl += 1;
    }
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), vec![C::PnlMismatch, C::PassInvalid]);
    assert!(r.per_pass[0].reasons.contains(&"history_invalid".to_string()));
}

#[test]
fn a_pass_requested_after_a_missed_breach_is_invalid() {
    // the evaluator "forgot" a breach and then confirmed a pass request: the walk up to the request time finds it
    let mut b = breach_then_recovery();
    let c = b.find(|e| matches!(e, E::Close { .. }));
    let br = b.find(|e| matches!(e, E::Breach { .. }));
    b.remove_lines(&[c, br]);
    let idx = b.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut b, idx - 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(1, 0, 100_000_000_000, seq, t);
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::MissedBreach) && r.classes().contains(&C::PassInvalid), "{:#?}", r.findings);
}

#[test]
fn pass_finalized_starts_a_fresh_phase_and_the_phases_get_their_own_verdicts() {
    let mut b = pass_case(0);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    b.push_pass_confirmed(2, 8_000_000_000, 108_000_000_000, seq, t + 172_800);
    b.push_raw(&format!(
        "\"type\":\"pass_finalized\",\"phase\":2,\"to_phase\":3,\"amount\":8000000000,\"request_id\":1,\"tally_count\":1,\"tally_total\":8000000000,\"window_total\":8000000000,\"time\":{}",
        t + 172_800
    ));
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert_eq!(r.state.phase, 3);
    assert_eq!(r.state.b, 100_000_000_000, "the next phase starts fresh");
    assert_eq!(r.per_phase.len(), 2);
    assert_eq!((r.per_phase[0].phase, r.per_phase[0].first_seq, r.per_phase[0].last_seq, r.per_phase[0].valid), (2, 1, seq, true));
    assert_eq!((r.per_phase[1].phase, r.per_phase[1].events, r.per_phase[1].valid), (3, 0, true));
}

#[test]
fn a_phase_with_a_bad_history_is_marked_invalid() {
    let mut b = pass_case(0);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { credited, .. } = b.ev_mut(i) {
        *credited += 1;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::CreditMismatch);
    assert!(!r.per_phase[0].valid);
}

#[test]
fn pass_finalized_without_a_passing_state_is_invalid() {
    let mut b = pass_case(-1);
    let t = last_event_time(&b);
    b.push_raw(&format!(
        "\"type\":\"pass_finalized\",\"phase\":2,\"to_phase\":3,\"amount\":8000000000,\"request_id\":1,\"tally_count\":1,\"tally_total\":8000000000,\"window_total\":8000000000,\"time\":{t}"
    ));
    only(&verify_no_snapshot(&b), C::PassInvalid);
}

// ---------------------------------------------------------------- determinism, as-of, watch

#[test]
fn stage_2_output_is_byte_identical_between_runs() {
    let b = honest_long();
    let a = serde_json::to_string(&verify(&b)).unwrap();
    let c = serde_json::to_string(&verify(&b)).unwrap();
    assert_eq!(a, c);
    let m = breach_then_recovery();
    let (x, y) = (serde_json::to_string(&verify_second(&m, m.archive_bytes())).unwrap(), serde_json::to_string(&verify_second(&m, m.archive_bytes())).unwrap());
    assert_eq!(x, y);
}

#[test]
fn as_of_seq_replays_up_to_a_point_and_the_walk_stops_there() {
    let b = breach_then_recovery();
    let f = b.files();
    let mut inputs = f.inputs();
    inputs.snapshot = None;
    // up to the TightStart batch (seq 4): tight mode on, no breach yet
    inputs.as_of_seq = Some(4);
    let r = setl8_verify::verify_account(&inputs).unwrap();
    assert_eq!(r.verdict, Verdict::Valid, "{:#?}", r.findings);
    assert!(r.state.tight_on);
    assert_eq!(r.state.seq, 4);
    // up to the end
    inputs.as_of_seq = None;
    let r = setl8_verify::verify_account(&inputs).unwrap();
    assert_eq!(r.verdict, Verdict::Valid);
    assert!(!r.state.tight_on);
}

#[test]
fn watch_reports_the_newest_pending_pass() {
    use setl8_verify::cli;
    let run = |b: &Bundle| {
        let f = b.files();
        let get = |p: &str| -> std::io::Result<Vec<u8>> {
            Ok(match p {
                "e" => f.events.clone(),
                "r" => f.rules.clone(),
                "a" => f.archive.clone(),
                _ => f.intents.clone(),
            })
        };
        let args: Vec<String> = ["watch", "--events", "e", "--rules", "r", "--archive", "a", "--intents", "i"].iter().map(|s| s.to_string()).collect();
        let mut out = String::new();
        let code = cli::run(&args, &get, &mut out);
        (code, out)
    };
    let mut ok = pass_case(0);
    let (seq, t) = (ok.last_seq(), last_event_time(&ok));
    ok.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    assert_eq!(run(&ok), (0, format!("VALID seq={seq}\n")));
    let mut short = pass_case(-1);
    short.push_pass_requested(2, 8_000_000_000, 108_000_000_000 - 1, seq, t);
    assert_eq!(run(&short), (1, format!("MISMATCH seq={seq} classes=PassInvalid\n")));
    // a rejected pass is no longer pending
    let mut done = pass_case(-1);
    done.push_pass_requested(2, 8_000_000_000, 108_000_000_000 - 1, seq, t);
    done.push_raw("\"type\":\"pass_rejected\",\"phase\":2,\"reason\":2");
    assert_eq!(run(&done), (0, "NO_PENDING_PASS\n".to_string()));
}

// ---------------------------------------------------------------- quick-limit fail, caps, misc

fn quick_overshoot() -> Bundle {
    // 2,000,000 EUR bought and closed 20 s later after a 0.0035 rise: a quick profit of about 7,000 is credited about 3,500
    // which is above 0.028 S = 2,800: the account fails with PhaseQuick
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 2_000_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_835) },
        Script::Order { t: T_1400 + 20, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 20) },
        Script::Heartbeat { t: T_1400 + 20 },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn quick_limit_fail_is_valid_when_the_account_is_failed() {
    let b = quick_overshoot();
    assert!(b.events().iter().any(|e| matches!(e, E::Fail { reason: journal_types::FailReason::PhaseQuick, .. })), "{:?}", kinds(&b));
    valid(&b);
    let r = verify(&b);
    assert!(r.state.terminal && r.state.q_tally > 2_800_000_000);
}

#[test]
fn missed_fail_the_quick_limit_is_exceeded_and_nothing_fails_the_account() {
    // the Fail is replaced by an ordinary TightEnd: the account carries on although Q is above 0.028 S
    let mut b = quick_overshoot();
    let f = b.find(|e| matches!(e, E::Fail { .. }));
    b.lines[f] = GLine::Event { seq: 0, ev: E::TightEnd { time: T_1400 + 20 }, proof: None };
    b.renumber();
    only(&verify_no_snapshot(&b), C::MissedFail);
    // the Fail simply missing: the next Meta notices
    let mut b = quick_overshoot();
    let f = b.find(|e| matches!(e, E::Fail { .. }));
    b.remove_lines(&[f]);
    assert!(verify_no_snapshot(&b).classes().contains(&C::MissedFail));
}

#[test]
fn findings_are_capped_at_one_thousand_and_the_verdict_stays_mismatch() {
    let mut b = meta_only(1_200);
    for l in &mut b.lines {
        if let GLine::Event { ev: E::Meta { engine_version, .. }, .. } = l {
            *engine_version = 2;
        }
    }
    let r = verify_no_snapshot(&b);
    assert_eq!(r.findings.len(), 1_000);
    assert!(r.truncated);
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(!verify_no_snapshot(&meta_only(3)).truncated);
}

#[test]
fn intents_beyond_the_committed_prefix_are_not_judged() {
    let mut b = close_case(600);
    // a junk intent appended after the last committed line (the evaluator received it after the last batch)
    let mut late = b.intents[0].clone();
    late.nonce = 77;
    late.sig[0] ^= 1;
    b.intents.push(late);
    valid(&b);
}

#[test]
fn all_snapshot_fields_are_compared() {
    let b = design_4_9();
    for field in ["latch", "tight_on", "r", "g_peak", "l_level", "flat", "phase", "trades_closed", "lock_until", "b", "q_tally"] {
        let mut f = b.files();
        let mut snap: serde_json::Value = serde_json::from_slice(f.snapshot.as_ref().unwrap()).unwrap();
        let v = snap[field].as_i64().unwrap() + 1;
        snap[field] = serde_json::json!(v);
        f.snapshot = Some(serde_json::to_vec(&snap).unwrap());
        let r = setl8_verify::verify_account(&f.inputs()).unwrap();
        only(&r, C::StateMismatch);
        assert!(r.findings[0].message.contains(field), "{field}: {}", r.findings[0].message);
    }
    let mut f = b.files();
    let mut snap: serde_json::Value = serde_json::from_slice(f.snapshot.as_ref().unwrap()).unwrap();
    snap["status"] = serde_json::json!(1);
    f.snapshot = Some(serde_json::to_vec(&snap).unwrap());
    only(&setl8_verify::verify_account(&f.inputs()).unwrap(), C::StateMismatch);
}

#[test]
fn an_account_opened_twice_and_lines_of_another_account() {
    let mut b = meta_only(1);
    let o = b.lines[0].clone();
    b.lines.insert(1, o);
    only(&verify_no_snapshot(&b), C::StateMismatch);
    let mut b = meta_only(1);
    b.lines.push(GLine::Raw(format!("{{\"type\":\"pass_rejected\",\"account\":\"{}\",\"phase\":1,\"reason\":0}}", setl8_verify::log::base58(&[9; 32]))));
    assert!(setl8_verify::verify_account(&b.files().inputs()).is_err());
}

#[test]
fn events_after_a_fail_are_refused_but_the_closing_meta_is_fine() {
    let mut b = quick_overshoot();
    valid(&b);
    // a trade after the account failed
    let proof = b.lines.iter().find_map(|l| if let GLine::Event { proof: Some(p), .. } = l { Some(p.clone()) } else { None }).unwrap();
    let ev = E::Fee { kind: journal_types::FeeKind::Commission, market: 0, amount: 1, time: T_1400 + 20 };
    let _ = proof;
    let last = b.lines.len();
    b.lines.insert(last, GLine::Event { seq: 0, ev, proof: None });
    b.renumber();
    assert!(verify_no_snapshot(&b).classes().contains(&C::StateMismatch));
}

#[test]
fn rollover_instants() {
    use setl8_verify::replay::next_rollover_after;
    let r = T_1400 + 8 * HOUR; // 22:00:00
    assert_eq!(next_rollover_after(T_1400), r);
    assert_eq!(next_rollover_after(r - 1), r);
    assert_eq!(next_rollover_after(r), r + 86_400, "strictly after");
    assert_eq!(next_rollover_after(r + 86_399), r + 86_400);
}
