//! P29c: the typed lifecycle (handover/runtime-hosting-design.md 7, handover/runtime-api.md "Lifecycle"), as the phase-end contract
//! table of 7.3 plays out: a scripted account from phase 1 to graduation (a reject-style resolve, a veto, a cancel, a hold and its release,
//! finalizations, `end_externally`) run through the evaluator AND the public verifier (VALID), refused calls that change nothing, the
//! breach lock kept across a phase change on both sides, and everything that must continue across a phase change.
mod common;
use common::bytes_gate::StateView;
use common::runner::*;
use common::*;
use evaluator::*;

const AMOUNT: u64 = 8_000_000_000; // 0.08 S of S100K

struct Flow {
    w: World,
    items: Vec<Item>,
    t: i64,
}

impl Flow {
    fn new(phase: u8) -> Flow {
        let mut m = eur_market(0);
        m.half_spread_e6 = TEST_HALF_SPREAD_E6;
        m.max_age_secs = 3_600;
        Flow { w: World::new(base_rules(vec![m]), S100K, phase, T_1400 - HOUR), items: Vec::new(), t: T_1400 }
    }
    fn account(&self) -> [u8; 32] {
        self.w.opened.account
    }
    fn line(&mut self, l: String) {
        self.items.push(Item::Line(l));
    }
    fn post(&mut self) {
        for b in self.w.ev.drain_batches(&BatchPolicy::MAX) {
            self.items.push(Item::Batch(b));
        }
    }
    /// The +8.5 % winner of the other pass tests, started at `self.t`; afterwards flat, B = 1.085 S, journal posted.
    fn winner(&mut self) -> i64 {
        let t = self.t;
        self.w.sample(0, t, p4(10_000));
        self.w.order(market_order(0, true, MEUR, t)).unwrap();
        self.w.sample(0, t + 100, p4(10_085));
        self.w.order(close_order(0, 0, t + 100)).unwrap();
        self.w.sample(0, t + 110, p4(10_085));
        self.w.ev.tick(t + 111);
        self.post();
        t + 112
    }
    fn view(&self) -> ChainView {
        ChainView { seq: self.w.ev.seq(), journal_hash: self.w.ev.head(), b: self.w.ev.state().b, flat: true, phase: self.w.ev.phase() }
    }
    /// `PassRequested` on chain and the evaluator's answer (Confirm).
    fn request(&mut self, at: i64) -> ChainView {
        let v = self.view();
        let amount = if v.phase == 1 { 0 } else { AMOUNT };
        let line = pass_requested_line(&self.account(), v.phase, amount, &v, at);
        self.line(line);
        assert_eq!(self.w.ev.request_pass(at, &v, ReplayCheck::Valid), PassDecision::Confirm);
        assert!(self.w.ev.is_frozen());
        v
    }
    /// The chain's view after a transition into `phase`: the journal head unchanged, a fresh balance, flat.
    fn after(&self, v: &ChainView, phase: u8) -> ChainView {
        ChainView { b: S100K, flat: true, phase, ..*v }
    }
    fn wait_and_finalize(&mut self, v: &ChainView, requested_at: i64) -> i64 {
        let phase = v.phase;
        let eligible = requested_at + 1 + PASS_DELAY_SECS;
        let line = pass_confirmed_line(&self.account(), phase, AMOUNT, v, eligible);
        self.line(line);
        self.w.ev.tick(eligible);
        eligible
    }
}

/// Everything a refused lifecycle call must leave alone, as plain data.
fn snapshot(w: &World) -> (StateView, usize, usize, bool) {
    (StateView::of(&w.ev), w.ev.archive_bytes().len(), w.ev.intents_bytes().len(), w.ev.is_frozen())
}

#[test]
fn the_phase_end_contract_from_phase_1_to_graduation_is_valid_for_the_verifier() {
    let mut f = Flow::new(1);
    let acct = f.account();

    // ---- phase 1: PassRequested, then confirm_pass starts phase 2 at once (PhaseAdvanced 1 -> 2)
    let at = f.winner();
    let v = f.request(at);
    // before the transition every wrong call is refused and changes nothing
    let before = snapshot(&f.w);
    assert_eq!(f.w.ev.phase_started(3, &f.after(&v, 3)), Err(LifecycleError::WrongPhase { have: 3, want: 2 }));
    assert_eq!(f.w.ev.phase_started(1, &f.after(&v, 1)), Err(LifecycleError::WrongPhase { have: 1, want: 2 }));
    assert_eq!(f.w.ev.phase_started(2, &ChainView { b: v.b, ..f.after(&v, 2) }), Err(LifecycleError::ChainMismatch { what: "b" }));
    assert_eq!(f.w.ev.phase_started(2, &ChainView { seq: v.seq + 1, ..f.after(&v, 2) }), Err(LifecycleError::ChainMismatch { what: "seq" }));
    assert_eq!(snapshot(&f.w), before, "refused calls change nothing");
    f.line(phase_advanced_line(&acct, 1, 2, at + 1));
    let (seq, head, trades) = (f.w.ev.seq(), f.w.ev.head(), f.w.ev.trades_closed());
    let (archive_len, intents_len) = (f.w.ev.archive_bytes().len(), f.w.ev.intents_bytes().len());
    let archive_before = f.w.ev.archive_bytes().to_vec();
    assert_eq!(f.w.ev.phase_started(2, &f.after(&v, 2)), Ok(()));
    assert_eq!((f.w.ev.phase(), f.w.ev.is_frozen(), f.w.ev.state().b), (2, false, S100K));
    assert_eq!((f.w.ev.seq(), f.w.ev.head(), f.w.ev.trades_closed()), (seq, head, trades), "the journal continues untouched");
    assert_eq!((f.w.ev.archive_bytes().len(), f.w.ev.intents_bytes().len()), (archive_len, intents_len));
    assert_eq!(f.w.ev.archive_bytes(), &archive_before[..]);
    assert_eq!(f.w.ev.pass_resolved(), Err(LifecycleError::NotFrozen), "the pass is over");
    f.t = at + 100;

    // ---- phase 2: PassRequested, PassConfirmed, VETO (PassVetoed -> pass_resolved), then a second request that is finalized (2 -> 3)
    let at = f.winner();
    let v = f.request(at);
    f.wait_and_finalize(&v, at); // PassConfirmed line, then the delay elapses ...
    f.line(format!("{{\"type\":\"pass_vetoed\",\"account\":\"{}\",\"phase\":2,\"admin_a\":\"a\",\"admin_b\":\"b\",\"slot\":9}}\n", log_b58(&acct))); // ... but both admins veto first
    assert_eq!(f.w.ev.pass_resolved(), Ok(()));
    assert!(!f.w.ev.is_frozen() && f.w.ev.phase() == 2);
    let at2 = at + PASS_DELAY_SECS + 100;
    let v = f.request(at2);
    let tf = f.wait_and_finalize(&v, at2);
    f.line(pass_finalized_line(&acct, 2, 3, AMOUNT, 1, tf));
    f.line(phase_advanced_line(&acct, 2, 3, tf));
    assert_eq!(f.w.ev.phase_started(3, &f.after(&v, 3)), Ok(()));
    assert_eq!(f.w.ev.phase(), 3);
    f.t = tf + 100;

    // ---- phase 3: a hold and its release (AccountHeld / HoldReleased), then a finalization (3 -> 4)
    f.line(log_hold(&acct, f.t, true));
    assert_eq!(f.w.ev.set_held(true), Ok(()));
    let before = snapshot(&f.w);
    assert_eq!(f.w.ev.set_held(true), Err(LifecycleError::AlreadyHeld));
    assert_eq!(snapshot(&f.w), before);
    f.line(log_hold(&acct, f.t + 1, false));
    assert_eq!(f.w.ev.set_held(false), Ok(()));
    assert_eq!(f.w.ev.set_held(false), Err(LifecycleError::NotHeld));
    f.t += 10;
    let at = f.winner();
    let v = f.request(at);
    let tf = f.wait_and_finalize(&v, at);
    f.line(pass_finalized_line(&acct, 3, 4, AMOUNT, 2, tf));
    f.line(phase_advanced_line(&acct, 3, 4, tf));
    assert_eq!(f.w.ev.phase_started(4, &f.after(&v, 4)), Ok(()));
    f.t = tf + 100;

    // ---- phase 4: PassCancelled (the trader withdrew an unanswered request), then a finalization (4 -> 5)
    let at = f.winner();
    let _ = f.request(at);
    f.line(format!("{{\"type\":\"pass_cancelled\",\"account\":\"{}\",\"phase\":4,\"time\":{}}}\n", log_b58(&acct), at + CANCEL_AFTER_SECS));
    assert_eq!(f.w.ev.pass_resolved(), Ok(()));
    let at2 = at + CANCEL_AFTER_SECS + 100;
    let v = f.request(at2);
    let tf = f.wait_and_finalize(&v, at2);
    f.line(pass_finalized_line(&acct, 4, 5, AMOUNT, 3, tf));
    f.line(phase_advanced_line(&acct, 4, 5, tf));
    assert_eq!(f.w.ev.phase_started(5, &f.after(&v, 5)), Ok(()));
    f.t = tf + 100;

    // ---- phase 5: PassFinalized to phase 0 (graduation): phase_started has no phase to start, the host ends the evaluator
    let at = f.winner();
    let v = f.request(at);
    let tf = f.wait_and_finalize(&v, at);
    f.line(pass_finalized_line(&acct, 5, 0, AMOUNT, 4, tf));
    let before = snapshot(&f.w);
    assert_eq!(f.w.ev.phase_started(5, &f.after(&v, 5)), Err(LifecycleError::WrongPhase { have: 5, want: 0 }));
    assert_eq!(f.w.ev.phase_started(0, &f.after(&v, 0)), Err(LifecycleError::WrongPhase { have: 0, want: 0 }));
    assert_eq!(snapshot(&f.w), before);
    assert_eq!(f.w.ev.end_externally(), Ok(()));
    assert!(f.w.ev.is_ended());
    assert_eq!(f.w.ev.end_externally(), Err(LifecycleError::AlreadyEnded));
    assert_eq!(f.w.ev.pass_resolved(), Err(LifecycleError::Ended));
    assert_eq!(f.w.ev.set_held(true), Err(LifecycleError::Ended));
    assert_eq!(f.w.ev.phase_started(2, &f.after(&v, 2)), Err(LifecycleError::Ended));

    // ---- the verifier reads the whole history and says VALID
    let rep = verify_items(&f.w, &f.items, None, None);
    assert!(rep.verdict == setl8_verify::Verdict::Valid, "the scripted history:\n{}", describe(&rep));
}

fn log_b58(key: &[u8; 32]) -> String {
    setl8_verify::log::base58(key)
}

fn log_hold(key: &[u8; 32], t: i64, held: bool) -> String {
    setl8_verify::log::hold_line(key, t, held)
}

/// An abandonment (`AccountAbandoned`, or `ViolationFail` on a failed account) ends the evaluator from any state.
#[test]
fn abandonment_and_a_violation_fail_end_the_evaluator_even_after_a_fail() {
    let mut f = Flow::new(2);
    f.w.sample(0, f.t, p4(10_000));
    f.w.ev.tick(f.t + 5);
    assert_eq!(f.w.ev.end_externally(), Ok(()));
    assert_eq!(f.w.ev.end_externally(), Err(LifecycleError::AlreadyEnded));
    // a failed account (the evaluator's own report_fail) that both admins then fail for violation
    let mut g = Flow::new(2);
    g.w.sample(0, g.t, p4(10_000));
    g.w.order(market_order(0, true, 45_000_000_000_000 / 10, g.t)).unwrap();
    g.w.sample(0, g.t + 1, p4(10_000));
    g.w.sample(0, g.t + 10, p4(9_000));
    g.w.ev.tick(g.t + 11);
    assert!(matches!(g.w.ev.status(), Status::Failed(_)), "a static-floor fail");
    assert_eq!(g.w.ev.set_held(true), Err(LifecycleError::NotActive), "hold_account needs an active account");
    assert_eq!(g.w.ev.pass_resolved(), Err(LifecycleError::NotActive));
    assert_eq!(g.w.ev.end_externally(), Ok(()));
}

/// design 7.3: the 12-hour breach lock is KEPT across a phase change, by the program, by the evaluator and by the verifier.
#[test]
fn the_breach_lock_is_kept_across_a_phase_change_on_both_sides() {
    let mut f = Flow::new(1);
    let acct = f.account();
    f.winner(); // B = 1.085 S, flat, tight mode ended
    // a second trade: +10 % lifts the tight floor to ~1.99 S, the fall back to 1.00 breaches it: closed with B ~ 1.085 S, a 12 h lock
    let u = f.t + 200;
    f.w.sample(0, u, p4(10_000));
    f.w.order(market_order(0, true, MEUR, u)).unwrap();
    f.w.sample(0, u + 30, p4(11_000));
    f.w.sample(0, u + 60, p4(10_000));
    f.w.ev.tick(u + 61);
    f.post();
    let lock = f.w.ev.state().lock_until;
    assert_eq!(lock, u + 60 + 43_200, "the breach locked the account for 12 hours");
    assert!(f.w.ev.state().b >= S100K / 100 * 108, "the balance passes the target in spite of the breach: {}", f.w.ev.state().b);
    assert!(f.w.ev.positions().is_empty() && !f.w.ev.state().tight_on);
    // the pass is requested inside the lock and confirmed (phase 1 -> 2)
    let v = f.request(u + 62);
    f.line(phase_advanced_line(&acct, 1, 2, u + 63));
    assert_eq!(f.w.ev.phase_started(2, &f.after(&v, 2)), Ok(()));
    assert_eq!(f.w.ev.state().lock_until, lock, "the evaluator keeps the lock");
    assert_eq!(f.w.ev.state().b, S100K, "the rest of the phase state is fresh");
    // an opening order during the lock is refused at execution by the evaluator ...
    f.w.sample(0, u + 70, p4(10_000));
    f.w.order(market_order(0, true, MEUR, u + 70)).unwrap();
    f.w.sample(0, u + 72, p4(10_000));
    f.w.ev.tick(u + 73);
    assert_eq!(f.w.ev.refusals().last().map(|r| r.reason), Some(OrderReject::AccountLocked));
    f.post();
    // ... and the verifier, replaying the same history, ends with the same lock (the snapshot is compared field by field, lock_until included)
    let batches: Vec<Batch> = f.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.clone()) } else { None }).collect();
    let snap = snapshot_json(&f.w.ev, &batches, 2, 0);
    assert!(snap.contains(&format!("\"lock_until\":{lock}")));
    let rep = verify_items(&f.w, &f.items, Some(&snap), None);
    assert!(rep.verdict == setl8_verify::Verdict::Valid, "{}", describe(&rep));
    // the comparison has teeth: a snapshot that says the lock was cleared by the phase change is a MISMATCH
    let cleared = snap.replace(&format!("\"lock_until\":{lock}"), "\"lock_until\":0");
    let rep = verify_items(&f.w, &f.items, Some(&cleared), None);
    assert!(rep.verdict != setl8_verify::Verdict::Valid, "the verifier must keep the lock too");
}
