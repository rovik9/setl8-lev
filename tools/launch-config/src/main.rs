//! `launch-config generate | verify [--production] | probe`. No network, no keys.
use launch_config::{check, emit, generate, parse, probe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  launch-config generate [--root DIR]
  launch-config verify [--production] [--root DIR] [--hl-csv F] [--gmx-1min F] [--gmx-samples F]
  launch-config probe --hl-csv F --gmx-1min F [--gmx-samples F]
--root DIR is the repository root (default: the nearest ancestor of the working directory that has a config/ directory).";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn root(args: &[String]) -> PathBuf {
    if let Some(r) = flag(args, "--root") {
        return PathBuf::from(r);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.ancestors().find(|p| p.join("config").join("launch-markets.toml").is_file()).map(Path::to_path_buf).unwrap_or(cwd)
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "generate" => {
            let dir = root(rest).join("config");
            let input = parse(&read(&dir.join("launch-markets.toml"))?)?;
            let launch = generate(&input)?;
            let violations = check::all(&input, &launch, false);
            if !violations.is_empty() {
                violations.iter().for_each(|v| eprintln!("VIOLATION: {v}"));
                return Ok(ExitCode::FAILURE);
            }
            std::fs::write(dir.join("launch.json"), emit::json(&launch)).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("launch.md"), emit::markdown(&input, &launch)).map_err(|e| e.to_string())?;
            println!("wrote config/launch.json and config/launch.md: {} markets, {} PENDING flags", launch.markets.len(), emit::pending_flags(&input));
            Ok(ExitCode::SUCCESS)
        }
        "verify" => {
            let production = rest.iter().any(|a| a == "--production");
            let dir = root(rest).join("config");
            let input = parse(&read(&dir.join("launch-markets.toml"))?)?;
            let launch = generate(&input)?;
            let mut v = check::all(&input, &launch, production);
            v.extend(check::files(&input, &launch, &read(&dir.join("launch.json"))?, &read(&dir.join("launch.md"))?));
            let csv = |name: &str| flag(rest, name).map(|p| read(Path::new(&p))).transpose();
            let (hl, g1, gs) = (csv("--hl-csv")?, csv("--gmx-1min")?, csv("--gmx-samples")?);
            if hl.is_some() || g1.is_some() || gs.is_some() {
                v.extend(check::against_probe_data(&input, hl.as_deref(), g1.as_deref(), gs.as_deref())?);
            }
            if v.is_empty() {
                println!("OK: {} markets, {} PENDING flags{}", launch.markets.len(), emit::pending_flags(&input), if production { " (production)" } else { "" });
                Ok(ExitCode::SUCCESS)
            } else {
                v.iter().for_each(|x| eprintln!("VIOLATION: {x}"));
                Ok(ExitCode::FAILURE)
            }
        }
        "probe" => {
            let hl = read(Path::new(&flag(rest, "--hl-csv").ok_or("--hl-csv is required")?))?;
            let g1 = read(Path::new(&flag(rest, "--gmx-1min").ok_or("--gmx-1min is required")?))?;
            println!("# Hyperliquid oracle (WebSocket rows): coin, messages, median bp, p95 bp, max bp, longest gap between oracle changes ms, never changed, median oraclePx");
            for (coin, series) in probe::parse_hl_oracle(&hl)? {
                if let Some(s) = probe::oracle_stats(&series) {
                    println!("{coin}\t{}\t{:.2}\t{:.2}\t{:.2}\t{}\t{}\t{}", s.messages, s.median_bp, s.p95_bp, s.max_bp, s.freeze_ms, s.never_changed, s.price_median);
                }
            }
            println!("# GMX 1-minute tickers: symbol, median USD");
            for (sym, p) in probe::gmx_price_medians(&g1)? {
                println!("{sym}\t{p}");
            }
            if let Some(p) = flag(rest, "--gmx-samples") {
                let csv = read(Path::new(&p))?;
                println!("# GMX samples: symbol, longest gap between price changes ms");
                for sym in ["BTC", "ETH", "SOL", "GOLD", "SPY", "WTIOIL"] {
                    println!("{sym}\t{:?}", probe::gmx_longest_gap_ms(&csv, sym)?);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
