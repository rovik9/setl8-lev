//! The command line, run as a process on a scratch copy of `config/`.
mod common;
use common::*;
use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("launch-config-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/launch-markets.toml"), toml()).unwrap();
    std::fs::write(root.join("config/launch.json"), json()).unwrap();
    std::fs::write(root.join("config/launch.md"), md()).unwrap();
    root
}

fn run(root: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_launch-config")).args(args).arg("--root").arg(root).output().unwrap()
}

fn out(o: &Output) -> (i32, String, String) {
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

#[test]
fn verify_passes_on_the_committed_files() {
    let root = scratch("verify");
    let (code, so, se) = out(&run(&root, &["verify"]));
    assert_eq!((code, so.trim(), se.as_str()), (0, "OK: 21 markets, 21 PENDING flags", ""));
}

#[test]
fn production_verify_fails_while_pending_flags_remain() {
    let root = scratch("production");
    let (code, so, se) = out(&run(&root, &["verify", "--production"]));
    assert_eq!(code, 1);
    assert_eq!(so, "");
    // decision 72 (P24a): the 21 market spreads are the ONLY thing that keeps production from passing; the commissions are final
    assert!(se.contains("VIOLATION: 21 PENDING flags remain (21 market spreads, 0 class commissions)"), "{se}");
    assert_eq!(se.lines().filter(|l| l.starts_with("VIOLATION")).count(), 1, "{se}");
}

#[test]
fn production_verify_passes_once_every_flag_is_cleared() {
    let root = scratch("production-clear");
    let cleared = toml().replace("spread_pending = true", "spread_pending = false").replace("pending = true", "pending = false");
    std::fs::write(root.join("config/launch-markets.toml"), &cleared).unwrap();
    assert_eq!(out(&run(&root, &["generate"])).0, 0);
    let (code, so, se) = out(&run(&root, &["verify", "--production"]));
    assert_eq!((code, so.trim(), se.as_str()), (0, "OK: 21 markets, 0 PENDING flags (production)", ""));
    assert!(std::fs::read_to_string(root.join("config/launch.md")).unwrap().contains("**PENDING flags: 0**"));
}

#[test]
fn generate_is_idempotent_and_reproduces_the_committed_bytes() {
    let root = scratch("generate");
    std::fs::write(root.join("config/launch.json"), "stale").unwrap();
    std::fs::write(root.join("config/launch.md"), "stale").unwrap();
    let (code, so, _) = out(&run(&root, &["generate"]));
    assert_eq!(code, 0);
    assert!(so.contains("21 markets, 21 PENDING flags"));
    assert_eq!(std::fs::read_to_string(root.join("config/launch.json")).unwrap(), json());
    assert_eq!(std::fs::read_to_string(root.join("config/launch.md")).unwrap(), md());
    // a second run changes nothing
    out(&run(&root, &["generate"]));
    assert_eq!(std::fs::read_to_string(root.join("config/launch.json")).unwrap(), json());
}

#[test]
fn verify_fails_on_a_hand_edited_output_or_input() {
    let root = scratch("tamper");
    std::fs::write(root.join("config/launch.json"), json().replacen("\"15\"", "\"16\"", 1)).unwrap();
    let (code, _, se) = out(&run(&root, &["verify"]));
    assert_eq!(code, 1);
    assert!(se.contains("config/launch.json differs"), "{se}");
    // an edit of the input that is not regenerated
    let root = scratch("tamper-input");
    std::fs::write(root.join("config/launch-markets.toml"), toml().replacen("half_spread_e6 = 50", "half_spread_e6 = 51", 1)).unwrap();
    let (code, _, se) = out(&run(&root, &["verify"]));
    assert_eq!(code, 1);
    assert!(se.contains("config/launch.json differs") && se.contains("config/launch.md differs"), "{se}");
}

#[test]
fn generate_refuses_an_input_that_breaks_a_rule_and_writes_nothing() {
    let root = scratch("refuse");
    std::fs::write(root.join("config/launch-markets.toml"), toml().replacen("symbol = \"BTC\"", "symbol = \"DOGE\"", 1)).unwrap();
    std::fs::write(root.join("config/launch.json"), "untouched").unwrap();
    let (code, _, se) = out(&run(&root, &["generate"]));
    assert_eq!(code, 1);
    assert!(se.contains("VIOLATION: DOGE: not an allowed symbol for source 2"), "{se}");
    assert_eq!(std::fs::read_to_string(root.join("config/launch.json")).unwrap(), "untouched");
}

#[test]
fn bad_usage_and_missing_files() {
    let root = scratch("usage");
    let (code, _, se) = out(&run(&root, &["frobnicate"]));
    assert_eq!(code, 2);
    assert!(se.contains("usage:"));
    let (code, _, se) = out(&Command::new(env!("CARGO_BIN_EXE_launch-config")).output().unwrap());
    assert_eq!(code, 2);
    assert!(se.contains("usage:"));
    std::fs::remove_file(root.join("config/launch.json")).unwrap();
    let (code, _, se) = out(&run(&root, &["verify"]));
    assert_eq!(code, 1);
    assert!(se.contains("error:") && se.contains("launch.json"), "{se}");
    let (code, _, se) = out(&run(&root, &["probe"]));
    assert_eq!(code, 1);
    assert!(se.contains("--hl-csv is required"), "{se}");
}

#[test]
fn verify_with_probe_csvs_compares_the_committed_numbers() {
    let root = scratch("probe");
    let hl = root.join("hl.csv");
    std::fs::write(&hl, "src,recv_ms,coin,oraclePx,markPx,midPx\nws,0,xyz:AAPL,100,100,100\nws,1000,xyz:AAPL,101,101,101\n").unwrap();
    let (code, _, se) = out(&run(&root, &["verify", "--hl-csv", hl.to_str().unwrap()]));
    assert_eq!(code, 1);
    assert!(se.contains("xyz:AAPL: freeze_ms 15285 in the input, 1000 in the data"), "{se}");
}

#[test]
fn probe_prints_the_numbers() {
    let root = scratch("probe-print");
    let hl = root.join("hl.csv");
    std::fs::write(&hl, "src,recv_ms,coin,oraclePx,markPx,midPx\nws,0,xyz:AAPL,100,100,100\nws,1000,xyz:AAPL,101,101,101\nws,2000,xyz:AAPL,102,102,102\n").unwrap();
    let g1 = root.join("g1.csv");
    std::fs::write(&g1, "recv_ms,symbol,usd\n1,BTC,10\n2,BTC,30\n").unwrap();
    let gs = root.join("gs.csv");
    std::fs::write(&gs, "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\nBTC,1,1000,1,1,1\nBTC,2,3500,2,2,2\nBTC,3,4000,3,3,3\n").unwrap();
    let (code, so, _) = out(&Command::new(env!("CARGO_BIN_EXE_launch-config")).args(["probe", "--hl-csv", hl.to_str().unwrap(), "--gmx-1min", g1.to_str().unwrap(), "--gmx-samples", gs.to_str().unwrap()]).output().unwrap());
    assert_eq!(code, 0);
    assert!(so.contains("xyz:AAPL\t3\t0.00\t0.00\t0.00\t1000\tfalse\t101"), "{so}");
    assert!(so.contains("BTC\t20"), "{so}");
    assert!(so.contains("BTC\tSome(2500)"), "{so}");
}

#[test]
fn each_probe_csv_alone_triggers_its_comparison() {
    let root = scratch("probe-each");
    let g1 = root.join("g1.csv");
    std::fs::write(&g1, "recv_ms,symbol,usd\n1,BTC,81000\n").unwrap();
    let (code, _, se) = out(&run(&root, &["verify", "--gmx-1min", g1.to_str().unwrap()]));
    assert_eq!(code, 1);
    assert!(se.contains("BTC: price_usd 81131.526567 in the input, 81000 in the data"), "{se}");
    let gs = root.join("gs.csv");
    std::fs::write(&gs, "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\nBTC,1,1000,1,1,1\nBTC,2,9000,2,2,2\nBTC,3,20000,3,3,3\n").unwrap();
    let (code, _, se) = out(&run(&root, &["verify", "--gmx-samples", gs.to_str().unwrap()]));
    assert_eq!(code, 1);
    assert!(se.contains("BTC: freeze_ms 3664 is below the 11000 ms gap in the samples"), "{se}");
    // matching data: no violation
    std::fs::write(&g1, "recv_ms,symbol,usd\n1,BTC,81131.526567\n").unwrap();
    assert_eq!(out(&run(&root, &["verify", "--gmx-1min", g1.to_str().unwrap()])).0, 0);
}

#[test]
fn verify_refuses_an_unpriced_market_for_every_id_through_the_command_line() {
    // the process-level proof: a zero half-spread for market `k`, regenerated (so the files are consistent) or not, fails `verify`
    // with that market named; `generate` itself refuses to write such a rule set
    for (k, _) in toml().lines().enumerate().filter(|(_, l)| l.starts_with("half_spread_e6")) {
        let symbol_line = toml().lines().take(k + 1).filter(|l| l.starts_with("symbol")).last().unwrap().to_string();
        let sym = symbol_line.split('"').nth(1).unwrap().to_string();
        let root = scratch(&format!("unpriced-{k}"));
        let edited: String = toml().lines().enumerate().map(|(i, l)| if i == k { "half_spread_e6 = 0".to_string() } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n";
        std::fs::write(root.join("config/launch-markets.toml"), edited).unwrap();
        let (code, _, se) = out(&run(&root, &["verify"]));
        assert_eq!(code, 1, "{sym}: {se}");
        assert!(se.contains(&format!("VIOLATION: {sym}: half-spread 0 in the rule set: an unpriced market")), "{sym}: {se}");
        let (code, _, se) = out(&run(&root, &["generate"]));
        assert_eq!(code, 1, "{sym}: generate must refuse");
        assert!(se.contains("an unpriced market"), "{se}");
    }
}
