//! P30 payout leg (S2-payout): after A1's `finalize_pass` queued a payout claim in the vault, one heartbeat cycle pays it:
//! `reconcile_product`, `begin_heartbeat`, `settle_claims`, `finalize_heartbeat`, all permissionless and sent by the keeper. Then the chain
//! is read back: the claim is closed (or paid down), the trader's token balance rose by what was paid, the vault's books agree with the
//! sector's payout tally.
use anchor_lang::solana_program::pubkey::Pubkey;

use crate::ix;
use crate::rehearsal::{usd, Rehearsal};
use crate::vaultview;

/// Most claims one `settle_claims` call takes (the vault's `MAX_SETTLE_BATCH`).
pub const SETTLE_BATCH: usize = 6;
/// Compute limit of the settle transaction (the pool transfers and the claim closes).
const SETTLE_CU: u32 = 400_000;

/// What the pools are short of to cover every open claim in full (0 when they cover it).
pub fn pool_shortfall(open_claims_total: u64, usdc_pool: u64, usdt_pool: u64) -> u64 {
    open_claims_total.saturating_sub(usdc_pool.saturating_add(usdt_pool))
}

/// What the vault pays one claim in a cycle: `min(floor(owed x min(available, owed_snapshot) / owed_snapshot), live pools)`; the pro-rata
/// ratio is `min(available, owed_snapshot) / owed_snapshot` (1 when the pools cover the queue).
pub fn expected_payment(owed: u64, owed_snapshot: u64, available_snapshot: u64, live_pools: u64) -> u64 {
    if owed_snapshot == 0 {
        return 0;
    }
    let target = u128::from(owed) * u128::from(available_snapshot.min(owed_snapshot)) / u128::from(owed_snapshot);
    u64::try_from(target.min(u128::from(live_pools))).unwrap_or(u64::MAX)
}

/// The claims to settle in batches of `SETTLE_BATCH`.
pub fn batches<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    items.chunks(SETTLE_BATCH).map(<[T]>::to_vec).collect()
}

impl Rehearsal {
    /// Runs the payout leg once A1's claim exists (day 2). Idempotent: a finished payout is not repeated; a cycle left open by an
    /// interrupted run is settled and finalized.
    pub fn payout(&mut self) -> Result<(), String> {
        if self.st.is_done("payout") {
            self.say("payout: already done");
            return Ok(());
        }
        let (Some(claim), Some(request)) = (self.st.addrs.get("claim-A1").cloned(), self.st.addrs.get("claim-A1-request").cloned()) else {
            self.say("payout: A1 has no queued claim yet (finalize_pass is day 2)");
            return Ok(());
        };
        let claim: Pubkey = claim.parse().map_err(|e| format!("{e}"))?;
        let _request: u64 = request.parse().map_err(|e| format!("{e}"))?;
        self.begin("S2-payout");
        if let Err(e) = self.payout_inner(claim) {
            self.fail_with(&format!("payout: {e}"));
        }
        self.end("the payout leg: reconcile, begin_heartbeat, settle_claims, finalize_heartbeat; the claim is paid into the trader's token account");
        Ok(())
    }

    fn vault_view(&self) -> Result<vaultview::VaultState, String> {
        let a = self.c.account(&self.vault_state()).map_err(|e| e.to_string())?.ok_or("the vault state does not exist")?;
        vaultview::parse_vault_state(&a.data).ok_or_else(|| "the vault state is not readable".to_string())
    }

    fn payout_inner(&mut self, claim: Pubkey) -> Result<(), String> {
        let a = self.acct_ref("A1")?;
        let keeper = self.keys.pubkey("keeper");
        let (usdc, usdt) = (self.usdc(), self.usdt());
        let vs_key = self.vault_state();
        let (pool_c, pool_t) = (self.ids.pool(&vs_key, &usdc), self.ids.pool(&vs_key, &usdt));
        let (ta_c, ta_t) = (ix::ata(&a.trader, &usdc), ix::ata(&a.trader, &usdt));

        let claim_before = self.c.account(&claim).map_err(|e| e.to_string())?.and_then(|c| vaultview::parse_claim(&c.data));
        let vs0 = self.vault_view()?;
        if claim_before.is_none() && !vs0.cycle_active {
            return Err("the claim account is gone before the payout (already settled by an earlier run?)".into());
        }
        let owed = claim_before.as_ref().map_or(0, |c| c.owed);
        self.check(claim_before.as_ref().is_some_and(|c| c.trader_wallet == a.trader), &format!("the claim {claim} is owed to A1's trader and queues {} USDC", usd(owed)));
        self.check(vs0.open_claims_count == 1, &format!("the vault holds exactly one open claim ({} open, {} USDC owed)", vs0.open_claims_count, usd(vs0.open_claims_total)));
        if vs0.open_claims_count != 1 {
            return Err("other claims are open: this leg settles only A1's claim".into());
        }

        // the destinations must exist and be usable (both token accounts of the trader)
        if !self.exists(&ta_t) {
            self.send("create_ata trader USDT", &[ix::create_ata_idempotent(&keeper, &a.trader, &usdt)], "keeper", &[], false).map_err(|e| e.text)?;
        }
        if !self.exists(&ta_c) {
            self.send("create_ata trader USDC", &[ix::create_ata_idempotent(&keeper, &a.trader, &usdc)], "keeper", &[], false).map_err(|e| e.text)?;
        }

        // the pool: the purchases' own fee split (and the setup mint) cover the queue; if not, test USDC is minted straight into the pool
        let (bal_c, bal_t) = (self.token_balance(&pool_c), self.token_balance(&pool_t));
        let short = pool_shortfall(vs0.open_claims_total, bal_c, bal_t);
        if short > 0 {
            self.say(&format!("payout: the pools hold {} USDC, {} short of the queue: minting test USDC into the USDC pool with the keeper's mint authority", usd(bal_c + bal_t), usd(short)));
            self.send("mint_to pool (payout shortfall)", &[ix::mint_to(&usdc, &pool_c, &keeper, short)], "keeper", &[], false).map_err(|e| e.text)?;
            self.st.addrs.insert("payout-pool-funding".into(), format!("minted {short} micro-USDC into the pool (shortfall)"));
        } else {
            self.st.addrs.insert("payout-pool-funding".into(), "none needed: the pools already covered the queue (purchase fee splits and the setup mint)".into());
        }
        self.check(short == 0 || self.token_balance(&pool_c) + self.token_balance(&pool_t) >= vs0.open_claims_total, "the pools cover the queue");

        // balances before
        let before = (self.token_balance(&ta_c), self.token_balance(&ta_t));
        let pools_before = (self.token_balance(&pool_c), self.token_balance(&pool_t));
        self.st.addrs.insert("payout-trader-before".into(), format!("{}", before.0 + before.1));
        self.save()?;

        // reconcile (the keeper runs it before every cycle), then begin
        if !vs0.cycle_active {
            self.send("reconcile_product", &[ix::reconcile_product(&self.ids, &keeper)], "keeper", &[], false).map_err(|e| e.text)?;
            self.send("begin_heartbeat", &[ix::begin_heartbeat(&self.ids, &keeper, &vs_key, &pool_c, &pool_t)], "keeper", &[], false).map_err(|e| e.text)?;
        }
        let vs1 = self.vault_view()?;
        self.check(vs1.cycle_active && vs1.cycle_eligible_count == 1 && vs1.cycle_owed_snapshot == owed, &format!("begin_heartbeat: cycle {} open, 1 eligible claim, {} USDC owed, {} USDC available", vs1.cycle_id, usd(vs1.cycle_owed_snapshot), usd(vs1.cycle_available_snapshot)));

        // settle in batches
        for (i, batch) in batches(&[[claim, ta_c, ta_t]]).iter().enumerate() {
            let ixs = [ix::compute_budget_limit(SETTLE_CU), ix::settle_claims(&self.ids, &keeper, &vs_key, &usdc, &usdt, &pool_c, &pool_t, batch)];
            self.send(&format!("settle_claims A1 batch {}", i + 1), &ixs, "keeper", &[], false).map_err(|e| e.text)?;
        }
        self.send("finalize_heartbeat", &[ix::finalize_heartbeat(&self.ids, &keeper, &vs_key, &pool_c, &pool_t)], "keeper", &[], false).map_err(|e| e.text)?;

        // verify on chain
        let after = (self.token_balance(&ta_c), self.token_balance(&ta_t));
        let pools_after = (self.token_balance(&pool_c), self.token_balance(&pool_t));
        let paid = (after.0 + after.1).saturating_sub(before.0 + before.1);
        let drained = (pools_before.0 + pools_before.1).saturating_sub(pools_after.0 + pools_after.1);
        let want = expected_payment(owed, vs1.cycle_owed_snapshot, vs1.cycle_available_snapshot, pools_before.0 + pools_before.1);
        self.st.addrs.insert("payout-trader-after".into(), format!("{}", after.0 + after.1));
        self.st.addrs.insert("payout-amount".into(), format!("{paid}"));
        self.st.addrs.insert("payout-owed".into(), format!("{owed}"));
        self.check(paid == want && paid > 0, &format!("the trader's token balance rose by the paid amount: {} -> {} USDC (+{}; the cycle's rule pays {})", usd(before.0 + before.1), usd(after.0 + after.1), usd(paid), usd(want)));
        let ratio_bps = if vs1.cycle_owed_snapshot == 0 { 0 } else { u128::from(vs1.cycle_available_snapshot.min(vs1.cycle_owed_snapshot)) * 10_000 / u128::from(vs1.cycle_owed_snapshot) };
        self.say(&format!("  pro-rata ratio of the cycle: {ratio_bps} / 10000 (available {} USDC against {} USDC owed)", usd(vs1.cycle_available_snapshot), usd(vs1.cycle_owed_snapshot)));
        self.check(drained == paid, &format!("the pools paid out exactly what the trader received ({} USDC)", usd(drained)));
        let claim_after = self.c.account(&claim).map_err(|e| e.to_string())?.and_then(|c| vaultview::parse_claim(&c.data));
        let remaining = claim_after.as_ref().map_or(0, |c| c.owed);
        self.check(remaining == owed - paid, &format!("the claim is {} ({} USDC still owed)", if claim_after.is_none() { "closed" } else { "paid down" }, usd(remaining)));
        let vs2 = self.vault_view()?;
        self.check(!vs2.cycle_active && vs2.open_claims_total == remaining && vs2.open_claims_count == u64::from(remaining > 0), &format!("finalize_heartbeat: the cycle is closed; the vault's queue is {} claim(s), {} USDC", vs2.open_claims_count, usd(vs2.open_claims_total)));
        self.check(vs2.usdc_floor == self.token_balance(&pool_c) / 4 && vs2.usdt_floor == self.token_balance(&pool_t) / 4, "the reserve floors were recomputed (25% of each pool)");

        // reconciliation: the vault's books and the sector's tally
        self.send("reconcile_product (after the payout)", &[ix::reconcile_product(&self.ids, &keeper)], "keeper", &[], false).map_err(|e| e.text)?;
        let reg = self.c.account(&self.ids.registry()).map_err(|e| e.to_string())?.and_then(|r| vaultview::parse_registry(&r.data)).ok_or("the product registry is not readable")?;
        let tally = self.c.account(&self.ids.tally()).map_err(|e| e.to_string())?.and_then(|t| setl8_shared_interfaces::PayoutTally::parse(&t.data).ok());
        self.check(
            reg.active && tally.as_ref().is_some_and(|t| t.requested_count == reg.total_requests_emitted && t.requested_total == reg.total_requested_amount),
            &format!(
                "reconciliation: the vault's books ({} request(s), {} USDC) equal the sector's tally ({} request(s), {} USDC); the product is still active",
                reg.total_requests_emitted,
                usd(reg.total_requested_amount),
                tally.as_ref().map_or(0, |t| t.requested_count),
                usd(tally.as_ref().map_or(0, |t| t.requested_total))
            ),
        );
        self.mark_done("payout")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shortfall_is_what_the_pools_lack_for_the_queue() {
        assert_eq!(pool_shortfall(200_000_000, 100_000_000, 50_000_000), 50_000_000);
        assert_eq!(pool_shortfall(200_000_000, 275_925_000, 0), 0);
        assert_eq!(pool_shortfall(200_000_000, 0, 200_000_000), 0);
        assert_eq!(pool_shortfall(u64::MAX, u64::MAX, u64::MAX), 0);
    }

    #[test]
    fn a_covered_queue_pays_in_full_and_a_short_pool_pays_pro_rata() {
        assert_eq!(expected_payment(200_000_000, 200_000_000, 50_275_925_000, 50_275_925_000), 200_000_000);
        // pools hold half of the queue: half of the claim
        assert_eq!(expected_payment(200_000_000, 400_000_000, 200_000_000, 200_000_000), 100_000_000);
        // rounding goes down, the dust stays owed
        assert_eq!(expected_payment(10, 3, 1, 1), 1);
        // the live balance can be lower than the snapshot
        assert_eq!(expected_payment(100, 100, 100, 40), 40);
        assert_eq!(expected_payment(100, 0, 100, 100), 0);
    }

    #[test]
    fn claims_are_settled_in_batches_of_the_vaults_maximum() {
        let items: Vec<u32> = (0..14).collect();
        let b = batches(&items);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), vec![6, 6, 2]);
        assert!(batches::<u32>(&[]).is_empty());
    }
}
