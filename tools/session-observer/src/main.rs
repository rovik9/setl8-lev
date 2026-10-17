//! `session-observer run --until <UTC ISO> --data-dir DIR [--launch config/launch.json]` and
//! `session-observer analyze <data dir(s)> --out FILE [--launch config/launch.json]`.
use session_observer::{analyze, lanes, markets, timeutil};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  session-observer run --until <UTC ISO, e.g. 2026-10-19T21:30:00Z> --data-dir <dir> [--launch <launch.json>]
  session-observer analyze <data dir>... --out <report.md> [--launch <launch.json>] [--toml <launch-markets.toml>]";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn launch_path(args: &[String]) -> PathBuf {
    flag(args, "--launch").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("config/launch.json"))
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "run" => {
            let text = std::fs::read_to_string(launch_path(rest)).map_err(|e| format!("launch.json: {e}"))?;
            let ms = markets::parse_launch(&text)?;
            let until = flag(rest, "--until").map(|s| timeutil::parse_until(&s)).transpose()?;
            let dir = PathBuf::from(flag(rest, "--data-dir").ok_or("--data-dir is required")?);
            lanes::run(ms, dir, until)
        }
        "analyze" => analyze::cli(rest),
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
