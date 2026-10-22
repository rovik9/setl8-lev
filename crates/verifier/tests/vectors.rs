//! The committed vector files are the output of the test-only generator (real engine and journal-types); this test
//! regenerates them and compares byte for byte. `SL8_WRITE_VECTORS=1 cargo test -p setl8-lev-verifier --test vectors`
//! rewrites them. A second test verifies the files FROM DISK through the CLI entry point.
mod common;
use common::*;

#[test]
fn vectors_are_the_generators_output() {
    for (name, b) in vector_sets() {
        let dir = vector_dir(name);
        for (file, bytes) in vector_files(&b) {
            let path = dir.join(file);
            if std::env::var("SL8_WRITE_VECTORS").is_ok() {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &bytes).unwrap();
            }
            let on_disk = std::fs::read(&path).unwrap_or_else(|_| panic!("missing vector {}; run with SL8_WRITE_VECTORS=1", path.display()));
            assert_eq!(on_disk, bytes, "{} differs from the generator", path.display());
        }
        let expected = serde_json::to_string_pretty(&expected_json(&b)).unwrap() + "\n";
        let path = dir.join("expected.json");
        if std::env::var("SL8_WRITE_VECTORS").is_ok() {
            std::fs::write(&path, &expected).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected, "{}", path.display());
    }
}

fn expected_json(b: &Bundle) -> serde_json::Value {
    let r = verify(b);
    serde_json::json!({"verdict": "VALID", "findings": r.findings.len(), "state": r.state})
}

fn run_cli(dir: &std::path::Path, extra: &[&str]) -> (i32, String) {
    let p = |f: &str| dir.join(f).to_string_lossy().to_string();
    let mut args: Vec<String> = vec!["account".into(), "--events".into(), p("events.jsonl"), "--rules".into(), p("rules.json"), "--archive".into(), p("archive.bin"), "--intents".into(), p("intents.jsonl"), "--snapshot".into(), p("snapshot.json")];
    args.extend(extra.iter().map(|s| s.to_string()));
    let mut out = String::new();
    let code = setl8_verify::cli::run(&args, &|path| std::fs::read(path), &mut out);
    (code, out)
}

#[test]
fn vectors_from_disk_verify_valid_through_the_cli() {
    for (name, _) in vector_sets() {
        let (code, out) = run_cli(&vector_dir(name), &[]);
        assert_eq!(code, 0, "{name}: {out}");
        assert!(out.starts_with("VERDICT: VALID"), "{out}");
        let (code, json) = run_cli(&vector_dir(name), &["--json"]);
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let expected: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vector_dir(name).join("expected.json")).unwrap()).unwrap();
        assert_eq!(v["state"], expected["state"]);
        assert_eq!(v["verdict"], "Valid");
    }
}

#[test]
fn the_4_9_vector_carries_the_design_numbers() {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vector_dir("design_4_9").join("expected.json")).unwrap()).unwrap();
    assert_eq!(v["state"]["b"], 97_935_260_000i64);
    assert_eq!(v["state"]["flat"], true);
    assert_eq!(v["state"]["trades_closed"], 1);
}
