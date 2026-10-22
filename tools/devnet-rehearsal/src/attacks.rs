//! S3: a short list of attacks on the real runtime. Each is LANDED on the cluster (no preflight, so the runtime itself refuses it and
//! the failed transaction is on chain) and must fail with the exact error the LiteSVM suite expects.
use anchor_lang::solana_program::pubkey::Pubkey;
use journal_types::{JournalBatch, JournalEvent, OrderKind, PriceProof, Side};
use setl8_lev_trading::errors::Setl8Error;
use setl8_lev_trading::instructions::{CreateMarketArgs, MarketParams};
use setl8_lev_trading::state::Session;

use crate::ix;
use crate::live::AcctRef;
use crate::rehearsal::Rehearsal;
use crate::vaultview;

/// The id of the market the attacks create AFTER rule set v1: its half-spread in v1 is 0 (decision 64).
pub const LATE_MARKET: u16 = 21;
/// Anchor's `ConstraintSeeds`.
pub const CONSTRAINT_SEEDS: u32 = 2006;
/// The vault's `DuplicateChallengeSize`.
pub const VAULT_DUPLICATE_SIZE: u32 = 6044;

pub fn heartbeat(first_seq: u64, prev_hash: [u8; 32], time: i64) -> JournalBatch {
    JournalBatch {
        first_seq,
        prev_hash,
        events: vec![JournalEvent::Meta { time, flat: true, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION }],
        proofs: vec![],
    }
}

impl Rehearsal {
    fn expect(&mut self, label: &str, ixs: &[anchor_lang::solana_program::instruction::Instruction], payer: &str, signers: &[&str], want: u32, what: &str) {
        match self.send_expect_error(label, ixs, payer, signers, want) {
            Ok(sig) => {
                self.check(true, &format!("{what}: refused with error {want} [{sig}]"));
            }
            Err(e) => {
                self.check(false, &format!("{what}: {e}"));
            }
        }
    }

    pub fn s3(&mut self) -> Result<(), String> {
        self.begin("S3");
        if let Err(e) = self.s3_inner() {
            self.fail_with(&e);
        }
        self.end("non-evaluator post, replayed batch, wrong account, wrong PDA seeds, a Fill in an unpriced market, a 7-day pass delay, a duplicate tier at the vault: each refused by the real runtime with the exact error");
        Ok(())
    }

    fn account_head(&self, a: &AcctRef) -> Result<(u64, [u8; 32], i64), String> {
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        Ok((acct.seq, acct.journal_hash, acct.last_batch_time))
    }

    fn s3_inner(&mut self) -> Result<(), String> {
        let g1 = self.open("G1", "trader7", true)?;
        let g2 = self.open("G2", "trader7", true)?;
        let evaluator = self.keys.pubkey("evaluator");
        let rsv = self.config()?.current_ruleset;
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let keeper = self.keys.pubkey("keeper");

        // --- the late market (for attack 5): created by the admins after rule set v1
        if !self.exists(&self.ids.market(LATE_MARKET)) {
            let mut sessions = [Session::default(); 6];
            sessions[0] = Session { start_min: 0, end_min: 10_080 };
            let args = CreateMarketArgs {
                market_id: LATE_MARKET,
                class: 6,
                params: MarketParams { status: 1, feed_id: [0x21; 32], max_age_secs: 60, qty_step: 1_000, qty_min: 1_000, max_notional_usd: 1_000_000_000_000, session_count: 1, sessions, quote_conv_market: setl8_lev_trading::constants::NO_QUOTE_CONV },
            };
            let ixn = ix::create_market(&self.ids, &keeper, &x, &y, &args);
            self.send("create_market 21", &[ixn], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
        }

        // --- 3. a batch built for G1 posted to G2 (both at sequence 0): the genesis hash binds the account
        let (s1, h1, _) = self.account_head(&g1)?;
        let (s2, h2, _) = self.account_head(&g2)?;
        if !self.st.is_done("S3:wrong-account") {
            let now = self.c.refresh_clock().map_err(|e| e.to_string())?;
            let b1 = heartbeat(s1 + 1, h1, now);
            self.check(s1 == 0 && s2 == 0 && h1 != h2, "G1 and G2 are both at sequence 0 with different genesis hashes");
            let ixn = ix::post_journal(&self.ids, &evaluator, &g2.trader, g2.challenge_id, rsv, b1.to_bytes().map_err(|e| format!("{e:?}"))?, &[]);
            self.expect("post_journal G2 (batch of G1)", &[ixn], "evaluator", &[], u32::from(Setl8Error::BadPrevHash), "wrong account: G1's batch posted to G2");
            self.mark_done("S3:wrong-account")?;
        }

        // --- the same batch is valid for G1 ...
        if !self.st.is_done("S3:g1-first") {
            let (s, h, _) = self.account_head(&g1)?;
            let now = self.c.refresh_clock().map_err(|e| e.to_string())?;
            let b = heartbeat(s + 1, h, now);
            self.c.wait_until(now).map_err(|e| e.to_string())?;
            let ixn = ix::post_journal(&self.ids, &evaluator, &g1.trader, g1.challenge_id, rsv, b.to_bytes().map_err(|e| format!("{e:?}"))?, &[]);
            self.send("post_journal G1 heartbeat", &[ixn], "evaluator", &[], false).map_err(|e| e.text)?;
            self.mark_done("S3:g1-first")?;
            // ... and a second time it is a replay: the sequence has moved on
            let ixn = ix::post_journal(&self.ids, &evaluator, &g1.trader, g1.challenge_id, rsv, b.to_bytes().map_err(|e| format!("{e:?}"))?, &[]);
            let before = self.account_head(&g1)?;
            self.expect("post_journal G1 (replay)", &[ixn], "evaluator", &[], u32::from(Setl8Error::BadSequence), "replayed batch: the same bytes a second time");
            self.check(self.account_head(&g1)? == before, "the replay changed nothing");
        }

        // --- 1. a batch signed by someone who is not the evaluator
        if !self.st.is_done("S3:non-evaluator") {
            let (s, h, last) = self.account_head(&g1)?;
            let now = self.c.refresh_clock().map_err(|e| e.to_string())?.max(last);
            let b = heartbeat(s + 1, h, now);
            let attacker = self.keys.pubkey("trader7");
            let ixn = ix::post_journal(&self.ids, &attacker, &g1.trader, g1.challenge_id, rsv, b.to_bytes().map_err(|e| format!("{e:?}"))?, &[]);
            let before = self.account_head(&g1)?;
            self.expect("post_journal G1 (trader signs)", &[ixn], "trader7", &[], u32::from(Setl8Error::Unauthorized), "non-evaluator post_journal: the trader's own key in the evaluator slot");
            self.check(self.account_head(&g1)? == before, "the refused post changed nothing");
            self.mark_done("S3:non-evaluator")?;
        }

        // --- 5. a Fill in a market the account's rule set does not price
        if !self.st.is_done("S3:unpriced") {
            let (s, h, last) = self.account_head(&g1)?;
            let now = self.c.refresh_clock().map_err(|e| e.to_string())?.max(last);
            let mid: u64 = 100 * 1_000_000_000_000;
            let proof = PriceProof { source: 255, feed_id: [0x21; 32], timestamp_ns: u64::try_from(now).unwrap_or(0) * 1_000_000_000, quantized_value: i128::from(mid) * 1_000_000, signed_update: vec![] };
            let fill = JournalEvent::Fill { market: LATE_MARKET, side: Side::Long, order_kind: OrderKind::Market, qty: 10_000, price_e12: mid, time: now, avg_entry_after: mid, margin: 0, sl: 0, tp: 0, intent_id: 1, proof: 0 };
            let meta = JournalEvent::Meta { time: now, flat: false, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION };
            let batch = JournalBatch { first_seq: s + 1, prev_hash: h, events: vec![fill, meta], proofs: vec![proof] };
            let ixn = ix::post_journal(&self.ids, &evaluator, &g1.trader, g1.challenge_id, rsv, batch.to_bytes().map_err(|e| format!("{e:?}"))?, &[LATE_MARKET]);
            let before = self.account_head(&g1)?;
            self.expect("post_journal G1 (unpriced fill)", &[ixn], "evaluator", &[], u32::from(Setl8Error::MarketUnpriced), "a Fill in market 21, created after rule set v1 (half-spread 0): MarketUnpriced");
            self.check(self.account_head(&g1)? == before, "the refused fill changed nothing");
            self.mark_done("S3:unpriced")?;
        }

        // --- 4. a wrong PDA in the sector-authority slot of a vault-calling instruction
        if !self.st.is_done("S3:seeds") {
            let mut ixn = ix::request_advance(&self.ids, &g1.trader, g1.challenge_id);
            ixn.accounts[4].pubkey = Pubkey::new_unique();
            self.expect("request_advance G1 (wrong PDA)", &[ixn], "trader7", &[], CONSTRAINT_SEEDS, "wrong PDA seeds: another key in the sector_authority slot (Anchor ConstraintSeeds)");
            self.mark_done("S3:seeds")?;
        }

        // --- 6. a pass delay of 7 days
        if !self.st.is_done("S3:delay") {
            let ixn = ix::set_limits(&self.ids, &x, &y, 7 * 86_400, 100_000_000_000, 86_400);
            self.expect("set_limits (7 days)", &[ixn], "keeper", &["lev-admin-sl8", "lev-admin-rov"], u32::from(Setl8Error::InvalidLimits), "a pass delay of 7 days (the vault abandons a record idle for 7 days)");
            let cfg = self.config()?;
            self.check(cfg.pass_delay_secs == self.pass_delay, "the refused set_limits changed nothing");
            self.mark_done("S3:delay")?;
        }

        // --- 7. a repeated tier size at the vault
        if !self.st.is_done("S3:duplicate") {
            let mut tiers = self.launch.tiers();
            tiers[1].0 = tiers[0].0;
            let (vx, vy) = (self.keys.pubkey("vault-admin-sl8"), self.keys.pubkey("vault-admin-rov"));
            let ixn = ix::update_product_tiers(&self.ids, &vx, &vy, &tiers, crate::scenarios::FEE_SPLIT_BPS, crate::scenarios::MAX_PAYOUT_COUNT, &self.launch.ladder());
            self.expect("update_product_config (duplicate tier)", &[ixn], "keeper", &["vault-admin-sl8", "vault-admin-rov"], VAULT_DUPLICATE_SIZE, "a repeated tier size at the vault: DuplicateChallengeSize");
            let acc = self.c.account(&self.ids.registry()).map_err(|e| e.to_string())?.ok_or("registry")?;
            let reg = vaultview::parse_registry(&acc.data).ok_or("registry decode")?;
            self.check(reg.tiers == self.launch.tiers(), "the refused update changed nothing: the registry still holds the 10 launch tiers");
            self.mark_done("S3:duplicate")?;
        }
        Ok(())
    }

    pub fn mark_done(&mut self, label: &str) -> Result<(), String> {
        self.st.done.insert(label.to_string());
        self.save()
    }
}
