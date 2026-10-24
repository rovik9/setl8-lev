//! M8 (`reset_account`): the evaluator of the NEW account starts in the phase the failed account was in, with fresh state.
//! `Evaluator::new(rules, AccountParams { phase: p, .. })` is the whole mechanism (the same one the differential campaign
//! has always used to start paths in phases 2 to 5); nothing in the evaluator needed to change for M8, these tests pin it.
mod common;
use common::*;
use evaluator::*;

fn world_in(phase: u8) -> World {
    World::new(base_rules(vec![eur_market(0)]), S100K, phase, T_1400 - HOUR)
}

#[test]
fn an_account_started_in_phase_p_has_the_fresh_state_of_that_phase_for_every_p() {
    for p in 1..=5u8 {
        let w = world_in(p);
        assert_eq!(w.ev.phase(), p);
        assert_eq!(*w.ev.state(), engine::reset_phase_state(S100K), "phase {p}");
        assert_eq!((w.ev.seq(), w.ev.head(), w.ev.status()), (0, journal_types::genesis_hash(&journal_types::NativeSha256, &account_key()), Status::Active));
        assert!(w.ev.positions().is_empty());
        let st = w.ev.state();
        assert_eq!((st.b, st.q_tally, st.latch, st.tight_on, st.flat, st.lock_until), (S100K, 0, false, false, true, 0));
    }
}

#[test]
fn phase_0_and_phase_6_are_not_phases() {
    for p in [0u8, 6, 255] {
        let r = Evaluator::new(base_rules(vec![eur_market(0)]), params(S100K, p, T_1400), Box::new(Ed25519Check));
        assert!(matches!(r, Err(ConfigError::BadPhase)), "phase {p}");
    }
}

/// B after a `winner_in`: the long fills at the ask of mid 1.0 (`ASK_AT_ONE`), the close sells at the bid of mid 1.0085,
/// `1_008_500_000_000 - 1_008_500 = 1_008_498_991_500`; qty = 1 MEUR makes the pnl in micro-USD the price difference:
/// 1_008_498_991_500 - 1_000_001_000_000 = 8_497_991_500 (no commission, no rollover).
const WINNER_B: i64 = S100K + 8_497_991_500;

/// A +8.5% (mid) winner in `p`: the pass flow of the new account is the ordinary one of that phase.
fn winner_in(phase: u8) -> (World, i64) {
    let mut m = eur_market(0);
    m.half_spread_e6 = TEST_HALF_SPREAD_E6;
    m.max_age_secs = 3_600;
    let mut w = World::new(base_rules(vec![m]), S100K, phase, T_1400 - HOUR);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 100, p4(10_085));
    w.order(close_order(0, 0, t + 100)).unwrap();
    w.sample(0, t + 110, p4(10_085));
    w.ev.tick(t + 111);
    (w, t + 111)
}

#[test]
fn a_phase_p_start_trades_passes_and_the_verifier_agrees_for_every_p() {
    for p in 1..=5u8 {
        let (mut w, t) = winner_in(p);
        let bs = w.ev.drain_batches(&BatchPolicy::MAX);
        let v = ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b: w.ev.state().b, flat: true, phase: w.ev.phase() };
        assert_eq!(v.b, WINNER_B, "phase {p}");
        // the verifier calls the history VALID from the phase-p start
        let rep = verify(&w, &bs);
        assert!(rep.verdict == setl8_verify::Verdict::Valid, "phase {p}:\n{}", describe(&rep));
        // the evaluator confirms in phase p (and only a request carrying phase p)
        assert_eq!(w.ev.request_pass(t, &ChainView { phase: if p == 1 { 2 } else { p - 1 }, ..v }, ReplayCheck::Valid), PassDecision::Reject(PassReject::Other), "a request in another phase");
        w.ev.pass_resolved().unwrap();
        assert_eq!(w.ev.request_pass(t, &v, ReplayCheck::Valid), PassDecision::Confirm, "phase {p}");
        if p == 5 {
            // phase 5 graduates: `phase_started` has no phase to start (it names 0 as the expected one), the host ends the evaluator
            assert_eq!(w.ev.phase_started(5, &ChainView { b: S100K, ..v }), Err(LifecycleError::WrongPhase { have: 5, want: 0 }));
            assert_eq!(w.ev.end_externally(), Ok(()));
            assert!(w.ev.is_ended());
        } else {
            // the next phase starts fresh
            assert_eq!(w.ev.phase_started(p + 1, &ChainView { b: S100K, phase: p + 1, ..v }), Ok(()));
            assert_eq!((w.ev.phase(), w.ev.state().b), (p + 1, S100K));
        }
    }
}

#[test]
fn the_new_account_of_a_reset_is_independent_of_the_failed_one() {
    // the failed account: a quick-limit fail in phase 3
    let mut m0 = eur_market(0);
    m0.half_spread_e6 = TEST_HALF_SPREAD_E6;
    m0.max_age_secs = 3_600;
    let mut a = World::new(base_rules(vec![m0]), S100K, 3, T_1400 - HOUR);
    let t = T_1400;
    a.sample(0, t, p4(10_000));
    a.order(market_order(0, true, MEUR, t)).unwrap();
    a.sample(0, t + 30, p4(10_060));
    a.order(close_order(0, 0, t + 30)).unwrap();
    a.sample(0, t + 40, p4(10_060));
    a.ev.tick(t + 41);
    assert!(matches!(a.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
    let failed_phase = a.ev.phase();
    assert_eq!(failed_phase, 3);
    let a_batches = a.ev.drain_batches(&BatchPolicy::MAX);
    assert_valid(&a, &a_batches);

    // the reset: a NEW account (other address, challenge 1) in the same phase, fresh state, a new journal and intents file
    let mut p2 = params(S100K, failed_phase, t + 5_000);
    p2.account = [0xA8; 32];
    p2.challenge_id = 1;
    let mut m = eur_market(0);
    m.half_spread_e6 = TEST_HALF_SPREAD_E6;
    m.max_age_secs = 3_600;
    let mut b = World::from_params(base_rules(vec![m]), p2);
    assert_eq!((b.ev.phase(), *b.ev.state(), b.ev.seq()), (3, engine::reset_phase_state(S100K), 0));
    assert_ne!(b.ev.head(), a.ev.head());
    assert_eq!(b.ev.head(), journal_types::genesis_hash(&journal_types::NativeSha256, &[0xA8; 32]));
    // an order signed for the failed account's address is not accepted by the new one (the signature binds the account)
    let stale = sign(&account_key(), 1, &market_order(0, true, MEUR, t + 5_100));
    b.ev.tick(t + 5_100);
    assert_eq!(b.ev.on_order(&stale), Err(OrderReject::BadSignature));
    // the new account trades on its own: VALID, and the failed one is still a failed one
    b.sample(0, t + 5_100, p4(10_000));
    b.order(market_order(0, true, MEUR, t + 5_100)).unwrap();
    b.sample(0, t + 5_200, p4(10_085));
    b.order(close_order(0, 0, t + 5_200)).unwrap();
    b.sample(0, t + 5_210, p4(10_085));
    b.ev.tick(t + 5_211);
    let b_batches = b.ev.drain_batches(&BatchPolicy::MAX);
    assert_eq!(b.ev.state().b, WINNER_B);
    assert_valid(&b, &b_batches);
    assert!(matches!(a.ev.status(), Status::Failed(_)));
    assert_eq!(a.order(market_order(0, true, MEUR, t + 5_300)), Err(OrderReject::AccountEnded));
}
