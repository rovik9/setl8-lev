//! Journal test harness: a world with the real vault, markets, a rule set with spreads and ONE opened account, plus a
//! batch builder written on the real `journal_types` crate (never program code) and `post`.
#![allow(dead_code)]

use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use journal_types::{
    apply_batch, feed_id, genesis_hash, next_hash, verify_chain, ChainEntry, CloseReason, FailReason, JournalBatch, JournalEvent,
    NativeSha256, OrderKind, PriceProof, Side,
};
use litesvm::types::{TransactionMetadata, TransactionResult};
use setl8_lev_trading::{
    accounts, constants as c,
    events::EventLog,
    instruction,
    instructions::{CreateMarketArgs, MarketParams},
    state::{Session, TradingAccount},
};
use solana_signer::Signer;

use anchor_lang::AccountDeserialize;

use super::vault::*;
use super::*;

pub const MARKET_EUR: u16 = 0;
pub const MARKET_BTC: u16 = 1;
pub const MARKET_REDUCE_ONLY: u16 = 2;
pub const MARKET_DISABLED: u16 = 3;
pub const MARKET_SHORT_SESSION: u16 = 4;
pub const SOURCE_GMX: u8 = 2;
/// Half-spreads of the test rule set (e6): EURUSD 30 as in DESIGN 4.9, BTC 50, the others 10.
pub const H_EUR: u32 = 30;
pub const H_BTC: u32 = 50;
pub const H_OTHER: u32 = 10;
pub const S100K: i64 = 100_000_000_000;

pub fn usd(d: i64) -> i64 {
    d * 1_000_000
}

pub fn feed(symbol: &str) -> [u8; 32] {
    feed_id(&NativeSha256, SOURCE_GMX, symbol)
}

pub fn symbol_of(market: u16) -> &'static str {
    match market {
        MARKET_EUR => "EURUSD",
        MARKET_BTC => "BTC",
        MARKET_REDUCE_ONLY => "REDUCE",
        MARKET_DISABLED => "DISABLED",
        _ => "SHORT",
    }
}

/// A source-2 proof of `mid_e12` observed at unix second `time`.
pub fn proof(market: u16, time: i64, mid_e12: u64) -> PriceProof {
    PriceProof {
        source: SOURCE_GMX,
        feed_id: feed(symbol_of(market)),
        timestamp_ns: u64::try_from(time).unwrap() * 1_000_000_000,
        quantized_value: i128::from(mid_e12) * 1_000_000,
        signed_update: vec![],
    }
}

pub fn ask(mid: u64, h: u32) -> u64 {
    engine::ask_from_mid(mid, h).unwrap()
}
pub fn bid(mid: u64, h: u32) -> u64 {
    engine::bid_from_mid(mid, h).unwrap()
}

pub fn fill(market: u16, long: bool, qty: u64, price: u64, time: i64, proof: u8) -> JournalEvent {
    JournalEvent::Fill {
        market,
        side: if long { Side::Long } else { Side::Short },
        order_kind: OrderKind::Market,
        qty,
        price_e12: price,
        time,
        avg_entry_after: price,
        margin: 0,
        sl: 0,
        tp: 0,
        intent_id: 1,
        proof,
    }
}

/// A close with the claims filled in from the engine (so a test can corrupt exactly one field).
#[allow(clippy::too_many_arguments)]
pub fn close(market: u16, long: bool, qty: u64, entry: u64, exit: u64, time: i64, timer_start: i64, proof: u8) -> JournalEvent {
    let pnl = engine::pnl_micro(long, qty, entry, exit).unwrap();
    let quick = time - timer_start <= 45;
    let (credited, _) = engine::slice_credit(pnl, quick);
    JournalEvent::Close {
        market,
        side: if long { Side::Long } else { Side::Short },
        qty,
        entry_e12: entry,
        exit_e12: exit,
        time,
        timer_start,
        pnl,
        quick,
        credited,
        reason: CloseReason::Manual,
        intent_id: 1,
        proof,
    }
}

pub fn fee(market: u16, amount: u64, time: i64) -> JournalEvent {
    JournalEvent::Fee { kind: journal_types::FeeKind::Commission, market, amount, time }
}

pub fn meta(time: i64, flat: bool) -> JournalEvent {
    JournalEvent::Meta { time, flat, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION }
}

pub fn fail_event(time: i64, proof: u8) -> JournalEvent {
    JournalEvent::Fail { time, reason: FailReason::StaticFloor, b_after: 0, proof }
}

/// Rule set 1 with spreads (launch values otherwise) so executed prices differ from the mid.
pub fn spread_ruleset() -> journal_types_args::Args {
    let mut a = launch_ruleset_args();
    a.spread_e6[usize::from(MARKET_EUR)] = u16::try_from(H_EUR).unwrap();
    a.spread_e6[usize::from(MARKET_BTC)] = u16::try_from(H_BTC).unwrap();
    for m in [MARKET_REDUCE_ONLY, MARKET_DISABLED, MARKET_SHORT_SESSION] {
        a.spread_e6[usize::from(m)] = u16::try_from(H_OTHER).unwrap();
    }
    a
}
pub mod journal_types_args {
    pub type Args = setl8_lev_trading::instructions::RuleSetArgs;
}

fn market_args(id: u16, status: u8, sessions: &[(u16, u16)]) -> CreateMarketArgs {
    let mut s = [Session::default(); 6];
    for (i, (a, b)) in sessions.iter().enumerate() {
        s[i] = Session { start_min: *a, end_min: *b };
    }
    CreateMarketArgs {
        market_id: id,
        class: 0,
        params: MarketParams {
            status,
            feed_id: feed(symbol_of(id)),
            max_age_secs: 60,
            qty_step: 1_000,
            qty_min: 1_000_000,
            max_notional_usd: 1_000_000_000_000_000,
            session_count: u8::try_from(sessions.len()).unwrap(),
            sessions: s,
            quote_conv_market: c::NO_QUOTE_CONV,
        },
    }
}

/// All events of a transaction: the `EventLog`s emitted through `emit_cpi!`.
pub fn event_logs(meta: &TransactionMetadata) -> Vec<EventLog> {
    let mut out = Vec::new();
    for group in &meta.inner_instructions {
        for inner in group {
            let d = &inner.instruction.data;
            if d.len() > 16 && d[..8] == *anchor_lang::event::EVENT_IX_TAG_LE && d[8..16] == *EventLog::DISCRIMINATOR {
                out.push(anchor_lang::AnchorDeserialize::deserialize(&mut &d[16..]).expect("EventLog decodes"));
            }
        }
    }
    out
}

pub struct JEnv {
    pub v: VaultEnv,
    pub trader: solana_keypair::Keypair,
    pub acct: Pubkey,
    /// Time of account creation (the cluster clock at open).
    pub t0: i64,
}

/// A copy of an event with its `time` replaced.
pub fn at(ev: &JournalEvent, t: i64) -> JournalEvent {
    let mut e = ev.clone();
    match &mut e {
        JournalEvent::Fill { time, .. }
        | JournalEvent::Close { time, .. }
        | JournalEvent::Fee { time, .. }
        | JournalEvent::TightStart { time, .. }
        | JournalEvent::FloorRaised { time, .. }
        | JournalEvent::TightEnd { time }
        | JournalEvent::Breach { time, .. }
        | JournalEvent::Fail { time, .. }
        | JournalEvent::Meta { time, .. } => *time = t,
    }
    e
}

impl JEnv {
    /// One $100,000 account (tier index 8) of a fresh trader; markets 0 to 4 exist.
    pub fn new() -> Self {
        Self::with_tier(8)
    }

    pub fn with_tier(tier: u8) -> Self {
        Self::with_trader(tier, None)
    }

    /// `with_tier` with a trader of a fixed key (`seed`), or a random one.
    pub fn with_trader(tier: u8, seed: Option<u32>) -> Self {
        let mut v = VaultEnv::with(Opts { ruleset: spread_ruleset(), ..Opts::default() });
        let full_week = [(0u16, 10_080u16)];
        v.e.create_market_ok(&market_args(MARKET_EUR, 1, &full_week));
        v.e.create_market_ok(&market_args(MARKET_BTC, 1, &full_week));
        v.e.create_market_ok(&market_args(MARKET_REDUCE_ONLY, 2, &full_week));
        v.e.create_market_ok(&market_args(MARKET_DISABLED, 0, &full_week));
        // a market that is open only during the first minute of the week
        v.e.create_market_ok(&market_args(MARKET_SHORT_SESSION, 1, &[(0, 1)]));
        let trader = match seed {
            Some(sd) => v.new_trader_seeded(sd),
            None => v.new_trader(),
        };
        assert!(v.open(&trader, tier).is_ok(), "open_account");
        let acct = acct_pda(&trader.pubkey(), 0);
        let t0 = v.account(&acct).created_at;
        JEnv { v, trader, acct, t0 }
    }

    pub fn account(&self) -> TradingAccount {
        self.v.account(&self.acct)
    }

    /// Edits the trading account in place (test helper for states that would take long journals to reach).
    pub fn edit_account(&mut self, f: impl FnOnce(&mut TradingAccount)) {
        let mut acc = self.v.e.svm.get_account(&self.acct).expect("account");
        let mut a = TradingAccount::try_deserialize(&mut acc.data.as_slice()).unwrap();
        f(&mut a);
        let mut data = Vec::new();
        anchor_lang::AccountSerialize::try_serialize(&a, &mut data).unwrap();
        acc.data = data;
        self.v.e.svm.set_account(self.acct, acc).unwrap();
    }

    /// Creates one more market whose feed is `feed_id(source, symbol)`.
    pub fn add_market(&mut self, id: u16, source: u8, symbol: &str, status: u8) {
        let mut a = market_args(id, status, &[(0, 10_080)]);
        a.params.feed_id = feed_id(&NativeSha256, source, symbol);
        self.v.e.create_market_ok(&a);
    }

    pub fn digest(&self) -> [u8; 32] {
        self.v.digest(&self.trader.pubkey(), &[])
    }

    /// A batch continuing the account's chain: `first_seq = seq + 1`, `prev_hash = journal_hash`.
    pub fn batch(&self, events: Vec<JournalEvent>, proofs: Vec<PriceProof>) -> JournalBatch {
        let a = self.account();
        JournalBatch { first_seq: a.seq + 1, prev_hash: a.journal_hash, events, proofs }
    }

    pub fn market_metas(markets: &[u16]) -> Vec<AccountMeta> {
        markets.iter().map(|m| AccountMeta::new_readonly(market_pda(*m), false)).collect()
    }

    /// Sends `post_journal` with the raw batch bytes at cluster time `clock`, markets as remaining accounts.
    pub fn post_bytes(&mut self, clock: i64, signer: &solana_keypair::Keypair, bytes: Vec<u8>, markets: &[u16]) -> TransactionResult {
        self.v.e.set_clock(clock);
        let ruleset = ruleset_pda(self.account().ruleset_version);
        let mut metas = accounts::PostJournal {
            evaluator: signer.pubkey(),
            trading_account: self.acct,
            config: config_pda(),
            ruleset,
            event_authority: Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0,
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(Self::market_metas(markets));
        let ix = Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::PostJournal { batch_bytes: bytes }.data() };
        // post_journal needs more than the default 200k units for large batches: ask for 600k
        let budget = Instruction {
            program_id: "ComputeBudget111111111111111111111111111111".parse().unwrap(),
            accounts: vec![],
            data: [vec![2u8], 600_000u32.to_le_bytes().to_vec()].concat(),
        };
        let s = dup(signer);
        self.v.e.send(&[budget, ix], &[&s])
    }

    pub fn post(&mut self, clock: i64, batch: &JournalBatch, markets: &[u16]) -> TransactionResult {
        let bytes = batch.to_bytes().expect("batch encodes");
        let ev = dup(&self.v.evaluator);
        self.post_bytes(clock, &ev, bytes, markets)
    }

    /// Posts and expects success.
    pub fn post_ok(&mut self, clock: i64, batch: &JournalBatch, markets: &[u16]) -> TransactionMetadata {
        match self.post(clock, batch, markets) {
            Ok(m) => m,
            Err(f) => panic!("post_journal failed: {:?}\n{}", f.err, f.meta.logs.join("\n")),
        }
    }

    /// Posts, expects failure with exactly `want` at instruction 1 (instruction 0 is the compute budget) and an unchanged world.
    pub fn post_rejected(&mut self, clock: i64, batch: &JournalBatch, markets: &[u16], want: setl8_lev_trading::errors::Setl8Error) {
        let before = self.digest();
        let r = self.post(clock, batch, markets);
        assert_err_at(&r, 1, want);
        assert_eq!(self.digest(), before, "a rejected batch changed the world ({want:?})");
    }

    /// The chain head the real journal-types crate computes for `batch` on top of the account's current head.
    pub fn expected_head(&self, batch: &JournalBatch) -> ([u8; 32], u64) {
        apply_batch(&NativeSha256, &self.account().journal_hash, batch.first_seq, batch).unwrap()
    }
}

/// Exact program error `want` at instruction `idx`.
pub fn assert_err_at(r: &TransactionResult, idx: u8, want: setl8_lev_trading::errors::Setl8Error) {
    use anchor_lang::solana_program::instruction::error::InstructionError;
    use solana_transaction_error::TransactionError;
    match r {
        Ok(m) => panic!("expected {want:?} but the transaction succeeded\n{}", m.logs.join("\n")),
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, InstructionError::Custom(code)) => {
                assert_eq!(*i, idx, "failed at the wrong instruction\n{}", f.meta.logs.join("\n"));
                assert_eq!(*code, u32::from(want), "wrong error code for {want:?}, logs:\n{}", f.meta.logs.join("\n"));
            }
            other => panic!("expected {want:?}, got {other:?}\n{}", f.meta.logs.join("\n")),
        },
    }
}

/// Anchor framework error code at instruction `idx`.
pub fn assert_anchor_at(r: &TransactionResult, idx: u8, want: u32) {
    use anchor_lang::solana_program::instruction::error::InstructionError;
    use solana_transaction_error::TransactionError;
    match r {
        Ok(_) => panic!("expected anchor error {want} but the transaction succeeded"),
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, InstructionError::Custom(code)) => {
                assert_eq!((*i, *code), (idx, want), "logs:\n{}", f.meta.logs.join("\n"));
            }
            other => panic!("expected anchor error {want}, got {other:?}"),
        },
    }
}

/// Recomputes the chain from emitted `EventLog`s with `journal_types::verify_chain` (the independent check).
pub fn chain_head_from_logs(logs: &[EventLog], pubkey: &Pubkey) -> [u8; 32] {
    let genesis = genesis_hash(&NativeSha256, &pubkey.to_bytes());
    let mut entries = Vec::new();
    let mut prev = genesis;
    for l in logs {
        let event = JournalEvent::decode(&l.event_bytes).expect("event bytes decode");
        let proof = if l.proof_bytes.is_empty() { None } else { Some(PriceProof::decode(&l.proof_bytes).expect("proof bytes decode")) };
        prev = next_hash(&NativeSha256, &prev, l.seq, &event, proof.as_ref()).expect("next_hash");
        entries.push(ChainEntry { seq: l.seq, event, proof, hash: prev });
    }
    verify_chain(&NativeSha256, &genesis, 1, &entries).expect("verify_chain over the emitted logs")
}

/// Raw batch bytes built by hand, for inputs the encoder refuses to produce (counts that lie, extra bytes).
pub fn raw_batch(first_seq: u64, prev: [u8; 32], events: &[Vec<u8>], n_events: u32, proofs: &[Vec<u8>], n_proofs: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&first_seq.to_le_bytes());
    v.extend_from_slice(&prev);
    v.extend_from_slice(&n_events.to_le_bytes());
    for e in events {
        v.extend_from_slice(e);
    }
    v.extend_from_slice(&n_proofs.to_le_bytes());
    for p in proofs {
        v.extend_from_slice(p);
    }
    v
}

// ---------------------------------------------------------------- shared scenario helpers

/// 1,000,000 units of the base currency in 1e-6 units: at this quantity `pnl = exit - entry` in micro-USD.
pub const MEUR: u64 = 1_000_000_000_000;
/// Proof mid used by most tests (1.0).
pub const MID: u64 = 1_000_000_000_000;

pub fn eur_ask() -> u64 {
    ask(MID, H_EUR)
}
pub fn eur_bid() -> u64 {
    bid(MID, H_EUR)
}
pub fn p_eur(t: i64) -> PriceProof {
    proof(MARKET_EUR, t, MID)
}
/// A long fill of 1,000,000 EUR at the ask of `MID`, referencing proof `idx`.
pub fn fill_long(t: i64, idx: u8) -> JournalEvent {
    fill(MARKET_EUR, true, MEUR, eur_ask(), t, idx)
}
/// A close of a LONG of 1,000,000 EUR at the bid of `MID` whose realized P&L is exactly `pnl` micro-USD.
pub fn close_long_pnl(t: i64, timer_start: i64, pnl: i64) -> JournalEvent {
    let exit = eur_bid();
    let entry = u64::try_from(i128::from(exit) - i128::from(pnl)).unwrap();
    close(MARKET_EUR, true, MEUR, entry, exit, t, timer_start, 0)
}
/// `[events..., Meta(flat)]` with one EUR proof at `t` (index 0) and the Meta at time `t`.
pub fn simple(w: &JEnv, t: i64, mut events: Vec<JournalEvent>, flat: bool) -> JournalBatch {
    events.push(meta(t, flat));
    w.batch(events, vec![p_eur(t)])
}
/// Puts the account into tight mode (R 100k, l_start 96k, g_peak 1.2k, L 96.2k) with closed balance `b`.
pub fn set_tight(w: &mut JEnv, b: i64) {
    w.edit_account(|a| {
        a.tight_on = 1;
        a.r = usd(100_000);
        a.g_peak = usd(1_200);
        a.l_level = usd(96_200);
        a.b = b;
        a.flat = 0;
    });
}
