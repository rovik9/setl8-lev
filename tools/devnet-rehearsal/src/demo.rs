//! `demo`: the A1 story as an ordered list of steps with their PUBLIC signatures, Solana Explorer links and account addresses, read
//! from the run's `state.json` (no cluster access, no key). Steps that need day 2 print when they become possible.
use crate::state::State;

const EXPLORER: &str = "https://explorer.solana.com";

pub fn tx_link(sig: &str) -> String {
    format!("{EXPLORER}/tx/{sig}?cluster=devnet")
}

pub fn address_link(addr: &str) -> String {
    format!("{EXPLORER}/address/{addr}?cluster=devnet")
}

/// UTC `YYYY-MM-DD HH:MM:SS` of a unix second (civil-from-days, proleptic Gregorian).
pub fn utc(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

pub struct Step {
    pub title: String,
    pub proves: String,
    pub sigs: Vec<(String, String)>,
    pub addrs: Vec<(String, String)>,
    /// Set when the step does not exist yet.
    pub pending: Option<String>,
}

fn usdc(micro: &str) -> String {
    match micro.parse::<u64>() {
        Ok(m) => format!("{}.{:06} USDC", m / 1_000_000, m % 1_000_000),
        Err(_) => micro.to_string(),
    }
}

/// The signatures of the successful transactions whose label starts with `prefix` (in the order they were sent).
fn sigs_of(st: &State, prefix: &str) -> Vec<(String, String)> {
    st.sigs.iter().filter(|s| s.ok && s.label.starts_with(prefix)).map(|s| (s.label.clone(), s.sig.clone())).collect()
}

pub fn steps(st: &State) -> Vec<Step> {
    let a1 = st.accounts.get("A1");
    let acct = a1.map(|a| a.address.clone()).unwrap_or_default();
    let trader = a1.map(|a| a.trader.clone()).unwrap_or_default();
    let addr = |k: &str| st.addrs.get(k).cloned().unwrap_or_default();
    let base = vec![("A1 trading account".to_string(), acct), ("A1 trader wallet".to_string(), trader.clone())];
    let day2 = st.pending_finalize.get("A1").copied();
    let pending_day2 = || day2.map(|t| format!("not yet: it exists after day 2, from {} UTC (unix second {t}); run `run` again then", utc(t)));

    let opens = sigs_of(st, "open_account A1");
    let posts: Vec<(String, String)> = st
        .sigs
        .iter()
        .take_while(|s| !(s.label.starts_with("request_advance A1") && s.ok))
        .filter(|s| s.ok && s.label.starts_with("post_journal") && s.label.ends_with("A1"))
        .map(|s| (s.label.clone(), s.sig.clone()))
        .collect();
    let adv = sigs_of(st, "request_advance A1");
    let conf = sigs_of(st, "confirm_pass A1");
    let fin = sigs_of(st, "finalize_pass A1");
    let fin_ok: Vec<(String, String)> = fin.into_iter().filter(|(l, _)| !l.contains("early")).collect();
    let mut heart = sigs_of(st, "reconcile_product");
    heart.retain(|(l, _)| !l.contains("after"));
    heart.extend(sigs_of(st, "begin_heartbeat"));
    heart.extend(sigs_of(st, "settle_claims"));
    heart.extend(sigs_of(st, "finalize_heartbeat"));

    let mut out = Vec::new();
    out.push(Step {
        title: "The challenge purchase (open_account)".into(),
        proves: "The trader paid the challenge price plus the 0.4 % fee in the test stablecoin in one transaction; the vault split the price between its payout pool and the platform wallet, and recorded the challenge.".into(),
        sigs: opens.iter().take(1).cloned().collect(),
        addrs: [base.clone(), vec![("vault state".into(), addr("vault_state")), ("payout pool".into(), addr("usdc_pool")), ("test stablecoin mint".into(), addr("mint-usdc"))]].concat(),
        pending: if opens.is_empty() { Some("not run yet".into()) } else { None },
    });
    out.push(Step {
        title: "The first journal batches (the winning history)".into(),
        proves: "The evaluator posted the trader's signed trades as journal batches; the program re-checked every rule on chain and the account's equity and sequence advanced only through them.".into(),
        sigs: posts.clone(),
        addrs: base.clone(),
        pending: if posts.is_empty() { Some("not run yet".into()) } else { None },
    });
    out.push(Step {
        title: "Phase 1 passed: request_advance and confirm_pass".into(),
        proves: "The account met the phase-1 target; the trader asked to advance and the evaluator confirmed, so the program moved the account to phase 2.".into(),
        sigs: [adv.iter().take(1).cloned().collect::<Vec<_>>(), conf.iter().take(1).cloned().collect()].concat(),
        addrs: base.clone(),
        pending: if conf.is_empty() { Some("not run yet".into()) } else { None },
    });
    out.push(Step {
        title: "Phase 2 passed: request_advance and confirm_pass".into(),
        proves: "The account met the phase-2 target and the pass is confirmed; the program now holds it for the one-day pass delay before the payout can be queued.".into(),
        sigs: [adv.iter().skip(1).take(1).cloned().collect::<Vec<_>>(), conf.iter().skip(1).take(1).cloned().collect()].concat(),
        addrs: base.clone(),
        pending: if conf.len() < 2 { Some("not run yet".into()) } else { None },
    });
    out.push(Step {
        title: "finalize_pass and the queued payout claim".into(),
        proves: "After the pass delay the program started the funded phase and the vault queued a payout claim for the trader; the claim account is owned by the vault.".into(),
        sigs: fin_ok.clone(),
        addrs: [base.clone(), vec![("payout claim".into(), addr("claim-A1"))]].concat(),
        pending: if fin_ok.is_empty() { pending_day2() } else { None },
    });
    let paid = st.addrs.contains_key("payout-amount");
    out.push(Step {
        title: "The heartbeat and the settlement".into(),
        proves: "Anyone can run the vault's heartbeat: it reconciled the vault's books with the trading program's tally, froze the pool balances, paid the claim to the trader's token account and closed the cycle.".into(),
        sigs: heart,
        addrs: [base.clone(), vec![("payout claim".into(), addr("claim-A1")), ("payout pool".into(), addr("usdc_pool"))]].concat(),
        pending: if paid { None } else { Some(day2.map_or("not yet: after day 2".to_string(), |t| format!("not yet: it exists after day 2, from {} UTC (unix second {t}) and the payout step; run `run` again then", utc(t)))) },
    });
    out.push(Step {
        title: "The trader's balance before and after".into(),
        proves: format!(
            "The payout landed in the trader's own token account: the balance rose by {} (the claim, paid in full when the pool covers the queue).",
            st.addrs.get("payout-amount").map_or("the paid amount".to_string(), |m| usdc(m))
        ),
        sigs: vec![],
        addrs: [base, vec![("pool funding".into(), addr("payout-pool-funding"))]].concat(),
        pending: if paid { None } else { Some("not yet: after the payout step".into()) },
    });
    out
}

/// The same facts that `steps` carries but as the balance sentence of step 7.
pub fn balance_line(st: &State) -> Option<String> {
    let (b, a) = (st.addrs.get("payout-trader-before")?, st.addrs.get("payout-trader-after")?);
    Some(format!("trader token balance before {} -> after {}", usdc(b), usdc(a)))
}

pub fn render_text(st: &State) -> String {
    let mut s = String::from("DEMO: the A1 story (devnet, public data only)\n");
    for (i, st_) in steps(st).iter().enumerate() {
        s.push_str(&format!("\n{}. {}\n   {}\n", i + 1, st_.title, st_.proves));
        if let Some(p) = &st_.pending {
            s.push_str(&format!("   [{p}]\n"));
        }
        for (l, g) in &st_.sigs {
            s.push_str(&format!("   tx {l}: {g}\n       {}\n", tx_link(g)));
        }
        for (role, a) in st_.addrs.iter().filter(|(_, a)| !a.is_empty()) {
            s.push_str(&format!("   {role}: {a}\n       {}\n", address_link(a)));
        }
    }
    if let Some(b) = balance_line(st) {
        s.push_str(&format!("\n{b}\n"));
    }
    s
}

pub fn render_markdown(st: &State) -> String {
    let mut s = String::from("# Devnet demo: one challenge from purchase to payout\n\nPublic data only. Every link opens the transaction or account on Solana Explorer (devnet).\n");
    for (i, st_) in steps(st).iter().enumerate() {
        s.push_str(&format!("\n## {}. {}\n\n{}\n\n", i + 1, st_.title, st_.proves));
        if let Some(p) = &st_.pending {
            s.push_str(&format!("*{p}*\n\n"));
        }
        for (l, g) in &st_.sigs {
            s.push_str(&format!("- {l}: [{g}]({})\n", tx_link(g)));
        }
        for (role, a) in st_.addrs.iter().filter(|(_, a)| !a.is_empty()) {
            s.push_str(&format!("- {role}: [{a}]({})\n", address_link(a)));
        }
    }
    if let Some(b) = balance_line(st) {
        s.push_str(&format!("\n**{b}**\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AcctRec, SigRec};

    fn sig(label: &str, sig: &str, ok: bool) -> SigRec {
        SigRec { label: label.into(), sig: sig.into(), slot: 1, ok, err: String::new(), cu: 1, size: 1, fee: 5000, block_time: 1 }
    }

    fn day1() -> State {
        let mut st = State::default();
        st.accounts.insert("A1".into(), AcctRec { name: "A1".into(), trader: "TRADER".into(), address: "ACCT".into(), ..Default::default() });
        st.addrs.insert("vault_state".into(), "VS".into());
        for (l, g, ok) in [
            ("open_account A1", "OPEN", true),
            ("post_journal ev=5 pr=3 A1", "POST1", true),
            ("request_advance A1", "ADV1", true),
            ("confirm_pass A1", "CONF1", true),
            ("post_journal ev=4 pr=3 A1", "POST2", true),
            ("request_advance A1", "ADV2", true),
            ("confirm_pass A1", "CONF2", true),
            ("post_journal A1 (pass pending)", "REFUSED", false),
            ("finalize_pass A1 (early)", "EARLY", false),
        ] {
            st.sigs.push(sig(l, g, ok));
        }
        st.pending_finalize.insert("A1".into(), 1_793_535_634);
        st
    }

    #[test]
    fn the_utc_conversion_matches_known_instants() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_793_535_634), "2026-11-01 12:20:34");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn day_one_lists_the_first_four_steps_and_marks_the_payout_steps_pending_with_the_time() {
        let st = day1();
        let steps = steps(&st);
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[0].sigs, vec![("open_account A1".to_string(), "OPEN".to_string())]);
        assert_eq!(steps[1].sigs.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["POST1"], "only the batches before the first advance");
        assert_eq!(steps[2].sigs.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["ADV1", "CONF1"]);
        assert_eq!(steps[3].sigs.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["ADV2", "CONF2"]);
        assert!(steps[..4].iter().all(|s| s.pending.is_none()));
        assert!(steps[4].pending.as_ref().unwrap().contains("2026-11-01 12:20:34 UTC"), "the day-2 time is printed");
        assert!(steps[4].sigs.is_empty(), "the early (refused) finalize is not a step of the story");
        assert!(steps[5].pending.is_some() && steps[6].pending.is_some());
    }

    #[test]
    fn after_the_payout_every_step_is_complete_with_explorer_links() {
        let mut st = day1();
        for (l, g) in [("finalize_pass A1", "FIN"), ("reconcile_product", "REC"), ("begin_heartbeat", "BEG"), ("settle_claims A1 batch 1", "SET"), ("finalize_heartbeat", "FH"), ("reconcile_product (after the payout)", "REC2")] {
            st.sigs.push(sig(l, g, true));
        }
        st.addrs.insert("claim-A1".into(), "CLAIM".into());
        st.addrs.insert("payout-amount".into(), "200000000".into());
        st.addrs.insert("payout-trader-before".into(), "1000000".into());
        st.addrs.insert("payout-trader-after".into(), "201000000".into());
        let steps = steps(&st);
        assert!(steps.iter().all(|s| s.pending.is_none()));
        assert_eq!(steps[5].sigs.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["REC", "BEG", "SET", "FH"]);
        let text = render_text(&st);
        assert!(text.contains("https://explorer.solana.com/tx/FIN?cluster=devnet"));
        assert!(text.contains("https://explorer.solana.com/address/CLAIM?cluster=devnet"));
        assert!(text.contains("trader token balance before 1.000000 USDC -> after 201.000000 USDC"));
        assert!(render_markdown(&st).contains("[FIN](https://explorer.solana.com/tx/FIN?cluster=devnet)"));
    }

    #[test]
    fn the_demo_text_is_about_one_product_only() {
        let st = day1();
        let all = format!("{}{}", render_text(&st), render_markdown(&st)).to_lowercase();
        for w in ["option", "predict", "hft", "simfunded", "sector 2", "roadmap"] {
            assert!(!all.contains(w), "{w}");
        }
    }
}
