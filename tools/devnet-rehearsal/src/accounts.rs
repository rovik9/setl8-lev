//! S2: the trader lifecycle on the real runtime. Eight accounts of seven traders:
//!   A1  honest: phase 1 pass, phase 2 pass (confirmed, waiting out the 1-day delay); day 2: touch_activity, finalize_pass (the
//!       vault queues the payout), phase 3, a static-floor fail there and a reset at the third ladder step
//!   B1  static-floor fail in phase 1, `reset_account` (ladder step 1), B2: phase 1 pass, a fail in phase 2, reset (ladder step 2), B3
//!   C1  a hold, then a fail while held: `reset_account` -> ReviewPending, `fail_for_violation`, `reset_account` -> ViolationNoReset
//!   D1  a phase 2 pass confirmed, then vetoed by both admins
//!   E1  the forced close-all after a quick-limit overshoot (decision 69)
//!   F1  a breach (12 h lock) traded with MOCK proofs; day 2: trades again once the lock has run out
use anchor_lang::solana_program::pubkey::Pubkey;
use evaluator::PassDecision;
use journal_types::{CloseReason, FailReason, JournalEvent};

use crate::events::{self, Ev};
use crate::ix;
use crate::live::{AcctRef, Live, Net};
use crate::rehearsal::{usd, Rehearsal};
use crate::stories::{self, BTC, ETH, SOL};
use setl8_lev_trading::errors::Setl8Error;

pub const TIER: u8 = 0;
/// Tier 0: $2,500.
pub const SIZE: i64 = 2_500_000_000;

/// A net for rebuilding an evaluator: it does nothing and answers yes. A resumed run replays each account's stories over it, so the
/// evaluator ends exactly where the first run left it (the stories are deterministic) without a single request to the cluster.
#[derive(Default)]
pub struct ReplayNet;

impl Net for ReplayNet {
    fn post(&mut self, _a: &AcctRef, _b: &evaluator::Batch) -> Result<(), String> {
        Ok(())
    }
    fn view(&mut self, _a: &AcctRef) -> Result<evaluator::ChainView, String> {
        Err("a replay never asks the chain".into())
    }
    fn request_advance(&mut self, _a: &AcctRef, _t: i64) -> Result<bool, String> {
        Ok(true)
    }
    fn confirm_pass(&mut self, _a: &AcctRef, _t: i64) -> Result<(), String> {
        Ok(())
    }
    fn reject_pass(&mut self, _a: &AcctRef, _t: i64, _reason: u8) -> Result<(), String> {
        Ok(())
    }
    fn hold(&mut self, _a: &AcctRef, _t: i64) -> Result<(), String> {
        Ok(())
    }
    fn pass_done(&self, _a: &AcctRef, _t: i64) -> bool {
        true
    }
}

// ------------------------------------------------------------------------------------------------ the histories (generic over the net)

fn want_confirm(d: PassDecision, what: &str) -> Result<(), String> {
    match d {
        PassDecision::Confirm => Ok(()),
        PassDecision::Reject(r) => Err(format!("{what}: the evaluator did not confirm the pass ({})", crate::live::reject_name(r))),
    }
}

/// Phase 1 pass, then a phase 2 pass left waiting out its delay.
pub fn history_a1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::pass_story(l, net, a, BTC.0, SIZE)?;
    want_confirm(l.pass(net, a)?, "phase 1")?;
    stories::pass_story(l, net, a, BTC.0, SIZE)?;
    want_confirm(l.pass(net, a)?, "phase 2")
}

pub fn history_b1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::static_fail_story(l, net, a, BTC.0, SIZE)
}

/// A phase 1 pass, then a fail in phase 2.
pub fn history_b2(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::pass_story(l, net, a, BTC.0, SIZE)?;
    want_confirm(l.pass(net, a)?, "phase 1")?;
    stories::static_fail_story(l, net, a, BTC.0, SIZE)
}

pub fn history_c1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::violation_story(l, net, a, BTC.0, SIZE)
}

pub fn history_d1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    history_a1(l, net, a)
}

pub fn history_e1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::quick_story(l, net, a, SIZE)
}

pub fn history_f1(l: &mut Live, net: &mut dyn Net, a: &AcctRef) -> Result<(), String> {
    stories::breach_story(l, net, a, BTC.0, SIZE)
}

pub const START: [(u16, u64); 3] = [BTC, ETH, SOL];

/// A history function: the stories of one account, generic over the net it posts to.
type History = dyn Fn(&mut Live, &mut dyn Net, &AcctRef) -> Result<(), String>;

impl Rehearsal {
    /// Opens (or finds) the named account of `trader_name`: the purchase is checked to the base unit against the rule set, the vault
    /// split and the 0.4% fee. Returns the account's reference.
    pub fn open(&mut self, name: &str, trader_name: &str, mock: bool) -> Result<AcctRef, String> {
        if let Some(r) = self.st.accounts.get(name) {
            if r.created_at != 0 {
                return self.acct_ref(name);
            }
        }
        let trader = self.keys.pubkey(trader_name);
        let wallet = self.wallet_profile(&trader)?;
        let next_id = wallet.as_ref().map_or(0, |w| w.next_challenge_id);
        // an account the earlier run created but did not record (the process stopped after the transaction)
        if self.exists(&self.ids.acct(&trader, next_id)) {
            self.record_account(name, trader_name, next_id, TIER)?;
            return self.acct_ref(name);
        }
        let cfg = self.config()?;
        let rs = self.ruleset(cfg.current_ruleset)?;
        let cost = rs.tier_cost[usize::from(TIER)];
        let fee = engine::swap_fee_micro(cost, rs.fee_bps).map_err(|e| format!("{e:?}"))?;
        let pay = self.pay_for(trader_name);
        let before = (self.token_balance(&pay.trader_token), self.token_balance(&pay.pool), self.token_balance(&pay.vault_sl8_token), self.token_balance(&pay.fee_token));
        let ixn = ix::open_account(&self.ids, &trader, cfg.current_ruleset, next_id, &pay, TIER);
        let sent = self.send(&format!("open_account {name}"), &[ixn], trader_name, &[], false).map_err(|e| e.text)?;
        self.record_account(name, trader_name, next_id, TIER)?;
        if let Some(r) = self.st.accounts.get_mut(name) {
            r.mock = mock;
        }
        self.save()?;
        let after = (self.token_balance(&pay.trader_token), self.token_balance(&pay.pool), self.token_balance(&pay.vault_sl8_token), self.token_balance(&pay.fee_token));
        let pool_part = u64::try_from(u128::from(cost) * u128::from(crate::scenarios::FEE_SPLIT_BPS) / 10_000).map_err(|e| e.to_string())?;
        self.check(before.0 - after.0 == cost + fee, &format!("{name}: open_account debits the trader exactly {} (price {} + fee {})", usd(cost + fee), usd(cost), usd(fee)));
        self.check(after.1 - before.1 == pool_part && after.2 - before.2 == cost - pool_part, &format!("{name}: the vault split is exact (pool +{}, SL8 +{})", usd(pool_part), usd(cost - pool_part)));
        self.check(after.3 - before.3 == fee, &format!("{name}: the 0.4% fee {} reaches the fee wallet", usd(fee)));
        let acct = self.trading_account(&trader, next_id)?;
        self.check(acct.phase == 1 && acct.b == SIZE && acct.status == 0 && acct.flat == 1 && acct.seq == 0, &format!("{name}: a fresh phase-1 account of {} ({})", usd(SIZE as u64), sent.info.signature));
        let vts = self.c.account(&self.ids.vault_trader_state(&trader, next_id)).map_err(|e| e.to_string())?.and_then(|a| crate::vaultview::parse_trader_state(&a.data));
        self.check(vts.as_ref().is_some_and(|v| v.account_size == acct.size_s && v.status == 0 && v.product_program_id == self.ids.lev), &format!("{name}: the vault holds the record of the challenge"));
        // the Opened event is what the log's first line is made of
        let opened = events::decode(&self.ids.lev, &sent.info).into_iter().any(|e| matches!(e, Ev::Opened(o) if o.account == self.ids.acct(&trader, next_id) && o.size_s == acct.size_s));
        self.check(opened, &format!("{name}: the Opened event is in the transaction's inner instructions"));
        self.acct_ref(name)
    }

    /// `reset_account` of the failed account `prev` into the new account `name` (same trader). Checks the price against the vault's own
    /// function and the ladder, the new account's phase and the vault record.
    pub fn reset(&mut self, prev: &str, name: &str) -> Result<AcctRef, String> {
        if let Some(r) = self.st.accounts.get(name) {
            if r.created_at != 0 {
                return self.acct_ref(name);
            }
        }
        let p = self.acct_ref(prev)?;
        let tn = self.trader_name_of(&p)?;
        let prev_acct = self.trading_account(&p.trader, p.challenge_id)?;
        let wallet = self.wallet_profile(&p.trader)?.ok_or("no wallet profile")?;
        let next_id = wallet.next_challenge_id;
        if self.exists(&self.ids.acct(&p.trader, next_id)) {
            self.record_account(name, &tn, next_id, TIER)?;
            return self.acct_ref(name);
        }
        let cfg = self.config()?;
        let rs = self.ruleset(cfg.current_ruleset)?;
        let ladder = &rs.reset_ladder_bps[..usize::from(rs.ladder_len)];
        let step = usize::from(prev_acct.phase) - 1;
        let price = setl8_shared_interfaces::reset_price(rs.tier_cost[usize::from(TIER)], ladder[step]).ok_or("reset price overflow")?;
        let fee = engine::swap_fee_micro(price, rs.fee_bps).map_err(|e| format!("{e:?}"))?;
        let pay = self.pay_for(&tn);
        let before_trader = self.token_balance(&pay.trader_token);
        let ixn = ix::reset_account(&self.ids, &p.trader, cfg.current_ruleset, p.challenge_id, next_id, &pay);
        let sent = self.send(&format!("reset_account {prev}->{name}"), &[ixn], &tn, &[], false).map_err(|e| e.text)?;
        self.record_account(name, &tn, next_id, TIER)?;
        let after_trader = self.token_balance(&pay.trader_token);
        let ev = events::decode(&self.ids.lev, &sent.info).into_iter().find_map(|e| if let Ev::Reset(r) = e { Some(r) } else { None });
        self.check(
            ev.as_ref().is_some_and(|r| r.price == price && r.fee == fee && r.phase == prev_acct.phase),
            &format!("{name}: AccountReset price {} = ceil(tier cost x ladder[{}] / 10000) = {} x {} bps, fee {}", usd(price), step, usd(rs.tier_cost[usize::from(TIER)]), ladder[step], usd(fee)),
        );
        self.check(before_trader - after_trader == price + fee, &format!("{name}: the trader pays exactly {}", usd(price + fee)));
        let fresh = self.trading_account(&p.trader, next_id)?;
        self.check(fresh.phase == prev_acct.phase && fresh.b == SIZE && fresh.status == 0, &format!("{name}: a fresh account in the failed phase {}", prev_acct.phase));
        self.acct_ref(name)
    }

    fn live(&self, name: &str) -> Result<Live, String> {
        self.live_for(name, &START)
    }

    /// Rebuilds the evaluator of an account by replaying its histories over a [`ReplayNet`].
    fn replay(&self, name: &str, f: &History) -> Result<(Live, AcctRef), String> {
        let a = self.acct_ref(name)?;
        let mut l = self.live(name)?;
        let mut net = ReplayNet;
        f(&mut l, &mut net, &a)?;
        Ok((l, a))
    }

    fn mark(&mut self, label: &str) -> Result<(), String> {
        self.st.done.insert(label.to_string());
        self.save()
    }

    // ------------------------------------------------------------------------------------------------ S2, day 1

    pub fn s2_day1(&mut self) -> Result<(), String> {
        self.begin("S2");
        for (who, f) in [
            ("A1", Rehearsal::run_a1 as fn(&mut Rehearsal) -> Result<(), String>),
            ("B", Rehearsal::run_b),
            ("C1", Rehearsal::run_c1),
            ("D1", Rehearsal::run_d1),
            ("E1", Rehearsal::run_e1),
            ("F1", Rehearsal::run_f1),
        ] {
            if let Err(e) = f(self) {
                self.fail_with(&format!("{who}: {e}"));
            }
        }
        self.end("eight accounts of seven traders on the real runtime: pass, fail + reset ladder, hold + violation, veto, forced close-all, breach");
        Ok(())
    }

    fn run_a1(&mut self) -> Result<(), String> {
        let a = self.open("A1", "trader1", false)?;
        if self.st.is_done("A1:day1") {
            return Ok(());
        }
        let mut l = self.live("A1")?;
        self.say("A1: pass story, phase 1");
        history_a1(&mut l, self, &a)?;
        self.save_account_files("A1", &l)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.phase == 2 && acct.pass_pending == 2, "A1: phase 1 advanced to phase 2 at once; the phase 2 pass is CONFIRMED and waits out its delay (pass_pending 2)");
        self.check(acct.b >= SIZE / 100 * 108 && acct.flat == 1, &format!("A1: balance {} >= 1.08 S, flat", usd(acct.b as u64)));
        let eligible = acct.pass_proposed_at + i64::from(self.config()?.pass_delay_secs);
        self.st.pending_finalize.insert("A1".into(), eligible);
        // while the pass waits: a journal batch is refused (PassPending), the evaluator keeps the vault clock fresh
        self.check_pass_pending_refuses_journal("A1", &a)?;
        let tx = self.touch("A1", &a)?;
        self.check(tx, "A1: touch_activity succeeds while the pass waits (throttled: the vault was told at the request)");
        // finalize before the delay has elapsed is refused
        let vs = self.vault_state();
        let early = ix::finalize_pass(&self.ids, &self.keys.pubkey("keeper"), &a.trader, a.challenge_id, acct.next_request_id, &vs);
        match self.send_expect_error("finalize_pass A1 (early)", &[early], "keeper", &[], u32::from(Setl8Error::PassDelayNotElapsed)) {
            Ok(sig) => {
                self.check(true, &format!("A1: finalize_pass before the delay has elapsed is refused with PassDelayNotElapsed [{sig}]"));
            }
            Err(e) => {
                self.check(false, &e);
            }
        }
        self.mark("A1:day1")?;
        self.say(&format!("A1: the confirmed pass can be finalized from unix second {eligible}"));
        Ok(())
    }

    /// While `pass_pending == 2` the program refuses `post_journal` (PassPending): a heartbeat batch goes in and must be refused.
    fn check_pass_pending_refuses_journal(&mut self, name: &str, a: &AcctRef) -> Result<(), String> {
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        let now = self.c.now().map_err(|e| e.to_string())?;
        let batch = journal_types::JournalBatch {
            first_seq: acct.seq + 1,
            prev_hash: acct.journal_hash,
            events: vec![JournalEvent::Meta { time: now.max(acct.last_batch_time), flat: true, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION }],
            proofs: vec![],
        };
        let bytes = batch.to_bytes().map_err(|e| format!("{e:?}"))?;
        let ixn = ix::post_journal(&self.ids, &self.keys.pubkey("evaluator"), &a.trader, a.challenge_id, acct.ruleset_version, bytes, &[]);
        let want = u32::from(Setl8Error::PassPending);
        match self.send_expect_error(&format!("post_journal {name} (pass pending)"), &[ixn], "evaluator", &[], want) {
            Ok(sig) => {
                self.check(true, &format!("{name}: post_journal while the pass is pending is refused with PassPending ({want}) [{sig}]"));
            }
            Err(e) => {
                self.check(false, &e);
            }
        }
        Ok(())
    }

    fn touch(&mut self, name: &str, a: &AcctRef) -> Result<bool, String> {
        let ixn = ix::touch_activity(&self.ids, &self.keys.pubkey("evaluator"), &a.trader, a.challenge_id);
        Ok(self.send(&format!("touch_activity {name}"), &[ixn], "evaluator", &[], false).is_ok())
    }

    fn run_b(&mut self) -> Result<(), String> {
        let b1 = self.open("B1", "trader2", false)?;
        if !self.st.is_done("B1:day1") {
            let mut l = self.live("B1")?;
            self.say("B1: static-floor fail in phase 1");
            history_b1(&mut l, self, &b1)?;
            self.save_account_files("B1", &l)?;
            let acct = self.trading_account(&b1.trader, b1.challenge_id)?;
            self.check(acct.status == 1 && acct.phase == 1, "B1: report_fail(StaticFloor) ended the account: Failed in phase 1");
            let vts = self.c.account(&self.ids.vault_trader_state(&b1.trader, b1.challenge_id)).map_err(|e| e.to_string())?.and_then(|a| crate::vaultview::parse_trader_state(&a.data));
            self.check(vts.is_some_and(|v| v.status == 2), "B1: the vault record is Failed (flag_trader_failed)");
            self.mark("B1:day1")?;
        }
        let b2 = self.reset("B1", "B2")?;
        if !self.st.is_done("B2:day1") {
            let mut l = self.live("B2")?;
            self.say("B2: phase 1 pass, then a fail in phase 2");
            history_b2(&mut l, self, &b2)?;
            self.save_account_files("B2", &l)?;
            let acct = self.trading_account(&b2.trader, b2.challenge_id)?;
            self.check(acct.status == 1 && acct.phase == 2, "B2: passed phase 1, then Failed in phase 2");
            self.mark("B2:day1")?;
        }
        let b3 = self.reset("B2", "B3")?;
        let acct = self.trading_account(&b3.trader, b3.challenge_id)?;
        self.check(acct.phase == 2 && acct.status == 0, "B3: the reset at ladder step 2 (115%) opens a fresh account IN PHASE 2");
        Ok(())
    }

    fn run_c1(&mut self) -> Result<(), String> {
        let a = self.open("C1", "trader3", false)?;
        if self.st.is_done("C1:day1") {
            return Ok(());
        }
        let tn = "trader3";
        let cfg = self.config()?;
        let pay = self.pay_for(tn);
        if !self.st.is_done("C1:fail") {
            let mut l = self.live("C1")?;
            self.say("C1: hold, then a fail while held");
            history_c1(&mut l, self, &a)?;
            self.save_account_files("C1", &l)?;
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            self.check(acct.status == 1 && acct.held == 1, "C1: report_fail on a held account leaves it Failed + held");
            self.mark("C1:fail")?;
        }
        let next_id = self.wallet_profile(&a.trader)?.map_or(0, |w| w.next_challenge_id);
        let reset = |s: &Rehearsal| ix::reset_account(&s.ids, &a.trader, cfg.current_ruleset, a.challenge_id, next_id, &pay);
        // reset is refused while held
        if !self.st.is_done("C1:review") {
            let ixn = reset(self);
            match self.send_expect_error("reset_account C1 (held)", &[ixn], tn, &[], u32::from(Setl8Error::ReviewPending)) {
                Ok(sig) => {
                    self.check(true, &format!("C1: reset_account on the Failed + held account is refused with ReviewPending (6110) [{sig}]"));
                }
                Err(e) => {
                    self.check(false, &e);
                }
            }
            self.mark("C1:review")?;
        }
        // both admins: fail_for_violation
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        if !self.st.is_done("C1:violation") {
            let ixn = ix::fail_for_violation(&self.ids, &x, &y, &a.trader, a.challenge_id, [0x77; 32]);
            self.send("fail_for_violation C1", &[ixn], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            let wallet = self.wallet_profile(&a.trader)?.ok_or("wallet")?;
            self.check(acct.pass_pending == 3 && acct.held == 0 && acct.status == 1, "C1: fail_for_violation clears the hold and leaves the violation marker (pass_pending 3)");
            self.check(wallet.banned == 1, "C1: the wallet is banned");
            self.mark("C1:violation")?;
        }
        // the ban is checked first: WalletBanned. Only after the admins unban the wallet does the marker itself refuse the reset, for good.
        if !self.st.is_done("C1:banned") {
            let ixn = reset(self);
            match self.send_expect_error("reset_account C1 (banned)", &[ixn], tn, &[], u32::from(Setl8Error::WalletBanned)) {
                Ok(sig) => {
                    self.check(true, &format!("C1: reset_account of the banned wallet is refused with WalletBanned ({}) [{sig}]", u32::from(Setl8Error::WalletBanned)));
                }
                Err(e) => {
                    self.check(false, &e);
                }
            }
            self.mark("C1:banned")?;
        }
        if !self.st.is_done("C1:unban") {
            let ixn = ix::unban_wallet(&self.ids, &x, &y, &a.trader);
            self.send("unban_wallet C1", &[ixn], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
            let wallet = self.wallet_profile(&a.trader)?.ok_or("wallet")?;
            self.check(wallet.banned == 0, "C1: both admins unban the wallet");
            self.mark("C1:unban")?;
        }
        if !self.st.is_done("C1:no-reset") {
            let ixn = reset(self);
            match self.send_expect_error("reset_account C1 (violation)", &[ixn], tn, &[], u32::from(Setl8Error::ViolationNoReset)) {
                Ok(sig) => {
                    self.check(true, &format!("C1: reset_account after the unban is refused with ViolationNoReset (6108): the violation marker is for good [{sig}]"));
                }
                Err(e) => {
                    self.check(false, &e);
                }
            }
            self.mark("C1:no-reset")?;
        }
        self.mark("C1:day1")
    }

    fn run_d1(&mut self) -> Result<(), String> {
        let a = self.open("D1", "trader4", false)?;
        if self.st.is_done("D1:day1") {
            return Ok(());
        }
        let mut l = self.live("D1")?;
        self.say("D1: two passes, the second one vetoed");
        history_d1(&mut l, self, &a)?;
        self.save_account_files("D1", &l)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.phase == 2 && acct.pass_pending == 2, "D1: the phase 2 pass is confirmed (pending 2)");
        let (x, y) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let ixn = ix::veto_pass(&self.ids, &x, &y, &a.trader, a.challenge_id);
        let sent = self.send("veto_pass D1", &[ixn], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.pass_pending == 0 && acct.vetoes == 1 && acct.phase == 2, "D1: both admins vetoed: no pass pending, vetoes 1, same phase and balance");
        self.check(events::decode(&self.ids.lev, &sent.info).iter().any(|e| matches!(e, Ev::Vetoed(_))), "D1: PassVetoed is emitted");
        self.mark("D1:day1")
    }

    fn run_e1(&mut self) -> Result<(), String> {
        let a = self.open("E1", "trader5", false)?;
        if self.st.is_done("E1:day1") {
            return Ok(());
        }
        let mut l = self.live("E1")?;
        self.say("E1: the forced close-all after a quick-limit overshoot (decision 69)");
        history_e1(&mut l, self, &a)?;
        self.save_account_files("E1", &l)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.status == 1 && acct.flat == 1 && acct.q_tally > SIZE / 1000 * 28, "E1: Failed by the quick limit (q_tally above 0.028 S), flat");
        // the failing batch on the real runtime: the closes after the overshoot are Close{Fail}
        let sig = self.st.sigs.iter().rev().find(|s| s.label.starts_with("report_fail") && s.label.ends_with("E1")).map(|s| s.sig.clone()).ok_or("no report_fail for E1")?;
        let tx = self.c.transaction(&sig).map_err(|e| e.to_string())?.ok_or("report_fail transaction")?;
        let evs: Vec<JournalEvent> = events::decode(&self.ids.lev, &tx).into_iter().filter_map(|e| if let Ev::Log(l) = e { JournalEvent::decode(&l.event_bytes).ok() } else { None }).collect();
        let reasons: Vec<CloseReason> = evs.iter().filter_map(|e| if let JournalEvent::Close { reason, .. } = e { Some(*reason) } else { None }).collect();
        let forced = reasons.iter().filter(|r| **r == CloseReason::Fail).count();
        self.check(
            reasons.last() == Some(&CloseReason::Fail) && forced == 1 && reasons.contains(&CloseReason::Manual),
            &format!("E1: the failing batch holds the crossing close(s) and ONE Close{{Fail}} after the overshoot: {reasons:?} ({sig})"),
        );
        self.check(evs.iter().any(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })), "E1: Fail{PhaseQuick} follows the forced close");
        self.mark("E1:day1")
    }

    fn run_f1(&mut self) -> Result<(), String> {
        let a = self.open("F1", "trader6", true)?;
        if self.st.is_done("F1:day1") {
            return Ok(());
        }
        let mut l = self.live("F1")?;
        self.say("F1: breach with mock proofs (12 h lock)");
        history_f1(&mut l, self, &a)?;
        self.save_account_files("F1", &l)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        let breach_at = l.posted.iter().flat_map(|b| b.batch.events.iter()).find_map(|e| if let JournalEvent::Breach { time, .. } = e { Some(*time) } else { None });
        self.check(breach_at.is_some_and(|t| acct.lock_until == t + 12 * 3_600), &format!("F1: Breach at {breach_at:?} locked the account until {} (12 h)", acct.lock_until));
        self.check(acct.status == 0 && acct.flat == 1 && acct.tight_on == 0, "F1: the phase is NOT failed; flat, tight mode ended");
        self.mark("F1:day1")
    }

    // ------------------------------------------------------------------------------------------------ S2, day 2

    /// Everything that needs the clock to have moved: the pass of A1 is finalized, A1 fails in phase 3 and resets, F1 trades after its
    /// lock. Returns false when nothing is due yet.
    pub fn s2_day2(&mut self) -> Result<bool, String> {
        let now = self.c.now().map_err(|e| e.to_string())?;
        let mut worked = false;
        if self.st.pending_finalize.contains_key("A1") && !self.st.is_done("A1:day2") {
            let due = self.st.pending_finalize["A1"];
            if now < due {
                self.say(&format!("A1: the pass can be finalized in {} s (unix second {due}); resume then", due - now));
            } else {
                self.begin("S2-day2");
                if let Err(e) = self.run_a1_day2() {
                    self.fail_with(&format!("A1 day 2: {e}"));
                }
                self.end("day 2: finalize_pass paid by the vault, phase 3, a fail there and the reset at the third ladder step");
                worked = true;
            }
        }
        if self.st.is_done("F1:day1") && !self.st.is_done("F1:day2") {
            let a = self.acct_ref("F1")?;
            let acct = self.trading_account(&a.trader, a.challenge_id)?;
            if now >= acct.lock_until {
                self.begin("S2-lock");
                if let Err(e) = self.run_f1_day2() {
                    self.fail_with(&format!("F1 day 2: {e}"));
                }
                self.end("the 12 h lock ran out on the real clock and the account trades again");
                worked = true;
            } else {
                self.say(&format!("F1: locked until unix second {} ({} s from now)", acct.lock_until, acct.lock_until - now));
            }
        }
        Ok(worked)
    }

    fn run_a1_day2(&mut self) -> Result<(), String> {
        let (mut l, a) = self.replay("A1", &history_a1)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.pass_pending == 2, "A1: the confirmed pass is still waiting");
        // finalize before the delay is refused? it is due now. The evaluator keeps the vault clock fresh first (F-30 duty).
        let t_ok = self.touch("A1", &a)?;
        self.check(t_ok, "A1: touch_activity (the vault is told: the throttle ran out)");
        let vs = self.vault_state();
        let payer = "keeper";
        let ixn = ix::finalize_pass(&self.ids, &self.keys.pubkey(payer), &a.trader, a.challenge_id, acct.next_request_id, &vs);
        let before_tally = self.read_tally();
        let sent = self.send("finalize_pass A1", &[ixn], payer, &[], false).map_err(|e| e.text)?;
        let after = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(after.phase == 3 && after.pass_pending == 0 && after.status == 0 && after.b == SIZE, "A1: finalize_pass starts phase 3 fresh (b = S, nothing pending)");
        let vts = self.ids.vault_trader_state(&a.trader, a.challenge_id);
        let claim = self.c.account(&self.ids.claim(&vts, acct.next_request_id)).map_err(|e| e.to_string())?.and_then(|c| crate::vaultview::parse_claim(&c.data));
        let amount = (SIZE as u64) * 8 / 100;
        self.check(claim.as_ref().is_some_and(|c| c.owed == amount && c.request_id == acct.next_request_id), &format!("A1: the vault queued a payout claim of {} (0.08 S) for request {}", usd(amount), acct.next_request_id));
        let ts = self.c.account(&vts).map_err(|e| e.to_string())?.and_then(|c| crate::vaultview::parse_trader_state(&c.data));
        self.check(ts.is_some_and(|v| v.payout_count == 1), "A1: the vault counts one payout");
        let after_tally = self.read_tally();
        self.check(after_tally.0 == before_tally.0 + 1 && after_tally.1 == before_tally.1 + amount, "A1: the payout tally grew by one payout of 0.08 S");
        self.check(events::decode(&self.ids.lev, &sent.info).iter().any(|e| matches!(e, Ev::Finalized(f) if f.to_phase == 3)), "A1: PassFinalized (to phase 3) is emitted");
        // phase 3: the evaluator starts it, then the account fails on the static floor
        l.ev.phase_started(3);
        let now = self.c.now().map_err(|e| e.to_string())?;
        l.t = l.t.max(now);
        l.ev.tick(l.t);
        stories::static_fail_story(&mut l, self, &a, BTC.0, SIZE)?;
        self.save_account_files("A1", &l)?;
        let failed = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(failed.status == 1 && failed.phase == 3, "A1: Failed in phase 3");
        let a3 = self.reset("A1", "A3")?;
        let fresh = self.trading_account(&a3.trader, a3.challenge_id)?;
        self.check(fresh.phase == 3, "A3: the reset at ladder step 3 (120%) opens a fresh account in phase 3");
        self.mark("A1:day2")
    }

    fn read_tally(&self) -> (u64, u64) {
        match self.c.account(&self.ids.tally()).ok().flatten().and_then(|a| setl8_shared_interfaces::PayoutTally::parse(&a.data).ok()) {
            Some(t) => (t.requested_count, t.requested_total),
            None => (0, 0),
        }
    }

    fn run_f1_day2(&mut self) -> Result<(), String> {
        let (mut l, a) = self.replay("F1", &history_f1)?;
        let now = self.c.now().map_err(|e| e.to_string())?;
        l.t = l.t.max(now);
        l.ev.tick(l.t);
        stories::pass_story(&mut l, self, &a, BTC.0, SIZE)?;
        self.save_account_files("F1", &l)?;
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        self.check(acct.b >= SIZE / 100 * 108 && acct.status == 0, &format!("F1: after the lock the account trades and reaches {}", usd(acct.b as u64)));
        self.mark("F1:day2")
    }
}

/// The pubkey of `Pubkey::default()` is never a trader; kept so the import list stays honest.
#[allow(dead_code)]
fn _unused(_: Pubkey) {}
