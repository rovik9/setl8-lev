mod common;

use anchor_lang::solana_program::pubkey::Pubkey;
use common::*;
use setl8_lev_trading::errors::Setl8Error;
use solana_keypair::Keypair;
use solana_signer::Signer;

// ------------------------------------------------------------------ set_evaluator

#[test]
fn set_evaluator_succeeds_with_both_admins() {
    let mut e = Env::initialised();
    let new_key = Pubkey::new_unique();
    let ix = set_evaluator_ix(both(&e), new_key);
    assert_ok(e.send_as_admins(&[ix]));
    assert_eq!(e.config().evaluator, new_key);
}

#[test]
fn set_evaluator_fails_with_one_admin_signer() {
    let mut e = Env::initialised();
    let before = e.config().evaluator;
    for metas in [only_sl8(&e), only_rov(&e)] {
        let ix = set_evaluator_ix(metas, Pubkey::new_unique());
        assert_err(&e.send_as_admins(&[ix]), Setl8Error::Unauthorized);
    }
    assert_eq!(e.config().evaluator, before);
}

#[test]
fn set_evaluator_fails_for_non_admins() {
    let mut e = Env::initialised();
    let (x, y) = (Keypair::new(), Keypair::new());
    let metas = admin_metas((x.pubkey(), true), (y.pubkey(), true));
    let ix = set_evaluator_ix(metas, Pubkey::new_unique());
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
}

#[test]
fn set_evaluator_rejects_the_default_key() {
    let mut e = Env::initialised();
    let before = e.config().evaluator;
    let ix = set_evaluator_ix(both(&e), Pubkey::default());
    assert_err(&e.send_as_admins(&[ix]), Setl8Error::Unauthorized);
    assert_eq!(e.config().evaluator, before);
}

// -------------------------------------------------------------------- set_limits

#[test]
fn set_limits_succeeds_with_both_admins() {
    let mut e = Env::initialised();
    let ix = set_limits_ix(both(&e), 259_200, 7_000_000, 43_200);
    assert_ok(e.send_as_admins(&[ix]));
    let c = e.config();
    assert_eq!((c.pass_delay_secs, c.max_payout_per_window, c.window_secs), (259_200, 7_000_000, 43_200));
}

#[test]
fn set_limits_fails_with_one_admin_signer() {
    let mut e = Env::initialised();
    let before = e.config();
    for metas in [only_sl8(&e), only_rov(&e)] {
        let ix = set_limits_ix(metas, 259_200, 7_000_000, 43_200);
        assert_err(&e.send_as_admins(&[ix]), Setl8Error::Unauthorized);
    }
    let after = e.config();
    assert_eq!(after.pass_delay_secs, before.pass_delay_secs);
    assert_eq!(after.max_payout_per_window, before.max_payout_per_window);
    assert_eq!(after.window_secs, before.window_secs);
}

#[test]
fn set_limits_boundary_values_succeed() {
    for delay in [86_400u32, 518_400] {
        let mut e = Env::initialised();
        let ix = set_limits_ix(both(&e), delay, 1, 1);
        assert_ok(e.send_as_admins(&[ix]));
        let c = e.config();
        assert_eq!((c.pass_delay_secs, c.max_payout_per_window, c.window_secs), (delay, 1, 1));
    }
}

#[test]
fn set_limits_rejects_out_of_range_values() {
    let mut e = Env::initialised();
    let before = e.config();
    for (d, cap, w) in [
        (86_399u32, 1u64, 1u32),
        (1_209_601, 1, 1),
        (172_800, 0, 86_400),
        (172_800, 1, 0),
        (0, 1, 1),
        (u32::MAX, 1, 1),
    ] {
        let ix = set_limits_ix(both(&e), d, cap, w);
        assert_err(&e.send_as_admins(&[ix]), Setl8Error::InvalidLimits);
    }
    let after = e.config();
    assert_eq!(after.pass_delay_secs, before.pass_delay_secs);
    assert_eq!(after.max_payout_per_window, before.max_payout_per_window);
    assert_eq!(after.window_secs, before.window_secs);
}

// -------------------------------------------------------------------- set_paused

#[test]
fn either_admin_can_pause() {
    for pick in 0..2 {
        let mut e = Env::initialised();
        let metas = if pick == 0 { only_sl8(&e) } else { only_rov(&e) };
        let ix = set_paused_ix(metas, true);
        assert_ok(e.send_as_admins(&[ix]));
        assert_eq!(e.config().paused, 1);
    }
}

#[test]
fn one_admin_cannot_unpause_but_both_can() {
    let mut e = Env::initialised();
    let ix = set_paused_ix(both(&e), true);
    assert_ok(e.send_as_admins(&[ix]));
    assert_eq!(e.config().paused, 1);

    for metas in [only_sl8(&e), only_rov(&e)] {
        let ix = set_paused_ix(metas, false);
        assert_err(&e.send_as_admins(&[ix]), Setl8Error::Unauthorized);
        assert_eq!(e.config().paused, 1);
    }

    let ix = set_paused_ix(both(&e), false);
    assert_ok(e.send_as_admins(&[ix]));
    assert_eq!(e.config().paused, 0);
}

#[test]
fn non_admin_cannot_pause_or_unpause() {
    let mut e = Env::initialised();
    let (x, y) = (Keypair::new(), Keypair::new());
    let junk = || admin_metas((x.pubkey(), true), (y.pubkey(), true));

    let ix = set_paused_ix(junk(), true);
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    assert_eq!(e.config().paused, 0);

    // a stranger signing next to one real admin who did NOT sign is still not enough to unpause
    let ix = set_paused_ix(both(&e), true);
    assert_ok(e.send_as_admins(&[ix]));
    let ix = set_paused_ix(junk(), false);
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    assert_eq!(e.config().paused, 1);
}

#[test]
fn a_non_admin_signer_cannot_stand_in_for_one_admin() {
    let mut e = Env::initialised();
    let stranger = Keypair::new();
    // real SL8 signs; the Rov slot holds a stranger who signs
    let metas = admin_metas((e.sl8.pubkey(), true), (stranger.pubkey(), true));
    let sl8 = dup(&e.sl8);
    let ix = set_paused_ix(metas.clone(), true);
    // pausing needs one real admin: SL8 signed, so this works
    assert_ok(e.send(&[ix], &[&sl8, &stranger]));
    // unpausing needs both: the stranger is not the stored Rov key
    let ix = set_paused_ix(metas, false);
    assert_err(&e.send(&[ix], &[&sl8, &stranger]), Setl8Error::Unauthorized);
    assert_eq!(e.config().paused, 1);
}

#[test]
fn admin_instructions_fail_before_init() {
    let mut e = Env::new();
    let ix = set_paused_ix(both(&e), true);
    let r = e.send_as_admins(&[ix]);
    assert!(r.is_err());
}

/// Decision 67 (F-30): the delay range is 1 to 6 days. Hand-written boundaries: 6 days accepted, 6 days + 1 s refused (and 14 days, the old
/// maximum, and u32::MAX), 1 day accepted, 1 day - 1 s and 0 refused; a refusal changes nothing and is `InvalidLimits`.
#[test]
fn set_limits_delay_boundaries_are_one_day_to_six_days() {
    for (delay, ok) in [(0u32, false), (1, false), (86_399, false), (86_400, true), (86_401, true), (518_399, true), (518_400, true), (518_401, false), (604_800, false), (1_209_600, false), (u32::MAX, false)] {
        let mut e = Env::initialised();
        let before = e.config();
        let ix = set_limits_ix(both(&e), delay, 1, 1);
        let r = e.send_as_admins(&[ix]);
        if ok {
            assert_ok(r);
            assert_eq!(e.config().pass_delay_secs, delay);
        } else {
            assert_err(&r, Setl8Error::InvalidLimits);
            assert_eq!(e.config().pass_delay_secs, before.pass_delay_secs, "a refused delay {delay} changed the config");
        }
    }
}
