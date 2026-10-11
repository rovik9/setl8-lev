//! Test-only evaluator: drives the REAL `engine` and `journal_types` (never the verifier's replay code) over a scripted
//! sample stream and signed orders, and writes the five input files the verifier reads. The tamper tests edit the
//! structured `Bundle` and re-serialise it, so each tamper changes exactly one thing.
#![allow(dead_code)]

use std::collections::BTreeMap;

use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signer};
use engine::{AccountState, Position, Rules as ERules, SampleAction, Side as ESide};
use journal_types::{CloseReason, FailReason, FeeKind, JournalEvent, NativeSha256, OrderKind, PriceProof, Side};
use serde_json::json;
use setl8_verify::archive::{ArchiveWriter, Sample, RECORD_LEN};
use setl8_verify::hexutil;
pub use setl8_verify::intents::IntentKind;
use setl8_verify::intents::{self, OrderIntent};
use setl8_verify::log::{self, OpenedRec};

pub const USD: i64 = 1_000_000;
pub const S100K: i64 = 100_000 * USD;
pub const E12: u64 = 1_000_000_000_000;
/// 2026-10-12 (a Monday) 14:00:00 UTC.
pub const T_1400: i64 = 1_791_813_600;
pub const HOUR: i64 = 3_600;

pub fn account_key() -> [u8; 32] {
    [0xA7; 32]
}

pub fn trader_keypair() -> Keypair {
    let secret = SecretKey::from_bytes(&[0x42; 32]).expect("seed");
    let public: PublicKey = (&secret).into();
    Keypair { secret, public }
}

#[derive(Clone, Debug)]
pub struct MarketSpec {
    pub id: u16,
    pub class: u8,
    pub source: u8,
    pub symbol: &'static str,
    pub spread_e6: u16,
    pub max_age_secs: u16,
    pub sessions: Vec<(u16, u16)>,
}

impl MarketSpec {
    pub fn feed(&self) -> [u8; 32] {
        journal_types::feed_id(&NativeSha256, self.source, self.symbol)
    }
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub size: i64,
    pub phase: u8,
    pub leverage: [u16; 8],
    pub holding: [u32; 8],
    pub commission: [u32; 8],
    pub markets: Vec<MarketSpec>,
    pub opened_time: i64,
}

impl Spec {
    pub fn base() -> Spec {
        Spec {
            size: S100K,
            phase: 1,
            leverage: [50, 20, 20, 20, 10, 5, 5, 2],
            holding: [100, 200, 400, 200, 400, 400, 1000, 1600],
            commission: [0; 8],
            markets: vec![MarketSpec { id: 0, class: 0, source: 2, symbol: "EURUSD", spread_e6: 30, max_age_secs: 60, sessions: vec![(0, 10_080)] }],
            opened_time: T_1400 - HOUR,
        }
    }

    pub fn market(&self, id: u16) -> &MarketSpec {
        self.markets.iter().find(|m| m.id == id).expect("market")
    }

    pub fn rules_json(&self) -> String {
        let markets: Vec<_> = self
            .markets
            .iter()
            .map(|m| {
                json!({"id": m.id, "class": m.class, "feed_id": hexutil::encode(&m.feed()), "max_age_secs": m.max_age_secs, "status": 1,
                       "qty_step": 1000, "qty_min": 1000, "max_notional_usd": 1_000_000_000_000_000u64,
                       "sessions": m.sessions.iter().map(|s| json!([s.0, s.1])).collect::<Vec<_>>()})
            })
            .collect();
        let spread: serde_json::Map<String, serde_json::Value> = self.markets.iter().map(|m| (m.id.to_string(), json!(m.spread_e6))).collect();
        serde_json::to_string(&json!({"version": 1, "ruleset_version": 1, "fee_bps": 40, "leverage_x": self.leverage, "holding_fee_e6": self.holding,
            "commission_e6": self.commission, "tier_sizes": [self.size], "spread_e6": spread, "markets": markets}))
        .expect("json")
    }
}

#[derive(Clone, Debug)]
pub struct IntentRec {
    pub nonce: u32,
    pub intent: OrderIntent,
    pub order: Vec<u8>,
    pub sig: [u8; 64],
}

#[derive(Clone, Debug)]
pub enum GLine {
    Opened(OpenedRec),
    Event { seq: u64, ev: JournalEvent, proof: Option<PriceProof> },
    Raw(String),
}

#[derive(Clone, Debug)]
pub struct MetaCommit {
    pub line: usize,
    pub n_samples: usize,
    pub n_intent_lines: usize,
}

/// Everything the verifier reads, in structured form.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub spec: Spec,
    pub samples: Vec<Sample>,
    pub intents: Vec<IntentRec>,
    pub lines: Vec<GLine>,
    pub metas: Vec<MetaCommit>,
    pub final_state: AccountState,
    pub final_trades: u32,
    pub final_phase: u8,
}

pub fn sample_for(spec: &Spec, market: u16, t: i64, mid_e12: u64) -> Sample {
    let ts_ns = u64::try_from(t).expect("t") * 1_000_000_000;
    let source = spec.market(market).source;
    // source 3 records the sha256 of the signed blob (here a stand-in blob derived from market and time)
    let digest = if source == 3 {
        use sha2::{Digest, Sha256};
        Sha256::digest([market.to_le_bytes().as_slice(), ts_ns.to_le_bytes().as_slice()].concat()).into()
    } else {
        [0; 32]
    };
    Sample { market, source, recv_ns: ts_ns + 1_000_000, ts_ns, value: i128::from(mid_e12) * 1_000_000, digest }
}

pub fn proof_of(spec: &Spec, s: &Sample) -> PriceProof {
    let signed_update = if s.source == 3 { s.digest.to_vec() } else { vec![] };
    PriceProof { source: s.source, feed_id: spec.market(s.market).feed(), timestamp_ns: s.ts_ns, quantized_value: s.value, signed_update }
}

#[allow(clippy::too_many_arguments)]
pub fn order(kind: IntentKind, market: u16, side: u8, qty: u64, price: u64, sl: u64, tp: u64, target: u32, t: i64) -> OrderIntent {
    OrderIntent { kind, market, side, qty, price_e12: price, sl_e12: sl, tp_e12: tp, target_nonce: target, client_time: t }
}

pub fn market_order(market: u16, long: bool, qty: u64, t: i64) -> OrderIntent {
    order(IntentKind::Market, market, if long { 0 } else { 1 }, qty, 0, 0, 0, 0, t)
}

pub fn sign(nonce: u32, intent: &OrderIntent) -> IntentRec {
    let kp = trader_keypair();
    let ob = intent.to_bytes().to_vec();
    let sig = kp.sign(&intents::signing_message(&account_key(), nonce, &ob)).to_bytes();
    IntentRec { nonce, intent: *intent, order: ob, sig }
}

// -------------------------------------------------------------------------------------------- the evaluator

#[derive(Clone, Copy, Debug)]
struct GPos {
    p: Position,
    sl: u64,
    tp: u64,
}

#[derive(Clone, Debug)]
struct Pending {
    nonce: u32,
    intent: OrderIntent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Sample { market: u16, t: i64, mid: u64 },
    Order { t: i64, intent: OrderIntent },
    /// A batch that carries only a `Meta` (the evaluator says "I have processed everything up to `t`").
    Heartbeat { t: i64 },
}

pub struct Gen {
    pub spec: Spec,
    st: AccountState,
    pos: BTreeMap<u16, GPos>,
    pending: Vec<Pending>,
    samples: Vec<Sample>,
    latest: BTreeMap<u16, usize>,
    last_change: BTreeMap<u16, i64>,
    last_value: BTreeMap<u16, i128>,
    intents: Vec<IntentRec>,
    next_nonce: u32,
    lines: Vec<GLine>,
    metas: Vec<MetaCommit>,
    seq: u64,
    roll_cursor: i64,
    batch: Vec<(JournalEvent, Option<PriceProof>)>,
    terminal: bool,
    trades: u32,
    phase: u8,
    pub skip_fees: bool,
}

fn eside(long: bool) -> ESide {
    if long {
        ESide::Buy
    } else {
        ESide::Sell
    }
}

impl Gen {
    pub fn new(spec: Spec) -> Gen {
        let st = engine::reset_phase_state(spec.size);
        let mut lines = vec![];
        let genesis = journal_types::genesis_hash(&NativeSha256, &account_key());
        lines.push(GLine::Opened(OpenedRec {
            account: account_key(),
            trader: trader_keypair().public.to_bytes(),
            challenge_id: 0,
            size_s: u64::try_from(spec.size).expect("size"),
            tier_index: 8,
            phase: spec.phase,
            ruleset_version: 1,
            genesis_hash: genesis,
            time: spec.opened_time,
        }));
        let roll_cursor = spec.opened_time;
        let phase = spec.phase;
        Gen {
            spec,
            st,
            pos: BTreeMap::new(),
            pending: vec![],
            samples: vec![],
            latest: BTreeMap::new(),
            last_change: BTreeMap::new(),
            last_value: BTreeMap::new(),
            intents: vec![],
            next_nonce: 1,
            lines,
            metas: vec![],
            seq: 0,
            roll_cursor,
            batch: vec![],
            terminal: false,
            trades: 0,
            phase,
            skip_fees: false,
        }
    }

    pub fn run(mut self, script: &[Script]) -> Bundle {
        for s in script {
            match *s {
                Script::Sample { market, t, mid } => self.on_sample(market, t, mid),
                Script::Order { t, intent } => self.on_order(t, intent),
                Script::Heartbeat { t } => self.heartbeat(t),
            }
        }
        self.finish()
    }

    pub fn finish(self) -> Bundle {
        let mut st = self.st;
        st.flat = self.pos.is_empty();
        let mut b = Bundle {
            spec: self.spec,
            samples: self.samples,
            intents: self.intents,
            lines: self.lines,
            metas: self.metas,
            final_state: st,
            final_trades: self.trades,
            final_phase: self.phase,
        };
        b.refresh_commitments();
        b
    }

    fn bid_ask(&self, market: u16, idx: usize) -> (u64, u64) {
        let mid = self.samples[idx].mid_e12().expect("mid");
        let h = u32::from(self.spec.market(market).spread_e6);
        (engine::bid_from_mid(mid, h).expect("bid"), engine::ask_from_mid(mid, h).expect("ask"))
    }

    fn rate(&self, market: u16, holding: bool) -> u32 {
        let c = usize::from(self.spec.market(market).class);
        if holding {
            self.spec.holding[c]
        } else {
            self.spec.commission[c]
        }
    }

    fn push(&mut self, ev: JournalEvent, sample: Option<usize>) {
        let proof = sample.map(|i| proof_of(&self.spec, &self.samples[i]));
        self.batch.push((ev, proof));
    }

    fn push_commission(&mut self, market: u16, qty: u64, price: u64, t: i64) {
        let rate = self.rate(market, false);
        if rate == 0 {
            return;
        }
        let notional = engine::margin_micro(qty, price, 1).expect("notional");
        let fee = engine::fee_ceil_micro(notional, rate).expect("fee");
        self.push(JournalEvent::Fee { kind: FeeKind::Commission, market, amount: u64::try_from(fee).expect("fee"), time: t }, None);
        self.debit(fee);
    }

    fn debit(&mut self, amount: i64) {
        self.st = engine::apply_closed_slice(&self.st, &ERules::default(), -amount, 0).expect("debit").state;
    }

    fn heartbeat(&mut self, t: i64) {
        self.flush_batch_force(t, true);
    }

    fn flush_batch(&mut self, t: i64) {
        self.flush_batch_force(t, false);
    }

    fn flush_batch_force(&mut self, t: i64, force: bool) {
        if self.batch.is_empty() && !force {
            return;
        }
        let events = std::mem::take(&mut self.batch);
        for (ev, proof) in events {
            self.seq += 1;
            self.lines.push(GLine::Event { seq: self.seq, ev, proof });
        }
        self.seq += 1;
        let meta = JournalEvent::Meta { time: t, flat: self.pos.is_empty(), samples_hash: [0; 32], intents_hash: [0; 32], engine_version: engine::ENGINE_VERSION };
        self.metas.push(MetaCommit { line: self.lines.len(), n_samples: self.samples.len(), n_intent_lines: self.intents.len() });
        self.lines.push(GLine::Event { seq: self.seq, ev: meta, proof: None });
    }

    // ---- samples

    fn market_open(&self, market: u16, t: i64) -> bool {
        let m = self.spec.market(market);
        let last = self.last_change.get(&market).copied().unwrap_or(t);
        journal_types::market_open(&m.sessions, u8::try_from(m.sessions.len()).expect("n"), t, last, u32::from(m.max_age_secs))
    }

    fn on_sample(&mut self, market: u16, t: i64, mid: u64) {
        let s = sample_for(&self.spec, market, t, mid);
        self.samples.push(s);
        let idx = self.samples.len() - 1;
        self.latest.insert(market, idx);
        if self.last_value.get(&market) != Some(&s.value) {
            self.last_change.insert(market, t);
        }
        self.last_value.insert(market, s.value);
        if self.terminal {
            return;
        }
        // 1. holding fees of the rollovers that passed
        self.rollovers(t);
        // 2. a closed market acts on nothing
        if !self.market_open(market, t) {
            self.flush_batch(t);
            return;
        }
        // 3. floors
        let mut stopped = false;
        if !self.pos.is_empty() {
            stopped = self.floors(t, idx);
        }
        // 4. SL / TP of this market, 5. pending orders of this market
        if !stopped && !self.terminal {
            self.protective(market, t, idx);
            self.pending_orders(market, t, idx);
        }
        self.flush_batch(t);
    }

    fn rollovers(&mut self, t: i64) {
        if t <= self.roll_cursor {
            return;
        }
        if !self.pos.is_empty() && !self.skip_fees {
            let mut r = (engine::day_index(self.roll_cursor) + 1) * 86_400 + 22 * 3_600;
            while r <= t && !self.pos.is_empty() {
                let today = engine::day_index(r);
                let markets: Vec<u16> = self.pos.keys().copied().collect();
                for m in markets {
                    let p = self.pos[&m];
                    if p.p.fee_day >= today {
                        continue;
                    }
                    // the mark: the last sample of the market at or before r
                    let idx = self
                        .samples
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| s.market == m && s.ts_sec() <= r)
                        .map(|(i, _)| i)
                        .next_back()
                        .expect("mark");
                    let (bid, ask) = self.bid_ask(m, idx);
                    let mark = if p.p.qty > 0 { bid } else { ask };
                    let rate = self.rate(m, true);
                    if rate == 0 {
                        self.pos.get_mut(&m).expect("pos").p.fee_day = today;
                        continue;
                    }
                    let fee = engine::holding_fee_micro(p.p.qty.unsigned_abs(), mark, rate, 1).expect("fee");
                    self.push(JournalEvent::Fee { kind: FeeKind::Holding, market: m, amount: u64::try_from(fee).expect("fee"), time: r }, None);
                    self.debit(fee);
                    self.pos.get_mut(&m).expect("pos").p.fee_day = today;
                }
                r += 86_400;
            }
        }
        self.roll_cursor = t;
    }

    fn marks(&self) -> Vec<(Position, u64, u64)> {
        self.pos
            .iter()
            .map(|(m, p)| {
                let idx = self.latest[m];
                let (bid, ask) = self.bid_ask(*m, idx);
                (p.p, bid, ask)
            })
            .collect()
    }

    fn closing_commission_estimate(&self) -> i64 {
        let mut total = 0i64;
        for (m, p) in &self.pos {
            let rate = self.rate(*m, false);
            if rate == 0 {
                continue;
            }
            let idx = self.latest[m];
            let (bid, ask) = self.bid_ask(*m, idx);
            let mark = if p.p.qty > 0 { bid } else { ask };
            let notional = engine::margin_micro(p.p.qty.unsigned_abs(), mark, 1).expect("n");
            total += engine::fee_ceil_micro(notional, rate).expect("fee");
        }
        total
    }

    /// Returns true when the floor decision closed everything (Breach or Fail) so nothing else acts on this sample.
    fn floors(&mut self, t: i64, idx: usize) -> bool {
        let p_open = engine::open_profit_micro(&self.marks()).expect("p");
        let comm = self.closing_commission_estimate();
        let d = engine::evaluate_sample(&self.st, &ERules::default(), p_open, comm, t).expect("eval");
        match d.action {
            SampleAction::None => false,
            SampleAction::TightStart { r, l_start, g_peak, l_level } => {
                self.push(JournalEvent::TightStart { time: t, p_open, r, l_start, g_peak, l_level, proof: 0 }, Some(idx));
                self.st = d.state;
                false
            }
            SampleAction::FloorRaised { g_peak, l_level } => {
                self.push(JournalEvent::FloorRaised { time: t, g_peak, l_level, proof: 0 }, Some(idx));
                self.st = d.state;
                false
            }
            SampleAction::Breach { lock_until, .. } => {
                let l_before = self.st.l_level;
                self.close_all(t, CloseReason::Breach);
                let b_after = self.st.b;
                self.st = AccountState { tight_on: false, r: 0, l_start: 0, g_peak: 0, l_level: 0, lock_until, flat: true, ..self.st };
                self.push(JournalEvent::Breach { time: t, l_level: l_before, b_after, proof: 0 }, Some(idx));
                true
            }
            SampleAction::Fail(reason) => {
                self.close_all(t, CloseReason::Fail);
                let b_after = self.st.b;
                let reason = match reason {
                    engine::FailReason::StaticFloor => FailReason::StaticFloor,
                    engine::FailReason::PhaseQuick => FailReason::PhaseQuick,
                };
                self.push(JournalEvent::Fail { time: t, reason, b_after, proof: 0 }, Some(idx));
                self.terminal = true;
                true
            }
        }
    }

    fn close_all(&mut self, t: i64, reason: CloseReason) {
        let markets: Vec<u16> = self.pos.keys().copied().collect();
        for m in markets {
            let p = self.pos[&m];
            let idx = self.latest[&m];
            let (bid, ask) = self.bid_ask(m, idx);
            let long = p.p.qty > 0;
            let price = if long { bid } else { ask };
            self.close_slice(t, m, p.p.qty.unsigned_abs(), price, idx, reason, 0, long);
        }
    }

    /// Closes `qty` of the position in `m` at `price`; emits Close + commission; credits the balance.
    #[allow(clippy::too_many_arguments)]
    fn close_slice(&mut self, t: i64, m: u16, qty: u64, price: u64, idx: usize, reason: CloseReason, intent_id: u32, long: bool) {
        let p = self.pos[&m];
        let out = engine::apply_fill(Some(p.p), eside(!long), qty, price, t, engine::day_index(t)).expect("close");
        let slice = out.closed.expect("slice");
        let d = engine::apply_closed_slice(&self.st, &ERules::default(), slice.credit, out.quick_tally_delta).expect("slice");
        self.st = d.state;
        self.trades += 1;
        self.push(
            JournalEvent::Close {
                market: m,
                side: if long { Side::Long } else { Side::Short },
                qty,
                entry_e12: slice.entry_e12,
                exit_e12: slice.exit_e12,
                time: t,
                timer_start: p.p.timer_start,
                pnl: slice.pnl,
                quick: slice.quick,
                credited: slice.credit,
                reason,
                intent_id,
                proof: 0,
            },
            Some(idx),
        );
        match out.position {
            Some(np) => {
                self.pos.insert(m, GPos { p: np, ..p });
            }
            None => {
                self.pos.remove(&m);
            }
        }
        self.push_commission(m, qty, price, t);
        if d.fail.is_some() {
            // quick-limit fail
            let b_after = self.st.b;
            self.push(JournalEvent::Fail { time: t, reason: FailReason::PhaseQuick, b_after, proof: 0 }, Some(idx));
            self.terminal = true;
        }
    }

    fn protective(&mut self, market: u16, t: i64, idx: usize) {
        let Some(p) = self.pos.get(&market).copied() else { return };
        let (bid, ask) = self.bid_ask(market, idx);
        let long = p.p.qty > 0;
        let open = self.market_open(market, t);
        if let Some(trig) = engine::check_protective(eside(long), p.sl, p.tp, bid, ask, open) {
            let reason = match trig.kind {
                engine::ProtectiveKind::StopLoss => CloseReason::StopLoss,
                engine::ProtectiveKind::TakeProfit => CloseReason::TakeProfit,
            };
            self.close_slice(t, market, p.p.qty.unsigned_abs(), trig.price_e12, idx, reason, 0, long);
            self.after_close(t, idx);
        }
    }

    fn after_close(&mut self, t: i64, idx: usize) {
        if self.pos.is_empty() && self.st.tight_on && !self.terminal {
            self.push(JournalEvent::TightEnd { time: t }, None);
            self.st = engine::check_tight_end(&self.st).expect("end");
        }
        let _ = idx;
    }

    fn pending_orders(&mut self, market: u16, t: i64, idx: usize) {
        let (bid, ask) = self.bid_ask(market, idx);
        let open = self.market_open(market, t);
        let locked = !engine::can_increase_exposure(&self.st, t);
        let mut fired = vec![];
        for (i, pd) in self.pending.iter().enumerate() {
            if pd.intent.market != market {
                continue;
            }
            let kind = if pd.intent.kind == IntentKind::Limit { engine::PendingKind::Limit } else { engine::PendingKind::Stop };
            let side = if pd.intent.side == 0 { ESide::Buy } else { ESide::Sell };
            if let Some(price) = engine::check_pending(side, kind, pd.intent.price_e12, bid, ask, open, locked) {
                fired.push((i, price));
            }
        }
        for (i, price) in fired.into_iter().rev() {
            let pd = self.pending.remove(i);
            let ok = if pd.intent.kind == IntentKind::Limit { OrderKind::Limit } else { OrderKind::Stop };
            self.execute(t, idx, pd.nonce, &pd.intent, Some(price), ok);
        }
    }

    // ---- orders

    fn on_order(&mut self, t: i64, intent: OrderIntent) {
        let nonce = self.next_nonce;
        self.next_nonce += 1;
        let rec = sign(nonce, &intent);
        self.intents.push(rec);
        if self.terminal {
            return;
        }
        match intent.kind {
            IntentKind::Market => {
                let idx = self.latest[&intent.market];
                self.execute(t, idx, nonce, &intent, None, OrderKind::Market);
            }
            IntentKind::Limit | IntentKind::Stop => self.pending.push(Pending { nonce, intent }),
            IntentKind::Close => {
                let m = intent.market;
                if let Some(p) = self.pos.get(&m).copied() {
                    let idx = self.latest[&m];
                    let (bid, ask) = self.bid_ask(m, idx);
                    let long = p.p.qty > 0;
                    let qty = if intent.qty == 0 { p.p.qty.unsigned_abs() } else { intent.qty };
                    self.close_slice(t, m, qty, if long { bid } else { ask }, idx, CloseReason::Manual, nonce, long);
                    self.after_close(t, idx);
                }
            }
            IntentKind::Cancel => self.pending.retain(|p| p.nonce != intent.target_nonce),
            IntentKind::SetSlTp => {
                if let Some(p) = self.pos.get_mut(&intent.market) {
                    p.sl = intent.sl_e12;
                    p.tp = intent.tp_e12;
                }
            }
        }
        self.flush_batch(t);
    }

    /// Executes a market/limit/stop order at the latest sample (`price` overrides for triggered pending orders).
    fn execute(&mut self, t: i64, idx: usize, nonce: u32, intent: &OrderIntent, price: Option<u64>, kind: OrderKind) {
        let m = intent.market;
        let long = intent.side == 0;
        let (bid, ask) = self.bid_ask(m, idx);
        let px = price.unwrap_or(if long { ask } else { bid });
        let mut remaining = intent.qty;
        if let Some(p) = self.pos.get(&m).copied() {
            let pos_long = p.p.qty > 0;
            if pos_long != long {
                let c = remaining.min(p.p.qty.unsigned_abs());
                let flip = remaining > p.p.qty.unsigned_abs();
                self.close_slice(t, m, c, px, idx, if flip { CloseReason::Flip } else { CloseReason::Manual }, nonce, pos_long);
                remaining -= c;
            }
        }
        if remaining > 0 {
            let existing = self.pos.get(&m).copied();
            let out = engine::apply_fill(existing.map(|p| p.p), eside(long), remaining, px, t, engine::day_index(t)).expect("fill");
            let np = out.position.expect("position");
            let lev = self.spec.leverage[usize::from(self.spec.market(m).class)];
            let margin = engine::margin_micro(np.qty.unsigned_abs(), np.avg_entry_e12, lev).expect("margin");
            let (osl, otp) = existing.map(|p| (p.sl, p.tp)).unwrap_or((0, 0));
            self.pos.insert(
                m,
                GPos { p: np, sl: if intent.sl_e12 != 0 { intent.sl_e12 } else { osl }, tp: if intent.tp_e12 != 0 { intent.tp_e12 } else { otp } },
            );
            self.push(
                JournalEvent::Fill {
                    market: m,
                    side: if long { Side::Long } else { Side::Short },
                    order_kind: kind,
                    qty: remaining,
                    price_e12: px,
                    time: t,
                    avg_entry_after: np.avg_entry_e12,
                    margin: u64::try_from(margin).expect("margin"),
                    sl: intent.sl_e12,
                    tp: intent.tp_e12,
                    intent_id: nonce,
                    proof: 0,
                },
                Some(idx),
            );
            self.push_commission(m, remaining, px, t);
        } else {
            self.after_close(t, idx);
        }
    }
}

// -------------------------------------------------------------------------------------------- serialisation

impl Bundle {
    pub fn archive_bytes(&self) -> Vec<u8> {
        let mut w = ArchiveWriter::new();
        for s in &self.samples {
            w.push(s);
        }
        w.into_bytes()
    }

    pub fn stream(&self) -> Vec<u8> {
        self.samples.iter().flat_map(|s| s.to_bytes()).collect()
    }

    pub fn intents_text(&self) -> String {
        let mut t = intents::header_line();
        for r in &self.intents {
            t.push_str(&intents::line_for(r.nonce, &r.order, &r.sig));
        }
        t
    }

    pub fn intents_prefix_len(&self, lines: usize) -> usize {
        let mut t = intents::header_line();
        for r in self.intents.iter().take(lines) {
            t.push_str(&intents::line_for(r.nonce, &r.order, &r.sig));
        }
        t.len()
    }

    /// Recomputes every Meta's `samples_hash` and `intents_hash` from the current archive and intents (a consistent
    /// journal for the CURRENT files).
    pub fn refresh_commitments(&mut self) {
        use sha2::{Digest, Sha256};
        let stream = self.stream();
        let text = self.intents_text();
        let metas = self.metas.clone();
        for m in metas {
            let s: [u8; 32] = Sha256::digest(&stream[..(m.n_samples * RECORD_LEN).min(stream.len())]).into();
            let plen = self.intents_prefix_len(m.n_intent_lines);
            let i: [u8; 32] = Sha256::digest(&text.as_bytes()[..plen.min(text.len())]).into();
            if let GLine::Event { ev: JournalEvent::Meta { samples_hash, intents_hash, .. }, .. } = &mut self.lines[m.line] {
                *samples_hash = s;
                *intents_hash = i;
            }
        }
    }

    pub fn events_text(&self) -> String {
        let mut t = log::header_line();
        for l in &self.lines {
            match l {
                GLine::Opened(o) => t.push_str(&log::opened_line(o)),
                GLine::Event { seq, ev, proof } => {
                    let pb = proof.as_ref().map(|p| p.to_bytes().expect("proof")).unwrap_or_default();
                    t.push_str(&log::event_line(&account_key(), *seq, &ev.to_bytes(), &pb, 1000 + *seq));
                }
                GLine::Raw(s) => {
                    t.push_str(s);
                    t.push('\n');
                }
            }
        }
        t
    }

    /// The head of the hash chain over the events as they stand.
    pub fn head(&self) -> [u8; 32] {
        let mut h = journal_types::genesis_hash(&NativeSha256, &account_key());
        for l in &self.lines {
            if let GLine::Event { seq, ev, proof } = l {
                if let Ok(n) = journal_types::next_hash(&NativeSha256, &h, *seq, ev, proof.as_ref()) {
                    h = n;
                }
            }
        }
        h
    }

    pub fn last_seq(&self) -> u64 {
        self.lines.iter().filter_map(|l| if let GLine::Event { seq, .. } = l { Some(*seq) } else { None }).next_back().unwrap_or(0)
    }

    pub fn snapshot_json(&self) -> String {
        let s = &self.final_state;
        serde_json::to_string(&json!({"version": 1, "b": s.b, "q_tally": s.q_tally, "latch": u8::from(s.latch), "tight_on": u8::from(s.tight_on),
            "r": s.r, "g_peak": s.g_peak, "l_level": s.l_level, "lock_until": s.lock_until, "seq": self.last_seq(),
            "journal_hash": hexutil::encode(&self.head()), "flat": u8::from(s.flat), "phase": self.final_phase, "status": 0, "trades_closed": self.final_trades}))
        .expect("json")
    }

    pub fn files(&self) -> Files {
        Files {
            events: self.events_text().into_bytes(),
            rules: self.spec.rules_json().into_bytes(),
            archive: self.archive_bytes(),
            intents: self.intents_text().into_bytes(),
            snapshot: Some(self.snapshot_json().into_bytes()),
            second: None,
        }
    }

    /// Index of the first event line satisfying `f`.
    pub fn find(&self, f: impl Fn(&JournalEvent) -> bool) -> usize {
        self.lines.iter().position(|l| matches!(l, GLine::Event { ev, .. } if f(ev))).expect("event not found")
    }

    pub fn ev_mut(&mut self, line: usize) -> &mut JournalEvent {
        match &mut self.lines[line] {
            GLine::Event { ev, .. } => ev,
            _ => panic!("not an event line"),
        }
    }

    /// Renumbers the `seq` of every event line after an insertion or removal (so the tamper is only what the test wants).
    pub fn renumber(&mut self) {
        let mut n = 0;
        for l in &mut self.lines {
            if let GLine::Event { seq, .. } = l {
                n += 1;
                *seq = n;
            }
        }
    }

    pub fn events(&self) -> Vec<&JournalEvent> {
        self.lines.iter().filter_map(|l| if let GLine::Event { ev, .. } = l { Some(ev) } else { None }).collect()
    }
}

pub struct Files {
    pub events: Vec<u8>,
    pub rules: Vec<u8>,
    pub archive: Vec<u8>,
    pub intents: Vec<u8>,
    pub snapshot: Option<Vec<u8>>,
    pub second: Option<Vec<u8>>,
}

impl Files {
    pub fn inputs(&self) -> setl8_verify::Inputs<'_> {
        setl8_verify::Inputs {
            events: &self.events,
            rules: &self.rules,
            archive: &self.archive,
            intents: &self.intents,
            snapshot: self.snapshot.as_deref(),
            second_archive: self.second.as_deref(),
            as_of_seq: None,
        }
    }
}

pub fn verify(b: &Bundle) -> setl8_verify::Report {
    let f = b.files();
    setl8_verify::verify_account(&f.inputs()).expect("inputs parse")
}

pub fn verify_no_snapshot(b: &Bundle) -> setl8_verify::Report {
    let mut f = b.files();
    f.snapshot = None;
    setl8_verify::verify_account(&f.inputs()).expect("inputs parse")
}

// -------------------------------------------------------------------------------------------- scenarios

/// DESIGN 4.9: S = $100k, EURUSD half-spread 30 e6, fill 1,000,000 EUR at 14:00:00, tight start at 14:01:00, raise at
/// 14:01:30, nothing at 14:02:00, breach at 14:02:30. Samples every 30 s (the table's rows) plus the order.
pub fn design_4_9() -> Bundle {
    let spec = Spec::base();
    let mid = |p: u64| p * 100_000_000; // 1.0800 -> 1.08e12
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: mid(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 1_000_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: mid(10_806) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: mid(10_813) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: mid(10_831) },
        Script::Sample { market: 0, t: T_1400 + 120, mid: mid(10_811) },
        Script::Sample { market: 0, t: T_1400 + 150, mid: mid(10_780) },
    ];
    Gen::new(spec).run(&script)
}


/// `1.0800` -> `1.08e12`: prices with four decimals as e12 integers.
pub fn eur(p4: u64) -> u64 {
    p4 * 100_000_000
}

/// Buy 300,000 EUR at 21:00:00, hold through the 22:00 rollover (holding fee), close at 22:30:00.
pub fn fee_case(commission: u32) -> Bundle {
    let mut spec = Spec::base();
    spec.commission[0] = commission;
    let t_open = T_1400 + 7 * HOUR;
    let t_roll = T_1400 + 8 * HOUR;
    let t_close = t_roll + HOUR / 2;
    let script = vec![
        Script::Sample { market: 0, t: t_open, mid: eur(10_800) },
        Script::Order { t: t_open, intent: market_order(0, true, 300_000_000_000, t_open) },
        Script::Sample { market: 0, t: t_roll, mid: eur(10_810) },
        Script::Sample { market: 0, t: t_close, mid: eur(10_820) },
        Script::Order { t: t_close, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, t_close) },
    ];
    Gen::new(spec).run(&script)
}

/// Buy 200,000 EUR at 14:00:00 and close it `age` seconds later with a profit (age <= 45 is a quick slice).
pub fn close_case(age: i64) -> Bundle {
    let spec = Spec::base();
    let t2 = T_1400 + age;
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: t2, mid: eur(10_830) },
        Script::Order { t: t2, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, t2) },
    ];
    Gen::new(spec).run(&script)
}

/// An account with `n` Meta-only batches (no trading): the smallest journal, for chain and sequence tests.
pub fn meta_only(n: u64) -> Bundle {
    let spec = Spec::base();
    let mut g = Gen::new(spec);
    let mut lines = std::mem::take(&mut g.lines);
    let mut metas = vec![];
    for i in 1..=n {
        let t = T_1400 + i64::try_from(i).unwrap();
        metas.push(MetaCommit { line: lines.len(), n_samples: 0, n_intent_lines: 0 });
        lines.push(GLine::Event { seq: i, ev: JournalEvent::Meta { time: t, flat: true, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: engine::ENGINE_VERSION }, proof: None });
    }
    g.lines = lines;
    g.metas = metas;
    g.seq = n;
    g.finish()
}

pub fn classes(r: &setl8_verify::Report) -> Vec<setl8_verify::FindingClass> {
    r.classes()
}

// -------------------------------------------------------------------------------------------- vector files

pub fn vector_dir(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors").join(name)
}

/// (name, bundle) of every committed vector set.
pub fn vector_sets() -> Vec<(&'static str, Bundle)> {
    vec![("design_4_9", design_4_9()), ("quick_profit", close_case(20)), ("holding_night", fee_case(20))]
}

/// The files of a vector set, in order: (file name, bytes).
pub fn vector_files(b: &Bundle) -> Vec<(&'static str, Vec<u8>)> {
    let f = b.files();
    vec![("events.jsonl", f.events), ("rules.json", f.rules), ("archive.bin", f.archive), ("intents.jsonl", f.intents), ("snapshot.json", f.snapshot.unwrap())]
}


// -------------------------------------------------------------------------------------------- stage 2 scenarios

/// Linear interpolation (integer) between waypoints `(second offset, value)`, with a 1-unit wobble so the price changes
/// on every sample (a price that stands still for more than `max_age_secs` closes the market).
pub fn path(points: &[(i64, u64)], k: i64) -> u64 {
    let wobble = u64::from(k.rem_euclid(2) == 1) * 7;
    for w in points.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if k >= t0 && k <= t1 {
            let (a, b) = (i128::from(v0), i128::from(v1));
            let v = a + (b - a) * i128::from(k - t0) / i128::from(t1 - t0);
            return u64::try_from(v).expect("v") + wobble;
        }
    }
    points.last().expect("points").1 + wobble
}

/// A two-market account over 3.5 hours (20:00 to 23:30): a quick trade, a limit order that is cancelled and one that
/// fills, a stop-loss on BTC, a take-profit on EUR with a tight mode and a raise, a holding-fee night, a set_sl_tp, a
/// second tight mode with a raise and a breach, commissions throughout. Samples every 10 s for both markets.
pub fn honest_long() -> Bundle {
    let mut spec = Spec::base();
    spec.commission[0] = 20;
    spec.commission[6] = 50;
    spec.markets.push(MarketSpec { id: 1, class: 6, source: 2, symbol: "BTCUSD", spread_e6: 50, max_age_secs: 60, sessions: vec![(0, 10_080)] });
    let t0 = T_1400 + 6 * HOUR;
    let eur_path = [(0, eur(10_800)), (300, eur(10_800)), (320, eur(10_810)), (400, eur(10_800)), (1_200, eur(10_790)), (1_500, eur(10_800)), (2_400, eur(10_800)),
        (3_000, eur(10_850)), (3_600, eur(10_912)), (5_400, eur(10_900)), (6_300, eur(10_950)), (9_000, eur(10_980)), (10_800, eur(10_750)), (12_600, eur(10_740))];
    let btc_path = [(0, 60_000 * E12), (600, 60_000 * E12), (1_800, 58_900 * E12), (12_600, 59_000 * E12)];
    let orders: Vec<(i64, OrderIntent)> = vec![
        (300, market_order(0, true, 200_000_000_000, t0 + 300)),
        (320, market_order(0, false, 200_000_000_000, t0 + 320)),
        (600, order(IntentKind::Market, 1, 0, 500_000, 0, 59_000 * E12, 0, 0, t0 + 600)),
        (720, order(IntentKind::Limit, 0, 1, 50_000_000_000, eur(11_200), 0, 0, 0, t0 + 720)),
        (840, order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, 4, t0 + 840)),
        (900, order(IntentKind::Limit, 0, 0, 100_000_000_000, eur(10_795), 0, 0, 0, t0 + 900)),
        (2_400, order(IntentKind::Market, 0, 0, 300_000_000_000, 0, 0, eur(10_900), 0, t0 + 2_400)),
        (5_400, market_order(0, true, 400_000_000_000, t0 + 5_400)),
        (5_460, order(IntentKind::SetSlTp, 0, 0, 0, 0, eur(10_700), 0, 0, t0 + 5_460)),
    ];
    let mut script = vec![];
    for k in (0..=12_600).step_by(10) {
        let t = t0 + k;
        script.push(Script::Sample { market: 0, t, mid: path(&eur_path, k) });
        script.push(Script::Sample { market: 1, t, mid: path(&btc_path, k) });
        for (off, it) in &orders {
            if *off == k {
                script.push(Script::Order { t, intent: *it });
            }
        }
    }
    Gen::new(spec).run(&script)
}

/// Solves `bid_from_mid(mid, h) == target` for the mid (exists for every target: the bid steps by <= 1 per unit of mid).
pub fn mid_for_bid(target_bid: u64, h: u32) -> u64 {
    let m0 = u64::try_from(u128::from(target_bid) * 1_000_000 / u128::from(1_000_000 - h)).expect("m");
    for m in m0.saturating_sub(3)..m0 + 3 {
        if engine::bid_from_mid(m, h).expect("bid") == target_bid {
            return m;
        }
    }
    panic!("no mid for bid {target_bid}");
}

/// 1,000,000 EUR bought at 14:00:00 (mid 1.0800, ask 1.0800324); the account then sees `mids` (one sample per 30 s from
/// 14:00:30); the journal is whatever an honest evaluator makes of it; a heartbeat Meta follows at the last sample.
pub fn eur_run(spec: Spec, mids: &[u64]) -> Bundle {
    let mut script = vec![Script::Sample { market: 0, t: T_1400, mid: eur(10_800) }, Script::Order { t: T_1400, intent: market_order(0, true, 1_000_000_000_000, T_1400) }];
    let mut t = T_1400;
    for m in mids {
        t += 30;
        script.push(Script::Sample { market: 0, t, mid: *m });
    }
    script.push(Script::Heartbeat { t });
    Gen::new(spec).run(&script)
}

pub const ASK_1_0800: u64 = 1_080_032_400_000;
pub const BID_H30: u32 = 30;

/// The 4.9 path then a recovery, with a heartbeat: the breach sample is the 5th (14:02:30) and the price recovers after.
pub fn breach_then_recovery() -> Bundle {
    eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_780), eur(10_830)])
}

/// A pass case: phase 2 account, 1,000,000 EUR bought at 14:00:00 and closed at 14:20:00 for a profit of exactly
/// `8,000.000000 + delta` micro-USD (a non-quick slice, no commission), then flat; the closed balance is `1.08 S + delta`.
pub fn pass_case(delta: i64) -> Bundle {
    pass_case_in_phase(delta, 2)
}

pub fn pass_case_in_phase(delta: i64, phase: u8) -> Bundle {
    let mut spec = Spec::base();
    spec.phase = phase;
    let target_bid = u64::try_from(i64::try_from(ASK_1_0800).unwrap() + 8_000_000_000 + delta).unwrap();
    let close_mid = mid_for_bid(target_bid, BID_H30);
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 1_000_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 600, mid: eur(10_850) },
        Script::Sample { market: 0, t: T_1400 + 1_200, mid: close_mid },
        Script::Order { t: T_1400 + 1_200, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 1_200) },
    ];
    Gen::new(spec).run(&script)
}

impl Bundle {
    /// The chain head after the event with sequence number `seq`.
    pub fn head_at(&self, seq: u64) -> [u8; 32] {
        let mut h = journal_types::genesis_hash(&NativeSha256, &account_key());
        for l in &self.lines {
            if let GLine::Event { seq: s, ev, proof } = l {
                if *s > seq {
                    break;
                }
                h = journal_types::next_hash(&NativeSha256, &h, *s, ev, proof.as_ref()).expect("hash");
            }
        }
        h
    }

    /// Appends a `pass_requested` line for the current end of the journal (b, seq, head as the chain has them).
    pub fn push_pass_requested(&mut self, phase: u8, amount: u64, b: i64, seq: u64, time: i64) {
        let h = self.head_at(seq);
        self.lines.push(GLine::Raw(format!(
            "{{\"type\":\"pass_requested\",\"account\":\"{}\",\"phase\":{phase},\"amount\":{amount},\"b\":{b},\"seq\":{seq},\"journal_hash\":\"{}\",\"time\":{time}}}",
            setl8_verify::log::base58(&account_key()),
            hexutil::encode(&h)
        )));
    }

    pub fn push_pass_confirmed(&mut self, phase: u8, amount: u64, b: i64, seq: u64, eligible_at: i64) {
        let h = self.head_at(seq);
        self.lines.push(GLine::Raw(format!(
            "{{\"type\":\"pass_confirmed\",\"account\":\"{}\",\"phase\":{phase},\"amount\":{amount},\"b\":{b},\"seq\":{seq},\"journal_hash\":\"{}\",\"eligible_at\":{eligible_at}}}",
            setl8_verify::log::base58(&account_key()),
            hexutil::encode(&h)
        )));
    }

    pub fn push_raw(&mut self, ty_fields: &str) {
        self.lines.push(GLine::Raw(format!("{{{ty_fields},\"account\":\"{}\"}}", setl8_verify::log::base58(&account_key()))));
    }

    /// Removes the lines at the given indexes, keeps the Meta commit table pointing at the right lines, and renumbers.
    pub fn remove_lines(&mut self, idx: &[usize]) {
        let set: std::collections::HashSet<usize> = idx.iter().copied().collect();
        let mut map = vec![None; self.lines.len()];
        let mut n = 0usize;
        for (i, slot) in map.iter_mut().enumerate() {
            if !set.contains(&i) {
                *slot = Some(n);
                n += 1;
            }
        }
        let old = std::mem::take(&mut self.lines);
        self.lines = old.into_iter().enumerate().filter(|(i, _)| !set.contains(i)).map(|(_, l)| l).collect();
        let metas = std::mem::take(&mut self.metas);
        self.metas = metas.into_iter().filter_map(|m| map[m.line].map(|line| MetaCommit { line, ..m })).collect();
        self.renumber();
    }
}


pub fn verify_second(a: &Bundle, b_archive: Vec<u8>) -> setl8_verify::Report {
    let mut f = a.files();
    f.snapshot = None;
    f.second = Some(b_archive);
    setl8_verify::verify_account(&f.inputs()).expect("inputs parse")
}

/// The mid at which a 1,000,000 EUR long bought at ask 1.0800324 has an open profit of exactly `pnl` micro-USD.
pub fn mid_for_pnl(pnl: i64) -> u64 {
    mid_for_bid(u64::try_from(i64::try_from(ASK_1_0800).unwrap() + pnl).unwrap(), BID_H30)
}

pub fn last_event_time(b: &Bundle) -> i64 {
    b.events().iter().map(|e| e.time()).max().unwrap()
}
