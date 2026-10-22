//! P27 (decision 71): the compute budgets MEASURED ON A REAL RUNTIME, asserted without a validator.
//!
//! `handover/cu-runtime.json` is written by `tools/devnet-rehearsal measure` (`scripts/devnet-rehearsal.sh selftest measure`) on a local
//! `solana-test-validator` (Agave 4.0.2): the worst case of every instruction, each shape run three times. LiteSVM 0.7.0 embeds the
//! Agave 2.3.13 runtime and under-counts the compute of an event (`handover/cu-runtime.md`, "Why LiteSVM differs"), so `budget.rs`
//! keeps its LiteSVM ceilings as a REGRESSION check (relative changes) and the ABSOLUTE numbers are these.
//!
//! What this file asserts, from the recorded numbers alone:
//!   * the file is complete and was measured as the contract says (three runs per shape, the explicit 400,000 limit, a validator
//!     version and a feature summary);
//!   * every instruction stays under its RUNTIME ceiling (measured worst case + 20 %, rounded up to 100) and under 1,232 bytes;
//!   * every `post_journal` / `report_fail` shape is covered, with the 20 % margin, by the evaluator's `recommended_cu_limit`, and no
//!     limit is above 1,400,000;
//!   * every instruction above 70 % of the default 200,000 units is named in `handover/backend-compute-contract.md` (the backend MUST
//!     send a `SetComputeUnitLimit` for it);
//!   * the lookup-table transaction size the evaluator models equals what `solana-message` serializes, byte for byte, and the legacy size
//!     likewise.
//!
//! A re-measurement that raises a number fails here and in `crates/evaluator` (the table test) until the ceilings and the model are
//! updated on purpose. `BUDGET_RUNTIME_CALIBRATE=1` prints the ceilings to paste into `CEILINGS`.
mod common;

use std::collections::BTreeMap;

use anchor_lang::{InstructionData, ToAccountMetas};
use common::journal::*;
use common::vault::*;
use common::*;
use ::evaluator::compute::{recommended_cu_limit, MAX_CU_LIMIT};
use ::evaluator::{BatchKind, TxFormat};
use journal_types::{CloseReason, FailReason, FeeKind, JournalBatch, JournalEvent, PriceProof, Side};
use serde_json::Value;
use setl8_lev_trading::{accounts, instruction};
use solana_keypair::Keypair;
use anchor_lang::solana_program::instruction::Instruction;
use solana_message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::{versioned::VersionedTransaction, Transaction};

const DEFAULT_LIMIT: u64 = 200_000;
const TX_LIMIT: usize = 1_232;

/// Runtime ceilings per instruction: (name, compute units, transaction bytes with the limit instruction). Measured worst case + 20 %,
/// rounded up to 100 units / 10 bytes. Compare `budget.rs` (LiteSVM): eight instructions are above their LiteSVM ceilings here
/// (confirm_pass, create_market, create_ruleset, hold_account, unban_wallet, veto_pass, post_journal, report_fail).
const CEILINGS: &[(&str, u64, usize)] = &[
    ("ban_wallet", 46900, 610),
    ("cancel_pass", 37400, 250),
    ("close_account", 4100, 220),
    ("confirm_pass", 42800, 280),
    ("create_market", 50700, 640),
    ("create_ruleset", 70200, 1060),
    ("fail_for_violation", 60000, 670),
    ("finalize_pass", 104600, 580),
    ("hold_account", 43900, 350),
    ("init_config", 17400, 630),
    ("open_account", 123900, 790),
    ("post_journal", 226600, 1230),
    ("reject_pass", 40400, 280),
    ("release_account", 46400, 450),
    ("release_hold", 43900, 510),
    ("report_fail", 269400, 1220),
    ("request_advance", 61900, 450),
    ("reset_account", 185200, 860),
    ("set_evaluator", 37800, 510),
    ("set_limits", 37200, 500),
    ("set_paused", 37000, 480),
    ("touch_activity", 46400, 450),
    ("unban_wallet", 39300, 550),
    ("update_market", 41000, 600),
    ("veto_pass", 41600, 480),
];

fn runtime() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/cu-runtime.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("cu-runtime.json is JSON")
}

/// (max CU, max bytes) per instruction.
fn per_instruction(v: &Value) -> BTreeMap<String, (u64, usize)> {
    let mut out: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    for s in v["shapes"].as_array().expect("shapes") {
        let name = s["instruction"].as_str().expect("instruction").to_string();
        let cu = s["max_cu"].as_u64().expect("max_cu");
        let bytes = usize::try_from(s["tx_bytes"].as_u64().expect("tx_bytes")).unwrap();
        let e = out.entry(name).or_default();
        e.0 = e.0.max(cu);
        e.1 = e.1.max(bytes);
    }
    out
}

fn ceiling_of(cu: u64, bytes: usize) -> (u64, usize) {
    ((cu * 12).div_ceil(10).div_ceil(100) * 100, bytes.div_ceil(10) * 10)
}

#[test]
fn the_recording_is_complete_and_was_measured_as_the_contract_says() {
    let v = runtime();
    assert_eq!(v["format"], "setl8-cu-runtime");
    assert_eq!(v["complete"], true, "an incomplete measurement: {}", v["errors"]);
    assert_eq!(v["limit_requested"], 400_000, "every measured transaction carried SetComputeUnitLimit(400000)");
    assert_eq!(v["runs_per_shape"], 3);
    assert!(v["validator"]["version"].as_str().is_some_and(|s| s.contains("4.")), "the validator version is recorded: {}", v["validator"]["version"]);
    assert!(v["validator"]["features_active"].as_u64().unwrap_or(0) > 200, "the feature summary is recorded");
    assert!(v["program"]["sha256"].as_str().is_some_and(|s| s.len() == 64));
    for s in v["shapes"].as_array().unwrap() {
        let runs = s["runs"].as_array().unwrap();
        let single = s["params"]["singleton"] == true;
        assert!(runs.len() >= 3 || single, "{} / {}: {} runs", s["instruction"], s["shape"], runs.len());
        assert_eq!(s["max_cu"].as_u64(), runs.iter().filter_map(Value::as_u64).max(), "{} / {}: the maximum of the runs", s["instruction"], s["shape"]);
    }
    // every instruction of the program was measured
    let names: Vec<String> = per_instruction(&v).keys().cloned().collect();
    for want in [
        "ban_wallet", "cancel_pass", "close_account", "confirm_pass", "create_market", "create_ruleset", "fail_for_violation", "finalize_pass", "hold_account", "init_config",
        "open_account", "post_journal", "reject_pass", "release_account", "release_hold", "report_fail", "request_advance", "reset_account", "set_evaluator", "set_limits",
        "set_paused", "touch_activity", "unban_wallet", "update_market", "veto_pass",
    ] {
        assert!(names.iter().any(|n| n == want), "no measurement of {want}");
    }
}

#[test]
fn every_instruction_is_under_its_runtime_ceiling_and_the_packet_size() {
    let v = runtime();
    let measured = per_instruction(&v);
    if std::env::var("BUDGET_RUNTIME_CALIBRATE").is_ok() {
        println!("const CEILINGS: &[(&str, u64, usize)] = &[");
        for (name, (cu, bytes)) in &measured {
            let (c, b) = ceiling_of(*cu, *bytes);
            println!("    (\"{name}\", {c}, {b}),");
        }
        println!("];");
        return;
    }
    let mut problems = Vec::new();
    for (name, (cu, bytes)) in &measured {
        if *bytes > TX_LIMIT {
            problems.push(format!("{name}: {bytes} bytes is over {TX_LIMIT}"));
        }
        match CEILINGS.iter().find(|c| c.0 == name) {
            None => problems.push(format!("{name}: no runtime ceiling (measured {cu} CU, {bytes} bytes)")),
            Some((_, c, b)) => {
                if cu > c {
                    problems.push(format!("{name}: {cu} CU is over the runtime ceiling {c}"));
                }
                if bytes > b {
                    problems.push(format!("{name}: {bytes} bytes is over the ceiling {b}"));
                }
                if *c > MAX_CU_LIMIT {
                    problems.push(format!("{name}: ceiling {c} is above the transaction maximum"));
                }
            }
        }
    }
    for (name, ..) in CEILINGS {
        if !measured.contains_key(*name) {
            problems.push(format!("a ceiling for {name} but no measurement"));
        }
    }
    assert!(problems.is_empty(), "runtime budget problems:\n{}", problems.join("\n"));
}

#[test]
fn the_evaluator_model_covers_every_measured_batch_shape_with_the_margin() {
    let v = runtime();
    let mut checked = 0;
    for s in v["shapes"].as_array().unwrap() {
        let kind = match s["instruction"].as_str().unwrap() {
            "post_journal" => BatchKind::PostJournal,
            "report_fail" => BatchKind::ReportFail(engine::FailReason::StaticFloor),
            _ => continue,
        };
        let events = usize::try_from(s["params"]["events"].as_u64().unwrap()).unwrap();
        let proofs = usize::try_from(s["params"]["proofs"].as_u64().unwrap()).unwrap();
        let max = s["max_cu"].as_u64().unwrap();
        let limit = recommended_cu_limit(kind, events, proofs);
        assert!(limit * 10 >= max * 12 || limit == MAX_CU_LIMIT, "{} / {}: the limit {limit} is below 1.20 x {max}", s["instruction"], s["shape"]);
        assert!(limit <= MAX_CU_LIMIT);
        assert!(::evaluator::compute::measured_cu(kind, events, proofs) >= max, "{} / {}", s["instruction"], s["shape"]);
        checked += 1;
    }
    assert!(checked >= 60, "{checked} batch shapes checked");
    // the default 200,000 is not enough for the largest batches, the model asks for more
    assert!(recommended_cu_limit(BatchKind::ReportFail(engine::FailReason::PhaseQuick), 8, 3) > DEFAULT_LIMIT);
    assert!(recommended_cu_limit(BatchKind::PostJournal, 8, 3) > DEFAULT_LIMIT);
}

#[test]
fn every_instruction_above_70_percent_of_the_default_limit_is_named_in_the_backend_contract() {
    let v = runtime();
    let doc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../handover/backend-compute-contract.md");
    let doc = std::fs::read_to_string(&doc_path).unwrap_or_else(|e| panic!("{}: {e}", doc_path.display()));
    let mut heavy = Vec::new();
    for (name, (cu, _)) in per_instruction(&v) {
        if cu * 10 > DEFAULT_LIMIT * 7 {
            heavy.push(name.clone());
            assert!(doc.contains(&format!("`{name}`")), "{name} ({cu} CU, above 70% of 200,000) is not named in handover/backend-compute-contract.md");
        }
    }
    assert!(heavy.iter().any(|n| n == "post_journal") && heavy.iter().any(|n| n == "report_fail"), "{heavy:?}");
}

// ---------------------------------------------------------------- transaction sizes against the real serializer

fn proof(source: u8, i: u8) -> PriceProof {
    PriceProof { source, feed_id: [i; 32], timestamp_ns: 1_000_000_000, quantized_value: 1_000_000_000_000, signed_update: if source == 3 { vec![7; 32] } else { Vec::new() } }
}

fn close(market: u16, proof: u8) -> JournalEvent {
    JournalEvent::Close { market, side: Side::Long, qty: 1, entry_e12: 1, exit_e12: 1, time: 1, timer_start: 1, pnl: 0, quick: false, credited: 0, reason: CloseReason::Fail, intent_id: 0, proof }
}

fn meta() -> JournalEvent {
    JournalEvent::Meta { time: 1, flat: true, samples_hash: [1; 32], intents_hash: [2; 32], engine_version: 1 }
}

fn budget_ix(limit: u32) -> Instruction {
    Instruction { program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(), accounts: vec![], data: [vec![2u8], limit.to_le_bytes().to_vec()].concat() }
}

fn price_ix(micro_lamports: u64) -> Instruction {
    Instruction { program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(), accounts: vec![], data: [vec![3u8], micro_lamports.to_le_bytes().to_vec()].concat() }
}

/// Legacy and lookup-table transactions of real `post_journal` / `report_fail` instructions, serialized by `solana-message`.
#[test]
fn the_modelled_transaction_sizes_are_what_solana_message_serializes() {
    let w = JEnv::with_trader(8, Some(1));
    let ev: Keypair = Keypair::new_from_array([9; 32]);
    let blockhash = Default::default();
    let mut seen_v0 = 0;
    for n_markets in 1..=3usize {
        let markets: Vec<u16> = (0..n_markets as u16).collect();
        for (closes, fees) in [(1usize, 0usize), (3, 0), (3, 3), (2, 4), (5, 0), (6, 0)] {
            let mut events: Vec<JournalEvent> = (0..closes).map(|i| close(markets[i % n_markets], u8::try_from(i % n_markets).unwrap())).collect();
            events.extend((0..fees).map(|_| JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: 1, time: 1 }));
            events.push(JournalEvent::Fail { time: 1, reason: FailReason::StaticFloor, b_after: 1, proof: 0 });
            events.push(meta());
            if events.len() > 8 {
                continue;
            }
            let proofs: Vec<PriceProof> = (0..n_markets).map(|i| proof(3, u8::try_from(i).unwrap())).collect();
            let batch = JournalBatch { first_seq: 1, prev_hash: [0; 32], events, proofs };
            let bytes = batch.to_bytes().unwrap();
            let len = bytes.len();
            for fail in [false, true] {
                let (ix, kind, shared): (Instruction, BatchKind, Vec<solana_pubkey::Pubkey>) = if fail {
                    let ix = w.report_fail_ix(ev.pubkey(), bytes.clone(), 0, &markets);
                    let shared = vec![config_pda(), ruleset_pda(w.account().ruleset_version), sector_authority_pda(), VAULT_ID, registry_pda(), event_authority_pda()];
                    (ix, BatchKind::ReportFail(engine::FailReason::StaticFloor), shared)
                } else {
                    let mut metas = accounts::PostJournal { evaluator: ev.pubkey(), trading_account: w.acct, config: config_pda(), ruleset: ruleset_pda(w.account().ruleset_version), event_authority: event_authority_pda(), program: PROGRAM_ID }
                        .to_account_metas(None);
                    metas.extend(JEnv::market_metas(&markets));
                    let ix = Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::PostJournal { batch_bytes: bytes.clone() }.data() };
                    (ix, BatchKind::PostJournal, vec![config_pda(), ruleset_pda(w.account().ruleset_version), event_authority_pda()])
                };
                for price in [false, true] {
                    let mut ixs = vec![budget_ix(300_000)];
                    if price {
                        ixs.push(price_ix(1));
                    }
                    ixs.push(ix.clone());
                    // legacy
                    let msg = Message::new_with_blockhash(&ixs, Some(&ev.pubkey()), &blockhash);
                    let mut tx = Transaction::new_unsigned(msg);
                    tx.partial_sign(&[&ev], blockhash);
                    let legacy = bincode::serialize(&tx).unwrap().len();
                    assert_eq!(legacy, ::evaluator::compute::tx_size_with_budget(kind, n_markets, len, TxFormat::Legacy, price), "legacy: fail {fail} markets {n_markets} closes {closes} fees {fees} price {price}");
                    // version 0 with a table that holds the shared accounts and the markets
                    let mut addresses = shared.clone();
                    addresses.extend(markets.iter().map(|m| market_pda(*m)));
                    let table = AddressLookupTableAccount { key: solana_pubkey::Pubkey::new_unique(), addresses };
                    let m0 = v0::Message::try_compile(&ev.pubkey(), &ixs, &[table], blockhash).expect("compiles");
                    assert_eq!(m0.address_table_lookups.len(), 1);
                    let loaded: usize = m0.address_table_lookups.iter().map(|l| l.writable_indexes.len() + l.readonly_indexes.len()).sum();
                    assert_eq!(loaded, shared.len() + n_markets, "the accounts the model says a lookup table supplies");
                    let vtx = VersionedTransaction::try_new(VersionedMessage::V0(m0), &[&ev]).expect("signs");
                    let wire = bincode::serialize(&vtx).unwrap().len();
                    assert_eq!(wire, ::evaluator::compute::tx_size_with_budget(kind, n_markets, len, TxFormat::V0Lookup, price), "v0: fail {fail} markets {n_markets} closes {closes} fees {fees} price {price}");
                    if legacy > TX_LIMIT {
                        assert!(wire <= TX_LIMIT, "the lookup-table form must fit: {wire}");
                        seen_v0 += 1;
                    }
                }
            }
        }
    }
    assert!(seen_v0 > 0, "no shape needed the lookup table: the fallback is untested");
}
