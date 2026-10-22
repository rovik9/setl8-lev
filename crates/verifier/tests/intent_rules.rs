//! P25b (decision 68), audit finding F-14: a signed intent is executed ONCE (nonces are unique across the WHOLE log, not per batch), and
//! an order that executes at once (market, close) is executed within 5 s of the time the trader signed it. Expectations by hand.
mod common;
use common::*;
use journal_types::JournalEvent as E;
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

const W: i64 = 5;

/// A market buy of 100,000 EUR at +0 (intent 1) and a close-all at +60 (intent 2).
fn buy_and_close() -> Bundle {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 100_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_801) },
        Script::Order { t: T_1400 + 60, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 60) },
        Script::Heartbeat { t: T_1400 + 61 },
    ];
    Gen::new(spec).run(&script)
}

fn resign_time(b: &mut Bundle, idx: usize, nonce: u32, t: i64) {
    let mut it = b.intents[idx].intent;
    it.client_time = t;
    b.intents[idx] = sign(nonce, &it);
    b.refresh_commitments();
}

fn forged(b: &Bundle) -> usize {
    verify_no_snapshot(b).findings.iter().filter(|f| f.class == C::ForgedTrade).count()
}

#[test]
fn the_honest_log_is_valid() {
    let r = verify_no_snapshot(&buy_and_close());
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

#[test]
fn a_market_intent_signed_within_five_seconds_of_the_trade_is_accepted_and_six_is_not() {
    for (delta, bad) in [(-W, false), (-W - 1, true), (W, false), (W + 1, true), (0, false)] {
        let mut b = buy_and_close();
        resign_time(&mut b, 0, 1, T_1400 + delta);
        assert_eq!(forged(&b) > 0, bad, "market intent signed {delta} s from the trade: {:#?}", verify_no_snapshot(&b).findings);
    }
}

#[test]
fn a_close_all_intent_is_bound_to_its_window_too() {
    for (delta, bad) in [(-W, false), (W, false), (W + 1, true), (-W - 1, true)] {
        let mut b = buy_and_close();
        resign_time(&mut b, 1, 2, T_1400 + 60 + delta);
        assert_eq!(forged(&b) > 0, bad, "close intent signed {delta} s from the trade");
    }
    // a close-all signed long before: it is valid for its own moment only
    let mut b = buy_and_close();
    resign_time(&mut b, 1, 2, T_1400 + 10);
    assert!(forged(&b) > 0);
}

#[test]
fn an_intent_executed_in_one_batch_cannot_be_used_by_a_later_one() {
    // the second trade of a two-trade log cites the first one's nonce (a different batch)
    let spec = Spec::base();
    let q = 100_000_000_000u64;
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_801) },
        Script::Order { t: T_1400, intent: market_order(0, true, q, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_800) },
        Script::Order { t: T_1400 + 20, intent: market_order(0, true, q, T_1400 + 20) },
        Script::Heartbeat { t: T_1400 + 21 },
    ];
    let honest = Gen::new(spec).run(&script);
    let r = verify_no_snapshot(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
    let mut b = honest.clone();
    let fills: Vec<usize> = b.lines.iter().enumerate().filter(|(_, l)| matches!(l, GLine::Event { ev: E::Fill { .. }, .. })).map(|(i, _)| i).collect();
    assert_eq!(fills.len(), 2);
    if let E::Fill { intent_id, .. } = b.ev_mut(fills[1]) {
        *intent_id = 1;
    }
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert!(r.findings.iter().any(|f| f.class == C::ForgedTrade && f.message.contains("already executed by an earlier batch")), "{:#?}", r.findings);
}

#[test]
fn a_pending_order_is_placed_before_it_fills_the_boundary_is_the_fill_second() {
    // a limit buy placed at +5, filled at +60 (stage2.rs limit_case); the fill may not precede the signed time of its order
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_798) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_810) },
        Script::Heartbeat { t: T_1400 + 90 },
    ];
    let honest = Gen::new(spec).run(&script);
    let dated_after = |t: i64| -> bool {
        let mut b = honest.clone();
        resign_time(&mut b, 0, 1, t);
        verify_no_snapshot(&b).findings.iter().any(|f| f.class == C::ForgedTrade && f.message.contains("dated after the trade"))
    };
    assert!(!dated_after(T_1400 + 5), "the honest placement");
    assert!(!dated_after(T_1400 + 60), "signed in the fill's own second: not after the trade");
    assert!(dated_after(T_1400 + 61), "signed one second after the fill");
}
