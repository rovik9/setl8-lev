//! Every way an order is refused: at arrival (`on_order` returns the reason, nothing is recorded) or when its second closes
//! (the order stays in the intents file, no event is written, `refusals()` says why).
mod common;
use common::*;
use evaluator::codec::{IntentKind, OrderIntent};
use evaluator::{OrderReject as R, Status, STATUS_REDUCE_ONLY};

fn w0() -> World {
    let mut w = fx_world(0);
    w.sample(0, T_1400, p4(10_000));
    w
}

fn intents_lines(w: &World) -> usize {
    String::from_utf8_lossy(w.ev.intents_bytes()).lines().count() - 1
}

#[test]
fn a_refused_order_leaves_nothing_in_the_intents_file() {
    let mut w = w0();
    assert_eq!(intents_lines(&w), 0);
    assert_eq!(w.order(market_order(9, true, MEUR, T_1400)), Err(R::UnknownMarket));
    assert_eq!(intents_lines(&w), 0);
    assert!(w.order(market_order(0, true, MEUR, T_1400)).is_ok());
    assert_eq!(intents_lines(&w), 1);
}

#[test]
fn signature_nonce_and_decoding() {
    let mut w = w0();
    let acct = w.opened.account;
    // a bit flipped in the signature
    let mut s = sign(&acct, 1, &market_order(0, true, MEUR, T_1400));
    s.sig[0] ^= 1;
    assert_eq!(w.ev.on_order(&s), Err(R::BadSignature));
    // a signature for another account
    let other = sign(&[9; 32], 1, &market_order(0, true, MEUR, T_1400));
    assert_eq!(w.ev.on_order(&other), Err(R::BadSignature));
    // nonce 0 is reserved
    let z = sign(&acct, 0, &market_order(0, true, MEUR, T_1400));
    assert_eq!(w.ev.on_order(&z), Err(R::BadNonce));
    // a nonce is used once
    let ok = sign(&acct, 7, &market_order(0, true, MEUR, T_1400));
    assert_eq!(w.ev.on_order(&ok), Ok(()));
    assert_eq!(w.ev.on_order(&ok), Err(R::BadNonce));
    // order bytes that are not an OrderIntent (unknown kind, side 2)
    let mut bad = market_order(0, true, MEUR, T_1400).to_bytes();
    bad[0] = 9;
    assert_eq!(w.ev.on_order(&sign_raw(&acct, 8, bad)), Err(R::BadOrder));
    let mut bad = market_order(0, true, MEUR, T_1400).to_bytes();
    bad[3] = 2;
    assert_eq!(w.ev.on_order(&sign_raw(&acct, 9, bad)), Err(R::BadOrder));
}

#[test]
fn order_times() {
    let mut w = w0();
    w.sample(0, T_1400 + 10, p4(10_000));
    // the evaluator is at second T+10: an order dated T+9 is stale, T+10 is fine, T+15 (+5) is fine, T+16 is too far ahead
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 9)), Err(R::StaleOrder));
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 10)).is_ok());
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 16)), Err(R::FutureOrder));
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 15)).is_ok());
    // accepting an order dated ahead moves the clock: later samples of earlier seconds are out of order
    assert_eq!(w.ev.clock(), T_1400 + 15);
    assert_eq!(w.ev.on_sample_mid(0, T_1400 + 12, p4(10_000)), Err(evaluator::SampleError::OutOfOrder));
}

#[test]
fn markets_quantities_levels() {
    let mut w = w0();
    assert_eq!(w.order(market_order(0, true, 0, T_1400)), Err(R::InvalidQuantity));
    assert_eq!(w.order(market_order(0, true, 1_500, T_1400)), Err(R::InvalidQuantity), "not a multiple of the step 1,000");
    assert_eq!(w.order(market_order(0, true, 0, T_1400)), Err(R::InvalidQuantity));
    // qty_min is 1,000 and the step 1,000 in this market
    assert!(w.order(market_order(0, true, 1_000, T_1400)).is_ok());
    // limit/stop need a level; stop-loss and take-profit must be on the right side of the level
    let lim = |price, sl, tp, side| order(IntentKind::Limit, 0, side, MEUR, price, sl, tp, 0, T_1400);
    assert_eq!(w.order(lim(0, 0, 0, 0)), Err(R::InvalidLevel));
    assert_eq!(w.order(lim(p4(9_900), p4(9_950), 0, 0)), Err(R::InvalidLevel), "a buy limit's stop-loss must be below the level");
    assert_eq!(w.order(lim(p4(9_900), 0, p4(9_800), 0)), Err(R::InvalidLevel), "a buy limit's take-profit must be above");
    assert_eq!(w.order(order(IntentKind::Stop, 0, 0, MEUR, p4(10_100), p4(10_200), 0, 0, T_1400)), Err(R::InvalidLevel), "a buy stop's stop-loss must be below its level too");
}

#[test]
fn limit_levels_for_a_sell() {
    let mut w = w0();
    let lim = |price, sl, tp| order(IntentKind::Limit, 0, 1, MEUR, price, sl, tp, 0, T_1400);
    assert_eq!(w.order(lim(p4(10_100), p4(10_050), 0)), Err(R::InvalidLevel), "a sell limit's stop-loss must be above the level");
    assert_eq!(w.order(lim(p4(10_100), 0, p4(10_200))), Err(R::InvalidLevel), "a sell limit's take-profit must be below");
    assert!(w.order(lim(p4(10_100), p4(10_200), p4(10_000))).is_ok());
}

#[test]
fn close_cancel_and_set_levels() {
    let mut w = w0();
    assert_eq!(w.order(close_order(0, 0, T_1400)), Err(R::NoPosition));
    assert_eq!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(9_000), 0, 0, T_1400)), Err(R::NoPosition));
    assert_eq!(w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, 77, T_1400)), Err(R::NoSuchOrder));
    // open a position, then: a close larger than the position, a stop-loss above the bid, a take-profit below it
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), 1);
    assert_eq!(w.order(close_order(0, 2 * MEUR, T_1400 + 6)), Err(R::InvalidQuantity));
    assert_eq!(w.order(close_order(0, 1_500, T_1400 + 6)), Err(R::InvalidQuantity));
    assert_eq!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(10_100), 0, 0, T_1400 + 6)), Err(R::InvalidLevel));
    assert_eq!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, 0, p4(9_900), 0, T_1400 + 6)), Err(R::InvalidLevel));
    // valid ones, and the cancel of a pending order
    assert!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(9_900), p4(10_100), 0, T_1400 + 6)).is_ok());
    let n = w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, T_1400 + 6)).unwrap();
    assert!(w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, n, T_1400 + 6)).is_ok());
    w.ev.tick(T_1400 + 7);
    assert!(w.ev.pending().is_empty());
    let acct = w.opened.account;
    let _ = acct;
}

#[test]
fn pending_order_limits() {
    let mut w = w0();
    for i in 0..10u64 {
        assert!(w.order(order(IntentKind::Limit, 0, 0, 1_000_000, p4(9_000 + i), 0, 0, 0, T_1400)).is_ok(), "order {i}");
    }
    assert_eq!(w.order(order(IntentKind::Limit, 0, 0, 1_000_000, p4(9_000), 0, 0, 0, T_1400)), Err(R::TooManyPending));
    // a cancel makes room
    assert!(w.order(order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, 1, T_1400)).is_ok());
}

#[test]
fn freezing_and_ending() {
    let mut w = w0();
    assert!(!w.ev.is_frozen());
    w.ev.tick(T_1400 + 1);
    let chain = evaluator::ChainView { seq: 0, journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    let d = w.ev.request_pass(T_1400 + 1, &chain, evaluator::ReplayCheck::Skipped);
    assert_eq!(d, evaluator::PassDecision::Reject(evaluator::PassReject::TargetNotReached));
    assert!(w.ev.is_frozen());
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 2)), Err(R::AccountFrozen));
    w.ev.pass_resolved();
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 2)).is_ok());
    w.sample(0, T_1400 + 3, p4(10_000));
    w.ev.tick(T_1400 + 4);
    assert_eq!(w.ev.positions().len(), 1);
    // a held account refuses limit orders up front and opening market orders at execution
    w.ev.set_held(true);
    assert_eq!(w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_000), 0, 0, 0, T_1400 + 4)), Err(R::AccountHeld));
    assert_eq!(w.ev.refusals().len(), 0);
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 4)).is_ok());
    w.sample(0, T_1400 + 6, p4(10_000));
    w.ev.tick(T_1400 + 7);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::AccountHeld), "an opening order is refused while held");
    // a close is not an opening order: it goes through
    assert!(w.order(close_order(0, 0, T_1400 + 7)).is_ok());
    w.sample(0, T_1400 + 9, p4(10_000));
    w.ev.tick(T_1400 + 10);
    assert!(w.ev.positions().is_empty());
    w.ev.set_held(false);
    // an account that failed refuses everything
    w.order(market_order(0, true, 45_000_000_000_000 / 10, T_1400 + 10)).unwrap();
    w.sample(0, T_1400 + 11, p4(10_000));
    w.sample(0, T_1400 + 20, p4(9_000));
    w.ev.tick(T_1400 + 21);
    assert!(matches!(w.ev.status(), Status::Failed(_)), "{:?}", w.ev.stats());
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 21)), Err(R::AccountEnded));
}

#[test]
fn market_status_sessions_and_staleness() {
    // reduce-only: limit orders refused, an opening market order refused at execution, a close allowed
    let mut m = eur_market(0);
    m.half_spread_e6 = 0;
    m.max_age_secs = 3_600;
    let mut w = World::new(base_rules(vec![m.clone()]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 2, p4(10_000));
    w.ev.tick(T_1400 + 3);
    assert_eq!(w.ev.positions().len(), 1);
    // reduce-only world
    let mut ro = m.clone();
    ro.status = STATUS_REDUCE_ONLY;
    let mut w = World::new(base_rules(vec![ro.clone()]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    assert_eq!(w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_900), 0, 0, 0, T_1400)), Err(R::MarketNotTradable));
    assert!(w.order(market_order(0, true, MEUR, T_1400)).is_ok());
    w.sample(0, T_1400 + 2, p4(10_000));
    w.ev.tick(T_1400 + 3);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::MarketNotTradable));
    assert!(w.ev.positions().is_empty());
    // outside the session windows: closed
    let mut sess = m.clone();
    sess.sessions = vec![(0, 60)]; // Monday 00:00-01:00 only; T_1400 is Monday 14:00
    let mut w = World::new(base_rules(vec![sess]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400)), Err(R::MarketClosed));
    assert_eq!(w.ev.stats().closed_market_rejects, 1);
    // no session at all is closed too
    let mut none = m.clone();
    none.sessions = vec![];
    let mut w = World::new(base_rules(vec![none]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400)), Err(R::MarketClosed));
    // no price yet, a price older than max_age, and a price that has not CHANGED within max_age
    let mut w = World::new(base_rules(vec![eur_market(0)]), S100K, 2, T_1400 - HOUR);
    w.ev.tick(T_1400);
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400)), Err(R::NoFreshPrice));
    w.sample(0, T_1400, p4(10_000));
    w.ev.tick(T_1400 + 100);
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 100)), Err(R::NoFreshPrice), "the newest sample is 100 s old, max_age is 60");
    w.sample(0, T_1400 + 100, p4(10_000));
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 100)), Err(R::MarketClosed), "fresh sample, but the value has not changed for 100 s");
    w.sample(0, T_1400 + 101, p4(10_001));
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 101)).is_ok());
}

#[test]
fn notional_cap_margin_and_position_limits() {
    // per-position cap: 1,000,000 EUR at 1.0 is $1,000,000
    let mut m = eur_market(0);
    m.half_spread_e6 = 0;
    m.max_notional_micro = 999_999_000_000;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    assert!(w.order(market_order(0, true, 999_000_000_000, T_1400)).is_ok());
    assert_eq!(w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(10_000), 0, 0, 0, T_1400)), Err(R::NotionalCap), "a pending order is capped at arrival");
    w.sample(0, T_1400 + 2, p4(10_000));
    w.ev.tick(T_1400 + 3);
    assert_eq!(w.ev.positions().len(), 1);
    w.order(market_order(0, true, 2_000_000_000, T_1400 + 3)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::NotionalCap), "adding 2,000 EUR would pass the cap");
    // margin: 1:50 on EURUSD, equity $100,000: $5.1M of notional needs $102,000 of margin
    let mut w = fx_world(0);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 5_100 * MEUR / 1000, T_1400)).unwrap();
    w.sample(0, T_1400 + 2, p4(10_000));
    w.ev.tick(T_1400 + 3);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::InsufficientMargin));
    assert!(w.ev.positions().is_empty());
    // $4.9M needs $98,000: allowed
    w.order(market_order(0, true, 4_900 * MEUR / 1000, T_1400 + 3)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.ev.positions().len(), 1);
}

#[test]
fn eight_positions_at_most() {
    let markets: Vec<_> = (0..9u16).map(|i| common::market(i, 0, &format!("S{i}"), 0)).collect();
    let mut w = World::new(base_rules(markets), S100K, 2, T_1400 - HOUR);
    for i in 0..9u16 {
        w.sample(i, T_1400 + i64::from(i), p4(10_000));
    }
    for i in 0..9u16 {
        w.order(market_order(i, true, MEUR / 100, T_1400 + 8)).unwrap();
    }
    w.ev.tick(T_1400 + 30);
    assert_eq!(w.ev.positions().len(), 8);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::TooManyPositions));
}

#[test]
fn a_flip_remainder_must_be_a_valid_fill_quantity() {
    let mut m = eur_market(0);
    m.half_spread_e6 = 0;
    m.qty_min = 10_000_000;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 100_000_000, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    // sell 105,000,000: the closing slice is 100,000,000 and the remainder 5,000,000 is below the minimum
    w.order(market_order(0, false, 105_000_000, T_1400 + 6)).unwrap();
    w.sample(0, T_1400 + 8, p4(10_000));
    w.ev.tick(T_1400 + 9);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::InvalidRemainder));
    assert_eq!(w.ev.positions()[&0].p.qty, 100_000_000, "nothing happened");
}

#[test]
fn the_breach_lock_blocks_opening_but_not_closing() {
    let mut w = fx_world(0);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    for (dt, p) in [(30, 10_006), (60, 10_012), (90, 10_030), (150, 9_980)] {
        w.sample(0, t + dt, p4(p));
    }
    w.ev.tick(t + 151);
    assert!(w.ev.state().lock_until > t);
    w.order(market_order(0, true, MEUR, t + 151)).unwrap();
    w.sample(0, t + 160, p4(9_981));
    w.ev.tick(t + 161);
    assert_eq!(w.ev.refusals().last().map(|r| r.reason), Some(R::AccountLocked));
    assert!(w.ev.positions().is_empty());
    // a limit order is accepted during the lock (it stays alive, blocked) 
    assert!(w.order(order(IntentKind::Limit, 0, 0, MEUR, p4(9_900), 0, 0, 0, t + 161)).is_ok());
    let acct = w.opened.account;
    let _ = acct;
}

#[test]
fn intent_helper_is_consistent() {
    // the harness's OrderIntent round trips through the evaluator's codec
    let i = OrderIntent { kind: IntentKind::Stop, market: 3, side: 1, qty: 5, price_e12: 6, sl_e12: 7, tp_e12: 8, target_nonce: 9, client_time: 10 };
    assert_eq!(OrderIntent::from_bytes(&i.to_bytes()), Some(i));
}

#[test]
fn accept_all_takes_any_signature() {
    use evaluator::{AcceptAll, Evaluator, IntentVerifier, SignedIntent};
    assert!(AcceptAll.verify(&[0; 32], b"anything", &[0; 64]));
    let mut ev = Evaluator::new(base_rules(vec![eur_market(0)]), params(S100K, 2, T_1400 - HOUR), Box::new(AcceptAll)).unwrap();
    ev.on_sample_mid(0, T_1400, p4(10_000)).unwrap();
    let s = SignedIntent { nonce: 1, order: market_order(0, true, MEUR, T_1400).to_bytes(), sig: [0; 64] };
    assert_eq!(ev.on_order(&s), Ok(()));
}

#[test]
fn a_pending_order_waits_for_the_second_after_its_client_time() {
    let mut w = w0();
    // accepted at T_1400 for a price the sample of the SAME second already satisfies: effective strictly after client_time
    w.order(order(IntentKind::Limit, 0, 0, MEUR / 10, p4(10_000), 0, 0, 0, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    let bs = w.ev.drain_batches(&evaluator::BatchPolicy::MAX);
    let fill_time = common::events_of(&bs).iter().find_map(|e| if let journal_types::JournalEvent::Fill { time, .. } = e { Some(*time) } else { None });
    assert_eq!(fill_time, Some(T_1400 + 5), "the sample at T_1400 itself must not fill it");
    assert_valid(&w, &bs);
}

#[test]
fn a_pending_order_of_another_market_does_not_fill_on_this_markets_sample() {
    let mut m1 = common::market(1, 0, "OTHER", 0);
    m1.max_age_secs = 3_600;
    let mut m0 = eur_market(0);
    m0.half_spread_e6 = 0;
    m0.max_age_secs = 3_600;
    let mut w = World::new(base_rules(vec![m0, m1]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    w.sample(1, T_1400, p4(20_000));
    // a buy limit on market 1 at 1.9000; market 0 trades at 1.0 (which would satisfy it by number)
    w.order(order(IntentKind::Limit, 1, 0, MEUR / 10, p4(19_000), 0, 0, 0, T_1400)).unwrap();
    w.sample(0, T_1400 + 10, p4(10_000));
    w.sample(0, T_1400 + 20, p4(10_001));
    w.ev.tick(T_1400 + 30);
    assert_eq!(w.ev.stats().limit_fills, 0);
    assert_eq!(w.ev.pending().len(), 1);
}

#[test]
fn level_boundaries_are_strict() {
    let mut w = w0();
    let buy = |price, sl, tp| order(IntentKind::Limit, 0, 0, MEUR, price, sl, tp, 0, T_1400);
    let sell = |price, sl, tp| order(IntentKind::Limit, 0, 1, MEUR, price, sl, tp, 0, T_1400);
    let lv = p4(10_000);
    // a level equal to the order's own price is on the wrong side, one step either way is the first valid value
    assert_eq!(w.order(buy(lv, lv, 0)), Err(R::InvalidLevel));
    assert_eq!(w.order(buy(lv, 0, lv)), Err(R::InvalidLevel));
    assert_eq!(w.order(sell(lv, lv, 0)), Err(R::InvalidLevel));
    assert_eq!(w.order(sell(lv, 0, lv)), Err(R::InvalidLevel));
    assert!(w.order(buy(lv, lv - 1, lv + 1)).is_ok());
    assert!(w.order(sell(lv, lv + 1, lv - 1)).is_ok());
}

#[test]
fn notional_cap_is_inclusive_and_a_close_may_take_the_whole_position() {
    let notional = engine::units::notional_micro(MEUR, p4(10_000)).unwrap();
    let mut m = eur_market(0);
    m.max_notional_micro = u64::try_from(notional).unwrap();
    let mut w = World::new(base_rules(vec![m.clone()]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    let lim = |price| order(IntentKind::Limit, 0, 0, MEUR, price, 0, 0, 0, T_1400);
    assert!(w.order(lim(p4(10_000))).is_ok(), "a notional equal to the cap is allowed");
    assert_eq!(w.order(lim(p4(10_001))), Err(R::NotionalCap));
    // closing exactly the position is fine, one step more is not
    let mut w = w0();
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert_eq!(w.order(close_order(0, MEUR + 1_000, T_1400 + 6)), Err(R::InvalidQuantity));
    assert!(w.order(close_order(0, MEUR, T_1400 + 6)).is_ok());
}

#[test]
fn set_levels_one_at_a_time() {
    let mut w = w0();
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    assert!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, p4(9_900), 0, 0, T_1400 + 6)).is_ok(), "stop-loss only");
    assert!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, 0, p4(10_100), 0, T_1400 + 6)).is_ok(), "take-profit only");
    assert!(w.order(order(IntentKind::SetSlTp, 0, 0, 0, 0, 0, 0, 0, T_1400 + 6)).is_ok(), "both cleared");
}

#[test]
fn a_price_exactly_max_age_old_is_still_fresh() {
    let mut m = eur_market(0);
    m.max_age_secs = 60;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    w.ev.tick(T_1400 + 60);
    assert!(w.order(market_order(0, true, MEUR, T_1400 + 60)).is_ok());
    w.ev.tick(T_1400 + 61);
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400 + 61)), Err(R::NoFreshPrice));
}

#[test]
fn a_sample_may_arrive_in_the_current_second_but_not_before_it() {
    let mut w = w0();
    w.ev.tick(T_1400 + 5);
    assert!(w.ev.on_sample_mid(0, T_1400 + 5, p4(10_001)).is_ok());
    assert_eq!(w.ev.on_sample_mid(0, T_1400 + 4, p4(10_002)), Err(evaluator::SampleError::OutOfOrder));
}

#[test]
fn a_market_order_trades_on_the_newest_sample_that_existed_when_it_arrived() {
    let mut m = eur_market(0);
    m.half_spread_e6 = 0;
    m.max_age_secs = 3_600;
    let mut w = World::new(base_rules(vec![m]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400 - 5, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400, p4(10_500)); // arrives after the order, same second: the order must not see it
    w.ev.tick(T_1400 + 2);
    let bs = w.ev.drain_batches(&evaluator::BatchPolicy::MAX);
    let price = common::events_of(&bs).iter().find_map(|e| if let journal_types::JournalEvent::Fill { price_e12, .. } = e { Some(*price_e12) } else { None });
    assert_eq!(price, Some(p4(10_000)));
}

#[test]
fn two_updates_with_the_same_source_time_are_both_taken() {
    let mut w = w0();
    let t = u64::try_from(T_1400 + 3).unwrap() * 1_000_000_000;
    let v = |p| i128::from(p4(p)) * 1_000_000;
    assert!(w.ev.on_sample(0, t + 500, v(10_001), t + 600, [0; 32]).is_ok());
    assert!(w.ev.on_sample(0, t + 500, v(10_002), t + 700, [0; 32]).is_ok(), "equal source times are not 'backwards'");
    assert_eq!(w.ev.on_sample(0, t + 499, v(10_003), t + 800, [0; 32]), Err(evaluator::SampleError::MarketTimeBackwards));
}
