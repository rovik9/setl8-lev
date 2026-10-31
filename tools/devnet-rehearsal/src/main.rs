//! Rehearsal driver: the frozen `setl8-lev-trading` program and the real core vault, driven end to end on a real cluster with the
//! reference evaluator, the chain's own events replayed through the public verifier. DEVNET ONLY (or a local validator for the
//! selftest): see `scripts/devnet-rehearsal.sh` for the guards and `handover/devnet-rehearsal.md` for the runbook.
//!
//! Output rule: only PUBLIC data is printed (addresses, signatures, balances, hashes). No key, key file path, seed or RPC URL.
pub mod accounts;
pub mod attacks;
pub mod cluster;
pub mod demo;
pub mod events;
pub mod ix;
pub mod launch;
pub mod live;
pub mod manifest;
pub mod measure;
pub mod payout;
pub mod rehearsal;
pub mod report;
pub mod runtime;
pub mod scenarios;
pub mod state;
pub mod stories;
pub mod vaultview;
pub mod verify;

use std::path::PathBuf;

use anchor_lang::solana_program::pubkey::Pubkey;
use cluster::{check_genesis, check_url, Cluster, ClusterKind};
use rehearsal::{sol, Keys, Rehearsal, KEY_NAMES};

struct Args {
    rpc_url: String,
    cluster: String,
    expect_genesis: String,
    keys_dir: PathBuf,
    work_dir: PathBuf,
    vault_program: Option<Pubkey>,
    lev_program: Option<Pubkey>,
    vault_so: Option<PathBuf>,
    lev_so: Option<PathBuf>,
    vault_commit: String,
    verify_bin: Option<PathBuf>,
    launch: PathBuf,
    only: Vec<String>,
    pass_delay: u32,
    out_dir: PathBuf,
    validator_info: Option<PathBuf>,
    command: String,
    resume: bool,
    markdown: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        rpc_url: String::new(),
        cluster: String::new(),
        expect_genesis: String::new(),
        keys_dir: PathBuf::new(),
        work_dir: PathBuf::new(),
        vault_program: None,
        lev_program: None,
        vault_so: None,
        lev_so: None,
        vault_commit: String::new(),
        verify_bin: None,
        launch: PathBuf::from("config/launch.json"),
        only: Vec::new(),
        pass_delay: scenarios::PASS_DELAY_SECS,
        out_dir: PathBuf::from("handover"),
        validator_info: None,
        command: String::new(),
        resume: false,
        markdown: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--rpc-url" => a.rpc_url = val("--rpc-url")?,
            "--cluster" => a.cluster = val("--cluster")?,
            "--expect-genesis" => a.expect_genesis = val("--expect-genesis")?,
            "--keys-dir" => a.keys_dir = PathBuf::from(val("--keys-dir")?),
            "--work-dir" => a.work_dir = PathBuf::from(val("--work-dir")?),
            "--vault-program" => a.vault_program = Some(val("--vault-program")?.parse().map_err(|e| format!("--vault-program: {e}"))?),
            "--lev-program" => a.lev_program = Some(val("--lev-program")?.parse().map_err(|e| format!("--lev-program: {e}"))?),
            "--vault-so" => a.vault_so = Some(PathBuf::from(val("--vault-so")?)),
            "--lev-so" => a.lev_so = Some(PathBuf::from(val("--lev-so")?)),
            "--vault-commit" => a.vault_commit = val("--vault-commit")?,
            "--verify-bin" => a.verify_bin = Some(PathBuf::from(val("--verify-bin")?)),
            "--launch" => a.launch = PathBuf::from(val("--launch")?),
            "--only" => a.only = val("--only")?.split(',').map(str::to_string).collect(),
            "--pass-delay-secs" => a.pass_delay = val("--pass-delay-secs")?.parse().map_err(|e| format!("--pass-delay-secs: {e}"))?,
            "--out-dir" => a.out_dir = PathBuf::from(val("--out-dir")?),
            "--validator-info" => a.validator_info = Some(PathBuf::from(val("--validator-info")?)),
            "--resume" => a.resume = true,
            "--markdown" => a.markdown = true,
            c if !c.starts_with("--") && a.command.is_empty() => a.command = c.to_string(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.command.is_empty() {
        return Err("usage: devnet-rehearsal --rpc-url U --cluster devnet|localnet --expect-genesis H --keys-dir D --work-dir W --vault-program P --lev-program P [--vault-so F --lev-so F --vault-commit C --verify-bin F --only S0,S1] <keys|pubkeys|needs|fund|distribute|run|payout|demo|status>".into());
    }
    Ok(a)
}

fn kind_of(cluster: &str) -> Result<ClusterKind, String> {
    match cluster {
        "devnet" => Ok(ClusterKind::Devnet),
        "localnet" => Ok(ClusterKind::Local),
        other => Err(format!("refusing: --cluster {other} (only devnet, or localnet for the selftest)")),
    }
}

/// SOL each key should hold before the run, in lamports. `deploy_peak` is what the upgrade authority needs for the two deployments.
pub fn needs(deploy_peak: u64) -> Vec<(&'static str, u64, &'static str)> {
    const L: u64 = 1_000_000_000;
    vec![
        ("upgrade-authority", deploy_peak + L / 10, "rent of both programs (doubled while a deploy buffer exists)"),
        ("keeper", 3 * L / 4, "fee payer and rent payer of the admin transactions, the mints, the token accounts and the claims"),
        ("evaluator", L / 4, "fee payer of every journal batch"),
        ("vault-admin-sl8", L / 10, "pays the rent of the vault state and the registry"),
        ("vault-admin-rov", L / 100, "signs"),
        ("lev-admin-sl8", L / 100, "signs"),
        ("lev-admin-rov", L / 100, "signs"),
        ("trader1", L / 20, "rent of its accounts and fees"),
        ("trader2", L / 20, "rent of its accounts and fees (three accounts)"),
        ("trader3", L / 20, "rent of its accounts and fees"),
        ("trader4", L / 20, "rent of its accounts and fees"),
        ("trader5", L / 20, "rent of its accounts and fees"),
        ("trader6", L / 20, "rent of its accounts and fees"),
        ("trader7", L / 20, "rent of its accounts and fees (two accounts)"),
    ]
}

fn deploy_peak(c: &Cluster, vault_len: u64, lev_len: u64) -> u64 {
    let r = |n: u64| c.min_rent(usize::try_from(n).unwrap_or(0)).unwrap_or(n * 6960);
    let program = r(36);
    let vault_total = r(vault_len + 45) + program;
    let vault_peak = vault_total + r(vault_len + 37);
    let lev_peak = vault_total + r(lev_len + 45) + program + r(lev_len + 37);
    vault_peak.max(lev_peak)
}

fn run() -> Result<i32, String> {
    let a = parse_args()?;
    let kind = kind_of(&a.cluster)?;
    check_url(kind, &a.rpc_url)?;
    // keys that only need the directory
    if a.command == "keys" {
        return keys_command(&a);
    }
    if a.command == "render-runtime" {
        // regenerates handover/cu-runtime.md from handover/cu-runtime.json (no validator)
        let text = std::fs::read_to_string(a.out_dir.join("cu-runtime.json")).map_err(|e| e.to_string())?;
        let j: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        std::fs::write(a.out_dir.join("cu-runtime.md"), runtime::to_markdown(&j)).map_err(|e| e.to_string())?;
        println!("render-runtime: {} written", a.out_dir.join("cu-runtime.md").display());
        return Ok(0);
    }
    let c = Cluster::new(&a.rpc_url);
    let got = c.genesis_hash().map_err(|e| e.to_string())?;
    check_genesis(kind, &got, &a.expect_genesis)?;
    let keys = Keys::load(&a.keys_dir)?;
    if a.command == "pubkeys" {
        for (n, k) in keys.public_list() {
            println!("{n} {k}");
        }
        return Ok(0);
    }
    let ids = ix::Ids { lev: a.lev_program.ok_or("--lev-program is required")?, vault: a.vault_program.ok_or("--vault-program is required")? };
    std::fs::create_dir_all(&a.work_dir).map_err(|e| format!("work dir: {e}"))?;
    let st_path = a.work_dir.join("state.json");
    let mut st = state::State::load(&st_path)?;
    st.cluster = a.cluster.clone();
    st.genesis = got.clone();
    if !a.vault_commit.is_empty() {
        st.vault_commit = a.vault_commit.clone();
    }
    rehearsal::started_now(&mut st);
    let launch_text = std::fs::read_to_string(&a.launch).map_err(|e| format!("{}: {e}", a.launch.display()))?;
    let launch = launch::parse(&launch_text)?;
    let mut r = Rehearsal { c, kind, ids, keys, st, st_path, work: a.work_dir.clone(), launch, quiet: false, cur: None, vault_so: a.vault_so.clone(), lev_so: a.lev_so.clone(), verify_bin: a.verify_bin.clone(), pass_delay: a.pass_delay, alt: None };

    match a.command.as_str() {
        "needs" => needs_command(&r, &a),
        "fund" => fund_command(&r, &a),
        "distribute" => distribute_command(&mut r, &a),
        "payout" => {
            r.payout()?;
            Ok(if r.st.scenarios.get("S2-payout").is_some_and(|s| s.result == "FAIL") { 1 } else { 0 })
        }
        "manifest" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let shown = a.keys_dir.join("evaluator.json").display().to_string();
            let shown = if !home.is_empty() && shown.starts_with(&home) { shown.replacen(&home, "~", 1) } else { shown };
            manifest::write_manifest(&r, &a.out_dir, &shown)?;
            println!("manifest: devnet-deployment.json and devnet-deployment.md written to {}", a.out_dir.display());
            Ok(0)
        }
        "demo" => {
            print!("{}", if a.markdown { demo::render_markdown(&r.st) } else { demo::render_text(&r.st) });
            Ok(0)
        }
        "status" => {
            status_command(&r);
            Ok(0)
        }
        "run" => run_command(&mut r, &a),
        "measure" => measure_command(&mut r, &a),
        other => Err(format!("unknown command {other}")),
    }
}

fn file_len(p: &Option<PathBuf>) -> u64 {
    p.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0)
}

fn needs_command(r: &Rehearsal, a: &Args) -> Result<i32, String> {
    let peak = deploy_peak(&r.c, file_len(&a.vault_so).max(1), file_len(&a.lev_so).max(1));
    let mut short = false;
    for (name, want, why) in needs(peak) {
        let have = r.sol(name);
        if have < want {
            short = true;
            println!("NEEDS FUNDING: {name} {} has {} SOL, needs {} SOL ({why})", r.keys.pubkey(name), sol(have), sol(want));
        } else {
            println!("ok: {name} {} has {} SOL (needs {})", r.keys.pubkey(name), sol(have), sol(want));
        }
    }
    let total: u64 = needs(peak).iter().map(|n| n.1).sum();
    println!("total SOL for a full run: {}", sol(total));
    Ok(if short { 3 } else { 0 })
}

/// P30: extra SOL the keeper adds on top of `needs()` when it tops up the upgrade authority.
const UPGRADE_MARGIN: u64 = 200_000_000;

/// One transfer of the keeper's distribution plan.
#[derive(Debug, PartialEq, Eq)]
pub struct Transfer {
    pub name: &'static str,
    pub lamports: u64,
}

/// The keeper's distribution plan. `held` gives each key's balance. The small keys (everything but the upgrade authority and the keeper) get
/// what `needs()` asks; the upgrade authority is topped up to its need plus `UPGRADE_MARGIN`, but only if the keeper keeps its own need plus what
/// the small keys take (the top-up never eats into those). Returns the transfers and the keeper's shortfall (0 when the plan is payable;
/// then nothing is to be sent), with the upgrade-authority top-up skipped (and reported) when it does not fit.
pub fn distribution_plan(needs: &[(&'static str, u64, &'static str)], held: impl Fn(&str) -> u64) -> (Vec<Transfer>, u64, Option<u64>) {
    let mut small = Vec::new();
    let mut small_total = 0u64;
    let mut keeper_want = 0u64;
    let mut upgrade_want = 0u64;
    for &(name, want, _) in needs {
        match name {
            "keeper" => keeper_want = want,
            "upgrade-authority" => upgrade_want = want,
            _ => {
                let have = held(name);
                if have < want {
                    small.push(Transfer { name, lamports: want - have });
                    small_total += want - have;
                }
            }
        }
    }
    let keeper_have = held("keeper");
    let reserved = small_total + keeper_want;
    let shortfall = reserved.saturating_sub(keeper_have);
    let mut skipped = None;
    let top_up = (upgrade_want + UPGRADE_MARGIN).saturating_sub(held("upgrade-authority"));
    if shortfall == 0 && top_up > 0 {
        if keeper_have >= reserved + top_up {
            small.push(Transfer { name: "upgrade-authority", lamports: top_up });
        } else {
            skipped = Some(reserved + top_up - keeper_have);
        }
    }
    (small, shortfall, skipped)
}

/// P30: when only the upgrade authority and the keeper were funded (in the browser faucet), the keeper pays the small keys and, if it can
/// afford it, tops up the upgrade authority: one transaction of system transfers from the keeper, explicit keypair and url. Public data only.
fn distribute_command(r: &mut Rehearsal, a: &Args) -> Result<i32, String> {
    let peak = deploy_peak(&r.c, file_len(&a.vault_so).max(1), file_len(&a.lev_so).max(1));
    let keeper = r.keys.pubkey("keeper");
    let table = needs(peak);
    let (plan, shortfall, skipped) = distribution_plan(&table, |n| r.sol(n));
    if shortfall > 0 {
        println!("distribute: the keeper holds {} SOL and needs {} SOL more for the small keys and its own work: fund the keeper with at least that much", sol(r.sol("keeper")), sol(shortfall));
        return Ok(3);
    }
    if let Some(missing) = skipped {
        println!("distribute: the upgrade-authority top-up does not fit (the keeper must keep its own need and the small keys'): {} SOL more in the keeper would allow it", sol(missing));
    }
    if plan.is_empty() {
        println!("distribute: nothing to transfer");
        return needs_command(r, a);
    }
    let mut ixs = Vec::new();
    for t in &plan {
        println!("distribute: keeper -> {} {} : {} SOL", t.name, r.keys.pubkey(t.name), sol(t.lamports));
        ixs.push(anchor_lang::solana_program::system_instruction::transfer(&keeper, &r.keys.pubkey(t.name), t.lamports));
    }
    let sent = r.send("distribute", &ixs, "keeper", &[], false).map_err(|e| e.text)?;
    println!("distribute: {} transfers in one transaction [{}]", ixs.len(), sent.info.signature);
    needs_command(r, a)
}

fn fund_command(r: &Rehearsal, a: &Args) -> Result<i32, String> {
    let peak = deploy_peak(&r.c, file_len(&a.vault_so).max(1), file_len(&a.lev_so).max(1));
    for (name, want, _) in needs(peak) {
        let key = r.keys.pubkey(name);
        for attempt in 0..6u64 {
            let have = r.sol(name);
            if have >= want {
                break;
            }
            let ask = (want - have).min(if r.kind == ClusterKind::Local { 100_000_000_000 } else { 5_000_000_000 });
            match r.c.airdrop(&key, ask) {
                Ok(_) => {
                    println!("{name}: airdrop of {} SOL requested", sol(ask));
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                Err(e) if e.to_string().contains("faucet is closed") => {
                    println!("the RPC faucet of this cluster is closed: fund the public addresses of `needs` in the browser faucet (https://faucet.solana.com), then run `distribute`");
                    return needs_command(r, a);
                }
                Err(e) => {
                    println!("{name}: airdrop refused ({e}); attempt {}", attempt + 1);
                    std::thread::sleep(std::time::Duration::from_secs(10 * (attempt + 1)));
                }
            }
        }
    }
    needs_command(r, a)
}

fn status_command(r: &Rehearsal) {
    println!("cluster {} genesis {}", r.st.cluster, r.st.genesis);
    println!("vault {} sha256 {} ({} bytes) commit {}", r.st.vault.id, r.st.vault.sha256, r.st.vault.size, r.st.vault_commit);
    println!("lev   {} sha256 {} ({} bytes)", r.st.lev.id, r.st.lev.sha256, r.st.lev.size);
    for (n, a) in &r.st.accounts {
        println!("account {n}: trader {} challenge {} address {} created_at {}{}", a.trader, a.challenge_id, a.address, a.created_at, if a.mock { " (mock proofs)" } else { "" });
    }
    println!("{}", r.results_table());
    for f in &r.st.findings {
        println!("finding {} [{}] {} {}", f.id, f.severity, f.sig, f.text);
    }
    for (acct, t) in &r.st.pending_finalize {
        println!("day 2: {acct} can be finalized from unix second {t}");
    }
    println!("{} transactions recorded", r.st.sigs.len());
}

fn run_command(r: &mut Rehearsal, a: &Args) -> Result<i32, String> {
    let want = |id: &str| a.only.is_empty() || a.only.iter().any(|o| o == id);
    for (name, _, _) in needs(0) {
        let b = r.sol(name);
        r.st.start_balances.entry(name.to_string()).or_insert(b);
    }
    r.save()?;
    if want("S0") {
        r.s0()?;
    }
    if want("S1") {
        r.s1()?;
    }
    if want("S2") {
        r.s2_day1()?;
        r.s2_day2()?;
    }
    if want("S2") {
        r.payout()?;
    }
    if want("S3") {
        r.s3()?;
    }
    if want("S4") {
        r.s4()?;
    }
    if want("S5") {
        r.s5()?;
    }
    let table = r.results_table();
    println!("\n{table}");
    std::fs::write(r.work.join("results.md"), &table).map_err(|e| e.to_string())?;
    let spent: u64 = r.st.start_balances.iter().map(|(n, b)| b.saturating_sub(r.sol(n))).sum();
    println!("SOL spent so far (all rehearsal keys, balances now against the first run): {}", sol(spent));
    for (acct, t) in &r.st.pending_finalize {
        if !r.st.is_done(&format!("{acct}:day2")) {
            println!("day 2: {acct} can be finalized from unix second {t}; resume with the same command (`run --resume`)");
        }
    }
    let failed = r.st.scenarios.values().any(|s| s.result == "FAIL");
    Ok(if failed { 1 } else { 0 })
}

/// Parses the validator information file the script writes: the version line, then `solana feature status`.
pub fn parse_validator_info(text: &str) -> serde_json::Value {
    let version = text.lines().next().unwrap_or("").trim().to_string();
    let mut active = 0u32;
    let mut inactive = Vec::new();
    for l in text.lines().skip(1) {
        let t = l.trim();
        if t.contains("| active") || t.contains("active since") && !t.contains("inactive") {
            active += 1;
        } else if t.contains("inactive") || t.contains("pending") {
            inactive.push(t.to_string());
        }
    }
    serde_json::json!({ "version": version, "features_active": active, "features_inactive": inactive.len(), "inactive_features": inactive })
}

fn measure_command(r: &mut Rehearsal, a: &Args) -> Result<i32, String> {
    let info = a.validator_info.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let validator = parse_validator_info(&info);
    let lev_sha = r.lev_so.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| rehearsal::sha256_hex(&b)).unwrap_or_default();
    let program = serde_json::json!({ "id": r.ids.lev.to_string(), "sha256": lev_sha, "vault_commit": r.st.vault_commit });
    let meas = r.measure_all(&validator, &program)?;
    let j = meas.to_json(&validator, &program);
    std::fs::create_dir_all(&a.out_dir).map_err(|e| e.to_string())?;
    std::fs::write(a.out_dir.join("cu-runtime.json"), serde_json::to_string_pretty(&j).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    std::fs::write(a.out_dir.join("cu-runtime.md"), runtime::to_markdown(&j)).map_err(|e| e.to_string())?;
    println!("measure: {} shapes written to {}", meas.shapes.len(), a.out_dir.display());
    if !meas.errors.is_empty() {
        println!("measure: INCOMPLETE, {} section(s) failed", meas.errors.len());
        return Ok(2);
    }
    Ok(0)
}

fn keys_command(a: &Args) -> Result<i32, String> {
    use solana_keypair::Keypair;
    use solana_signer::Signer;
    std::fs::create_dir_all(&a.keys_dir).map_err(|e| e.to_string())?;
    let mut extra: Vec<&str> = KEY_NAMES.to_vec();
    extra.extend(["program-vault", "program-lev"]);
    for n in extra {
        let path = a.keys_dir.join(format!("{n}.json"));
        if path.exists() {
            continue;
        }
        let kp = Keypair::new();
        std::fs::write(&path, serde_json::to_string(&kp.to_bytes().to_vec()).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        println!("{n} {}", kp.pubkey());
    }
    Ok(0)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod no_secrets {
    //! A grep, as a test: no output statement of the driver mentions a key file, a key's bytes, a seed or the RPC URL.
    const SOURCES: &[(&str, &str)] = &[
        ("accounts.rs", include_str!("accounts.rs")),
        ("attacks.rs", include_str!("attacks.rs")),
        ("cluster.rs", include_str!("cluster.rs")),
        ("demo.rs", include_str!("demo.rs")),
        ("events.rs", include_str!("events.rs")),
        ("ix.rs", include_str!("ix.rs")),
        ("launch.rs", include_str!("launch.rs")),
        ("live.rs", include_str!("live.rs")),
        ("manifest.rs", include_str!("manifest.rs")),
        ("main.rs", include_str!("main.rs")),
        ("measure.rs", include_str!("measure.rs")),
        ("payout.rs", include_str!("payout.rs")),
        ("rehearsal.rs", include_str!("rehearsal.rs")),
        ("report.rs", include_str!("report.rs")),
        ("scenarios.rs", include_str!("scenarios.rs")),
        ("state.rs", include_str!("state.rs")),
        ("stories.rs", include_str!("stories.rs")),
        ("vaultview.rs", include_str!("vaultview.rs")),
        ("verify.rs", include_str!("verify.rs")),
    ];

    /// Statements that write to the terminal or into a message the terminal shows.
    fn is_output(line: &str) -> bool {
        ["println!", "eprintln!", "self.say(", "Err(format!(", "return Err(", "Err(\""].iter().any(|m| line.contains(m))
    }

    #[test]
    fn no_output_statement_names_a_key_file_a_seed_a_secret_or_the_url() {
        let bad = ["keys_dir", "secret", "seed", "rpc_url", "a.rpc", "keypair", "from_bytes", "KEY_NAMES", "private"];
        let mut hits = Vec::new();
        for (file, text) in SOURCES {
            // the test modules and this module may name the words they check for
            let body = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in body.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//") || !is_output(line) {
                    continue;
                }
                for b in bad {
                    if line.contains(b) {
                        hits.push(format!("{file}:{}: {t}", n + 1));
                    }
                }
            }
        }
        // the only allowed mentions are messages that say a key is missing or malformed BY NAME (never its content or path)
        hits.retain(|h| !h.contains("key {n}:") && !h.contains("run `keys` first") && !h.contains("trader key:") && !h.contains("--keys-dir"));
        assert!(hits.is_empty(), "output statements that mention secrets or paths:\n{}", hits.join("\n"));
    }

    #[test]
    fn the_state_and_the_report_code_print_only_public_things() {
        // keys are held in `Keys` (no Debug, no Display) and only `pubkey()` is ever formatted
        let rehearsal = include_str!("rehearsal.rs");
        assert!(!rehearsal.contains("derive(Debug)]\npub struct Keys"), "Keys must not derive Debug");
        assert!(rehearsal.contains("pub struct Keys"));
    }
}

#[cfg(test)]
mod distribution_tests {
    use super::*;
    use std::collections::HashMap;

    const L: u64 = 1_000_000_000;
    const PEAK: u64 = 11_490_751_720;

    fn plan(held: &[(&str, u64)]) -> (Vec<Transfer>, u64, Option<u64>) {
        let m: HashMap<&str, u64> = held.iter().copied().collect();
        distribution_plan(&needs(PEAK), |n| m.get(n).copied().unwrap_or(0))
    }

    fn lamports(p: &[Transfer], name: &str) -> Option<u64> {
        p.iter().find(|t| t.name == name).map(|t| t.lamports)
    }

    #[test]
    fn ten_and_ten_sol_pays_the_small_keys_and_tops_the_upgrade_authority_to_its_need_plus_the_margin() {
        let (p, shortfall, skipped) = plan(&[("upgrade-authority", 10 * L), ("keeper", 10 * L)]);
        assert_eq!((shortfall, skipped), (0, None));
        assert_eq!(lamports(&p, "upgrade-authority"), Some(PEAK + L / 10 + UPGRADE_MARGIN - 10 * L));
        assert_eq!(lamports(&p, "evaluator"), Some(L / 4));
        assert_eq!(lamports(&p, "trader7"), Some(L / 20));
        assert_eq!(lamports(&p, "keeper"), None);
        let total: u64 = p.iter().map(|t| t.lamports).sum();
        assert!(10 * L - total >= 3 * L / 4, "the keeper keeps its own need");
    }

    #[test]
    fn the_top_up_is_skipped_when_the_keeper_would_not_keep_its_own_need_and_the_small_keys_need() {
        // small keys 0.73 SOL + keeper 0.75 SOL = 1.48 SOL; top-up of 1.79 SOL does not fit in 3 SOL
        let (p, shortfall, skipped) = plan(&[("upgrade-authority", 10 * L), ("keeper", 3 * L)]);
        assert_eq!(shortfall, 0);
        assert!(lamports(&p, "upgrade-authority").is_none());
        assert_eq!(lamports(&p, "evaluator"), Some(L / 4));
        let top_up = PEAK + L / 10 + UPGRADE_MARGIN - 10 * L;
        assert_eq!(skipped, Some(1_480_000_000 + top_up - 3 * L));
    }

    #[test]
    fn the_top_up_fits_exactly_at_the_boundary_and_not_one_lamport_below() {
        let top_up = PEAK + L / 10 + UPGRADE_MARGIN - 10 * L;
        let exact = 1_480_000_000 + top_up;
        let (p, _, skipped) = plan(&[("upgrade-authority", 10 * L), ("keeper", exact)]);
        assert_eq!((lamports(&p, "upgrade-authority"), skipped), (Some(top_up), None));
        let (p, _, skipped) = plan(&[("upgrade-authority", 10 * L), ("keeper", exact - 1)]);
        assert_eq!((lamports(&p, "upgrade-authority"), skipped), (None, Some(1)));
    }

    #[test]
    fn a_keeper_too_poor_for_the_small_keys_reports_the_shortfall_and_sends_nothing_to_the_upgrade_authority() {
        let (p, shortfall, skipped) = plan(&[("upgrade-authority", 10 * L), ("keeper", L)]);
        assert_eq!(shortfall, 480_000_000);
        assert_eq!(skipped, None);
        assert!(lamports(&p, "upgrade-authority").is_none());
    }

    #[test]
    fn nothing_is_sent_when_every_key_already_holds_its_need() {
        let mut held = vec![("upgrade-authority", 12 * L), ("keeper", 10 * L)];
        for (n, w, _) in needs(PEAK) {
            if n != "upgrade-authority" && n != "keeper" {
                held.push((n, w));
            }
        }
        let (p, shortfall, skipped) = plan(&held);
        assert!(p.is_empty());
        assert_eq!((shortfall, skipped), (0, None));
    }
}
