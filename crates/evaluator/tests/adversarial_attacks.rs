//! M12 item 1, evaluator side: DESIGN 13.2 attacks whose control is the evaluator's own engine (SELF-audit by the builder).
//! The program-side half of each attack is in `tests-rs/tests/adversarial.rs` (modules `a09_...` and `a14_...`).
//!
//! * A-9: split a position to dodge the 45 s rule. The timer restarts on every same-direction add, a partial close does not
//!   reset it, a flip starts a new position with a new timer, the boundary is 45 s (quick) and 46 s (not quick). Every
//!   scenario is also run through the public verifier, which must call it VALID.
//! * A-14: the trader presses the button with a position open. `request_pass` rejects with `NotFlat` whatever the chain's
//!   stored `flat` says, and rejects with `JournalBehind` when batches are still queued.
mod common;
use common::*;
use evaluator::{ChainView, PassDecision, PassReject, ReplayCheck};
use journal_types::{CloseReason, JournalEvent};

/// Class 0, half-spread 1e-6, price memory of an hour, S = $100,000, phase 2.
fn w0() -> World {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w
}

/// A market order at second `sec` (sample at that second at `mid`, order dated `sec`, the second is closed by `tick(sec + 1)`).
fn market_at(w: &mut World, sec: i64, long: bool, qty: u64, mid: u64) {
    w.sample(0, sec, mid);
    w.order(market_order(0, long, qty, sec)).expect("order accepted");
    w.ev.tick(sec + 1);
}

/// The Close events of everything drained so far: (qty, timer_start, time, quick, reason).
type CloseRow = (u64, i64, i64, bool, CloseReason);

fn closes(w: &mut World) -> (Vec<CloseRow>, Vec<evaluator::Batch>) {
    let bs = w.drain();
    let v = events_of(&bs)
        .iter()
        .filter_map(|e| if let JournalEvent::Close { qty, timer_start, time, quick, reason, .. } = e { Some((*qty, *timer_start, *time, *quick, *reason)) } else { None })
        .collect();
    (v, bs)
}

/// (quick flags, timer starts) of all closes, for the assertions that follow.
fn quick_and_timer(c: &[CloseRow]) -> Vec<(bool, i64)> {
    c.iter().map(|x| (x.3, x.1)).collect()
}

// ------------------------------------------------------------------------------------------------ A-9

/// DESIGN 13.2 A-9: "Split a position to dodge the 45 s rule | Timer restarts on every add; partial closes do not reset; flip makes
/// a new position." The attacker's idea: build the position early with a tiny order so that the big add looks old, or split the
/// position so that the close seems far from the open.
#[test]
fn a09_the_timer_restarts_on_every_add_so_a_tiny_early_order_does_not_age_the_big_one() {
    // tiny (1,000 units = the market minimum) at T, the big order at T+44, everything closed 6 s later
    let mut w = w0();
    market_at(&mut w, T_1400, true, 1_000, p4(10_000));
    market_at(&mut w, T_1400 + 44, true, 2 * MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 50, false, 2 * MEUR + 1_000, p4(10_010));
    let (c, bs) = closes(&mut w);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, 2 * MEUR + 1_000, "one merged position");
    assert_eq!((c[0].1, c[0].2, c[0].3), (T_1400 + 44, T_1400 + 50, true), "timer = the big add, so the close is quick although the position is 50 s old");
    assert_valid(&w, &bs);

    // the same with 10 slices every 4 seconds: the timer is the LAST slice, whatever came before
    let mut w = w0();
    for k in 0..10i64 {
        market_at(&mut w, T_1400 + 4 * k, true, MEUR / 10, p4(10_000));
    }
    assert_eq!(w.ev.positions()[&0].p.timer_start, T_1400 + 36);
    market_at(&mut w, T_1400 + 36 + 45, false, MEUR, p4(10_010)); // 45 s after the last add: quick (81 s after the first)
    let (c, bs) = closes(&mut w);
    assert_eq!(quick_and_timer(&c), vec![(true, T_1400 + 36)]);
    assert_valid(&w, &bs);

    // one second later it is not quick: the rule is about the last add, nothing else
    let mut w = w0();
    for k in 0..10i64 {
        market_at(&mut w, T_1400 + 4 * k, true, MEUR / 10, p4(10_000));
    }
    market_at(&mut w, T_1400 + 36 + 46, false, MEUR, p4(10_010));
    let (c, bs) = closes(&mut w);
    assert_eq!(quick_and_timer(&c), vec![(false, T_1400 + 36)]);
    assert_valid(&w, &bs);
}

#[test]
fn a09_the_boundary_is_45_seconds_quick_and_46_seconds_not() {
    for (age, quick) in [(1i64, true), (44, true), (45, true), (46, false), (47, false)] {
        let mut w = w0();
        market_at(&mut w, T_1400, true, MEUR, p4(10_000));
        market_at(&mut w, T_1400 + age, false, MEUR, p4(10_010));
        let (c, bs) = closes(&mut w);
        assert_eq!(quick_and_timer(&c), vec![(quick, T_1400)], "age {age}");
        assert_valid(&w, &bs);
    }
}

#[test]
fn a09_a_partial_close_does_not_reset_the_timer() {
    let mut w = w0();
    market_at(&mut w, T_1400, true, 2 * MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 30, false, MEUR, p4(10_005)); // half out after 30 s: quick
    market_at(&mut w, T_1400 + 50, false, MEUR, p4(10_005)); // the rest 50 s after the OPEN (20 s after the partial close): not quick
    let (c, bs) = closes(&mut w);
    assert_eq!(c.iter().map(|x| x.0).collect::<Vec<_>>(), vec![MEUR, MEUR]);
    assert_eq!(quick_and_timer(&c), vec![(true, T_1400), (false, T_1400)], "the second slice keeps the original timer");
    assert_valid(&w, &bs);
}

#[test]
fn a09_a_flip_closes_the_old_position_and_the_new_one_gets_its_own_timer() {
    let mut w = w0();
    market_at(&mut w, T_1400, true, MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 30, false, 2 * MEUR, p4(10_005)); // sells 2 M: closes the 1 M long, opens a 1 M short
    assert_eq!(w.ev.positions()[&0].p.qty, -(MEUR as i64));
    assert_eq!(w.ev.positions()[&0].p.timer_start, T_1400 + 30, "the short starts a new timer");
    market_at(&mut w, T_1400 + 60, true, MEUR, p4(10_005)); // closes the short 30 s after ITS open (60 s after the long's)
    let (c, bs) = closes(&mut w);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].1, c[0].3, c[0].4), (T_1400, true, CloseReason::Flip), "the long closes by the flip, quick");
    assert_eq!((c[1].1, c[1].3), (T_1400 + 30, true), "the short is judged from the flip, not from the first open");
    assert_valid(&w, &bs);
}

// ------------------------------------------------------------------------------------------------ A-14

/// DESIGN 13.2 A-14: "Trader presses the button while a position is open | Chain `flat` can be stale; blocked by `confirm_pass` /
/// `reject_pass` (the evaluator compares with its engine) and by the replay of the watchdog."
fn chain_of(w: &World, b: i64, flat: bool) -> ChainView {
    ChainView { seq: w.ev.seq(), journal_hash: w.ev.head(), b, flat, phase: w.ev.phase() }
}

#[test]
fn a14_request_pass_with_an_open_position_is_rejected_not_flat_whatever_the_chain_says() {
    // b at target through a closed profit, then a NEW position is opened and the trader presses the button
    let mut w = w0();
    market_at(&mut w, T_1400, true, 4 * MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 100, false, 4 * MEUR, p4(10_030)); // +0.3% on 4 M EUR = about $11,990
    assert!(w.ev.state().b >= 108_000_000_000, "b = {}", w.ev.state().b);
    market_at(&mut w, T_1400 + 110, true, MEUR, p4(10_030)); // a position is open again
    let _ = w.drain();
    let b = w.ev.state().b;
    for chain_flat in [true, false] {
        let chain = chain_of(&w, b, chain_flat);
        assert_eq!(w.ev.request_pass(T_1400 + 120, &chain, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat), "chain says flat = {chain_flat}");
    }
}

#[test]
fn a14_the_chain_saying_not_flat_is_enough_to_reject_even_when_the_engine_is_flat() {
    let mut w = w0();
    market_at(&mut w, T_1400, true, 4 * MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 100, false, 4 * MEUR, p4(10_030));
    let bs = w.drain();
    assert!(w.ev.positions().is_empty());
    let b = w.ev.state().b;
    // control: engine flat, chain flat, b at target, replay valid: confirm
    let chain = chain_of(&w, b, true);
    assert_eq!(w.ev.request_pass(T_1400 + 110, &chain, ReplayCheck::Valid), PassDecision::Confirm);
    // the chain's stored flag is false: reject NotFlat (a stale flag is the other half of the staleness)
    let mut w2 = w0();
    market_at(&mut w2, T_1400, true, 4 * MEUR, p4(10_000));
    market_at(&mut w2, T_1400 + 100, false, 4 * MEUR, p4(10_030));
    let _ = w2.drain();
    let chain = chain_of(&w2, b, false);
    assert_eq!(w2.ev.request_pass(T_1400 + 110, &chain, ReplayCheck::Valid), PassDecision::Reject(PassReject::NotFlat));
    assert_valid(&w, &bs);
}

#[test]
fn a14_batches_still_queued_make_the_evaluator_say_journal_behind_before_anything_else() {
    let mut w = w0();
    market_at(&mut w, T_1400, true, MEUR, p4(10_000)); // open position, events not drained
    let chain = ChainView { seq: 0, journal_hash: w.ev.head(), b: S100K, flat: true, phase: 2 };
    assert_eq!(w.ev.request_pass(T_1400 + 5, &chain, ReplayCheck::Skipped), PassDecision::Reject(PassReject::JournalBehind));
}

#[test]
fn a14_the_watchdog_mismatch_blocks_a_pass_that_everything_else_allows() {
    let mut w = w0();
    market_at(&mut w, T_1400, true, 4 * MEUR, p4(10_000));
    market_at(&mut w, T_1400 + 100, false, 4 * MEUR, p4(10_030));
    let _ = w.drain();
    let chain = chain_of(&w, w.ev.state().b, true);
    assert_eq!(w.ev.request_pass(T_1400 + 110, &chain, ReplayCheck::Mismatch), PassDecision::Reject(PassReject::Other));
}
