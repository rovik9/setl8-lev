//! M12 ITEM 4: journal and chain attacks on `post_journal` and `report_fail` (DESIGN 4.4 I-1 to I-12, 8.2, 8.3, 13.2 A-1, A-2,
//! A-4, A-5, A-10, A-12). SELF-audit by the builder, written to BREAK the program, not to confirm it.
//!
//! Every refusal test asserts the EXACT program error code (`Setl8Error` variant, or the Anchor framework code where the
//! framework refuses before the program runs) AND that nothing changed: the whole `TradingAccount` as raw bytes, the config,
//! the wallet profile and the vault `TraderState`, plus a digest of every account `pass_digest` and the markets can touch.
//! A refusal also must not emit an `EventLog`. Classes: BLOCKED (exact refusal), LIMITED (the test pins EXACTLY what still
//! passes), OFF-CHAIN (the control lives in the evaluator or verifier), POLICY (no code control). Findings carry a
//! `// FINDING JRN-n` comment and an `#[ignore = "FINDING ..."]` that asserts the DESIGN behaviour.
//!
//! The fragment `handover/audit/04-journal-chain-attacks.md` lists every test and what it attacks.
mod common;

use anchor_lang::solana_program::{
    instruction::{error::InstructionError, Instruction},
    pubkey::Pubkey,
};
use anchor_lang::{InstructionData, ToAccountMetas};
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::*;
use journal_types::{feed_id, FailReason, JournalBatch, JournalEvent, NativeSha256, PriceProof};
use litesvm::types::{TransactionMetadata, TransactionResult};
use setl8_lev_trading::{accounts, constants as c, errors::Setl8Error as E, instruction};
use solana_account::Account as RawAccount;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

/// The market that carries a source-3 feed (proof `signed_update` is the 32-byte digest). Created by `with_src3`.
const SRC3: u16 = 5;
/// Anchor `InstructionDidNotDeserialize`.
const ANCHOR_IX_DID_NOT_DESERIALIZE: u32 = 102;
/// Largest quantity that is a multiple of the test markets' step (1,000).
#[allow(dead_code)]
const QTY_MAX_STEP: u64 = u64::MAX - u64::MAX % 1_000;
/// Highest compute-unit use allowed for a REFUSED malformed batch (measured about 11,000 to 18,000; a blow-up would show here).
const REFUSAL_CU_CEILING: u64 = 60_000;

// ================================================================ the two instructions under attack

/// Which instruction a case is sent through. `Fail(arg)` is `report_fail` with the wire reason `arg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Post,
    Fail(u8),
}

const SR: u8 = 0; // StaticFloor
#[allow(dead_code)]
const PQ: u8 = 1; // PhaseQuick

fn budget() -> Instruction {
    Instruction {
        program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(),
        accounts: vec![],
        data: [vec![2u8], 600_000u32.to_le_bytes().to_vec()].concat(),
    }
}

/// The instruction for `w`'s current account (market accounts as remaining accounts).
fn ix_for(w: &JEnv, kind: Kind, signer: Pubkey, bytes: Vec<u8>, markets: &[u16]) -> Instruction {
    match kind {
        Kind::Post => {
            let mut metas = accounts::PostJournal {
                evaluator: signer,
                trading_account: w.acct,
                config: config_pda(),
                ruleset: ruleset_pda(w.account().ruleset_version),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None);
            metas.extend(JEnv::market_metas(markets));
            Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::PostJournal { batch_bytes: bytes }.data() }
        }
        Kind::Fail(arg) => w.report_fail_ix(signer, bytes, arg, markets),
    }
}

/// `[compute budget, instruction]` signed by the real evaluator at cluster time `clock`. The instruction is index 1.
fn send(w: &mut JEnv, kind: Kind, clock: i64, bytes: Vec<u8>, markets: &[u16]) -> TransactionResult {
    let ev = dup(&w.v.evaluator);
    let ix = ix_for(w, kind, ev.pubkey(), bytes, markets);
    w.v.e.set_clock(clock);
    w.v.e.send(&[budget(), ix], &[&ev])
}

/// Same with the instruction data replaced as a whole (to lie about the OUTER Borsh vector length).
fn send_data(w: &mut JEnv, kind: Kind, clock: i64, data: Vec<u8>, markets: &[u16]) -> TransactionResult {
    let ev = dup(&w.v.evaluator);
    let mut ix = ix_for(w, kind, ev.pubkey(), vec![], markets);
    ix.data = data;
    w.v.e.set_clock(clock);
    w.v.e.send(&[budget(), ix], &[&ev])
}

/// The instruction data of a correct call for `bytes`.
fn data_of(w: &JEnv, kind: Kind, bytes: Vec<u8>) -> Vec<u8> {
    ix_for(w, kind, dup(&w.v.evaluator).pubkey(), bytes, &[]).data
}

fn accept(w: &mut JEnv, kind: Kind, clock: i64, bytes: Vec<u8>, markets: &[u16]) -> TransactionMetadata {
    match send(w, kind, clock, bytes, markets) {
        Ok(m) => m,
        Err(f) => panic!("{kind:?} was refused: {:?}\n{}", f.err, f.meta.logs.join("\n")),
    }
}

fn bytes_of(b: &JournalBatch) -> Vec<u8> {
    b.to_bytes().expect("batch encodes")
}

// ---------------------------------------------------------------- what "nothing changed" means

/// Raw bytes of the accounts a journal instruction can touch, plus the digest of everything else it could reach.
#[derive(Debug, PartialEq, Eq)]
struct Snap {
    acct: Vec<u8>,
    acct_lamports: u64,
    config: Vec<u8>,
    wallet: Vec<u8>,
    vault_state: Vec<u8>,
    digest: [u8; 32],
}

fn raw(w: &JEnv, k: &Pubkey) -> Vec<u8> {
    w.v.e.svm.get_account(k).map(|a| a.data).unwrap_or_default()
}

fn snap(w: &JEnv) -> Snap {
    let trader = w.trader.pubkey();
    let mut extra: Vec<Pubkey> = (0..9u16).map(market_pda).collect();
    extra.push(w.vts());
    extra.extend((1..=6u64).map(|i| claim_pda(&w.vts(), i)));
    Snap {
        acct: raw(w, &w.acct),
        acct_lamports: w.v.e.svm.get_account(&w.acct).map_or(0, |a| a.lamports),
        config: raw(w, &config_pda()),
        wallet: raw(w, &wallet_pda(&trader)),
        vault_state: raw(w, &w.vts()),
        digest: w.v.digest(&trader, &extra),
    }
}

/// Asserts `r` failed with exactly `want` at instruction `idx`, that no `EventLog` was emitted, and that nothing changed.
/// Returns the compute units the refused transaction used.
fn check_refused(w: &JEnv, before: &Snap, r: &TransactionResult, idx: u8, want: E) -> u64 {
    assert_err_at(r, idx, want);
    let meta = &r.as_ref().unwrap_err().meta;
    assert!(event_logs(meta).is_empty(), "a refused transaction emitted an EventLog ({want:?})");
    assert_eq!(&snap(w), before, "a refused transaction changed the world ({want:?})");
    meta.compute_units_consumed
}

/// Sends `bytes` and demands the exact refusal `want` with an unchanged world. Returns the compute units used.
fn refuse(w: &mut JEnv, kind: Kind, clock: i64, bytes: Vec<u8>, markets: &[u16], want: E) -> u64 {
    let before = snap(w);
    let r = send(w, kind, clock, bytes, markets);
    check_refused(w, &before, &r, 1, want)
}

fn refuse_batch(w: &mut JEnv, kind: Kind, clock: i64, b: &JournalBatch, markets: &[u16], want: E) -> u64 {
    refuse(w, kind, clock, bytes_of(b), markets, want)
}

/// The framework (Anchor) refuses at instruction 1 with `code`; nothing changed.
fn refuse_anchor(w: &mut JEnv, kind: Kind, clock: i64, data: Vec<u8>, markets: &[u16], code: u32) -> u64 {
    let before = snap(w);
    let r = send_data(w, kind, clock, data, markets);
    assert_anchor_at(&r, 1, code);
    let meta = &r.as_ref().unwrap_err().meta;
    assert_eq!(snap(w), before, "a refused transaction changed the world (anchor {code})");
    meta.compute_units_consumed
}

/// The account as stored, so a test can put it back after an ACCEPTED mutation.
#[allow(dead_code)]
fn save(w: &JEnv) -> RawAccount {
    w.v.e.svm.get_account(&w.acct).expect("trading account")
}
#[allow(dead_code)]
fn restore(w: &mut JEnv, a: &RawAccount) {
    w.v.e.svm.set_account(w.acct, a.clone()).unwrap();
}

/// Every account a SUCCESSFUL `report_fail` writes (trading account, wallet profile, vault trader state), to undo it.
#[allow(dead_code)]
fn save_fail_set(w: &JEnv) -> Vec<(Pubkey, RawAccount)> {
    [w.acct, wallet_pda(&w.trader.pubkey()), w.vts()].into_iter().map(|k| (k, w.v.e.svm.get_account(&k).expect("account"))).collect()
}
#[allow(dead_code)]
fn restore_set(w: &mut JEnv, set: &[(Pubkey, RawAccount)]) {
    for (k, a) in set {
        w.v.e.svm.set_account(*k, a.clone()).unwrap();
    }
}

// ---------------------------------------------------------------- worlds and batches

fn src3_digest(fill: u8) -> [u8; 32] {
    [fill; 32]
}

/// A source-3 proof (GMX signed report) for the `SIGNED` feed at time `t`; `signed_update` is the 32-byte blob digest.
fn p3(t: i64, digest: [u8; 32]) -> PriceProof {
    PriceProof {
        source: 3,
        feed_id: feed_id(&NativeSha256, 3, "SIGNED"),
        timestamp_ns: u64::try_from(t).unwrap() * 1_000_000_000,
        quantized_value: i128::from(MID) * 1_000_000,
        signed_update: digest.to_vec(),
    }
}

/// A fill in the source-3 market at the ask of `MID` with half-spread `TEST_HALF_SPREAD_E6`.
fn fill_src3(t: i64, proof_idx: u8) -> JournalEvent {
    fill(SRC3, true, MEUR, ask(MID, TEST_HALF_SPREAD_E6), t, proof_idx)
}

/// A fresh world with the source-3 market (id 5) and a post base time 1,000 s after the account was created.
fn post_world() -> (JEnv, i64) {
    let mut w = JEnv::new();
    w.add_market(SRC3, 3, "SIGNED", 1);
    let t = w.t0 + 1_000;
    (w, t)
}

/// A fresh world whose account sits at the static floor plus 5 micro (so a 5-micro fee lands exactly on the floor).
fn fail_world() -> (JEnv, i64) {
    let (mut w, t) = post_world();
    w.edit_account(|a| a.b = FLOOR + 5);
    (w, t)
}

/// A valid 5-event `post_journal` batch with both proof kinds, markets `[EUR, SRC3]`:
/// `Fill(EUR, proof 0), Fill(SRC3, proof 1), Fee 5, Close +7 (proof 0), Meta(flat)`.
fn rich_post(w: &JEnv, t: i64) -> (JournalBatch, [u16; 2]) {
    let events = vec![
        fill_long(t, 0),
        fill_src3(t, 1),
        fee(MARKET_EUR, 5, t),
        close_long_pnl(t, t - 100, 7),
        meta(t, true),
    ];
    (w.batch(events, vec![p_eur(t), p3(t, src3_digest(0xD1))]), [MARKET_EUR, SRC3])
}

/// A valid `report_fail(StaticFloor)` batch for `fail_world`: `Fill, Fee 5 (lands on the floor), Fail(proof 1 = source 3), Meta(flat)`.
fn rich_fail(w: &JEnv, t: i64) -> (JournalBatch, [u16; 2]) {
    let events = vec![fill_long(t, 0), fee(MARKET_EUR, 5, t), fail_ev(t, FailReason::StaticFloor, FLOOR, 1), meta(t, true)];
    (w.batch(events, vec![p_eur(t), p3(t, src3_digest(0xD2))]), [MARKET_EUR, SRC3])
}

/// A plain valid fail batch `[Fail, Meta]` (account exactly at the floor).
#[allow(dead_code)]
fn plain_fail(w: &JEnv, t: i64) -> JournalBatch {
    w.fail_batch(t, vec![], FailReason::StaticFloor, FLOOR, true)
}

/// A second trader with an account of its own in the same world. Opened at the current clock, like the first.
fn second_account(w: &mut JEnv) -> (Keypair, Pubkey) {
    let b = w.v.new_trader();
    assert_ok(w.v.open(&b, 8));
    let acct = acct_pda(&b.pubkey(), 0);
    (b, acct)
}

/// Points the harness at another trader's account.
fn on(w: &mut JEnv, who: &(Keypair, Pubkey)) {
    w.trader = dup(&who.0);
    w.acct = who.1;
}

fn current(w: &JEnv) -> (Keypair, Pubkey) {
    (dup(&w.trader), w.acct)
}

/// Overwrites the Meta `flat`... helper kept tiny: the `time` of the LAST event (the Meta).
#[allow(dead_code)]
fn with_last_time(mut b: JournalBatch, t: i64) -> JournalBatch {
    let last = b.events.len() - 1;
    b.events[last] = at(&b.events[last], t);
    b
}

/// Raw batch bytes from already-encoded pieces, with honest counts.
fn raw_ok(first_seq: u64, prev: [u8; 32], events: &[Vec<u8>], proofs: &[Vec<u8>]) -> Vec<u8> {
    raw_batch(first_seq, prev, events, u32::try_from(events.len()).unwrap(), proofs, u32::try_from(proofs.len()).unwrap())
}

/// An error code the framework or the program can return as `Custom`; `None` for any other `InstructionError`
/// (a panic or abort shows up as `ProgramFailedToComplete`, an out-of-memory abort the same).
#[allow(dead_code)]
fn custom_code(r: &TransactionResult) -> Option<u32> {
    match r {
        Err(f) => match &f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(code)) => Some(*code),
            _ => None,
        },
        Ok(_) => None,
    }
}

/// The program's own error range.
#[allow(dead_code)]
fn is_program_code(code: u32) -> bool {
    (6000..6300).contains(&code)
}

// ================================================================ SECTION 1: chain attacks on post_journal

/// REPLAY TO ANOTHER ACCOUNT (BLOCKED, `BadPrevHash`). The genesis hash is `sha256("SL8LEV" || account pubkey)`, so a batch
/// built for account A cannot extend account B, even when both are at `seq` 0 (so `first_seq` matches).
#[test]
fn chain_replay_of_a_batch_to_another_account_is_refused() {
    let (mut w, t) = post_world();
    let a = current(&w);
    let b = second_account(&mut w);
    on(&mut w, &a);
    let batch = w.batch(vec![fee(MARKET_EUR, 5, t), close_long_pnl(t, t - 100, 7), meta(t, true)], vec![p_eur(t)]);
    assert_eq!(w.account().seq, 0);
    // B is also at seq 0, so first_seq (1) matches: only the pubkey-bound genesis hash can stop the replay
    on(&mut w, &b);
    assert_eq!(w.account().seq, 0);
    assert_ne!(w.account().journal_hash, batch.prev_hash, "the two genesis hashes differ");
    refuse_batch(&mut w, Kind::Post, t + 1, &batch, &[MARKET_EUR], E::BadPrevHash);
    // A takes it
    on(&mut w, &a);
    let head_a = w.expected_head(&batch);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&batch), &[MARKET_EUR]);
    assert_eq!(w.account().journal_hash, head_a.0);
    // B still refuses it, now also by sequence once B has advanced
    on(&mut w, &b);
    refuse_batch(&mut w, Kind::Post, t + 2, &batch, &[MARKET_EUR], E::BadPrevHash);
    // the same events rebuilt on B's own head are a different chain: same seqs, same events, different head
    let own = w.batch(batch.events.clone(), batch.proofs.clone());
    let head_b = w.expected_head(&own);
    let m = accept(&mut w, Kind::Post, t + 2, bytes_of(&own), &[MARKET_EUR]);
    assert_ne!(head_a.0, head_b.0, "the account key is part of every head");
    assert_eq!(w.account().journal_hash, head_b.0);
    assert_eq!(chain_head_from_logs(&event_logs(&m), &w.acct), head_b.0, "the head recomputed independently from the emitted logs");
    // and after B has advanced to seq 3, A's old batch fails on the sequence first
    refuse_batch(&mut w, Kind::Post, t + 3, &batch, &[MARKET_EUR], E::BadSequence);
}

/// THE SAME BATCH TWICE (BLOCKED, `BadSequence`), in two transactions and inside ONE transaction. The second instruction of a
/// transaction fails, so the whole transaction (including the first, valid post) is rolled back.
#[test]
fn chain_the_same_batch_twice_is_refused() {
    let (mut w, t) = post_world();
    let batch = w.batch(vec![fee(MARKET_EUR, 9, t), meta(t, false)], vec![]);
    let bytes = bytes_of(&batch);
    accept(&mut w, Kind::Post, t + 1, bytes.clone(), &[MARKET_EUR]);
    let after_first = w.account();
    assert_eq!((after_first.seq, after_first.b), (2, S100K - 9));
    refuse(&mut w, Kind::Post, t + 2, bytes.clone(), &[MARKET_EUR], E::BadSequence);
    assert_eq!((w.account().seq, w.account().b), (2, S100K - 9), "the fee was charged once");

    // ONE transaction carrying the batch twice, on a fresh world
    let (mut w, t) = post_world();
    let batch = w.batch(vec![fee(MARKET_EUR, 9, t), meta(t, false)], vec![]);
    let before = snap(&w);
    let ev = dup(&w.v.evaluator);
    let ix = ix_for(&w, Kind::Post, ev.pubkey(), bytes_of(&batch), &[MARKET_EUR]);
    w.v.e.set_clock(t + 1);
    let r = w.v.e.send(&[budget(), ix.clone(), ix], &[&ev]);
    assert_err_at(&r, 2, E::BadSequence);
    assert_eq!(snap(&w), before, "the first (valid) copy was rolled back with the failing second one");
    assert_eq!(w.account().seq, 0);
}

/// SEQUENCE GAP AND REUSE (BLOCKED, `BadSequence`): after a 2-event batch the account is at seq 2 and only 3 is accepted.
/// 0, the already used 1 and 2, the gap 4 and 5, a far gap and `u64::MAX` are all refused, each leaving the world untouched.
#[test]
fn chain_sequence_gap_and_reuse_are_refused() {
    let (mut w, t) = post_world();
    let first = w.batch(vec![fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&first), &[MARKET_EUR]);
    assert_eq!(w.account().seq, 2);
    for bad in [0u64, 1, 2, 4, 5, 100, u64::MAX - 1, u64::MAX] {
        let mut b = w.batch(vec![fee(MARKET_EUR, 1, t + 1), meta(t + 1, false)], vec![]);
        b.first_seq = bad;
        refuse_batch(&mut w, Kind::Post, t + 2, &b, &[MARKET_EUR], E::BadSequence);
    }
    // 3 is accepted: the loop above left no residue
    let ok = w.batch(vec![fee(MARKET_EUR, 1, t + 1), meta(t + 1, false)], vec![]);
    assert_eq!(ok.first_seq, 3);
    accept(&mut w, Kind::Post, t + 2, bytes_of(&ok), &[MARKET_EUR]);
    assert_eq!(w.account().seq, 4);
}

/// SEQUENCE AT THE END OF u64 (BLOCKED, `MathOverflow`). Not reachable in real life (2^64 events); pinned so the arithmetic cannot
/// wrap: a batch that would pass `u64::MAX` is refused, the batch that lands exactly on it is accepted, and the account then
/// refuses every further batch.
#[test]
fn chain_sequence_overflow_is_refused_and_never_wraps() {
    let (mut w, t) = post_world();
    w.edit_account(|a| a.seq = u64::MAX - 1);
    // first_seq == u64::MAX with TWO events: the second event would be seq 2^64
    let two = JournalBatch { first_seq: u64::MAX, prev_hash: w.account().journal_hash, events: vec![fee(MARKET_EUR, 1, t), meta(t, false)], proofs: vec![] };
    refuse_batch(&mut w, Kind::Post, t + 1, &two, &[MARKET_EUR], E::MathOverflow);
    // exactly one event lands on u64::MAX
    let one = JournalBatch { first_seq: u64::MAX, prev_hash: w.account().journal_hash, events: vec![meta(t, false)], proofs: vec![] };
    accept(&mut w, Kind::Post, t + 1, bytes_of(&one), &[]);
    assert_eq!(w.account().seq, u64::MAX);
    // seq + 1 overflows: whatever first_seq says, the batch is refused with MathOverflow (never wraps to 0)
    for first_seq in [0u64, 1, u64::MAX] {
        let b = JournalBatch { first_seq, prev_hash: w.account().journal_hash, events: vec![meta(t, false)], proofs: vec![] };
        refuse_batch(&mut w, Kind::Post, t + 2, &b, &[], E::MathOverflow);
    }
}

/// FORK: two different batches built on the SAME head. Only the first is accepted; the second is refused by `first_seq`, and a
/// fork that corrects `first_seq` but keeps the old `prev_hash` is refused by the hash. Inside ONE transaction the fork is
/// refused and the first branch is rolled back too.
#[test]
fn chain_a_fork_on_the_same_head_is_refused() {
    let (mut w, t) = post_world();
    let head0 = w.account().journal_hash;
    let x = w.batch(vec![fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    let y = w.batch(vec![fee(MARKET_EUR, 2, t), meta(t, false)], vec![]);
    assert_eq!((x.first_seq, x.prev_hash), (y.first_seq, y.prev_hash), "both branches start from one head");
    let head_x = w.expected_head(&x).0;
    assert_ne!(head_x, w.expected_head(&y).0);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&x), &[MARKET_EUR]);
    assert_eq!(w.account().journal_hash, head_x);
    refuse_batch(&mut w, Kind::Post, t + 2, &y, &[MARKET_EUR], E::BadSequence);
    // fork with first_seq corrected but the OLD prev_hash
    let mut y2 = y.clone();
    y2.first_seq = w.account().seq + 1;
    assert_eq!(y2.prev_hash, head0);
    refuse_batch(&mut w, Kind::Post, t + 2, &y2, &[MARKET_EUR], E::BadPrevHash);
    // a continuation (not a fork) is accepted: history stays linear
    let z = w.batch(y.events.clone(), vec![]);
    accept(&mut w, Kind::Post, t + 2, bytes_of(&z), &[MARKET_EUR]);
    assert_eq!(w.account().seq, 4);

    // both branches in one transaction (fresh world)
    let (mut w, t) = post_world();
    let x = w.batch(vec![fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    let y = w.batch(vec![fee(MARKET_EUR, 2, t), meta(t, false)], vec![]);
    let before = snap(&w);
    let ev = dup(&w.v.evaluator);
    let ix_x = ix_for(&w, Kind::Post, ev.pubkey(), bytes_of(&x), &[MARKET_EUR]);
    let ix_y = ix_for(&w, Kind::Post, ev.pubkey(), bytes_of(&y), &[MARKET_EUR]);
    w.v.e.set_clock(t + 1);
    let r = w.v.e.send(&[budget(), ix_x, ix_y], &[&ev]);
    assert_err_at(&r, 2, E::BadSequence);
    assert_eq!(snap(&w), before);
}

/// REORDERED EVENTS. BLOCKED where the order is a rule (Meta last, time order, batch order), LIMITED where the program has no
/// position book (it cannot know that a Close came before its Fill) and where it cannot tell a duplicate from a second event.
#[test]
fn chain_reordered_events_blocked_where_order_is_a_rule_and_limited_elsewhere() {
    let (mut w, t) = post_world();
    // Meta first: MetaNotLast
    let b = w.batch(vec![meta(t, false), fee(MARKET_EUR, 1, t)], vec![]);
    refuse_batch(&mut w, Kind::Post, t + 1, &b, &[MARKET_EUR], E::MetaNotLast);
    // two events whose times are swapped: TimeBackwards (the 1-second case is its own test)
    let b = w.batch(vec![fee(MARKET_EUR, 1, t + 5), fee(MARKET_EUR, 1, t + 4), meta(t + 5, false)], vec![]);
    refuse_batch(&mut w, Kind::Post, t + 10, &b, &[MARKET_EUR], E::TimeBackwards);
    // batch 2 sent before batch 1: the second one's first_seq is in the future
    let b1 = w.batch(vec![fee(MARKET_EUR, 1, t), meta(t, false)], vec![]);
    let head1 = w.expected_head(&b1);
    // build batch 2 on top of batch 1's head without posting batch 1
    let b2 = JournalBatch { first_seq: head1.1 + 1, prev_hash: head1.0, events: vec![fee(MARKET_EUR, 1, t), meta(t, false)], proofs: vec![] };
    refuse_batch(&mut w, Kind::Post, t + 1, &b2, &[MARKET_EUR], E::BadSequence);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&b1), &[MARKET_EUR]);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&b2), &[MARKET_EUR]);
    assert_eq!(w.account().seq, 4);

    // LIMITED: a Close that comes BEFORE its Fill is accepted. The program keeps no position book (DESIGN 1.6: fills are not
    // provable on-chain; the verifier replays positions). Both orders are valid; the heads differ, so the order is committed.
    let (mut w, t) = post_world();
    let fill_first = w.batch(vec![fill_long(t, 0), close_long_pnl(t, t - 100, 7), meta(t, false)], vec![p_eur(t)]);
    let close_first = w.batch(vec![close_long_pnl(t, t - 100, 7), fill_long(t, 0), meta(t, false)], vec![p_eur(t)]);
    let (h_fill_first, h_close_first) = (w.expected_head(&fill_first).0, w.expected_head(&close_first).0);
    assert_ne!(h_fill_first, h_close_first, "the hash chain commits to the order");
    accept(&mut w, Kind::Post, t + 1, bytes_of(&close_first), &[MARKET_EUR]);
    let a = w.account();
    assert_eq!((a.journal_hash, a.trades_closed, a.b), (h_close_first, 1, S100K + 7), "a close with no position was credited");

    // LIMITED: the same Fee twice is applied twice (the program cannot tell a duplicate from a second fee; the verifier
    // recomputes every fee from the engine)
    let (mut w, t) = post_world();
    let b = w.batch(vec![fee(MARKET_EUR, 5, t), fee(MARKET_EUR, 5, t), meta(t, false)], vec![]);
    accept(&mut w, Kind::Post, t + 1, bytes_of(&b), &[MARKET_EUR]);
    assert_eq!(w.account().b, S100K - 10);
}

/// LIMITED (OFF-CHAIN): a proof that NO event references is ignored: not shape-checked beyond decode, not hashed, not emitted.
/// So two batches that differ only in an unreferenced proof reach the SAME head. Nothing can be smuggled into the chain this
/// way (the chain commits to referenced proofs only), but the transaction bytes are not canonical.
#[test]
fn chain_an_unreferenced_proof_is_ignored_and_not_part_of_the_chain() {
    let (mut w, t) = post_world();
    let honest = w.batch(vec![fee(MARKET_EUR, 3, t), meta(t, false)], vec![]);
    let mut junk = honest.clone();
    // garbage: unknown source, wrong feed, value negative, time in 1970
    junk.proofs = vec![PriceProof { source: 77, feed_id: [0xEE; 32], timestamp_ns: 1, quantized_value: -5, signed_update: vec![1, 2, 3] }];
    let honest_head = w.expected_head(&honest);
    assert_eq!(w.expected_head(&junk), honest_head, "an unreferenced proof is not in the hash");
    let m = accept(&mut w, Kind::Post, t + 1, bytes_of(&junk), &[MARKET_EUR]);
    assert_eq!(w.account().journal_hash, honest_head.0);
    assert!(event_logs(&m).iter().all(|l| l.proof_bytes.is_empty()), "the junk proof was not emitted");
}

// ================================================================ SECTION 2: wire-format attacks (both instructions)

const KINDS: [Kind; 2] = [Kind::Post, Kind::Fail(SR)];

/// A world plus the pieces of a minimal valid batch of `kind`: `[X, Meta]` with one proof, X = a Fill (post) or the Fail (report_fail).
struct Wire {
    w: JEnv,
    t: i64,
    kind: Kind,
    markets: Vec<u16>,
    ev: Vec<Vec<u8>>,
    pr: Vec<Vec<u8>>,
    head: [u8; 32],
}

fn wire(kind: Kind) -> Wire {
    let (mut w, t) = post_world();
    let ev = match kind {
        Kind::Post => vec![fill_long(t, 0).to_bytes(), meta(t, false).to_bytes()],
        Kind::Fail(_) => {
            w.edit_account(|a| a.b = FLOOR);
            vec![fail_ev(t, FailReason::StaticFloor, FLOOR, 0).to_bytes(), meta(t, true).to_bytes()]
        }
    };
    let head = w.account().journal_hash;
    Wire { w, t, kind, markets: vec![MARKET_EUR], ev, pr: vec![p_eur(t).to_bytes().unwrap()], head }
}

impl Wire {
    fn honest(&self) -> Vec<u8> {
        raw_ok(1, self.head, &self.ev, &self.pr)
    }
    fn refuse(&mut self, bytes: Vec<u8>, want: E) -> u64 {
        let (kind, clock, markets) = (self.kind, self.t + 1, self.markets.clone());
        refuse(&mut self.w, kind, clock, bytes, &markets, want)
    }
    fn accept(&mut self, bytes: Vec<u8>) -> TransactionMetadata {
        let (kind, clock, markets) = (self.kind, self.t + 1, self.markets.clone());
        accept(&mut self.w, kind, clock, bytes, &markets)
    }
}

/// WIRE: bytes after the batch, or inside the last event, are refused (BLOCKED). The exact code depends on how the decoder
/// misreads the shifted bytes; the codes below are derived by hand from `codec.rs`.
#[test]
fn wire_trailing_bytes_are_refused() {
    for kind in KINDS {
        let mut x = wire(kind);
        // the honest bytes are valid (a control: the same bytes are accepted in a twin world below)
        for extra in [vec![0u8], vec![0xFF], vec![0u8; 3], vec![0xAB; 300]] {
            let mut b = x.honest();
            b.extend(extra);
            x.refuse(b, E::BatchMalformed);
        }
        // one extra byte INSIDE the last event (Meta): the u32 proof count is then read from [extra, 1, 0, 0] = extra + 256,
        // which is above the cap of 3 for every value: BatchTooLarge
        for extra in [0x00u8, 0x01, 0x04, 0xFF] {
            let mut evs = x.ev.clone();
            evs.last_mut().unwrap().push(extra);
            x.refuse(raw_ok(1, x.head, &evs, &x.pr), E::BatchTooLarge);
        }
        // the same extra byte in a batch WITHOUT proofs (post only: a Fee and a Meta): the count is read from [extra, 0, 0, 0]
        if kind == Kind::Post {
            let (fee_b, meta_b) = (fee(MARKET_EUR, 1, x.t).to_bytes(), meta(x.t, false).to_bytes());
            for (extra, want) in [(0x00u8, E::BatchMalformed), (0x01, E::BatchMalformed), (0x03, E::BatchMalformed), (0x04, E::BatchTooLarge), (0xFF, E::BatchTooLarge)] {
                let mut m = meta_b.clone();
                m.push(extra);
                // 0x00: count 0, one byte left over (trailing). 0x01 and 0x03: a proof is announced but the bytes end. 0x04, 0xFF: above the cap.
                x.refuse(raw_ok(1, x.head, &[fee_b.clone(), m], &[]), want);
            }
        }
        // the honest bytes pass (the world was not changed by any refusal above)
        x.accept(x.honest());
    }
}

/// LIMITED: Anchor ignores bytes AFTER the instruction's arguments (its deserializer does not demand an exhausted slice). The
/// batch itself is unaffected (it is length-prefixed and checked for trailing bytes), so nothing can be smuggled into the chain;
/// what changes is only the transaction's size. Pinned: a later change of the framework behaviour breaks this test.
#[test]
fn wire_bytes_after_the_instruction_arguments_are_ignored_by_the_framework() {
    for kind in KINDS {
        let mut x = wire(kind);
        let mut d = data_of(&x.w, kind, x.honest());
        d.extend([0xCD; 7]);
        let (clock, markets) = (x.t + 1, x.markets.clone());
        let r = send_data(&mut x.w, kind, clock, d, &markets);
        assert!(r.is_ok(), "{kind:?}: trailing instruction data is ignored by the framework: {:?}", r.err().map(|f| f.err));
        // the stored head is exactly the honest chain's
        assert_eq!(x.w.account().seq, 2);
    }
}

/// WIRE: Borsh length prefixes that lie, for both instructions. 2^32-1 and 2^16 must be refused cheaply and with the program's
/// own error (an allocation of that size would abort the program, not return a `Custom` error, and would show as CU blow-up).
#[test]
fn wire_lying_borsh_length_prefixes_are_refused_without_allocation() {
    let mut worst = 0u64;
    for kind in KINDS {
        let mut x = wire(kind);
        let (head, ev, pr) = (x.head, x.ev.clone(), x.pr.clone());
        // ---- the event count
        for (n, want) in [(u32::MAX, E::BatchTooLarge), (65_536, E::BatchTooLarge), (255, E::BatchTooLarge), (9, E::BatchTooLarge), (0, E::EmptyBatch)] {
            worst = worst.max(x.refuse(raw_batch(1, head, &ev, n, &pr, 1), want));
        }
        // announces fewer events than present: the proof count is then read from the Meta bytes (8, time...): above the cap
        worst = worst.max(x.refuse(raw_batch(1, head, &ev, 1, &pr, 1), E::BatchTooLarge));
        // announces more events than present: the next "event" runs into the end of the bytes
        worst = worst.max(x.refuse(raw_batch(1, head, &ev, 3, &pr, 1), E::BatchMalformed));
        // ---- the proof count
        for n in [u32::MAX, 65_536, 4] {
            worst = worst.max(x.refuse(raw_batch(1, head, &ev, 2, &pr, n), E::BatchTooLarge));
        }
        worst = worst.max(x.refuse(raw_batch(1, head, &ev, 2, &pr, 2), E::BatchMalformed)); // one more than present: EOF
        worst = worst.max(x.refuse(raw_batch(1, head, &ev, 2, &pr, 0), E::BatchMalformed)); // fewer: the proof is trailing
        // ---- the signed_update length inside the proof
        let honest_proof = pr[0].clone();
        assert_eq!(honest_proof.len(), 61);
        for len in [u32::MAX, 1 << 31, 65_536, 513] {
            let mut p = honest_proof[..57].to_vec();
            p.extend(len.to_le_bytes());
            worst = worst.max(x.refuse(raw_ok(1, head, &ev, &[p]), E::ProofBadSignedUpdate));
        }
        for len in [1u32, 255, 512] {
            // within the cap, but the bytes are not there
            let mut p = honest_proof[..57].to_vec();
            p.extend(len.to_le_bytes());
            worst = worst.max(x.refuse(raw_ok(1, head, &ev, &[p]), E::BatchMalformed));
        }
        // 512 real bytes on a source-2 proof pass the decoder and are refused by the shape rule (source 2 carries none)
        let mut p = honest_proof[..57].to_vec();
        p.extend(512u32.to_le_bytes());
        p.extend(vec![7u8; 512]);
        worst = worst.max(x.refuse(raw_ok(1, head, &ev, &[p]), E::ProofBadSignedUpdate));
        // ---- the OUTER vector (the instruction argument itself): the framework refuses before the program runs
        let honest = x.honest();
        let good = data_of(&x.w, kind, honest.clone());
        let (clock, markets) = (x.t + 1, x.markets.clone());
        // (65,536 and 2^32-1 are the FINDING JRN-1 tests below: the program aborts instead of returning 102)
        for lie in [1_233u32, 8_192, 20_000] {
            let mut d = good[..8].to_vec();
            d.extend(lie.to_le_bytes());
            d.extend(&honest);
            if let Kind::Fail(arg) = kind {
                d.push(arg);
            }
            worst = worst.max(refuse_anchor(&mut x.w, kind, clock, d, &markets, ANCHOR_IX_DID_NOT_DESERIALIZE));
        }
        // an outer prefix SHORTER than the bytes: the program sees a short batch (and the framework drops the rest)
        let mut d = good[..8].to_vec();
        d.extend(5u32.to_le_bytes());
        d.extend(&honest[..5]);
        if let Kind::Fail(arg) = kind {
            d.push(arg);
        }
        x.refuse_data(d, E::BatchMalformed);
        // an outer prefix of 0: an empty batch buffer does not decode
        let mut d = good[..8].to_vec();
        d.extend(0u32.to_le_bytes());
        if let Kind::Fail(arg) = kind {
            d.push(arg);
        }
        x.refuse_data(d, E::BatchMalformed);
    }
    println!("worst refused-malformed compute units: {worst}");
    assert!(worst < REFUSAL_CU_CEILING, "a malformed batch used {worst} CU: a length prefix must never cost allocation work");
}

impl Wire {
    fn refuse_data(&mut self, data: Vec<u8>, want: E) {
        let before = snap(&self.w);
        let (kind, clock, markets) = (self.kind, self.t + 1, self.markets.clone());
        let r = send_data(&mut self.w, kind, clock, data, &markets);
        check_refused(&self.w, &before, &r, 1, want);
    }
}

/// WIRE: TRUNCATION. Every proper prefix of a valid batch (all points, from 0 bytes to len - 1) is refused with `BatchMalformed`
/// and changes nothing, for both instructions. The instruction data itself truncated at every point is refused by the framework.
#[test]
fn wire_every_truncation_point_of_a_valid_batch_is_refused() {
    // post: the rich batch (two proof kinds, 5 events)
    let (mut w, t) = post_world();
    let (b, markets) = rich_post(&w, t);
    let honest = bytes_of(&b);
    let before = snap(&w);
    let mut worst = 0u64;
    for cut in 0..honest.len() {
        let r = send(&mut w, Kind::Post, t + 1, honest[..cut].to_vec(), &markets);
        worst = worst.max(check_refused(&w, &before, &r, 1, E::BatchMalformed));
    }
    // the instruction data cut at every point after the discriminator
    let data = data_of(&w, Kind::Post, honest.clone());
    for cut in 8..data.len() {
        let r = send_data(&mut w, Kind::Post, t + 1, data[..cut].to_vec(), &markets);
        assert_anchor_at(&r, 1, ANCHOR_IX_DID_NOT_DESERIALIZE);
        assert_eq!(snap(&w), before);
    }
    println!("post: {} byte truncations + {} data truncations, worst CU {worst}", honest.len(), data.len() - 8);
    assert!(worst < REFUSAL_CU_CEILING);
    accept(&mut w, Kind::Post, t + 1, honest.clone(), &markets);

    // report_fail: the rich fail batch
    let (mut w, t) = fail_world();
    let (b, markets) = rich_fail(&w, t);
    let honest = bytes_of(&b);
    let before = snap(&w);
    let mut worst = 0u64;
    for cut in 0..honest.len() {
        let r = send(&mut w, Kind::Fail(SR), t + 1, honest[..cut].to_vec(), &markets);
        worst = worst.max(check_refused(&w, &before, &r, 1, E::BatchMalformed));
    }
    let data = data_of(&w, Kind::Fail(SR), honest.clone());
    for cut in 8..data.len() {
        let r = send_data(&mut w, Kind::Fail(SR), t + 1, data[..cut].to_vec(), &markets);
        assert_anchor_at(&r, 1, ANCHOR_IX_DID_NOT_DESERIALIZE);
        assert_eq!(snap(&w), before);
    }
    println!("report_fail: {} byte truncations + {} data truncations, worst CU {worst}", honest.len(), data.len() - 8);
    assert!(worst < REFUSAL_CU_CEILING);
    accept(&mut w, Kind::Fail(SR), t + 1, honest, &markets);
    assert_eq!(w.account().status, c::STATUS_FAILED);
}

/// A-12 CAPS (BLOCKED): 0 and 9 events, 4 proofs, 9 market accounts; the boundaries 8 events, 3 proofs and 8 market accounts pass.
#[test]
fn caps_event_proof_and_market_account_limits_for_both_instructions() {
    for kind in KINDS {
        let mut x = wire(kind);
        let (head, t) = (x.head, x.t);
        let one = x.ev.clone();
        let meta_b = one[1].clone();
        let head_ev = one[0].clone();
        let fee0 = fee(MARKET_EUR, 0, t).to_bytes();
        let pr = x.pr.clone();
        // 0 events
        x.refuse(raw_batch(1, head, &[], 0, &[], 0), E::EmptyBatch);
        // 9 events: Post = 8 fees + Meta; report_fail = 7 fees + Fail + Meta
        let nine: Vec<Vec<u8>> = match kind {
            Kind::Post => [vec![fee0.clone(); 8], vec![meta_b.clone()]].concat(),
            Kind::Fail(_) => [vec![fee0.clone(); 7], vec![head_ev.clone(), meta_b.clone()]].concat(),
        };
        assert_eq!(nine.len(), 9);
        x.refuse(raw_ok(1, head, &nine, &pr), E::BatchTooLarge);
        // 4 proofs (three would be fine)
        x.refuse(raw_ok(1, head, &one, &vec![pr[0].clone(); 4]), E::BatchTooLarge);
        // 9 market accounts (a batch of one event needs one)
        let before = snap(&x.w);
        let nine_markets = [MARKET_EUR; 9];
        let honest = x.honest();
        let r = send(&mut x.w, kind, t + 1, honest, &nine_markets);
        check_refused(&x.w, &before, &r, 1, E::BatchTooLarge);
        // BOUNDARIES: 8 events + 3 proofs + 8 market accounts are accepted
        let eight: Vec<Vec<u8>> = match kind {
            Kind::Post => [vec![fee0.clone(); 7], vec![meta_b.clone()]].concat(),
            Kind::Fail(_) => [vec![fee0.clone(); 6], vec![head_ev.clone(), meta_b.clone()]].concat(),
        };
        assert_eq!(eight.len(), 8);
        let bytes = raw_ok(1, head, &eight, &vec![pr[0].clone(); 3]);
        let eight_markets = [MARKET_EUR; 8];
        let m = accept(&mut x.w, kind, t + 1, bytes, &eight_markets);
        assert_eq!(event_logs(&m).len(), 8);
        assert_eq!(x.w.account().seq, 8);
    }
}

/// Pins TODAY's behaviour for the lying OUTER length prefix (the `Vec<u8>` instruction argument): with a prefix the 32 KB heap
/// cannot hold (2^16 and 2^32-1 here) the argument deserializer tries to allocate it and the program ABORTS ("memory allocation
/// failed, out of memory", `ProgramFailedToComplete`) instead of returning Anchor's `InstructionDidNotDeserialize`. The
/// transaction fails either way and nothing changes; the abort costs about 2,300 CU. See FINDING JRN-1 below. If JRN-1 is fixed this
/// test must be changed to the 102 expectation (and the ignore of the finding test removed).
#[test]
fn wire_outer_length_prefix_beyond_the_heap_aborts_the_program_today() {
    for kind in KINDS {
        let mut x = wire(kind);
        let honest = x.honest();
        let good = data_of(&x.w, kind, honest.clone());
        let (clock, markets) = (x.t + 1, x.markets.clone());
        for lie in [65_536u32, u32::MAX] {
            let mut d = good[..8].to_vec();
            d.extend(lie.to_le_bytes());
            d.extend(&honest);
            if let Kind::Fail(arg) = kind {
                d.push(arg);
            }
            let before = snap(&x.w);
            let r = send_data(&mut x.w, kind, clock, d, &markets);
            let f = r.as_ref().expect_err("the call fails");
            assert!(
                matches!(&f.err, TransactionError::InstructionError(1, InstructionError::ProgramFailedToComplete)),
                "{kind:?} lie {lie}: expected the abort, got {:?}",
                f.err
            );
            assert!(f.meta.logs.iter().any(|l| l.contains("memory allocation failed")), "{kind:?}: {:?}", f.meta.logs);
            assert!(f.meta.compute_units_consumed < 5_000);
            assert!(event_logs(&f.meta).is_empty());
            assert_eq!(snap(&x.w), before, "the aborted call changed something");
        }
    }
}

// FINDING JRN-1 (low): a lying OUTER Borsh length prefix of the `batch_bytes` argument (2^16 or 2^32-1, with a few real bytes after it)
// makes `post_journal` and `report_fail` abort with an out-of-memory panic instead of returning an error. DESIGN 13 / the M12 prompt
// ask for "refused, no panic". Anyone can trigger it without being the evaluator (the argument is decoded before the signer check),
// but it only fails the caller's own transaction and changes nothing. This test asserts the correct behaviour: the framework's
// `InstructionDidNotDeserialize` (102).
#[test]
#[ignore = "FINDING JRN-1: a lying outer Borsh length prefix (>= 32 KB) aborts post_journal/report_fail with an out-of-memory panic instead of an error"]
fn finding_jrn_1_outer_length_prefix_must_be_refused_not_aborted() {
    for kind in KINDS {
        let mut x = wire(kind);
        let honest = x.honest();
        let good = data_of(&x.w, kind, honest.clone());
        let (clock, markets) = (x.t + 1, x.markets.clone());
        for lie in [65_536u32, u32::MAX] {
            let mut d = good[..8].to_vec();
            d.extend(lie.to_le_bytes());
            d.extend(&honest);
            if let Kind::Fail(arg) = kind {
                d.push(arg);
            }
            refuse_anchor(&mut x.w, kind, clock, d, &markets, ANCHOR_IX_DID_NOT_DESERIALIZE);
        }
    }
}
