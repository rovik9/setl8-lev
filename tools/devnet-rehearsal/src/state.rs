//! `state.json`: what makes the run resumable. Program ids, transaction signatures, account addresses, timestamps and results,
//! public data only; never a key, a seed or a key file path.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct ProgramRec {
    pub id: String,
    pub sha256: String,
    pub size: u64,
}

/// One transaction the tool sent.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SigRec {
    pub label: String,
    pub sig: String,
    pub slot: u64,
    pub ok: bool,
    pub err: String,
    pub cu: u64,
    pub size: u64,
    pub fee: u64,
    pub block_time: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct AcctRec {
    pub name: String,
    /// The trader's name in the keys directory (`trader1`), never a path.
    pub trader_name: String,
    pub trader: String,
    pub challenge_id: u64,
    pub address: String,
    pub tier: u8,
    pub phase_at_open: u8,
    pub created_at: i64,
    /// The account's proofs are mock proofs (source 255, localnet build only) instead of the market's real source format.
    #[serde(default)]
    pub mock: bool,
    /// Story state for the day-2 part, free text for the runbook.
    pub note: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ScenarioRec {
    /// `PASS`, `FAIL` or `PENDING`.
    pub result: String,
    pub detail: String,
    pub sigs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub sig: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    pub version: u32,
    pub cluster: String,
    pub genesis: String,
    pub started_at: i64,
    pub vault_commit: String,
    pub vault: ProgramRec,
    pub lev: ProgramRec,
    /// Public addresses by name (mints, pools, vault state, fee token accounts ...).
    pub addrs: BTreeMap<String, String>,
    pub accounts: BTreeMap<String, AcctRec>,
    pub scenarios: BTreeMap<String, ScenarioRec>,
    pub sigs: Vec<SigRec>,
    pub done: BTreeSet<String>,
    pub findings: Vec<Finding>,
    /// SOL balances (lamports) of the funded keys when the run started, by key name (public data).
    pub start_balances: BTreeMap<String, u64>,
    /// Day-2 work: account name -> the second the confirmed pass can be finalized.
    pub pending_finalize: BTreeMap<String, i64>,
}

impl State {
    pub fn load(path: &Path) -> Result<State, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State { version: 1, ..Default::default() }),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes through a temporary file so a crash never leaves half a state.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn is_done(&self, label: &str) -> bool {
        self.done.contains(label)
    }

    pub fn set_scenario(&mut self, id: &str, result: &str, detail: &str, sigs: Vec<String>) {
        self.scenarios.insert(id.to_string(), ScenarioRec { result: result.to_string(), detail: detail.to_string(), sigs });
    }

    pub fn add_finding(&mut self, id: &str, severity: &str, sig: &str, text: &str) {
        if !self.findings.iter().any(|f| f.id == id) {
            self.findings.push(Finding { id: id.into(), severity: severity.into(), sig: sig.into(), text: text.into() });
        }
    }

    /// The signatures recorded under labels that start with `prefix`.
    pub fn sigs_with(&self, prefix: &str) -> Vec<String> {
        self.sigs.iter().filter(|s| s.label.starts_with(prefix)).map(|s| s.sig.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_survives_a_save_and_a_load_and_a_missing_file_is_an_empty_state() {
        let dir = std::env::temp_dir().join(format!("devnet-rehearsal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        assert_eq!(State::load(&path).unwrap().version, 1);
        let mut s = State::load(&path).unwrap();
        s.cluster = "localnet".into();
        s.done.insert("x".into());
        s.sigs.push(SigRec { label: "post_journal A1".into(), sig: "abc".into(), slot: 3, ok: true, err: String::new(), cu: 9, size: 8, fee: 5000, block_time: 7 });
        s.set_scenario("S0", "PASS", "ok", vec!["abc".into()]);
        s.add_finding("F-1", "low", "abc", "text");
        s.add_finding("F-1", "low", "abc", "again");
        s.save(&path).unwrap();
        let back = State::load(&path).unwrap();
        assert_eq!(back.cluster, "localnet");
        assert!(back.is_done("x") && !back.is_done("y"));
        assert_eq!(back.findings.len(), 1, "a finding id is recorded once");
        assert_eq!(back.sigs_with("post_journal"), vec!["abc".to_string()]);
        assert_eq!(back.scenarios["S0"].result, "PASS");
        // a corrupt file is an error, not an empty state
        std::fs::write(&path, "{").unwrap();
        assert!(State::load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_state_holds_no_key_material_by_construction() {
        // every field is a string, number or map of them; the only name that looks like a key is the trader's NAME in the keys directory
        let a = AcctRec { name: "A1".into(), trader_name: "trader1".into(), ..Default::default() };
        let text = serde_json::to_string(&a).unwrap();
        assert!(!text.contains(".json") && !text.contains("secret") && !text.contains("seed"));
    }
}
