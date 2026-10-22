//! `measure`: the worst-case shape of EVERY instruction of the program on a real runtime (the local validator), each run at least three
//! times, written as `cu-runtime.json` and `cu-runtime.md` (P27). The program checks the arithmetic of every journal event and its
//! proofs, not a position book, so junk accounts take the made-up but well-formed batches that give the heavy shapes (8 events with 3
//! proofs, forced close-alls, commission fees). Every batch carries an explicit `SetComputeUnitLimit` of 400,000 so it completes and
//! its true cost shows.
use std::collections::BTreeMap;

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey, system_instruction};
use journal_types::{CloseReason, FailReason, FeeKind, JournalBatch, JournalEvent, OrderKind, PriceProof, Side};
use serde_json::{json, Value};
use setl8_lev_trading::instructions::{CreateMarketArgs, MarketParams, RuleSetArgs, UpdateMarketArgs};
use setl8_lev_trading::state::{Market, RuleSet, Session};
use solana_keypair::Keypair;
use solana_signer::Signer;

use crate::cluster::Sent;
use crate::ix;
use crate::live::AcctRef;
use crate::rehearsal::{clone_key, Rehearsal};

/// The explicit limit every measured transaction carries: high enough for any shape, so the true cost shows.
pub const LIMIT: u32 = 400_000;
/// Runs per shape.
pub const RUNS: usize = 3;
/// Bytes a `SetComputeUnitLimit` adds to a transaction (program key 32 + instruction 1 + 1 + 1 + 5) and a `SetComputeUnitPrice` on top (1 + 1 + 1 + 9).
pub const BUDGET_LIMIT_BYTES: usize = 32 + 8;
pub const BUDGET_PRICE_BYTES: usize = 12;

#[derive(Clone, Debug)]
pub struct Run {
    pub cu: u64,
    pub bytes: usize,
    pub sig: String,
}

#[derive(Clone, Debug)]
pub struct ShapeRec {
    pub instruction: String,
    pub shape: String,
    pub params: Value,
    pub runs: Vec<Run>,
}

/// One section of the measurement.
type SectionFn = fn(&mut Rehearsal, &mut Meas, &AcctRef) -> Result<(), String>;

#[derive(Default)]
pub struct Meas {
    pub shapes: BTreeMap<(String, String), ShapeRec>,
    /// Shapes the batch format allows but no transaction can carry (they would exceed 1,232 bytes with the limit instruction): (shape, bytes).
    pub unsendable: Vec<(String, usize)>,
    /// Sections of the measurement that failed (the others are kept): (section, error).
    pub errors: Vec<(String, String)>,
}

impl Meas {
    pub fn add(&mut self, instruction: &str, shape: &str, mut params: Value, sent: &Sent) {
        if let Value::Object(o) = &mut params {
            o.insert("tx_format".into(), json!(if sent.v0 { "v0_lookup" } else { "legacy" }));
        }
        let e = self.shapes.entry((instruction.to_string(), shape.to_string())).or_insert_with(|| ShapeRec { instruction: instruction.into(), shape: shape.into(), params, runs: Vec::new() });
        e.runs.push(Run { cu: sent.info.cu, bytes: sent.size, sig: sent.info.signature.clone() });
    }

    pub fn to_json(&self, validator: &Value, program: &Value) -> Value {
        let shapes: Vec<Value> = self
            .shapes
            .values()
            .map(|s| {
                let max = s.runs.iter().max_by_key(|r| r.cu);
                json!({
                    "instruction": s.instruction, "shape": s.shape, "params": s.params,
                    "runs": s.runs.iter().map(|r| r.cu).collect::<Vec<_>>(),
                    "max_cu": max.map(|r| r.cu).unwrap_or(0),
                    "tx_bytes": s.runs.iter().map(|r| r.bytes).max().unwrap_or(0),
                    "max_signature": max.map(|r| r.sig.clone()).unwrap_or_default(),
                })
            })
            .collect();
        json!({
            "format": "setl8-cu-runtime", "version": 1, "limit_requested": LIMIT, "runs_per_shape": RUNS, "validator": validator, "program": program, "shapes": shapes,
            "complete": self.errors.is_empty(),
            "errors": self.errors.iter().map(|(s, e)| json!({"section": s, "error": e})).collect::<Vec<_>>(),
            "unsendable": self.unsendable.iter().map(|(s, b)| json!({"shape": s, "tx_bytes": b})).collect::<Vec<_>>()
        })
    }
}

/// Instructions whose compute depends on the trader's key (the bump searches of the program's and the vault's addresses): the measured
/// figure is the worst of 30,000 keys, each further failed attempt costs about 1,500, so the contract gives them a fixed generous limit.
pub const KEY_DEPENDENT: [&str; 5] = ["open_account", "reset_account", "finalize_pass", "touch_activity", "release_account"];
/// The limit the contract gives a key-dependent instruction.
pub const KEY_DEPENDENT_LIMIT: u64 = 400_000;

/// Why LiteSVM differs (P27), with the evidence of the day it was measured.
pub const WHY_LITESVM: &str = "## Why LiteSVM differs

LiteSVM 0.7.0 (what `tests-rs` runs) embeds the Agave **2.3.13** runtime crates (`solana-program-runtime`, `solana-bpf-loader-program`, `solana-compute-budget` 2.3.13, `agave-feature-set` 2.3.13) and starts with `FeatureSet::all_enabled()` and `ComputeBudget::default()` of those crates; the validator that produced this table is Agave **4.0.2** (feature summary above). The program is the same source in both (the measured build only swaps the program id and the two admin keys and shortens four time constants).

Evidence, from the two runtimes' own numbers:

* An instruction that emits no event costs the SAME in both: `close_account` 3,340 CU in LiteSVM and 3,340 CU on the validator. The program's own instructions, the syscalls it makes and the account handling are priced alike.
* Every journal event is an Anchor `emit_cpi!`, a self-invocation of the program. The log line of the inner invocation reads `consumed 6539` in LiteSVM (`hold_account`, `post_journal` of 2 and 8 events: always 6,539) and `consumed 12539` on the validator (the P23 logs, `handover/devnet-rehearsal.md` D-01): **6,000 more inside every event**. Per extra event the whole transaction grows by 10,656 CU in LiteSVM (`post_journal` of 2 events 43,779, of 8 events 107,712) and by 16,604 on the validator (fees only: 2 events 61,672, 8 events 161,294): +5,948.
* So the error grows with the number of events: one event (`hold_account`) 24,633 against 36,578 (+11,945), eight events with three proofs `post_journal` 125,290 against 188,799 (x1.51), `report_fail` 151,284 against 224,440 (x1.48).
* It cannot be configured away. `LiteSVM::with_compute_budget` and `with_feature_set` are the only knobs. Every field of the 2.3.13 `ComputeBudget` (`invoke_units`, `cpi_bytes_per_unit`, `syscall_base_cost`, the hash and curve costs, the stack and heap sizes) prices something the CALLER pays (a syscall, the set-up of a call, bytes copied), none adds a cost inside the called program, and every feature of the 2.3.13 set is already on. The extra 6,000 therefore comes from code that only the 4.0.2 runtime has; which change of Agave 3.x to 4.0 it is was not isolated (no validator source is in this repository's reach).

Consequence: LiteSVM stays the REGRESSION harness (`tests-rs/tests/budget.rs`, a relative check; its ceilings are unchanged) and the ABSOLUTE numbers are this table. `tests-rs/tests/budget_runtime.rs` asserts the recorded runtime numbers; `crates/evaluator/src/compute.rs` turns them into the limit a transaction must request (`handover/backend-compute-contract.md`).
";

/// The markdown of the measurement.
pub fn to_markdown(j: &Value) -> String {
    let mut s = String::new();
    s.push_str("# Runtime compute table (P27)\n\nGenerated by `tools/devnet-rehearsal measure` (`scripts/devnet-rehearsal.sh selftest measure`) on a local validator, rendered by `tools/devnet-rehearsal render-runtime`; do not edit by hand. `handover/cu-runtime.json` is the machine-readable twin that `crates/evaluator` and `tests-rs/tests/budget_runtime.rs` test against.\n\n");
    s.push_str(&format!("* validator: `{}`\n", j["validator"]["version"].as_str().unwrap_or("?")));
    s.push_str(&format!("* features: {} active, {} inactive on the cluster the measurement ran on", j["validator"]["features_active"], j["validator"]["features_inactive"]));
    if let Some(a) = j["validator"]["inactive_features"].as_array() {
        let names: Vec<String> = a.iter().filter_map(|x| x.as_str()).map(|x| x.split('|').next_back().unwrap_or("").trim().to_string()).collect();
        if !names.is_empty() {
            s.push_str(&format!(" (inactive: {})", names.join("; ")));
        }
    }
    s.push('\n');
    s.push_str(&format!("* program: sha256 `{}` (a selftest build: only the minimum pass delay, the activity throttle, the cancel delay and the retention are shortened)\n", j["program"]["sha256"].as_str().unwrap_or("?")));
    s.push_str(&format!("* every transaction carried `SetComputeUnitLimit({})`; `tx_bytes` includes that instruction (40 bytes); a `SetComputeUnitPrice` adds {} more\n", j["limit_requested"], BUDGET_PRICE_BYTES));
    s.push_str(&format!("* {} runs per shape, the maximum is kept (init_config is a singleton: one run)\n", j["runs_per_shape"]));
    s.push_str("* proofs are mock proofs (source 255, the localnet build). The source changes the compute a little (LiteSVM shows up to 8% between sources 2, 3 and 4 for one shape: events=4, proofs=3, 72,920 / 74,438 / 78,446 CU, inside the 20% margin; to be re-measured with the real sources on devnet) and the BYTES: a source-3 proof carries a 32-byte digest more, so the transaction sizes in this table are a lower bound for the real sources; the evaluator computes the exact size from the batch it builds\n");
    s.push_str("* `tx_format` `v0_lookup`: a version-0 transaction with an address lookup table (`handover/backend-compute-contract.md`), used where a legacy transaction would exceed 1,232 bytes\n\n");
    // the summary per instruction
    s.push_str("## Per instruction: the worst case on the validator against LiteSVM\n\n| instruction | runtime max CU | tx bytes | LiteSVM worst CU (budget.rs) | runtime / LiteSVM | LiteSVM ceiling | over the LiteSVM ceiling | explicit limit |\n|---|---:|---:|---:|---:|---:|---|---|\n");
    let mut per: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    if let Some(a) = j["shapes"].as_array() {
        for x in a {
            let e = per.entry(x["instruction"].as_str().unwrap_or("").to_string()).or_default();
            e.0 = e.0.max(x["max_cu"].as_u64().unwrap_or(0));
            e.1 = e.1.max(x["tx_bytes"].as_u64().unwrap_or(0));
        }
    }
    for (name, (cu, bytes)) in &per {
        let lite = crate::measure::LITESVM_WORST.iter().find(|x| x.0 == name).map(|x| x.1);
        let ceil = crate::measure::CEILINGS.iter().find(|x| x.0 == name).map(|x| x.1);
        let ratio = lite.map(|l| format!("{:.2}", *cu as f64 / l as f64)).unwrap_or_else(|| "-".into());
        let over = match ceil {
            Some(c) if *cu > c => "OVER",
            Some(_) => "no",
            None => "-",
        };
        let limit = if name == "post_journal" || name == "report_fail" {
            "per shape: `recommended_cu_limit`".to_string()
        } else if KEY_DEPENDENT.contains(&name.as_str()) {
            format!("{KEY_DEPENDENT_LIMIT} (depends on the trader's key)")
        } else if *cu * 10 > 200_000 * 7 {
            evaluator::compute::limit_for(*cu).to_string()
        } else {
            "default (200,000) is enough".to_string()
        };
        s.push_str(&format!("| {name} | {cu} | {bytes} | {} | {ratio} | {} | {over} | {limit} |\n", lite.map(|x| x.to_string()).unwrap_or_else(|| "-".into()), ceil.map(|x| x.to_string()).unwrap_or_else(|| "-".into())));
    }
    s.push('\n');
    s.push_str(WHY_LITESVM);
    s.push_str("\n## Every shape\n\n| instruction | shape | tx format | runs (CU) | max CU | tx bytes |\n|---|---|---|---|---:|---:|\n");
    if let Some(a) = j["shapes"].as_array() {
        for x in a {
            let runs = x["runs"].as_array().map(|r| r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            s.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n", x["instruction"].as_str().unwrap_or(""), x["shape"].as_str().unwrap_or(""), x["params"]["tx_format"].as_str().unwrap_or("legacy"), runs, x["max_cu"], x["tx_bytes"]));
        }
    }
    if let Some(u) = j["unsendable"].as_array() {
        if !u.is_empty() {
            s.push_str("\nShapes the batch format allows that no transaction can carry (more than 1,232 bytes even as a lookup-table transaction):\n\n| shape | tx bytes |\n|---|---:|\n");
            for x in u {
                s.push_str(&format!("| {} | {} |\n", x["shape"].as_str().unwrap_or(""), x["tx_bytes"]));
            }
        }
    }
    if let Some(e) = j["errors"].as_array() {
        for x in e {
            s.push_str(&format!("\nINCOMPLETE: section {} failed: {}\n", x["section"], x["error"]));
        }
    }
    s
}

/// Sum over the program-derived addresses of `open_account` / `reset_account` of the failed bump attempts for `trader`: the key that
/// makes the bump searches longest costs the most compute.
pub fn bump_score(ids: &ix::Ids, trader: &Pubkey) -> u32 {
    let mut score = 0u32;
    for ch in 0..2u64 {
        score += 255 - u32::from(Pubkey::find_program_address(&[b"acct", trader.as_ref(), &ch.to_le_bytes()], &ids.lev).1);
        score += 255 - u32::from(Pubkey::find_program_address(&[b"trader_state", ids.lev.as_ref(), trader.as_ref(), &ch.to_le_bytes()], &ids.vault).1);
    }
    score += 255 - u32::from(Pubkey::find_program_address(&[b"wallet", trader.as_ref()], &ids.lev).1);
    score
}

/// A deterministic key from a number (a measurement key, not a secret: the seed is `n` as four little-endian bytes then 0x5A).
pub fn seeded_key(n: u32) -> Keypair {
    let mut bytes = [0x5Au8; 32];
    bytes[..4].copy_from_slice(&n.to_le_bytes());
    Keypair::new_from_array(bytes)
}

/// The seed in `0..n` whose key has the longest bump searches for this pair of program ids.
pub fn worst_seed(ids: &ix::Ids, n: u32) -> (u32, u32) {
    (0..n).map(|i| (i, bump_score(ids, &seeded_key(i).pubkey()))).max_by_key(|x| x.1).unwrap_or((0, 0))
}

/// The made-up batches: base mids of the three markets, their spreads, and the proofs/events built from them.
struct Bld {
    spread: [u32; 3],
    feed: [[u8; 32]; 3],
    qty_min: [u64; 3],
    qty_step: [u64; 3],
}

const MIDS: [u64; 3] = [60_000 * 1_000_000_000_000, 2_600 * 1_000_000_000_000, 150 * 1_000_000_000_000];

impl Bld {
    fn new(rs: &RuleSet, mk: &[Market; 3]) -> Bld {
        Bld {
            spread: [u32::from(rs.spread_e6[0]), u32::from(rs.spread_e6[1]), u32::from(rs.spread_e6[2])],
            feed: [mk[0].feed_id, mk[1].feed_id, mk[2].feed_id],
            qty_min: [mk[0].qty_min, mk[1].qty_min, mk[2].qty_min],
            qty_step: [mk[0].qty_step, mk[1].qty_step, mk[2].qty_step],
        }
    }

    fn proof(&self, m: usize, mid: u64, t: i64) -> PriceProof {
        PriceProof { source: 255, feed_id: self.feed[m], timestamp_ns: u64::try_from(t).unwrap_or(0) * 1_000_000_000, quantized_value: i128::from(mid) * 1_000_000, signed_update: vec![] }
    }

    fn ask(&self, m: usize, mid: u64) -> u64 {
        engine::ask_from_mid(mid, self.spread[m]).unwrap_or(mid)
    }

    fn bid(&self, m: usize, mid: u64) -> u64 {
        engine::bid_from_mid(mid, self.spread[m]).unwrap_or(mid)
    }

    fn qty(&self, m: usize, k: u64) -> u64 {
        self.qty_min[m] * k
    }

    fn fill(&self, m: usize, t: i64, proof: u8) -> JournalEvent {
        let ask = self.ask(m, MIDS[m]);
        JournalEvent::Fill { market: m as u16, side: Side::Long, order_kind: OrderKind::Market, qty: self.qty(m, 10), price_e12: ask, time: t, avg_entry_after: ask, margin: 0, sl: 0, tp: 0, intent_id: 1, proof }
    }

    /// A close of `qty` units entered at the ask of the base mid and exited at the bid of `exit_mid` (the proof's price).
    #[allow(clippy::too_many_arguments)]
    fn close(&self, m: usize, qty: u64, exit_mid: u64, t: i64, proof: u8, reason: CloseReason, quick: bool) -> (JournalEvent, i64, i64) {
        let entry = self.ask(m, MIDS[m]);
        let exit = self.bid(m, exit_mid);
        let pnl = engine::pnl_micro(true, qty, entry, exit).unwrap_or(0);
        let (credited, _) = engine::slice_credit(pnl, quick);
        let timer = if quick { t - 10 } else { t - 100 };
        (JournalEvent::Close { market: m as u16, side: Side::Long, qty, entry_e12: entry, exit_e12: exit, time: t, timer_start: timer, pnl, quick, credited, reason, intent_id: 1, proof }, credited, pnl)
    }

    fn fee(&self, t: i64) -> JournalEvent {
        JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: 0, time: t }
    }

    fn meta(&self, t: i64, flat: bool) -> JournalEvent {
        JournalEvent::Meta { time: t, flat, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION }
    }

    /// A quantity whose close from the base mid to `exit_mid` makes `target_pnl` micro-USD (positive for a rise).
    fn qty_for_pnl(&self, m: usize, exit_mid: u64, target_pnl: i64) -> u64 {
        let entry = i128::from(self.ask(m, MIDS[m]));
        let exit = i128::from(self.bid(m, exit_mid));
        let diff = (exit - entry).abs().max(1);
        let raw = i128::from(target_pnl).abs() * 1_000_000_000_000 / diff;
        let step = i128::from(self.qty_step[m]);
        u64::try_from((raw / step + 1) * step).unwrap_or(u64::MAX).max(self.qty_min[m])
    }
}

fn markets_of(events: &[JournalEvent], proofs: &[PriceProof], feeds: &[[u8; 32]; 3]) -> Vec<u16> {
    let mut v: Vec<u16> = Vec::new();
    for e in events {
        match e {
            JournalEvent::Fill { market, .. } | JournalEvent::Close { market, .. } | JournalEvent::Fee { market, .. } => v.push(*market),
            _ => {}
        }
    }
    for p in proofs {
        if let Some(i) = feeds.iter().position(|f| *f == p.feed_id) {
            v.push(i as u16);
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Bytes of the signed `report_fail` transaction (with the limit instruction) of `k` closes, `f` fees, the Fail and the Meta over `p` proofs.
fn fail_tx_size(b: &Bld, p: usize, k: usize, f: usize) -> usize {
    let (events, proofs) = fail_events(b, 0, p, k, f, 0, FailReason::StaticFloor, false);
    let batch = JournalBatch { first_seq: 1, prev_hash: [0; 32], events, proofs };
    let markets = markets_of(&batch.events, &batch.proofs, &b.feed).len();
    best_size(evaluator::BatchKind::ReportFail(engine::FailReason::StaticFloor), markets, batch.to_bytes().map(|x| x.len()).unwrap_or(0), false).0
}

/// The size of a batch's transaction: the legacy one when it fits a packet, else the lookup-table one (decision 71), and which it is.
fn best_size(kind: evaluator::BatchKind, markets: usize, len: usize, price: bool) -> (usize, evaluator::TxFormat) {
    let legacy = evaluator::compute::tx_size_with_budget(kind, markets, len, evaluator::TxFormat::Legacy, price);
    if legacy <= 1_232 {
        (legacy, evaluator::TxFormat::Legacy)
    } else {
        (evaluator::compute::tx_size_with_budget(kind, markets, len, evaluator::TxFormat::V0Lookup, price), evaluator::TxFormat::V0Lookup)
    }
}

/// The events and proofs of a failing batch: `k` closes round-robin over `p` markets, `f` fees, the Fail (`bal` is b after the closes), the Meta.
#[allow(clippy::too_many_arguments)]
fn fail_events(b: &Bld, t: i64, p: usize, k: usize, f: usize, bal: i64, reason: FailReason, _quick: bool) -> (Vec<JournalEvent>, Vec<PriceProof>) {
    let proofs: Vec<PriceProof> = (0..p).map(|i| b.proof(i, MIDS[i], t)).collect();
    let mut events = Vec::new();
    for j in 0..k {
        let mk = j % p;
        events.push(b.close(mk, b.qty(mk, 10), MIDS[mk], t, u8::try_from(mk).unwrap_or(0), CloseReason::Manual, false).0);
    }
    for _ in 0..f {
        events.push(b.fee(t));
    }
    events.push(JournalEvent::Fail { time: t, reason, b_after: bal, proof: 0 });
    events.push(b.meta(t, true));
    (events, proofs)
}

impl Rehearsal {
    fn clock_now(&self) -> Result<i64, String> {
        self.c.refresh_clock().map_err(|e| e.to_string())
    }

    fn bld(&self) -> Result<(Bld, RuleSet, [Market; 3]), String> {
        let rs = self.ruleset(self.config()?.current_ruleset.max(1))?;
        let mk = [self.market(0)?, self.market(1)?, self.market(2)?];
        Ok((Bld::new(&rs, &mk), rs, mk))
    }

    /// Lands a journal batch with the explicit limit; `fail` = `report_fail` with that reason. Returns the transaction.
    #[allow(clippy::too_many_arguments)]
    fn land(&mut self, a: &AcctRef, label: &str, events: Vec<JournalEvent>, proofs: Vec<PriceProof>, feeds: &[[u8; 32]; 3], fail: Option<FailReason>, price: bool) -> Result<Sent, String> {
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        let batch = JournalBatch { first_seq: acct.seq + 1, prev_hash: acct.journal_hash, events, proofs };
        let markets = markets_of(&batch.events, &batch.proofs, feeds);
        let bytes = batch.to_bytes().map_err(|e| format!("{e:?}"))?;
        let ev = self.keys.pubkey("evaluator");
        let main = match fail {
            None => ix::post_journal(&self.ids, &ev, &a.trader, a.challenge_id, acct.ruleset_version, bytes, &markets),
            Some(r) => ix::report_fail(&self.ids, &ev, &a.trader, a.challenge_id, acct.ruleset_version, bytes, r as u8, &markets),
        };
        let mut ixs = vec![ix::compute_budget_limit(LIMIT)];
        if price {
            ixs.push(ix::compute_budget_price(1));
        }
        ixs.push(main);
        let kind = if fail.is_some() { evaluator::BatchKind::ReportFail(engine::FailReason::StaticFloor) } else { evaluator::BatchKind::PostJournal };
        let (size, format) = best_size(kind, markets.len(), batch.to_bytes().map(|x| x.len()).unwrap_or(0), price);
        if size > 1_232 {
            return Err(format!("TOOBIG {size}"));
        }
        let t = batch.events.last().map(|e| e.time()).unwrap_or(0);
        self.c.wait_until_block_time(t).map_err(|e| e.to_string())?;
        if format == evaluator::TxFormat::V0Lookup {
            let table = self.lookup_table()?;
            let payer = clone_key(self.keys.get("evaluator"));
            let sent = self.send_kp_tables(label, &ixs, &payer, &[], false, &[table]).map_err(|e| e.text)?;
            // the model's size is the wire size, to the byte
            if sent.size != size {
                return Err(format!("{label}: the lookup-table transaction is {} bytes on the wire, the model says {size}", sent.size));
            }
            return Ok(sent);
        }
        self.send(label, &ixs, "evaluator", &[], false).map_err(|e| e.text)
    }

    /// The address lookup table of the shared accounts (config, the current rule set, the event authority, the sector authority, the vault
    /// program, the product registry, markets 0 to 2), created on first use and waited for until it is usable.
    fn lookup_table(&mut self) -> Result<solana_message::AddressLookupTableAccount, String> {
        use solana_address_lookup_table_interface::{instruction::{create_lookup_table, extend_lookup_table}, state::AddressLookupTable};
        if let Some(a) = &self.alt {
            return Ok(a.clone());
        }
        let keeper = self.keys.pubkey("keeper");
        let slot = self.c.finalized_slot().map_err(|e| e.to_string())?;
        let (create, table) = create_lookup_table(keeper, keeper, slot);
        self.send("lookup table: create", &[create], "keeper", &[], false).map_err(|e| e.text)?;
        let cfg = self.config()?;
        let mut addrs = vec![self.ids.config(), self.ids.ruleset(cfg.current_ruleset), self.ids.event_authority(), self.ids.sector_authority(), self.ids.vault, self.ids.registry()];
        addrs.extend((0..3u16).map(|i| self.ids.market(i)));
        let extend = extend_lookup_table(table, keeper, Some(keeper), addrs);
        self.send("lookup table: extend", &[extend], "keeper", &[], false).map_err(|e| e.text)?;
        // the addresses are usable from the slot after the extension
        let at = self.c.confirmed_slot().map_err(|e| e.to_string())?;
        while self.c.confirmed_slot().map_err(|e| e.to_string())? <= at + 2 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let data = self.c.account(&table).map_err(|e| e.to_string())?.ok_or("the lookup table does not exist")?;
        let parsed = AddressLookupTable::deserialize(&data.data).map_err(|e| format!("lookup table: {e}"))?;
        let acc = solana_message::AddressLookupTableAccount { key: table, addresses: parsed.addresses.to_vec() };
        self.alt = Some(acc.clone());
        Ok(acc)
    }

    /// A junk account for a named trader (mock proofs; the program checks claims, not a position book).
    fn junk(&mut self, name: &str, trader: &str) -> Result<AcctRef, String> {
        self.open(name, trader, true)
    }

    fn fund_extra(&mut self, kp: &Keypair, usdc: u64) -> Result<(), String> {
        let keeper = self.keys.pubkey("keeper");
        let usdc_mint = self.usdc();
        let ata = ix::ata(&kp.pubkey(), &usdc_mint);
        let ixs = vec![
            system_instruction::transfer(&keeper, &kp.pubkey(), 3_000_000_000),
            ix::create_ata_idempotent(&keeper, &kp.pubkey(), &usdc_mint),
            ix::mint_to(&usdc_mint, &ata, &keeper, usdc),
        ];
        self.send("fund extra", &ixs, "keeper", &[], false).map_err(|e| e.text)?;
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ the batch grids

    /// post_journal: events 1..=8 (the Meta included) x proofs 0..=3, in two variants (all proof-bearing events; as many fees as possible).
    fn grid_post(&mut self, m: &mut Meas, a: &AcctRef) -> Result<(), String> {
        let (b, _, _) = self.bld()?;
        let mut cases: Vec<(usize, usize, bool)> = Vec::new();
        for n in 1..=8usize {
            for p in 0..=3usize {
                if p + 1 > n && p > 0 {
                    continue;
                }
                cases.push((n, p, false));
                if p < n - 1 {
                    cases.push((n, p, true));
                }
            }
        }
        for (n, p, fees_variant) in cases {
            let shape = format!("events={n} proofs={p} {}", if p == 0 { "fees" } else if fees_variant { "fees+fills/closes" } else { "fills/closes" });
            for _ in 0..RUNS {
                let t = self.clock_now()?.max(self.trading_account(&a.trader, a.challenge_id)?.last_batch_time);
                let proofs: Vec<PriceProof> = (0..p).map(|i| b.proof(i, MIDS[i], t)).collect();
                let mut events = Vec::new();
                let body = n - 1;
                let mut fees = 0;
                for j in 0..body {
                    if p == 0 || (fees_variant && j >= p) {
                        events.push(b.fee(t));
                        fees += 1;
                    } else if j % 2 == 0 {
                        events.push(b.fill(j % p, t, u8::try_from(j % p).unwrap_or(0)));
                    } else {
                        events.push(b.close(j % p, b.qty(j % p, 10), MIDS[j % p], t, u8::try_from(j % p).unwrap_or(0), CloseReason::Manual, false).0);
                    }
                }
                events.push(b.meta(t, false));
                let sent = match self.land(a, &format!("post_journal ev={n} pr={p} grid"), events, proofs, &b.feed, None, false) {
                    Ok(s) => s,
                    Err(e) if e.starts_with("TOOBIG") => {
                        m.unsendable.push((format!("post_journal {shape}"), e[7..].parse().unwrap_or(0)));
                        break;
                    }
                    Err(e) => return Err(e),
                };
                m.add("post_journal", &shape, json!({"events": n, "proofs": p, "fee_events": fees, "variant": if fees_variant { "fees" } else { "proof-bearing" }, "limit": LIMIT}), &sent);
            }
        }
        // the budget test's heaviest mix: two fills, two closes, a tight start, a floor raise, a fee, the meta (3 proofs)
        for run in 0..RUNS {
            // a fresh account each run: the batch changes the junk account's state
            let a = &self.junk(&format!("MX{run}"), "trader6")?;
            let t = self.clock_now()?.max(self.trading_account(&a.trader, a.challenge_id)?.last_batch_time);
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let proofs: Vec<PriceProof> = (0..3).map(|i| b.proof(i, MIDS[i], t)).collect();
            let c1 = b.close(0, b.qty(0, 10), MIDS[0], t, 0, CloseReason::Manual, false);
            let c2 = b.close(1, b.qty(1, 10), MIDS[1], t, 1, CloseReason::Manual, false);
            let r = acct.b + c1.1 + c2.1;
            let size = i64::try_from(acct.size_s).unwrap_or(0);
            let l_start = (size / 100 * 92 + size / 100 * 4).min(r);
            let g = size / 100 + 1_000;
            let events = vec![
                b.fill(0, t, 0),
                b.fill(1, t, 1),
                c1.0,
                c2.0,
                JournalEvent::TightStart { time: t, p_open: size / 100 + 1_000, r, l_start, g_peak: g, l_level: l_start + (g - size / 100).max(0), proof: 0 },
                JournalEvent::FloorRaised { time: t, g_peak: g + 1_000, l_level: l_start + (g + 1_000 - size / 100).max(0), proof: 2 },
                b.fee(t),
                b.meta(t, false),
            ];
            match self.land(a, "post_journal ev=8 pr=3 mix", events, proofs, &b.feed, None, false) {
                Ok(sent) => m.add("post_journal", "events=8 proofs=3 mix (2 fills, 2 closes, tight start, floor raise, fee)", json!({"events": 8, "proofs": 3, "fee_events": 1, "variant": "mix", "limit": LIMIT}), &sent),
                Err(e) => {
                    self.say(&format!("  mix shape skipped: {e}"));
                    break;
                }
            }
        }
        Ok(())
    }

    /// One junk account taken to `b` below the static floor (flat claimed false), ready for a failing batch.
    fn broke_account(&mut self, name: &str, trader: &str, b: &Bld) -> Result<AcctRef, String> {
        let a = self.junk(name, trader)?;
        let t = self.clock_now()?;
        let proofs = vec![b.proof(0, MIDS[0] / 10 * 3, t)];
        let qty = b.qty_for_pnl(0, MIDS[0] / 10 * 3, 225_000_000);
        let (close, _, _) = b.close(0, qty, MIDS[0] / 10 * 3, t, 0, CloseReason::Manual, false);
        let events = vec![close, b.meta(t, false)];
        self.land(&a, &format!("post_journal ev=2 pr=1 broke {name}"), events, proofs, &b.feed, None, false)?;
        Ok(a)
    }

    /// report_fail (static floor): `k` closes, `f` fees, the Fail, the Meta.
    fn grid_fail(&mut self, m: &mut Meas) -> Result<(), String> {
        let (b, _, _) = self.bld()?;
        let traders = ["trader5", "trader6", "trader7"];
        let mut counter = 0usize;
        let mut cases: Vec<(usize, usize, usize, usize)> = Vec::new(); // (events, proofs, closes, fees)
        for n in 2..=8usize {
            for p in 1..=3usize {
                let k_max = n - 2;
                if k_max == 0 {
                    if p == 1 {
                        cases.push((n, p, 0, 0));
                    }
                    continue;
                }
                if p > k_max {
                    continue;
                }
                // the variant with the most closes that a transaction can carry, and the one with the fewest (the rest fees)
                let mut placed = Vec::new();
                for k in (p..=k_max).rev() {
                    if fail_tx_size(&b, p, k, k_max - k) <= 1_232 {
                        placed.push(k);
                        break;
                    }
                }
                if placed.is_empty() {
                    m.unsendable.push((format!("static floor events={n} proofs={p} closes>={p}"), fail_tx_size(&b, p, p, k_max - p)));
                    continue;
                }
                if !placed.contains(&p) && fail_tx_size(&b, p, p, k_max - p) <= 1_232 {
                    placed.push(p);
                }
                for k in placed {
                    cases.push((n, p, k, k_max - k));
                }
                if fail_tx_size(&b, p, k_max, 0) > 1_232 {
                    m.unsendable.push((format!("static floor events={n} proofs={p} closes={k_max} fees=0"), fail_tx_size(&b, p, k_max, 0)));
                }
            }
        }
        for (n, p, k, f) in cases {
            let shape = format!("static floor events={n} proofs={p} closes={k} fees={f}");
            for _ in 0..RUNS {
                counter += 1;
                let name = format!("MF{counter}");
                let a = self.broke_account(&name, traders[counter % 3], &b)?;
                let acct = self.trading_account(&a.trader, a.challenge_id)?;
                let t = self.clock_now()?.max(acct.last_batch_time);
                let proofs: Vec<PriceProof> = (0..p).map(|i| b.proof(i, MIDS[i], t)).collect();
                let mut events = Vec::new();
                let mut bal = acct.b;
                for j in 0..k {
                    let mk = j % p;
                    let (c, credited, _) = b.close(mk, b.qty(mk, 10), MIDS[mk], t, u8::try_from(mk).unwrap_or(0), CloseReason::Manual, false);
                    bal += credited;
                    events.push(c);
                }
                for _ in 0..f {
                    events.push(b.fee(t));
                }
                events.push(JournalEvent::Fail { time: t, reason: FailReason::StaticFloor, b_after: bal, proof: 0 });
                events.push(b.meta(t, true));
                let sent = self.land(&a, &format!("report_fail ev={n} pr={p} grid"), events, proofs, &b.feed, Some(FailReason::StaticFloor), false)?;
                m.add("report_fail", &shape, json!({"events": n, "proofs": p, "closes": k, "fee_events": f, "reason": "static floor", "limit": LIMIT}), &sent);
            }
        }
        Ok(())
    }

    /// report_fail by the quick limit with forced closes after the overshoot (decision 69): fees first, the crossing close, then `Close{Fail}`s.
    fn grid_quick(&mut self, m: &mut Meas) -> Result<(), String> {
        let (b, _, _) = self.bld()?;
        let traders = ["trader5", "trader6", "trader7"];
        let mut counter = 0usize;
        // (closes, proofs, fees): the 8-event commission case, the 3-position forced close-all, and the extreme six closes
        for (k, p, f) in [(2usize, 2usize, 0usize), (3, 3, 0), (3, 3, 3), (6, 3, 0)] {
            let shape = format!("quick limit forced close-all closes={k} proofs={p} fees={f}");
            for _ in 0..RUNS {
                counter += 1;
                let name = format!("MQ{counter}");
                let a = self.junk(&name, traders[counter % 3])?;
                let acct = self.trading_account(&a.trader, a.challenge_id)?;
                let t = self.clock_now()?.max(acct.last_batch_time);
                let hi = [MIDS[0] / 100 * 102, MIDS[1] / 100 * 102, MIDS[2] / 100 * 102];
                let proofs: Vec<PriceProof> = (0..p).map(|i| b.proof(i, hi[i], t)).collect();
                let mut events = Vec::new();
                let mut bal = acct.b;
                for _ in 0..f {
                    events.push(b.fee(t));
                }
                // the crossing close: a quick profit of $160 (credit $80 > 0.028 S = $70)
                let q0 = b.qty_for_pnl(0, hi[0], 160_000_000);
                let (c0, cr0, _) = b.close(0, q0, hi[0], t, 0, CloseReason::Manual, true);
                bal += cr0;
                events.push(c0);
                for j in 1..k {
                    let mk = j % p;
                    let (c, cr, _) = b.close(mk, b.qty(mk, 10), hi[mk], t, u8::try_from(mk).unwrap_or(0), CloseReason::Fail, true);
                    bal += cr;
                    events.push(c);
                }
                events.push(JournalEvent::Fail { time: t, reason: FailReason::PhaseQuick, b_after: bal, proof: 0 });
                events.push(b.meta(t, true));
                let n = events.len();
                let sent = match self.land(&a, &format!("report_fail ev={n} pr={p} quick"), events, proofs, &b.feed, Some(FailReason::PhaseQuick), false) {
                    Ok(s) => s,
                    Err(e) if e.starts_with("TOOBIG") => {
                        m.unsendable.push((shape.clone(), e[7..].parse().unwrap_or(0)));
                        break;
                    }
                    Err(e) => return Err(e),
                };
                m.add("report_fail", &shape, json!({"events": n, "proofs": p, "closes": k, "fee_events": f, "reason": "quick limit (decision 69)", "limit": LIMIT}), &sent);
            }
        }
        // the heaviest transaction with a priority fee as well (its size is what matters; the compute is that of the same shape without)
        for run in 0..RUNS {
            let name = format!("MQ{}p{run}", counter + 1);
            let a = self.broke_account(&name, "trader5", &b)?;
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let t = self.clock_now()?.max(acct.last_batch_time);
            let proofs: Vec<PriceProof> = (0..3).map(|i| b.proof(i, MIDS[i], t)).collect();
            let mut events = Vec::new();
            let mut bal = acct.b;
            for (j, mid) in MIDS.iter().enumerate() {
                let (c, cr, _) = b.close(j, b.qty(j, 10), *mid, t, u8::try_from(j).unwrap_or(0), CloseReason::Manual, false);
                bal += cr;
                events.push(c);
            }
            for _ in 0..3 {
                events.push(b.fee(t));
            }
            events.push(JournalEvent::Fail { time: t, reason: FailReason::StaticFloor, b_after: bal, proof: 0 });
            events.push(b.meta(t, true));
            let sent = self.land(&a, "report_fail ev=8 pr=3 with price", events, proofs, &b.feed, Some(FailReason::StaticFloor), true)?;
            m.add("report_fail", "static floor events=8 proofs=3 closes=3 fees=3 WITH SetComputeUnitPrice", json!({"events": 8, "proofs": 3, "closes": 3, "fee_events": 3, "price_instruction": true, "limit": LIMIT}), &sent);
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ the other instructions

    /// A gain batch: a close whose profit lifts the balance above 1.08 S (not quick), flat.
    fn gain(&mut self, a: &AcctRef, b: &Bld, label: &str) -> Result<(), String> {
        let t = self.clock_now()?.max(self.trading_account(&a.trader, a.challenge_id)?.last_batch_time);
        let hi = MIDS[0] / 100 * 110;
        let qty = b.qty_for_pnl(0, hi, 225_000_000);
        let (c, _, _) = b.close(0, qty, hi, t, 0, CloseReason::Manual, false);
        self.land(a, label, vec![c, b.meta(t, true)], vec![b.proof(0, hi, t)], &b.feed, None, false)?;
        Ok(())
    }

    fn rec(&mut self, m: &mut Meas, instruction: &str, shape: &str, params: Value, r: Result<Sent, crate::rehearsal::SendErr>) -> Result<(), String> {
        match r {
            Ok(s) => {
                m.add(instruction, shape, params, &s);
                Ok(())
            }
            Err(e) => Err(format!("{instruction} ({shape}): {}", e.text)),
        }
    }

    /// Pass lifecycles of three junk accounts: request/confirm in phase 1 and 2, veto, finalize, reject, cancel.
    fn lifecycle(&mut self, m: &mut Meas) -> Result<(), String> {
        let (b, _, _) = self.bld()?;
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let ev = self.keys.pubkey("evaluator");
        let vs = self.vault_state();
        let mut accts = Vec::new();
        for i in 0..RUNS {
            let a = self.junk(&format!("ML{}", i + 1), "trader2")?;
            accts.push(a);
        }
        for a in &accts {
            let tn = self.trader_name_of(a)?;
            // phase 1
            self.gain(a, &b, &format!("post_journal gain {}", a.name))?;
            let r = self.send(&format!("request_advance {}", a.name), &[ix::request_advance(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false);
            self.rec(m, "request_advance", "phase 1", json!({"phase": 1}), r)?;
            let r = self.send(&format!("confirm_pass {}", a.name), &[ix::confirm_pass(&self.ids, &ev, &a.trader, a.challenge_id)], "evaluator", &[], false);
            self.rec(m, "confirm_pass", "phase 1 (starts phase 2 at once)", json!({"phase": 1}), r)?;
            // phase 2: request, confirm, veto
            self.gain(a, &b, &format!("post_journal gain {}", a.name))?;
            let r = self.send(&format!("request_advance {}", a.name), &[ix::request_advance(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false);
            self.rec(m, "request_advance", "phase 2", json!({"phase": 2}), r)?;
            let r = self.send(&format!("confirm_pass {}", a.name), &[ix::confirm_pass(&self.ids, &ev, &a.trader, a.challenge_id)], "evaluator", &[], false);
            self.rec(m, "confirm_pass", "phase 2 to 5 (pending)", json!({"phase": 2}), r)?;
            let r = self.send(&format!("veto_pass {}", a.name), &[ix::veto_pass(&self.ids, &x, &y, &a.trader, a.challenge_id)], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false);
            self.rec(m, "veto_pass", "confirmed pass", json!({}), r)?;
            // again, then finalize after the delay
            self.send(&format!("request_advance {}", a.name), &[ix::request_advance(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false).map_err(|e| e.text)?;
            self.send(&format!("confirm_pass {}", a.name), &[ix::confirm_pass(&self.ids, &ev, &a.trader, a.challenge_id)], "evaluator", &[], false).map_err(|e| e.text)?;
        }
        let wait_to = self.clock_now()? + i64::from(self.pass_delay) + 2;
        self.c.wait_until_block_time(wait_to).map_err(|e| e.to_string())?;
        for a in &accts {
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let ixn = ix::finalize_pass(&self.ids, &self.keys.pubkey("keeper"), &a.trader, a.challenge_id, acct.next_request_id, &vs);
            let r = self.send(&format!("finalize_pass {}", a.name), &[ixn], "keeper", &[], false);
            self.rec(m, "finalize_pass", "phase 2 -> 3 (payout claim created, vault CPI)", json!({"phase": 2}), r)?;
        }
        // phase 3: reject, cancel
        for a in &accts {
            let tn = self.trader_name_of(a)?;
            self.gain(a, &b, &format!("post_journal gain {}", a.name))?;
            self.send(&format!("request_advance {}", a.name), &[ix::request_advance(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false).map_err(|e| e.text)?;
            let r = self.send(&format!("reject_pass {}", a.name), &[ix::reject_pass(&self.ids, &ev, &a.trader, a.challenge_id, 0)], "evaluator", &[], false);
            self.rec(m, "reject_pass", "NotFlat", json!({}), r)?;
            self.send(&format!("request_advance {}", a.name), &[ix::request_advance(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false).map_err(|e| e.text)?;
        }
        self.c.wait_until_block_time(self.clock_now()? + 7).map_err(|e| e.to_string())?;
        for a in &accts {
            let tn = self.trader_name_of(a)?;
            let r = self.send(&format!("cancel_pass {}", a.name), &[ix::cancel_pass(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false);
            self.rec(m, "cancel_pass", "after the (selftest) delay", json!({}), r)?;
        }
        Ok(())
    }

    /// hold, release, hold again, fail_for_violation; touch_activity (throttled, then the vault call) and release_account.
    fn holds_and_touches(&mut self, m: &mut Meas) -> Result<(), String> {
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let ev = self.keys.pubkey("evaluator");
        let mut v = Vec::new();
        let mut t = Vec::new();
        for i in 0..RUNS {
            v.push(self.junk(&format!("MV{}", i + 1), "trader3")?);
            t.push(self.junk(&format!("MT{}", i + 1), "trader4")?);
        }
        for a in &t {
            let r = self.send(&format!("touch_activity {}", a.name), &[ix::touch_activity(&self.ids, &ev, &a.trader, a.challenge_id)], "evaluator", &[], false);
            self.rec(m, "touch_activity", "throttled (no vault call)", json!({"throttled": true}), r)?;
        }
        for a in &v {
            let r = self.send(&format!("hold_account {}", a.name), &[ix::hold_account(&self.ids, &ev, &a.trader, a.challenge_id, [0x5A; 32])], "evaluator", &[], false);
            self.rec(m, "hold_account", "fresh account", json!({}), r)?;
            let r = self.send(&format!("release_hold {}", a.name), &[ix::release_hold(&self.ids, &x, &y, &a.trader, a.challenge_id)], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false);
            self.rec(m, "release_hold", "held account", json!({}), r)?;
            self.send(&format!("hold_account {}", a.name), &[ix::hold_account(&self.ids, &ev, &a.trader, a.challenge_id, [0x5A; 32])], "evaluator", &[], false).map_err(|e| e.text)?;
            let r = self.send(&format!("fail_for_violation {}", a.name), &[ix::fail_for_violation(&self.ids, &x, &y, &a.trader, a.challenge_id, [0x77; 32])], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false);
            self.rec(m, "fail_for_violation", "held account (vault flag_trader_failed)", json!({}), r)?;
        }
        self.c.wait_until_block_time(self.clock_now()? + 7).map_err(|e| e.to_string())?;
        for a in &t {
            let r = self.send(&format!("touch_activity {}", a.name), &[ix::touch_activity(&self.ids, &ev, &a.trader, a.challenge_id)], "evaluator", &[], false);
            self.rec(m, "touch_activity", "with the vault call (throttle ran out)", json!({"throttled": false}), r)?;
        }
        self.c.wait_until_block_time(self.clock_now()? + 7).map_err(|e| e.to_string())?;
        for a in &t {
            let tn = self.trader_name_of(a)?;
            let r = self.send(&format!("release_account {}", a.name), &[ix::release_account(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false);
            self.rec(m, "release_account", "with the vault call", json!({}), r)?;
        }
        Ok(())
    }

    /// open_account and reset_account with the key whose bump searches are longest, on the biggest tier.
    fn worst_key_shapes(&mut self, m: &mut Meas) -> Result<(), String> {
        let (seed, score) = worst_seed(&self.ids, 30_000);
        let kp = seeded_key(seed);
        self.say(&format!("  worst key: seed {seed} (bump score {score}, the average is about 6)"));
        self.fund_extra(&kp, 100_000_000_000)?;
        let cfg = self.config()?;
        let (b, _, _) = self.bld()?;
        let usdc = self.usdc();
        let pay = ix::Pay { vault_state: self.vault_state(), trader_token: ix::ata(&kp.pubkey(), &usdc), mint: usdc, pool: self.ids.pool(&self.vault_state(), &usdc), vault_sl8_token: ix::ata(&self.keys.pubkey("vault-admin-sl8"), &usdc), fee_token: ix::ata(&self.fee_wallet(), &usdc) };
        let mut next = self.wallet_profile(&kp.pubkey())?.map_or(0, |w| w.next_challenge_id);
        let tier = 9u8;
        for run in 0..RUNS {
            let ixn = ix::open_account(&self.ids, &kp.pubkey(), cfg.current_ruleset, next, &pay, tier);
            let r = self.send_kp(&format!("open_account worst key {run}"), &[ix::compute_budget_limit(LIMIT), ixn], &kp, &[], false);
            self.rec(m, "open_account", "worst key, tier 9", json!({"seed": seed, "bump_score": score, "tier": 9, "first_account_of_wallet": run == 0}), r)?;
            // fail it: a loss batch, then report_fail; then reset into the next account
            let a = AcctRef { name: format!("MW{run}"), trader: kp.pubkey(), challenge_id: next };
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let t = self.clock_now()?.max(acct.last_batch_time);
            let low = MIDS[0] / 10 * 3;
            let size = i64::try_from(acct.size_s).unwrap_or(0);
            let qty = b.qty_for_pnl(0, low, size / 100 * 9);
            let (c, cr, _) = b.close(0, qty, low, t, 0, CloseReason::Manual, false);
            self.land(&a, "post_journal broke worst", vec![c, b.meta(t, false)], vec![b.proof(0, low, t)], &b.feed, None, false)?;
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let t = self.clock_now()?.max(acct.last_batch_time);
            let _ = cr;
            let events = vec![JournalEvent::Fail { time: t, reason: FailReason::StaticFloor, b_after: acct.b, proof: 0 }, b.meta(t, true)];
            self.land(&a, "report_fail worst", events, vec![b.proof(0, MIDS[0], t)], &b.feed, Some(FailReason::StaticFloor), false)?;
            let ixn = ix::reset_account(&self.ids, &kp.pubkey(), cfg.current_ruleset, next, next + 1, &pay);
            let r = self.send_kp(&format!("reset_account worst key {run}"), &[ix::compute_budget_limit(LIMIT), ixn], &kp, &[], false);
            self.rec(m, "reset_account", "worst key, tier 9, failed in phase 1", json!({"seed": seed, "bump_score": score, "tier": 9, "phase": 1}), r)?;
            // the reset opened the account `next + 1`: the next open takes the id after it
            next += 2;
        }
        Ok(())
    }

    /// Admin instructions: create_market (six sessions), update_market, create_ruleset, set_limits, set_paused, set_evaluator, ban/unban.
    fn admin_shapes(&mut self, m: &mut Meas) -> Result<(), String> {
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let keeper = self.keys.pubkey("keeper");
        let adm = ["lev-admin-sl8", "lev-admin-rov"];
        let cap = [ix::compute_budget_limit(LIMIT)];
        let with = |i: Instruction| -> Vec<Instruction> { cap.iter().cloned().chain(std::iter::once(i)).collect() };
        // create_market with six sessions: ids 22 and up (21 is the attacks' market when S3 ran; ids are sequential)
        let mut next = self.config()?.market_count;
        for _ in 0..RUNS {
            let mut sessions = [Session::default(); 6];
            for (k, s) in sessions.iter_mut().enumerate() {
                *s = Session { start_min: u16::try_from(k * 1_000).unwrap_or(0), end_min: u16::try_from(k * 1_000 + 900).unwrap_or(0) };
            }
            let args = CreateMarketArgs { market_id: next, class: 6, params: MarketParams { status: 1, feed_id: [u8::try_from(next % 200).unwrap_or(1) + 30; 32], max_age_secs: 40, qty_step: 1_000, qty_min: 1_000, max_notional_usd: 1_000_000_000_000, session_count: 6, sessions, quote_conv_market: setl8_lev_trading::constants::NO_QUOTE_CONV } };
            let r = self.send(&format!("create_market {next}"), &with(ix::create_market(&self.ids, &keeper, &x, &y, &args)), "keeper", &adm, false);
            self.rec(m, "create_market", "six sessions", json!({"sessions": 6}), r)?;
            let upd = UpdateMarketArgs { market_id: next, params: args.params.clone() };
            let r = self.send(&format!("update_market {next}"), &with(ix::update_market(&self.ids, &x, &y, &upd)), "keeper", &adm, false);
            self.rec(m, "update_market", "feed remap, six sessions", json!({"sessions": 6}), r)?;
            next += 1;
        }
        let base = self.launch.ruleset.clone();
        let mut version = self.config()?.current_ruleset;
        for _ in 0..RUNS {
            version += 1;
            let rs = RuleSetArgs { version, ..base.clone() };
            let r = self.send(&format!("create_ruleset {version}"), &with(ix::create_ruleset(&self.ids, &keeper, &x, &y, &rs)), "keeper", &adm, false);
            self.rec(m, "create_ruleset", "the 552-byte rule set, 10 tiers", json!({"version": version}), r)?;
        }
        for i in 0..RUNS {
            let r = self.send("set_limits", &with(ix::set_limits(&self.ids, &x, &y, self.pass_delay + i as u32, 100_000_000_000, 86_400)), "keeper", &adm, false);
            self.rec(m, "set_limits", "both admins", json!({}), r)?;
        }
        for _ in 0..RUNS {
            let r = self.send("set_paused true", &with(ix::set_paused(&self.ids, &x, &y, true)), "keeper", &adm, false);
            self.rec(m, "set_paused", "pause", json!({"paused": true}), r)?;
            let r = self.send("set_paused false", &with(ix::set_paused(&self.ids, &x, &y, false)), "keeper", &adm, false);
            self.rec(m, "set_paused", "unpause", json!({"paused": false}), r)?;
        }
        for i in 0..RUNS {
            let victim = Pubkey::new_unique();
            let r = self.send(&format!("ban_wallet {i}"), &with(ix::ban_wallet(&self.ids, &keeper, &x, &y, &victim, [0x44; 32])), "keeper", &adm, false);
            self.rec(m, "ban_wallet", "wallet profile created", json!({}), r)?;
            let r = self.send(&format!("unban_wallet {i}"), &with(ix::unban_wallet(&self.ids, &x, &y, &victim)), "keeper", &adm, false);
            self.rec(m, "unban_wallet", "existing profile", json!({}), r)?;
        }
        // set_evaluator last: rotate to another key and back
        let other = self.keys.pubkey("trader7");
        let ev = self.keys.pubkey("evaluator");
        for _ in 0..RUNS {
            let r = self.send("set_evaluator", &with(ix::set_evaluator(&self.ids, &x, &y, &other)), "keeper", &adm, false);
            self.rec(m, "set_evaluator", "both admins", json!({}), r)?;
            self.send("set_evaluator back", &with(ix::set_evaluator(&self.ids, &x, &y, &ev)), "keeper", &adm, false).map_err(|e| e.text)?;
        }
        Ok(())
    }

    /// close_account on failed accounts of the grids (the selftest retention is 5 s).
    fn closes(&mut self, m: &mut Meas) -> Result<(), String> {
        self.c.wait_until_block_time(self.clock_now()? + 7).map_err(|e| e.to_string())?;
        let failed: Vec<String> = self.st.accounts.keys().filter(|n| n.starts_with("MF") || n.starts_with("MQ")).cloned().collect();
        let mut done = 0;
        for n in failed {
            if done >= RUNS {
                break;
            }
            let a = self.acct_ref(&n)?;
            if self.trading_account(&a.trader, a.challenge_id)?.status != 1 {
                continue;
            }
            let tn = self.trader_name_of(&a)?;
            let r = self.send(&format!("close_account {n}"), &[ix::close_account(&self.ids, &a.trader, a.challenge_id)], &tn, &[], false);
            self.rec(m, "close_account", "failed account after the (selftest) retention", json!({}), r)?;
            done += 1;
        }
        Ok(())
    }

/// The whole measurement.
    pub fn measure_all(&mut self, validator: &Value, program: &Value) -> Result<Meas, String> {
        let mut m = Meas::default();
        // plenty of SOL for the many accounts of a measurement (the local faucet has no limit)
        for n in ["keeper", "evaluator", "trader1", "trader2", "trader3", "trader4", "trader5", "trader6", "trader7"] {
            let _ = self.c.airdrop(&self.keys.pubkey(n), 400_000_000_000);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        self.quiet = true;
        let _ = validator;
        let _ = program;
        self.begin("MEASURE");
        self.s0()?;
        self.s1()?;
        self.quiet = true;
        self.cur = None;
        println!("measure: init_config / create_market / create_ruleset of S1 are measured by the admin shapes below plus the setup transactions");
        let setup = self.st.sigs.clone();
        for s in &setup {
            // the singletons: one run each (init_config, init_vault, register_product)
            if s.ok && matches!(crate::measure::instruction_of(&s.label), "init_config") {
                let sent = Sent { info: crate::cluster::TxInfo { signature: s.sig.clone(), cu: s.cu, ..Default::default() }, size: usize::try_from(s.size).unwrap_or(0) + BUDGET_LIMIT_BYTES, v0: false };
                m.add("init_config", "both admins + payer (singleton: one run; the limit instruction is added to the size)", json!({"singleton": true}), &sent);
            }
        }
        // every section runs on its own: a failure is recorded and the others are kept; MEASURE_ONLY=post,worst limits the run (debugging)
        let only = std::env::var("MEASURE_ONLY").unwrap_or_default();
        let want = |k: &str| only.is_empty() || only.split(',').any(|x| x == k);
        let p1 = self.junk("MP1", "trader1")?;
        let sections: [(&str, &str, SectionFn); 8] = [
            ("post", "post_journal grid", |r, m, p1| r.grid_post(m, p1)),
            ("fail", "report_fail static-floor grid", |r, m, _| r.grid_fail(m)),
            ("quick", "report_fail quick-limit forced close-all shapes", |r, m, _| r.grid_quick(m)),
            ("lifecycle", "pass lifecycle (request, confirm, veto, finalize, reject, cancel)", |r, m, _| r.lifecycle(m)),
            ("holds", "hold, violation, touch_activity, release_account", |r, m, _| r.holds_and_touches(m)),
            ("worst", "open_account and reset_account with the worst key", |r, m, _| r.worst_key_shapes(m)),
            ("closes", "close_account", |r, m, _| r.closes(m)),
            ("admin", "admin instructions", |r, m, _| r.admin_shapes(m)),
        ];
        for (key, title, f) in sections {
            if !want(key) {
                continue;
            }
            println!("measure: {title}");
            if let Err(e) = f(self, &mut m, &p1) {
                println!("measure: SECTION {key} FAILED: {e}");
                m.errors.push((key.to_string(), e));
            }
        }
        let _ = clone_key;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> ix::Ids {
        ix::Ids { lev: Pubkey::new_from_array([1; 32]), vault: Pubkey::new_from_array([2; 32]) }
    }

    #[test]
    fn the_worst_key_search_is_deterministic_and_beats_the_average() {
        let i = ids();
        let (seed, score) = worst_seed(&i, 3_000);
        assert_eq!(worst_seed(&i, 3_000), (seed, score));
        let avg: u32 = (0..200).map(|n| bump_score(&i, &seeded_key(n).pubkey())).sum::<u32>() / 200;
        assert!(score > avg + 5, "worst {score} against an average of {avg}");
        assert_eq!(seeded_key(seed).pubkey(), seeded_key(seed).pubkey());
    }

    #[test]
    fn a_measurement_keeps_every_run_and_reports_the_maximum() {
        let mut m = Meas::default();
        for cu in [100u64, 300, 200] {
            let sent = Sent { info: crate::cluster::TxInfo { signature: format!("s{cu}"), cu, ..Default::default() }, size: 500 + cu as usize, v0: false };
            m.add("post_journal", "events=2 proofs=1", json!({"events": 2}), &sent);
        }
        let j = m.to_json(&json!({"version": "x"}), &json!({}));
        let s = &j["shapes"][0];
        assert_eq!((s["max_cu"].as_u64(), s["tx_bytes"].as_u64(), s["max_signature"].as_str()), (Some(300), Some(800), Some("s300")));
        assert_eq!(s["runs"].as_array().map(|r| r.len()), Some(3));
        assert!(to_markdown(&j).contains("| post_journal | events=2 proofs=1 | legacy | 100, 300, 200 | 300 | 800 |"));
    }

    #[test]
    fn the_budget_instructions_add_the_bytes_the_contract_states() {
        assert_eq!(BUDGET_LIMIT_BYTES, 40);
        assert_eq!(BUDGET_PRICE_BYTES, 12);
        let limit = ix::compute_budget_limit(400_000);
        let price = ix::compute_budget_price(1);
        // instruction bytes: program index 1 + accounts length 1 + data length 1 + data
        assert_eq!((limit.data.len(), price.data.len()), (5, 9));
    }
}
