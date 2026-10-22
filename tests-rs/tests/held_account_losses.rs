//! P25a, decision D-F32 precondition (audit finding F-32), pinned again by P25c (decision 67 amendment). The owner asked for
//! `report_fail` to be refused on a HELD account, but only if a held account is frozen for trading. It is NOT: a held account refuses
//! new exposure (`Fill`, `AccountHeld`), but it still accepts `Fee` events (holding fees, commissions) and `Close` events at a loss,
//! so its balance can fall to the floor while held. Refusing `report_fail` there would leave an account below its floor that can
//! neither post a flat batch (`MustReportFail`) nor be failed until both admins release it, so `report_fail` stays allowed (P25a STOP).
//! P25c (option (a)): `report_fail` on a held account fails it but KEEPS the hold, so the review is not lost; the new behaviour is
//! tested in `held_fail_review.rs`.
mod common;

use common::journal::*;
use common::lifecycle::*;
use common::*;
use journal_types::{FailReason, FeeKind};
use setl8_lev_trading::errors::Setl8Error as E;

fn held() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.ok(|w| w.hold(t, hash(1)));
    (w, t + 10)
}

#[test]
fn a_held_account_refuses_new_exposure() {
    let (mut w, t) = held();
    let f = fill(MARKET_EUR, true, MEUR, eur_ask(), t, 0);
    let b = w.batch(vec![f, meta(t, false)], vec![p_eur(t)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::AccountHeld);
}

#[test]
fn a_held_account_still_accepts_fee_events_and_the_balance_falls() {
    let (mut w, t) = held();
    let b0 = w.account().b;
    let fee_ev = fee(MARKET_EUR, 1_000_000_000, t);
    assert!(matches!(fee_ev, journal_types::JournalEvent::Fee { kind: FeeKind::Commission, .. }));
    let b = w.batch(vec![fee_ev, meta(t, false)], vec![]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().b, b0 - 1_000_000_000, "a $1,000 fee was booked on a held account");
}

#[test]
fn a_held_account_still_accepts_a_close_at_a_loss() {
    let (mut w, t) = held();
    let b0 = w.account().b;
    let c = close(MARKET_EUR, true, MEUR, eur_bid() + 5_000_000_000, eur_bid(), t, t - 100, 0);
    let b = w.batch(vec![c, meta(t, true)], vec![p_eur(t)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert!(w.account().b < b0, "the close booked a loss on a held account");
}

#[test]
fn a_held_account_below_the_floor_is_failed_by_report_fail_and_the_hold_is_kept() {
    let (mut w, t) = held();
    let b = w.fail_batch(t, vec![], FailReason::StaticFloor, S100K / 100 * 92, true);
    w.edit_account(|a| a.b = S100K / 100 * 92);
    assert_ok(w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
    let a = w.account();
    assert_eq!((a.status, a.held), (setl8_lev_trading::constants::STATUS_FAILED, 1), "Failed, and the hold stays for the admins' review");
    assert_eq!(w.wallet_profile().held_count, 1);
}
