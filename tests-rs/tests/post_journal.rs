//! `post_journal` (DESIGN 4.4 I-1 to I-12) against the real vault harness. Every failure test asserts the exact error code
//! and an unchanged world digest (account bytes + all token balances). Boundary tests also run the exact-boundary pass.
mod common;

use common::journal::*;
use litesvm::types::TransactionMetadata;
use common::*;
use journal_types::{JournalEvent, PriceProof};
use setl8_lev_trading::{constants as c, errors::Setl8Error as E};
use solana_keypair::Keypair;
use solana_signer::Signer;

const LONG: bool = true;

/// Fresh world, event base time (account time + 1,000 s).
fn base() -> (JEnv, i64) {
    let w = JEnv::new();
    let t = w.t0 + 1_000;
    (w, t)
}

// ================================================================ DESIGN 4.9

#[test]
fn design_4_9_worked_example_in_three_transactions() {
    let mut w = JEnv::new();
    let t0 = w.t0 + 1_000; // 14:00:00
    let acct0 = w.account();
    assert_eq!((acct0.b, acct0.seq), (S100K, 0));

    // ---- Tx A: fill 1,000,000 EUR at the ask of mid 1.08 (30 e6 half-spread) + tight start at 14:01:00
    let ask = ask(1_080_000_000_000, H_EUR);
    assert_eq!(ask, 1_080_032_400_000, "ask 1.0800324");
    assert_eq!(engine::margin_micro(MEUR, ask, 50), Ok(21_600_648_000), "margin $21,600.648");
    let mut fill_ev = fill(MARKET_EUR, LONG, MEUR, ask, t0, 0);
    if let JournalEvent::Fill { margin, .. } = &mut fill_ev {
        *margin = 21_600_648_000;
    }
    let a = w.batch(
        vec![
            fill_ev,
            JournalEvent::TightStart { time: t0 + 60, p_open: 1_235_161_000, r: S100K, l_start: usd(96_000), g_peak: 1_235_161_000, l_level: 96_235_161_000, proof: 1 },
            meta(t0 + 60, false),
        ],
        vec![proof(MARKET_EUR, t0, 1_080_000_000_000), proof(MARKET_EUR, t0 + 60, 1_081_300_000_000)],
    );
    let head_a = w.expected_head(&a);
    let ma = w.post_ok(t0 + 70, &a, &[MARKET_EUR]);
    let s = w.account();
    assert_eq!((s.b, s.q_tally, s.tight_on, s.r, s.g_peak, s.l_level), (S100K, 0, 1, S100K, 1_235_161_000, 96_235_161_000));
    assert_eq!((s.lock_until, s.flat, s.seq, s.trades_closed, s.last_batch_time), (0, 0, 3, 0, t0 + 60));
    assert_eq!(s.journal_hash, head_a.0, "head equals the independent journal_types computation");
    let mut logs = event_logs(&ma);
    assert_eq!(logs.len(), 3);

    // ---- Tx B: new high at 14:01:30
    let b = w.batch(
        vec![JournalEvent::FloorRaised { time: t0 + 90, g_peak: 3_035_107_000, l_level: 98_035_107_000, proof: 0 }, meta(t0 + 90, false)],
        vec![proof(MARKET_EUR, t0 + 90, 1_083_100_000_000)],
    );
    let head_b = w.expected_head(&b);
    let mb = w.post_ok(t0 + 100, &b, &[MARKET_EUR]);
    let s = w.account();
    assert_eq!((s.b, s.q_tally, s.tight_on, s.r, s.g_peak, s.l_level), (S100K, 0, 1, S100K, 3_035_107_000, 98_035_107_000));
    assert_eq!((s.lock_until, s.flat, s.seq, s.trades_closed), (0, 0, 5, 0));
    assert_eq!(s.journal_hash, head_b.0);
    logs.extend(event_logs(&mb));

    // ---- Tx C: close at the sample bid 1.07796766 (mid 1.078) + breach at 14:02:30
    let exit = bid(1_078_000_000_000, H_EUR);
    assert_eq!(exit, 1_077_967_660_000, "bid 1.07796766");
    let close_ev = close(MARKET_EUR, LONG, MEUR, ask, exit, t0 + 150, t0, 0);
    if let JournalEvent::Close { pnl, quick, credited, .. } = &close_ev {
        assert_eq!((*pnl, *quick, *credited), (-2_064_740_000, false, -2_064_740_000), "pnl -2,064.74, slice age 150 s");
    }
    let c = w.batch(
        vec![
            close_ev,
            JournalEvent::Breach { time: t0 + 150, l_level: 98_035_107_000, b_after: 97_935_260_000, proof: 0 },
            meta(t0 + 150, true),
        ],
        vec![proof(MARKET_EUR, t0 + 150, 1_078_000_000_000)],
    );
    let head_c = w.expected_head(&c);
    let mc = w.post_ok(t0 + 160, &c, &[MARKET_EUR]);
    let s = w.account();
    assert_eq!(s.b, 97_935_260_000, "B 97,935.26");
    assert_eq!((s.q_tally, s.tight_on, s.r, s.g_peak, s.l_level), (0, 0, 0, 0, 0));
    assert_eq!(s.lock_until, t0 + 150 + 12 * 3600, "12 h lock");
    assert_eq!((s.flat, s.seq, s.trades_closed, s.latch, s.phase, s.status), (1, 8, 1, 0, 1, 0));
    assert_eq!(s.journal_hash, head_c.0);
    logs.extend(event_logs(&mc));

    // the head recomputed from the EventLogs of the three transactions by journal_types::verify_chain
    assert_eq!(logs.len(), 8);
    assert_eq!(logs.iter().map(|l| l.seq).collect::<Vec<_>>(), (1..=8).collect::<Vec<u64>>());
    assert_eq!(chain_head_from_logs(&logs, &w.acct), s.journal_hash);
    assert_eq!((logs[0].proof_bytes.len(), logs[1].proof_bytes.len()), (61, 61), "Fill and TightStart carry their proofs");
    assert!(logs[2].proof_bytes.is_empty() && logs[7].proof_bytes.is_empty(), "Meta references no proof");
    PriceProof::decode(&logs[0].proof_bytes).unwrap();
}

#[test]
fn the_executed_price_follows_the_proof_mid_and_the_spread_for_every_side() {
    // RESOLVE (a): Fill long = ask, Fill short = bid, Close of a long = bid, Close of a short = ask
    let (mut w, t) = base();
    let (a, b) = (ask(MID, H_EUR), bid(MID, H_EUR));
    assert!(b < MID && MID < a, "the spread is visible in the numbers");
    let events = vec![
        fill(MARKET_EUR, true, MEUR, a, t, 0),
        fill(MARKET_EUR, false, MEUR, b, t, 0),
        close(MARKET_EUR, true, MEUR, a, b, t, t - 100, 0),
        close(MARKET_EUR, false, MEUR, b, a, t, t - 100, 0),
    ];
    let batch = simple(&w, t, events, false);
    w.post_ok(t + 1, &batch, &[MARKET_EUR]);
    assert_eq!(w.account().trades_closed, 2);
}

// ================================================================ I-1

#[test]
fn i1_wrong_signer_is_refused() {
    let (mut w, t) = base();
    let batch = simple(&w, t, vec![], false);
    let before = w.digest();
    let other = Keypair::new();
    w.v.e.svm.airdrop(&other.pubkey(), 1_000_000_000).unwrap();
    let r = w.post_bytes(t + 1, &other, batch.to_bytes().unwrap(), &[]);
    assert_err_at(&r, 1, E::Unauthorized);
    assert_eq!(w.digest(), before);
    // the trader is not the evaluator either
    let tr = dup(&w.trader);
    let r = w.post_bytes(t + 1, &tr, batch.to_bytes().unwrap(), &[]);
    assert_err_at(&r, 1, E::Unauthorized);
    // the real evaluator works
    w.post_ok(t + 1, &batch, &[]);
}

#[test]
fn i1_unsigned_evaluator_is_refused() {
    let (mut w, t) = base();
    let batch = simple(&w, t, vec![], false);
    let ev = dup(&w.v.evaluator);
    // build the instruction by hand with the signer flag cleared
    w.v.e.set_clock(t + 1);
    let mut metas = setl8_lev_trading::accounts::PostJournal {
        evaluator: ev.pubkey(),
        trading_account: w.acct,
        config: config_pda(),
        ruleset: ruleset_pda(1),
        event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0,
        program: PROGRAM_ID,
    }
    .to_account_metas(None);
    metas[0].is_signer = false;
    use anchor_lang::{InstructionData, ToAccountMetas};
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: metas,
        data: setl8_lev_trading::instruction::PostJournal { batch_bytes: batch.to_bytes().unwrap() }.data(),
    };
    let before = w.digest();
    let r = w.v.e.send(&[ix], &[]);
    assert_anchor_at(&r, 0, 3010);
    assert_eq!(w.digest(), before);
}

#[test]
fn i1_account_must_be_active() {
    for status in [c::STATUS_FAILED, c::STATUS_ABANDONED, c::STATUS_GRADUATED] {
        let (mut w, t) = base();
        w.edit_account(|a| a.status = status);
        let batch = simple(&w, t, vec![], false);
        w.post_rejected(t + 1, &batch, &[], E::AccountNotActive);
    }
}

#[test]
fn i1_a_confirmed_pass_freezes_the_journal_but_a_requested_one_does_not() {
    let (mut w, t) = base();
    w.edit_account(|a| a.pass_pending = c::PASS_CONFIRMED);
    let batch = simple(&w, t, vec![], false);
    w.post_rejected(t + 1, &batch, &[], E::PassPending);
    w.edit_account(|a| a.pass_pending = c::PASS_REQUESTED);
    let batch = simple(&w, t, vec![], false);
    w.post_ok(t + 1, &batch, &[]);
    w.edit_account(|a| a.pass_pending = c::PASS_NONE);
    let batch = simple(&w, t + 2, vec![], false);
    w.post_ok(t + 3, &batch, &[]);
}

#[test]
fn i1_a_held_account_refuses_fills_but_allows_closes() {
    let (mut w, t) = base();
    w.edit_account(|a| a.held = 1);
    let batch = simple(&w, t, vec![fill_long(t, 0)], false);
    w.post_rejected(t + 1, &batch, &[MARKET_EUR], E::AccountHeld);
    let batch = simple(&w, t, vec![close_long_pnl(t, t - 100, 100)], true);
    w.post_ok(t + 1, &batch, &[MARKET_EUR]);
    assert_eq!(w.account().trades_closed, 1);
    // released: fills work again
    w.edit_account(|a| a.held = 0);
    let batch = simple(&w, t + 2, vec![fill_long(t + 2, 0)], false);
    w.post_ok(t + 3, &batch, &[MARKET_EUR]);
}

#[test]
fn the_sector_pause_does_not_stop_post_journal() {
    // DESIGN 6.1: set_paused blocks open_account, request_advance, finalize_pass...; post_journal keeps working (RESOLVE c)
    let (mut w, t) = base();
    let ix = set_paused_ix(both(&w.v.e), true);
    assert_ok(w.v.e.send_as_admins(&[ix]));
    assert_eq!(w.v.e.config().paused, 1);
    let batch = simple(&w, t, vec![fill_long(t, 0)], false);
    w.post_ok(t + 1, &batch, &[MARKET_EUR]);
    assert_eq!(w.account().seq, 2);
}

// ================================================================ I-2

#[test]
fn i2_first_seq_must_be_seq_plus_one() {
    for bad in [0u64, 2, 5, u64::MAX] {
        let (mut w, t) = base();
        let mut batch = simple(&w, t, vec![], false);
        batch.first_seq = bad;
        w.post_rejected(t + 1, &batch, &[], E::BadSequence);
    }
}

#[test]
fn i2_prev_hash_must_equal_the_stored_head_to_the_bit() {
    for bit in [0usize, 7, 100, 255] {
        let (mut w, t) = base();
        let mut batch = simple(&w, t, vec![], false);
        batch.prev_hash[bit / 8] ^= 1 << (bit % 8);
        w.post_rejected(t + 1, &batch, &[], E::BadPrevHash);
    }
}

#[test]
fn i2_a_replayed_batch_is_refused() {
    let (mut w, t) = base();
    let batch = simple(&w, t, vec![fee(MARKET_EUR, 5, t)], false);
    w.post_ok(t + 1, &batch, &[MARKET_EUR]);
    let after_first = w.account();
    w.post_rejected(t + 2, &batch, &[MARKET_EUR], E::BadSequence);
    assert_eq!(w.account().seq, after_first.seq);
    // a replay with the corrected first_seq but the old prev_hash fails on the hash
    let mut again = batch.clone();
    again.first_seq = after_first.seq + 1;
    w.post_rejected(t + 2, &again, &[MARKET_EUR], E::BadPrevHash);
}

#[test]
fn i2_batches_chain_and_the_emitted_logs_reproduce_the_head() {
    // I-12 makes "one batch of 4" and "two batches of 2" different event sequences (each batch has its own Meta), so the
    // equality checked here is: two batches posted in two transactions equal one continuous chain over their events,
    // recomputed independently by journal_types::verify_chain from the emitted logs; and two identical worlds agree.
    let mut heads = Vec::new();
    for _ in 0..2 {
        let (mut w, t) = base();
        let b1 = simple(&w, t, vec![fee(MARKET_EUR, 3, t)], false);
        let m1 = w.post_ok(t + 1, &b1, &[MARKET_EUR]);
        let b2 = simple(&w, t + 5, vec![fee(MARKET_EUR, 4, t + 5)], false);
        let m2 = w.post_ok(t + 6, &b2, &[MARKET_EUR]);
        let mut logs = event_logs(&m1);
        logs.extend(event_logs(&m2));
        assert_eq!(logs.len(), 4);
        let head = chain_head_from_logs(&logs, &w.acct);
        assert_eq!(w.account().journal_hash, head);
        assert_eq!(w.account().seq, 4);
        heads.push(head);
    }
    // the two worlds have different accounts (different random traders), so only the chain structure is compared
    assert_ne!(heads[0], heads[1], "the account key is part of the genesis hash");
}

#[test]
fn i2_a_reordered_or_dropped_event_changes_the_head_and_breaks_the_next_batch() {
    let build = |w: &JEnv, t: i64, order: &[usize]| {
        let evs = [fee(MARKET_EUR, 1, t), fee(MARKET_EUR, 2, t), fee(MARKET_EUR, 3, t)];
        let mut v: Vec<JournalEvent> = order.iter().map(|i| evs[*i].clone()).collect();
        v.push(meta(t, false));
        w.batch(v, vec![])
    };
    for (label, order) in [("reordered", vec![1usize, 0, 2]), ("dropped", vec![0, 1])] {
        let (mut w, t) = base();
        // the head the honest journal would reach, computed by the real crate before anything is posted
        let honest_head = w.expected_head(&build(&w, t, &[0, 1, 2])).0;
        let tampered = build(&w, t, &order);
        let head = w.expected_head(&tampered).0;
        assert_ne!(head, honest_head, "{label}: a different chain than the honest one");
        w.post_ok(t + 1, &tampered, &[MARKET_EUR]);
        assert_eq!(w.account().journal_hash, head, "{label}: the program follows the batch it was given");
        // a follow-up built on the HONEST head is refused
        let mut next = w.batch(vec![meta(t + 2, false)], vec![]);
        next.prev_hash = honest_head;
        w.post_rejected(t + 3, &next, &[], E::BadPrevHash);
        // and one built on the real head is accepted
        let next = w.batch(vec![meta(t + 2, false)], vec![]);
        w.post_ok(t + 3, &next, &[]);
    }
}

// ================================================================ I-3

#[test]
fn i3_time_never_goes_backwards() {
    let (mut w, _) = base();
    let last = w.account().last_batch_time;
    // 1 second before the last batch time
    let b = w.batch(vec![meta(last - 1, false)], vec![]);
    w.post_rejected(last + 5, &b, &[], E::TimeBackwards);
    // equal is fine
    let b = w.batch(vec![meta(last, false)], vec![]);
    w.post_ok(last + 5, &b, &[]);
    assert_eq!(w.account().last_batch_time, last);
    // the next batch may not start before the previous one's last event
    let b = w.batch(vec![meta(last - 1, false)], vec![]);
    w.post_rejected(last + 5, &b, &[], E::TimeBackwards);
}

#[test]
fn i3_time_within_a_batch_is_ordered() {
    let (mut w, t) = base();
    let b = w.batch(vec![fee(MARKET_EUR, 1, t + 5), fee(MARKET_EUR, 1, t + 4), meta(t + 5, false)], vec![]);
    w.post_rejected(t + 10, &b, &[MARKET_EUR], E::TimeBackwards);
    let b = w.batch(vec![fee(MARKET_EUR, 1, t + 4), fee(MARKET_EUR, 1, t + 4), meta(t + 5, false)], vec![]);
    w.post_ok(t + 10, &b, &[MARKET_EUR]);
    // Meta earlier than the event before it
    let b = w.batch(vec![fee(MARKET_EUR, 1, t + 8), meta(t + 7, false)], vec![]);
    w.post_rejected(t + 10, &b, &[MARKET_EUR], E::TimeBackwards);
}

#[test]
fn i3_events_may_be_at_most_two_seconds_ahead_of_the_block_time() {
    let (mut w, t) = base();
    let now = t + 50;
    let b = w.batch(vec![meta(now + 3, false)], vec![]);
    w.post_rejected(now, &b, &[], E::TimeInFuture);
    let b = w.batch(vec![meta(now + 2, false)], vec![]);
    w.post_ok(now, &b, &[]);
    assert_eq!(w.account().last_batch_time, now + 2);
    // only the failing event matters: an early event in a batch with a late Meta
    let b = w.batch(vec![fee(MARKET_EUR, 1, now + 2), meta(now + 3, false)], vec![]);
    w.post_rejected(now, &b, &[MARKET_EUR], E::TimeInFuture);
}

// ================================================================ I-4

#[test]
fn i4_markets_must_exist_and_be_passed() {
    let (mut w, t) = base();
    // unknown id (no such market anywhere)
    let b = w.batch(vec![fee(77, 1, t), meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::MarketUnknown);
    // an existing market that was not passed
    let b = w.batch(vec![fee(MARKET_BTC, 1, t), meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::MarketUnknown);
    // the passed market makes the same event valid
    let b = w.batch(vec![fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
}

#[test]
fn i4_proof_feed_must_be_the_markets_current_feed() {
    let (mut w, t) = base();
    // a BTC proof on the EURUSD market
    let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![proof(MARKET_BTC, t, MID)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
    // the feed after an update_market swap: the old proofs stop working at once
    let mut params = market_params(7);
    params.feed_id = feed("NEWFEED");
    params.sessions = {
        let mut s = [setl8_lev_trading::state::Session::default(); 6];
        s[0] = setl8_lev_trading::state::Session { start_min: 0, end_min: 10_080 };
        s
    };
    params.session_count = 1;
    params.qty_step = 1_000;
    params.qty_min = 1_000_000;
    let admins = Admins::both(&w.v.e);
    let ix = update_market_ix(admins, &setl8_lev_trading::instructions::UpdateMarketArgs { market_id: MARKET_EUR, params });
    assert_ok(w.v.e.send_admins(ix, admins));
    let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p_eur(t)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
}

#[test]
fn i4_event_price_must_equal_the_price_derived_from_the_proof_and_the_spread() {
    let (mut w, t) = base();
    let a = eur_ask();
    let events = |p: u64| vec![fill(MARKET_EUR, true, MEUR, p, t, 0), meta(t, false)];
    // off by one unit of e12 in both directions
    for p in [a + 1, a - 1] {
        let b = w.batch(events(p), vec![p_eur(t)]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
    }
    // the mid itself (no spread) and the bid are wrong for a long fill
    for p in [MID, eur_bid()] {
        let b = w.batch(events(p), vec![p_eur(t)]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
    }
    // a short fill at the ask is wrong, at the bid right
    let b = w.batch(vec![fill(MARKET_EUR, false, MEUR, a, t, 0), meta(t, false)], vec![p_eur(t)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::EventPriceMismatch);
    let b = w.batch(vec![fill(MARKET_EUR, false, MEUR, eur_bid(), t, 0), meta(t, false)], vec![p_eur(t)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    // closes: a long closes at the bid, a short at the ask
    let b = w.batch(vec![close(MARKET_EUR, true, MEUR, MID, a, t + 2, t, 0), meta(t + 2, false)], vec![p_eur(t + 2)]);
    w.post_rejected(t + 3, &b, &[MARKET_EUR], E::EventPriceMismatch);
    let b = w.batch(vec![close(MARKET_EUR, false, MEUR, MID, eur_bid(), t + 2, t, 0), meta(t + 2, false)], vec![p_eur(t + 2)]);
    w.post_rejected(t + 3, &b, &[MARKET_EUR], E::EventPriceMismatch);
    // each market uses ITS spread: BTC has 50 e6
    let btc_ask = ask(MID, H_BTC);
    let b = w.batch(vec![fill(MARKET_BTC, true, MEUR, btc_ask, t + 2, 0), meta(t + 2, false)], vec![proof(MARKET_BTC, t + 2, MID)]);
    w.post_ok(t + 3, &b, &[MARKET_BTC]);
    let b = w.batch(vec![fill(MARKET_BTC, true, MEUR, eur_ask(), t + 4, 0), meta(t + 4, false)], vec![proof(MARKET_BTC, t + 4, MID)]);
    w.post_rejected(t + 5, &b, &[MARKET_BTC], E::EventPriceMismatch);
}

#[test]
fn i4_proof_age_boundary_is_max_age_secs_inclusive() {
    let (mut w, t) = base();
    let events = |t: i64| vec![fill_long(t, 0), meta(t, false)];
    // max_age_secs = 60: exactly 60 s old is fine, 61 s is too old
    let b = w.batch(events(t), vec![p_eur(t - 60)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    let b = w.batch(events(t + 2), vec![p_eur(t + 2 - 61)]);
    w.post_rejected(t + 3, &b, &[MARKET_EUR], E::ProofTooOld);
}

#[test]
fn i4_proof_from_the_future_boundary_is_two_seconds() {
    let (mut w, t) = base();
    let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p_eur(t + 2)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    let b = w.batch(vec![fill_long(t + 2, 0), meta(t + 2, false)], vec![p_eur(t + 2 + 3)]);
    w.post_rejected(t + 3, &b, &[MARKET_EUR], E::ProofInFuture);
}

#[test]
fn i4_proof_value_must_be_a_positive_price() {
    let (mut w, t) = base();
    for v in [0i128, -5, 999_999] {
        let mut p = p_eur(t);
        p.quantized_value = v;
        let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofValueInvalid);
    }
    let mut p = p_eur(t);
    p.quantized_value = i128::MAX;
    let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofValueInvalid);
}

#[test]
fn i4_unsupported_sources_are_refused_and_the_mock_follows_the_build() {
    let (mut w, t) = base();
    for source in [0u8, 1, 5] {
        let mut p = p_eur(t);
        p.source = source;
        let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofUnknownSource);
    }
    // source 255: this .so is the localnet build, so mock proofs pass (the market's feed id is checked first,
    // so build the market with a source-255 feed id)
    w.add_market(5, 255, "MOCK", 1);
    let mut p = proof(MARKET_EUR, t, MID);
    p.source = 255;
    p.feed_id = journal_types::feed_id(&journal_types::NativeSha256, 255, "MOCK");
    // spread_e6[5] is 0 in the test rule set, so the executed price of a long fill is the mid itself
    let b = w.batch(vec![fill(5, true, MEUR, MID, t, 0), meta(t, false)], vec![p]);
    w.post_ok(t + 1, &b, &[5]);
}

#[test]
fn i4_signed_update_shape_by_source() {
    let (mut w, t) = base();
    // source 2 with a non-empty signed_update
    let mut p = p_eur(t);
    p.signed_update = vec![1];
    let b = w.batch(vec![fill_long(t, 0), meta(t, false)], vec![p]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofBadSignedUpdate);
    // source 4 (Hyperliquid) with a non-empty signed_update
    w.add_market(5, 4, "HL", 1);
    let mut p4 = proof(MARKET_EUR, t, MID);
    p4.source = 4;
    p4.feed_id = journal_types::feed_id(&journal_types::NativeSha256, 4, "HL");
    p4.signed_update = vec![9; 32];
    let b = w.batch(vec![fill(5, true, MEUR, MID, t, 0), meta(t, false)], vec![p4.clone()]);
    w.post_rejected(t + 1, &b, &[5], E::ProofBadSignedUpdate);
    p4.signed_update = vec![];
    let b = w.batch(vec![fill(5, true, MEUR, ask(MID, 0), t, 0), meta(t, false)], vec![p4]);
    w.post_ok(t + 1, &b, &[5]);
    // source 3 needs exactly 32 bytes
    w.add_market(6, 3, "SIGNED", 1);
    for len in [0usize, 31, 33, 512] {
        let mut p3 = proof(MARKET_EUR, t + 2, MID);
        p3.source = 3;
        p3.feed_id = journal_types::feed_id(&journal_types::NativeSha256, 3, "SIGNED");
        p3.signed_update = vec![7; len];
        let b = w.batch(vec![fill(6, true, MEUR, ask(MID, 0), t + 2, 0), meta(t + 2, false)], vec![p3]);
        w.post_rejected(t + 3, &b, &[6], E::ProofBadSignedUpdate);
    }
    let mut p3 = proof(MARKET_EUR, t + 2, MID);
    p3.source = 3;
    p3.feed_id = journal_types::feed_id(&journal_types::NativeSha256, 3, "SIGNED");
    p3.signed_update = vec![7; 32];
    let b = w.batch(vec![fill(6, true, MEUR, ask(MID, 0), t + 2, 0), meta(t + 2, false)], vec![p3]);
    w.post_ok(t + 3, &b, &[6]);
}

#[test]
fn i4_decisive_events_need_a_proof_and_the_index_must_exist() {
    let (mut w, t) = base();
    // no proof: Fill and Close
    for ev in [fill(MARKET_EUR, true, MEUR, eur_ask(), t, 255), close(MARKET_EUR, true, MEUR, MID, eur_bid(), t, t - 100, 255)] {
        let b = w.batch(vec![ev, meta(t, false)], vec![]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofMissing);
    }
    // the floating events too
    let b = w.batch(vec![JournalEvent::FloorRaised { time: t, g_peak: 1, l_level: 1, proof: 255 }, meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofMissing);
    // proof index 1 with one proof: the batch does not even decode
    let ev = fill(MARKET_EUR, true, MEUR, eur_ask(), t, 1).to_bytes();
    let raw = raw_batch(1, w.account().journal_hash, &[ev, meta(t, false).to_bytes()], 2, &[p_eur(t).to_bytes().unwrap()], 1);
    let before = w.digest();
    let ev_key = dup(&w.v.evaluator);
    let r = w.post_bytes(t + 1, &ev_key, raw, &[MARKET_EUR]);
    assert_err_at(&r, 1, E::ProofIndexOutOfRange);
    assert_eq!(w.digest(), before);
}

#[test]
fn i4_market_status_and_sessions_decide_whether_opening_is_allowed() {
    let (mut w, t) = base();
    // reduce-only: fills refused, closes allowed
    let f = |m: u16, tt: i64| fill(m, true, MEUR, ask(MID, H_OTHER), tt, 0);
    let b = w.batch(vec![f(MARKET_REDUCE_ONLY, t), meta(t, false)], vec![proof(MARKET_REDUCE_ONLY, t, MID)]);
    w.post_rejected(t + 1, &b, &[MARKET_REDUCE_ONLY], E::MarketReduceOnly);
    let c = close(MARKET_REDUCE_ONLY, true, MEUR, MID, bid(MID, H_OTHER), t, t - 100, 0);
    let b = w.batch(vec![c, meta(t, false)], vec![proof(MARKET_REDUCE_ONLY, t, MID)]);
    w.post_ok(t + 1, &b, &[MARKET_REDUCE_ONLY]);
    // disabled: fills refused, closes allowed (a position is never trapped)
    let b = w.batch(vec![f(MARKET_DISABLED, t + 2), meta(t + 2, false)], vec![proof(MARKET_DISABLED, t + 2, MID)]);
    w.post_rejected(t + 3, &b, &[MARKET_DISABLED], E::MarketDisabled);
    let c = close(MARKET_DISABLED, true, MEUR, MID, bid(MID, H_OTHER), t + 2, t, 0);
    let b = w.batch(vec![c, meta(t + 2, false)], vec![proof(MARKET_DISABLED, t + 2, MID)]);
    w.post_ok(t + 3, &b, &[MARKET_DISABLED]);
    // session window [0, 1) minute of the week: minute 0 opens, minute 1 does not; closes always work
    // the next minute boundary that is Monday 00:00 UTC (minute of week 0)
    let mut monday = t + 60 - t.rem_euclid(60);
    while journal_types::minute_of_week(monday) != 0 {
        monday += 60;
        assert!(monday - t <= 8 * 86_400, "no monday found");
    }
    // `monday` is the first second of a Monday-00:00 minute
    let open_t = monday + 30;
    let closed_t = monday + 60;
    let b = w.batch(vec![f(MARKET_SHORT_SESSION, open_t), meta(open_t, false)], vec![proof(MARKET_SHORT_SESSION, open_t, MID)]);
    w.post_ok(open_t + 1, &b, &[MARKET_SHORT_SESSION]);
    let b = w.batch(vec![f(MARKET_SHORT_SESSION, closed_t), meta(closed_t, false)], vec![proof(MARKET_SHORT_SESSION, closed_t, MID)]);
    w.post_rejected(closed_t + 1, &b, &[MARKET_SHORT_SESSION], E::MarketClosed);
    let c = close(MARKET_SHORT_SESSION, true, MEUR, MID, bid(MID, H_OTHER), closed_t, closed_t - 100, 0);
    let b = w.batch(vec![c, meta(closed_t, false)], vec![proof(MARKET_SHORT_SESSION, closed_t, MID)]);
    w.post_ok(closed_t + 1, &b, &[MARKET_SHORT_SESSION]);
}

#[test]
fn i4_quantity_must_respect_the_markets_step_and_minimum() {
    let (mut w, t) = base();
    // qty_step 1_000, qty_min 1_000_000
    for qty in [0u64, 999_999, 1_500_500, MEUR + 1] {
        let b = w.batch(vec![fill(MARKET_EUR, true, qty, eur_ask(), t, 0), meta(t, false)], vec![p_eur(t)]);
        w.post_rejected(t + 1, &b, &[MARKET_EUR], E::QuantityInvalid);
    }
    for qty in [1_000_000u64, 1_001_000, MEUR] {
        let b = w.batch(vec![fill(MARKET_EUR, true, qty, eur_ask(), t, 0), meta(t, false)], vec![p_eur(t)]);
        w.post_ok(t + 1, &b, &[MARKET_EUR]);
    }
}

// ================================================================ tests that check the helpers themselves

#[test]
fn market_accounts_other_than_markets_are_refused() {
    let (mut w, t) = base();
    let b = w.batch(vec![meta(t, false)], vec![]);
    // pass the SectorConfig account as a "market": owner is the program but it is not a Market
    let ev = dup(&w.v.evaluator);
    w.v.e.set_clock(t + 1);
    use anchor_lang::{InstructionData, ToAccountMetas};
    let mut metas = setl8_lev_trading::accounts::PostJournal {
        evaluator: ev.pubkey(),
        trading_account: w.acct,
        config: config_pda(),
        ruleset: ruleset_pda(1),
        event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0,
        program: PROGRAM_ID,
    }
    .to_account_metas(None);
    metas.push(anchor_lang::solana_program::instruction::AccountMeta::new_readonly(config_pda(), false));
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: metas,
        data: setl8_lev_trading::instruction::PostJournal { batch_bytes: b.to_bytes().unwrap() }.data(),
    };
    let before = w.digest();
    let r = w.v.e.send(&[ix], &[&ev]);
    assert_err_at(&r, 0, E::MarketUnknown);
    assert_eq!(w.digest(), before);
}

// ================================================================ I-5 .. I-12 helpers

fn tweak_close(mut ev: JournalEvent, f: impl FnOnce(&mut i64, &mut bool, &mut i64)) -> JournalEvent {
    if let JournalEvent::Close { pnl, quick, credited, .. } = &mut ev {
        f(pnl, quick, credited);
    }
    ev
}

fn ts(t: i64, p_open: i64, r: i64, l_start: i64, g_peak: i64, l_level: i64) -> JournalEvent {
    JournalEvent::TightStart { time: t, p_open, r, l_start, g_peak, l_level, proof: 0 }
}

/// A batch of `events` + Meta with one EUR proof at time `t` (index 0) posted with the clock at `t + 1`.
fn rejected(w: &mut JEnv, t: i64, events: Vec<JournalEvent>, flat: bool, want: E) {
    let b = simple(w, t, events, flat);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], want);
}

fn accepted(w: &mut JEnv, t: i64, events: Vec<JournalEvent>, flat: bool) {
    let b = simple(w, t, events, flat);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
}

// ================================================================ I-5 Close

#[test]
fn i5_pnl_must_equal_the_engine_pnl_to_the_micro() {
    for delta in [1i64, -1] {
        let (mut w, t) = base();
        let ev = tweak_close(close_long_pnl(t, t - 100, 1_000), |p, _, c| {
            *p += delta;
            *c += delta; // keep credited consistent with the claimed pnl so only pnl is wrong
        });
        rejected(&mut w, t, vec![ev], true, E::PnlMismatch);
    }
    let (mut w, t) = base();
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, 1_000)], true);
    let a = w.account();
    assert_eq!((a.b, a.trades_closed, a.q_tally), (S100K + 1_000, 1, 0));
}

#[test]
fn i5_pnl_of_a_short_close_has_the_opposite_sign() {
    let (mut w, t) = base();
    // short closed at the ask: pnl = entry - exit
    let exit = ask(MID, H_EUR);
    let entry = exit + 700;
    let ev = close(MARKET_EUR, false, MEUR, entry, exit, t, t - 100, 0);
    if let JournalEvent::Close { pnl, .. } = &ev {
        assert_eq!(*pnl, 700);
    }
    accepted(&mut w, t, vec![ev], true);
    assert_eq!(w.account().b, S100K + 700);
}

#[test]
fn i5_quick_flag_boundary_is_45_seconds_inclusive() {
    // age 45: quick (credit halves a profit); age 46: not quick
    let (mut w, t) = base();
    let flip = |ev| tweak_close(ev, |_, q, _| *q = !*q);
    rejected(&mut w, t, vec![flip(close_long_pnl(t, t - 45, 1_000))], true, E::QuickMismatch);
    rejected(&mut w, t, vec![flip(close_long_pnl(t, t - 46, 1_000))], true, E::QuickMismatch);
    // quick claimed at 46 with the matching quick credit is still a quick mismatch
    let ev = tweak_close(close_long_pnl(t, t - 46, 1_000), |_, q, c| {
        *q = true;
        *c = 500;
    });
    rejected(&mut w, t, vec![ev], true, E::QuickMismatch);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 45, 1_000)], true);
    let a = w.account();
    assert_eq!((a.b, a.q_tally), (S100K + 500, 500), "age 45 is quick");
    accepted(&mut w, t, vec![close_long_pnl(t, t - 46, 1_000)], true);
    let a = w.account();
    assert_eq!((a.b, a.q_tally), (S100K + 1_500, 500), "age 46 is not quick: full credit, tally unchanged");
}

#[test]
fn i5_credited_follows_the_quick_profit_rule() {
    let (mut w, t) = base();
    // quick profit 1,001: credited floor(1001/2) = 500 (not 501, not 1001)
    for bad in [501i64, 1_001, 0, 499] {
        let ev = tweak_close(close_long_pnl(t, t - 10, 1_001), |_, _, c| *c = bad);
        rejected(&mut w, t, vec![ev], true, E::CreditMismatch);
    }
    accepted(&mut w, t, vec![close_long_pnl(t, t - 10, 1_001)], true);
    let a = w.account();
    assert_eq!((a.b, a.q_tally), (S100K + 500, 500));
    // quick loss: credited in full, tally untouched
    let ev = tweak_close(close_long_pnl(t, t - 10, -400), |_, _, c| *c = -200);
    rejected(&mut w, t, vec![ev], true, E::CreditMismatch);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 10, -400)], true);
    let a = w.account();
    assert_eq!((a.b, a.q_tally), (S100K + 100, 500), "a quick loss never reduces Q");
    // non-quick profit is credited in full
    let ev = tweak_close(close_long_pnl(t, t - 100, 1_001), |_, _, c| *c = 500);
    rejected(&mut w, t, vec![ev], true, E::CreditMismatch);
    // negative odd quick: not halved
    let ev = tweak_close(close_long_pnl(t, t - 10, -3), |_, _, c| *c = -2);
    rejected(&mut w, t, vec![ev], true, E::CreditMismatch);
}

#[test]
fn i5_timer_start_after_the_close_time_and_zero_fields_are_refused() {
    let (mut w, t) = base();
    let mut ev = close_long_pnl(t, t - 100, 10);
    if let JournalEvent::Close { timer_start, .. } = &mut ev {
        *timer_start = t + 1;
    }
    rejected(&mut w, t, vec![ev], true, E::TimerInvalid);
    let mut ev = close_long_pnl(t, t - 100, 10);
    if let JournalEvent::Close { qty, .. } = &mut ev {
        *qty = 0;
    }
    rejected(&mut w, t, vec![ev], true, E::QuantityInvalid);
    let mut ev = close_long_pnl(t, t - 100, 10);
    if let JournalEvent::Close { entry_e12, .. } = &mut ev {
        *entry_e12 = 0;
    }
    rejected(&mut w, t, vec![ev], true, E::PriceInvalid);
}

// ================================================================ I-6 Fee

#[test]
fn i6_fee_debits_the_balance_and_zero_is_allowed() {
    let (mut w, t) = base();
    accepted(&mut w, t, vec![fee(MARKET_EUR, 0, t)], false);
    assert_eq!(w.account().b, S100K);
    accepted(&mut w, t, vec![fee(MARKET_EUR, 1, t), fee(MARKET_EUR, 2_500_000, t)], false);
    let a = w.account();
    assert_eq!(a.b, S100K - 2_500_001, "a fee lowers b, never raises it");
    assert_eq!(a.q_tally, 0);
}

#[test]
fn i6_fee_amount_beyond_i64_is_refused() {
    let (mut w, t) = base();
    rejected(&mut w, t, vec![fee(MARKET_EUR, u64::try_from(i64::MAX).unwrap() + 1, t)], false, E::FeeInvalid);
    rejected(&mut w, t, vec![fee(MARKET_EUR, u64::MAX, t)], false, E::FeeInvalid);
    // i64::MAX itself fits the type; b - i64::MAX is still an i64, so it is applied (the static floor is report_fail's job)
    accepted(&mut w, t, vec![fee(MARKET_EUR, u64::try_from(i64::MAX).unwrap(), t)], false);
    assert_eq!(w.account().b, S100K - i64::MAX);
    // a second maximal fee would underflow i64: refused by the checked arithmetic, nothing changes
    rejected(&mut w, t, vec![fee(MARKET_EUR, u64::try_from(i64::MAX).unwrap(), t)], false, E::MathOverflow);
}

// ================================================================ I-7 latch, I-8 quick tally

#[test]
fn i7_latch_sets_at_exactly_104_percent_and_never_clears() {
    let (mut w, t) = base();
    // 1.04 S - 1 micro: no latch
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, 3_999_999_999)], true);
    let a = w.account();
    assert_eq!((a.b, a.latch), (usd(104_000) - 1, 0));
    // +1 micro reaches 1.04 S exactly
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, 1)], true);
    let a = w.account();
    assert_eq!((a.b, a.latch), (usd(104_000), 1));
    // a later loss does not clear it
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, -5_000_000_000)], true);
    let a = w.account();
    assert_eq!((a.b, a.latch), (usd(99_000), 1));
}

#[test]
fn i7_latch_is_reached_inside_one_batch_by_a_chain_of_slices() {
    let (mut w, t) = base();
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, 2_000_000_000), close_long_pnl(t, t - 100, 2_000_000_000)], true);
    let a = w.account();
    assert_eq!((a.b, a.latch, a.trades_closed), (usd(104_000), 1, 2));
}

#[test]
fn i8_quick_tally_at_28_percent_is_allowed_and_one_micro_more_is_refused() {
    let (mut w, t) = base();
    // pnl 5,600,000,002 -> credited 2,800,000,001 > 0.028 S
    rejected(&mut w, t, vec![close_long_pnl(t, t - 10, 5_600_000_002)], true, E::QuickLimitExceeded);
    rejected(&mut w, t, vec![close_long_pnl(t, t - 10, 5_600_000_003)], true, E::QuickLimitExceeded);
    // pnl 5,600,000,001 -> credited floor = 2,800,000,000 == limit: allowed
    accepted(&mut w, t, vec![close_long_pnl(t, t - 10, 5_600_000_001)], true);
    let a = w.account();
    assert_eq!((a.q_tally, a.b), (usd(2_800), S100K + usd(2_800)));
    // at the limit any further quick profit is refused; a quick loss and a normal profit are not
    rejected(&mut w, t, vec![close_long_pnl(t, t - 10, 2)], true, E::QuickLimitExceeded);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 10, -2)], true);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, 2)], true);
    assert_eq!(w.account().q_tally, usd(2_800));
}

#[test]
fn i8_quick_tally_accumulates_across_batches_and_inside_a_batch() {
    let (mut w, t) = base();
    w.edit_account(|a| a.q_tally = usd(1_000));
    // two slices of +1,800 credited each: 1,000 + 900 + 900 = 2,800 allowed; a third +2 pushes over
    accepted(&mut w, t, vec![close_long_pnl(t, t - 5, 1_800_000_000), close_long_pnl(t, t - 5, 1_800_000_000)], true);
    assert_eq!(w.account().q_tally, usd(2_800));
    let (mut w2, t2) = base();
    w2.edit_account(|a| a.q_tally = usd(1_000));
    rejected(
        &mut w2,
        t2,
        vec![close_long_pnl(t2, t2 - 5, 1_800_000_000), close_long_pnl(t2, t2 - 5, 1_800_000_000), close_long_pnl(t2, t2 - 5, 2)],
        true,
        E::QuickLimitExceeded,
    );
}

// ================================================================ I-9 tight mode

fn tight_start_ok(p_open: i64, b: i64) -> (i64, i64, i64, i64, i64) {
    // returns (p_open, r, l_start, g_peak, l_level) for the 100k account at closed balance b
    let r = b;
    let l_start = (usd(92_000) + usd(4_000)).min(r);
    let g_peak = p_open;
    let l_level = l_start + (g_peak - usd(1_000)).max(0);
    (p_open, r, l_start, g_peak, l_level)
}

#[test]
fn i9_tight_start_p_open_must_be_strictly_above_one_percent() {
    let (mut w, t) = base();
    let (p, r, ls, g, l) = tight_start_ok(usd(1_000), S100K);
    rejected(&mut w, t, vec![ts(t, p, r, ls, g, l)], false, E::TightStartPOpen);
    let (p, r, ls, g, l) = tight_start_ok(usd(1_000) - 1, S100K);
    rejected(&mut w, t, vec![ts(t, p, r, ls, g, l)], false, E::TightStartPOpen);
    let (p, r, ls, g, l) = tight_start_ok(usd(1_000) + 1, S100K);
    accepted(&mut w, t, vec![ts(t, p, r, ls, g, l)], false);
    let a = w.account();
    assert_eq!((a.tight_on, a.r, a.g_peak, a.l_level), (1, S100K, usd(1_000) + 1, usd(96_000) + 1));
    assert_eq!(a.b, S100K, "tight start moves no money");
}

#[test]
fn i9_tight_start_r_lstart_llevel_gpeak_are_checked_to_the_micro() {
    let (p, r, ls, g, l) = tight_start_ok(usd(1_235), S100K);
    for d in [1i64, -1] {
        let (mut w, t) = base();
        rejected(&mut w, t, vec![ts(t, p, r + d, ls, g, l)], false, E::TightStartR);
        rejected(&mut w, t, vec![ts(t, p, r, ls + d, g, l)], false, E::TightStartLStart);
        rejected(&mut w, t, vec![ts(t, p, r, ls, g, l + d)], false, E::TightStartLLevel);
    }
    let (mut w, t) = base();
    // g_peak above the open profit, or negative
    rejected(&mut w, t, vec![ts(t, p, r, ls, p + 1, l + 1)], false, E::TightStartGPeak);
    rejected(&mut w, t, vec![ts(t, p, r, ls, -1, ls)], false, E::TightStartGPeak);
    // g_peak below p_open is fine (commission lowers E - R)
    let g2 = usd(1_200);
    accepted(&mut w, t, vec![ts(t, p, r, ls, g2, ls + (g2 - usd(1_000)))], false);
    assert_eq!(w.account().g_peak, g2);
}

#[test]
fn i9_tight_start_cap_applies_when_the_balance_is_close_to_the_static_floor() {
    // decision 31: B 93k, F_s 92k -> l_start = min(96k, 93k) = 93k
    let (mut w, t) = base();
    w.edit_account(|a| a.b = usd(93_000));
    let (p, r, ls, g, l) = tight_start_ok(usd(1_100), usd(93_000));
    assert_eq!((ls, l), (usd(93_000), usd(93_100)));
    rejected(&mut w, t, vec![ts(t, p, r, usd(96_000), g, usd(96_100))], false, E::TightStartLStart);
    accepted(&mut w, t, vec![ts(t, p, r, ls, g, l)], false);
    assert_eq!(w.account().l_level, usd(93_100));
}

#[test]
fn i9_tight_start_uses_the_latched_static_floor() {
    // latched: F_s = 96k, so l_start = min(100k, r)
    let (mut w, t) = base();
    w.edit_account(|a| {
        a.latch = 1;
        a.b = usd(104_000);
    });
    let (p, r) = (usd(1_500), usd(104_000));
    let ls = usd(100_000);
    let g = p;
    let l = ls + (g - usd(1_000));
    rejected(&mut w, t, vec![ts(t, p, r, ls + 1, g, l)], false, E::TightStartLStart);
    accepted(&mut w, t, vec![ts(t, p, r, ls, g, l)], false);
    assert_eq!(w.account().l_level, usd(100_500));
}

#[test]
fn i9_tight_start_while_on_is_refused_and_needs_a_proof() {
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    let (p, r, ls, g, l) = tight_start_ok(usd(1_235), S100K);
    rejected(&mut w, t, vec![ts(t, p, r, ls, g, l)], false, E::TightAlreadyOn);
    let (mut w, t) = base();
    let mut ev = ts(t, p, r, ls, g, l);
    if let JournalEvent::TightStart { proof, .. } = &mut ev {
        *proof = 255;
    }
    let b = w.batch(vec![ev, meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofMissing);
}

#[test]
fn i9_floor_raised_needs_a_strictly_higher_peak_and_the_formula_level() {
    let fr = |t: i64, g: i64, l: i64| JournalEvent::FloorRaised { time: t, g_peak: g, l_level: l, proof: 0 };
    let (mut w, t) = base();
    set_tight(&mut w, S100K); // g_peak 1,200, L 96,200 (l_start 96,000)
    rejected(&mut w, t, vec![fr(t, usd(1_200), usd(96_200))], false, E::FloorNotRaised);
    rejected(&mut w, t, vec![fr(t, usd(1_199), usd(96_199))], false, E::FloorNotRaised);
    for d in [1i64, -1] {
        rejected(&mut w, t, vec![fr(t, usd(3_035), usd(98_035) + d)], false, E::FloorLLevel);
    }
    accepted(&mut w, t, vec![fr(t, usd(1_200) + 1, usd(96_200) + 1)], false);
    let a = w.account();
    assert_eq!((a.g_peak, a.l_level, a.tight_on), (usd(1_200) + 1, usd(96_200) + 1, 1));
    // two raises in one batch
    accepted(&mut w, t, vec![fr(t, usd(2_000), usd(97_000)), fr(t, usd(3_000), usd(98_000))], false);
    assert_eq!(w.account().l_level, usd(98_000));
    // tight off
    let (mut w, t) = base();
    rejected(&mut w, t, vec![fr(t, usd(2_000), usd(97_000))], false, E::TightNotOn);
}

#[test]
fn i9_tight_end_requires_a_flat_batch_and_clears_the_levels() {
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    rejected(&mut w, t, vec![JournalEvent::TightEnd { time: t }], false, E::TightEndNotFlat);
    accepted(&mut w, t, vec![JournalEvent::TightEnd { time: t }], true);
    let a = w.account();
    assert_eq!((a.tight_on, a.r, a.g_peak, a.l_level, a.flat), (0, 0, 0, 0, 1));
    assert_eq!(a.lock_until, 0, "a normal end carries no lock");
    // not on
    rejected(&mut w, t, vec![JournalEvent::TightEnd { time: t }], true, E::TightNotOn);
}

#[test]
fn i9_a_flat_batch_cannot_leave_tight_mode_on() {
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    rejected(&mut w, t, vec![fee(MARKET_EUR, 1, t)], true, E::TightNotEnded);
    // the same batch not flat is fine
    accepted(&mut w, t, vec![fee(MARKET_EUR, 1, t)], false);
}

// ================================================================ I-10 breach and lock

fn breach(t: i64, l: i64, b_after: i64) -> JournalEvent {
    JournalEvent::Breach { time: t, l_level: l, b_after, proof: 0 }
}

#[test]
fn i10_breach_b_after_must_be_at_or_below_the_tight_floor() {
    // L = 96,200; closing at -3,800 gives B == L (a touch breaches)
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    let l = usd(96_200);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l, l)], true);
    let a = w.account();
    assert_eq!((a.b, a.tight_on, a.r, a.g_peak, a.l_level, a.flat), (l, 0, 0, 0, 0, 1));
    assert_eq!(a.lock_until, t + 12 * 3600, "12 h lock from the breach time");
    // one micro above the floor is not a breach
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -3_799_999_999), breach(t, l, l + 1)], true, E::BreachNotBelowFloor);
}

#[test]
fn i10_breach_claims_are_checked_against_the_stored_state() {
    let l = usd(96_200);
    for d in [1i64, -1] {
        let (mut w, t) = base();
        set_tight(&mut w, S100K);
        rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l + d, l)], true, E::BreachLLevel);
        rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l, l + d)], true, E::BreachBalance);
    }
    // not tight
    let (mut w, t) = base();
    rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l, S100K - 3_800_000_000)], true, E::TightNotOn);
    // not flat
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l, l)], false, E::BreachNotFlat);
}

#[test]
fn i10_at_or_below_the_static_floor_is_a_fail_not_a_breach() {
    // F_s = 92,000: B == F_s is refused as a breach, B == F_s + 1 micro is a breach
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    rejected(&mut w, t, vec![close_long_pnl(t, t - 100, -8_000_000_000), breach(t, usd(96_200), usd(92_000))], true, E::BreachBelowStaticFloor);
    accepted(&mut w, t, vec![close_long_pnl(t, t - 100, -7_999_999_999), breach(t, usd(96_200), usd(92_000) + 1)], true);
    assert_eq!(w.account().b, usd(92_000) + 1);
}

#[test]
fn i10_fills_are_refused_until_lock_until() {
    let (mut w, t) = base();
    w.edit_account(|a| a.lock_until = t + 100);
    // one second before the lock ends
    let b = w.batch(vec![fill_long(t + 99, 0), meta(t + 99, false)], vec![p_eur(t + 99)]);
    w.post_rejected(t + 100, &b, &[MARKET_EUR], E::AccountLocked);
    // a locked account can still close and pay fees
    accepted(&mut w, t + 99, vec![close_long_pnl(t + 99, t, 7), fee(MARKET_EUR, 1, t + 99)], true);
    // at lock_until it is allowed
    let b = w.batch(vec![fill_long(t + 100, 0), meta(t + 100, false)], vec![p_eur(t + 100)]);
    w.post_ok(t + 101, &b, &[MARKET_EUR]);
}

#[test]
fn i10_a_breach_locks_the_rest_of_its_own_batch() {
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    let l = usd(96_200);
    rejected(
        &mut w,
        t,
        vec![close_long_pnl(t, t - 100, -3_800_000_000), breach(t, l, l), fill_long(t, 0)],
        true,
        E::AccountLocked,
    );
}

// ================================================================ I-11, I-12

#[test]
fn i11_a_fail_event_is_refused() {
    let (mut w, t) = base();
    rejected(&mut w, t, vec![fail_event(t, 0)], true, E::FailEventNotAllowed);
    rejected(&mut w, t, vec![fail_event(t, 255)], true, E::FailEventNotAllowed);
}

#[test]
fn i12_meta_must_be_present_last_and_unique() {
    let (mut w, t) = base();
    // missing
    let b = w.batch(vec![fee(MARKET_EUR, 1, t)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::MetaMissing);
    // not last
    let b = w.batch(vec![meta(t, false), fee(MARKET_EUR, 1, t)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::MetaNotLast);
    // twice (last one is a Meta, still refused)
    let b = w.batch(vec![meta(t, false), meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[], E::MetaTwice);
    let b = w.batch(vec![meta(t, false), fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::MetaTwice);
}

#[test]
fn i12_meta_flat_is_stored_as_given_and_the_engine_version_must_match() {
    let (mut w, t) = base();
    accepted(&mut w, t, vec![], false);
    assert_eq!(w.account().flat, 0);
    accepted(&mut w, t, vec![], true);
    assert_eq!(w.account().flat, 1);
    let mut m = meta(t, false);
    if let JournalEvent::Meta { engine_version, .. } = &mut m {
        *engine_version += 1;
    }
    let b = w.batch(vec![m], vec![]);
    w.post_rejected(t + 1, &b, &[], E::EngineVersionMismatch);
}

// ================================================================ limits

fn raw_post(w: &mut JEnv, t: i64, bytes: Vec<u8>, want: E) {
    let before = w.digest();
    let ev = dup(&w.v.evaluator);
    let r = w.post_bytes(t + 1, &ev, bytes, &[MARKET_EUR]);
    assert_err_at(&r, 1, want);
    assert_eq!(w.digest(), before);
}

#[test]
fn limits_event_count_proof_count_and_bytes() {
    let (mut w, t) = base();
    let a = w.account();
    let m = meta(t, false).to_bytes();
    let f = fee(MARKET_EUR, 1, t).to_bytes();
    let p = p_eur(t).to_bytes().unwrap();
    // 0 events
    raw_post(&mut w, t, raw_batch(1, a.journal_hash, &[], 0, &[], 0), E::EmptyBatch);
    // 9 events
    let mut nine = vec![f.clone(); 8];
    nine.push(m.clone());
    raw_post(&mut w, t, raw_batch(1, a.journal_hash, &nine, 9, &[], 0), E::BatchTooLarge);
    // 8 events (7 fees + meta) are fine
    let mut eight = vec![f.clone(); 7];
    eight.push(m.clone());
    let ev = dup(&w.v.evaluator);
    assert!(w.post_bytes(t + 1, &ev, raw_batch(1, a.journal_hash, &eight, 8, &[], 0), &[MARKET_EUR]).is_ok());
    // 4 proofs
    let (mut w, t) = base();
    let a = w.account();
    raw_post(&mut w, t, raw_batch(1, a.journal_hash, std::slice::from_ref(&m), 1, &vec![p.clone(); 4], 4), E::BatchTooLarge);
    // trailing byte
    let mut bytes = raw_batch(1, a.journal_hash, std::slice::from_ref(&m), 1, &[], 0);
    bytes.push(0);
    raw_post(&mut w, t, bytes, E::BatchMalformed);
    // truncated
    let bytes = raw_batch(1, a.journal_hash, std::slice::from_ref(&m), 1, &[], 0);
    raw_post(&mut w, t, bytes[..bytes.len() - 1].to_vec(), E::BatchMalformed);
    raw_post(&mut w, t, vec![], E::BatchMalformed);
    // an event that references proof 3 of 3
    let fill3 = fill(MARKET_EUR, true, MEUR, eur_ask(), t, 3).to_bytes();
    raw_post(&mut w, t, raw_batch(1, a.journal_hash, &[fill3, m.clone()], 2, &vec![p.clone(); 3], 3), E::ProofIndexOutOfRange);
    // three proofs are accepted when the events use them
    let evs = [fill(MARKET_EUR, true, MEUR, eur_ask(), t, 2).to_bytes(), m.clone()];
    let ev = dup(&w.v.evaluator);
    assert!(w.post_bytes(t + 1, &ev, raw_batch(1, a.journal_hash, &evs, 2, &vec![p.clone(); 3], 3), &[MARKET_EUR]).is_ok());
}

#[test]
fn limits_too_many_market_accounts_are_refused() {
    let (mut w, t) = base();
    let b = w.batch(vec![meta(t, false)], vec![]);
    let nine = [MARKET_EUR; 9];
    w.post_rejected(t + 1, &b, &nine, E::BatchTooLarge);
}

// ================================================================ atomicity

#[test]
fn atomicity_a_bad_last_event_changes_nothing() {
    let (mut w, t) = base();
    let before = w.account();
    let bad = tweak_close(close_long_pnl(t, t - 100, 5), |p, _, c| {
        *p += 1;
        *c += 1;
    });
    let b = simple(&w, t, vec![fee(MARKET_EUR, 10, t), close_long_pnl(t, t - 100, 100), fee(MARKET_EUR, 10, t), bad], true);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::PnlMismatch);
    let after = w.account();
    assert_eq!((after.seq, after.journal_hash, after.b, after.last_batch_time, after.trades_closed), (before.seq, before.journal_hash, before.b, before.last_batch_time, before.trades_closed));
    // the honest batch with the same first_seq still goes through
    let b = simple(&w, t, vec![fee(MARKET_EUR, 10, t), close_long_pnl(t, t - 100, 100)], true);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
    assert_eq!(w.account().seq, 3);
}

// ================================================================ compute units

#[test]
fn compute_units_for_one_four_and_eight_event_batches() {
    let cu = |m: &TransactionMetadata| m.compute_units_consumed;
    let (mut w, t) = base();
    let b1 = simple(&w, t, vec![], false);
    let c1 = cu(&w.post_ok(t + 1, &b1, &[]));
    let b4 = simple(&w, t, vec![fee(MARKET_EUR, 1, t), fee(MARKET_EUR, 1, t), fee(MARKET_EUR, 1, t)], false);
    let c4 = cu(&w.post_ok(t + 1, &b4, &[MARKET_EUR]));
    let mut fees = vec![fee(MARKET_EUR, 1, t); 7];
    let b8 = simple(&w, t, std::mem::take(&mut fees), false);
    let c8 = cu(&w.post_ok(t + 1, &b8, &[MARKET_EUR]));
    // heaviest: 8 events, 3 proofs (2 fills, 2 closes, tight start, floor raise, fee, meta)
    let (mut w, t) = base();
    let (p, r, ls, g, l) = tight_start_ok(usd(1_100), S100K + 20);
    let events = vec![
        fill(MARKET_EUR, true, MEUR, eur_ask(), t, 0),
        fill(MARKET_EUR, false, MEUR, eur_bid(), t, 1),
        close_long_pnl(t, t - 100, 10),
        close_long_pnl(t, t - 100, 10),
        ts(t, p, r, ls, g, l),
        JournalEvent::FloorRaised { time: t, g_peak: usd(1_200), l_level: ls + usd(200), proof: 2 },
        fee(MARKET_EUR, 1, t),
        meta(t, false),
    ];
    // the closes reference proofs too
    let mut events = events;
    for (i, idx) in [(2usize, 0u8), (3, 1)] {
        if let JournalEvent::Close { proof, .. } = &mut events[i] {
            *proof = idx;
        }
    }
    let b = w.batch(events, vec![p_eur(t), p_eur(t), p_eur(t)]);
    let c8h = cu(&w.post_ok(t + 1, &b, &[MARKET_EUR]));
    println!("CU 1-event {c1}, 4-event {c4}, 8-event light {c8}, 8-event heavy (3 proofs) {c8h}");
    for c in [c1, c4, c8, c8h] {
        assert!(c < 1_400_000);
    }
    assert!(c1 < c4 && c4 < c8 && c8 < c8h);
}

// ================================================================ proofs of events without a market field

#[test]
fn i4_floating_events_check_their_proof_against_a_passed_markets_feed_and_age() {
    let fr = |t: i64| JournalEvent::FloorRaised { time: t, g_peak: usd(1_201), l_level: usd(96_201), proof: 0 };
    // a feed no passed market has
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    let b = w.batch(vec![fr(t), meta(t, false)], vec![proof(MARKET_BTC, t, MID)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofWrongFeed);
    // ... but with BTC passed too, the BTC feed is acceptable
    let b = w.batch(vec![fr(t), meta(t, false)], vec![proof(MARKET_BTC, t, MID)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR, MARKET_BTC]);
    // age: 60 s fine, 61 s too old; from the future: +2 fine, +3 not
    let (mut w, t) = base();
    set_tight(&mut w, S100K);
    let b = w.batch(vec![fr(t), meta(t, false)], vec![p_eur(t - 61)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofTooOld);
    let b = w.batch(vec![fr(t), meta(t, false)], vec![p_eur(t + 3)]);
    w.post_rejected(t + 1, &b, &[MARKET_EUR], E::ProofInFuture);
    let b = w.batch(vec![fr(t), meta(t, false)], vec![p_eur(t - 60)]);
    w.post_ok(t + 1, &b, &[MARKET_EUR]);
}
