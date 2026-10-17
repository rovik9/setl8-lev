#![allow(dead_code)]
use launch_config::{generate, parse, Input, Launch};

use std::sync::OnceLock;

/// The committed configuration, read at run time: from `LAUNCH_ROOT` when set (cargo-mutants runs the tests in a copy of the
/// crate), else from the repository this crate lives in.
fn read(name: &str, cell: &'static OnceLock<String>) -> &'static str {
    cell.get_or_init(|| {
        let root = std::env::var("LAUNCH_ROOT").unwrap_or_else(|_| format!("{}/../..", env!("CARGO_MANIFEST_DIR")));
        std::fs::read_to_string(format!("{root}/config/{name}")).unwrap_or_else(|e| panic!("{root}/config/{name}: {e}"))
    })
}

pub fn toml() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    read("launch-markets.toml", &C)
}

pub fn json() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    read("launch.json", &C)
}

pub fn md() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    read("launch.md", &C)
}

pub fn base() -> Input {
    parse(toml()).expect("the committed input parses")
}

pub fn launch(i: &Input) -> Launch {
    generate(i).expect("the committed input generates")
}

pub fn idx(i: &Input, symbol: &str) -> usize {
    i.market.iter().position(|m| m.symbol == symbol).unwrap_or_else(|| panic!("no market {symbol}"))
}

/// Whether any violation sentence contains `needle`.
pub fn has(v: &[String], needle: &str) -> bool {
    v.iter().any(|s| s.contains(needle))
}
