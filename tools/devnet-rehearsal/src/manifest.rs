//! `manifest`: the deployment manifest for the backend and frontend teams (`handover/devnet-deployment.json` and `.md`), built from the
//! run's `state.json`, the launch file and the keys' PUBLIC halves. Public data only; the evaluator's key file PATH is the only path in it.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::demo::{address_link, tx_link};
use crate::rehearsal::Rehearsal;
use crate::vaultview;

/// The localnet build the deployed programs were derived from (the frozen program).
pub const LOCALNET_HASH_LEV: &str = "43fdd9356c774ea343686e5f6bf781b998a01670664c2af6b8233bf97f22bc1b";
pub const DEFAULT_HASH_LEV: &str = "ebbd0786d2d1a58a9ef11e539baed4c9057fc3eab59ed8e62a5506bcba6f2c3b";

/// What each account of the run demonstrates (lev trading only).
pub fn role_of(name: &str) -> &'static str {
    match name {
        "A1" => "honest challenge: passes phase 1 and phase 2, the pass is finalized after the delay and the payout is paid",
        "B1" => "fails the static floor in phase 1; reset (ladder step 1) opens B2",
        "B2" => "passes phase 1, fails in phase 2; reset (ladder step 2) opens B3",
        "B3" => "the account opened by the second reset, in phase 2",
        "C1" => "a hold and then a failure while held (review, then no reset)",
        "D1" => "a confirmed phase 2 pass that both admins veto",
        "E1" => "the forced close-all after a quick-limit overshoot",
        "F1" => "a breach with the 12 hour lock (rehearsal price proofs)",
        n if n.starts_with('G') || n.starts_with('H') => "helper account of the refusal and measurement checks (rehearsal price proofs)",
        _ => "rehearsal account",
    }
}

/// The first successful signature of each instruction, by the label's first word.
pub fn example_signatures(sigs: &[crate::state::SigRec]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for s in sigs.iter().filter(|s| s.ok) {
        let name = s.label.split([' ', ':']).next().unwrap_or("").to_string();
        if !name.is_empty() {
            out.entry(name).or_insert_with(|| s.sig.clone());
        }
    }
    out
}

impl Rehearsal {
    pub fn manifest_json(&self, key_file_of_evaluator: &str) -> Result<Value, String> {
        let st = &self.st;
        let pk = |n: &str| self.keys.pubkey(n).to_string();
        let (usdc, usdt) = (self.usdc(), self.usdt());
        let vs = self.vault_state();
        let registry = self.c.account(&self.ids.registry()).map_err(|e| e.to_string())?.and_then(|r| vaultview::parse_registry(&r.data));
        let cfg = self.config()?;
        let markets: Vec<Value> = self.launch.markets.iter().map(|m| json!({"id": m.args.market_id, "symbol": m.symbol, "class": m.args.class, "address": self.ids.market(m.args.market_id).to_string()})).collect();
        let tiers: Vec<Value> = self.launch.tiers().iter().enumerate().map(|(i, (size, cost))| json!({"tier": i, "account_size_usd": size / 1_000_000, "price_usd": cost / 1_000_000})).collect();
        let accounts: Vec<Value> = st
            .accounts
            .values()
            .map(|a| json!({"name": a.name, "role": role_of(&a.name), "trader": a.trader, "challenge_id": a.challenge_id, "address": a.address, "tier": a.tier, "phase_at_open": a.phase_at_open, "created_at": a.created_at, "mock_price_proofs": a.mock}))
            .collect();
        let sigs = example_signatures(&st.sigs);
        let ladder = self.launch.ladder();
        Ok(json!({
            "format": "setl8-lev-trading-devnet-deployment",
            "version": 1,
            "cluster": st.cluster,
            "genesis_hash": st.genesis,
            "note": "The vault is the core vault with a product registry; one product is registered: lev trading. The admin keys of this deployment are throwaway keys of a rehearsal and must never be reused anywhere.",
            "programs": {
                "vault": {"id": st.vault.id, "sha256": st.vault.sha256, "size_bytes": st.vault.size, "source_commit": st.vault_commit},
                "lev": {"id": st.lev.id, "sha256": st.lev.sha256, "size_bytes": st.lev.size,
                        "derived_from_localnet_build_sha256": LOCALNET_HASH_LEV, "default_build_sha256": DEFAULT_HASH_LEV}
            },
            "vault": {
                "vault_state": vs.to_string(),
                "product_registry": self.ids.registry().to_string(),
                "payout_tally": self.ids.tally().to_string(),
                "registered_products": [{"product": "lev trading", "program": st.lev.id, "fee_split_bps": registry.as_ref().map(|r| r.fee_split_bps), "max_payout_count": registry.as_ref().map(|r| r.max_payout_count), "reset_price_bps": registry.as_ref().map(|r| r.reset_price_bps.clone())}],
                "usdc_pool": self.ids.pool(&vs, &usdc).to_string(),
                "usdt_pool": self.ids.pool(&vs, &usdt).to_string()
            },
            "lev": {
                "config": self.ids.config().to_string(),
                "current_rule_set_version": cfg.current_ruleset,
                "rule_set": self.ids.ruleset(cfg.current_ruleset).to_string(),
                "event_authority": self.ids.event_authority().to_string(),
                "program_authority": self.ids.sector_authority().to_string(),
                "pass_delay_secs": cfg.pass_delay_secs,
                "markets": markets,
                "tiers": tiers,
                "reset_ladder_bps": ladder,
                "challenge_fee_bps": self.launch.ruleset.fee_bps
            },
            "test_mints": {"usdc": usdc.to_string(), "usdt": usdt.to_string(), "decimals": 6, "mint_authority": pk("keeper"), "note": "test stablecoins created for this rehearsal"},
            "fee_wallet": pk("keeper"),
            "evaluator": {"public_key": pk("evaluator"), "key_file_path_for_local_devnet_use": key_file_of_evaluator},
            "admins_throwaway": {
                "vault_admin_sl8": pk("vault-admin-sl8"), "vault_admin_rov": pk("vault-admin-rov"),
                "lev_admin_sl8": pk("lev-admin-sl8"), "lev_admin_rov": pk("lev-admin-rov"),
                "upgrade_authority": pk("upgrade-authority"), "keeper": pk("keeper")
            },
            "lookup_table": st.addrs.get("lookup-table").cloned(),
            "example_accounts": accounts,
            "payout": {"claim": st.addrs.get("claim-A1"), "trader_balance_before_micro_usdc": st.addrs.get("payout-trader-before"), "trader_balance_after_micro_usdc": st.addrs.get("payout-trader-after"), "pool_funding": st.addrs.get("payout-pool-funding")},
            "example_signatures": sigs
        }))
    }
}

fn line(s: &mut String, k: &str, v: &str) {
    s.push_str(&format!("- {k}: `{v}`\n"));
}

fn addr_line(s: &mut String, k: &str, v: &str) {
    s.push_str(&format!("- {k}: [`{v}`]({})\n", address_link(v)));
}

/// The markdown companion of the manifest.
pub fn to_markdown(j: &Value) -> String {
    let g = |p: &str| -> String { j.pointer(&format!("/{}", p.replace('.', "/"))).map(|v| v.as_str().map_or(v.to_string(), str::to_string)).unwrap_or_default() };
    let mut s = String::from("# Devnet deployment of the lev trading challenge\n\nPublic data only. Machine-readable twin: `handover/devnet-deployment.json`.\n\n");
    s.push_str("**The admin keys of this deployment are throwaway keys of a rehearsal. They must never be reused anywhere.**\n\n");
    s.push_str("The core vault holds a product registry; one product is registered: lev trading (the leveraged-trading challenge).\n\n");
    s.push_str("## Cluster\n\n");
    line(&mut s, "cluster", &g("cluster"));
    line(&mut s, "genesis hash", &g("genesis_hash"));
    s.push_str("\n## Programs\n\n");
    addr_line(&mut s, "vault program", &g("programs.vault.id"));
    line(&mut s, "vault sha256", &g("programs.vault.sha256"));
    line(&mut s, "vault size (bytes)", &g("programs.vault.size_bytes"));
    addr_line(&mut s, "lev trading program", &g("programs.lev.id"));
    line(&mut s, "lev sha256", &g("programs.lev.sha256"));
    line(&mut s, "lev size (bytes)", &g("programs.lev.size_bytes"));
    line(&mut s, "derived from the localnet build (sha256)", &g("programs.lev.derived_from_localnet_build_sha256"));
    s.push_str("\n## The vault\n\n");
    for (k, p) in [("vault state", "vault.vault_state"), ("product registry", "vault.product_registry"), ("payout tally", "vault.payout_tally"), ("USDC payout pool", "vault.usdc_pool"), ("USDT payout pool", "vault.usdt_pool")] {
        addr_line(&mut s, k, &g(p));
    }
    s.push_str("\n## Lev trading\n\n");
    for (k, p) in [("config", "lev.config"), ("rule set", "lev.rule_set"), ("event authority", "lev.event_authority"), ("program authority", "lev.program_authority")] {
        addr_line(&mut s, k, &g(p));
    }
    line(&mut s, "pass delay (seconds)", &g("lev.pass_delay_secs"));
    line(&mut s, "challenge fee (bps of the price)", &g("lev.challenge_fee_bps"));
    s.push_str("\n### Tiers\n\n| tier | account size (USD) | price (USD) |\n|---|---:|---:|\n");
    for t in j["lev"]["tiers"].as_array().into_iter().flatten() {
        s.push_str(&format!("| {} | {} | {} |\n", t["tier"], t["account_size_usd"], t["price_usd"]));
    }
    s.push_str("\n### Markets (21)\n\n| id | symbol | address |\n|---:|---|---|\n");
    for m in j["lev"]["markets"].as_array().into_iter().flatten() {
        let a = m["address"].as_str().unwrap_or("");
        s.push_str(&format!("| {} | {} | [`{a}`]({}) |\n", m["id"], m["symbol"].as_str().unwrap_or(""), address_link(a)));
    }
    s.push_str("\n## Test mints and keys\n\n");
    addr_line(&mut s, "test USDC mint", &g("test_mints.usdc"));
    addr_line(&mut s, "test USDT mint", &g("test_mints.usdt"));
    addr_line(&mut s, "fee wallet (also the test mint authority)", &g("fee_wallet"));
    addr_line(&mut s, "evaluator public key", &g("evaluator.public_key"));
    line(&mut s, "evaluator key file (local devnet use only; the file is not part of this manifest)", &g("evaluator.key_file_path_for_local_devnet_use"));
    for (k, p) in [("vault admin SL8 (throwaway)", "admins_throwaway.vault_admin_sl8"), ("vault admin ROV (throwaway)", "admins_throwaway.vault_admin_rov"), ("lev admin SL8 (throwaway)", "admins_throwaway.lev_admin_sl8"), ("lev admin ROV (throwaway)", "admins_throwaway.lev_admin_rov"), ("upgrade authority (throwaway)", "admins_throwaway.upgrade_authority")] {
        addr_line(&mut s, k, &g(p));
    }
    let alt = g("lookup_table");
    s.push_str(&format!("- address lookup table: {}\n", if alt.is_empty() || alt == "null" { "none was created by this run".to_string() } else { format!("`{alt}`") }));
    s.push_str("\n## Example accounts\n\n| name | role | trading account | trader wallet |\n|---|---|---|---|\n");
    for a in j["example_accounts"].as_array().into_iter().flatten() {
        let (ad, tr) = (a["address"].as_str().unwrap_or(""), a["trader"].as_str().unwrap_or(""));
        s.push_str(&format!("| {} | {} | [`{ad}`]({}) | [`{tr}`]({}) |\n", a["name"].as_str().unwrap_or(""), a["role"].as_str().unwrap_or(""), address_link(ad), address_link(tr)));
    }
    s.push_str("\n## One example transaction per instruction\n\n");
    if let Some(m) = j["example_signatures"].as_object() {
        for (name, sig) in m {
            let sig = sig.as_str().unwrap_or("");
            s.push_str(&format!("- `{name}`: [{sig}]({})\n", tx_link(sig)));
        }
    }
    s
}

pub fn write_manifest(r: &Rehearsal, out_dir: &Path, evaluator_key_file: &str) -> Result<(), String> {
    let j = r.manifest_json(evaluator_key_file)?;
    std::fs::write(out_dir.join("devnet-deployment.json"), serde_json::to_string_pretty(&j).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("devnet-deployment.md"), to_markdown(&j)).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SigRec;

    fn sig(label: &str, s: &str, ok: bool) -> SigRec {
        SigRec { label: label.into(), sig: s.into(), slot: 1, ok, err: String::new(), cu: 1, size: 1, fee: 1, block_time: 1 }
    }

    #[test]
    fn one_example_signature_per_instruction_the_first_successful_one() {
        let m = example_signatures(&[sig("post_journal ev=5 A1", "P1", true), sig("open_account A1", "O1", false), sig("open_account A2", "O2", true), sig("post_journal x", "P2", true), sig("lookup table: create", "L", true)]);
        assert_eq!(m["post_journal"], "P1");
        assert_eq!(m["open_account"], "O2");
        assert_eq!(m["lookup"], "L");
    }

    #[test]
    fn the_markdown_carries_the_scope_sentence_and_the_throwaway_warning_and_no_other_product() {
        let j = json!({"cluster": "devnet", "lev": {"tiers": [{"tier": 0, "account_size_usd": 2500, "price_usd": 30}], "markets": [{"id": 0, "symbol": "BTC", "address": "M"}]}, "example_accounts": [], "example_signatures": {}});
        let md = to_markdown(&j);
        assert!(md.contains("one product is registered: lev trading"));
        assert!(md.contains("throwaway keys of a rehearsal"));
        let low = md.to_lowercase();
        for w in ["option", "predict", "hft", "simfunded", "sector 2", "roadmap"] {
            assert!(!low.contains(w), "{w}");
        }
    }

    #[test]
    fn every_role_text_is_about_lev_trading_only() {
        for n in ["A1", "B1", "B2", "B3", "C1", "D1", "E1", "F1", "G1", "H4", "X"] {
            let low = role_of(n).to_lowercase();
            for w in ["option", "predict", "hft", "simfunded", "sector 2", "roadmap"] {
                assert!(!low.contains(w), "{n}: {w}");
            }
        }
    }
}
