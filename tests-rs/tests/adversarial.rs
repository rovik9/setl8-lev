//! M12 item 1: DESIGN 13.2 attacks A-1 to A-18 as executable tests (SELF-audit by the builder, not an independent audit).
//!
//! One module per attack (`a01_...` to `a18_...`). Every module starts with the DESIGN row it covers and declares its class:
//!
//! * BLOCKED: the program refuses; the test asserts the exact error code and that account bytes did not change.
//! * LIMITED: the program lets a bounded form of the attack through; the test pins EXACTLY what still passes (and the
//!   first thing that stops), so a later change that widens the hole breaks the test.
//! * OFFCHAIN: the control lives in the evaluator or the public verifier; the test beside it is in
//!   `crates/evaluator/tests/adversarial_attacks.rs` or `crates/verifier/tests/adversarial_attacks.rs`; this file
//!   only pins what the program lets through.
//! * POLICY: no code control; the test shows the visible trace (counter, event, state).
//! * OPERATIONAL: no fake test (A-11 upgrade key: none; A-13 new wallet: only the cap fact); the audit report carries a
//!   checklist item (the `CHECKLIST` constants in the modules).
//!
//! Counts: BLOCKED 6, LIMITED 5, OFFCHAIN 3, POLICY 2, OPERATIONAL 2 (18 attacks; `attack_table_is_complete` checks this line
//! against the table at the end of the file).
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
        type Case = (&'static str, Box<dyn Fn(&mut JEnv)>, E);
        let cases: Vec<Case> = vec![
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

// ================================================================================================ A-6

/// DESIGN 13.2 A-6: "Cross-account hedging: wallet A long, wallet B short, one passes | Prohibited (decision 36, 2.9): backend
/// detects and holds, both admins review, then fail and ban. Residual: detection is heuristic (...) and fresh wallets can slip
/// through; a false match holds an honest trader until review; a ban is per wallet and a new wallet evades it. HIGH severity
/// economics: the loser costs a reset of 100% to 130% of the tier price; the winner collects $8,000 per phase."
///
/// Class OFFCHAIN. Detection is backend work and is NOT in this repo (nothing here compares positions of two accounts or two
/// wallets). What exists, and is tested here: (1) the program lets opposite positions in the same market stand, in two wallets
/// and in two accounts of one wallet, and pays the winner; (2) the on-chain half of the response: `hold_account` (evaluator),
/// `release_hold` / `fail_for_violation` (both admins), the wallet ban, and the fresh wallet that is not banned.
mod a06_cross_account_hedging {
    use super::*;
    pub const CLASS: Class = Class::OffChain;

    const CAP: u64 = 100_000_000_000;
    const DELAY: i64 = 172_800;
    /// +0.5% (a hedge pair moves the same way for both sides, one up one down).
    const MOVED: u64 = 1_005_000_000_000;

    fn short_fill(t: i64) -> JournalEvent {
        fill(MARKET_EUR, false, MEUR, eur_bid(), t, 0)
    }

    /// Wallet B: a second trader with its own $100k account in the same world.
    fn second_wallet(w: &mut JEnv) -> (Keypair, Pk) {
        let b = w.v.new_trader();
        assert_ok(w.v.open(&b, 8));
        let acct = acct_pda(&b.pubkey(), 0);
        (b, acct)
    }

    /// Runs `f` with `w` pointed at (trader, account) and puts the original back.
    fn on<R>(w: &mut JEnv, trader: &mut Keypair, acct: &mut Pk, f: impl FnOnce(&mut JEnv) -> R) -> R {
        std::mem::swap(&mut w.trader, trader);
        std::mem::swap(&mut w.acct, acct);
        let r = f(w);
        std::mem::swap(&mut w.trader, trader);
        std::mem::swap(&mut w.acct, acct);
        r
    }

    fn open_hedge(w: &mut JEnv, t: i64) {
        let b = w.batch(vec![fill(MARKET_EUR, true, MEUR, eur_ask(), t, 0), meta(t, false)], vec![proof(MARKET_EUR, t, MID)]);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
    }
    fn open_hedge_short(w: &mut JEnv, t: i64) {
        let b = w.batch(vec![short_fill(t), meta(t, false)], vec![proof(MARKET_EUR, t, MID)]);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
    }

    #[test]
    fn two_wallets_hold_opposite_positions_in_one_market_and_the_program_sees_nothing_wrong() {
        let (mut w, t) = base();
        let (mut tb, mut ab) = second_wallet(&mut w);
        let b_before = snap(&w, &ab);
        open_hedge(&mut w, t); // wallet A: long 1M EUR
        assert_eq!(snap(&w, &ab), b_before, "A's journal did not touch B's account");
        on(&mut w, &mut tb, &mut ab, |w| open_hedge_short(w, t)); // wallet B: short 1M EUR, same second, same size
        let a = w.account();
        let b = on(&mut w, &mut tb, &mut ab, |w| w.account());
        assert_eq!((a.seq, a.flat, a.b, a.status), (2, 0, S100K, c::STATUS_ACTIVE));
        assert_eq!((b.seq, b.flat, b.b, b.status), (2, 0, S100K, c::STATUS_ACTIVE));
        assert_ne!(a.trader, b.trader, "two wallets");

        // the price moves 0.5%: A closes its long at a profit, B closes its short at a loss
        let t2 = t + 100;
        let pa = engine::pnl_micro(LONG, MEUR, eur_ask(), bid(MOVED, H_EUR)).unwrap();
        let pb = engine::pnl_micro(false, MEUR, eur_bid(), ask(MOVED, H_EUR)).unwrap();
        let ca = close(MARKET_EUR, LONG, MEUR, eur_ask(), bid(MOVED, H_EUR), t2, t, 0);
        let ba = w.batch(vec![ca, meta(t2, true)], vec![proof(MARKET_EUR, t2, MOVED)]);
        w.post_ok(t2 + 1, &ba, &[MARKET_EUR]);
        on(&mut w, &mut tb, &mut ab, |w| {
            let cb = close(MARKET_EUR, false, MEUR, eur_bid(), ask(MOVED, H_EUR), t2, t, 0);
            let bb = w.batch(vec![cb, meta(t2, true)], vec![proof(MARKET_EUR, t2, MOVED)]);
            w.post_ok(t2 + 1, &bb, &[MARKET_EUR]);
        });
        let a = w.account();
        let b = on(&mut w, &mut tb, &mut ab, |w| w.account());
        assert_eq!((a.b, b.b), (S100K + pa, S100K + pb));
        assert!(pa > 0 && pb < 0);
        // the pair as a whole loses exactly the two round-trip spreads: hedging costs the spread, nothing else
        let spreads = i64::try_from((eur_ask() - eur_bid()) + (ask(MOVED, H_EUR) - bid(MOVED, H_EUR))).unwrap();
        assert_eq!(pa + pb, -spreads);
    }

    #[test]
    fn the_winner_of_a_hedge_pair_is_paid_the_full_formula_amount_by_the_program() {
        // This is the harm: nothing on chain links wallet A to wallet B. A earns its pass (written into the account here, the
        // real route is the journal of the test above), the program pays 0.08 S through the real vault.
        let (mut w, t) = base();
        let (mut tb, mut ab) = second_wallet(&mut w);
        w.set_limits(DELAY as u32, CAP, 86_400);
        open_hedge(&mut w, t);
        on(&mut w, &mut tb, &mut ab, |w| open_hedge_short(w, t));
        let b_acct = snap(&w, &ab);
        w.ready(2);
        let payer = w.new_payer();
        w.ok(|w| w.request(t + 10));
        w.ok(|w| w.confirm(t + 11));
        w.ok(|w| w.finalize(t + 11 + DELAY, &payer));
        assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
        assert_eq!(w.tally().requested_total, PAYOUT_100K);
        assert_eq!(snap(&w, &ab), b_acct, "wallet B's account is not looked at");
    }

    #[test]
    fn opposite_positions_in_two_accounts_of_one_wallet_are_equally_legal() {
        let (mut w, _) = base();
        let first = w.acct;
        let second_id = w.open_another();
        let second = w.acct;
        assert_ne!(first, second);
        let t = w.account().created_at.max(w.t0) + 1_000;
        w.acct = first;
        open_hedge(&mut w, t);
        w.acct = second;
        open_hedge_short(&mut w, t);
        assert_eq!(w.wallet_profile().active_count, 2);
        assert_eq!((w.account().seq, w.account().challenge_id), (2, second_id));
        w.acct = first;
        assert_eq!(w.account().seq, 2);
    }

    #[test]
    fn the_on_chain_response_hold_then_both_admins_fail_and_ban_the_wallet() {
        let (mut w, t) = base();
        let (mut tb, mut ab) = second_wallet(&mut w);
        open_hedge(&mut w, t);
        on(&mut w, &mut tb, &mut ab, |w| open_hedge_short(w, t));
        w.set_limits(DELAY as u32, CAP, 86_400);
        // detection output (backend) -> hold by the evaluator. A random key cannot hold.
        let stranger = Keypair::new();
        w.v.e.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 5);
            let ix = w.hold_ix(stranger.pubkey(), hash(9));
            w.v.e.send(&[ix], &[&dup(&stranger)])
        });
        w.fails(E::EvidenceMissing, |w| w.hold(t + 5, [0u8; 32]));
        w.ok(|w| w.hold(t + 6, hash(9)));
        assert_eq!((w.account().held, w.wallet_profile().held_count), (1, 1));
        // a held account cannot ask for a pass; a confirmed one cannot finalize (the delay clock keeps running)
        w.ready(2);
        w.fails(E::AccountHeld, |w| w.request(t + 7));
        w.set_pending(c::PASS_CONFIRMED, t, t);
        let payer = w.new_payer();
        w.fails(E::AccountHeld, |w| w.finalize(t + DELAY + 100, &payer));
        // one admin cannot fail it or release it
        for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e)] {
            w.fails(E::Unauthorized, |w| {
                w.set_time(t + 20);
                let ix = w.violation_ix(admins, hash(9));
                w.v.e.send_admins(ix, admins)
            });
            w.fails(E::Unauthorized, |w| {
                w.set_time(t + 20);
                let ix = w.release_hold_ix(admins);
                w.v.e.send_admins(ix, admins)
            });
        }
        // both admins: the account fails for good, the wallet is banned, the vault is told, no payout exists
        let b_acct = snap(&w, &ab);
        w.ok(|w| w.violation(t + 30, hash(9)));
        let a = w.account();
        assert_eq!((a.status, a.pass_pending, a.held), (c::STATUS_FAILED, c::PASS_VIOLATION, 0));
        let wl = w.wallet_profile();
        assert_eq!((wl.banned, wl.active_count, wl.held_count), (1, 0, 0));
        assert_eq!(w.vault_status().status, 2, "vault: Failed");
        assert_eq!(w.claim(1), None);
        let tr = dup(&w.trader);
        assert_err(&w.v.open(&tr, 8), E::WalletBanned);
        assert_eq!(snap(&w, &ab), b_acct, "wallet B is not touched: the match was only on A");
        // LIMITED: a ban is per wallet. A fresh wallet opens at once.
        let fresh = w.v.new_trader();
        assert_ok(w.v.open(&fresh, 8));
    }

    #[test]
    fn a_false_match_holds_an_honest_trader_until_both_admins_release() {
        let (mut w, t) = base();
        w.ok(|w| w.hold(t, hash(3)));
        w.ready(2);
        w.fails(E::AccountHeld, |w| w.request(t + 1));
        // one admin alone cannot release; both can; then the trader is back
        let a = Admins::only_rov(&w.v.e);
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 2);
            let ix = w.release_hold_ix(a);
            w.v.e.send_admins(ix, a)
        });
        w.ok(|w| w.release_hold(t + 3));
        w.ok(|w| w.request(t + 4));
        assert_eq!(w.account().pass_pending, c::PASS_REQUESTED);
    }
}

// ================================================================================================ A-7

/// DESIGN 13.2 A-7: "Latency arbitrage against a stale oracle | Partly mitigated: spread, tight `max_age_secs`, the quick-trade
/// rule; the backend can also rate-limit. Not eliminated."
///
/// Class LIMITED. The tests pin what still passes (a price that is 59 s old is a valid price; a profit held 46 s is booked in
/// full) and the three mitigations the program does enforce (spread, the quick tally limit, the admin-tunable max age). Rate
/// limiting is backend work and has no test in this repo.
mod a07_latency_arbitrage {
    use super::*;
    pub const CLASS: Class = Class::Limited;

    /// A stale price 0.5% below the true mid.
    const STALE: u64 = 995_000_000_000;

    fn stale_buy(w: &mut JEnv, t: i64, age: i64) {
        let b = fill_batch(w, t, STALE, proof(MARKET_EUR, t - age, STALE));
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
    }

    fn sell_at_true_price(t_close: i64, timer_start: i64) -> (JournalEvent, PriceProof) {
        (close(MARKET_EUR, LONG, MEUR, ask(STALE, H_EUR), bid(MID, H_EUR), t_close, timer_start, 0), proof(MARKET_EUR, t_close, MID))
    }

    #[test]
    fn a_59_second_old_price_buys_and_a_46_second_hold_books_the_whole_profit() {
        let (mut w, t) = base();
        stale_buy(&mut w, t, 59);
        let (cl, pr) = sell_at_true_price(t + 46, t);
        let pnl = engine::pnl_micro(LONG, MEUR, ask(STALE, H_EUR), bid(MID, H_EUR)).unwrap();
        assert!(pnl > 4_000_000_000, "about $4.9k on 1M EUR");
        let b = w.batch(vec![cl, meta(t + 46, true)], vec![pr]);
        w.post_ok(t + 47, &b, &[MARKET_EUR]);
        let a = w.account();
        assert_eq!((a.b, a.q_tally), (S100K + pnl, 0), "46 s is not a quick trade: credited in full, tally untouched");
    }

    #[test]
    fn a_quick_exit_is_halved_and_the_quick_tally_caps_the_harvest_at_0_028_s() {
        // 20 s hold: the credit is floor(pnl/2) and goes into q_tally; the limit is 0.028 S = $2,800
        let (mut w, t) = base();
        stale_buy(&mut w, t, 59);
        let (cl, pr) = sell_at_true_price(t + 20, t);
        let pnl = engine::pnl_micro(LONG, MEUR, ask(STALE, H_EUR), bid(MID, H_EUR)).unwrap();
        let b = w.batch(vec![cl.clone(), meta(t + 20, true)], vec![pr.clone()]);
        w.post_ok(t + 21, &b, &[MARKET_EUR]);
        let a = w.account();
        assert_eq!((a.b, a.q_tally), (S100K + pnl / 2, pnl / 2));
        // the same trade a second time would take the tally to about $4.9k: refused
        let t2 = t + 100;
        stale_buy(&mut w, t2, 59);
        let (cl2, pr2) = sell_at_true_price(t2 + 20, t2);
        let b = w.batch(vec![cl2, meta(t2 + 20, true)], vec![pr2]);
        rejected(&mut w, t2 + 21, &b, &[MARKET_EUR], E::QuickLimitExceeded);
        assert_eq!(w.account().q_tally, pnl / 2);
        let _ = (cl, pr);
    }

    #[test]
    fn the_spread_eats_every_gap_smaller_than_the_round_trip() {
        // stale by 0.5 basis points: the round trip costs 2 x 30 micro-units of 1.0 = 6 bp, so the "arbitrage" loses money
        let (mut w, t) = base();
        let stale = 999_995_000_000u64;
        let b = fill_batch(&w, t, stale, proof(MARKET_EUR, t - 59, stale));
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        let cl = close(MARKET_EUR, LONG, MEUR, ask(stale, H_EUR), bid(MID, H_EUR), t + 60, t, 0);
        let b = w.batch(vec![cl, meta(t + 60, true)], vec![proof(MARKET_EUR, t + 60, MID)]);
        w.post_ok(t + 61, &b, &[MARKET_EUR]);
        let lost = S100K - w.account().b;
        assert!(lost > 0, "the trade lost {lost} micro-USD");
        // exactly: ask(stale) - bid(1.0), numerically equal to the micro-USD at this quantity
        assert_eq!(i128::from(lost), i128::from(ask(stale, H_EUR)) - i128::from(bid(MID, H_EUR)));
    }

    #[test]
    fn either_admin_can_tighten_max_age_and_the_same_stale_price_is_then_refused() {
        for who in ["sl8", "rov"] {
            let (mut w, t) = base();
            let mut p = params_of(&w.v.e.market(MARKET_EUR));
            p.max_age_secs = 5;
            let admins = if who == "sl8" { Admins::only_sl8(&w.v.e) } else { Admins::only_rov(&w.v.e) };
            let ix = update_market_ix(admins, &UpdateMarketArgs { market_id: MARKET_EUR, params: p });
            assert_ok(w.v.e.send_admins(ix, admins));
            let b = fill_batch(&w, t, STALE, proof(MARKET_EUR, t - 59, STALE));
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::ProofTooOld);
            // a 5 s old price is still fine
            let b = fill_batch(&w, t, STALE, proof(MARKET_EUR, t - 5, STALE));
            w.post_ok(t + 1, &b, &[MARKET_EUR]);
        }
    }
}

// ================================================================================================ A-8

/// DESIGN 13.2 A-8: "Keep an account alive or kill it from outside | Evaluator-only `touch_activity` follows real signed
/// orders; the trader's own `release_account` also refreshes. A silent evaluator can let an account lapse (R-3)."
///
/// Class LIMITED. Blocked: nobody but the evaluator touches, nobody but the owner releases, a dead account cannot be refreshed,
/// the push is throttled to once per 86,000 s. Limited (pinned): "follows real signed orders" is not checkable on chain, so the
/// evaluator can keep an idle account alive for ever; and the vault's `mark_abandoned` is permissionless once the account has
/// been idle for more than 7 days.
mod a08_keep_alive_or_kill {
    use super::*;
    pub const CLASS: Class = Class::Limited;
    /// The vault's `NotAbandonable` (index 10 of its error enum).
    const VAULT_NOT_ABANDONABLE: u32 = 6010;

    fn last_push(w: &JEnv) -> (i64, i64) {
        (w.account().last_activity_pushed, w.vault_status().last_activity_timestamp)
    }

    #[test]
    fn nobody_but_the_evaluator_touches_and_nobody_but_the_owner_releases() {
        let (mut w, _) = base();
        let now = w.t0 + 100_000;
        let other = w.v.new_trader();
        let (tr, ev, st) = (dup(&w.trader), dup(&w.v.evaluator), Keypair::new());
        w.v.e.svm.airdrop(&st.pubkey(), 1_000_000_000).unwrap();
        let before = last_push(&w);
        for who in [&tr, &st, &other] {
            w.fails(E::Unauthorized, |w| {
                w.set_time(now);
                let ix = w.touch_ix(who.pubkey());
                w.v.e.send(&[ix], &[&dup(who)])
            });
        }
        for who in [&ev, &st, &other] {
            w.fails(E::Unauthorized, |w| {
                w.set_time(now);
                let ix = w.release_acct_ix(who.pubkey());
                w.v.e.send(&[ix], &[&dup(who)])
            });
        }
        assert_eq!(last_push(&w), before, "no outsider moved the activity clock");
    }

    #[test]
    fn the_push_is_throttled_twice_here_after_85999_s_and_by_the_vault_after_86399_s() {
        let (mut w, _) = base();
        let t0 = w.t0 + 100_000;
        w.ok(|w| w.touch(t0));
        assert_eq!(last_push(&w), (t0, t0));
        let d = dig(&w);
        for dt in [1i64, 3_600, 85_999] {
            w.ok(|w| w.touch(t0 + dt));
            assert_eq!(dig(&w), d, "touch at +{dt} s is a harmless success that changes nothing");
        }
        // 86,000 s later this program's own throttle lets the call through to the vault; the vault has its own, longer, interval
        // (86,400 s) and answers Throttled: nothing is stored on either side
        for dt in [86_000i64, 86_399] {
            w.ok(|w| w.touch(t0 + dt));
            assert_eq!(last_push(&w), (t0, t0), "touch at +{dt} s: the vault says Throttled");
        }
        w.ok(|w| w.touch(t0 + 86_400));
        assert_eq!(last_push(&w), (t0 + 86_400, t0 + 86_400));
        // the trader's release uses the same two throttles
        w.ok(|w| w.release_acct(t0 + 86_400 + 85_999));
        assert_eq!(last_push(&w), (t0 + 86_400, t0 + 86_400));
        w.ok(|w| w.release_acct(t0 + 2 * 86_400));
        assert_eq!(last_push(&w), (t0 + 2 * 86_400, t0 + 2 * 86_400));
    }

    #[test]
    fn a_dead_account_cannot_be_refreshed_by_anyone() {
        for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
            let (mut w, _) = base();
            let now = w.t0 + 100_000;
            w.edit_account(|a| a.status = status);
            w.fails(E::AccountNotActive, |w| w.touch(now));
            w.fails(E::AccountNotActive, |w| w.release_acct(now));
        }
    }

    #[test]
    fn limited_the_evaluator_alone_keeps_an_idle_account_alive_past_seven_days() {
        // 10 touches, 86,000 s apart = 9.95 days, the trader never acts: the account is still Active, the vault still Active.
        // The program cannot know whether a "real signed order" stood behind a touch (that proof is in the intents file).
        let (mut w, _) = base();
        let start = w.t0;
        for i in 1..=10i64 {
            w.ok(|w| w.touch(start + i * 86_000));
        }
        assert_eq!((w.account().status, w.vault_status().status), (c::STATUS_ACTIVE, 0));
        assert!(w.account().last_activity_pushed - start > 604_800, "idle for more than 7 days of real time");
    }

    #[test]
    fn limited_a_silent_evaluator_lets_the_account_lapse_and_anyone_may_then_kill_it_in_the_vault() {
        let (mut w, _) = base();
        let limit = w.t0 + 604_800; // 7 days idle
        // exactly 7 days idle: the vault refuses to abandon (the account is not stale yet)
        let before = dig(&w);
        let r = w.vault_mark_abandoned(limit);
        assert_anchor_code(&r, VAULT_NOT_ABANDONABLE);
        assert_eq!(dig(&w), before);
        assert_eq!(w.vault_status().status, 0);
        // one second more: the permissionless call (a stranger pays) abandons the vault record
        assert_ok(w.vault_mark_abandoned(limit + 1));
        assert_eq!(w.vault_status().status, 3, "vault: Abandoned");
        // locally the account is Active until somebody pushes activity: then it learns it is dead and the wallet slot is freed
        assert_eq!(w.account().status, c::STATUS_ACTIVE);
        w.ok(|w| w.release_acct(limit + 2));
        assert_eq!(w.account().status, c::STATUS_ABANDONED);
        assert_eq!(w.wallet_profile().active_count, 0);
    }

    #[test]
    fn the_traders_own_release_keeps_the_account_alive_without_the_evaluator() {
        let (mut w, _) = base();
        let limit = w.t0 + 604_800;
        w.ok(|w| w.release_acct(limit - 1)); // the evaluator was silent all week, the trader refreshes
        assert_eq!(w.vault_status().last_activity_timestamp, limit - 1);
        let r = w.vault_mark_abandoned(limit + 100);
        assert_anchor_code(&r, VAULT_NOT_ABANDONABLE); // refreshed: not stale
        assert_eq!(w.vault_status().status, 0);
    }
}

// ================================================================================================ A-9

/// DESIGN 13.2 A-9: "Split a position to dodge the 45 s rule | Timer restarts on every add; partial closes do not reset; flip
/// makes a new position."
///
/// Class OFFCHAIN. The timer lives in the engine's position book, which only the evaluator (and the verifier's replay) keeps:
/// the program stores NO positions. The real tests are in `crates/evaluator/tests/adversarial_attacks.rs` (timer restart on
/// every add, partial close keeps the timer, flip starts a new timer, 45 s versus 46 s) and in
/// `crates/verifier/tests/adversarial_attacks.rs` (a `timer_start` that does not follow from the fills is a finding). This
/// module pins what the PROGRAM lets through: `timer_start` is a claim, checked only against `time` and against the `quick` flag.
mod a09_split_position {
    use super::*;
    pub const CLASS: Class = Class::OffChain;

    /// +0.4%: a long of 1M EUR from the ask of 1.0 closed at the bid of this is worth about $3,940 (credit $1,970 if quick).
    const UP: u64 = 1_004_000_000_000;

    /// A close of the 1M EUR long whose claimed timer start is `timer_start`, with the `quick` flag set to `claim` and the
    /// credit following the CLAIM (so exactly one thing is wrong when `claim` disagrees with the 45 s rule).
    fn close_claiming(t: i64, timer_start: i64, claim: bool) -> JournalEvent {
        let mut e = close(MARKET_EUR, LONG, MEUR, eur_ask(), bid(UP, H_EUR), t, timer_start, 0);
        if let JournalEvent::Close { quick, credited, pnl, .. } = &mut e {
            *quick = claim;
            *credited = engine::slice_credit(*pnl, claim).0;
        }
        e
    }

    fn post_close(w: &mut JEnv, t: i64, e: JournalEvent) -> litesvm::types::TransactionResult {
        let b = w.batch(vec![e, meta(t, true)], vec![proof(MARKET_EUR, t, UP)]);
        w.post(t + 1, &b, &[MARKET_EUR])
    }

    #[test]
    fn the_45_second_flag_is_checked_to_the_second() {
        let pnl = engine::pnl_micro(LONG, MEUR, eur_ask(), bid(UP, H_EUR)).unwrap();
        // honest flags: 0, 44, 45 s are quick (half credit, tally), 46 s and older are not
        for (age, quick) in [(0i64, true), (44, true), (45, true), (46, false), (600, false)] {
            let (mut w, t) = base();
            let r = post_close(&mut w, t, close_claiming(t, t - age, quick));
            assert_ok(r);
            let a = w.account();
            if quick {
                assert_eq!((a.b, a.q_tally), (S100K + pnl / 2, pnl / 2), "age {age}");
            } else {
                assert_eq!((a.b, a.q_tally), (S100K + pnl, 0), "age {age}");
            }
        }
        // the wrong flag at the two edges
        for (age, wrong) in [(45i64, false), (46, true), (0, false)] {
            let (mut w, t) = base();
            let b = w.batch(vec![close_claiming(t, t - age, wrong), meta(t, true)], vec![proof(MARKET_EUR, t, UP)]);
            rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::QuickMismatch);
        }
        // a timer that starts after the close is refused
        let (mut w, t) = base();
        let b = w.batch(vec![close_claiming(t, t + 1, true), meta(t, true)], vec![proof(MARKET_EUR, t, UP)]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::TimerInvalid);
    }

    #[test]
    fn limited_the_program_cannot_tell_a_split_from_a_fresh_position() {
        // A compromised or buggy evaluator may claim ANY timer start that is not in the future: 10 million seconds ago is
        // accepted and makes a 5-second round trip a full-credit, non-quick trade. Only the verifier's replay of the fills
        // can notice (crates/verifier/tests/adversarial_attacks.rs).
        let pnl = engine::pnl_micro(LONG, MEUR, eur_ask(), bid(UP, H_EUR)).unwrap();
        let (mut w, t) = base();
        assert_ok(post_close(&mut w, t, close_claiming(t, t - 10_000_000, false)));
        assert_eq!((w.account().b, w.account().q_tally), (S100K + pnl, 0));
        // and the other way round: a split that RESTARTED the timer is a claim like any other
        let (mut w, t) = base();
        assert_ok(post_close(&mut w, t, close_claiming(t, t - 1, true)));
        assert_eq!(w.account().q_tally, pnl / 2);
    }

    #[test]
    fn limited_the_program_keeps_no_positions_so_a_close_needs_no_fill_before_it() {
        let (mut w, t) = base();
        assert_eq!(w.account().trades_closed, 0);
        assert_ok(post_close(&mut w, t, close_claiming(t, t - 100, false)));
        assert_eq!(w.account().trades_closed, 1, "a close of a position that no Fill of this journal ever opened");
    }
}

// ================================================================================================ A-10

/// DESIGN 13.2 A-10: "Overflow or rounding drift between engine and program | `i128` intermediates, checked arithmetic, the
/// program re-computes pnl with the same `engine` function the evaluator uses, differential tests, mutation tests on netting and
/// ratchet."
///
/// Class BLOCKED. This is the short equivalence test; the 100,000-case rounding properties and the differential paths are
/// another file's work. Here: the program refuses every `pnl` that differs from `engine::pnl_micro` by one micro-USD, accepts the
/// engine's own number in 128 pseudo-random cases (long and short), and answers extreme inputs with a named error and an
/// unchanged world (never a panic: a panic would show as a non-Custom instruction error).
mod a10_overflow_and_rounding_drift {
    use super::*;
    pub const CLASS: Class = Class::Blocked;

    /// xorshift64*, fixed seed: the cases are the same on every run.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
    }
    const SEED: u64 = 0x5E78_A10D_1FF0_0D12;

    fn close_event(long: bool, qty: u64, entry: u64, mid: u64, t: i64, pnl_delta: i64) -> JournalEvent {
        let exit = if long { bid(mid, H_EUR) } else { ask(mid, H_EUR) };
        let mut e = close(MARKET_EUR, long, qty, entry, exit, t, t - 100, 0);
        if let JournalEvent::Close { pnl, credited, .. } = &mut e {
            *pnl += pnl_delta;
            *credited = *pnl; // not quick: credit = pnl
        }
        e
    }

    #[test]
    fn the_program_agrees_with_the_engine_in_128_cases_and_refuses_pnl_off_by_one_micro() {
        let mut rng = Rng(SEED);
        println!("A-10 equivalence seed {SEED:#x}");
        let (mut w, t0) = base();
        let (mut accepted, mut wins, mut losses) = (0u32, 0u32, 0u32);
        for i in 0..128i64 {
            let t = t0 + i * 10;
            let long = rng.next().is_multiple_of(2);
            let qty = (rng.next() % 5_000 + 1) * 1_000_000_000; // 1,000 .. 5,000,000 EUR
            let entry = 1_000_000_000_000 - 500_000_000 + rng.next() % 1_000_000_000; // 0.9995 .. 1.0005
            let mid = 1_000_000_000_000 - 500_000_000 + rng.next() % 1_000_000_000;
            let exit = if long { bid(mid, H_EUR) } else { ask(mid, H_EUR) };
            let pnl = engine::pnl_micro(long, qty, entry, exit).unwrap();
            assert!(pnl.abs() < 6_000_000_000, "case {i}: stays away from the floor");
            // reset the balance so every case starts from the same account
            w.edit_account(|a| {
                a.b = S100K;
                a.latch = 0;
                a.q_tally = 0;
            });
            let pr = proof(MARKET_EUR, t, mid);
            for d in [1i64, -1] {
                let b = w.batch(vec![close_event(long, qty, entry, mid, t, d), meta(t, true)], vec![pr.clone()]);
                rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::PnlMismatch);
            }
            let b = w.batch(vec![close_event(long, qty, entry, mid, t, 0), meta(t, true)], vec![pr]);
            w.post_ok(t + 1, &b, &[MARKET_EUR]);
            assert_eq!(w.account().b, S100K + pnl, "case {i}: long {long} qty {qty} entry {entry} mid {mid}");
            accepted += 1;
            if pnl > 0 {
                wins += 1;
            } else {
                losses += 1;
            }
        }
        println!("A-10: {accepted} cases accepted at the engine's number ({wins} profits, {losses} losses/zero), 256 off-by-one refusals");
        assert_eq!(accepted, 128);
        assert!(wins > 20 && losses > 20);
    }

    /// The program's answer to an event: (instruction index, custom code) or "ok".
    fn code_of(r: &litesvm::types::TransactionResult) -> Option<u32> {
        use anchor_lang::solana_program::instruction::error::InstructionError;
        match r {
            Ok(_) => None,
            Err(f) => match &f.err {
                solana_transaction_error::TransactionError::InstructionError(_, InstructionError::Custom(c)) => Some(*c),
                other => panic!("not a program error (a panic or an abort?): {other:?}\n{}", f.meta.logs.join("\n")),
            },
        }
    }

    fn extreme(events: Vec<JournalEvent>, pr: PriceProof, t: i64, w: &mut JEnv) -> Option<u32> {
        let (d, r0) = (dig(w), raw(w));
        let b = w.batch(events, vec![pr]);
        let r = w.post(t + 1, &b, &[MARKET_EUR]);
        let c = code_of(&r);
        if c.is_some() {
            assert_eq!((dig(w), raw(w)), (d, r0), "a refused extreme input changed the world");
        }
        c
    }

    #[test]
    fn extreme_quantities_and_prices_get_a_named_error_and_change_nothing() {
        let max = u64::MAX;
        let code = |e: E| Some(u32::from(e));
        // Close: pnl overflows i64 (qty 1.8e19 x price 1.8e19) -> MathOverflow, whatever the claim says
        let (mut w, t) = base();
        let mut e = close(MARKET_EUR, LONG, MEUR, eur_ask(), eur_bid(), t, t - 100, 0);
        if let JournalEvent::Close { qty, entry_e12, .. } = &mut e {
            *qty = max;
            *entry_e12 = 1;
        }
        assert_eq!(extreme(vec![e, meta(t, true)], proof(MARKET_EUR, t, MID), t, &mut w), code(E::MathOverflow));
        // Close: zero quantity / zero entry
        for (q, en, want) in [(0u64, eur_ask(), E::QuantityInvalid), (MEUR, 0, E::PriceInvalid)] {
            let (mut w, t) = base();
            let mut e = close(MARKET_EUR, LONG, MEUR, eur_ask(), eur_bid(), t, t - 100, 0);
            if let JournalEvent::Close { qty, entry_e12, .. } = &mut e {
                *qty = q;
                *entry_e12 = en;
            }
            assert_eq!(extreme(vec![e, meta(t, true)], proof(MARKET_EUR, t, MID), t, &mut w), code(want), "qty {q} entry {en}");
        }
        // Close: i64::MAX / i64::MIN as the claimed pnl and credit
        for claim in [i64::MAX, i64::MIN, 0, -1] {
            let (mut w, t) = base();
            let mut e = close(MARKET_EUR, LONG, MEUR, eur_ask(), eur_bid(), t, t - 100, 0);
            if let JournalEvent::Close { pnl, credited, .. } = &mut e {
                *pnl = claim;
                *credited = claim;
            }
            assert_eq!(extreme(vec![e, meta(t, true)], proof(MARKET_EUR, t, MID), t, &mut w), code(E::PnlMismatch), "claimed pnl {claim}");
        }
        // Fill: zero, below the minimum, off the step (u64::MAX, 2^63, 1) -> QuantityInvalid
        for q in [0u64, max, 1u64 << 63, 1, 999_999, 1_000_001] {
            let (mut w, t) = base();
            let f = fill(MARKET_EUR, LONG, q, eur_ask(), t, 0);
            assert_eq!(extreme(vec![f, meta(t, false)], proof(MARKET_EUR, t, MID), t, &mut w), code(E::QuantityInvalid), "fill qty {q}");
        }
        // LIMITED: the notional cap (`max_notional_usd`) is not an on-chain rule (DESIGN 3.2: the chain uses a market only for its
        // existence and feed). A step multiple of 1.8e19 units (about $18 trillion at 1.0) is accepted by the program; the
        // evaluator refuses such an order and the verifier flags it. Pinned so that a later on-chain check shows up here.
        let (mut w, t) = base();
        let big = 18_446_744_073_709_000_000u64;
        assert_eq!(big % 1_000, 0);
        assert_eq!(extreme(vec![fill(MARKET_EUR, LONG, big, eur_ask(), t, 0), meta(t, false)], proof(MARKET_EUR, t, MID), t, &mut w), None);
        assert_eq!(w.account().seq, 2);
        // Fill: price zero -> PriceInvalid; any price that is not the derived one (1, u64::MAX) -> EventPriceMismatch
        for (p, want) in [(0u64, E::PriceInvalid), (u64::MAX, E::EventPriceMismatch), (1, E::EventPriceMismatch)] {
            let (mut w, t) = base();
            let f = fill(MARKET_EUR, LONG, MEUR, p, t, 0);
            assert_eq!(extreme(vec![f, meta(t, false)], proof(MARKET_EUR, t, MID), t, &mut w), code(want), "fill price {p}");
        }
        // Proof values: i128::MAX, one above u64::MAX, below 1e-12 -> ProofValueInvalid; exactly u64::MAX x 1e6 fits as a price but
        // the spread arithmetic on it overflows -> MathOverflow
        for (q, want) in [
            (i128::MAX, E::ProofValueInvalid),
            (i128::from(u64::MAX) * 1_000_000, E::MathOverflow),
            (i128::from(u64::MAX) * 1_000_000 + 1_000_000, E::ProofValueInvalid),
            (1i128, E::ProofValueInvalid),
            (999_999, E::ProofValueInvalid),
        ] {
            let (mut w, t) = base();
            let mut p = proof(MARKET_EUR, t, MID);
            p.quantized_value = q;
            let f = fill(MARKET_EUR, LONG, MEUR, eur_ask(), t, 0);
            assert_eq!(extreme(vec![f, meta(t, false)], p, t, &mut w), code(want), "proof value {q}");
        }
        // Fee: above i64::MAX -> FeeInvalid; exactly i64::MAX is a legal amount to the program (see A-18: one Fee event can
        // push an honest account to the floor; the rule is only `amount >= 0`)
        for amt in [u64::MAX, i64::MAX as u64 + 1] {
            let (mut w, t) = base();
            assert_eq!(extreme(vec![fee(MARKET_EUR, amt, t), meta(t, false)], proof(MARKET_EUR, t, MID), t, &mut w), code(E::FeeInvalid), "fee {amt}");
        }
        let (mut w, t) = base();
        assert_eq!(extreme(vec![fee(MARKET_EUR, i64::MAX as u64, t), meta(t, false)], proof(MARKET_EUR, t, MID), t, &mut w), None);
        assert_eq!(w.account().b, S100K - i64::MAX, "b went to about minus 9.2e18 without an overflow");
        // the same Fee when b is already that low cannot subtract again: MathOverflow, nothing changes
        let t2 = t + 1;
        assert_eq!(extreme(vec![fee(MARKET_EUR, i64::MAX as u64, t2), meta(t2, false)], proof(MARKET_EUR, t2, MID), t2, &mut w), code(E::MathOverflow));
    }
}

// ================================================================================================ A-11

/// DESIGN 13.2 A-11: "Upgrade key compromise | 2-of-2 multisig (decision 30); burn decision after the audit."
///
/// Class OPERATIONAL. No test: the upgrade authority is not set by this program and cannot be exercised from LiteSVM without
/// faking the loader's upgrade flow. The audit report carries this checklist (copied into
/// `handover/audit/01-attacks-a01-a18.md` and the deploy checklist):
mod a11_upgrade_key_compromise {
    use super::*;
    pub const CLASS: Class = Class::Operational;

    /// What a human must check; nothing here is executable.
    pub const CHECKLIST: [&str; 6] = [
        "after deploy, `solana program show <id>` lists the upgrade authority: it must be the 2-of-2 multisig of SL8 admin + Rov admin, not a hot key",
        "the deploy keypair is destroyed or moved to cold storage after the authority transfer, and nobody holds a copy",
        "each of the two multisig members signs on its own hardware wallet, in different places",
        "the burn decision (make the program immutable) is taken after the independent audit, written down, dated",
        "a monitor alerts both admins on any Upgrade or SetAuthority transaction of the program id (the upgrade is the one lever that can request payouts up to the vault cap, DESIGN 14)",
        "the verifiable build (hash of the deployed .so) is published; every upgrade is rebuilt and compared before signing",
    ];
}

// ================================================================================================ A-12

/// DESIGN 13.2 A-12: "Compute or account-size DoS | Hard caps: 8 events and 3 proofs per batch; accounts fixed-size."
///
/// Class BLOCKED (the wire-level cases with lying length prefixes are in tests-rs/tests/journal_attacks.rs).
mod a12_compute_and_account_size_dos {
    use super::*;
    pub const CLASS: Class = Class::Blocked;

    fn proofs3(t: i64) -> Vec<PriceProof> {
        vec![proof(MARKET_EUR, t, MID), proof(MARKET_BTC, t, MID), proof(MARKET_EUR, t, MID)]
    }

    #[test]
    fn nine_events_and_four_proofs_are_refused_by_name_and_the_world_is_unchanged() {
        let (mut w, t) = base();
        let head = w.account().journal_hash;
        let fee0 = fee(MARKET_EUR, 0, t).to_bytes();
        let m = meta(t, false).to_bytes();
        let p = proof(MARKET_EUR, t, MID).to_bytes().unwrap();
        // 8 fees + Meta = 9 events
        let nine: Vec<Vec<u8>> = [vec![fee0.clone(); 8], vec![m.clone()]].concat();
        rejected_bytes(&mut w, t + 1, raw_batch(1, head, &nine, 9, &[], 0), &[MARKET_EUR], E::BatchTooLarge);
        // 4 proofs with one event
        rejected_bytes(&mut w, t + 1, raw_batch(1, head, std::slice::from_ref(&m), 1, &vec![p.clone(); 4], 4), &[MARKET_EUR], E::BatchTooLarge);
        // 9 market accounts
        let b = w.batch(vec![fee(MARKET_EUR, 0, t), meta(t, false)], vec![]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR; 9], E::BatchTooLarge);
        // the counts at the cap are fine (8 events, 3 proofs, 8 markets): see the next test
        assert_eq!(w.account().seq, 0);
    }

    #[test]
    fn the_largest_legal_batch_runs_inside_the_budget_and_the_account_does_not_grow() {
        let (mut w, t) = base();
        let len0 = raw(&w).len();
        // 8 events: 7 fills/closes referencing the three proofs, then Meta. Three proofs, three different mids.
        let evs = vec![
            fill(MARKET_EUR, LONG, MEUR, eur_ask(), t, 0),
            fill(MARKET_BTC, LONG, MEUR, ask(MID, H_BTC), t, 1),
            fill(MARKET_EUR, LONG, MEUR, eur_ask(), t, 2),
            fill(MARKET_BTC, false, MEUR, bid(MID, H_BTC), t, 1),
            fill(MARKET_EUR, false, MEUR, eur_bid(), t, 0),
            fill(MARKET_EUR, LONG, MEUR, eur_ask(), t, 2),
            fill(MARKET_BTC, LONG, MEUR, ask(MID, H_BTC), t, 1),
            meta(t, false),
        ];
        assert_eq!(evs.len(), 8);
        let b = w.batch(evs, proofs3(t));
        let m = w.post_ok(t + 1, &b, &[MARKET_EUR, MARKET_BTC]);
        println!("A-12: the largest legal batch (8 events, 3 proofs, 2 markets) used {} compute units", m.compute_units_consumed);
        assert!(m.compute_units_consumed < 600_000, "inside the 600k the harness requests");
        assert_eq!(raw(&w).len(), len0, "fixed-size account: no growth");
        assert_eq!(w.account().seq, 8);
    }

    #[test]
    fn the_account_never_changes_size_through_the_whole_lifecycle() {
        let (mut w, t) = base();
        let len0 = raw(&w).len();
        assert_eq!(len0, setl8_lev_trading::state::TradingAccount::LEN, "the fixed size of the account (discriminator included)");
        assert_eq!(len0, 207);
        let b = close_batch(&w, t, MID, proof(MARKET_EUR, t, MID));
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(raw(&w).len(), len0);
        w.ready(2);
        w.set_limits(172_800, 100_000_000_000, 86_400);
        w.ok(|w| w.request(t + 10));
        assert_eq!(raw(&w).len(), len0);
        w.ok(|w| w.confirm(t + 11));
        assert_eq!(raw(&w).len(), len0);
        let payer = w.new_payer();
        w.ok(|w| w.finalize(t + 11 + 172_800, &payer));
        assert_eq!(raw(&w).len(), len0);
        w.edit_account(|a| a.b = 92_000_000_000);
        let fb = w.fail_batch(t + 200_000, vec![], FailReason::StaticFloor, 92_000_000_000, true);
        assert_ok(w.report_fail(t + 200_001, &fb, 0, &[MARKET_EUR]));
        assert_eq!(raw(&w).len(), len0);
    }
}

// ================================================================================================ A-13

/// DESIGN 13.2 A-13: "Wallet cap evasion | Not blocked (new wallet). The cap limits convenience only."
///
/// Class OPERATIONAL (nothing to block: the cap is 5 active accounts per wallet, a fresh wallet has its own). No fake test of
/// "evasion". The one fact that is tested is the cap itself, so that the checklist below states the right number.
mod a13_wallet_cap_evasion {
    use super::*;
    pub const CLASS: Class = Class::Operational;

    pub const CHECKLIST: [&str; 4] = [
        "the backend (KYC or per-device or per-funding-source limits) decides how many wallets one person may use; the program only caps ONE wallet at 5 active accounts",
        "a ban (ban_wallet, fail_for_violation) is per wallet: the runbook must say how a banned person is recognised again off chain",
        "the sector-wide exposure that matters is the daily payout cap and the vault's record chain bound (R-1), not the wallet count",
        "review the funding source of new wallets that open accounts in bulk (the program emits `Opened` events that make this a query)",
    ];

    #[test]
    fn the_cap_the_checklist_talks_about_is_five_active_accounts_per_wallet() {
        let (mut w, _) = base();
        for _ in 1..c::MAX_ACTIVE_ACCOUNTS {
            w.open_another();
        }
        assert_eq!(w.wallet_profile().active_count, c::MAX_ACTIVE_ACCOUNTS);
        let tr = dup(&w.trader);
        assert_err(&w.v.open(&tr, 8), E::WalletCapReached);
        // a fresh wallet is not limited by it
        let fresh = w.v.new_trader();
        assert_ok(w.v.open(&fresh, 8));
    }
}

// ================================================================================================ A-14

/// DESIGN 13.2 A-14: "Trader presses the button while a position is open | Chain `flat` can be stale; blocked by `confirm_pass`
/// / `reject_pass` (the evaluator compares with its engine) and by the replay of the watchdog."
///
/// Class OFFCHAIN. The program has the stored `flat` flag and nothing else: it does not hold positions, so a `flat` that the
/// evaluator wrote as true while a position is open cannot be seen on chain. On chain: `request_advance` and `confirm_pass`
/// re-read the stored flag (`NotFlat`); `reject_pass` accepts any of four reasons at any time; `post_journal` is frozen from
/// the moment of confirmation. The evaluator side is in `crates/evaluator/tests/adversarial_attacks.rs`
/// (`a14_*`), the verifier side (Meta.flat that disagrees with the replayed positions) in
/// `crates/verifier/tests/adversarial_attacks.rs`.
mod a14_button_with_an_open_position {
    use super::*;
    pub const CLASS: Class = Class::OffChain;

    const TARGET: i64 = 108_000_000_000;

    /// `[Fill long 1M EUR, Meta(flat)]`.
    fn open_batch(w: &JEnv, t: i64, flat: bool) -> JournalBatch {
        w.batch(vec![fill_long(t, 0), meta(t, flat)], vec![p_eur(t)])
    }

    #[test]
    fn when_the_chain_says_not_flat_the_request_is_refused_by_name() {
        let (mut w, t) = base();
        let b = open_batch(&w, t, false);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(w.account().flat, 0);
        w.edit_account(|a| a.b = TARGET); // the balance is right, only the flat flag is wrong
        w.fails(E::NotFlat, |w| w.request(t + 2));
        assert_eq!(w.account().pass_pending, c::PASS_NONE);
        assert_eq!(w.claim(1), None);
    }

    #[test]
    fn a_position_opened_after_the_press_is_caught_by_confirm_pass_and_answered_by_reject_pass() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t)); // chain is flat, b is at target: the program has to accept
        assert_eq!(w.account().pass_pending, c::PASS_REQUESTED);
        // state 1 does not freeze the journal: the trader sends an order, the evaluator posts the fill
        let b = open_batch(&w, t + 5, false);
        w.post_ok(t + 6, &b, &[MARKET_EUR]);
        assert_eq!((w.account().flat, w.account().b, w.account().pass_pending), (0, TARGET, c::PASS_REQUESTED));
        // the stored flag is re-read: confirm_pass refuses, exact error, nothing changes
        w.fails(E::NotFlat, |w| w.confirm(t + 7));
        // a non-evaluator cannot reject
        let tr = dup(&w.trader);
        w.fails(E::Unauthorized, |w| {
            w.set_time(t + 8);
            let ix = w.reject_ix(tr.pubkey(), c::REJECT_NOT_FLAT);
            w.v.e.send(&[ix], &[&tr])
        });
        // the evaluator rejects with reason NotFlat; the trader's request is gone, phase and balance untouched
        let before = w.account();
        let m = w.ok(|w| w.reject(t + 9, c::REJECT_NOT_FLAT));
        let ev = cpi_events::<PassRejected>(&m);
        assert_eq!((ev.len(), ev[0].reason, ev[0].phase), (1, c::REJECT_NOT_FLAT, 2));
        let a = w.account();
        assert_eq!((a.pass_pending, a.phase, a.b, a.flat, a.seq), (c::PASS_NONE, before.phase, before.b, before.flat, before.seq));
    }

    #[test]
    fn limited_a_stale_flat_true_is_accepted_by_the_program_and_the_pass_goes_through_on_chain() {
        // An evaluator that writes Meta{flat: true} after a fill (a bug, or a compromise) leaves the chain saying flat. The
        // program has no position book to contradict it: request, confirm and (after the delay) finalize all succeed. Only the
        // evaluator's own request_pass check and the verifier's replay flag it.
        let (mut w, t) = base();
        w.set_limits(172_800, 100_000_000_000, 86_400);
        w.edit_account(|a| {
            a.b = TARGET;
            a.phase = 2; // phase 1 pays nothing
        });
        let b = open_batch(&w, t, true); // a fill that is never closed, declared flat
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!(w.account().flat, 1);
        w.ok(|w| w.request(t + 2));
        w.ok(|w| w.confirm(t + 3));
        let payer = w.new_payer();
        w.ok(|w| w.finalize(t + 3 + 172_800, &payer));
        assert_eq!(w.claim(1).unwrap().owed, PAYOUT_100K);
    }

    #[test]
    fn a_confirmed_pass_freezes_the_journal_so_no_position_can_be_opened_behind_the_confirmation() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        let b = open_batch(&w, t + 2, false);
        rejected(&mut w, t + 3, &b, &[MARKET_EUR], E::PassPending);
        // the failing route is closed as well while confirmed: report_fail needs pass_pending != 2
        w.edit_account(|a| a.b = 92_000_000_000);
        let fb = w.fail_batch(t + 4, vec![], FailReason::StaticFloor, 92_000_000_000, true);
        w.fails(E::PassPending, |w| w.report_fail(t + 5, &fb, 0, &[MARKET_EUR]));
        // hold_account is the one thing the evaluator can still do (and both admins can veto)
        w.ok(|w| w.hold(t + 6, hash(4)));
    }
}

// ================================================================================================ A-15

/// DESIGN 13.2 A-15: "Trader spams `request_advance` to burn evaluator transactions | Cost parity (both pay a base fee);
/// `reject_pass` blocks nothing else. Accepted."
///
/// Class POLICY (accepted). There is no rate limit in the program: the trace is that every cycle leaves the account as it was
/// and costs each side one single-signature transaction.
mod a15_request_spam {
    use super::*;
    pub const CLASS: Class = Class::Policy;

    #[test]
    fn twenty_five_request_reject_cycles_in_one_second_change_nothing_but_the_request_timestamp() {
        let (mut w, t) = base();
        w.ready(2);
        let before = w.account();
        let totals0 = (w.registry_totals(), w.tally().requested_count);
        let mut cu = (0u64, 0u64);
        for i in 0..25 {
            let m = w.ok(|w| w.request(t));
            let trader_tx = w.v.e.last_tx;
            cu.0 = cu.0.max(m.compute_units_consumed);
            let m = w.ok(|w| w.reject(t, c::REJECT_NOT_FLAT));
            let evaluator_tx = w.v.e.last_tx;
            cu.1 = cu.1.max(m.compute_units_consumed);
            // cost parity: both sides send one instruction in a transaction of the same shape (fee payer + own signer)
            assert_eq!(trader_tx.signatures, evaluator_tx.signatures, "cycle {i}: same number of signatures, so the same base fee");
            let a = w.account();
            assert_eq!(a.pass_pending, c::PASS_NONE, "cycle {i}");
        }
        println!("A-15: request_advance max {} CU, reject_pass max {} CU over 25 cycles", cu.0, cu.1);
        let a = w.account();
        assert_eq!(
            (a.b, a.phase, a.seq, a.journal_hash, a.vetoes, a.next_request_id, a.status, a.held, a.flat),
            (before.b, before.phase, before.seq, before.journal_hash, before.vetoes, before.next_request_id, before.status, before.held, before.flat)
        );
        assert_eq!((w.registry_totals(), w.tally().requested_count), totals0, "no payout, no vault counter moved");
        assert_eq!(w.claim(1), None);
    }

    #[test]
    fn a_second_request_while_one_is_open_is_refused_and_costs_only_the_senders_fee() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        for k in 1..=5 {
            w.fails(E::PassAlreadyPending, |w| w.request(t + k));
        }
        // the open request is the evaluator's single decision; the trader can withdraw it after an hour on his own
        w.fails(E::CancelTooEarly, |w| w.cancel(t + 3_599));
        w.ok(|w| w.cancel(t + 3_600));
        assert_eq!(w.account().pass_pending, c::PASS_NONE);
    }

    #[test]
    fn reject_pass_blocks_nothing_else_the_journal_flows_while_a_request_is_open() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        // state 1: journal posts, hold, touch all work; only confirm/reject/veto/cancel/finalize look at the state
        let b = close_batch(&w, t + 1, MID, proof(MARKET_EUR, t + 1, MID));
        w.post_ok(t + 2, &b, &[MARKET_EUR]);
        w.ok(|w| w.touch(t + 3));
        w.ok(|w| w.reject(t + 4, c::REJECT_OTHER));
        let b = close_batch(&w, t + 5, MID, proof(MARKET_EUR, t + 5, MID));
        w.post_ok(t + 6, &b, &[MARKET_EUR]);
    }

    #[test]
    fn limited_reject_pass_needs_no_justification_the_evaluator_can_refuse_a_valid_pass_for_ever() {
        // flat, b exactly at target, nothing else wrong: reject_pass still goes through with any of the four reasons, 4 times
        let (mut w, t) = base();
        w.ready(2);
        for (i, reason) in [c::REJECT_NOT_FLAT, c::REJECT_JOURNAL_BEHIND, c::REJECT_TARGET_NOT_REACHED, c::REJECT_OTHER].into_iter().enumerate() {
            w.ok(|w| w.request(t + i as i64));
            w.ok(|w| w.reject(t + i as i64, reason));
        }
        // a fifth reason code is refused by name
        w.ok(|w| w.request(t + 10));
        w.fails(E::RejectReasonInvalid, |w| w.reject(t + 10, 4));
        assert_eq!(w.account().pass_pending, c::PASS_REQUESTED);
    }
}

// ================================================================================================ A-16

/// DESIGN 13.2 A-16: "Admins veto without cause | They cannot move money, but can delay a trader without limit. Visible
/// (`vetoes`, event). Policy item, OPEN_QUESTIONS #8."
///
/// Class POLICY. No code control against a pointless veto. The test shows the whole trace: counter, event, no state change
/// besides `pass_pending` and `vetoes`, and that nothing limits how often it can be done.
mod a16_veto_without_cause {
    use super::*;
    pub const CLASS: Class = Class::Policy;

    const DELAY: i64 = 172_800;

    #[test]
    fn one_honest_valid_pass_is_vetoed_and_leaves_exactly_two_traces() {
        let (mut w, t) = base();
        w.set_limits(DELAY as u32, 100_000_000_000, 86_400);
        w.ready(2);
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        let cfg0 = snap(&w, &config_pda());
        let before = w.account();
        let m = w.ok(|w| w.veto(t + DELAY)); // the last second before the pass is payable
        let a = w.account();
        assert_eq!((a.pass_pending, a.vetoes), (c::PASS_NONE, 1));
        // everything else is byte-identical to the account before
        let mut exp = before.clone();
        exp.pass_pending = c::PASS_NONE;
        exp.vetoes = 1;
        assert_eq!(a.b, exp.b);
        assert_eq!((a.phase, a.flat, a.seq, a.journal_hash, a.next_request_id, a.held), (exp.phase, exp.flat, exp.seq, exp.journal_hash, exp.next_request_id, exp.held));
        let ev = cpi_events::<PassVetoed>(&m);
        let e = &w.v.e;
        assert_eq!((ev.len(), ev[0].account, ev[0].phase, ev[0].admin_a, ev[0].admin_b), (1, w.acct, 2, e.sl8.pubkey(), e.rov.pubkey()));
        assert_eq!(snap(&w, &config_pda()), cfg0, "the config (cap counters) is untouched");
        assert_eq!((w.claim(1), w.tally().requested_count, w.registry_totals()), (None, 0, (0, 0)), "no money moved");
        // and the pass can no longer be paid
        let payer = w.new_payer();
        w.fails(E::NoPendingPass, |w| w.finalize(t + DELAY + 1, &payer));
    }

    #[test]
    fn nothing_in_the_program_limits_how_often_the_same_pass_is_vetoed() {
        // 12 cycles: request, confirm, veto in the last second of the delay = 24 days with no payout, counter 12.
        let (mut w, t0) = base();
        w.set_limits(DELAY as u32, 100_000_000_000, 86_400);
        w.ready(2);
        let mut now = t0;
        for i in 1..=12u8 {
            w.ok(|w| w.request(now));
            w.ok(|w| w.confirm(now + 1));
            w.ok(|w| w.veto(now + DELAY));
            assert_eq!(w.account().vetoes, i);
            now += DELAY + 2;
        }
        assert!(now - t0 > 24 * 86_400);
        let a = w.account();
        assert_eq!((a.b, a.phase, a.status, a.pass_pending), (108_000_000_000, 2, c::STATUS_ACTIVE, c::PASS_NONE), "still eligible, never paid");
        assert_eq!((w.claim(1), w.tally().requested_total), (None, 0));
    }

    #[test]
    fn it_takes_both_admins_one_admin_and_strangers_are_refused_and_leave_no_trace() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        for admins in [Admins::only_sl8(&w.v.e), Admins::only_rov(&w.v.e), Admins::neither(&w.v.e)] {
            w.fails(E::Unauthorized, |w| {
                w.set_time(t + 5);
                let ix = w.veto_ix(admins);
                w.v.e.send_admins(ix, admins)
            });
        }
        assert_eq!((w.account().vetoes, w.account().pass_pending), (0, c::PASS_CONFIRMED));
        // a veto needs something to veto
        w.ok(|w| w.veto(t + 6));
        w.fails(E::NoPendingPass, |w| w.veto(t + 7));
        assert_eq!(w.account().vetoes, 1);
    }
}

// ================================================================================================ A-17

/// DESIGN 13.2 A-17: "Compromised evaluator fills the daily cap with fake passes to starve honest ones | Fake passes are vetoed
/// (the cap counts only finalized passes, so vetoed ones do not consume it); honest passes finalize after the next window if
/// needed."
///
/// Class LIMITED. The test builds the fake passes the way the evaluator would (a journal of a single Close with no Fill and an
/// invented entry price: +$8,000, flat), so it also shows how cheap a fake pass is (R-1). With the veto the cap is untouched; if
/// the admins miss the window (R-6) each fake that is finalized takes the whole window and the honest trader waits.
mod a17_fake_passes_starve_the_cap {
    use super::*;
    pub const CLASS: Class = Class::Limited;

    const DELAY: i64 = 172_800;
    const WINDOW: u32 = 86_400;

    /// Three accounts of one trader: two fake (F1, F2) and one honest (H), all open at the same clock. Cap = one payout per window.
    fn world() -> (JEnv, [Pk; 3], i64) {
        let mut w = JEnv::new();
        let a0 = w.acct;
        w.open_another();
        let a1 = w.acct;
        w.open_another();
        let a2 = w.acct;
        let t = w.t0.max(w.account().created_at) + 1_000;
        w.set_limits(DELAY as u32, PAYOUT_100K, WINDOW);
        (w, [a0, a1, a2], t)
    }

    /// The evaluator's fabrication: one Close of a 1M EUR long that was never opened, entry invented so that pnl = +$8,000.
    fn fabricate(w: &mut JEnv, t: i64) {
        w.edit_account(|a| a.phase = 2); // phase 1 pays nothing; the fake pass is in a paying phase
        let b = w.batch(vec![close_long_pnl(t, t - 100, 8_000_000_000), meta(t, true)], vec![p_eur(t)]);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
        assert_eq!((w.account().b, w.account().flat, w.account().trades_closed), (108_000_000_000, 1, 1));
    }

    /// Request by the trader, confirmation by the evaluator.
    fn request_and_confirm(w: &mut JEnv, t: i64) {
        w.ok(|w| w.request(t));
        w.ok(|w| w.confirm(t + 1));
        assert_eq!(w.account().pass_pending, c::PASS_CONFIRMED);
    }

    #[test]
    fn with_the_veto_the_fakes_leave_the_cap_untouched_and_the_honest_pass_pays_on_time() {
        let (mut w, [f1, f2, h], t) = world();
        let cfg0 = snap(&w, &config_pda());
        for (i, f) in [f1, f2].into_iter().enumerate() {
            w.acct = f;
            fabricate(&mut w, t + 10 * i as i64);
            request_and_confirm(&mut w, t + 20 + 10 * i as i64);
        }
        w.acct = h;
        w.ready(2);
        request_and_confirm(&mut w, t + 40);
        assert_eq!(snap(&w, &config_pda()), cfg0, "requesting and confirming three passes did not touch the cap counters");
        // the admins (watchdog) veto both fakes
        for f in [f1, f2] {
            w.acct = f;
            w.ok(|w| w.veto(t + 100));
            let a = w.account();
            assert_eq!((a.pass_pending, a.vetoes), (c::PASS_NONE, 1));
        }
        assert_eq!(snap(&w, &config_pda()), cfg0, "vetoed passes consumed no cap");
        assert_eq!(w.tally().requested_count, 0);
        // the honest pass finalizes at its own time, for the formula amount
        w.acct = h;
        let payer = w.new_payer();
        w.ok(|w| w.finalize(t + 41 + DELAY, &payer));
        assert_eq!((w.claim(1).unwrap().owed, w.tally().requested_count, w.v.e.config().window_total), (PAYOUT_100K, 1, PAYOUT_100K));
        // a vetoed fake has nothing left to finalize
        w.acct = f1;
        w.fails(E::NoPendingPass, |w| w.finalize(t + 42 + DELAY, &payer));
    }

    #[test]
    fn limited_without_the_veto_each_finalized_fake_takes_a_whole_window_and_the_honest_one_waits() {
        let (mut w, [f1, f2, h], t) = world();
        for (i, f) in [f1, f2].into_iter().enumerate() {
            w.acct = f;
            fabricate(&mut w, t + 10 * i as i64);
            request_and_confirm(&mut w, t + 20 + 10 * i as i64);
        }
        w.acct = h;
        w.ready(2);
        request_and_confirm(&mut w, t + 40);
        let payer = w.new_payer();
        // R-6: nobody vetoes. Fake 1 finalizes first (anyone may call finalize; the attacker picks the order)
        let t1 = t + 21 + DELAY;
        w.acct = f1;
        w.ok(|w| w.finalize(t1, &payer));
        assert_eq!((w.v.e.config().window_total, w.v.e.config().window_start, w.tally().requested_count), (PAYOUT_100K, t1, 1));
        // the honest pass is eligible a few seconds later and meets the cap, by name, and changes nothing
        w.acct = h;
        w.fails(E::DailyPayoutCapReached, |w| w.finalize(t1 + 20, &payer));
        // the window rolls at the exact second; the attacker calls first again: fake 2 takes the next window
        let t2 = t1 + i64::from(WINDOW);
        w.fails(E::DailyPayoutCapReached, |w| w.finalize(t2 - 1, &payer));
        w.acct = f2;
        w.ok(|w| w.finalize(t2, &payer));
        w.acct = h;
        w.fails(E::DailyPayoutCapReached, |w| w.finalize(t2 + 1, &payer));
        // only when no fake is left does the honest pass get its window
        let t3 = t2 + i64::from(WINDOW);
        w.ok(|w| w.finalize(t3, &payer));
        assert_eq!(w.tally().requested_count, 3);
        assert_eq!(w.tally().requested_total, 3 * PAYOUT_100K, "two fake payouts reached the vault queue: that is R-1, bounded by the vault");
    }
}

// ================================================================================================ A-18

/// DESIGN 13.2 A-18: "Compromised evaluator mass-fails honest accounts | Possible and visible; rotation by both admins; no
/// automatic remedy (OPEN_QUESTIONS #9)."
///
/// Class LIMITED. What the evaluator must write to fail an account the program accepts as a failure: (1) a batch
/// `events..., Fail, Meta(flat)` through `report_fail`; (2) `Fail.b_after` equal to the stored balance AFTER the batch's own
/// events, (3) that balance at or below the static floor (0.92 S), so the batch has to carry a fabricated loss: a `Fee` of the
/// right size, or a `Close` with an invented entry price (the program has no position book). The test pins each of these
/// and what is visible afterwards.
mod a18_mass_fail_honest_accounts {
    use super::*;
    pub const CLASS: Class = Class::Limited;

    #[test]
    fn a_bare_fail_event_is_refused_for_an_honest_account_each_missing_condition_by_name() {
        let (mut w, t) = base(); // b = 100,000: 8,000 above the floor
        // the right shape, but b_after is the honest balance: the floor is not reached
        let b = w.fail_batch(t, vec![], FailReason::StaticFloor, S100K, true);
        w.fails(E::FailStaticFloorNotReached, |w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
        // b_after lies (claims the floor): it differs from the stored balance
        let b = w.fail_batch(t, vec![], FailReason::StaticFloor, FLOOR, true);
        w.fails(E::FailBalanceMismatch, |w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
        // PhaseQuick with a quick tally of zero
        let b = w.fail_batch(t, vec![], FailReason::PhaseQuick, S100K, true);
        w.fails(E::FailQuickNotExceeded, |w| w.report_fail(t + 1, &b, 1, &[MARKET_EUR]));
        // the reason argument and the event must agree
        let b = w.fail_batch(t, vec![], FailReason::StaticFloor, S100K, true);
        w.fails(E::FailReasonMismatch, |w| w.report_fail(t + 1, &b, 1, &[MARKET_EUR]));
        // not flat; no Fail; Fail not right before Meta; unknown reason code
        let b = w.fail_batch(t, vec![], FailReason::StaticFloor, S100K, false);
        w.fails(E::FailNotFlat, |w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
        let b = w.batch(vec![fee(MARKET_EUR, 0, t), meta(t, true)], vec![]);
        w.fails(E::FailMissing, |w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
        let b = w.batch(vec![fail_ev(t, FailReason::StaticFloor, S100K, 0), fee(MARKET_EUR, 0, t), meta(t, true)], vec![p_eur(t)]);
        w.fails(E::FailNotBeforeMeta, |w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
        let b = w.fail_batch(t, vec![], FailReason::StaticFloor, S100K, true);
        w.fails(E::FailReasonInvalid, |w| w.report_fail(t + 1, &b, 9, &[MARKET_EUR]));
        // and the plain post_journal route never carries a Fail
        let b = w.batch(vec![fail_ev(t, FailReason::StaticFloor, S100K, 0), meta(t, true)], vec![p_eur(t)]);
        rejected(&mut w, t + 1, &b, &[MARKET_EUR], E::FailEventNotAllowed);
        assert_eq!(w.account().status, c::STATUS_ACTIVE);
    }

    /// The two fabrications that take 8,000 USD off an honest account: a Fee, or a Close with an invented entry price.
    fn fabrications(t: i64) -> Vec<(&'static str, JournalEvent)> {
        vec![
            ("a Fee of 8,000 USD (no rule calls for it)", fee(MARKET_EUR, 8_000_000_000, t)),
            ("a Close of a position nobody opened, entry invented, pnl -8,000 USD", close_long_pnl(t, t - 100, -8_000_000_000)),
        ]
    }

    #[test]
    fn one_fabricated_loss_event_is_enough_to_fail_any_account_and_the_trace_is_complete() {
        for (what, ev) in fabrications(0) {
            let (mut w, t) = base();
            let ev = at(&ev, t);
            let wallet0 = w.wallet_profile();
            assert_eq!(wallet0.active_count, 1);
            let b = w.fail_batch(t, vec![ev], FailReason::StaticFloor, FLOOR, true);
            let m = w.ok(|w| w.report_fail(t + 1, &b, 0, &[MARKET_EUR]));
            let a = w.account();
            assert_eq!((a.status, a.b, a.flat, a.seq), (c::STATUS_FAILED, FLOOR, 1, 3), "{what}");
            // visible: the AccountFailed event, the vault flag, the freed slot, and the three journal events in the log
            let failed = cpi_events::<AccountFailed>(&m);
            assert_eq!((failed.len(), failed[0].account, failed[0].reason, failed[0].b, failed[0].seq), (1, w.acct, 0, FLOOR, 3), "{what}");
            assert_eq!(w.vault_status().status, 2, "vault: Failed");
            assert_eq!(w.wallet_profile().active_count, 0);
            let logs = event_logs(&m);
            assert_eq!(logs.len(), 3, "{what}: [loss event, Fail, Meta]");
            assert_eq!(w.claim(1), None);
        }
    }

    #[test]
    fn mass_fail_of_five_accounts_is_possible_visible_and_stopped_only_by_rotation() {
        let (mut w, _) = base();
        let mut accts = vec![w.acct];
        for _ in 1..c::MAX_ACTIVE_ACCOUNTS {
            w.open_another();
            accts.push(w.acct);
        }
        let t = accts.iter().map(|a| w.v.account(a).created_at).max().unwrap().max(w.t0) + 1_000;
        let mut seen = 0usize;
        for (i, a) in accts.iter().enumerate().take(4) {
            w.acct = *a;
            let b = w.fail_batch(t + i as i64, vec![fee(MARKET_EUR, 8_000_000_000, t + i as i64)], FailReason::StaticFloor, FLOOR, true);
            let m = w.ok(|w| w.report_fail(t + 10 + i as i64, &b, 0, &[MARKET_EUR]));
            seen += cpi_events::<AccountFailed>(&m).len();
        }
        assert_eq!(seen, 4, "four failures, four AccountFailed events: visible");
        assert_eq!(w.wallet_profile().active_count, 1);
        // both admins rotate the evaluator key (one admin alone cannot: tested in a04)
        let new_ev = Keypair::new();
        let ix = set_evaluator_ix(both(&w.v.e), new_ev.pubkey());
        assert_ok(w.v.e.send_as_admins(&[ix]));
        // the old key can fail nobody any more: the fifth account is still Active and untouched
        w.acct = accts[4];
        let b = w.fail_batch(t, vec![fee(MARKET_EUR, 8_000_000_000, t)], FailReason::StaticFloor, FLOOR, true);
        w.fails(E::Unauthorized, |w| w.report_fail(t + 20, &b, 0, &[MARKET_EUR]));
        assert_eq!((w.account().status, w.account().b), (c::STATUS_ACTIVE, S100K));
        // no automatic remedy: the four failed accounts stay Failed; the new evaluator is refused on them
        w.acct = accts[0];
        w.v.e.svm.airdrop(&new_ev.pubkey(), 1_000_000_000).unwrap();
        let b = w.batch(vec![meta(t + 30, false)], vec![]);
        let r = w.post_bytes(t + 31, &new_ev, b.to_bytes().unwrap(), &[MARKET_EUR]);
        assert_err_at(&r, 1, E::AccountNotActive);
        assert_eq!(w.account().status, c::STATUS_FAILED);
        // the only road back is the trader's own paid reset
        let before = w.v.token_balance(&w.v.trader_token(&w.trader.pubkey()));
        w.reset_ok(t + 40);
        let after = w.v.token_balance(&w.v.trader_token(&w.trader.pubkey()));
        assert!(before > after, "the reset costs the trader money ({before} -> {after})");
        assert_eq!(w.account().status, c::STATUS_ACTIVE);
    }

    #[test]
    fn a_confirmed_pass_cannot_be_failed_but_a_requested_one_can() {
        let (mut w, t) = base();
        w.ready(2);
        w.ok(|w| w.request(t));
        w.edit_account(|a| a.b = FLOOR);
        let b = w.fail_batch(t + 5, vec![], FailReason::StaticFloor, FLOOR, true);
        let m = w.ok(|w| w.report_fail(t + 6, &b, 0, &[MARKET_EUR]));
        assert_eq!((w.account().status, w.account().pass_pending), (c::STATUS_FAILED, c::PASS_NONE), "state 1 is cleared by the fail");
        assert_eq!(cpi_events::<AccountFailed>(&m).len(), 1);
        let (mut w, t) = base();
        w.ready(2);
        w.set_pending(c::PASS_CONFIRMED, t - 10, t - 5);
        w.edit_account(|a| a.b = FLOOR);
        let b = w.fail_batch(t + 5, vec![], FailReason::StaticFloor, FLOOR, true);
        w.fails(E::PassPending, |w| w.report_fail(t + 6, &b, 0, &[MARKET_EUR]));
    }
}

// ================================================================================================ the table

/// One row per DESIGN 13.2 attack: (number, class, module name, the module's own `CLASS` constant).
macro_rules! row {
    ($n:expr, $m:ident, $class:ident) => {
        ($n, Class::$class, stringify!($m), $m::CLASS)
    };
}

const ATTACK_TABLE: [(u8, Class, &str, Class); 18] = [
    row!(1, a01_old_price_replay, Limited),
    row!(2, a02_wrong_feed_id, Blocked),
    row!(3, a03_impersonate_the_program_to_the_vault, Blocked),
    row!(4, a04_journal_by_anyone_but_the_evaluator, Blocked),
    row!(5, a05_trader_fabricates_a_pass, Blocked),
    row!(6, a06_cross_account_hedging, OffChain),
    row!(7, a07_latency_arbitrage, Limited),
    row!(8, a08_keep_alive_or_kill, Limited),
    row!(9, a09_split_position, OffChain),
    row!(10, a10_overflow_and_rounding_drift, Blocked),
    row!(11, a11_upgrade_key_compromise, Operational),
    row!(12, a12_compute_and_account_size_dos, Blocked),
    row!(13, a13_wallet_cap_evasion, Operational),
    row!(14, a14_button_with_an_open_position, OffChain),
    row!(15, a15_request_spam, Policy),
    row!(16, a16_veto_without_cause, Policy),
    row!(17, a17_fake_passes_starve_the_cap, Limited),
    row!(18, a18_mass_fail_honest_accounts, Limited),
];

/// The count the header comment of this file states for a class name (`//! Counts: BLOCKED 6, LIMITED 5, ...`).
fn header_count(src: &str, name: &str) -> usize {
    let line = src.lines().find(|l| l.starts_with("//! Counts:")).expect("the header has a Counts line");
    let pos = line.find(name).unwrap_or_else(|| panic!("{name} missing in the header Counts line"));
    line[pos + name.len()..].trim_start().split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().expect("a number after the class name")
}

/// The source text of the module `name`: from `mod name {` to the first closing brace at column 0.
fn module_source<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src.find(&format!("\nmod {name} {{")).unwrap_or_else(|| panic!("module {name} not found"));
    let rest = &src[start + 1..];
    &rest[..rest.find("\n}\n").expect("module end")]
}

#[test]
fn attack_table_is_complete() {
    let src = include_str!("adversarial.rs");
    // all 18 numbers once, in order, each module named a<NN>_...
    let numbers: Vec<u8> = ATTACK_TABLE.iter().map(|r| r.0).collect();
    assert_eq!(numbers, (1..=18).collect::<Vec<u8>>());
    for (n, class, name, own) in ATTACK_TABLE {
        assert!(name.starts_with(&format!("a{n:02}_")), "module {name} is not attack {n}");
        assert_eq!(class, own, "{name}: the table and the module disagree about the class");
    }
    // no module a<NN>_ exists that the table does not list, and none is listed twice
    let mut modules: Vec<&str> = src.lines().filter_map(|l| l.strip_prefix("mod a")).filter(|r| r.as_bytes().get(2) == Some(&b'_')).map(|r| r.trim_end_matches(" {")).collect();
    modules.sort_unstable();
    let mut listed: Vec<String> = ATTACK_TABLE.iter().map(|r| r.2.strip_prefix('a').unwrap().to_string()).collect();
    listed.sort_unstable();
    assert_eq!(modules.len(), 18);
    assert_eq!(modules.iter().map(|m| m.to_string()).collect::<Vec<_>>(), listed);

    // class counts: the numbers the header states
    let count = |c: Class| ATTACK_TABLE.iter().filter(|r| r.1 == c).count();
    let (b, l, o, p, op) = (count(Class::Blocked), count(Class::Limited), count(Class::OffChain), count(Class::Policy), count(Class::Operational));
    println!("A-1..A-18 classes: BLOCKED {b}, LIMITED {l}, OFF-CHAIN {o}, POLICY {p}, OPERATIONAL {op}");
    assert_eq!(b + l + o + p + op, 18);
    assert_eq!(
        (b, l, o, p, op),
        (header_count(src, "BLOCKED"), header_count(src, "LIMITED"), header_count(src, "OFFCHAIN"), header_count(src, "POLICY"), header_count(src, "OPERATIONAL"))
    );

    // tests per module: every attack that is not OPERATIONAL has at least one test; A-11 has none (no fake test) and a checklist
    let mut total = 0;
    let mut ignored = 0;
    for (n, class, name, _) in ATTACK_TABLE {
        let m = module_source(src, name);
        let tests = m.matches("#[test]").count();
        ignored += m.matches("#[ignore").count();
        total += tests;
        if class == Class::Operational {
            assert!(m.contains("CHECKLIST"), "A-{n}: an operational attack carries a checklist");
        } else {
            assert!(tests >= 1, "A-{n} ({name}) has no test");
        }
        if n == 11 {
            assert_eq!(tests, 0, "A-11 has no fake test");
        }
    }
    println!("A-1..A-18: {total} tests in the modules ({ignored} ignored as FINDINGs)");
    assert!(total >= 18);
    // the two checklists exist and are not empty
    assert!(!a11_upgrade_key_compromise::CHECKLIST.is_empty() && !a13_wallet_cap_evasion::CHECKLIST.is_empty());
}
