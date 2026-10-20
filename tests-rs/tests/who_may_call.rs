//! M12 item 2: the WHO-MAY-CALL-WHAT matrix. Every instruction of the program x every signer class, against DESIGN 6 and 14.
//!
//! Method.
//! * The instruction list is read from `programs/setl8-lev-trading/src/lib.rs` (every `pub fn` inside the `#[program]` block) and
//!   compared with the rows of `rows()` below. A new instruction without a row, or a row without an instruction, fails
//!   `rows_and_program_instructions_are_the_same_set`. The parser itself is tested on edited copies of the source.
//! * The expected result of EVERY cell is written by hand in `rows()`, from DESIGN 6.1 to 6.4, 10.x and 14. Nothing in the table is
//!   derived from the program. An allowed cell must succeed (and leave the state the instruction should leave). A refused cell must
//!   fail with exactly the stated error (a `Setl8Error` code or the Anchor framework code 3010 `AccountNotSigner`) and leave every
//!   account byte-identical: config, payout tally, wallet profiles, markets, rule sets, trading accounts, vault registry, vault state,
//!   vault trader states, payout claims, token accounts and the lamports of every key involved (the fee payer of the transaction is
//!   the only account left out of the comparison, because it pays the fee).
//! * Signer classes (columns): the trader (owner of the account), the evaluator (`config.evaluator`), admin SL8 alone, admin Rov
//!   alone, both admins, a random funded key, the vault authority PDA (`sector_authority`) in the signer slot as a plain non-signer, and
//!   no signer at all (the right key is in the slot but is not flagged as a signer).
//!   * Instructions with the two admin slots: the class fills BOTH slots (trader, evaluator, random: the same key twice, signing;
//!     SL8 alone: SL8 signs, Rov is present but does not sign; both admins: each in its own slot, both sign; PDA: the PDA twice, not signing).
//!   * Instructions with ONE signer slot: the class fills it. "Both admins" there means: SL8 in the slot, and Rov as the fee payer
//!     of the same transaction (both admin signatures are on the transaction, the instruction can see only one).
//!   * `open_account` / `reset_account` have a trader slot and a payer slot: the class is the buyer in both (the buyer owns a funded token
//!     account), "both admins" puts SL8 in the trader slot and Rov in the payer slot.
//! * Extra classes (not in the 208-cell matrix, counted separately): admin slots swapped, the same admin twice, the OLD evaluator after a rotation
//!   and the NEW evaluator after it.
//!
//! Every table cell of a row runs in a fresh world, so a cell cannot lean on what an earlier cell left behind.
//! `WHO_MATRIX_PRINT=1 cargo test --offline --test who_may_call -- --nocapture print_the_matrix` prints the matrix as markdown.
mod common;

use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use anchor_lang::solana_program::instruction::{error::InstructionError, AccountMeta, Instruction};
use anchor_lang::solana_program::pubkey::Pubkey;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::journal::*;
use common::lifecycle::*;
use common::pass::*;
use common::vault::*;
use common::{
    assert_ok, ban_wallet_ix, config_pda, create_market_args, create_market_ix, create_ruleset_ix, default_args, dup, init_ix_with,
    launch_ruleset_args, market_params, market_pda, ruleset_pda, set_evaluator_ix, set_limits_ix, set_paused_ix, tally_pda,
    unban_wallet_ix, update_market_ix, wallet_pda, Admins, Env, PROGRAM_ID,
};
use litesvm::types::TransactionResult;
use setl8_lev_trading::{accounts, constants as c, errors::Setl8Error as E, instruction, instructions::UpdateMarketArgs};
use sha2::{Digest, Sha256};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

// ====================================================================================================== classes and expectations

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Trader,
    Evaluator,
    Sl8Alone,
    RovAlone,
    BothAdmins,
    Random,
    AuthorityPda,
    NoSigner,
    // extra classes (not in the matrix proper)
    BothSwapped,
    Sl8Twice,
    RovTwice,
    OldEvaluator,
    NewEvaluator,
}

/// The eight columns of the matrix, in the order of every `expect` array.
const CLASSES: [Class; 8] = [
    Class::Trader,
    Class::Evaluator,
    Class::Sl8Alone,
    Class::RovAlone,
    Class::BothAdmins,
    Class::Random,
    Class::AuthorityPda,
    Class::NoSigner,
];

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Trader => "trader",
        Class::Evaluator => "evaluator",
        Class::Sl8Alone => "SL8 alone",
        Class::RovAlone => "Rov alone",
        Class::BothAdmins => "both admins",
        Class::Random => "random key",
        Class::AuthorityPda => "authority PDA (non-signer)",
        Class::NoSigner => "no signer",
        Class::BothSwapped => "admin slots swapped",
        Class::Sl8Twice => "SL8 in both slots",
        Class::RovTwice => "Rov in both slots",
        Class::OldEvaluator => "old evaluator (rotated out)",
        Class::NewEvaluator => "new evaluator (rotated in)",
    }
}

/// What a cell must do. `Prog` is a program error code (DESIGN 12), `Anchor` an Anchor framework code.
#[derive(Clone, Copy, Debug)]
enum Exp {
    Ok,
    Prog(E),
    Anchor(u32),
}

/// `Setl8Error` has no `PartialEq`: two expectations are equal when they name the same code.
impl PartialEq for Exp {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
    }
}

impl Exp {
    fn code(self) -> Option<u32> {
        match self {
            Exp::Ok => None,
            Exp::Prog(p) => Some(u32::from(p)),
            Exp::Anchor(n) => Some(n),
        }
    }
}

/// Anchor 3010: a typed `Signer` account did not sign.
const NOT_SIGNER: u32 = anchor_lang::error::ErrorCode::AccountNotSigner as u32;

const OK: Exp = Exp::Ok;
/// `Unauthorized` 6000.
const U: Exp = Exp::Prog(E::Unauthorized);
/// `AccountNotSigner` 3010.
const NS: Exp = Exp::Anchor(NOT_SIGNER);

fn exp_text(e: Exp) -> String {
    match e {
        Exp::Ok => "ALLOWED".to_string(),
        Exp::Prog(p) => format!("refused {:?} {}", p, u32::from(p)),
        Exp::Anchor(NOT_SIGNER) => "refused NotSigner 3010".to_string(),
        Exp::Anchor(n) => format!("refused anchor {n}"),
    }
}

/// One row of the matrix: an instruction (or a variant of it), the DESIGN rule, and the eight hand-written cells.
struct Row {
    ix: &'static str,
    variant: &'static str,
    design: &'static str,
    expect: [Exp; 8],
    /// Extra classes with their hand-written result (not counted in the 208 cells).
    extra: &'static [(Class, Exp)],
    run: fn(Class) -> Cell,
}

// the extra-class expectations of the three admin shapes
const X_BOTH_ADMINS: &[(Class, Exp)] = &[(Class::BothSwapped, U), (Class::Sl8Twice, U), (Class::RovTwice, U)];
const X_EITHER_ADMIN: &[(Class, Exp)] = &[(Class::BothSwapped, U), (Class::Sl8Twice, OK), (Class::RovTwice, OK)];
const X_EVALUATOR: &[(Class, Exp)] = &[(Class::OldEvaluator, U), (Class::NewEvaluator, OK)];

/// The matrix. Column order: trader, evaluator, SL8 alone, Rov alone, both admins, random, authority PDA, no signer.
///
/// Reading the cells (all written from the DESIGN, not from the program):
/// * both-admin instructions (6.1, 14): only `OK` for both admins. Everything where an admin slot is a typed `Signer` (`init_config`)
///   refuses an unsigned slot with 3010 before any key is compared; the others compare keys and flags in the handler and answer 6000.
/// * either-admin instructions (6.1 `create_market`, `update_market`, `set_paused(true)`): one admin is enough.
/// * evaluator instructions (6.3, 14): only the evaluator key; a typed `Signer` slot that did not sign is 3010, a wrong signer 6000.
/// * trader instructions (6.2): the owner of the account; a stranger who signs is 6000 (the constraint `trading_account.trader == signer`).
/// * `finalize_pass` (6.4): any payer who signs.
/// * `open_account`: the key that signs both slots buys for itself (the program has no role check; see the fragment, finding WHO-1).
fn rows() -> Vec<Row> {
    vec![
        Row {
            ix: "init_config",
            variant: "",
            design: "6.1: SL8 admin and Rov admin, both compiled-in keys",
            expect: [U, U, NS, NS, OK, U, NS, NS],
            extra: X_BOTH_ADMINS,
            run: r_init_config,
        },
        Row {
            ix: "set_evaluator",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_set_evaluator,
        },
        Row {
            ix: "set_limits",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_set_limits,
        },
        Row {
            ix: "set_paused",
            variant: "pause",
            design: "6.1 / 14: pause = either admin",
            expect: [U, U, OK, OK, OK, U, U, U],
            extra: X_EITHER_ADMIN,
            run: r_pause,
        },
        Row {
            ix: "set_paused",
            variant: "unpause",
            design: "6.1 / 14: unpause = both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_unpause,
        },
        Row {
            ix: "create_ruleset",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_create_ruleset,
        },
        Row {
            ix: "create_market",
            variant: "",
            design: "6.1 / 14: either admin",
            expect: [U, U, OK, OK, OK, U, U, U],
            extra: X_EITHER_ADMIN,
            run: r_create_market,
        },
        Row {
            ix: "update_market",
            variant: "",
            design: "6.1 / 14: either admin (decision 24 exception)",
            expect: [U, U, OK, OK, OK, U, U, U],
            extra: X_EITHER_ADMIN,
            run: r_update_market,
        },
        Row {
            ix: "ban_wallet",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_ban_wallet,
        },
        Row {
            ix: "unban_wallet",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_unban_wallet,
        },
        Row {
            ix: "open_account",
            variant: "",
            design: "6.2: trader [S] and payer [S], payer == trader (a different payer is Unauthorized)",
            expect: [OK, OK, OK, OK, U, OK, NS, NS],
            extra: &[],
            run: r_open_account,
        },
        Row {
            ix: "reset_account",
            variant: "",
            design: "6.2: the trader of the failed account (other classes own a wallet but name the trader's failed account)",
            expect: [OK, U, U, U, U, U, NS, NS],
            extra: &[],
            run: r_reset_account,
        },
        Row {
            ix: "post_journal",
            variant: "",
            design: "6.3 / 14: evaluator only (I-1)",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_post_journal,
        },
        Row {
            ix: "request_advance",
            variant: "",
            design: "6.2: the trader (owner of the account)",
            expect: [OK, U, U, U, U, U, NS, NS],
            extra: &[],
            run: r_request_advance,
        },
        Row {
            ix: "cancel_pass",
            variant: "",
            design: "6.2: the trader (owner of the account)",
            expect: [OK, U, U, U, U, U, NS, NS],
            extra: &[],
            run: r_cancel_pass,
        },
        Row {
            ix: "confirm_pass",
            variant: "",
            design: "6.3 / 14: evaluator only",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_confirm_pass,
        },
        Row {
            ix: "reject_pass",
            variant: "",
            design: "6.3 / 14: evaluator only",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_reject_pass,
        },
        Row {
            ix: "veto_pass",
            variant: "",
            design: "6.1 / 10.7 / 14: both admins in one transaction",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_veto_pass,
        },
        Row {
            ix: "finalize_pass",
            variant: "",
            design: "6.4: anyone who signs as payer (no role)",
            expect: [OK, OK, OK, OK, OK, OK, NS, NS],
            extra: &[],
            run: r_finalize_pass,
        },
        Row {
            ix: "report_fail",
            variant: "",
            design: "6.3 / 14: evaluator only",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_report_fail,
        },
        Row {
            ix: "hold_account",
            variant: "",
            design: "6.3 / 14: evaluator only",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_hold_account,
        },
        Row {
            ix: "release_hold",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_release_hold,
        },
        Row {
            ix: "fail_for_violation",
            variant: "",
            design: "6.1 / 14: both admins",
            expect: [U, U, U, U, OK, U, U, U],
            extra: X_BOTH_ADMINS,
            run: r_fail_for_violation,
        },
        Row {
            ix: "touch_activity",
            variant: "",
            design: "6.3 / 14: evaluator only",
            expect: [U, OK, U, U, U, U, NS, NS],
            extra: X_EVALUATOR,
            run: r_touch_activity,
        },
        Row {
            ix: "release_account",
            variant: "",
            design: "6.2: the trader (owner of the account)",
            expect: [OK, U, U, U, U, U, NS, NS],
            extra: &[],
            run: r_release_account,
        },
        Row {
            ix: "close_account",
            variant: "",
            design: "6.2: the trader (owner of the account)",
            expect: [OK, U, U, U, U, U, NS, NS],
            extra: &[],
            run: r_close_account,
        },
    ]
}

/// Totals the lead pastes into the report. They are asserted against the table, so changing a cell means changing these.
const INSTRUCTIONS: usize = 25; // the prompt said 26; `pub fn` inside the #[program] block counts 25 at HEAD
const ROWS: usize = 26; // `set_paused` has two rows: pause (either admin) and unpause (both)
const CELLS: usize = 208; // 26 rows x 8 classes (25 instructions x 8 = 200, plus the 8 cells of the second set_paused row)
const ALLOWED_CELLS: usize = 41;
const REFUSED_CELLS: usize = 167;
const EXTRA_CELLS: usize = 51;

// ====================================================================================================== the instruction list

/// The names of the `pub fn`s inside the `#[program]` module of `src`.
fn program_instructions(src: &str) -> Vec<String> {
    let start = src.find("#[program]").expect("no #[program] block");
    let open = start + src[start..].find('{').expect("program module has no body");
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut end = bytes.len();
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &src[open..end];
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("pub fn ") {
        let tail = &rest[i + "pub fn ".len()..];
        let name: String = tail.chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect();
        names.push(name);
        rest = tail;
    }
    names
}

/// (instructions without a row, rows without an instruction)
fn diff_against_rows(src: &str) -> (Vec<String>, Vec<String>) {
    let program: BTreeSet<String> = program_instructions(src).into_iter().collect();
    let table: BTreeSet<String> = rows().iter().map(|r| r.ix.to_string()).collect();
    (program.difference(&table).cloned().collect(), table.difference(&program).cloned().collect())
}

const LIB_RS: &str = include_str!("../../programs/setl8-lev-trading/src/lib.rs");

#[test]
fn rows_and_program_instructions_are_the_same_set() {
    let names = program_instructions(LIB_RS);
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(names.len(), unique.len(), "an instruction name appears twice in the #[program] block");
    assert_eq!(names.len(), INSTRUCTIONS, "the program has {} instructions, this test was written for {INSTRUCTIONS}: add the rows and update the constants", names.len());
    let (no_row, no_ix) = diff_against_rows(LIB_RS);
    assert!(no_row.is_empty(), "instructions of the program with NO ROW in the who-may-call matrix: {no_row:?}");
    assert!(no_ix.is_empty(), "rows of the matrix with NO INSTRUCTION in the program: {no_ix:?}");
    // two rows may share an instruction only when the DESIGN gives the two variants different signers (set_paused)
    let rs = rows();
    assert_eq!(rs.len(), ROWS);
    let mut seen = BTreeSet::new();
    for r in &rs {
        assert!(seen.insert((r.ix, r.variant)), "duplicate row {} {}", r.ix, r.variant);
        if r.variant.is_empty() {
            assert_eq!(rs.iter().filter(|o| o.ix == r.ix).count(), 1, "{} has an unnamed row and another row", r.ix);
        }
    }
}

#[test]
fn the_instruction_parser_catches_a_new_instruction_and_a_removed_one() {
    // an instruction added to the program without a row
    let with_new = LIB_RS.replacen(
        "    pub fn close_account(",
        "    pub fn drain_everything(ctx: Context<CloseAccount>) -> Result<()> {\n        Ok(())\n    }\n\n    pub fn close_account(",
        1,
    );
    assert_ne!(with_new, LIB_RS, "the edit did not apply: the parser test is void");
    let (no_row, no_ix) = diff_against_rows(&with_new);
    assert_eq!(no_row, vec!["drain_everything".to_string()]);
    assert!(no_ix.is_empty());
    // an instruction renamed: one missing row, one row without an instruction
    let renamed = LIB_RS.replacen("pub fn veto_pass(", "pub fn veto_pass_v2(", 1);
    assert_ne!(renamed, LIB_RS);
    let (no_row, no_ix) = diff_against_rows(&renamed);
    assert_eq!(no_row, vec!["veto_pass_v2".to_string()]);
    assert_eq!(no_ix, vec!["veto_pass".to_string()]);
    // an instruction removed from the program: a row without an instruction
    let removed = LIB_RS.replacen("pub fn touch_activity(", "fn touch_activity(", 1);
    assert_ne!(removed, LIB_RS);
    let (no_row, no_ix) = diff_against_rows(&removed);
    assert!(no_row.is_empty());
    assert_eq!(no_ix, vec!["touch_activity".to_string()]);
    // helper functions OUTSIDE the #[program] block are not instructions
    let outside = format!("{LIB_RS}\npub fn not_an_instruction() {{}}\n");
    assert_eq!(program_instructions(&outside).len(), INSTRUCTIONS);
}

#[test]
fn the_table_counts_are_the_ones_reported() {
    let rs = rows();
    let cells: usize = rs.len() * CLASSES.len();
    let allowed = rs.iter().flat_map(|r| r.expect.iter()).filter(|e| **e == OK).count();
    let extra: usize = rs.iter().map(|r| r.extra.len()).sum();
    assert_eq!((cells, allowed, cells - allowed, extra), (CELLS, ALLOWED_CELLS, REFUSED_CELLS, EXTRA_CELLS));
    assert_eq!(NOT_SIGNER, 3010);
    // every refused cell is a specific error, never a bare failure
    for r in &rs {
        for e in r.expect.iter().chain(r.extra.iter().map(|(_, e)| e)) {
            assert!(matches!(e, Exp::Ok | Exp::Prog(_) | Exp::Anchor(_)));
        }
    }
}

#[test]
fn the_hand_written_table_obeys_design_14_by_itself() {
    // Independent sanity of the TABLE (not of the program): the properties DESIGN 14 states in words must hold for the cells.
    let rs = rows();
    let col = |c: Class| CLASSES.iter().position(|x| *x == c).unwrap();
    for r in &rs {
        // nobody who cannot sign is ever allowed
        for c in [Class::AuthorityPda, Class::NoSigner] {
            assert_ne!(r.expect[col(c)], OK, "{} {}: a non-signer is allowed", r.ix, r.variant);
        }
        // no instruction is ever allowed for the authority PDA
        // (it can only sign through this program's own invoke_signed)
        assert_ne!(r.expect[col(Class::AuthorityPda)], OK);
    }
    let by = |name: &str| rs.iter().find(|r| r.ix == name).unwrap_or_else(|| panic!("no row {name}"));
    // the evaluator is allowed exactly on its own instructions, plus the open-to-everyone ones
    let evaluator_ok: BTreeSet<&str> = rs.iter().filter(|r| r.expect[col(Class::Evaluator)] == OK).map(|r| r.ix).collect();
    let want: BTreeSet<&str> =
        ["post_journal", "report_fail", "hold_account", "confirm_pass", "reject_pass", "touch_activity", "finalize_pass", "open_account"].into_iter().collect();
    assert_eq!(evaluator_ok, want, "DESIGN 14: the evaluator posts journals, fails, holds, confirms, rejects and refreshes activity (plus anyone-instructions)");
    // the evaluator has no money or config power: not set_evaluator, set_limits, set_paused, the rule set, markets, bans, veto, release, violation
    for n in ["set_evaluator", "set_limits", "set_paused", "create_ruleset", "create_market", "update_market", "ban_wallet", "unban_wallet", "veto_pass", "release_hold", "fail_for_violation", "request_advance"] {
        assert_ne!(by(n).expect[col(Class::Evaluator)], OK, "{n}");
    }
    // a single admin is never enough, except for markets and pausing (6.1)
    let single_admin_ok: BTreeSet<&str> = rs
        .iter()
        .filter(|r| r.expect[col(Class::Sl8Alone)] == OK || r.expect[col(Class::RovAlone)] == OK)
        .map(|r| r.ix)
        .collect();
    let want: BTreeSet<&str> = ["create_market", "update_market", "set_paused", "finalize_pass", "open_account"].into_iter().collect();
    assert_eq!(single_admin_ok, want);
    // SL8 alone and Rov alone always get the same answer (the two halves are symmetric)
    for r in &rs {
        if r.ix != "init_config" {
            assert_eq!(r.expect[col(Class::Sl8Alone)], r.expect[col(Class::RovAlone)], "{} {}", r.ix, r.variant);
        }
    }
    // the trader is allowed exactly on the trader instructions (+ the anyone ones)
    let trader_ok: BTreeSet<&str> = rs.iter().filter(|r| r.expect[col(Class::Trader)] == OK).map(|r| r.ix).collect();
    let want: BTreeSet<&str> =
        ["open_account", "reset_account", "request_advance", "cancel_pass", "release_account", "close_account", "finalize_pass"].into_iter().collect();
    assert_eq!(trader_ok, want);
    // both admins together may do everything the admins may do, and veto
    for n in ["init_config", "set_evaluator", "set_limits", "create_ruleset", "create_market", "update_market", "ban_wallet", "unban_wallet", "veto_pass", "release_hold", "fail_for_violation"] {
        assert_eq!(by(n).expect[col(Class::BothAdmins)], OK, "{n}");
    }
    // nothing the admins do alone or together moves or approves money: finalize_pass is the only payout path and needs no admin
    // (the request/confirm path is the trader's and the evaluator's, never an admin's)
    for n in ["request_advance", "confirm_pass", "cancel_pass", "reject_pass"] {
        assert_ne!(by(n).expect[col(Class::BothAdmins)], OK, "{n}");
    }
}

// ====================================================================================================== world, keys, patching, sending

struct Keys {
    trader: Keypair,
    evaluator: Keypair,
    sl8: Keypair,
    rov: Keypair,
    random: Keypair,
    new_eval: Keypair,
    authority: Pubkey,
}

impl Keys {
    fn all(&self) -> [&Keypair; 6] {
        [&self.trader, &self.evaluator, &self.sl8, &self.rov, &self.random, &self.new_eval]
    }
}

fn fresh() -> (JEnv, Keys) {
    let mut w = JEnv::new();
    let random = w.new_payer();
    let new_eval = Keypair::new();
    w.v.e.svm.airdrop(&new_eval.pubkey(), 1_000_000_000).unwrap();
    let k = Keys {
        trader: dup(&w.trader),
        evaluator: dup(&w.v.evaluator),
        sl8: dup(&w.v.e.sl8),
        rov: dup(&w.v.e.rov),
        random,
        new_eval,
        authority: sector_authority_pda(),
    };
    (w, k)
}

/// (key, signs) of the single signer slot for `class`. `legit` is the key that belongs there (used when nobody signs).
fn one_slot(class: Class, k: &Keys, legit: Pubkey) -> (Pubkey, bool) {
    match class {
        Class::Trader => (k.trader.pubkey(), true),
        Class::Evaluator | Class::OldEvaluator => (k.evaluator.pubkey(), true),
        Class::NewEvaluator => (k.new_eval.pubkey(), true),
        Class::Sl8Alone | Class::BothAdmins => (k.sl8.pubkey(), true),
        Class::RovAlone => (k.rov.pubkey(), true),
        Class::Random => (k.random.pubkey(), true),
        Class::AuthorityPda => (k.authority, false),
        Class::NoSigner => (legit, false),
        other => panic!("{other:?} has no meaning for a one-slot instruction"),
    }
}

/// (key, signs) of the admin slots a and b for `class`.
fn admin_slots(class: Class, k: &Keys) -> [(Pubkey, bool); 2] {
    let (sl8, rov) = (k.sl8.pubkey(), k.rov.pubkey());
    match class {
        Class::Trader => [(k.trader.pubkey(), true); 2],
        Class::Evaluator => [(k.evaluator.pubkey(), true); 2],
        Class::Sl8Alone => [(sl8, true), (rov, false)],
        Class::RovAlone => [(sl8, false), (rov, true)],
        Class::BothAdmins => [(sl8, true), (rov, true)],
        Class::Random => [(k.random.pubkey(), true); 2],
        Class::AuthorityPda => [(k.authority, false); 2],
        Class::NoSigner => [(sl8, false), (rov, false)],
        Class::BothSwapped => [(rov, true), (sl8, true)],
        Class::Sl8Twice => [(sl8, true); 2],
        Class::RovTwice => [(rov, true); 2],
        other => panic!("{other:?} has no meaning for an admin instruction"),
    }
}

fn set_slot(ix: &mut Instruction, idx: usize, key: Pubkey, signs: bool) {
    let m = &mut ix.accounts[idx];
    *m = AccountMeta { pubkey: key, is_signer: signs, is_writable: m.is_writable };
}

enum Shape {
    /// The single signer slot is account 0; the instruction was built with `legit` in it.
    One(Pubkey),
    /// The two admin slots, found by the SL8 and Rov keys.
    Admins,
    /// The instruction is already final (open_account, reset_account).
    Done,
}

struct Cell {
    result: TransactionResult,
    /// Index of the instruction under test in the transaction (a compute-budget instruction may precede it).
    idx: u8,
    before: [u8; 32],
    after: [u8; 32],
    /// The postcondition of a success (only meaningful when `result` is Ok).
    post_ok: bool,
}

fn send_with(e: &mut Env, k: &Keys, fee_payer: &Keypair, ixs: &[Instruction]) -> TransactionResult {
    let bh = e.svm.latest_blockhash();
    let msg = Message::new_with_blockhash(ixs, Some(&fee_payer.pubkey()), &bh);
    let mut tx = Transaction::new_unsigned(msg);
    let n = tx.message.header.num_required_signatures as usize;
    let required: Vec<Pubkey> = tx.message.account_keys[..n].to_vec();
    let mut seen = BTreeSet::new();
    let mut sigs: Vec<&Keypair> = Vec::new();
    for cand in std::iter::once(fee_payer).chain(k.all()) {
        if required.contains(&cand.pubkey()) && seen.insert(cand.pubkey()) {
            sigs.push(cand);
        }
    }
    tx.partial_sign(sigs.as_slice(), bh);
    let r = e.svm.send_transaction(tx);
    e.svm.expire_blockhash();
    r
}

/// Digest of every account an instruction of this program can touch, in a vault world. The accounts named by the instruction and the keys
/// of every class are added; the fee payer is left out (it pays the fee).
fn world_digest(w: &JEnv, k: &Keys, ixs: &[Instruction], fee_payer: &Pubkey) -> [u8; 32] {
    let mut extra: Vec<Pubkey> = Vec::new();
    for ix in ixs {
        extra.extend(ix.accounts.iter().map(|m| m.pubkey));
    }
    extra.extend(k.all().iter().map(|x| x.pubkey()));
    extra.push(k.authority);
    for m in 0..8u16 {
        extra.push(market_pda(m));
    }
    let tr = k.trader.pubkey();
    for id in 0..4u64 {
        let vts = vault_trader_state_pda(&tr, id);
        for rid in 1..=6u64 {
            extra.push(claim_pda(&vts, rid));
        }
    }
    for other in [&k.evaluator, &k.sl8, &k.rov, &k.random, &k.new_eval] {
        let pk = other.pubkey();
        extra.push(wallet_pda(&pk));
        for id in 0..4u64 {
            extra.push(acct_pda(&pk, id));
            extra.push(vault_trader_state_pda(&pk, id));
        }
    }
    extra.retain(|a| a != fee_payer);
    extra.sort();
    extra.dedup();
    w.v.digest(&tr, &extra)
}

/// Rotates the evaluator to `k.new_eval` through the real instruction (both admins), for the old/new evaluator extra classes.
fn rotate_evaluator(w: &mut JEnv, k: &Keys) {
    let ix = set_evaluator_ix(common::both(&w.v.e), k.new_eval.pubkey());
    assert_ok(w.v.e.send_as_admins(&[ix]));
    assert_eq!(w.v.e.config().evaluator, k.new_eval.pubkey());
}

/// Runs `ix` as `class` in the vault world `w` at clock `t`, with `pre` instructions (compute budget) in front.
#[allow(clippy::too_many_arguments)]
fn go(w: &mut JEnv, k: &Keys, class: Class, shape: Shape, pre: Vec<Instruction>, mut ix: Instruction, t: i64, post: &dyn Fn(&JEnv) -> bool) -> Cell {
    if matches!(class, Class::OldEvaluator | Class::NewEvaluator) {
        rotate_evaluator(w, k);
    }
    let mut fee_payer = dup(&w.v.e.payer);
    match shape {
        Shape::One(legit) => {
            assert_eq!(ix.accounts[0].pubkey, legit, "the builder did not put the legitimate key in slot 0");
            let (key, signs) = one_slot(class, k, legit);
            set_slot(&mut ix, 0, key, signs);
            if class == Class::BothAdmins {
                // both admin signatures are on the transaction: SL8 in the slot, Rov signs as the fee payer
                fee_payer = dup(&k.rov);
            }
        }
        Shape::Admins => {
            let a = ix.accounts.iter().position(|m| m.pubkey == k.sl8.pubkey()).expect("SL8 slot");
            let b = ix.accounts.iter().position(|m| m.pubkey == k.rov.pubkey()).expect("Rov slot");
            let [(ka, sa), (kb, sb)] = admin_slots(class, k);
            set_slot(&mut ix, a, ka, sa);
            set_slot(&mut ix, b, kb, sb);
        }
        Shape::Done => {}
    }
    w.set_time(t);
    let idx = u8::try_from(pre.len()).unwrap();
    let mut ixs = pre;
    ixs.push(ix);
    let before = world_digest(w, k, &ixs, &fee_payer.pubkey());
    let result = send_with(&mut w.v.e, k, &fee_payer, &ixs);
    let after = world_digest(w, k, &ixs, &fee_payer.pubkey());
    let post_ok = result.is_ok() && post(w);
    Cell { result, idx, before, after, post_ok }
}

/// The verdict of a cell against its hand-written expectation. `Err` carries a readable reason.
fn verdict(cell: &Cell, exp: Exp) -> Result<(), String> {
    match (exp, &cell.result) {
        (Exp::Ok, Ok(_)) => {
            if !cell.post_ok {
                return Err("succeeded but the postcondition of the instruction does not hold".into());
            }
            if cell.before == cell.after {
                return Err("succeeded but changed no account".into());
            }
            Ok(())
        }
        (Exp::Ok, Err(f)) => Err(format!("expected success, got {:?}\n{}", f.err, f.meta.logs.join("\n"))),
        (_, Ok(m)) => Err(format!("expected {}, but the transaction SUCCEEDED\n{}", exp_text(exp), m.logs.join("\n"))),
        (Exp::Prog(_) | Exp::Anchor(_), Err(f)) => {
            let want = match exp {
                Exp::Prog(p) => u32::from(p),
                Exp::Anchor(n) => n,
                Exp::Ok => unreachable!(),
            };
            match &f.err {
                TransactionError::InstructionError(i, InstructionError::Custom(code)) => {
                    if *i != cell.idx || *code != want {
                        return Err(format!("expected {} (code {want} at instruction {}), got code {code} at instruction {i}\n{}", exp_text(exp), cell.idx, f.meta.logs.join("\n")));
                    }
                }
                other => return Err(format!("expected {}, got {other:?}\n{}", exp_text(exp), f.meta.logs.join("\n"))),
            }
            if cell.before != cell.after {
                return Err("the refusal CHANGED account bytes (digest differs)".into());
            }
            Ok(())
        }
    }
}

/// Runs the eight cells (and the extra cells) of one row; collects every discrepancy.
fn run_row(ix: &str, variant: &str) {
    let rs = rows();
    let row = rs.iter().find(|r| r.ix == ix && r.variant == variant).unwrap_or_else(|| panic!("no row {ix} {variant}"));
    let mut failures: Vec<String> = Vec::new();
    let cells = CLASSES.iter().copied().zip(row.expect.iter().copied()).chain(row.extra.iter().copied());
    for (class, exp) in cells {
        let outcome = catch_unwind(AssertUnwindSafe(|| (row.run)(class)));
        let msg = match outcome {
            Ok(cell) => verdict(&cell, exp),
            Err(p) => {
                let text = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                Err(format!("the cell panicked: {text}"))
            }
        };
        if let Err(m) = msg {
            failures.push(format!("[{ix} {variant}] x [{}] expected {}: {m}", class_name(class), exp_text(exp)));
        }
    }
    assert!(failures.is_empty(), "{} cell(s) disagree with the hand-written table:\n{}", failures.len(), failures.join("\n---\n"));
}

// ====================================================================================================== state helpers

const CAP: u64 = 100_000_000_000;
const DELAY: u32 = 172_800;
const WINDOW: u32 = 86_400;

fn budget_ix() -> Instruction {
    Instruction {
        program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(),
        accounts: vec![],
        data: [vec![2u8], 600_000u32.to_le_bytes().to_vec()].concat(),
    }
}

fn post_journal_ix(w: &JEnv, signer: Pubkey, bytes: Vec<u8>) -> Instruction {
    let metas = accounts::PostJournal {
        evaluator: signer,
        trading_account: w.acct,
        config: config_pda(),
        ruleset: ruleset_pda(w.account().ruleset_version),
        event_authority: event_authority(),
        program: PROGRAM_ID,
    }
    .to_account_metas(None);
    Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::PostJournal { batch_bytes: bytes }.data() }
}

fn fund_buyer(w: &mut JEnv, pk: &Pubkey) {
    if !w.v.traders.contains_key(pk) {
        let mint = w.v.usdc;
        let ta = w.v.new_token_account(&mint, pk, TRADER_START);
        w.v.traders.insert(*pk, ta);
    }
}

fn exists_owned(w: &JEnv, addr: &Pubkey) -> bool {
    w.v.e.svm.get_account(addr).is_some_and(|a| a.owner == PROGRAM_ID && !a.data.is_empty())
}

// ====================================================================================================== the runners (one per row)

fn plain_digest(e: &Env, addrs: &[Pubkey]) -> [u8; 32] {
    let mut h = Sha256::new();
    for a in addrs {
        h.update(a.as_ref());
        match e.svm.get_account(a) {
            Some(acc) if acc.lamports > 0 => {
                h.update([1u8]);
                h.update(acc.lamports.to_le_bytes());
                h.update(acc.owner.as_ref());
                h.update(&acc.data);
            }
            _ => h.update([0u8]),
        }
    }
    h.finalize().into()
}

/// `init_config` needs a world WITHOUT a config, so it does not use the vault world.
fn r_init_config(class: Class) -> Cell {
    let mut e = Env::new();
    let k = Keys {
        trader: Keypair::new(),
        evaluator: Keypair::new(),
        sl8: dup(&e.sl8),
        rov: dup(&e.rov),
        random: Keypair::new(),
        new_eval: Keypair::new(),
        authority: sector_authority_pda(),
    };
    let args = default_args();
    let mut ix = init_ix_with(e.payer.pubkey(), k.sl8.pubkey(), k.rov.pubkey(), &args);
    let [(ka, sa), (kb, sb)] = admin_slots(class, &k);
    set_slot(&mut ix, 1, ka, sa);
    set_slot(&mut ix, 2, kb, sb);
    let fee_payer = dup(&e.payer);
    let mut watch = vec![config_pda(), tally_pda()];
    watch.extend(k.all().iter().map(|x| x.pubkey()));
    watch.retain(|a| *a != fee_payer.pubkey());
    watch.sort();
    watch.dedup();
    let before = plain_digest(&e, &watch);
    let result = send_with(&mut e, &k, &fee_payer, &[ix]);
    let after = plain_digest(&e, &watch);
    let post_ok = result.is_ok() && {
        let cfg = e.config();
        let tally = e.svm.get_account(&tally_pda()).unwrap();
        cfg.evaluator == args.evaluator && cfg.admin_a == k.sl8.pubkey() && cfg.admin_b == k.rov.pubkey() && tally.data.len() == 25
    };
    Cell { result, idx: 0, before, after, post_ok }
}

fn r_set_evaluator(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let new_key = Pubkey::new_from_array([0x42; 32]);
    let ix = set_evaluator_ix(common::both(&w.v.e), new_key);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| w.v.e.config().evaluator == new_key)
}

fn r_set_limits(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let ix = set_limits_ix(common::both(&w.v.e), 200_000, 6_000_000_000, 3_600);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| {
        let c = w.v.e.config();
        (c.pass_delay_secs, c.max_payout_per_window, c.window_secs) == (200_000, 6_000_000_000, 3_600)
    })
}

fn r_pause(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let ix = set_paused_ix(common::both(&w.v.e), true);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| w.v.e.config().paused == 1)
}

fn r_unpause(class: Class) -> Cell {
    let (mut w, k) = fresh();
    w.pause_sector(true);
    assert_eq!(w.v.e.config().paused, 1);
    let ix = set_paused_ix(common::both(&w.v.e), false);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| w.v.e.config().paused == 0)
}

fn r_create_ruleset(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let mut args = launch_ruleset_args();
    args.version = 2;
    let ix = create_ruleset_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), &args);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| w.v.e.config().current_ruleset == 2 && w.v.e.ruleset(2).version == 2)
}

fn r_create_market(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let id = w.v.e.config().market_count;
    let ix = create_market_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), &create_market_args(id, 0, 55));
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &move |w| w.v.e.config().market_count == id + 1 && w.v.e.market(id).feed_id == [55; 32])
}

fn r_update_market(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let args = UpdateMarketArgs { market_id: 0, params: market_params(77) };
    let ix = update_market_ix(Admins::both(&w.v.e), &args);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| w.v.e.market(0).feed_id == [77; 32])
}

fn r_ban_wallet(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let trader = k.trader.pubkey();
    let ix = ban_wallet_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), trader, hash(5));
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &move |w| w.v.e.wallet(&trader).banned == 1)
}

fn r_unban_wallet(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let trader = k.trader.pubkey();
    let ban = ban_wallet_ix(w.v.e.payer.pubkey(), Admins::both(&w.v.e), trader, hash(5));
    let a = Admins::both(&w.v.e);
    assert_ok(w.v.e.send_admins(ban, a));
    assert_eq!(w.v.e.wallet(&trader).banned, 1);
    let ix = unban_wallet_ix(Admins::both(&w.v.e), trader);
    let t = w.t0 + 1_000;
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &move |w| w.v.e.wallet(&trader).banned == 0)
}

fn r_open_account(class: Class) -> Cell {
    let (mut w, k) = fresh();
    for key in [&k.evaluator, &k.sl8, &k.rov, &k.random] {
        fund_buyer(&mut w, &key.pubkey());
    }
    let t = w.t0 + 1_000;
    let buyer = match class {
        Class::Trader | Class::AuthorityPda | Class::NoSigner => k.trader.pubkey(),
        Class::Evaluator => k.evaluator.pubkey(),
        Class::Sl8Alone | Class::BothAdmins => k.sl8.pubkey(),
        Class::RovAlone => k.rov.pubkey(),
        Class::Random => k.random.pubkey(),
        other => panic!("{other:?} not used for open_account"),
    };
    let mut oa = w.v.open_accounts(&buyer);
    match class {
        Class::BothAdmins => oa.payer = k.rov.pubkey(),
        Class::AuthorityPda => {
            oa.trader = k.authority;
            oa.payer = k.authority;
            oa.trader_signs = false;
            oa.payer_signs = false;
        }
        Class::NoSigner => {
            oa.trader_signs = false;
            oa.payer_signs = false;
        }
        _ => {}
    }
    let acct = oa.trading_account;
    let ix = oa.to_ix(0);
    go(&mut w, &k, class, Shape::Done, vec![], ix, t, &move |w| exists_owned(w, &acct))
}

fn r_reset_account(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    // the TRADER's account 0 is Failed and resettable
    w.fail_in(1, t);
    let buyer: Option<&Keypair> = match class {
        Class::Evaluator => Some(&k.evaluator),
        Class::Sl8Alone | Class::BothAdmins => Some(&k.sl8),
        Class::RovAlone => Some(&k.rov),
        Class::Random => Some(&k.random),
        _ => None,
    };
    // a non-trader buyer owns a funded token account and one live account (so its wallet profile exists), then names the TRADER's failed account
    if let Some(x) = buyer {
        fund_buyer(&mut w, &x.pubkey());
        assert_ok(w.v.open(x, 0));
    }
    let prev_id = w.account().challenge_id;
    let mut ra = w.reset_accts();
    if let Some(x) = buyer {
        let xp = x.pubkey();
        let id = w.v.wallet_opt(&xp).unwrap().next_challenge_id;
        ra.trader = xp;
        ra.payer = xp;
        ra.wallet = wallet_pda(&xp);
        ra.trading_account = acct_pda(&xp, id);
        ra.vault_trader_state = vault_trader_state_pda(&xp, id);
        ra.trader_token_account = w.v.trader_token(&xp);
    }
    match class {
        Class::BothAdmins => ra.payer = k.rov.pubkey(),
        Class::AuthorityPda => {
            ra.trader = k.authority;
            ra.payer = k.authority;
            ra.trader_signs = false;
            ra.payer_signs = false;
        }
        Class::NoSigner => {
            ra.trader_signs = false;
            ra.payer_signs = false;
        }
        _ => {}
    }
    let new_acct = ra.trading_account;
    let prev_acct = ra.prev_trading_account;
    let ix = ra.to_ix(prev_id);
    go(&mut w, &k, class, Shape::Done, vec![], ix, t + 100, &move |w| exists_owned(w, &new_acct) && w.v.account(&prev_acct).reset_at() != 0)
}

fn r_post_journal(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    let batch = simple(&w, t, vec![], true);
    let ix = post_journal_ix(&w, k.evaluator.pubkey(), batch.to_bytes().expect("encodes"));
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![budget_ix()], ix, t, &|w| w.account().seq == 1)
}

fn r_request_advance(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.ready(2);
    let ix = w.request_ix();
    go(&mut w, &k, class, Shape::One(k.trader.pubkey()), vec![], ix, t, &|w| w.account().pass_pending == c::PASS_REQUESTED)
}

fn r_cancel_pass(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 10_000;
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t - 4_000, 0);
    let ix = w.cancel_ix();
    go(&mut w, &k, class, Shape::One(k.trader.pubkey()), vec![], ix, t, &|w| w.account().pass_pending == c::PASS_NONE)
}

fn r_confirm_pass(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t - 100, 0);
    let ix = w.confirm_ix(k.evaluator.pubkey());
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![], ix, t, &|w| w.account().pass_pending == c::PASS_CONFIRMED)
}

fn r_reject_pass(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.ready(2);
    w.set_pending(c::PASS_REQUESTED, t - 100, 0);
    let ix = w.reject_ix(k.evaluator.pubkey(), c::REJECT_OTHER);
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![], ix, t, &|w| w.account().pass_pending == c::PASS_NONE)
}

fn r_veto_pass(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t - 200, t - 100);
    let ix = w.veto_ix(Admins::both(&w.v.e));
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t, &|w| {
        let a = w.account();
        a.pass_pending == c::PASS_NONE && a.vetoes == 1
    })
}

fn r_finalize_pass(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.set_limits(DELAY, CAP, WINDOW);
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, t - 200, t);
    // the delay has elapsed exactly at t + DELAY
    let ix = w.finalize_accts(k.trader.pubkey()).to_ix();
    go(&mut w, &k, class, Shape::One(k.trader.pubkey()), vec![], ix, t + i64::from(DELAY), &|w| {
        let a = w.account();
        a.phase == 3 && a.pass_pending == c::PASS_NONE && w.tally().requested_count == 1
    })
}

fn r_report_fail(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.edit_account(|a| a.b = FLOOR);
    let batch = w.fail_batch(t, vec![], journal_types::FailReason::StaticFloor, FLOOR, true);
    let ix = w.report_fail_ix(k.evaluator.pubkey(), batch.to_bytes().expect("encodes"), 0, &[MARKET_EUR]);
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![], ix, t + 1, &|w| w.account().status == c::STATUS_FAILED)
}

fn r_hold_account(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    let ix = w.hold_ix(k.evaluator.pubkey(), hash(1));
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![], ix, t, &|w| w.account().held == 1 && w.wallet_profile().held_count == 1)
}

fn r_release_hold(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    assert_ok(w.hold(t, hash(1)));
    assert_eq!(w.account().held, 1);
    let ix = w.release_hold_ix(Admins::both(&w.v.e));
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t + 1, &|w| w.account().held == 0 && w.wallet_profile().held_count == 0)
}

fn r_fail_for_violation(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    assert_ok(w.hold(t, hash(1)));
    let ix = w.violation_ix(Admins::both(&w.v.e), hash(9));
    go(&mut w, &k, class, Shape::Admins, vec![], ix, t + 1, &|w| {
        let a = w.account();
        a.status == c::STATUS_FAILED && a.violation_failed() && w.wallet_profile().banned == 1
    })
}

fn r_touch_activity(class: Class) -> Cell {
    let (mut w, k) = fresh();
    // past the 86,000 s throttle, so the vault call really runs and `last_activity_pushed` moves
    let t = w.t0 + 90_000;
    let ix = w.touch_ix(k.evaluator.pubkey());
    go(&mut w, &k, class, Shape::One(k.evaluator.pubkey()), vec![], ix, t, &move |w| w.account().last_activity_pushed == t)
}

fn r_release_account(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 90_000;
    let ix = w.release_acct_ix(k.trader.pubkey());
    go(&mut w, &k, class, Shape::One(k.trader.pubkey()), vec![], ix, t, &move |w| w.account().last_activity_pushed == t)
}

fn r_close_account(class: Class) -> Cell {
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.make_terminal(c::STATUS_FAILED, t);
    let ix = w.close_ix(k.trader.pubkey());
    let acct = w.acct;
    go(&mut w, &k, class, Shape::One(k.trader.pubkey()), vec![], ix, t + RETENTION, &move |w| {
        w.v.e.svm.get_account(&acct).is_none_or(|a| a.lamports == 0 || a.data.is_empty())
    })
}

// ====================================================================================================== the tests (one per row)

macro_rules! row_tests {
    ($($name:ident => ($ix:expr, $variant:expr)),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_row($ix, $variant);
            }
        )*
        /// Every row has a test of its own; keeps `row_tests!` and `rows()` from drifting apart.
        const TESTED_ROWS: &[(&str, &str)] = &[$(($ix, $variant)),*];
    };
}

row_tests! {
    matrix_init_config => ("init_config", ""),
    matrix_set_evaluator => ("set_evaluator", ""),
    matrix_set_limits => ("set_limits", ""),
    matrix_set_paused_pause => ("set_paused", "pause"),
    matrix_set_paused_unpause => ("set_paused", "unpause"),
    matrix_create_ruleset => ("create_ruleset", ""),
    matrix_create_market => ("create_market", ""),
    matrix_update_market => ("update_market", ""),
    matrix_ban_wallet => ("ban_wallet", ""),
    matrix_unban_wallet => ("unban_wallet", ""),
    matrix_open_account => ("open_account", ""),
    matrix_reset_account => ("reset_account", ""),
    matrix_post_journal => ("post_journal", ""),
    matrix_request_advance => ("request_advance", ""),
    matrix_cancel_pass => ("cancel_pass", ""),
    matrix_confirm_pass => ("confirm_pass", ""),
    matrix_reject_pass => ("reject_pass", ""),
    matrix_veto_pass => ("veto_pass", ""),
    matrix_finalize_pass => ("finalize_pass", ""),
    matrix_report_fail => ("report_fail", ""),
    matrix_hold_account => ("hold_account", ""),
    matrix_release_hold => ("release_hold", ""),
    matrix_fail_for_violation => ("fail_for_violation", ""),
    matrix_touch_activity => ("touch_activity", ""),
    matrix_release_account => ("release_account", ""),
    matrix_close_account => ("close_account", ""),
}

#[test]
fn every_row_has_its_own_test() {
    let tested: BTreeSet<(&str, &str)> = TESTED_ROWS.iter().copied().collect();
    let rows: BTreeSet<(&str, &str)> = rows().iter().map(|r| (r.ix, r.variant)).collect();
    assert_eq!(tested.len(), TESTED_ROWS.len(), "a row is tested twice");
    assert_eq!(tested, rows, "row_tests! and rows() disagree");
}

// ====================================================================================================== pinned observations (LIMITED)

/// LIMITED / finding WHO-1 (info). DESIGN 14 says the evaluator "cannot ... open accounts". The program has no role check on a buyer:
/// the evaluator key, an admin key or any random key buys an account for ITSELF like any wallet (it pays the price and the fee from its
/// own funds). The matrix cells above already show it; this test pins it so that a later role check (a DESIGN change) is a deliberate edit.
/// What stays impossible: opening an account FOR someone else (the trader slot must sign and the token account must be the signer's).
#[test]
fn limited_open_account_has_no_role_check_for_the_evaluator_or_admin_keys() {
    for class in [Class::Evaluator, Class::Sl8Alone, Class::RovAlone, Class::Random] {
        let cell = r_open_account(class);
        verdict(&cell, OK).unwrap_or_else(|m| panic!("{class:?}: {m}"));
    }
    // opening FOR someone else is refused: the victim's key in the trader slot without a signature
    let (mut w, k) = fresh();
    let victim = k.trader.pubkey();
    let mut oa = w.v.open_accounts(&victim);
    oa.trader_signs = false;
    oa.payer = k.random.pubkey();
    oa.payer_signs = true;
    let ix = oa.to_ix(0);
    let t = w.t0 + 1_000;
    let cell = go(&mut w, &k, Class::Random, Shape::Done, vec![], ix, t, &|_| true);
    verdict(&cell, NS).expect("a payer cannot open an account for a trader who did not sign");
}

#[test]
fn limited_swapped_admin_slots_are_refused_even_though_both_admins_signed() {
    // DESIGN says "both admins". The program compares the admin keys by POSITION (admin_a slot = config.admin_a): the right two
    // signatures in the wrong slots are Unauthorized. Stricter than the text, harmless (the client builds the order). Pinned for every
    // two-slot row by the extra class BothSwapped; this test shows the one case by itself with the digest unchanged.
    let (mut w, k) = fresh();
    let ix = set_evaluator_ix(common::both(&w.v.e), Pubkey::new_from_array([0x42; 32]));
    let t = w.t0 + 1_000;
    let cell = go(&mut w, &k, Class::BothSwapped, Shape::Admins, vec![], ix, t, &|_| true);
    verdict(&cell, U).expect("swapped slots must be Unauthorized and change nothing");
}

#[test]
fn authority_pda_cannot_sign_anything_outside_this_programs_own_cpi() {
    // The vault authority PDA has no key. A transaction can only carry it as a non-signer; the runtime refuses to mark a PDA of a
    // program as a signer unless that program signs for it with invoke_signed. The instructions that DO use it
    // (open/reset/request_advance/finalize/report_fail/fail_for_violation/touch/release) only pass it to the vault. Here: every
    // instruction row refuses the PDA class (asserted in the matrix), and the table has no ALLOWED cell in that column.
    let rs = rows();
    let col = CLASSES.iter().position(|c| *c == Class::AuthorityPda).unwrap();
    assert!(rs.iter().all(|r| r.expect[col] != OK));
}

// ====================================================================================================== the harness cannot pass vacuously

#[test]
fn the_verdict_rejects_every_kind_of_disagreement() {
    // a refused cell (Trader on set_evaluator is Unauthorized 6000 and changes nothing)
    let refused = r_set_evaluator(Class::Trader);
    verdict(&refused, U).expect("the real refusal matches its expectation");
    assert!(verdict(&refused, OK).is_err(), "a refusal must not pass as success");
    assert!(verdict(&refused, NS).is_err(), "a wrong error code must fail");
    assert!(verdict(&refused, Exp::Prog(E::Paused)).is_err(), "a wrong program error must fail");
    // the same refusal, but pretend it had changed a byte
    let mut dirty = Cell { result: r_set_evaluator(Class::Trader).result, idx: 0, before: refused.before, after: refused.after, post_ok: false };
    dirty.after[0] ^= 1;
    assert!(verdict(&dirty, U).unwrap_err().contains("CHANGED"), "a refusal that changes the digest must fail");
    // an error on the wrong instruction index must fail
    let mut wrong_idx = Cell { result: r_set_evaluator(Class::Trader).result, idx: 1, before: refused.before, after: refused.before, post_ok: false };
    assert!(verdict(&wrong_idx, U).is_err());
    wrong_idx.idx = 0;
    assert!(verdict(&wrong_idx, U).is_ok());
    // a success cell
    let allowed = r_set_evaluator(Class::BothAdmins);
    verdict(&allowed, OK).expect("both admins really succeed");
    assert!(verdict(&allowed, U).is_err(), "a success must not pass as a refusal");
    let mut no_post = Cell { result: r_set_evaluator(Class::BothAdmins).result, idx: 0, before: allowed.before, after: allowed.after, post_ok: false };
    assert!(verdict(&no_post, OK).is_err(), "a success whose postcondition fails must fail");
    no_post.post_ok = true;
    no_post.after = no_post.before;
    assert!(verdict(&no_post, OK).is_err(), "a success that changed nothing must fail");
}

#[test]
fn the_digest_moves_when_any_watched_account_changes() {
    // If the digest were blind to an account, a refusal that corrupted it would pass. Touch each watched account in turn.
    let (mut w, k) = fresh();
    let dummy = set_evaluator_ix(common::both(&w.v.e), Pubkey::new_unique());
    let fee_payer = w.v.e.payer.pubkey();
    // a claim and a wallet of another key must exist to be altered: create them as plain accounts
    let vts = vault_trader_state_pda(&k.trader.pubkey(), 0);
    let claim = claim_pda(&vts, 1);
    let watched: Vec<(&str, Pubkey)> = vec![
        ("config", config_pda()),
        ("payout tally", tally_pda()),
        ("wallet profile", wallet_pda(&k.trader.pubkey())),
        ("market 0", market_pda(0)),
        ("market 4", market_pda(4)),
        ("rule set 1", ruleset_pda(1)),
        ("trading account", w.acct),
        ("vault trader state", vts),
        ("vault registry", registry_pda()),
        ("vault state", w.v.vault_state),
        ("pool token account", w.v.usdc_pool),
        ("fee token account", w.v.fee_usdc),
        ("trader token account", w.v.trader_token(&k.trader.pubkey())),
        ("sector authority", k.authority),
        ("payout claim 1 (absent)", claim),
        ("trader lamports", k.trader.pubkey()),
        ("evaluator lamports", k.evaluator.pubkey()),
        ("SL8 lamports", k.sl8.pubkey()),
        ("random key lamports", k.random.pubkey()),
        ("wallet of the evaluator (absent)", wallet_pda(&k.evaluator.pubkey())),
        ("account 0 of SL8 (absent)", acct_pda(&k.sl8.pubkey(), 0)),
    ];
    let base = world_digest(&w, &k, std::slice::from_ref(&dummy), &fee_payer);
    for (name, addr) in watched {
        let original = w.v.e.svm.get_account(&addr);
        let mut changed = original.clone().unwrap_or_default();
        if changed.data.is_empty() || original.is_none() {
            changed.lamports = changed.lamports.wrapping_add(1).max(1);
            changed.owner = PROGRAM_ID;
        } else {
            changed.data[0] ^= 0x01;
        }
        w.v.e.svm.set_account(addr, changed).unwrap();
        let moved = world_digest(&w, &k, std::slice::from_ref(&dummy), &fee_payer);
        assert_ne!(moved, base, "the digest is blind to {name}");
        match original {
            Some(o) => w.v.e.svm.set_account(addr, o).unwrap(),
            None => w.v.e.svm.set_account(addr, solana_account::Account::default()).unwrap(),
        }
        assert_eq!(world_digest(&w, &k, std::slice::from_ref(&dummy), &fee_payer), base, "restoring {name} did not restore the digest");
    }
    // the fee payer is the one account the digest ignores
    let before = world_digest(&w, &k, std::slice::from_ref(&dummy), &fee_payer);
    w.v.e.svm.airdrop(&fee_payer, 1).unwrap();
    assert_eq!(world_digest(&w, &k, std::slice::from_ref(&dummy), &fee_payer), before);
}

// ====================================================================================================== other signers on the trader's account

/// A SECOND trader who owns a real account of his own signs for the FIRST trader's account. Same answer as a stranger (6000), nothing changes.
#[test]
fn another_traders_signature_is_refused_on_the_four_trader_instructions() {
    let t_req = |w: &mut JEnv, t: i64| {
        w.ready(2);
        w.set_pending(c::PASS_NONE, 0, 0);
        w.set_time(t);
        w.request_ix()
    };
    for which in ["request_advance", "cancel_pass", "release_account", "close_account"] {
        let (mut w, k) = fresh();
        let t = w.t0 + 90_000;
        let mut ix = match which {
            "request_advance" => t_req(&mut w, t),
            "cancel_pass" => {
                w.ready(2);
                w.set_pending(c::PASS_REQUESTED, t - 4_000, 0);
                w.cancel_ix()
            }
            "release_account" => w.release_acct_ix(k.trader.pubkey()),
            _ => {
                w.make_terminal(c::STATUS_FAILED, t - RETENTION);
                w.close_ix(k.trader.pubkey())
            }
        };
        // trader 2 has his own funded wallet, token account and live account
        let t2 = w.v.new_trader();
        assert_ok(w.v.open(&t2, 0));
        assert!(w.v.wallet_opt(&t2.pubkey()).is_some());
        set_slot(&mut ix, 0, t2.pubkey(), true);
        let before = world_digest(&w, &k, std::slice::from_ref(&ix), &w.v.e.payer.pubkey());
        let r = w.v.e.send(&[ix.clone()], &[&t2]);
        let cell = Cell { result: r, idx: 0, before, after: world_digest(&w, &k, std::slice::from_ref(&ix), &w.v.e.payer.pubkey()), post_ok: false };
        verdict(&cell, U).unwrap_or_else(|m| panic!("{which}: {m}"));
        // and the second trader's own account is untouched too
        assert_eq!(w.v.account(&acct_pda(&t2.pubkey(), 0)).seq, 0);
    }
}

/// `open_account` and `reset_account` require payer == trader (DESIGN 6.2). The trader signs his slot, ANOTHER key signs the payer slot.
#[test]
fn a_payer_that_is_not_the_trader_is_refused_by_open_and_reset() {
    // open_account
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    let mut oa = w.v.open_accounts(&k.trader.pubkey());
    oa.payer = k.random.pubkey();
    let ix = oa.to_ix(0);
    let cell = go(&mut w, &k, Class::Trader, Shape::Done, vec![], ix, t, &|_| true);
    verdict(&cell, U).unwrap_or_else(|m| panic!("open_account: {m}"));
    // reset_account
    let (mut w, k) = fresh();
    let t = w.t0 + 1_000;
    w.fail_in(1, t);
    let mut ra = w.reset_accts();
    ra.payer = k.random.pubkey();
    let ix = ra.to_ix(0);
    let cell = go(&mut w, &k, Class::Trader, Shape::Done, vec![], ix, t + 100, &|_| true);
    verdict(&cell, U).unwrap_or_else(|m| panic!("reset_account: {m}"));
}

// ====================================================================================================== the printed matrix

fn markdown() -> String {
    let rs = rows();
    let mut s = String::new();
    s.push_str("| instruction | DESIGN rule |");
    for c in CLASSES {
        s.push_str(&format!(" {} |", class_name(c)));
    }
    s.push_str("\n|---|---|");
    for _ in CLASSES {
        s.push_str("---|");
    }
    s.push('\n');
    for r in &rs {
        let name = if r.variant.is_empty() { r.ix.to_string() } else { format!("{}({})", r.ix, r.variant) };
        s.push_str(&format!("| `{name}` | {} |", r.design));
        for e in r.expect {
            s.push_str(&format!(" {} |", exp_text(e)));
        }
        s.push('\n');
    }
    s.push_str("\nExtra classes:\n\n| instruction | class | expected |\n|---|---|---|\n");
    for r in &rs {
        let name = if r.variant.is_empty() { r.ix.to_string() } else { format!("{}({})", r.ix, r.variant) };
        for (c, e) in r.extra {
            s.push_str(&format!("| `{name}` | {} | {} |\n", class_name(*c), exp_text(*e)));
        }
    }
    s
}

#[test]
fn print_the_matrix() {
    let md = markdown();
    assert!(md.contains("`init_config`") && md.contains("`close_account`"));
    if std::env::var("WHO_MATRIX_PRINT").is_ok() {
        println!("{md}");
        println!("instructions {INSTRUCTIONS}, rows {ROWS}, cells {CELLS}, allowed {ALLOWED_CELLS}, refused {REFUSED_CELLS}, extra cells {EXTRA_CELLS}");
    }
}
