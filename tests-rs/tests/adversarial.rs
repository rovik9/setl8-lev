//! M12 item 1: DESIGN 13.2 attacks A-1 to A-18 as executable tests (SELF-audit by the builder, not an independent audit).
//!
//! One module per attack (`a01_...` to `a18_...`). Every module starts with the DESIGN row it covers and declares its class:
//!
//! * BLOCKED   the program refuses; the test asserts the exact error code and that account bytes did not change.
//! * LIMITED   the program lets a bounded form of the attack through; the test pins EXACTLY what still passes (and the
//!             first thing that stops), so a later change that widens the hole breaks the test.
//! * OFFCHAIN  the control lives in the evaluator or the public verifier; the test beside it is in
//!             `crates/evaluator/tests/adversarial_attacks.rs` or `crates/verifier/tests/adversarial_attacks.rs`; this file
//!             only pins what the program lets through.
//! * POLICY    no code control; the test shows the visible trace (counter, event, state).
//! * OPERATIONAL no test at all (A-11 upgrade key, A-13 new wallet): the audit report carries a checklist item.
//!
//! Money paths run against the real vault fixture (sha256 42c62670...). A test marked `#[ignore = "FINDING ..."]` asserts the
//! behaviour the DESIGN requires and fails today; `cargo test -- --ignored` shows it.
mod common;

use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::*;
use journal_types::{FailReason, JournalBatch, JournalEvent, PriceProof};
use setl8_lev_trading::{
    constants as c,
    errors::Setl8Error as E,
    events::*,
    instructions::{MarketParams, UpdateMarketArgs},
    state::Market,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

// ------------------------------------------------------------------------------------------------ classes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Blocked,
    Limited,
    OffChain,
    Policy,
    Operational,
}

// ------------------------------------------------------------------------------------------------ shared helpers

const LONG: bool = true;

/// Fresh $100k account, event base time (account time + 1,000 s).
fn base() -> (JEnv, i64) {
    let w = JEnv::new();
    let t = w.t0 + 1_000;
    (w, t)
}

/// A digest of everything a journal or pass call can touch: the vault harness digest (config, tally, wallet, registry, vault
/// state, rule sets, accounts, vault records, all token accounts, the trader's lamports) plus the five markets and the first six
/// payout claims of the trader's current challenge.
fn dig(w: &JEnv) -> [u8; 32] {
    let vts = w.vts();
    let mut extra: Vec<anchor_lang::solana_program::pubkey::Pubkey> = (1..=6u64).map(|i| claim_pda(&vts, i)).collect();
    extra.extend((0..5u16).map(market_pda));
    extra.push(sector_authority_pda());
    w.v.digest(&w.trader.pubkey(), &extra)
}

/// The raw bytes of the active trading account.
fn raw(w: &JEnv) -> Vec<u8> {
    w.v.e.svm.get_account(&w.acct).expect("trading account").data
}

/// Raw bytes and lamports of any account (None when it does not exist).
fn snap(w: &JEnv, k: &anchor_lang::solana_program::pubkey::Pubkey) -> Option<(u64, Vec<u8>, anchor_lang::solana_program::pubkey::Pubkey)> {
    w.v.e.svm.get_account(k).map(|a| (a.lamports, a.data, a.owner))
}

/// Posts `batch` and requires exactly `want` at instruction 1 (instruction 0 is the compute budget), with the account bytes and
/// the whole digest unchanged.
fn rejected(w: &mut JEnv, clock: i64, batch: &JournalBatch, markets: &[u16], want: E) {
    let (d, r0) = (dig(w), raw(w));
    let r = w.post(clock, batch, markets);
    assert_err_at(&r, 1, want);
    assert_eq!(dig(w), d, "a rejected batch changed the world ({want:?})");
    assert_eq!(raw(w), r0, "a rejected batch changed the trading account ({want:?})");
}

/// Same as `rejected` for raw batch bytes.
fn rejected_bytes(w: &mut JEnv, clock: i64, bytes: Vec<u8>, markets: &[u16], want: E) {
    let (d, r0) = (dig(w), raw(w));
    let ev = dup(&w.v.evaluator);
    let r = w.post_bytes(clock, &ev, bytes, markets);
    assert_err_at(&r, 1, want);
    assert_eq!(dig(w), d, "a rejected batch changed the world ({want:?})");
    assert_eq!(raw(w), r0, "a rejected batch changed the trading account ({want:?})");
}

/// A proof of `mid` whose time is `ns` nanoseconds since the epoch.
fn proof_ns(market: u16, ns: u64, mid: u64) -> PriceProof {
    let mut p = proof(market, 0, mid);
    p.timestamp_ns = ns;
    p
}

/// A close of the 1,000,000 EUR long (entry = ask of MID) at the bid of `mid`, event time `t`, a normal (non-quick) slice,
/// price proof index 0.
fn close_eur(t: i64, mid: u64) -> JournalEvent {
    close(MARKET_EUR, LONG, MEUR, eur_ask(), bid(mid, H_EUR), t, t - 1_000, 0)
}

/// The pnl (micro-USD) `close_eur` claims.
fn pnl_eur(mid: u64) -> i64 {
    engine::pnl_micro(LONG, MEUR, eur_ask(), bid(mid, H_EUR)).unwrap()
}

/// `[close_eur(t, mid), Meta(t)]` with one proof.
fn close_batch(w: &JEnv, t: i64, mid: u64, p: PriceProof) -> JournalBatch {
    w.batch(vec![close_eur(t, mid), meta(t, false)], vec![p])
}

/// A long fill of 1,000,000 EUR at the ask of the PROOF mid `mid`, event time `t`, proof index 0.
fn fill_batch(w: &JEnv, t: i64, mid: u64, p: PriceProof) -> JournalBatch {
    w.batch(vec![fill(MARKET_EUR, LONG, MEUR, ask(mid, H_EUR), t, 0), meta(t, false)], vec![p])
}

/// 1% above MID: a "favourable" old price for a long.
const HIGH: u64 = 1_010_000_000_000;
const NS: u64 = 1_000_000_000;

fn secs(t: i64) -> u64 {
    u64::try_from(t).unwrap() * NS
}

fn params_of(m: &Market) -> MarketParams {
    MarketParams {
        status: m.status,
        feed_id: m.feed_id,
        max_age_secs: m.max_age_secs,
        qty_step: m.qty_step,
        qty_min: m.qty_min,
        max_notional_usd: m.max_notional_usd,
        session_count: m.session_count,
        sessions: m.sessions,
        quote_conv_market: m.quote_conv_market,
    }
}

type Pk = anchor_lang::solana_program::pubkey::Pubkey;

// ================================================================================================ A-1

/// DESIGN 13.2 A-1: "Evaluator replays an old favourable price | Limited: `proof.time` must be within `max_age_secs` of the
/// event time and event time never goes backwards. The verifier compares against the sample archive."
///
/// Class LIMITED (on chain) + OFF-CHAIN (the archive comparison is in crates/verifier/tests/adversarial_attacks.rs, test
/// `a01_late_breach_with_an_old_sample`).
mod a01_old_price_replay {
    use super::*;
    pub const CLASS: Class = Class::Limited;

    #[test]
    fn a_price_exactly_max_age_old_is_accepted_and_one_second_older_is_refused() {
        // the test markets have max_age_secs = 60. The old price is 1% above the "current" price MID: the program cannot know.
        let max_age = 60;
        for age in [0i64, 1, 30, 59, max_age] {
            let (mut w, t) = base();
            let before = w.account();
            let b = close_batch(&w, t, HIGH, proof(MARKET_EUR, t - age, HIGH));
            w.post_ok(t + 1, &b, &[MARKET_EUR]);
            let after = w.account();
            assert_eq!(after.b, before.b + pnl_eur(HIGH), "age {age}: the old price is booked in full");
            assert!(pnl_eur(HIGH) > 9_000_000_000, "the replayed price is worth about $9,900 on 1M EUR");
        }
        for age in [max_age + 1, max_age + 2, 3_600, 86_400] {
            let (mut w, t) = base();
            let b = close_batch(&w, t, HIGH, proof(MARKET_EUR, t - age, HIGH));
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofTooOld);
        }
    }

    #[test]
    fn the_age_is_counted_from_whole_seconds_of_the_proof_timestamp() {
        // proof_time_secs = timestamp_ns / 1e9 (floor): 59.999999999 s after the second boundary counts as that second
        let (mut w, t) = base();
        let edge = secs(t - 60);
        let b = close_batch(&w, t, HIGH, proof_ns(MARKET_EUR, edge, HIGH));
        w.post_ok(t + 1, &b, &[MARKET_EUR]);

        let (mut w, t) = base();
        let b = close_batch(&w, t, HIGH, proof_ns(MARKET_EUR, secs(t - 60) + NS - 1, HIGH));
        w.post_ok(t + 1, &b, &[MARKET_EUR]); // still second t-60

        let (mut w, t) = base();
        let b = close_batch(&w, t, HIGH, proof_ns(MARKET_EUR, secs(t - 60) - 1, HIGH));
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofTooOld); // second t-61
    }

    #[test]
    fn the_evaluator_picks_any_price_inside_the_window_for_one_event_time() {
        // For the event time t the proofs of seconds t-60 ... t+2 are all valid. A compromised evaluator chooses the best
        // price of a 63-second window; the program has no "current price" to compare with. This pins the width of that choice.
        let (lo, hi) = (990_000_000_000u64, HIGH);
        let results: Vec<(i64, bool)> = [-61i64, -60, 0, 2, 3]
            .into_iter()
            .map(|off| {
                let (mut w, t) = base();
                let mid = if off <= 0 { hi } else { lo };
                let b = close_batch(&w, t, mid, proof(MARKET_EUR, t + off, mid));
                (off, w.post(t + 5, &b, &[MARKET_EUR]).is_ok())
            })
            .collect();
        assert_eq!(results, vec![(-61, false), (-60, true), (0, true), (2, true), (3, false)]);
    }

    #[test]
    fn one_proof_can_justify_many_events_until_it_ages_out() {
        // There is no "one use" rule: the same proof backs closes at t, t+30 and t+60; at t+61 it is too old.
        let (mut w, t) = base();
        let p = proof(MARKET_EUR, t, HIGH);
        for (i, dt) in [0i64, 30, 60].into_iter().enumerate() {
            let b = close_batch(&w, t + dt, HIGH, p.clone());
            w.post_ok(t + dt + 1, &b, &[MARKET_EUR]);
            assert_eq!(w.account().trades_closed, i as u32 + 1);
        }
        let b = close_batch(&w, t + 61, HIGH, p);
        rejected(&mut w, t + 62, &b, &[MARKET_EUR], E::ProofTooOld);
    }

    #[test]
    fn event_time_never_goes_back_so_the_window_cannot_be_pulled_backwards() {
        let (mut w, t) = base();
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t, MID));
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(w.account().last_batch_time, t);
        // one second before the last batch time: refused (also with a proof that would be fresh for that time)
        let b = close_batch(&w, t - 1, MID, proof(MARKET_EUR, t - 1, MID));
        rejected(&mut w, t + 2, &b, &[MARKET_EUR], E::TimeBackwards);
        // the same second is fine
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t, MID));
        w.post_ok(t + 2, &b, &[MARKET_EUR]);
        // inside one batch: the second event one second before the first is refused
        let (mut w, t) = base();
        let b = w.batch(vec![close_eur(t + 10, MID), close_eur(t + 9, MID), meta(t + 10, false)], vec![proof(MARKET_EUR, t + 10, MID)]);
        rejected(&mut w, t + 12, &b, &[MARKET_EUR], E::TimeBackwards);
    }

    #[test]
    fn proof_and_event_may_run_at_most_two_seconds_ahead_of_the_chain() {
        // proof time = event time + 2 accepted, + 3 refused (ProofInFuture)
        let (mut w, t) = base();
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t + 2, MID));
        w.post_ok(t + 5, &b, &[MARKET_EUR]);
        let (mut w, t) = base();
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t + 3, MID));
        rejected(&mut w, t + 5, &b, &[MARKET_EUR], E::ProofInFuture);
        // event time = cluster time + 2 accepted, + 3 refused (TimeInFuture)
        let (mut w, t) = base();
        let b = close_batch(&w, t + 2, MID, proof(MARKET_EUR, t + 2, MID));
        w.post_ok(t, &b, &[MARKET_EUR]);
        let (mut w, t) = base();
        let b = close_batch(&w, t + 3, MID, proof(MARKET_EUR, t + 3, MID));
        rejected(&mut w, t, &b, &[MARKET_EUR], E::TimeInFuture);
    }

    #[test]
    fn either_admin_alone_can_widen_max_age_without_delay_and_the_window_follows() {
        // The knob of the A-1 control is the market's max_age_secs; DESIGN 14 / decision 24: either admin, no delay, no lock.
        for who in ["sl8", "rov"] {
            let (mut w, t) = base();
            let mut p = params_of(&w.v.e.market(MARKET_EUR));
            p.max_age_secs = u16::MAX;
            let args = UpdateMarketArgs { market_id: MARKET_EUR, params: p };
            let admins = if who == "sl8" { Admins::only_sl8(&w.v.e) } else { Admins::only_rov(&w.v.e) };
            let ix = update_market_ix(admins, &args);
            assert_ok(w.v.e.send_admins(ix, admins));
            assert_eq!(w.v.e.market(MARKET_EUR).max_age_secs, u16::MAX);
            // a proof 65,535 s (18 h 12 min) old is now accepted, 65,536 s is not
            let b = close_batch(&w, t, HIGH, proof(MARKET_EUR, t - 65_535, HIGH));
            w.post_ok(t + 1, &b, &[MARKET_EUR]);
            let (before, b) = (w.account().b, close_batch(&w, t, HIGH, proof(MARKET_EUR, t - 65_536, HIGH)));
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofTooOld);
            assert_eq!(w.account().b, before);
        }
        // a stranger cannot
        let (mut w, _t) = base();
        let stranger = Keypair::new();
        let mut p = params_of(&w.v.e.market(MARKET_EUR));
        p.max_age_secs = u16::MAX;
        let args = UpdateMarketArgs { market_id: MARKET_EUR, params: p };
        let admins = Admins { a: stranger.pubkey(), b: w.v.e.rov.pubkey(), a_signs: true, b_signs: false };
        let mut ix = update_market_ix(admins, &args);
        ix.accounts[1].pubkey = stranger.pubkey();
        let s = dup(&stranger);
        let r = w.v.e.send(&[ix], &[&s]);
        assert_err(&r, E::Unauthorized);
        assert_eq!(w.v.e.market(MARKET_EUR).max_age_secs, 60);
    }
}

// ================================================================================================ A-2

/// DESIGN 13.2 A-2: "Wrong or fake feed id in a proof | Blocked: I-4 checks the market's current feed id."
///
/// Class BLOCKED.
mod a02_wrong_feed_id {
    use super::*;
    pub const CLASS: Class = Class::Blocked;

    fn bad_feed_cases() -> Vec<(&'static str, [u8; 32])> {
        vec![
            ("another market's feed (BTC)", feed("BTC")),
            ("all zero", [0u8; 32]),
            ("all 0xFF", [0xFF; 32]),
            ("the right symbol with another source byte", journal_types::feed_id(&journal_types::NativeSha256, 1, "EURUSD")),
            ("a near miss: the right feed with one bit flipped", {
                let mut f = feed("EURUSD");
                f[31] ^= 1;
                f
            }),
            ("the right feed with the first bit flipped", {
                let mut f = feed("EURUSD");
                f[0] ^= 0x80;
                f
            }),
        ]
    }

    #[test]
    fn a_fill_with_the_proof_of_another_feed_is_refused_in_every_variant() {
        for (name, f) in bad_feed_cases() {
            let (mut w, t) = base();
            let mut p = proof(MARKET_EUR, t, MID);
            p.feed_id = f;
            let b = fill_batch(&w, t, MID, p);
            let (d, r0) = (dig(&w), raw(&w));
            let r = w.post(t + 1, &b, &[MARKET_EUR]);
            assert_err_at(&r, 1, E::ProofWrongFeed);
            assert_eq!((dig(&w), raw(&w)), (d, r0), "{name}");
        }
    }

    #[test]
    fn a_close_with_the_proof_of_another_feed_is_refused() {
        let (mut w, t) = base();
        let b = close_batch(&w, t, MID, proof(MARKET_BTC, t, MID)); // BTC's feed for the EUR market
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
    }

    #[test]
    fn the_floating_events_check_the_feed_against_the_markets_passed_in() {
        // TightStart, FloorRaised and Breach carry a proof but no market: the proof must carry the feed of one of the markets
        // passed. A feed that belongs to NO passed market is refused with ProofWrongFeed, before any number is looked at.
        let (w0, t) = base();
        let alien = proof(MARKET_BTC, t, MID); // BTC is a real market, but not passed in
        let events = [
            JournalEvent::TightStart { time: t, p_open: 1_235_161_000, r: S100K, l_start: usd(96_000), g_peak: 1_235_161_000, l_level: 96_235_161_000, proof: 0 },
            JournalEvent::FloorRaised { time: t, g_peak: 3_035_107_000, l_level: 98_035_107_000, proof: 0 },
            JournalEvent::Breach { time: t, l_level: 98_035_107_000, b_after: 97_935_260_000, proof: 0 },
        ];
        for ev in events {
            let (mut w, _) = base();
            let b = w.batch(vec![ev, meta(t, false)], vec![alien.clone()]);
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
        }
        drop(w0);
    }

    #[test]
    fn the_proof_value_must_be_the_event_price() {
        // a fabricated fill price: the event says 1.0, the proof (right feed) says 1.01
        let (mut w, t) = base();
        let b = w.batch(vec![fill(MARKET_EUR, LONG, MEUR, ask(MID, H_EUR), t, 0), meta(t, false)], vec![proof(MARKET_EUR, t, HIGH)]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
        // one unit (1e-12) off the derived ask
        let (mut w, t) = base();
        let b = w.batch(vec![fill(MARKET_EUR, LONG, MEUR, ask(MID, H_EUR) + 1, t, 0), meta(t, false)], vec![proof(MARKET_EUR, t, MID)]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
        // a fill at the mid itself (no spread) is also not the derived price
        let (mut w, t) = base();
        let b = w.batch(vec![fill(MARKET_EUR, LONG, MEUR, MID, t, 0), meta(t, false)], vec![proof(MARKET_EUR, t, MID)]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
    }

    #[test]
    fn a_remapped_feed_makes_every_proof_of_the_old_feed_worthless() {
        // I-4 uses the market's CURRENT feed id: after update_market the old feed is refused and the new one accepted.
        let (mut w, t) = base();
        let old = feed("EURUSD");
        let new = feed("EURUSD-REMAP");
        let mut p = params_of(&w.v.e.market(MARKET_EUR));
        p.feed_id = new;
        let admins = Admins::only_sl8(&w.v.e);
        let ix = update_market_ix(admins, &UpdateMarketArgs { market_id: MARKET_EUR, params: p });
        assert_ok(w.v.e.send_admins(ix, admins));
        let mut stale = proof(MARKET_EUR, t, MID);
        stale.feed_id = old;
        let b = fill_batch(&w, t, MID, stale);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
        let mut fresh = proof(MARKET_EUR, t, MID);
        fresh.feed_id = new;
        let b = fill_batch(&w, t, MID, fresh);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
    }

    #[test]
    fn unknown_sources_and_malformed_signed_updates_are_refused() {
        // Stork (0) and Pyth (1) are named in DESIGN 3.3 but not built: a proof of those sources is refused.
        for (src, want) in [(0u8, E::ProofUnknownSource), (1, E::ProofUnknownSource), (5, E::ProofUnknownSource), (254, E::ProofUnknownSource)] {
            let (mut w, t) = base();
            let mut p = proof(MARKET_EUR, t, MID);
            p.source = src;
            let b = fill_batch(&w, t, MID, p);
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], want);
        }
        // source 2 must carry an empty signed_update
        let (mut w, t) = base();
        let mut p = proof(MARKET_EUR, t, MID);
        p.signed_update = vec![1, 2, 3];
        let b = fill_batch(&w, t, MID, p);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofBadSignedUpdate);
    }

    #[test]
    fn a_proof_value_that_is_not_a_positive_price_is_refused() {
        for q in [0i128, -1, -1_000_000_000_000_000_000] {
            let (mut w, t) = base();
            let mut p = proof(MARKET_EUR, t, MID);
            p.quantized_value = q;
            let b = fill_batch(&w, t, MID, p);
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofValueInvalid);
        }
    }

    #[test]
    fn a_market_account_that_is_not_the_markets_pda_is_refused() {
        // the feed id the proof is compared with comes from the Market account: a forged copy at another address must not count
        let (mut w, t) = base();
        let real = w.v.e.svm.get_account(&market_pda(MARKET_EUR)).unwrap();
        let fake = Pk::new_unique();
        w.v.e.svm.set_account(fake, real).unwrap();
        let b = fill_batch(&w, t, MID, proof(MARKET_EUR, t, MID));
        let bytes = b.to_bytes().unwrap();
        let ev = dup(&w.v.evaluator);
        w.v.e.set_clock(t + 1);
        // build the instruction by hand with the fake market as the remaining account
        use anchor_lang::{InstructionData, ToAccountMetas};
        let mut metas = setl8_lev_trading::accounts::PostJournal {
            evaluator: ev.pubkey(),
            trading_account: w.acct,
            config: config_pda(),
            ruleset: ruleset_pda(1),
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.push(anchor_lang::solana_program::instruction::AccountMeta::new_readonly(fake, false));
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: PROGRAM_ID,
            accounts: metas,
            data: setl8_lev_trading::instruction::PostJournal { batch_bytes: bytes }.data(),
        };
        let (d, r0) = (dig(&w), raw(&w));
        let r = w.v.e.send(&[ix], &[&ev]);
        assert_err_at(&r, 0, E::MarketUnknown);
        assert_eq!((dig(&w), raw(&w)), (d, r0));
    }
}

// ================================================================================================ A-3

/// DESIGN 13.2 A-3: "Impersonate this program to the vault | Blocked by the vault: `sector_authority` must be a PDA signed by
/// this program id."
///
/// Class BLOCKED (by the vault fixture; this program's side is the seed check of its own `sector_authority` account).
/// A real attacker PROGRAM doing `invoke_signed` is not built here (no SBF attacker binary in this repo): what the vault sees
/// from such a program is the PDA of the attacker's own program id, which the tests below hand to the vault directly; whether
/// a program can sign for ANOTHER program's PDA is the Solana runtime's rule (privilege escalation is refused), not this
/// repo's.
mod a03_impersonate_the_program_to_the_vault {
    use super::*;
    use anchor_lang::solana_program::{instruction::{AccountMeta, Instruction}, system_program};
    use setl8_shared_interfaces as si;
    pub const CLASS: Class = Class::Blocked;

    /// The vault's `Unauthorized` (first variant of its error enum).
    const VAULT_UNAUTHORIZED: u32 = 6000;

    fn attacker_program() -> Pk {
        Pk::new_from_array([0xA7; 32])
    }
    fn attacker_pda() -> Pk {
        si::derive_sector_authority(&attacker_program()).0
    }

    /// The vault's sector instructions for the open challenge of `w`, with `authority` in the `sector_authority` slot.
    fn vault_calls(w: &JEnv, authority: Pk) -> Vec<(&'static str, Instruction)> {
        let a = w.account();
        let (vts, trader) = (w.vts(), a.trader);
        let new_vts = vault_trader_state_pda(&trader, 1);
        vec![
            (
                "record_activity",
                si::record_activity(
                    VAULT_ID,
                    authority,
                    registry_pda(),
                    &[AccountMeta::new(vts, false)],
                    si::RecordActivityArgs { trader_wallet: trader, product_program_id: PROGRAM_ID, challenge_id: a.challenge_id },
                ),
            ),
            (
                "flag_trader_failed",
                si::flag_trader_failed(
                    VAULT_ID,
                    authority,
                    registry_pda(),
                    &[AccountMeta::new(vts, false)],
                    si::FlagTraderFailedArgs { trader_wallet: trader, product_program_id: PROGRAM_ID, challenge_id: a.challenge_id },
                ),
            ),
            (
                "request_payout",
                si::request_payout(
                    VAULT_ID,
                    authority,
                    registry_pda(),
                    &[
                        AccountMeta::new(vts, false),
                        AccountMeta::new(w.v.vault_state, false),
                        AccountMeta::new(claim_pda(&vts, a.next_request_id), false),
                        AccountMeta::new(w.v.e.payer.pubkey(), true),
                        AccountMeta::new_readonly(system_program::ID, false),
                    ],
                    si::RequestPayoutArgs {
                        trader_wallet: trader,
                        amount: PAYOUT_100K,
                        product_program_id: PROGRAM_ID,
                        challenge_id: a.challenge_id,
                        proposed_request_id: a.next_request_id,
                    },
                ),
            ),
            (
                "deposit_fee",
                si::deposit_fee(
                    VAULT_ID,
                    authority,
                    registry_pda(),
                    &[
                        AccountMeta::new(new_vts, false),
                        AccountMeta::new(trader, true),
                        AccountMeta::new_readonly(system_program::ID, false),
                        AccountMeta::new_readonly(w.v.vault_state, false),
                        AccountMeta::new_readonly(trader, true),
                        AccountMeta::new(w.v.trader_token(&trader), false),
                        AccountMeta::new_readonly(w.v.usdc, false),
                        AccountMeta::new(w.v.usdc_pool, false),
                        AccountMeta::new(w.v.vault_sl8_usdc, false),
                        AccountMeta::new_readonly(anchor_spl::token::spl_token::ID, false),
                    ],
                    si::DepositFeeArgs {
                        amount: w.v.ruleset.tier_cost[8],
                        product_program_id: PROGRAM_ID,
                        challenge_id: 1,
                        trader_wallet: trader,
                        account_size: w.v.ruleset.tier_size[8],
                    },
                ),
            ),
        ]
    }

    #[test]
    fn the_vault_refuses_every_sector_call_whose_authority_is_not_this_programs_pda() {
        // Signature checks are off in the vault harness, so a PDA flagged as signer "signs": this is what a hostile PROGRAM
        // with its own seeds would present after a valid invoke_signed of ITS OWN pda. The vault compares the key with
        // derive_sector_authority(registry.product_program_id) and nothing else.
        let (mut w, t) = base();
        w.set_time(t);
        let evaluator = dup(&w.v.evaluator);
        let trader = dup(&w.trader);
        let authorities: Vec<(&str, Pk)> =
            vec![("another program's PDA", attacker_pda()), ("the evaluator key", evaluator.pubkey()), ("the trader key", trader.pubkey())];
        for (who, auth) in authorities {
            for (name, ix) in vault_calls(&w, auth) {
                let d = dig(&w);
                let r = w.v.e.send(&[ix], &[&trader, &evaluator]);
                assert_anchor_code(&r, VAULT_UNAUTHORIZED);
                assert_eq!(dig(&w), d, "{name} with {who} in the authority slot changed the world");
                assert_eq!(w.claim(1), None, "{name}: no payout claim may exist");
            }
        }
        assert_eq!(w.tally().requested_count, 0);
    }

    #[test]
    fn the_same_vault_call_is_accepted_when_the_key_is_this_programs_pda() {
        // control: the refusal above depends on the key and on nothing else (record_activity is harmless to run)
        let (mut w, t) = base();
        w.set_time(t + 90_000);
        let (_, ix) = vault_calls(&w, sector_authority_pda()).remove(0);
        assert_ok(w.v.e.send(&[ix], &[]));
    }

    #[test]
    fn with_real_signature_checks_nobody_can_sign_for_the_authority_pda_from_outside() {
        // A transaction that names this program's sector_authority as a signer needs a signature that nobody can make:
        // the PDA has no private key. With signature verification ON the transaction does not even reach the vault.
        let (mut w, t) = base();
        let svm = std::mem::replace(&mut w.v.e.svm, litesvm::LiteSVM::new());
        w.v.e.svm = svm.with_sigverify(true);
        w.set_time(t + 90_000);
        let trader = dup(&w.trader);
        for (name, ix) in vault_calls(&w, sector_authority_pda()) {
            let d = dig(&w);
            let r = w.v.e.send(&[ix], &[&trader]);
            match &r {
                Err(f) => assert_eq!(f.err, solana_transaction_error::TransactionError::SignatureFailure, "{name}"),
                Ok(_) => panic!("{name}: a transaction without the PDA's signature must not run"),
            }
            assert_eq!(dig(&w), d, "{name}");
        }
        // ... while this program, through invoke_signed, still can: the legitimate path works under real signature checks
        w.ready(2);
        let t2 = t + 90_001;
        w.ok(|w| w.request(t2));
        assert_eq!(w.account().pass_pending, c::PASS_REQUESTED);
    }

    #[test]
    fn this_program_refuses_a_foreign_authority_account_in_its_own_vault_calls() {
        // the converse: this program never lets a caller choose the account it signs with. Every instruction that signs a
        // vault call has `seeds = [SECTOR_AUTHORITY_SEED]` on that account: Anchor ConstraintSeeds (2006).
        let (mut w, t) = base();
        w.ready(2);
        w.fails_anchor(2006, |w| {
            w.set_time(t);
            let mut ix = w.request_ix();
            ix.accounts[4].pubkey = attacker_pda();
            let tr = dup(&w.trader);
            w.v.e.send(&[ix], &[&tr])
        });
        w.set_pending(c::PASS_CONFIRMED, t - 10, t - 5);
        let payer = w.new_payer();
        w.fails_anchor(2006, |w| {
            let mut fa = w.finalize_accts(payer.pubkey());
            fa.sector_authority = attacker_pda();
            w.set_time(t + 2 * 86_400);
            let p = dup(&payer);
            w.v.e.send(&[fa.to_ix()], &[&p])
        });
        w.fails_anchor(2006, |w| {
            w.set_time(t);
            let ev = dup(&w.v.evaluator);
            let mut ix = w.touch_ix(ev.pubkey());
            ix.accounts[4].pubkey = attacker_pda();
            w.v.e.send(&[ix], &[&ev])
        });
    }
}

// ================================================================================================ A-4

/// DESIGN 13.2 A-4: "Anyone but the evaluator posts journal | Blocked: I-1."
///
/// Class BLOCKED.
mod a04_journal_by_anyone_but_the_evaluator {
    use super::*;
    pub const CLASS: Class = Class::Blocked;

    /// Every signer class that is not the evaluator.
    fn outsiders(w: &JEnv) -> Vec<(&'static str, Keypair)> {
        vec![
            ("random key", Keypair::new()),
            ("trader", dup(&w.trader)),
            ("SL8 admin", dup(&w.v.e.sl8)),
            ("Rov admin", dup(&w.v.e.rov)),
            ("fee payer", dup(&w.v.e.payer)),
        ]
    }

    fn fund(w: &mut JEnv, k: &Keypair) {
        w.v.e.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap();
    }

    /// One evaluator-only call per instruction, signed by `k`; returns the result of sending it.
    fn call(w: &mut JEnv, which: &str, k: &Keypair, t: i64) -> litesvm::types::TransactionResult {
        let bytes = simple(w, t, vec![close_eur(t, HIGH)], false).to_bytes().unwrap();
        let fail_bytes = w.fail_batch(t, vec![], FailReason::StaticFloor, 0, true).to_bytes().unwrap();
        w.set_time(t + 1);
        let s = dup(k);
        match which {
            "post_journal" => w.post_bytes(t + 1, &s, bytes, &[MARKET_EUR]),
            "report_fail" => {
                let ix = w.report_fail_ix(s.pubkey(), fail_bytes, 0, &[MARKET_EUR]);
                w.v.e.send(&[ix], &[&s])
            }
            "confirm_pass" => {
                let ix = w.confirm_ix(s.pubkey());
                w.v.e.send(&[ix], &[&s])
            }
            "reject_pass" => {
                let ix = w.reject_ix(s.pubkey(), 0);
                w.v.e.send(&[ix], &[&s])
            }
            "hold_account" => {
                let ix = w.hold_ix(s.pubkey(), hash(7));
                w.v.e.send(&[ix], &[&s])
            }
            "touch_activity" => {
                let ix = w.touch_ix(s.pubkey());
                w.v.e.send(&[ix], &[&s])
            }
            other => panic!("{other}"),
        }
    }

    const EVALUATOR_ONLY: [&str; 6] = ["post_journal", "report_fail", "confirm_pass", "reject_pass", "hold_account", "touch_activity"];

    /// The instruction index of the failing instruction: post_journal is sent after a compute-budget instruction.
    fn idx(which: &str) -> u8 {
        u8::from(which == "post_journal")
    }

    #[test]
    fn every_evaluator_only_instruction_refuses_every_other_signer() {
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(c::PASS_REQUESTED, t - 10, 0); // so confirm and reject would otherwise be possible
        for (who, k) in outsiders(&w) {
            fund(&mut w, &k);
            for which in EVALUATOR_ONLY {
                let (d, r0) = (dig(&w), raw(&w));
                let r = call(&mut w, which, &k, t);
                assert_err_at(&r, idx(which), E::Unauthorized);
                assert_eq!((dig(&w), raw(&w)), (d, r0), "{which} by {who}");
            }
        }
        // the control: the evaluator itself is accepted (hold is the harmless one)
        let ev = dup(&w.v.evaluator);
        assert_ok(call(&mut w, "hold_account", &ev, t));
        assert_eq!(w.account().held, 1);
    }

    #[test]
    fn a_missing_signature_is_refused_by_the_framework_before_the_handler() {
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(c::PASS_REQUESTED, t - 10, 0);
        let ev = dup(&w.v.evaluator);
        for which in ["confirm_pass", "reject_pass", "hold_account", "touch_activity"] {
            let (d, r0) = (dig(&w), raw(&w));
            w.set_time(t + 1);
            let mut ix = match which {
                "confirm_pass" => w.confirm_ix(ev.pubkey()),
                "reject_pass" => w.reject_ix(ev.pubkey(), 0),
                "hold_account" => w.hold_ix(ev.pubkey(), hash(7)),
                _ => w.touch_ix(ev.pubkey()),
            };
            ix.accounts[0].is_signer = false; // the evaluator's key is named but did not sign
            let r = w.v.e.send(&[ix], &[]);
            assert_anchor_code(&r, 3010);
            assert_eq!((dig(&w), raw(&w)), (d, r0), "{which}");
        }
        // post_journal: same, with the compute-budget instruction in front
        let (d, r0) = (dig(&w), raw(&w));
        let bytes = simple(&w, t, vec![close_eur(t, HIGH)], false).to_bytes().unwrap();
        w.set_time(t + 1);
        use anchor_lang::{InstructionData, ToAccountMetas};
        let mut metas = setl8_lev_trading::accounts::PostJournal {
            evaluator: ev.pubkey(),
            trading_account: w.acct,
            config: config_pda(),
            ruleset: ruleset_pda(1),
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas[0].is_signer = false;
        metas.extend(JEnv::market_metas(&[MARKET_EUR]));
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: PROGRAM_ID,
            accounts: metas,
            data: setl8_lev_trading::instruction::PostJournal { batch_bytes: bytes }.data(),
        };
        let r = w.v.e.send(&[ix], &[]);
        assert_anchor_code(&r, 3010);
        assert_eq!((dig(&w), raw(&w)), (d, r0));
    }

    #[test]
    fn a_forged_config_account_naming_the_attacker_as_evaluator_is_not_the_config() {
        // the evaluator key is read from the config PDA; a copy at another address (attacker as evaluator) fails the seeds check
        let (mut w, t) = base();
        let attacker = Keypair::new();
        fund(&mut w, &attacker);
        let mut cfg = w.v.e.svm.get_account(&config_pda()).unwrap();
        cfg.data[72..104].copy_from_slice(attacker.pubkey().as_ref()); // admin_a(32) admin_b(32) evaluator(32) after the discriminator
        let fake = Pk::new_unique();
        w.v.e.svm.set_account(fake, cfg).unwrap();
        let bytes = simple(&w, t, vec![close_eur(t, HIGH)], false).to_bytes().unwrap();
        use anchor_lang::{InstructionData, ToAccountMetas};
        let mut metas = setl8_lev_trading::accounts::PostJournal {
            evaluator: attacker.pubkey(),
            trading_account: w.acct,
            config: fake,
            ruleset: ruleset_pda(1),
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(JEnv::market_metas(&[MARKET_EUR]));
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: PROGRAM_ID,
            accounts: metas,
            data: setl8_lev_trading::instruction::PostJournal { batch_bytes: bytes }.data(),
        };
        let (d, r0) = (dig(&w), raw(&w));
        w.set_time(t + 1);
        let r = w.v.e.send(&[ix], &[&attacker]);
        assert_anchor_code(&r, 2006);
        assert_eq!((dig(&w), raw(&w)), (d, r0));
    }

    #[test]
    fn after_a_rotation_the_old_evaluator_is_an_outsider_and_the_new_one_is_in() {
        let (mut w, t) = base();
        let old = dup(&w.v.evaluator);
        let new_ev = Keypair::new();
        fund(&mut w, &new_ev);
        // one admin alone, the old evaluator, a stranger: none of them can rotate
        let other = Keypair::new();
        for (who, metas) in [("SL8 alone", only_sl8(&w.v.e)), ("Rov alone", only_rov(&w.v.e))] {
            let ix = set_evaluator_ix(metas, new_ev.pubkey());
            let r = w.v.e.send(&[ix], &[&dup(&w.v.e.sl8), &dup(&w.v.e.rov)]);
            assert_err(&r, E::Unauthorized);
            assert_eq!(w.v.e.config().evaluator, old.pubkey(), "{who}");
        }
        for k in [&old, &other] {
            let metas = admin_metas((k.pubkey(), true), (w.v.e.rov.pubkey(), false));
            let ix = set_evaluator_ix(metas, new_ev.pubkey());
            let r = w.v.e.send(&[ix], &[k]);
            assert_err(&r, E::Unauthorized);
        }
        assert_eq!(w.v.e.config().evaluator, old.pubkey());
        // both admins rotate
        let ix = set_evaluator_ix(both(&w.v.e), new_ev.pubkey());
        assert_ok(w.v.e.send_as_admins(&[ix]));
        assert_eq!(w.v.e.config().evaluator, new_ev.pubkey());
        // the old key is refused everywhere, the new key posts
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t, MID));
        let bytes = b.to_bytes().unwrap();
        for which in EVALUATOR_ONLY {
            let (d, r0) = (dig(&w), raw(&w));
            let r = call(&mut w, which, &old, t);
            assert_err_at(&r, idx(which), E::Unauthorized);
            assert_eq!((dig(&w), raw(&w)), (d, r0), "old evaluator {which}");
        }
        let r = w.post_bytes(t + 1, &new_ev, bytes, &[MARKET_EUR]);
        assert_ok(r);
        assert_eq!(w.account().seq, 2);
    }
}

// ================================================================================================ A-5

/// DESIGN 13.2 A-5: "Trader fabricates his own pass | Blocked: only the evaluator posts the journal that gives him `b` and
/// `flat`."
///
/// Class BLOCKED on chain. The program cannot know that `b` and `flat` are TRUE (that is R-1); what a TRADER cannot do is
/// tested here. A pass fabricated by the evaluator is A-17 / A-18 and the verifier tests.
mod a05_trader_fabricates_a_pass {
    use super::*;
    pub const CLASS: Class = Class::Blocked;

    #[test]
    fn the_trader_cannot_post_a_journal_that_gives_him_a_profit() {
        let (mut w, t) = base();
        let tr = dup(&w.trader);
        // a perfectly valid batch (it would be accepted from the evaluator): +$9,939.70
        let batch = close_batch(&w, t, HIGH, proof(MARKET_EUR, t, HIGH));
        let (d, r0) = (dig(&w), raw(&w));
        let r = w.post_bytes(t + 1, &tr, batch.to_bytes().unwrap(), &[MARKET_EUR]);
        assert_err_at(&r, 1, E::Unauthorized);
        assert_eq!((dig(&w), raw(&w)), (d, r0));
        assert_eq!(w.account().b, S100K);
        // and with the same batch the evaluator is accepted: the signer is the only difference
        w.post_ok(t + 1, &batch, &[MARKET_EUR]);
        assert_eq!(w.account().b, S100K + pnl_eur(HIGH));
    }

    #[test]
    fn the_trader_cannot_confirm_his_own_request_and_nothing_pays_in_state_one() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        let tr = dup(&w.trader);
        let (d, r0) = (dig(&w), raw(&w));
        w.set_time(t + 1);
        let ix = w.confirm_ix(tr.pubkey());
        let r = w.v.e.send(&[ix], &[&tr]);
        assert_err(&r, E::Unauthorized);
        assert_eq!((dig(&w), raw(&w)), (d, r0));
        // finalize in state 1: refused, whoever pays, however late
        let payer = w.new_payer();
        w.fails(E::NoPendingPass, |w| w.finalize(t + 30 * 86_400, &payer));
        assert_eq!(w.claim(1), None);
        assert_eq!(w.tally().requested_count, 0);
        assert_eq!(w.registry_totals(), (0, 0), "the vault has queued nothing");
    }

    #[test]
    fn request_advance_rereads_the_stored_account_and_each_missing_condition_has_its_own_error() {
        let cases: Vec<(&str, Box<dyn Fn(&mut JEnv)>, E)> = vec![
            ("one micro below 1.08 S", Box::new(|w| w.edit_account(|a| a.b = 108_000_000_000 - 1)), E::TargetNotReached),
            ("not flat", Box::new(|w| w.edit_account(|a| a.flat = 0)), E::NotFlat),
            ("tight mode on", Box::new(|w| w.edit_account(|a| a.tight_on = 1)), E::TightModeOn),
            ("balance at the static floor (phase failed)", Box::new(|w| w.edit_account(|a| a.b = 92_000_000_000)), E::PhaseFailed),
            ("quick tally above 0.028 S", Box::new(|w| w.edit_account(|a| a.q_tally = 2_800_000_001)), E::PhaseFailed),
            ("a pass already pending", Box::new(|w| w.set_pending(c::PASS_REQUESTED, 1, 0)), E::PassAlreadyPending),
            ("held", Box::new(|w| w.edit_account(|a| a.held = 1)), E::AccountHeld),
            ("account failed", Box::new(|w| w.edit_account(|a| a.status = c::STATUS_FAILED)), E::AccountNotActive),
        ];
        for (name, setup, want) in cases {
            let (mut w, t) = base();
            w.ready(2);
            setup(&mut w);
            w.fails(want, |w| w.request(t));
            assert_eq!(w.claim(1), None, "{name}");
        }
        // exactly 1.08 S, flat: accepted (state 1 only; no money)
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        assert_eq!(w.account().pass_pending, c::PASS_REQUESTED);
        assert_eq!(w.claim(1), None);
    }

    #[test]
    fn another_traders_signature_cannot_request_for_my_account() {
        let (mut w, t) = base();
        w.ready(2);
        let other = w.v.new_trader();
        w.fails(E::Unauthorized, |w| {
            w.set_time(t);
            let mut ix = w.request_ix();
            ix.accounts[0].pubkey = other.pubkey();
            let o = dup(&other);
            w.v.e.send(&[ix], &[&o])
        });
    }

    #[test]
    fn nothing_the_trader_can_call_moves_the_closed_balance() {
        // The trader's instructions: open_account, reset_account, request_advance, cancel_pass, release_account, close_account.
        // After every one that can run on an Active account the stored b, q_tally, latch, flat and seq are unchanged.
        let (mut w, t) = base();
        let before = w.account();
        w.ready(2);
        let ready = w.account();
        w.ok(|w| w.request(t));
        w.ok(|w| w.cancel(t + 3_600));
        w.ok(|w| w.release_acct(t + 100_000));
        let after = w.account();
        assert_eq!((after.b, after.q_tally, after.latch, after.flat, after.seq, after.journal_hash), (ready.b, ready.q_tally, ready.latch, ready.flat, ready.seq, ready.journal_hash));
        assert_eq!(before.seq, after.seq);
    }
}

// @@APPEND@@

#[test]
fn attack_table_is_complete() {}
