//! M10 harness: the reference evaluator against the REAL program and vault in LiteSVM.
//!
//! `LiteChain` implements the shared runner's `Chain` over a `JEnv`: it posts every batch as the signed transaction the
//! SPLIT rule promised (evaluator = fee payer, one signature, no budget instruction), compares the stored 207-byte
//! `TradingAccount` with the evaluator's state after EVERY transaction, collects the emitted `EventLog`s, and drives the
//! pass flow (`request_advance`, `confirm_pass`, `reject_pass`, `cancel_pass`, `veto_pass`, `finalize_pass`).
#![allow(dead_code)]

use std::collections::BTreeMap;

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey};
use anchor_lang::{AccountSerialize, InstructionData, ToAccountMetas};
use ::evaluator::testkit::PathSpec;
use ::evaluator::{Batch, BatchKind, ChainView, PassReject};
use journal_types::JournalEvent;
use litesvm::types::{TransactionMetadata, TransactionResult};
use setl8_lev_trading::{
    accounts, constants as c,
    events::{PassConfirmed, PassFinalized, PassRequested, PhaseAdvanced},
    instruction,
    instructions::{CreateMarketArgs, MarketParams},
    state::{Session, TradingAccount},
};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

use super::journal::*;
use super::pass::*;
use super::vault::*;
use super::*;

#[path = "../../../crates/evaluator/tests/common/mod.rs"]
pub mod shared;

use shared::runner::{pass_confirmed_line, pass_finalized_line, pass_requested_line, phase_advanced_line, Chain};
use shared::World;

/// The trader wallet of every path: the keypair whose ed25519 key is the shared harness's `trader_keypair()`.
pub fn trader_wallet() -> Keypair {
    Keypair::new_from_array([0x42; 32])
}

/// Names of the `TradingAccount` fields by byte offset (DESIGN 5.1), to say WHICH field differs.
const FIELDS: [(&str, usize, usize); 28] = [
    ("discriminator", 0, 8), ("trader", 8, 32), ("challenge_id", 40, 8), ("size_s", 48, 8), ("created_at", 56, 8), ("b", 64, 8),
    ("q_tally", 72, 8), ("lock_until", 80, 8), ("last_batch_time", 88, 8), ("last_activity_pushed", 96, 8), ("next_request_id", 104, 8),
    ("seq", 112, 8), ("journal_hash", 120, 32), ("trades_closed", 152, 4), ("ruleset_version", 156, 2), ("status", 158, 1),
    ("phase", 159, 1), ("latch", 160, 1), ("flat", 161, 1), ("pass_pending", 162, 1), ("vetoes", 163, 1), ("bump", 164, 1),
    ("tight_on", 165, 1), ("held", 166, 1), ("r", 167, 8), ("g_peak", 175, 8), ("l_level", 183, 8), ("pass_requested_at", 191, 8),
];

fn account_bytes(a: &TradingAccount) -> Vec<u8> {
    let mut d = Vec::new();
    a.try_serialize(&mut d).expect("serialize");
    d
}

/// `None` when the two accounts are byte for byte equal, else a line naming every field that differs.
pub fn diff_accounts(actual: &TradingAccount, expected: &TradingAccount) -> Option<String> {
    let (a, e) = (account_bytes(actual), account_bytes(expected));
    if a == e {
        return None;
    }
    let mut out = Vec::new();
    for (name, off, w) in FIELDS.iter().chain([("pass_proposed_at", 199usize, 8usize)].iter()) {
        if a[*off..*off + *w] != e[*off..*off + *w] {
            out.push(format!("{name}: chain {:?} != evaluator {:?}", &a[*off..*off + *w], &e[*off..*off + *w]));
        }
    }
    Some(out.join("; "))
}

/// The account after `b` is applied, as the evaluator says it must be: the stored account with the fields the journal decides overwritten.
pub fn expected_after(prev: &TradingAccount, b: &Batch, clock: i64) -> TradingAccount {
    let mut e = prev.clone();
    let a = &b.account_after;
    e.b = a.b;
    e.q_tally = a.q_tally;
    e.lock_until = a.lock_until;
    e.last_batch_time = a.last_batch_time;
    e.seq = a.seq;
    e.journal_hash = a.journal_hash;
    e.trades_closed = a.trades_closed;
    e.latch = u8::from(a.latch);
    e.flat = u8::from(a.flat);
    e.tight_on = u8::from(a.tight_on);
    e.r = a.r;
    e.g_peak = a.g_peak;
    e.l_level = a.l_level;
    if a.failed {
        e.status = 1;
        e.pass_pending = 0;
        e.pass_proposed_at = clock;
    }
    e
}

/// Per batch shape: the largest signed transaction and compute use seen.
#[derive(Default, Clone, Debug)]
pub struct ShapeMax {
    pub count: u64,
    pub max_bytes: usize,
    pub max_cu: u64,
    pub model_bytes_max: usize,
    pub model_cu_max: u64,
}

/// A refused `post_journal` / `report_fail`: the program's error code when it is a custom error.
#[derive(Debug, Clone)]
pub struct PostError {
    pub code: Option<u32>,
    pub text: String,
}

pub struct LiteChain {
    pub j: JEnv,
    pub account: [u8; 32],
    pub params: ::evaluator::AccountParams,
    /// The event log lines after the header and the `opened` line, in file order, built from the program's own events.
    pub lines: Vec<String>,
    pub clock: i64,
    pub slot: u64,
    pub shapes: BTreeMap<String, ShapeMax>,
    pub txs: u64,
    pub opened_line: String,
    pub pass_events: u32,
}

fn market_args(spec: &PathSpec, i: usize) -> CreateMarketArgs {
    let m = &spec.markets[i];
    let mut sessions = [Session::default(); 6];
    for (k, (a, b)) in m.sessions.iter().enumerate() {
        sessions[k] = Session { start_min: *a, end_min: *b };
    }
    CreateMarketArgs {
        market_id: m.id,
        class: m.class,
        params: MarketParams {
            status: 1,
            feed_id: journal_types::feed_id(&journal_types::NativeSha256, m.source, &m.symbol),
            max_age_secs: m.max_age_secs,
            qty_step: 1_000,
            qty_min: 1_000,
            max_notional_usd: 1_000_000_000_000_000,
            session_count: u8::try_from(m.sessions.len()).unwrap(),
            sessions,
            quote_conv_market: c::NO_QUOTE_CONV,
        },
    }
}

impl LiteChain {
    /// A world for `spec`: the real vault, a rule set with the path's spreads and commissions, its markets, one account of
    /// the path's tier opened at `spec.opened_time`, set to the path's phase.
    pub fn new(spec: &PathSpec) -> LiteChain {
        let mut ruleset = launch_ruleset_args();
        ruleset.commission_e6 = spec.commission_e6;
        for m in &spec.markets {
            ruleset.spread_e6[usize::from(m.id)] = u16::try_from(m.spread_e6).unwrap();
        }
        let mut v = VaultEnv::with(Opts { ruleset, ..Opts::default() });
        v.e.set_clock(spec.opened_time);
        for i in 0..spec.markets.len() {
            v.e.create_market_ok(&market_args(spec, i));
        }
        let trader = trader_wallet();
        v.e.svm.airdrop(&trader.pubkey(), 10_000_000_000).unwrap();
        let mint = v.usdc;
        let ta = v.new_token_account(&mint, &trader.pubkey(), TRADER_START);
        v.traders.insert(trader.pubkey(), ta);
        assert_ok(v.open(&trader, u8::try_from(spec.tier).unwrap()));
        let acct = acct_pda(&trader.pubkey(), 0);
        let t0 = v.account(&acct).created_at;
        let mut j = JEnv { v, trader, acct, t0 };
        // the path's phase: payout count and request id as the vault and the account would have after phase-2 passes
        if spec.phase >= 2 {
            let p = spec.phase;
            j.edit_account(|a| {
                a.phase = p;
                a.next_request_id = u64::from(p) - 1;
            });
            let tk = j.trader.pubkey();
            j.v.edit_vault_record(&tk, 0, None, Some(u64::from(p) - 2));
        }
        // the daily cap must not stand in the way of a $16,000 payout
        j.set_limits(172_800, 100_000_000_000, 86_400);
        let a = j.account();
        let params = ::evaluator::AccountParams {
            account: acct.to_bytes(),
            trader: j.trader.pubkey().to_bytes(),
            challenge_id: a.challenge_id,
            size_micro: i64::try_from(a.size_s).unwrap(),
            phase: spec.phase,
            tier_index: u8::try_from(spec.tier).unwrap(),
            opened_time: a.created_at,
        };
        let opened_line = ::setl8_verify::log::opened_line(&shared::opened_rec(&params));
        LiteChain { account: acct.to_bytes(), j, params, lines: Vec::new(), clock: spec.opened_time, slot: 1000, shapes: BTreeMap::new(), txs: 0, opened_line, pass_events: 0 }
    }

    /// M8: the trader pays the reset price and continues in a NEW account of the same phase. The failed account (`self.j.acct`)
    /// goes through the REAL `reset_account` against the real vault at clock `t`; the chain then points at the new account
    /// (address, challenge id, event log and `opened` line), and the new account's stored bytes are compared with the fresh
    /// account the program must have created. Returns the new account's params (the evaluator of the second segment starts
    /// from them) and the `AccountReset` event.
    pub fn reset(&mut self, t: i64) -> (::evaluator::AccountParams, setl8_lev_trading::events::AccountReset) {
        self.set_clock(t);
        let failed = self.j.account();
        let ra = self.j.reset_accts();
        let clock = self.clock;
        let m = self.j.ok(|j| j.send_reset(clock, &ra, failed.challenge_id));
        self.txs += 1;
        let ev = cpi_events::<setl8_lev_trading::events::AccountReset>(&m).pop().expect("AccountReset event");
        let opened = cpi_events::<setl8_lev_trading::events::Opened>(&m).pop().expect("Opened event");
        self.j.acct = ra.trading_account;
        let a = self.j.account();
        // the account the program created: the failed phase, fresh state, a new journal, the payouts the vault carried over
        assert_eq!((a.phase, a.status, a.b, a.q_tally, a.latch, a.tight_on, a.flat, a.seq, a.lock_until), (failed.phase, 0, i64::try_from(failed.size_s).unwrap(), 0, 0, 0, 1, 0, 0));
        assert_eq!((a.challenge_id, a.size_s, a.created_at, a.next_request_id), (failed.challenge_id + 1, failed.size_s, self.clock, failed.next_request_id));
        assert_eq!(ev.phase, failed.phase);
        let params = ::evaluator::AccountParams {
            account: self.j.acct.to_bytes(),
            trader: self.j.trader.pubkey().to_bytes(),
            challenge_id: a.challenge_id,
            size_micro: i64::try_from(a.size_s).unwrap(),
            phase: failed.phase,
            tier_index: self.params.tier_index,
            opened_time: a.created_at,
        };
        // what the event log says about the new account is what the program emitted
        let rec = shared::opened_rec(&params);
        assert_eq!(
            (opened.account.to_bytes(), opened.trader.to_bytes(), opened.challenge_id, opened.size_s, opened.tier_index, opened.phase, opened.ruleset_version, opened.genesis_hash, opened.time),
            (rec.account, rec.trader, rec.challenge_id, rec.size_s, rec.tier_index, rec.phase, rec.ruleset_version, rec.genesis_hash, rec.time),
            "the program's Opened event is the log's opened line"
        );
        self.account = params.account;
        self.params = params.clone();
        self.lines.clear();
        self.opened_line = ::setl8_verify::log::opened_line(&rec);
        (params, ev)
    }

    fn set_clock(&mut self, t: i64) {
        if t > self.clock {
            self.clock = t;
        }
        self.j.v.e.set_clock(self.clock);
    }

    fn sign_and_send(&mut self, ix: Instruction) -> (TransactionResult, usize) {
        let kp = dup(&self.j.v.evaluator);
        let svm = &mut self.j.v.e.svm;
        let bh = svm.latest_blockhash();
        let msg = Message::new_with_blockhash(&[ix], Some(&kp.pubkey()), &bh);
        let mut tx = Transaction::new_unsigned(msg);
        tx.partial_sign(&[&kp], bh);
        let size = bincode::serialize(&tx).expect("serialize").len();
        let r = svm.send_transaction(tx);
        svm.expire_blockhash();
        self.txs += 1;
        (r, size)
    }

    /// Sends a batch without any check; `signer` replaces the evaluator key when given.
    pub fn try_post(&mut self, b: &Batch, signer: Option<&Keypair>) -> Result<TransactionMetadata, PostError> {
        self.try_post_bytes(b, None, signer)
    }

    /// Like `try_post`, with raw batch bytes instead of the encoding of `b.batch` (inputs the encoder refuses to produce).
    pub fn try_post_bytes(&mut self, b: &Batch, bytes: Option<Vec<u8>>, signer: Option<&Keypair>) -> Result<TransactionMetadata, PostError> {
        self.set_clock(b.meta_time);
        let mut ix = self.post_ix(b, bytes);
        let kp = match signer {
            Some(k) => {
                // the account in the signer slot changes; everything else stays
                ix.accounts[0].pubkey = k.pubkey();
                dup(k)
            }
            None => dup(&self.j.v.evaluator),
        };
        let svm = &mut self.j.v.e.svm;
        svm.airdrop(&kp.pubkey(), 1_000_000_000).ok();
        let bh = svm.latest_blockhash();
        let msg = Message::new_with_blockhash(&[ix], Some(&kp.pubkey()), &bh);
        let mut tx = Transaction::new_unsigned(msg);
        tx.partial_sign(&[&kp], bh);
        let r = svm.send_transaction(tx);
        svm.expire_blockhash();
        self.txs += 1;
        match r {
            Ok(m) => Ok(m),
            Err(f) => {
                use anchor_lang::solana_program::instruction::error::InstructionError;
                use solana_transaction_error::TransactionError;
                let code = match &f.err {
                    TransactionError::InstructionError(_, InstructionError::Custom(c)) => Some(*c),
                    _ => None,
                };
                Err(PostError { code, text: format!("{:?}\n{}", f.err, f.meta.logs.join("\n")) })
            }
        }
    }

    fn post_ix(&self, b: &Batch, bytes: Option<Vec<u8>>) -> Instruction {
        let bytes = bytes.unwrap_or_else(|| b.batch.to_bytes().expect("batch encodes"));
        let ev = self.j.v.evaluator.pubkey();
        match b.kind {
            BatchKind::PostJournal => {
                let a = self.j.account();
                let mut metas = accounts::PostJournal {
                    evaluator: ev,
                    trading_account: self.j.acct,
                    config: config_pda(),
                    ruleset: ruleset_pda(a.ruleset_version),
                    event_authority: Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0,
                    program: PROGRAM_ID,
                }
                .to_account_metas(None);
                metas.extend(JEnv::market_metas(&b.markets));
                Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::PostJournal { batch_bytes: bytes }.data() }
            }
            BatchKind::ReportFail(r) => self.j.report_fail_ix(ev, bytes, r as u8, &b.markets),
        }
    }

    fn shape_key(b: &Batch, w: &World) -> String {
        let proof_sources: Vec<u8> = {
            let mut v: Vec<u8> = b.batch.proofs.iter().map(|p| p.source).collect();
            v.sort();
            v.dedup();
            v
        };
        let _ = w;
        format!(
            "{} events={} proofs={} sources={:?}",
            match b.kind {
                BatchKind::PostJournal => "post_journal",
                BatchKind::ReportFail(_) => "report_fail",
            },
            b.batch.events.len(),
            b.batch.proofs.len(),
            proof_sources
        )
    }

    /// Sends the pass instruction's lines from the events of a successful transaction.
    fn pass_lines(&mut self, meta: &TransactionMetadata) {
        for e in cpi_events::<PassRequested>(meta) {
            let v = ChainView { seq: e.seq, journal_hash: e.journal_hash, b: e.b, flat: true, phase: e.phase };
            self.lines.push(pass_requested_line(&self.account, e.phase, e.amount, &v, e.time));
            self.pass_events += 1;
        }
        for e in cpi_events::<PassConfirmed>(meta) {
            let v = ChainView { seq: e.seq, journal_hash: e.journal_hash, b: e.b, flat: true, phase: e.phase };
            self.lines.push(pass_confirmed_line(&self.account, e.phase, e.amount, &v, e.eligible_at));
            self.pass_events += 1;
        }
        for e in &cpi_events::<PassFinalized>(meta) {
            self.lines.push(pass_finalized_line(&self.account, e.phase, e.to_phase, e.amount, e.request_id, e.time));
            self.pass_events += 1;
        }
        // the program emits `PhaseAdvanced` as well as `PassFinalized` for one `finalize_pass`; the verifier takes the one that
        // directly follows a `pass_finalized` with the same new phase as a confirmation (M9b decision D5)
        for e in cpi_events::<PhaseAdvanced>(meta) {
            self.lines.push(phase_advanced_line(&self.account, e.from_phase, e.to_phase, e.time));
            self.pass_events += 1;
        }
    }

    pub fn account_snapshot_json(&self, phase_override: Option<u8>) -> String {
        let a = self.j.account();
        serde_json::to_string(&serde_json::json!({"version": 1, "b": a.b, "q_tally": a.q_tally, "latch": a.latch, "tight_on": a.tight_on,
            "r": a.r, "g_peak": a.g_peak, "l_level": a.l_level, "lock_until": a.lock_until, "seq": a.seq,
            "journal_hash": ::setl8_verify::hexutil::encode(&a.journal_hash), "flat": a.flat, "phase": phase_override.unwrap_or(a.phase),
            "status": a.status, "trades_closed": a.trades_closed}))
        .expect("json")
    }

    pub fn events_text(&self) -> String {
        let mut t = ::setl8_verify::log::header_line();
        t.push_str(&self.opened_line);
        for l in &self.lines {
            t.push_str(l);
        }
        t
    }
}

impl Chain for LiteChain {
    fn post(&mut self, w: &World, b: &Batch) {
        self.set_clock(b.meta_time);
        let ix = self.post_ix(b, None);
        let before = self.j.account();
        let (r, size) = self.sign_and_send(ix);
        let meta = match r {
            Ok(m) => m,
            Err(f) => panic!(
                "an honest batch was rejected: {:?} (kind {:?}, {} events, first_seq {})\nlogs:\n{}",
                f.err,
                b.kind,
                b.batch.events.len(),
                b.batch.first_seq,
                f.meta.logs.join("\n")
            ),
        };
        // 1. the whole stored account equals the evaluator's state
        let after = self.j.account();
        let exp = expected_after(&before, b, self.clock);
        if let Some(d) = diff_accounts(&after, &exp) {
            panic!("the stored account differs from the evaluator's after a batch (first_seq {}): {d}", b.batch.first_seq);
        }
        // 2. the emitted EventLogs are the batch's events and proofs, and extend the chain
        let logs = event_logs(&meta);
        assert_eq!(logs.len(), b.batch.events.len(), "number of EventLogs");
        for (i, l) in logs.iter().enumerate() {
            assert_eq!(l.seq, b.batch.first_seq + i as u64, "EventLog seq");
            let ev = JournalEvent::decode(&l.event_bytes).expect("event decodes");
            assert_eq!(ev, b.batch.events[i], "EventLog {i} is not the posted event");
            let want_proof = ev.proof_index().and_then(|k| b.batch.proofs.get(k)).map(|p| p.to_bytes().unwrap()).unwrap_or_default();
            assert_eq!(l.proof_bytes, want_proof, "EventLog {i} proof bytes");
            self.lines.push(::setl8_verify::log::event_line(&self.account, l.seq, &l.event_bytes, &l.proof_bytes, self.slot));
            self.slot += 1;
        }
        // 3. size and compute against the model
        assert_eq!(size, b.tx_bytes, "the modelled transaction size is exact");
        assert!(size <= 1_232, "transaction of {size} bytes");
        assert!(meta.compute_units_consumed <= b.cu, "compute units {} above the model {}", meta.compute_units_consumed, b.cu);
        assert!(meta.compute_units_consumed <= 200_000);
        let key = Self::shape_key(b, w);
        let s = self.shapes.entry(key).or_default();
        s.count += 1;
        s.max_bytes = s.max_bytes.max(size);
        s.max_cu = s.max_cu.max(meta.compute_units_consumed);
        s.model_bytes_max = s.model_bytes_max.max(b.tx_bytes);
        s.model_cu_max = s.model_cu_max.max(b.cu);
    }

    fn view(&mut self, _w: &World) -> ChainView {
        let a = self.j.account();
        ChainView { seq: a.seq, journal_hash: a.journal_hash, b: a.b, flat: a.flat == 1, phase: a.phase }
    }

    fn request_advance(&mut self, _w: &World, t: i64) -> bool {
        self.set_clock(t);
        let r = self.j.request(self.clock);
        match r {
            Ok(meta) => {
                self.pass_lines(&meta);
                true
            }
            Err(f) => {
                use anchor_lang::solana_program::instruction::error::InstructionError;
                use solana_transaction_error::TransactionError;
                let code = match &f.err {
                    TransactionError::InstructionError(0, InstructionError::Custom(code)) => *code,
                    other => panic!("request_advance failed unexpectedly: {other:?}\n{}", f.meta.logs.join("\n")),
                };
                // TargetNotReached 6021, NotFlat 6020, TightModeOn 6085, PhaseFailed 6086
                assert!([6020, 6021, 6085, 6086].contains(&code), "request_advance error {code}");
                false
            }
        }
    }

    fn confirm_pass(&mut self, _w: &World, t: i64) {
        self.set_clock(t);
        let meta = self.j.confirm(self.clock).expect("confirm_pass");
        self.pass_lines(&meta);
    }

    fn reject_pass(&mut self, _w: &World, t: i64, reason: PassReject) {
        self.set_clock(t);
        self.j.reject(self.clock, reason as u8).expect("reject_pass");
    }

    fn cancel_pass(&mut self, _w: &World, t: i64) {
        self.set_clock(t);
        self.j.cancel(self.clock).expect("cancel_pass");
    }

    fn veto_pass(&mut self, _w: &World, t: i64) {
        self.set_clock(t);
        self.j.veto(self.clock).expect("veto_pass");
    }

    fn finalize_pass(&mut self, _w: &World, t: i64) {
        self.set_clock(t);
        let payer = self.j.new_payer();
        let meta = match self.j.finalize(self.clock, &payer) {
            Ok(m) => m,
            Err(f) => panic!("finalize_pass failed: {:?}\n{}", f.err, f.meta.logs.join("\n")),
        };
        self.pass_lines(&meta);
    }
}
