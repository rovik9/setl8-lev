//! Trust matrix rows of `reset_account` (M8): who catches each dishonest reset. The rows are asserted here and written into
//! `handover/trust-matrix.md` ("Reset (M8)"); the last test checks that every row name is in the document.
//!
//! WHO: PROGRAM = our program rejects (exact error code), VAULT = the vault rejects (exact vault error code or log name),
//! VERIFIER = the program accepts and the public verifier flags the history. A reset has no price argument and no phase
//! argument, so most of what a cheating trader or evaluator could try is simply not expressible; the rows are the ones that are.
mod common;

use common::journal::*;
use common::vault::*;
use common::*;
use setl8_lev_trading::{constants as c, errors::Setl8Error as E};
use solana_pubkey::Pubkey;
use solana_signer::Signer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Program,
    Vault,
    Verifier,
}

struct Row {
    name: &'static str,
    who: Who,
    /// The error code (program or vault) or the verifier's finding class name.
    caught_by: &'static str,
    run: fn() -> String,
}

const PRICES_100K: [u64; 5] = [859_000_000, 987_850_000, 1_030_800_000, 1_095_225_000, 1_116_700_000];

fn failed(phase: u8) -> (JEnv, i64) {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.fail_in(phase, t);
    (w, t + 100)
}

fn code_of(r: &litesvm::types::TransactionResult) -> String {
    use anchor_lang::solana_program::instruction::error::InstructionError;
    use solana_transaction_error::TransactionError;
    match r {
        Ok(_) => "accepted".into(),
        Err(f) => match &f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => c.to_string(),
            other => format!("{other:?}"),
        },
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "reset: a price below the registry's tier price paid to the vault (a fake reset price)",
            who: Who::Vault,
            caught_by: "6007",
            // not expressible through `reset_account` (no price argument; in production nobody but the program can sign as
            // `sector_authority`); the harness calls the vault directly with the PDA flagged as a signer
            run: || {
                let (mut w, _) = failed(2);
                let tk = w.trader.pubkey();
                let tr = dup(&w.trader);
                let r = w.v.vault_deposit_reset(&tr, 0, 1, PRICES_100K[1] - 1, 1);
                assert_eq!(w.v.vault_trader_state(&tk, 1), None, "nothing was created");
                code_of(&r)
            },
        },
        Row {
            name: "reset: the registry replaced by a copy with cheaper tiers at another address",
            who: Who::Program,
            caught_by: "6103",
            run: || {
                let (mut w, t) = failed(2);
                let real = w.v.e.svm.get_account(&registry_pda()).unwrap();
                let mut cheap = real.clone();
                let off = 8 + 32 + 4 + 8 * 16 + 8; // tier 8 (100K) cost
                cheap.data[off..off + 8].copy_from_slice(&1u64.to_le_bytes());
                let fake = Pubkey::new_unique();
                w.v.e.svm.set_account(fake, cheap).unwrap();
                let mut ra = w.reset_accts();
                ra.product_registry = fake;
                let before = w.pass_digest();
                let r = w.send_reset(t, &ra, 0);
                assert_eq!(w.pass_digest(), before);
                code_of(&r)
            },
        },
        Row {
            name: "reset: an account that has not failed (an Active account of a high phase)",
            who: Who::Program,
            caught_by: "6027",
            run: || {
                let mut w = JEnv::new();
                let t = w.t0 + 1_000;
                w.edit_account(|a| a.phase = 5);
                let r = w.reset(t);
                code_of(&r)
            },
        },
        Row {
            name: "reset: a second reset of the same failed record",
            who: Who::Program,
            caught_by: "6107",
            run: || {
                let (mut w, t) = failed(3);
                let tk = w.trader.pubkey();
                let ra = w.reset_accts();
                w.reset_ok(t);
                let mut again = ra.clone();
                again.trading_account = acct_pda(&tk, 2);
                again.vault_trader_state = vault_trader_state_pda(&tk, 2);
                code_of(&w.send_reset(t + 1, &again, 0))
            },
        },
        Row {
            name: "reset: a second reset of the same failed record with the local mark wiped (the vault is the backstop)",
            who: Who::Vault,
            caught_by: "6011",
            run: || {
                let (mut w, t) = failed(3);
                let tk = w.trader.pubkey();
                let prev = w.acct;
                let ra = w.reset_accts();
                w.reset_ok(t);
                let mut a = w.v.account(&prev);
                a.pass_requested_at = 0;
                let mut data = Vec::new();
                anchor_lang::AccountSerialize::try_serialize(&a, &mut data).unwrap();
                let mut acc = w.v.e.svm.get_account(&prev).unwrap();
                acc.data = data;
                w.v.e.svm.set_account(prev, acc).unwrap();
                let mut again = ra.clone();
                again.trading_account = acct_pda(&tk, 2);
                again.vault_trader_state = vault_trader_state_pda(&tk, 2);
                code_of(&w.send_reset(t + 2, &again, 0))
            },
        },
        Row {
            name: "reset: by a banned wallet",
            who: Who::Program,
            caught_by: "6035",
            run: || {
                let (mut w, t) = failed(2);
                let tk = w.trader.pubkey();
                let admins = Admins::both(&w.v.e);
                let ix = ban_wallet_ix(w.v.e.payer.pubkey(), admins, tk, [0xE5; 32]);
                assert_ok(w.v.e.send_admins(ix, admins));
                code_of(&w.reset(t))
            },
        },
        Row {
            name: "reset: a violation-failed account after the wallet was unbanned (DESIGN 2.9: no reset)",
            who: Who::Program,
            caught_by: "6108",
            run: || {
                let mut w = JEnv::new();
                let t = w.t0 + 1_000;
                w.ok(|w| w.hold(t, [1; 32]));
                w.ok(|w| w.violation(t + 1, [2; 32]));
                let tk = w.trader.pubkey();
                let admins = Admins::both(&w.v.e);
                assert_ok(w.v.e.send_admins(unban_wallet_ix(admins, tk), admins));
                let before = w.pass_digest();
                let r = w.reset(t + 100);
                assert_eq!(w.pass_digest(), before);
                code_of(&r)
            },
        },
        Row {
            name: "reset: while the sector is paused",
            who: Who::Program,
            caught_by: "6001",
            run: || {
                let (mut w, t) = failed(2);
                w.pause_sector(true);
                code_of(&w.reset(t))
            },
        },
        Row {
            name: "reset: while the vault pauses this product",
            who: Who::Vault,
            caught_by: "ProductNotActive",
            run: || {
                let (mut w, t) = failed(2);
                w.pause_vault_product();
                let r = w.reset(t);
                let logs = r.as_ref().err().map(|f| f.meta.logs.join("\n")).unwrap_or_default();
                if logs.contains("ProductNotActive") { "ProductNotActive".into() } else { code_of(&r) }
            },
        },
        Row {
            name: "reset into a higher phase: the new account copies the failed phase (there is no phase argument)",
            who: Who::Program,
            caught_by: "phase = failed phase",
            run: || {
                // whatever the ladder and the tiers say, the new account is in the failed account's phase and nowhere else
                let mut seen = Vec::new();
                for p in 1..=5u8 {
                    let (mut w, t) = failed(p);
                    w.reset_ok(t);
                    seen.push(w.account().phase);
                }
                assert_eq!(seen, vec![1, 2, 3, 4, 5]);
                "phase = failed phase".into()
            },
        },
        Row {
            name: "reset: an evaluator log that opens the new account in a higher phase than the chain's account",
            who: Who::Verifier,
            caught_by: "StateMismatch",
            run: || {
                // the chain's new account is in phase 3; the log's `opened` line says 4: the snapshot comparison flags it
                let (mut w, t) = failed(3);
                w.reset_ok(t);
                let a = w.account();
                let acct = w.acct.to_bytes();
                let rec = |phase: u8| setl8_verify::log::OpenedRec {
                    account: acct,
                    trader: w.trader.pubkey().to_bytes(),
                    challenge_id: a.challenge_id,
                    size_s: a.size_s,
                    tier_index: 8,
                    phase,
                    ruleset_version: 1,
                    genesis_hash: journal_types::genesis_hash(&journal_types::NativeSha256, &acct),
                    time: a.created_at,
                };
                let snapshot = serde_json::to_string(&serde_json::json!({"version": 1, "b": a.b, "q_tally": a.q_tally, "latch": a.latch, "tight_on": a.tight_on,
                    "r": a.r, "g_peak": a.g_peak, "l_level": a.l_level, "lock_until": a.lock_until, "seq": a.seq,
                    "journal_hash": setl8_verify::hexutil::encode(&a.journal_hash), "flat": a.flat, "phase": a.phase, "status": a.status, "trades_closed": a.trades_closed}))
                .unwrap();
                let rules = r#"{"version":1,"ruleset_version":1,"fee_bps":40,"leverage_x":[50,20,20,20,10,5,5,2],"holding_fee_e6":[100,200,400,200,400,400,1000,1600],"commission_e6":[0,0,0,0,0,0,0,0],"tier_sizes":[100000000000],"spread_e6":{},"markets":[]}"#;
                let archive = {
                    let mut v = Vec::new();
                    v.extend_from_slice(&setl8_verify::archive::header_bytes());
                    v
                };
                let verify = |phase: u8| {
                    let mut ev = setl8_verify::log::header_line();
                    ev.push_str(&setl8_verify::log::opened_line(&rec(phase)));
                    setl8_verify::verify_account(&setl8_verify::Inputs {
                        events: ev.as_bytes(),
                        rules: rules.as_bytes(),
                        archive: &archive,
                        intents: b"{\"type\":\"header\",\"format\":\"setl8-lev-intents\",\"version\":1}\n",
                        snapshot: Some(snapshot.as_bytes()),
                        second_archive: None,
                        as_of_seq: None,
                    })
                    .map(|r| r.classes().iter().map(|c| c.name()).collect::<Vec<_>>().join(","))
                    .unwrap_or_else(|e| format!("input error: {e}"))
                };
                assert_eq!(verify(3), "", "the honest opened line is accepted");
                verify(4)
            },
        },
    ]
}

#[test]
fn reset_trust_rows() {
    for row in rows() {
        let got = (row.run)();
        assert_eq!(got, row.caught_by, "{} ({:?})", row.name, row.who);
    }
}

#[test]
fn reset_trust_rows_are_documented() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/trust-matrix.md");
    let Ok(doc) = std::fs::read_to_string(&path) else { return };
    let section = doc.split("## Reset (M8)").nth(1).expect("a Reset (M8) section in trust-matrix.md");
    for r in rows() {
        assert!(section.contains(r.name), "reset row missing from the document: {}", r.name);
    }
    // the constants the rows rely on
    assert_eq!(c::STATUS_FAILED, 1);
    let _ = E::AccountAlreadyReset;
}
