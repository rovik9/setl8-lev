//! Boundaries inside the execution of an order: partial closes, the margin check, the notional cap, a flip that overshoots
//! the quick limit, and a forced close of several positions. Hand-written numbers (S = $100,000, class 0 leverage 50).
mod common;
use common::*;
use evaluator::codec::IntentKind;
use evaluator::{OrderReject as R, PassDecision, PassReject, Status};
use journal_types::{CloseReason, FailReason, JournalEvent};

/// Class 0, half-spread `TEST_HALF_SPREAD_E6` (1e-6), a long price memory; `commission_e6` is the rate of class 0.
fn world(commission_e6: u32, max_notional_micro: u64, markets: usize) -> World {
    world_with_size(S100K, commission_e6, max_notional_micro, markets)
}

fn world_with_size(size: i64, commission_e6: u32, max_notional_micro: u64, markets: usize) -> World {
    let ms: Vec<_> = (0..markets as u16)
        .map(|i| {
            let mut m = market(i, 0, &format!("PAIR{i}"), TEST_HALF_SPREAD_E6);
            m.max_age_secs = 3_600;
            m.max_notional_micro = max_notional_micro;
            m
        })
        .collect();
    let mut rules = base_rules(ms);
    rules.commission_e6[0] = commission_e6;
    World::new(rules, size, 2, T_1400 - HOUR)
}

#[test]
fn queued_closes_see_the_position_the_earlier_ones_left() {
    let mut w = world(0, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 2 * MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions()[&0].p.qty, (2 * MEUR) as i64);
    let t = T_1400 + 6;
    // 1.5 M of 2 M: a partial close. 1.0 M of the remaining 0.5 M: too much. 0.5 M of 0.5 M: the rest.
    w.order(close_order(0, MEUR + MEUR / 2, t)).unwrap();
    w.order(close_order(0, MEUR, t)).unwrap();
    w.order(close_order(0, MEUR / 2, t)).unwrap();
    w.ev.tick(t + 1);
    let r: Vec<_> = w.ev.refusals().iter().map(|r| r.reason).collect();
    assert_eq!(r, vec![R::InvalidQuantity]);
    assert!(w.ev.positions().is_empty());
    let bs = w.drain();
    let closes: Vec<u64> = events_of(&bs).iter().filter_map(|e| if let JournalEvent::Close { qty, .. } = e { Some(*qty) } else { None }).collect();
    assert_eq!(closes, vec![MEUR + MEUR / 2, MEUR / 2]);
    assert_valid(&w, &bs);
}

/// The quantity filled at the ask of mid 1.0000 (ASK_AT_ONE = 1.000001): 50 M EUR at leverage 50 is $1,000,001 of margin.
const FULL_MARGIN: u64 = 50 * MEUR;
/// The account size that makes equity exactly the margin of FULL_MARGIN: the margin $1,000,001 plus the open loss the long carries
/// at once (marked at the bid, 2 x 1e-6 below the ask: 50 M x 0.000002 = $100). At S = $100,000 no quantity on the 0.001 EUR step
/// lands exactly on the boundary (5 M EUR leaves 19 micro-USD over at the best, the next step is 1 short), and a size is a whole
/// number of dollars, so the quantity is scaled by ten (the smallest one for which the sum is a whole number of dollars).
const FULL_MARGIN_SIZE: i64 = 1_000_101 * USD;

#[test]
fn a_market_order_may_use_all_of_the_free_margin_but_not_one_step_more() {
    assert_eq!(engine::margin_micro(FULL_MARGIN, ASK_AT_ONE, 50).unwrap(), 1_000_001 * USD);
    assert_eq!(engine::position::pnl_micro(true, FULL_MARGIN, ASK_AT_ONE, BID_AT_ONE).unwrap(), -100 * USD);
    let mut w = world_with_size(FULL_MARGIN_SIZE, 0, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, FULL_MARGIN + 1_000, T_1400)).unwrap();
    w.order(market_order(0, true, FULL_MARGIN, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.refusals().iter().map(|r| r.reason).collect::<Vec<_>>(), vec![R::InsufficientMargin]);
    assert_eq!(w.ev.positions()[&0].p.qty, FULL_MARGIN as i64, "equity equal to the margin is enough");
}

#[test]
fn the_notional_cap_is_inclusive_for_a_market_order() {
    // the cap is checked on the fill (the ask): 1 M EUR x 1.000001 = $1,000,001
    let notional = engine::units::notional_micro(MEUR, ASK_AT_ONE).unwrap();
    for (cap, taken) in [(notional, true), (notional - 1, false)] {
        let mut w = world(0, u64::try_from(cap).unwrap(), 1);
        w.sample(0, T_1400, p4(10_000));
        w.order(market_order(0, true, MEUR, T_1400)).unwrap();
        w.sample(0, T_1400 + 5, p4(10_000));
        w.ev.tick(T_1400 + 6);
        assert_eq!(!w.ev.positions().is_empty(), taken, "cap {cap}");
        if !taken {
            assert_eq!(w.ev.refusals()[0].reason, R::NotionalCap);
        }
    }
}

#[test]
fn commissions_count_twice_in_the_margin_check() {
    // rate 0.005%: paid on the fill and reserved again for the close, and the long is marked at the bid (2e-6 under the ask), so
    // equity = 100,000 - 2 x 0.00005 N - 0.000002 N and margin = 1.000001 N / 50: N may be at most $4,974,624.
    // 4.9 M EUR passes (equity 99,500.2 against margin 98,000.1); 4.99 M does not (99,491.0 against 99,800.1),
    // although it would if the two commissions were added back (99,990.0 against 99,800.1).
    for (qty, taken) in [(4_900_000u64, true), (4_990_000, false)] {
        let mut w = world(50, 1_000_000_000_000_000, 1);
        w.sample(0, T_1400, p4(10_000));
        w.order(market_order(0, true, qty * 1_000_000, T_1400)).unwrap();
        w.sample(0, T_1400 + 5, p4(10_000));
        w.ev.tick(T_1400 + 6);
        assert_eq!(!w.ev.positions().is_empty(), taken, "{qty} EUR: {:?}", w.ev.refusals());
        if !taken {
            assert_eq!(w.ev.refusals()[0].reason, R::InsufficientMargin);
        }
    }
}

#[test]
fn open_profit_counts_for_the_margin_of_the_next_order() {
    // 1 M EUR bought at 1.0000 (filled at the ask 1.000001) with commission 0.05% ($500.0005 paid), then the mid goes to 1.0050
    // (bid 1.004999, ask 1.005001): open profit 1 M x (1.004998995 - 1.000001) = $4,998.
    // A second order of n M EUR: equity = 99,500 + 4,998 - 502.5 (closing commission, first) - 1,005 n (closing commission and
    // fill commission, second) and margin = 20,000 + 20,100 n. With the profit counted n may reach 3.98 (3,979,881 EUR); with it
    // subtracted only 3.51 (3,506,250 EUR); the spread moves these by less than a thousandth.
    let mut w = world(500, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.sample(0, T_1400 + 10, p4(10_050));
    w.ev.tick(T_1400 + 11);
    assert_eq!(w.ev.positions()[&0].p.qty, MEUR as i64);
    w.order(market_order(0, true, 3_700_000 * 1_000_000, T_1400 + 11)).unwrap();
    w.ev.tick(T_1400 + 12);
    assert_eq!(w.ev.refusals().len(), 0, "{:?}", w.ev.refusals());
    assert_eq!(w.ev.positions()[&0].p.qty, (4_700_000u64 * 1_000_000) as i64);
}

/// A long of `qty` opened at 1.0000 and marked at 1.0030 twenty seconds later: a quick trade worth `qty x 0.003` of profit.
fn quick_profit_world(markets: usize, qtys: &[u64]) -> World {
    let mut w = world(0, 1_000_000_000_000_000, markets);
    for m in 0..markets as u16 {
        w.sample(m, T_1400, p4(10_000));
    }
    for (m, q) in qtys.iter().enumerate() {
        w.order(market_order(m as u16, true, *q, T_1400)).unwrap();
    }
    for m in 0..markets as u16 {
        w.sample(m, T_1400 + 5, p4(10_000));
    }
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), markets);
    w
}

#[test]
fn a_flip_whose_closing_slice_overshoots_the_quick_limit_fails_the_phase() {
    // 2 M EUR, +0.30% in 20 s: profit $5,996 (ask in, bid out), the quick credit $2,998 is over 0.028 S = $2,800.
    let mut w = quick_profit_world(1, &[2 * MEUR]);
    w.sample(0, T_1400 + 20, p4(10_030));
    w.ev.tick(T_1400 + 21);
    assert!(matches!(w.ev.status(), Status::Active));
    w.order(market_order(0, false, 3 * MEUR, T_1400 + 21)).unwrap(); // sell 3 M: closes 2 M, would open 1 M short
    w.ev.tick(T_1400 + 22);
    assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)), "{:?}", w.ev.status());
    assert!(w.ev.positions().is_empty());
    let bs = w.drain();
    let evs = events_of(&bs);
    assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Fill { order_kind: journal_types::OrderKind::Market, qty, .. } if *qty == MEUR)), "the short is void");
    let k = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })).expect("a Fail");
    assert!(matches!(evs[k - 1], JournalEvent::Close { reason: CloseReason::Flip, qty, .. } if qty == 2 * MEUR), "{:?}", evs[k - 1]);
    // M9b decision D2: the trades under the intent end at the Fail, the verifier agrees
    assert_valid(&w, &bs);
    // a failed account asks for no pass
    let chain = evaluator::ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(T_1400 + 23, &chain, evaluator::ReplayCheck::Skipped), PassDecision::Reject(PassReject::Other));
}

#[test]
fn a_flip_that_stays_under_the_quick_limit_books_the_close_and_the_new_position() {
    // the same, +0.20%: profit $3,996, credit $1,998 < $2,800
    let mut w = quick_profit_world(1, &[2 * MEUR]);
    w.sample(0, T_1400 + 20, p4(10_020));
    w.ev.tick(T_1400 + 21);
    w.order(market_order(0, false, 3 * MEUR, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    assert!(matches!(w.ev.status(), Status::Active));
    assert_eq!(w.ev.positions()[&0].p.qty, -(MEUR as i64));
    let bs = w.drain();
    let evs = events_of(&bs);
    let k = evs.iter().position(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Flip, .. })).expect("a flip close");
    assert!(matches!(evs[k + 1], JournalEvent::Fill { qty, .. } if qty == MEUR));
    assert_valid(&w, &bs);
}

/// `others` positions of 0.5 M EUR (+0.6% = $2,999 profit, a $1,499 quick credit each) and a 1 M EUR one (+0.6% = $5,998) in the last
/// market; closing the big one by hand overshoots the quick limit, so the others are closed first with reason Fail.
fn forced_close(others: usize) -> World {
    let mut qtys = vec![MEUR / 2; others];
    qtys.push(MEUR);
    let mut w = quick_profit_world(others + 1, &qtys);
    for m in 0..=others as u16 {
        w.sample(m, T_1400 + 20, p4(10_060));
    }
    w.ev.tick(T_1400 + 21);
    assert!(matches!(w.ev.status(), Status::Active));
    w.order(close_order(others as u16, 0, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    w
}

#[test]
fn a_manual_close_over_the_quick_limit_closes_every_other_position_first() {
    for others in [2usize, 3] {
        let mut w = forced_close(others);
        assert!(matches!(w.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)), "{others}: {:?}", w.ev.status());
        assert!(w.ev.positions().is_empty(), "every position is closed, including the ones in other markets");
        let bs = w.drain();
        let evs = events_of(&bs);
        let reasons: Vec<CloseReason> = evs.iter().filter_map(|e| if let JournalEvent::Close { reason, .. } = e { Some(*reason) } else { None }).collect();
        let mut want = vec![CloseReason::Fail; others];
        want.push(CloseReason::Manual);
        assert_eq!(reasons, want, "{others}");
        assert!(matches!(evs.last(), Some(JournalEvent::Fail { reason: FailReason::PhaseQuick, .. }) | Some(JournalEvent::Meta { .. })));
        // the credits of the others run 1,499 / 2,999 / 4,498 against the limit 2,800: the slices after the first overshoot are
        // counted as unpostable, except the last one of the group
        let want_unpostable = if others == 2 { 0 } else { 1 };
        assert_eq!(w.ev.stats().unpostable, want_unpostable, "{others} others");
    }
}

#[test]
fn a_pass_request_waits_for_the_journal() {
    let mut w = world(0, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    w.order(close_order(0, 0, T_1400 + 6)).unwrap();
    w.ev.tick(T_1400 + 8);
    assert!(w.ev.positions().is_empty());
    // flat, but a Fill and a Close are still waiting to be posted
    let chain = evaluator::ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(T_1400 + 9, &chain, evaluator::ReplayCheck::Skipped), PassDecision::Reject(PassReject::JournalBehind));
}

#[test]
fn a_pending_order_blocks_a_pass_request() {
    let mut w = world(0, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, T_1400)).unwrap();
    w.ev.tick(T_1400 + 2);
    let _ = w.drain();
    let chain = evaluator::ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(T_1400 + 3, &chain, evaluator::ReplayCheck::Skipped), PassDecision::Reject(PassReject::Other));
}

#[test]
fn a_failed_account_answers_other_even_with_events_waiting() {
    // the flip that overshoots the quick limit, nothing drained yet: the answer is about the account, not the journal
    let mut w = quick_profit_world(1, &[2 * MEUR]);
    w.sample(0, T_1400 + 20, p4(10_030));
    w.ev.tick(T_1400 + 21);
    w.order(market_order(0, false, 3 * MEUR, T_1400 + 21)).unwrap();
    w.ev.tick(T_1400 + 22);
    assert!(matches!(w.ev.status(), Status::Failed(_)));
    assert!(w.ev.queued_events() > 0);
    let chain = evaluator::ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(T_1400 + 23, &chain, evaluator::ReplayCheck::Skipped), PassDecision::Reject(PassReject::Other));
}

#[test]
fn an_account_that_ended_on_the_chain_gets_no_heartbeat() {
    // the chain graduated the account (finalize_pass to phase 0): a Meta posted now would hit AccountNotActive
    let mut w = world(0, 1_000_000_000_000_000, 1);
    w.sample(0, T_1400, p4(10_000));
    w.ev.tick(T_1400 + 30);
    let policy = evaluator::BatchPolicy::MAX.with_heartbeat(true);
    w.ev.end_externally();
    assert!(w.ev.drain_batches(&policy).is_empty());
    let mut live = world(0, 1_000_000_000_000_000, 1);
    live.sample(0, T_1400, p4(10_000));
    live.ev.tick(T_1400 + 30);
    assert_eq!(live.ev.drain_batches(&policy).len(), 1, "an active account does get its heartbeat");
}
