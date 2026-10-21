//! M12 item 10: tests added because a mutant of an instruction survived the audit suites (killed only by pre-existing functional
//! tests). Each test names the mutant it kills. They assert behaviour the DESIGN states; none is a copy-paste of an old test's only
//! assertion, and each fails when its control is removed.
mod common;

use common::journal::*;
use common::pass::*;
use common::*;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E, events::AccountAbandoned};

fn base() -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.set_limits(172_800, 100_000_000_000, 86_400);
    (w, t)
}

/// post_journal.rs `check_open_allowed`: `delete match arm MARKET_STATUS_REDUCE_ONLY` (and the disabled arm): a Fill in a reduce-only or
/// disabled market is refused with the market\'s own error, a Close in the same market is accepted (a position is never trapped).
#[test]
fn a_fill_in_a_reduce_only_or_disabled_market_is_refused_by_name_and_a_close_is_not() {
    let (mut w, t) = base();
    let f = |m: u16, tt: i64| fill(m, true, MEUR, ask(MID, H_OTHER), tt, 0);
    for (market, want) in [(MARKET_REDUCE_ONLY, E::MarketReduceOnly), (MARKET_DISABLED, E::MarketDisabled)] {
        let b = w.batch(vec![f(market, t), meta(t, false)], vec![proof(market, t, MID)]);
        w.post_rejected(t + 1, &b, &[market], want);
        let cl = close(market, true, MEUR, MID, bid(MID, H_OTHER), t, t - 100, 0);
        let b = w.batch(vec![cl, meta(t, false)], vec![proof(market, t, MID)]);
        w.post_ok(t + 1, &b, &[market]);
    }
}

/// post_journal.rs `required_proof`: `replace match guard p != NO_PROOF with true` and `decode_err`: `delete match arm BadProofIndex`:
/// a decisive event with the "no proof" index is `ProofMissing`; a proof index beyond the batch\'s proofs is `ProofIndexOutOfRange`
/// (the batch does not decode); both leave the world byte-identical.
#[test]
fn a_decisive_event_without_a_proof_or_with_a_missing_one_is_refused_by_name() {
    let (mut w, t) = base();
    for ev in [fill(MARKET_EUR, true, MEUR, eur_ask(), t, 255), close(MARKET_EUR, true, MEUR, MID, eur_bid(), t, t - 100, 255)] {
        let b = w.batch(vec![ev, meta(t, false)], vec![]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofMissing);
    }
    for idx in [1u8, 2, 7, 254] {
        let ev = fill(MARKET_EUR, true, MEUR, eur_ask(), t, idx).to_bytes();
        let raw = raw_batch(1, w.account().journal_hash, &[ev, meta(t, false).to_bytes()], 2, &[p_eur(t).to_bytes().unwrap()], 1);
        let before = w.digest();
        let key = dup(&w.v.evaluator);
        let r = w.post_bytes(t + 1, &key, raw, &[MARKET_EUR]);
        assert_err_at(&r, 1, E::ProofIndexOutOfRange);
        assert_eq!(w.digest(), before, "proof index {idx}");
    }
}

/// activity.rs `push_activity`: `replace < with <=` / `with ==` on the throttle: a push exactly ACTIVITY_THROTTLE_SECS (86,000 s) after
/// the last one goes to the vault, one second earlier is skipped (a harmless success that changes nothing), for `touch_activity` too.
#[test]
fn touch_activity_pushes_at_exactly_the_throttle_and_skips_one_second_before() {
    let (mut w, _) = base();
    let now = w.t0 + 100_000;
    let v0 = w.vault_status().last_activity_timestamp;
    w.edit_account(|a| a.last_activity_pushed = now - (c::ACTIVITY_THROTTLE_SECS - 1));
    let before = w.account();
    w.ok(|w| w.touch(now));
    assert_eq!(w.vault_status().last_activity_timestamp, v0, "skipped: the vault was not called");
    assert_eq!(w.account().last_activity_pushed, before.last_activity_pushed);
    w.edit_account(|a| a.last_activity_pushed = now - c::ACTIVITY_THROTTLE_SECS);
    w.ok(|w| w.touch(now));
    assert_eq!((w.vault_status().last_activity_timestamp, w.account().last_activity_pushed), (now, now), "pushed");
}

/// activity.rs `handle_touch_activity` / `handle_release_account`: `replace == with !=` on `push == Push::Abandoned`: the
/// `AccountAbandoned` event is emitted exactly once when the push learns that the vault abandoned the challenge, and NOT on an ordinary push.
#[test]
fn account_abandoned_is_emitted_exactly_when_the_push_finds_an_abandoned_challenge() {
    for via_touch in [true, false] {
        // an ordinary push (the vault stays happy): no event
        let (mut w, _) = base();
        let ordinary = w.t0 + 100_000;
        w.edit_account(|a| a.last_activity_pushed = ordinary - c::ACTIVITY_THROTTLE_SECS);
        let m = w.ok(|w| if via_touch { w.touch(ordinary) } else { w.release_acct(ordinary) });
        assert!(cpi_events::<AccountAbandoned>(&m).is_empty(), "an ordinary push is not an abandonment");
        assert_eq!(w.account().status, c::STATUS_ACTIVE);
        // the vault's 7-day idle rule: exactly one event, the account is Abandoned
        let (mut w, _) = base();
        let limit = w.t0 + 604_800;
        let m = w.ok(|w| if via_touch { w.touch(limit + 1) } else { w.release_acct(limit + 1) });
        assert_eq!(cpi_events::<AccountAbandoned>(&m).len(), 1, "via_touch={via_touch}");
        assert_eq!(w.account().status, c::STATUS_ABANDONED);
    }
}
