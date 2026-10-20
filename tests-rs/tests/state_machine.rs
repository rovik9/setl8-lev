//! M12 AUDIT, ITEM 5: THE STATE MACHINE, attacked exhaustively.
//!
//! Part A is a hand-written transition table: (instruction) x (account state) x (sector paused yes/no) with the signer
//! class that IS allowed to call the instruction. The expected result of every cell was written by hand from DESIGN 4.4,
//! 4.6, 4.7, 6.1 to 6.4, 10.1 to 10.8 and 12, NOT read off the program. Where two preconditions of a cell are both violated the
//! DESIGN does not say which error comes first, so such a cell lists every error the DESIGN allows (`any`); a cell with
//! exactly one violated precondition names the one error (`x`). A cell that is allowed names the exact (status, held,
//! pass_pending) triple that must follow, and a full-struct comparison of the account checks that nothing else moved.
//!
//! The account states of the table (the "grid" is status x held x pass_pending, 4 x 2 x 4 = 32 coordinates; only 10 of them
//! can exist, `reachable()` says which and `reach_*` tests show why):
//!   A0 A1 A2   Active, not held, pass_pending 0 / 1 / 2
//!   H0 H1 H2   Active, HELD,      pass_pending 0 / 1 / 2
//!   F          Failed (report_fail), resettable            FX  Failed, already reset (`reset_at` != 0)
//!   FV         Failed by fail_for_violation (marker 3), wallet banned
//!   FVU        like FV, wallet unbanned afterwards (the marker, not the ban, refuses a reset)
//!   AB         Abandoned (the vault's 7-day idle rule)     GR  Graduated (phase 5 finalized)
//! F, FV, FVU, FX, AB, GR, A*, H* are all built with REAL instructions (report_fail, hold + fail_for_violation, release
//! after 7 idle days, four finalizes, ...); only the b/flat/pending fields that an honest journal would have needed
//! minutes to produce are written with `edit_account`, and the cells say so. In the terminal worlds `b` is edited to 1.08 S
//! so that the pass condition would hold: the status check is then the ONLY violated precondition of a pass instruction.
//!
//! Part B (`sweep_*`) runs the 22 non-existing grid coordinates x paused x every instruction with the bytes edited in: no
//! expected value is claimed (the state cannot exist), but the program must return an error code (never crash, never a
//! non-`Custom` failure) and a refusal must leave every byte unchanged.
//! Part C (`reach_*`) shows with real instructions that the non-existing coordinates cannot be reached.
//! Part D (`b_*`) are the pass boundaries to the second and the races.
mod common;

use anchor_lang::solana_program::instruction::error::InstructionError;
use anchor_lang::{AccountDeserialize, AccountSerialize};
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::{acct_pda, vault_trader_state_pda};
use common::*;
use journal_types::FailReason;
use litesvm::types::TransactionResult;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E, state::TradingAccount};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

const DELAY: i64 = 172_800;
const CAP: u64 = 100_000_000_000;
const WINDOW: u32 = 86_400;
const TARGET: i64 = 108_000_000_000;
const WEEK: i64 = 604_800;

// ============================================================================================ small helpers

fn bytes_of(a: &TradingAccount) -> Vec<u8> {
    let mut d = Vec::new();
    a.try_serialize(&mut d).unwrap();
    d
}

fn clone_acct(a: &TradingAccount) -> TradingAccount {
    TradingAccount::try_deserialize(&mut bytes_of(a).as_slice()).unwrap()
}

/// `after` must equal `before` with `f` applied, byte for byte.
fn assert_only(ctx: &str, before: &TradingAccount, after: &TradingAccount, f: impl FnOnce(&mut TradingAccount)) {
    let mut exp = clone_acct(before);
    f(&mut exp);
    assert_eq!(bytes_of(&exp), bytes_of(after), "{ctx}: the account changed in more (or fewer) fields than the table says");
}

/// The custom error code of a failed transaction at instruction `idx`, or a description of what else happened.
fn code_at(r: &TransactionResult, idx: u8) -> Result<u32, String> {
    match r {
        Ok(_) => Err("the transaction succeeded".to_string()),
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, InstructionError::Custom(code)) if *i == idx => Ok(*code),
            other => Err(format!("{other:?}\n{}", f.meta.logs.join("\n"))),
        },
    }
}

fn tri(a: &TradingAccount) -> (u8, u8, u8) {
    (a.status, a.held, a.pass_pending)
}

/// The ten (status, held, pass_pending) coordinates that can exist (DESIGN 5.1, 10.3, decision 60), by hand:
/// Active with held 0/1 and pending 0/1/2; Failed with pending 0 (reset-able) or 3 (violation marker); Abandoned and
/// Graduated with pending 0. A terminal account is never held and never pending (`end_account` clears both).
fn reachable(status: u8, held: u8, pending: u8) -> bool {
    matches!((status, held, pending), (0, 0 | 1, 0..=2) | (1, 0, 0 | 3) | (2, 0, 0) | (3, 0, 0))
}

/// Wallet counters must equal what the accounts say, and every account must sit on an existing coordinate.
fn world_invariants(w: &JEnv, ctx: &str) {
    let trader = w.trader.pubkey();
    let wallet = w.v.e.wallet(&trader);
    let (mut active, mut held) = (0u8, 0u8);
    for id in 0..8u64 {
        let Some(acc) = w.v.e.svm.get_account(&acct_pda(&trader, id)) else { continue };
        if acc.lamports == 0 || acc.data.is_empty() {
            continue;
        }
        let a = TradingAccount::try_deserialize(&mut acc.data.as_slice()).unwrap();
        assert!(reachable(a.status, a.held, a.pass_pending), "{ctx}: account {id} sits on a coordinate that cannot exist: {:?}", tri(&a));
        active += u8::from(a.status == c::STATUS_ACTIVE);
        held += a.held;
    }
    assert_eq!(wallet.active_count, active, "{ctx}: wallet.active_count disagrees with the accounts");
    assert_eq!(wallet.held_count, held, "{ctx}: wallet.held_count disagrees with the accounts");
}

// ============================================================================================ the table

#[derive(Clone, Copy)]
enum W {
    /// Allowed; the account must end on this (status, held, pass_pending).
    Ok(u8, u8, u8),
    /// Allowed; the account is closed (rent returned).
    Closed,
    /// Allowed; the failed account is marked reset and a new Active account exists.
    Reset,
    /// Refused with exactly this error.
    X(E),
    /// Refused; the DESIGN allows any of these errors (several preconditions are violated at once).
    Any(&'static [E]),
}

const fn ok(s: u8, h: u8, p: u8) -> W {
    W::Ok(s, h, p)
}
const fn x(e: E) -> W {
    W::X(e)
}
const fn any(e: &'static [E]) -> W {
    W::Any(e)
}

const NA: E = E::AccountNotActive;
const PP: E = E::PassPending;
const NP: E = E::NoPendingPass;
const AH: E = E::AccountHeld;
const PAU: E = E::Paused;
const ALR: E = E::PassAlreadyPending;
const NH: E = E::NotHeld;
const AL: E = E::AlreadyHeld;
const NT: E = E::AccountNotTerminal;
const RNA: E = E::ResetNotAllowed;
const WB: E = E::WalletBanned;
const VNR: E = E::ViolationNoReset;
const AAR: E = E::AccountAlreadyReset;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ins {
    PostMeta,
    PostFill,
    ReportFail,
    Request,
    Cancel,
    Confirm,
    Reject,
    Veto,
    Finalize,
    Hold,
    ReleaseHold,
    Violation,
    Touch,
    ReleaseAcct,
    Close,
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    A0,
    A1,
    A2,
    H0,
    H1,
    H2,
    F,
    FV,
    FVU,
    FX,
    AB,
    GR,
}
const STATES: [St; 12] = [St::A0, St::A1, St::A2, St::H0, St::H1, St::H2, St::F, St::FV, St::FVU, St::FX, St::AB, St::GR];

struct Row {
    ins: Ins,
    /// The program instruction this row exercises (must be a real instruction of lib.rs).
    program_ix: &'static str,
    signer: &'static str,
    /// Columns follow `STATES`.
    open: [W; 12],
    paused: [W; 12],
}

const fn same(ins: Ins, program_ix: &'static str, signer: &'static str, a: [W; 12]) -> Row {
    Row { ins, program_ix, signer, open: a, paused: a }
}

// column order:  A0  A1  A2  H0  H1  H2  F  FV  FVU  FX  AB  GR
const TABLE: [Row; 16] = [
    // post_journal with a Meta-only batch (DESIGN I-1: Active, pass_pending != 2; a held account may post anything that is not an
    // exposure-increasing Fill; 6.1 set_paused: post_journal keeps working while paused)
    same(
        Ins::PostMeta,
        "post_journal",
        "evaluator",
        [ok(0, 0, 0), ok(0, 0, 1), x(PP), ok(0, 1, 0), ok(0, 1, 1), x(PP), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // post_journal with a Fill (exposure-increasing: also needs held == 0, I-1 / 6.3 hold_account)
    same(
        Ins::PostFill,
        "post_journal",
        "evaluator",
        [ok(0, 0, 0), ok(0, 0, 1), x(PP), x(AH), x(AH), any(&[PP, AH]), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // report_fail at the static floor (I-11): Active, pass_pending != 2; a pending request (1) is cleared, a hold is cleared;
    // works while paused (6.1). A2/H2 are written by editing `b` to the floor (an honest account cannot have b at the floor while
    // pass_pending is 2, the journal is frozen): the cell tests the precondition, not an honest history.
    same(
        Ins::ReportFail,
        "report_fail",
        "evaluator",
        [ok(1, 0, 0), ok(1, 0, 0), x(PP), ok(1, 0, 0), ok(1, 0, 0), x(PP), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // request_advance (6.2): Active, not held, not paused, flat, tight off, nothing pending, b >= 1.08 S
    Row {
        ins: Ins::Request,
        program_ix: "request_advance",
        signer: "trader",
        open: [ok(0, 0, 1), x(ALR), x(ALR), x(AH), any(&[AH, ALR]), any(&[AH, ALR]), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
        paused: [
            x(PAU),
            any(&[PAU, ALR]),
            any(&[PAU, ALR]),
            any(&[PAU, AH]),
            any(&[PAU, AH, ALR]),
            any(&[PAU, AH, ALR]),
            any(&[PAU, NA]),
            any(&[PAU, NA]),
            any(&[PAU, NA]),
            any(&[PAU, NA]),
            any(&[PAU, NA]),
            any(&[PAU, NA]),
        ],
    },
    // cancel_pass (6.2): ONLY if pass_pending == 1 and 3,600 s have passed; status, hold and pause are not mentioned, so a held
    // account's trader may cancel and a paused sector does not stop it. The fixture requested 4,000 s ago.
    same(
        Ins::Cancel,
        "cancel_pass",
        "trader",
        [x(NP), ok(0, 0, 0), x(NP), x(NP), ok(0, 1, 0), x(NP), x(NP), x(NP), x(NP), x(NP), x(NP), x(NP)],
    ),
    // confirm_pass (6.3): state 1, held == 0, flat, b >= 1.08 S, Active; works while paused (6.1)
    same(
        Ins::Confirm,
        "confirm_pass",
        "evaluator",
        [
            x(NP),
            ok(0, 0, 2),
            x(NP),
            any(&[AH, NP]),
            x(AH),
            any(&[AH, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
        ],
    ),
    // reject_pass (6.3): state 1 only; works while paused; cannot touch money
    same(
        Ins::Reject,
        "reject_pass",
        "evaluator",
        [x(NP), ok(0, 0, 0), x(NP), x(NP), ok(0, 1, 0), x(NP), x(NP), x(NP), x(NP), x(NP), x(NP), x(NP)],
    ),
    // veto_pass (6.1, 10.7): both admins, pass_pending 1 or 2 (the marker 3 is not a pending pass); a held account can be vetoed
    same(
        Ins::Veto,
        "veto_pass",
        "both admins",
        [x(NP), ok(0, 0, 0), ok(0, 0, 0), x(NP), ok(0, 1, 0), ok(0, 1, 0), x(NP), x(NP), x(NP), x(NP), x(NP), x(NP)],
    ),
    // finalize_pass (6.4): state 2, held == 0, delay elapsed (fixture: it is), not paused, cap ok (fixture: it is), Active
    Row {
        ins: Ins::Finalize,
        program_ix: "finalize_pass",
        signer: "anyone (payer)",
        open: [
            x(NP),
            x(NP),
            ok(0, 0, 0),
            any(&[AH, NP]),
            any(&[AH, NP]),
            x(AH),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
            any(&[NA, NP]),
        ],
        paused: [
            any(&[PAU, NP]),
            any(&[PAU, NP]),
            x(PAU),
            any(&[PAU, AH, NP]),
            any(&[PAU, AH, NP]),
            any(&[PAU, AH]),
            any(&[PAU, NA, NP]),
            any(&[PAU, NA, NP]),
            any(&[PAU, NA, NP]),
            any(&[PAU, NA, NP]),
            any(&[PAU, NA, NP]),
            any(&[PAU, NA, NP]),
        ],
    },
    // hold_account (6.3): Active, not already held, non-zero evidence; "works in every pass state"; the pause does not stop it
    same(
        Ins::Hold,
        "hold_account",
        "evaluator",
        [ok(0, 1, 0), ok(0, 1, 1), ok(0, 1, 2), x(AL), x(AL), x(AL), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // release_hold (6.1): both admins, held == 0 is NotHeld; a pending pass is untouched and its delay clock runs on (10.3)
    same(
        Ins::ReleaseHold,
        "release_hold",
        "both admins",
        [x(NH), x(NH), x(NH), ok(0, 0, 0), ok(0, 0, 1), ok(0, 0, 2), x(NH), x(NH), x(NH), x(NH), x(NH), x(NH)],
    ),
    // fail_for_violation (6.1, 2.9): both admins, Active AND held; clears any pending pass (also state 2) and leaves the marker 3;
    // works while paused
    same(
        Ins::Violation,
        "fail_for_violation",
        "both admins",
        [
            x(NH),
            x(NH),
            x(NH),
            ok(1, 0, 3),
            ok(1, 0, 3),
            ok(1, 0, 3),
            any(&[NA, NH]),
            any(&[NA, NH]),
            any(&[NA, NH]),
            any(&[NA, NH]),
            any(&[NA, NH]),
            any(&[NA, NH]),
        ],
    ),
    // touch_activity (6.3, 10.1): a live (Active) account only; held, pending and pause do not matter
    same(
        Ins::Touch,
        "touch_activity",
        "evaluator",
        [ok(0, 0, 0), ok(0, 0, 1), ok(0, 0, 2), ok(0, 1, 0), ok(0, 1, 1), ok(0, 1, 2), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // release_account (6.2): the trader's own activity push, same states as touch_activity
    same(
        Ins::ReleaseAcct,
        "release_account",
        "trader",
        [ok(0, 0, 0), ok(0, 0, 1), ok(0, 0, 2), ok(0, 1, 0), ok(0, 1, 1), ok(0, 1, 2), x(NA), x(NA), x(NA), x(NA), x(NA), x(NA)],
    ),
    // close_account (6.2): a terminal account 7 days after it became terminal (fixture: 8 days), rent to the trader
    same(
        Ins::Close,
        "close_account",
        "trader",
        [x(NT), x(NT), x(NT), x(NT), x(NT), x(NT), W::Closed, W::Closed, W::Closed, W::Closed, W::Closed, W::Closed],
    ),
    // reset_account (6.2 "checks in order": paused, wallet not banned, previous account Failed, not violation-failed, not reset).
    // FV is both banned and marked, so either of the two errors is allowed there.
    Row {
        ins: Ins::Reset,
        program_ix: "reset_account",
        signer: "trader (+ payer = trader)",
        open: [x(RNA), x(RNA), x(RNA), x(RNA), x(RNA), x(RNA), W::Reset, any(&[WB, VNR]), x(VNR), x(AAR), x(RNA), x(RNA)],
        paused: [
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
            x(PAU),
            any(&[PAU, WB, VNR]),
            any(&[PAU, VNR]),
            any(&[PAU, AAR]),
            any(&[PAU, RNA]),
            any(&[PAU, RNA]),
        ],
    },
];

// ============================================================================================ worlds

struct World {
    w: JEnv,
    /// The clock every instruction of a cell is sent at.
    now: i64,
    payer: Keypair,
}

fn base_env() -> (JEnv, Keypair) {
    let mut w = JEnv::new();
    w.set_limits(172_800, CAP, WINDOW);
    let p = w.new_payer();
    (w, p)
}

/// `b` = 1.08 S, flat, tight off, q = 0: only the status can stop a pass instruction (written by `edit_account`).
fn pass_ready_bytes(w: &mut JEnv) {
    w.edit_account(|a| {
        a.b = TARGET;
        a.flat = 1;
        a.tight_on = 0;
        a.q_tally = 0;
    });
}

fn terminal_now(w: &JEnv) -> i64 {
    w.account().terminal_at() + WEEK + 5_000
}

/// Builds the world of one column of the table. Real instructions everywhere except where the module doc says so.
fn world(st: St, paused: bool) -> World {
    let (mut w, payer) = base_env();
    let t0 = w.t0;
    let now;
    match st {
        St::A0 | St::A1 | St::A2 | St::H0 | St::H1 | St::H2 => {
            now = t0 + 100_000;
            w.ready(3);
            match st {
                St::A1 | St::H1 => w.set_pending(c::PASS_REQUESTED, now - 4_000, 0),
                St::A2 | St::H2 => w.set_pending(c::PASS_CONFIRMED, now - DELAY - 200, now - DELAY - 100),
                _ => {}
            }
            if matches!(st, St::H0 | St::H1 | St::H2) {
                w.ok(|w| w.hold(now, hash(1)));
            }
        }
        St::F | St::FX => {
            let t = t0 + 1_000;
            w.fail_in(3, t);
            if st == St::FX {
                let prev = w.acct;
                w.reset_ok(t + 100);
                w.acct = prev;
            }
            pass_ready_bytes(&mut w);
            now = terminal_now(&w);
        }
        St::FV | St::FVU => {
            let t = t0 + 1_000;
            w.ok(|w| w.hold(t, hash(1)));
            w.ok(|w| w.violation(t + 1, hash(2)));
            if st == St::FVU {
                let admins = Admins::both(&w.v.e);
                let ix = unban_wallet_ix(admins, w.trader.pubkey());
                assert_ok(w.v.e.send_admins(ix, admins));
            }
            pass_ready_bytes(&mut w);
            now = terminal_now(&w);
        }
        St::AB => {
            let t = t0 + WEEK + 1;
            w.ok(|w| w.release_acct(t));
            pass_ready_bytes(&mut w);
            now = terminal_now(&w);
        }
        St::GR => {
            let t = w.advance_to(5, t0 + 1_000, &payer, DELAY);
            w.ready(5);
            w.ok(|w| w.request(t));
            w.ok(|w| w.confirm(t + 1));
            w.ok(|w| w.finalize(t + 1 + DELAY, &payer));
            pass_ready_bytes(&mut w);
            now = terminal_now(&w);
        }
    }
    let expect: (u8, u8, u8) = match st {
        St::A0 => (0, 0, 0),
        St::A1 => (0, 0, 1),
        St::A2 => (0, 0, 2),
        St::H0 => (0, 1, 0),
        St::H1 => (0, 1, 1),
        St::H2 => (0, 1, 2),
        St::F | St::FX => (1, 0, 0),
        St::FV | St::FVU => (1, 0, 3),
        St::AB => (2, 0, 0),
        St::GR => (3, 0, 0),
    };
    assert_eq!(tri(&w.account()), expect, "world {st:?} was built on the wrong coordinate");
    if paused {
        w.pause_sector(true);
    }
    world_invariants(&w, &format!("{st:?} built"));
    World { w, now, payer }
}

// ============================================================================================ executing one instruction

struct Aux {
    head: Option<([u8; 32], u64)>,
}

/// Some instructions need a fixture tweak that depends on the instruction, not on the state.
fn prep(wd: &mut World, ins: Ins) {
    if ins == Ins::ReportFail {
        // the static floor 0.92 S: the one fail an account can honestly be reported at without a quick tally
        wd.w.edit_account(|a| a.b = FLOOR);
    }
}

/// Sends `ins` with the signer class the table names, returns the result and the index of the program instruction.
fn exec(wd: &mut World, ins: Ins) -> (TransactionResult, u8, Aux) {
    let now = wd.now;
    let payer = &wd.payer;
    let w = &mut wd.w;
    match ins {
        Ins::PostMeta => {
            let b = w.batch(vec![meta(now, true)], vec![]);
            let head = w.expected_head(&b);
            (w.post(now, &b, &[]), 1, Aux { head: Some(head) })
        }
        Ins::PostFill => {
            let b = w.batch(vec![fill_long(now, 0), meta(now, false)], vec![p_eur(now)]);
            let head = w.expected_head(&b);
            (w.post(now, &b, &[MARKET_EUR]), 1, Aux { head: Some(head) })
        }
        Ins::ReportFail => {
            let b = w.fail_batch(now, vec![], FailReason::StaticFloor, FLOOR, true);
            let head = w.expected_head(&b);
            (w.report_fail(now, &b, 0, &[MARKET_EUR]), 0, Aux { head: Some(head) })
        }
        Ins::Request => (w.request(now), 0, Aux { head: None }),
        Ins::Cancel => (w.cancel(now), 0, Aux { head: None }),
        Ins::Confirm => (w.confirm(now), 0, Aux { head: None }),
        Ins::Reject => (w.reject(now, c::REJECT_NOT_FLAT), 0, Aux { head: None }),
        Ins::Veto => (w.veto(now), 0, Aux { head: None }),
        Ins::Finalize => (w.finalize(now, payer), 0, Aux { head: None }),
        Ins::Hold => (w.hold(now, hash(7)), 0, Aux { head: None }),
        Ins::ReleaseHold => (w.release_hold(now), 0, Aux { head: None }),
        Ins::Violation => (w.violation(now, hash(8)), 0, Aux { head: None }),
        Ins::Touch => (w.touch(now), 0, Aux { head: None }),
        Ins::ReleaseAcct => (w.release_acct(now), 0, Aux { head: None }),
        Ins::Close => (w.close(now), 0, Aux { head: None }),
        Ins::Reset => (w.reset(now), 0, Aux { head: None }),
    }
}

/// The account after an ALLOWED instruction, written by hand from the DESIGN (field by field), for the full-struct comparison.
fn expected_after(ins: Ins, pre: &TradingAccount, now: i64, aux: &Aux, post: &TradingAccount) -> TradingAccount {
    let mut e = clone_acct(pre);
    match ins {
        Ins::PostMeta => {
            let (h, s) = aux.head.unwrap();
            e.seq = s;
            e.journal_hash = h;
            e.last_batch_time = now;
            e.flat = 1;
        }
        Ins::PostFill => {
            let (h, s) = aux.head.unwrap();
            e.seq = s;
            e.journal_hash = h;
            e.last_batch_time = now;
            e.flat = 0;
        }
        Ins::ReportFail => {
            let (h, s) = aux.head.unwrap();
            e.seq = s;
            e.journal_hash = h;
            e.last_batch_time = now;
            e.flat = 1;
            e.status = c::STATUS_FAILED;
            e.held = 0;
            e.pass_pending = 0;
            e.pass_requested_at = 0;
            e.pass_proposed_at = now; // terminal_at
        }
        Ins::Request => {
            e.pass_pending = c::PASS_REQUESTED;
            e.pass_requested_at = now;
            // the vault may answer Recorded (stored) or Throttled (not stored)
            assert!(post.last_activity_pushed == pre.last_activity_pushed || post.last_activity_pushed == now);
            e.last_activity_pushed = post.last_activity_pushed;
        }
        Ins::Cancel | Ins::Reject => e.pass_pending = 0,
        Ins::Confirm => {
            e.pass_pending = c::PASS_CONFIRMED;
            e.pass_proposed_at = now;
        }
        Ins::Veto => {
            e.pass_pending = 0;
            e.vetoes = pre.vetoes + 1;
        }
        Ins::Finalize => {
            // phase 3 -> 4: engine::reset_phase_state(S), lock kept, nothing pending
            e.phase = pre.phase + 1;
            e.b = i64::try_from(pre.size_s).unwrap();
            e.q_tally = 0;
            e.latch = 0;
            e.tight_on = 0;
            e.r = 0;
            e.g_peak = 0;
            e.l_level = 0;
            e.flat = 1;
            e.pass_pending = 0;
            e.pass_requested_at = 0;
            e.pass_proposed_at = 0;
            e.next_request_id = pre.next_request_id + 1;
        }
        Ins::Hold => e.held = 1,
        Ins::ReleaseHold => e.held = 0,
        Ins::Violation => {
            e.status = c::STATUS_FAILED;
            e.held = 0;
            e.pass_pending = c::PASS_VIOLATION;
            e.pass_requested_at = 0;
            e.pass_proposed_at = now;
        }
        Ins::Touch | Ins::ReleaseAcct => {
            assert!(post.last_activity_pushed == pre.last_activity_pushed || post.last_activity_pushed == now);
            e.last_activity_pushed = post.last_activity_pushed;
        }
        Ins::Reset => e.pass_requested_at = now.max(1),
        Ins::Close => unreachable!("a closed account has no after-state"),
    }
    e
}

/// What one cell did, for the counters printed at the end of a test.
#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    cells: usize,
    allowed: usize,
    refused: usize,
}
impl Tally {
    fn add(&mut self, o: Tally) {
        self.cells += o.cells;
        self.allowed += o.allowed;
        self.refused += o.refused;
    }
}

/// Runs one table cell on a COPY of the world (LiteSVM is `Clone`; the world is restored afterwards) and asserts the table.
fn run_cell(wd: &mut World, ins: Ins, want: W, ctx: &str) -> Tally {
    let snap = wd.w.v.e.svm.clone();
    prep(wd, ins);
    let pre = wd.w.account();
    let pre_wallet = wd.w.wallet_profile();
    let before = wd.w.pass_digest();
    let (r, idx, aux) = exec(wd, ins);
    let mut t = Tally { cells: 1, ..Tally::default() };
    match want {
        W::Ok(s, h, p) => {
            if let Err(f) = &r {
                panic!("{ctx}: the table says ALLOWED, the program refused: {:?}\n{}", f.err, f.meta.logs.join("\n"));
            }
            let post = wd.w.account();
            assert_eq!(tri(&post), (s, h, p), "{ctx}: wrong (status, held, pass_pending) after an allowed call");
            let exp = expected_after(ins, &pre, wd.now, &aux, &post);
            assert_eq!(bytes_of(&exp), bytes_of(&post), "{ctx}: the account changed in more (or fewer) fields than the table says");
            check_wallet_effect(ins, &pre_wallet, &wd.w.wallet_profile(), ctx);
            t.allowed = 1;
        }
        W::Closed => {
            if let Err(f) = &r {
                panic!("{ctx}: the table says ALLOWED, the program refused: {:?}\n{}", f.err, f.meta.logs.join("\n"));
            }
            let gone = match wd.w.v.e.svm.get_account(&wd.w.acct) {
                None => true,
                Some(a) => a.lamports == 0 || a.data.iter().all(|b| *b == 0),
            };
            assert!(gone, "{ctx}: close_account succeeded but the account is still there");
            t.allowed = 1;
        }
        W::Reset => {
            if let Err(f) = &r {
                panic!("{ctx}: the table says ALLOWED, the program refused: {:?}\n{}", f.err, f.meta.logs.join("\n"));
            }
            let post = wd.w.account();
            let exp = expected_after(ins, &pre, wd.now, &aux, &post);
            assert_eq!(bytes_of(&exp), bytes_of(&post), "{ctx}: the failed account changed in more than `reset_at`");
            assert_eq!(tri(&post), (1, 0, 0));
            let new = wd.w.v.account(&acct_pda(&pre.trader, pre_wallet.next_challenge_id));
            assert_eq!((tri(&new), new.phase, new.b), ((0, 0, 0), pre.phase, i64::try_from(pre.size_s).unwrap()), "{ctx}: the new account");
            t.allowed = 1;
        }
        W::X(e) => {
            match code_at(&r, idx) {
                Ok(code) => assert_eq!(code, u32::from(e), "{ctx}: wrong error, the table says {e:?}"),
                Err(m) => panic!("{ctx}: the table says {e:?}, got: {m}"),
            }
            assert_eq!(wd.w.pass_digest(), before, "{ctx}: a refused call changed the world ({e:?})");
            assert_eq!(bytes_of(&wd.w.account()), bytes_of(&pre), "{ctx}: a refused call changed the account");
            t.refused = 1;
        }
        W::Any(set) => {
            match code_at(&r, idx) {
                Ok(code) => assert!(set.iter().any(|e| u32::from(*e) == code), "{ctx}: error {code} is not one of {set:?}"),
                Err(m) => panic!("{ctx}: the table says one of {set:?}, got: {m}"),
            }
            assert_eq!(wd.w.pass_digest(), before, "{ctx}: a refused call changed the world");
            assert_eq!(bytes_of(&wd.w.account()), bytes_of(&pre), "{ctx}: a refused call changed the account");
            t.refused = 1;
        }
    }
    if !matches!(want, W::Closed) {
        world_invariants(&wd.w, ctx);
    }
    wd.w.v.e.svm = snap;
    t
}

fn check_wallet_effect(ins: Ins, pre: &setl8_lev_trading::state::WalletProfile, post: &setl8_lev_trading::state::WalletProfile, ctx: &str) {
    let mut e = pre.clone();
    match ins {
        Ins::Hold => e.held_count += 1,
        Ins::ReleaseHold => e.held_count -= 1,
        Ins::ReportFail => {
            // the hold (if any) is released and the slot is freed
            e.active_count -= 1;
            e.held_count = 0;
        }
        Ins::Violation => {
            e.active_count -= 1;
            e.held_count = 0;
            e.banned = 1;
        }
        _ => {}
    }
    assert_eq!(
        (post.trader, post.next_challenge_id, post.active_count, post.bump, post.banned, post.held_count),
        (e.trader, e.next_challenge_id, e.active_count, e.bump, e.banned, e.held_count),
        "{ctx}: wallet profile"
    );
}

fn grid_column(st: St, paused: bool) {
    let mut wd = world(st, paused);
    let col = STATES.iter().position(|s| *s == st).unwrap();
    let mut total = Tally::default();
    for row in TABLE.iter() {
        let want = if paused { row.paused[col] } else { row.open[col] };
        let ctx = format!("[{:?} {} paused={paused}] {:?} ({})", st, if paused { "P" } else { "-" }, row.ins, row.program_ix);
        total.add(run_cell(&mut wd, row.ins, want, &ctx));
    }
    assert_eq!(total.cells, 16);
    println!("grid column {st:?} paused={paused}: {total:?}");
}

macro_rules! grid {
    ($($name:ident: $st:expr, $paused:expr;)*) => {
        $(
            #[test]
            fn $name() {
                grid_column($st, $paused);
            }
        )*
    };
}

grid! {
    grid_a0_open: St::A0, false;
    grid_a1_open: St::A1, false;
    grid_a2_open: St::A2, false;
    grid_h0_open: St::H0, false;
    grid_h1_open: St::H1, false;
    grid_h2_open: St::H2, false;
    grid_failed_open: St::F, false;
    grid_failed_violation_banned_open: St::FV, false;
    grid_failed_violation_unbanned_open: St::FVU, false;
    grid_failed_already_reset_open: St::FX, false;
    grid_abandoned_open: St::AB, false;
    grid_graduated_open: St::GR, false;
    grid_a0_paused: St::A0, true;
    grid_a1_paused: St::A1, true;
    grid_a2_paused: St::A2, true;
    grid_h0_paused: St::H0, true;
    grid_h1_paused: St::H1, true;
    grid_h2_paused: St::H2, true;
    grid_failed_paused: St::F, true;
    grid_failed_violation_banned_paused: St::FV, true;
    grid_failed_violation_unbanned_paused: St::FVU, true;
    grid_failed_already_reset_paused: St::FX, true;
    grid_abandoned_paused: St::AB, true;
    grid_graduated_paused: St::GR, true;
}

/// The table has a row for every instruction that reads a TradingAccount, and together with the account-free instructions of
/// `no_account_*` it covers every instruction of lib.rs: a new instruction cannot be added without a row.
#[test]
fn table_covers_every_instruction_of_the_program() {
    let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../programs/setl8-lev-trading/src/lib.rs")).unwrap();
    let mut program: Vec<String> = lib
        .lines()
        .filter_map(|l| l.strip_prefix("    pub fn "))
        .map(|l| l.split(['(', '<']).next().unwrap().to_string())
        .collect();
    program.sort();
    // the instructions that never read a TradingAccount: covered by `no_account_*` (paused sector x admin / purchase)
    let account_free = [
        "init_config",
        "set_evaluator",
        "set_limits",
        "set_paused",
        "create_ruleset",
        "create_market",
        "update_market",
        "ban_wallet",
        "unban_wallet",
        "open_account",
    ];
    let mut covered: Vec<String> = TABLE.iter().map(|r| r.program_ix.to_string()).chain(account_free.iter().map(|s| s.to_string())).collect();
    covered.sort();
    covered.dedup();
    assert_eq!(program, covered, "an instruction of lib.rs has no row in the state-machine table (or a row names a missing one)");
    assert_eq!(program.len(), 25);
    // 16 rows = 15 account instructions (post_journal twice: Meta-only and Fill)
    assert_eq!(TABLE.len(), 16);
    // every row has a signer class written down
    assert!(TABLE.iter().all(|r| !r.signer.is_empty()));
}

/// The numbers quoted in handover/audit/05-state-machine.md, recomputed from the table itself.
#[test]
fn table_cell_counts() {
    let (mut cells, mut allowed, mut exact, mut anyof) = (0, 0, 0, 0);
    for row in TABLE.iter() {
        for w in row.open.iter().chain(row.paused.iter()) {
            cells += 1;
            match w {
                W::Ok(..) | W::Closed | W::Reset => allowed += 1,
                W::X(_) => exact += 1,
                W::Any(_) => anyof += 1,
            }
        }
    }
    println!("table cells {cells}: allowed {allowed}, refused with one named error {exact}, refused with a DESIGN-allowed set {anyof}");
    assert_eq!(cells, 16 * 12 * 2);
    assert_eq!(cells, allowed + exact + anyof);
    // 32 grid coordinates x 2 (paused) x 16 rows = 1,024 grid cells; 10 coordinates exist => 320 grid cells; FVU and FX are the
    // two extra sub-states of the Failed coordinates (wallet ban, reset_at) => 64 more table cells
    let coords = (0..4u8).flat_map(|s| (0..2u8).flat_map(move |h| (0..4u8).map(move |p| (s, h, p)))).count();
    let existing = (0..4u8).flat_map(|s| (0..2u8).flat_map(move |h| (0..4u8).map(move |p| (s, h, p)))).filter(|(s, h, p)| reachable(*s, *h, *p)).count();
    assert_eq!((coords, existing), (32, 10));
    assert_eq!(coords * 2 * 16, 1_024);
    assert_eq!(existing * 2 * 16 + 2 * 2 * 16, cells);
    assert_eq!((coords - existing) * 2 * 16, 704);
}

// ============================================================================================ part B: the coordinates that cannot exist

/// Runs the whole instruction list on a world whose account is edited onto `(status, held, pending)`. No expected value is claimed.
/// The program must answer with a `Custom` error code or succeed, and a refusal must leave the world byte-identical.
fn sweep(status: u8, held: u8, pending: u8, paused: bool) -> (usize, usize, Vec<String>) {
    let (mut w, payer) = base_env();
    let now = w.t0 + 100_000;
    w.ready(3);
    let old = now - WEEK - 5_000;
    w.edit_account(|a| {
        a.status = status;
        a.held = held;
        a.pass_pending = pending;
        match (status, pending) {
            (0, 1) => a.pass_requested_at = now - 4_000,
            (0, 2) => {
                a.pass_requested_at = now - DELAY - 200;
                a.pass_proposed_at = now - DELAY - 100;
            }
            (0, _) => {}
            (_, 1) => a.pass_requested_at = now - 4_000,
            _ => a.pass_proposed_at = old,
        }
    });
    if held == 1 {
        let trader = w.trader.pubkey();
        w.v.edit_wallet(&trader, |p| p.held_count = 1);
    }
    if status != c::STATUS_ACTIVE {
        let trader = w.trader.pubkey();
        w.v.edit_wallet(&trader, |p| p.active_count = 0);
    }
    if paused {
        w.pause_sector(true);
    }
    let mut wd = World { w, now, payer };
    let (mut ok_n, mut err_n) = (0, 0);
    let mut accepted = Vec::new();
    for row in TABLE.iter() {
        let ctx = format!("[sweep status={status} held={held} pending={pending} paused={paused}] {:?}", row.ins);
        let snap = wd.w.v.e.svm.clone();
        prep(&mut wd, row.ins);
        let before = wd.w.pass_digest();
        let pre = bytes_of(&wd.w.account());
        let (r, idx, _) = exec(&mut wd, row.ins);
        match &r {
            Ok(_) => {
                ok_n += 1;
                accepted.push(format!("{:?}", row.ins));
            }
            Err(f) => {
                err_n += 1;
                match &f.err {
                    TransactionError::InstructionError(i, InstructionError::Custom(code)) if *i == idx => {
                        assert!((6000..6200).contains(code) || (2000..4000).contains(code) || *code < 100, "{ctx}: unexpected code {code}");
                    }
                    other => panic!("{ctx}: a non-Custom failure (a crash or a runtime error): {other:?}\n{}", f.meta.logs.join("\n")),
                }
                assert_eq!(wd.w.pass_digest(), before, "{ctx}: a refused call changed the world");
                assert_eq!(bytes_of(&wd.w.account()), pre, "{ctx}: a refused call changed the account");
            }
        }
        wd.w.v.e.svm = snap;
    }
    (ok_n, err_n, accepted)
}

fn sweep_status(status: u8, paused: bool) {
    let (mut cells, mut ok_n, mut err_n) = (0, 0, 0);
    let mut accepted = Vec::new();
    for held in 0..2u8 {
        for pending in 0..4u8 {
            if reachable(status, held, pending) {
                continue;
            }
            let (o, e, acc) = sweep(status, held, pending, paused);
            cells += o + e;
            ok_n += o;
            err_n += e;
            if !acc.is_empty() {
                accepted.push(format!("(held {held}, pending {pending}): {}", acc.join(",")));
            }
        }
    }
    println!("sweep status={status} paused={paused}: {cells} cells on non-existing coordinates, {ok_n} accepted, {err_n} refused; accepted: {accepted:?}");
    // 22 non-existing coordinates over the four statuses: Active 2, Failed 6, Abandoned 7, Graduated 7
    let want = match status {
        0 => 2,
        1 => 6,
        _ => 7,
    };
    assert_eq!(cells, want * 16);
}

#[test]
fn sweep_active_open() {
    sweep_status(0, false);
}
#[test]
fn sweep_active_paused() {
    sweep_status(0, true);
}
#[test]
fn sweep_failed_open() {
    sweep_status(1, false);
}
#[test]
fn sweep_failed_paused() {
    sweep_status(1, true);
}
#[test]
fn sweep_abandoned_open() {
    sweep_status(2, false);
}
#[test]
fn sweep_abandoned_paused() {
    sweep_status(2, true);
}
#[test]
fn sweep_graduated_open() {
    sweep_status(3, false);
}
#[test]
fn sweep_graduated_paused() {
    sweep_status(3, true);
}

// @@PART_C_AND_D@@
