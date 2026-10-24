//! One account's evaluator as the rehearsal drives it: the reference evaluator (`crates/evaluator`), the trader's signing key, a price
//! book and a virtual clock. A story is Rust code that moves the clock, feeds prices, signs orders, and at its posting points
//! hands the drained batches to a [`Net`] (the real cluster, or nothing at all in the unit tests).
//!
//! Time is the cluster's: a story is built on `t0 = the account's creation second`, and `Net::post` waits until the cluster clock
//! has reached the batch (the program refuses an event dated more than 2 s ahead of the block time, I-3).
use std::collections::BTreeMap;

use ed25519_dalek::Signer;
use evaluator::codec::{signing_message, IntentKind, OrderIntent};
use evaluator::{
    AccountParams, Batch, BatchPolicy, ChainView, Evaluator, IntentVerifier, PassDecision, PassReject, ReplayCheck, RulesCfg, SignedIntent, Status,
};

/// Which account a transaction is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcctRef {
    pub name: String,
    pub trader: anchor_lang::solana_program::pubkey::Pubkey,
    pub challenge_id: u64,
}

/// The cluster as a story needs it. Every method may block (waiting for the cluster clock) and fails with a plain message.
pub trait Net {
    /// Posts a drained batch (`post_journal` or `report_fail`), waiting until the cluster clock has reached its `Meta`.
    fn post(&mut self, a: &AcctRef, b: &Batch) -> Result<(), String>;
    /// The stored account as the pass check sees it.
    fn view(&mut self, a: &AcctRef) -> Result<ChainView, String>;
    /// The trader presses the button at second `t`. `Ok(false)` = the program refused (target not reached, not flat, tight mode on).
    fn request_advance(&mut self, a: &AcctRef, t: i64) -> Result<bool, String>;
    fn confirm_pass(&mut self, a: &AcctRef, t: i64) -> Result<(), String>;
    fn reject_pass(&mut self, a: &AcctRef, t: i64, reason: u8) -> Result<(), String>;
    /// `hold_account` at second `t`.
    fn hold(&mut self, a: &AcctRef, t: i64) -> Result<(), String>;
    /// Whether the pass requested at second `t` was already done by an earlier run (a resumed run replays its stories: the evaluator
    /// must decide on its OWN view then, because the chain has moved on).
    fn pass_done(&self, _a: &AcctRef, _t: i64) -> bool {
        false
    }
}

pub struct Ed25519Check;
impl IntentVerifier for Ed25519Check {
    fn verify(&self, trader: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(pk) = ed25519_dalek::PublicKey::from_bytes(trader) else { return false };
        let Ok(sig) = ed25519_dalek::Signature::from_bytes(sig) else { return false };
        pk.verify_strict(message, &sig).is_ok()
    }
}

/// `floor(notional_micro x 1e12 / price_e12)` rounded down to the market's step; at least the minimum.
pub fn qty_for(step: u64, min: u64, notional_micro: i64, price_e12: u64) -> u64 {
    let q = u128::try_from(notional_micro.max(0)).unwrap_or(0) * 1_000_000_000_000u128 / u128::from(price_e12.max(1));
    let q = u64::try_from(q).unwrap_or(u64::MAX);
    let q = q / step.max(1) * step.max(1);
    q.max(min)
}

pub struct Live {
    pub ev: Evaluator,
    pub rules: RulesCfg,
    key: ed25519_dalek::Keypair,
    pub account: [u8; 32],
    nonce: u32,
    /// The story's clock (unix seconds).
    pub t: i64,
    px: BTreeMap<u16, u64>,
    /// The markets sampled at every step, in sampling order (market `k` of the list is sampled `k` seconds after the step's second).
    pub active: Vec<u16>,
    flip: bool,
    pub policy: BatchPolicy,
    pub posted: Vec<Batch>,
    pub trace: Vec<String>,
}

impl Live {
    /// `secret64` is the trader's ed25519 keypair bytes (secret half then public half), as a Solana keypair file holds them.
    pub fn new(rules: RulesCfg, params: AccountParams, secret64: &[u8; 64], start: &[(u16, u64)]) -> Result<Live, String> {
        let key = ed25519_dalek::Keypair::from_bytes(secret64).map_err(|e| format!("trader key: {e}"))?;
        if key.public.to_bytes() != params.trader {
            return Err("the trader key is not the trader of the account".into());
        }
        let account = params.account;
        let t = params.opened_time;
        let ev = Evaluator::new(rules.clone(), params, Box::new(Ed25519Check)).map_err(|e| format!("evaluator config: {e:?}"))?;
        Ok(Live {
            ev,
            rules,
            key,
            account,
            nonce: 1,
            t,
            px: start.iter().copied().collect(),
            active: start.iter().map(|x| x.0).collect(),
            flip: false,
            policy: BatchPolicy::MAX,
            posted: Vec::new(),
            trace: Vec::new(),
        })
    }

    pub fn price(&self, m: u16) -> u64 {
        self.px.get(&m).copied().unwrap_or(0)
    }

    pub fn set_active(&mut self, markets: &[u16]) {
        self.active = markets.to_vec();
    }

    /// One sample of every active market: market `k` of the list at second `t + k`, then the clock is at `t + dt`. A flat price is
    /// nudged by 1 part in 100,000 every other step so the feed keeps changing (a market whose price has not changed for `max_age`
    /// is closed, decision 49).
    pub fn step(&mut self, dt: i64) {
        self.flip = !self.flip;
        for (k, m) in self.active.clone().into_iter().enumerate() {
            let sec = self.t + i64::try_from(k).unwrap_or(0);
            let p = self.price(m);
            let mid = if self.flip { p } else { p + p / 100_000 };
            // the price book keeps the un-nudged level; the sample carries the nudge
            let _ = self.ev.on_sample_mid(m, sec, mid);
        }
        self.t += dt;
    }

    /// `secs` seconds in steps of `dt` with every price moving linearly from where it is to `targets` (markets not listed stay).
    pub fn ramp(&mut self, targets: &[(u16, u64)], secs: i64, dt: i64) {
        let n = (secs / dt).max(1);
        let from: Vec<(u16, u64)> = targets.iter().map(|(m, _)| (*m, self.price(*m))).collect();
        for i in 1..=n {
            for ((m, to), (_, p0)) in targets.iter().zip(&from) {
                let v = i128::from(*p0) + (i128::from(*to) - i128::from(*p0)) * i128::from(i) / i128::from(n);
                self.px.insert(*m, u64::try_from(v.max(1)).unwrap_or(1));
            }
            self.step(dt);
        }
    }

    /// `secs` seconds with the prices where they are.
    pub fn hold_prices(&mut self, secs: i64, dt: i64) {
        for _ in 0..(secs / dt).max(1) {
            self.step(dt);
        }
    }

    /// `price x (1 + ppm / 1e6)`.
    pub fn scaled(&self, m: u16, ppm: i64) -> u64 {
        let p = i128::from(self.price(m));
        u64::try_from((p + p * i128::from(ppm) / 1_000_000).max(1)).unwrap_or(1)
    }

    pub fn qty(&self, m: u16, notional_micro: i64) -> u64 {
        match self.rules.market(m) {
            Some(c) => qty_for(c.qty_step, c.qty_min, notional_micro, self.price(m)),
            None => 0,
        }
    }

    fn sign(&mut self, intent: OrderIntent) -> SignedIntent {
        let nonce = self.nonce;
        self.nonce += 1;
        let order = intent.to_bytes();
        let sig = self.key.sign(&signing_message(&self.account, nonce, &order)).to_bytes();
        SignedIntent { nonce, order, sig }
    }

    fn intent(&self, kind: IntentKind, m: u16, side: u8, qty: u64) -> OrderIntent {
        OrderIntent { kind, market: m, side, qty, price_e12: 0, sl_e12: 0, tp_e12: 0, target_nonce: 0, client_time: self.t }
    }

    /// A signed market order dated at the story's clock. Returns the nonce.
    pub fn market_order(&mut self, m: u16, long: bool, qty: u64) -> Result<u32, String> {
        let s = self.sign(self.intent(IntentKind::Market, m, if long { 0 } else { 1 }, qty));
        self.ev.on_order(&s).map(|_| s.nonce).map_err(|e| format!("market order refused: {e:?}"))
    }

    /// A signed order that closes the whole position of `m`.
    pub fn close_order(&mut self, m: u16) -> Result<u32, String> {
        let s = self.sign(self.intent(IntentKind::Close, m, 0, 0));
        self.ev.on_order(&s).map(|_| s.nonce).map_err(|e| format!("close order refused: {e:?}"))
    }

    /// Closes the seconds up to the clock and returns the batches the queue makes.
    pub fn drain(&mut self) -> Vec<Batch> {
        self.ev.tick(self.t);
        self.ev.drain_batches(&self.policy)
    }

    /// Drains and posts everything the evaluator has queued.
    pub fn flush(&mut self, net: &mut dyn Net, a: &AcctRef) -> Result<usize, String> {
        let batches = self.drain();
        let n = batches.len();
        for b in batches {
            net.post(a, &b)?;
            self.posted.push(b);
        }
        Ok(n)
    }

    /// The chain's view as the evaluator would see it if it had posted everything (used by the offline net).
    pub fn local_view(&self) -> ChainView {
        match self.posted.last() {
            Some(b) => ChainView { seq: b.account_after.seq, journal_hash: b.account_after.journal_hash, b: b.account_after.b, flat: b.account_after.flat, phase: self.ev.phase() },
            None => ChainView { seq: 0, journal_hash: journal_types::genesis_hash(&journal_types::NativeSha256, &self.account), b: self.ev.state().b, flat: true, phase: self.ev.phase() },
        }
    }

    /// The pass flow of one request: post everything, the trader presses the button, the evaluator decides, `confirm_pass` /
    /// `reject_pass`. A phase-1 confirmation starts phase 2 at once; in phases 2 to 5 the pass is left waiting out its delay.
    pub fn pass(&mut self, net: &mut dyn Net, a: &AcctRef) -> Result<PassDecision, String> {
        self.flush(net, a)?;
        let t = self.t;
        let replay = net.pass_done(a, t);
        if !net.request_advance(a, t)? {
            // the evaluator was not asked yet (`request_pass` freezes it), so there is nothing to resolve
            return Err("the program refused request_advance".into());
        }
        let view = if replay { self.local_view() } else { net.view(a)? };
        let dec = self.ev.request_pass(t, &view, ReplayCheck::Skipped);
        match dec {
            PassDecision::Confirm => {
                net.confirm_pass(a, t + 1)?;
                if self.ev.phase() == 1 {
                    let fresh = ChainView { b: self.ev.account().size_micro, flat: true, phase: 2, ..view };
                    self.ev.phase_started(2, &fresh).map_err(|e| format!("the evaluator refused phase_started(2): {e}"))?;
                }
                self.t = t + 2;
            }
            PassDecision::Reject(r) => {
                net.reject_pass(a, t + 1, r as u8)?;
                self.ev.pass_resolved().map_err(|e| format!("the evaluator refused pass_resolved: {e}"))?;
                self.t = t + 2;
            }
        }
        Ok(dec)
    }

    /// The evaluator holds the account for review (DESIGN 2.9): everything posted first, then `hold_account`.
    pub fn hold(&mut self, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
        self.flush(net, a)?;
        net.hold(a, self.t)?;
        self.ev.set_held(true).map_err(|e| format!("the evaluator refused set_held(true): {e}"))?;
        self.t += 1;
        Ok(())
    }

    pub fn failed(&self) -> bool {
        matches!(self.ev.status(), Status::Failed(_))
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.trace.push(s.into());
    }
}

pub fn reject_name(r: PassReject) -> &'static str {
    match r {
        PassReject::NotFlat => "NotFlat",
        PassReject::JournalBehind => "JournalBehind",
        PassReject::TargetNotReached => "TargetNotReached",
        PassReject::Other => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_round_down_to_the_step_and_never_below_the_minimum() {
        // $7,500 of BTC at $60,000 = 0.125 BTC = 125,000 units of 1e-6, step 200
        assert_eq!(qty_for(200, 200, 7_500_000_000, 60_000 * 1_000_000_000_000), 125_000);
        assert_eq!(qty_for(5_000, 5_000, 3_000_000_000, 2_600 * 1_000_000_000_000), 1_150_000);
        assert_eq!(qty_for(200, 200, 1, 60_000 * 1_000_000_000_000), 200, "the minimum");
        assert_eq!(qty_for(200, 200, 7_500_000_000, 60_000 * 1_000_000_000_000 + 1) % 200, 0);
    }
}
