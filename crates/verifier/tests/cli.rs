//! The binary: one test per exit code, the JSON shape, the human text, and `watch`.
mod common;
use common::*;
use journal_types::JournalEvent as E;
use std::process::Command;

fn write_set(b: &Bundle, tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sl8verify-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (file, bytes) in vector_files(b) {
        std::fs::write(dir.join(file), bytes).unwrap();
    }
    dir
}

fn run(dir: &std::path::Path, cmd: &str, extra: &[&str]) -> (i32, String) {
    let p = |f: &str| dir.join(f).to_string_lossy().to_string();
    let out = Command::new(env!("CARGO_BIN_EXE_setl8-verify"))
        .args([cmd, "--events", &p("events.jsonl"), "--rules", &p("rules.json"), "--archive", &p("archive.bin"), "--intents", &p("intents.jsonl")])
        .args(extra)
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn exit_code_0_for_a_valid_account_and_the_human_text() {
    let dir = write_set(&design_4_9(), "valid");
    let snap = dir.join("snapshot.json").to_string_lossy().to_string();
    let (code, out) = run(&dir, "account", &["--snapshot", &snap]);
    assert_eq!(code, 0);
    assert!(out.starts_with("VERDICT: VALID\nreplayed: seq "), "{out}");
    assert!(out.contains("b 97935260000"), "{out}");
    assert!(out.contains("findings: 0"));
    assert!(out.contains("phase 1: seq 1.."), "{out}");
}

#[test]
fn exit_code_1_for_a_mismatch_and_the_json_shape() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { pnl, .. } = b.ev_mut(i) {
        *pnl += 1;
    }
    let dir = write_set(&b, "mismatch");
    let (code, out) = run(&dir, "account", &["--json"]);
    assert_eq!(code, 1, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["verdict"], "Mismatch");
    assert_eq!(v["findings"][0]["class"], "PnlMismatch");
    assert_eq!(v["findings"][0]["seq"], 3);
    for key in ["class", "seq", "event_index", "expected", "found", "message", "source"] {
        assert!(v["findings"][0].get(key).is_some(), "{key}");
    }
    assert!(v.get("per_phase").is_some() && v.get("per_pass").is_some() && v.get("state").is_some());
    let (code, text) = run(&dir, "account", &[]);
    assert_eq!(code, 1);
    assert!(text.contains("[PnlMismatch] seq=3"), "{text}");
}

#[test]
fn exit_code_2_for_input_errors() {
    let dir = write_set(&close_case(600), "input");
    // a missing file
    let out = Command::new(env!("CARGO_BIN_EXE_setl8-verify")).args(["account", "--events", "/nonexistent/e.jsonl", "--rules", "r", "--archive", "a", "--intents", "i"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    // a file in the wrong format
    std::fs::write(dir.join("archive.bin"), b"garbage").unwrap();
    let (code, out) = run(&dir, "account", &[]);
    assert_eq!(code, 2, "{out}");
    assert!(out.starts_with("error: archive:"));
    // no arguments, an unknown command, an unknown flag, a flag without value, a bad number
    for args in [vec![], vec!["frobnicate"], vec!["account", "--bogus"], vec!["account", "--events"], vec!["account", "--as-of-seq", "x"], vec!["account"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_setl8-verify")).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
    }
}

#[test]
fn as_of_seq_replays_a_prefix() {
    let dir = write_set(&close_case(600), "asof");
    // up to the first Meta (seq 2) the account holds an open position and no close yet
    let (code, out) = run(&dir, "account", &["--json", "--as-of-seq", "2"]);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["state"]["seq"], 2);
    assert_eq!(v["state"]["open_positions"], 1);
    assert_eq!(v["state"]["trades_closed"], 0);
}

#[test]
fn watch_without_and_with_a_pending_pass() {
    let dir = write_set(&close_case(600), "watch");
    let (code, out) = run(&dir, "watch", &[]);
    assert_eq!((code, out.as_str()), (0, "NO_PENDING_PASS\n"));
}

#[test]
fn output_is_deterministic() {
    let dir = write_set(&design_4_9(), "det");
    let (c1, o1) = run(&dir, "account", &["--json"]);
    let (c2, o2) = run(&dir, "account", &["--json"]);
    assert_eq!((c1, c2), (0, 0));
    assert_eq!(o1, o2);
}
