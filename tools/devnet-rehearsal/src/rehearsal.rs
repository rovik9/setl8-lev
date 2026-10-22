//! The rehearsal's working object: the cluster, the two program ids, the throwaway keys, the state file, and everything a scenario
//! does to the chain (send a transaction and record what the runtime made of it, read accounts, wait for the cluster clock).
//! It is also the [`Net`] the stories post to.
//!
//! Safety: keys are loaded from the keys directory by NAME and are only ever used to sign; no function here returns, logs or
//! formats a secret (`Keypair` has no `Debug` output in this crate and nothing prints one). The RPC URL is never printed.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey};
use anchor_lang::AccountDeserialize;
use evaluator::{Batch, BatchKind, ChainView};
use serde_json::Value;
use setl8_lev_trading::state::{Market, RuleSet, SectorConfig, TradingAccount, WalletProfile};
use sha2::{Digest, Sha256};
use solana_keypair::Keypair;
use solana_signer::Signer;

use crate::cluster::{wall_now, Cluster, ClusterKind, RpcError, Sent};
use crate::ix::Ids;
use crate::launch::Launch;
use crate::live::{AcctRef, Live, Net};
use crate::state::{AcctRec, SigRec, State};

/// The names of the throwaway keys in the keys directory.
pub const KEY_NAMES: &[&str] = &[
    "upgrade-authority", "keeper", "evaluator", "mint-usdc", "mint-usdt", "freeze-authority", "vault-admin-sl8", "vault-admin-rov", "lev-admin-sl8",
    "lev-admin-rov", "trader1", "trader2", "trader3", "trader4", "trader5", "trader6", "trader7",
];

pub struct Keys {
    map: BTreeMap<String, Keypair>,
}

impl Keys {
    /// Loads every key of [`KEY_NAMES`] from `dir`. A missing or unreadable file names the KEY, never prints its content.
    pub fn load(dir: &Path) -> Result<Keys, String> {
        let mut map = BTreeMap::new();
        for n in KEY_NAMES {
            let path = dir.join(format!("{n}.json"));
            let text = std::fs::read_to_string(&path).map_err(|_| format!("key {n}: cannot read {n}.json in the keys directory (run `keys` first)"))?;
            let bytes: Vec<u8> = serde_json::from_str(&text).map_err(|_| format!("key {n}: not a keypair file"))?;
            let kp = Keypair::try_from(bytes.as_slice()).map_err(|_| format!("key {n}: not a keypair file"))?;
            map.insert((*n).to_string(), kp);
        }
        Ok(Keys { map })
    }

    pub fn get(&self, name: &str) -> &Keypair {
        self.map.get(name).unwrap_or_else(|| panic!("no key named {name}"))
    }

    pub fn pubkey(&self, name: &str) -> Pubkey {
        self.get(name).pubkey()
    }

    /// The name of the key with this public key.
    pub fn name_of(&self, pk: &Pubkey) -> Option<&str> {
        self.map.iter().find(|(_, k)| k.pubkey() == *pk).map(|(n, _)| n.as_str())
    }

    pub fn public_list(&self) -> Vec<(String, Pubkey)> {
        self.map.iter().map(|(n, k)| (n.clone(), k.pubkey())).collect()
    }
}

/// A transaction that did not succeed: the program error code when there is one, and a one-line text (no logs of other programs).
#[derive(Clone, Debug)]
pub struct SendErr {
    pub code: Option<u32>,
    pub text: String,
    pub sent: Option<Box<Sent>>,
}

impl std::fmt::Display for SendErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}

pub struct Rehearsal {
    pub c: Cluster,
    pub kind: ClusterKind,
    pub ids: Ids,
    pub keys: Keys,
    pub st: State,
    pub st_path: PathBuf,
    pub work: PathBuf,
    pub launch: Launch,
    /// The sink of the run's narration (stdout).
    pub quiet: bool,
    /// The scenario being run (its checks so far).
    pub cur: Option<Cur>,
    /// Where the program binaries of the build are (their sha256 is compared with what the cluster holds).
    pub vault_so: Option<PathBuf>,
    pub lev_so: Option<PathBuf>,
    /// The built `setl8-verify` binary, when there is one (S5 also runs its command line).
    pub verify_bin: Option<PathBuf>,
    /// `pass_delay_secs` given to `init_config` (the rehearsal uses the minimum, 1 day; only the LOCAL selftest of a scratch build with a shorter
    /// minimum uses less, to reach the day-2 part).
    pub pass_delay: u32,
    /// The address lookup table of the measurement (created on first use): the shared accounts of the batch instructions.
    pub alt: Option<solana_message::AddressLookupTableAccount>,
}

/// One scenario's running tally.
#[derive(Clone, Debug, Default)]
pub struct Cur {
    pub id: String,
    pub checks: u32,
    pub fails: Vec<String>,
    pub sigs_before: usize,
}

/// An in-memory copy of a key (the keys directory is read once; nothing is written or printed).
pub fn clone_key(k: &Keypair) -> Keypair {
    Keypair::try_from(k.to_bytes().as_slice()).unwrap_or_else(|_| Keypair::new())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `Some(code)` for a program error inside an RPC preflight failure (`data.err.InstructionError[_, Custom(code)]`).
pub fn preflight_code(e: &RpcError) -> Option<(usize, u32)> {
    crate::cluster::parse_custom_error(e.data.as_ref()?.get("err")?)
}

impl Rehearsal {
    pub fn say(&self, s: &str) {
        if !self.quiet {
            println!("{s}");
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.st.save(&self.st_path)
    }

    // ------------------------------------------------------------------------------------------------ scenario bookkeeping

    pub fn begin(&mut self, id: &str) {
        self.say(&format!("\n=== {id} ==="));
        self.cur = Some(Cur { id: id.to_string(), sigs_before: self.st.sigs.len(), ..Default::default() });
    }

    /// One explicit check of the scenario: printed as PASS or FAIL, a FAIL makes the scenario FAIL.
    pub fn check(&mut self, ok: bool, what: &str) -> bool {
        self.say(&format!("  [{}] {what}", if ok { "PASS" } else { "FAIL" }));
        if let Some(c) = self.cur.as_mut() {
            c.checks += 1;
            if !ok {
                c.fails.push(what.to_string());
            }
        }
        ok
    }

    /// A hard error inside a scenario: recorded as a failed check.
    pub fn fail_with(&mut self, e: &str) {
        self.check(false, e);
    }

    /// Ends the scenario: PASS when it ran at least one check and none failed. The signatures it sent are listed with it. A scenario
    /// that ran no check (everything was done by an earlier run) keeps its earlier result.
    pub fn end(&mut self, detail: &str) {
        let Some(c) = self.cur.take() else { return };
        if c.checks == 0 {
            self.say(&format!("{}: nothing new to check in this run", c.id));
            return;
        }
        let result = if c.fails.is_empty() { "PASS" } else { "FAIL" };
        let mut sigs: Vec<String> = self.st.sigs[c.sigs_before.min(self.st.sigs.len())..].iter().map(|s| s.sig.clone()).collect();
        if let Some(old) = self.st.scenarios.get(&c.id) {
            let mut merged = old.sigs.clone();
            merged.append(&mut sigs);
            sigs = merged;
        }
        let detail = if c.fails.is_empty() { detail.to_string() } else { format!("{detail}; FAILED: {}", c.fails.join(" | ")) };
        self.say(&format!("{}: {result} ({} checks, {} signatures)", c.id, c.checks, sigs.len()));
        self.st.set_scenario(&c.id, result, &detail, sigs);
        let _ = self.save();
    }

    // ------------------------------------------------------------------------------------------------ sending

    /// Sends one transaction and records it. `Err` carries the program error code when the runtime refused it. With `skip_preflight`
    /// a transaction the simulation would refuse lands on chain and fails there (its signature, CU and error are recorded).
    pub fn send(&mut self, label: &str, ixs: &[Instruction], payer: &str, signers: &[&str], skip_preflight: bool) -> Result<Sent, SendErr> {
        let payer_kp = clone_key(self.keys.get(payer));
        let sg: Vec<Keypair> = signers.iter().map(|n| clone_key(self.keys.get(n))).collect();
        let sg_refs: Vec<&Keypair> = sg.iter().collect();
        self.send_kp(label, ixs, &payer_kp, &sg_refs, skip_preflight)
    }

    /// `send` with keypairs the caller holds (ephemeral measurement keys that are not in the keys directory).
    pub fn send_kp(&mut self, label: &str, ixs: &[Instruction], payer_kp: &Keypair, sg: &[&Keypair], skip_preflight: bool) -> Result<Sent, SendErr> {
        self.send_kp_tables(label, ixs, payer_kp, sg, skip_preflight, &[])
    }

    /// `send_kp` as a version-0 transaction with the given address lookup tables (none: a legacy transaction).
    pub fn send_kp_tables(&mut self, label: &str, ixs: &[Instruction], payer_kp: &Keypair, sg: &[&Keypair], skip_preflight: bool, tables: &[solana_message::AddressLookupTableAccount]) -> Result<Sent, SendErr> {
        let r = self.c.send_with(ixs, payer_kp, sg, skip_preflight, tables);
        match r {
            Ok(sent) => {
                let ok = sent.info.ok();
                self.st.sigs.push(SigRec {
                    label: label.to_string(),
                    sig: sent.info.signature.clone(),
                    slot: sent.info.slot,
                    ok,
                    err: if ok { String::new() } else { sent.info.err.to_string() },
                    cu: sent.info.cu,
                    size: sent.size as u64,
                    fee: sent.info.fee,
                    block_time: sent.info.block_time.unwrap_or(0),
                });
                let _ = self.save();
                if ok {
                    Ok(sent)
                } else {
                    let code = sent.info.custom_error().map(|c| c.1);
                    Err(SendErr { code, text: format!("{label}: failed on chain: {}", sent.info.err), sent: Some(Box::new(sent)) })
                }
            }
            Err(e) => {
                let code = preflight_code(&e).map(|c| c.1);
                let err_text = e.data.as_ref().and_then(|d| d.get("err")).map(|x| x.to_string()).unwrap_or_default();
                let logs = e.data.as_ref().and_then(|d| d.get("logs")).and_then(|l| l.as_array()).map(|l| l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" | ")).unwrap_or_default();
                Err(SendErr { code, text: format!("{label}: {} {err_text} {logs}", e), sent: None })
            }
        }
    }

    /// Sends a transaction that MUST fail with the program error `want` (`None` = any program error), landed on the real runtime
    /// (no preflight). Returns the signature when it failed as expected.
    pub fn send_expect_error(&mut self, label: &str, ixs: &[Instruction], payer: &str, signers: &[&str], want: u32) -> Result<String, String> {
        match self.send(label, ixs, payer, signers, true) {
            Ok(s) => Err(format!("{label}: the transaction SUCCEEDED ({}), expected error {want}", s.info.signature)),
            Err(e) => match (e.code, &e.sent) {
                (Some(c), Some(s)) if c == want => Ok(s.info.signature.clone()),
                (c, Some(s)) => Err(format!("{label}: error {c:?} instead of {want} ({})", s.info.signature)),
                (c, None) => Err(format!("{label}: error {c:?} instead of {want}: {}", e.text)),
            },
        }
    }

    // ------------------------------------------------------------------------------------------------ reading

    pub fn read<T: AccountDeserialize>(&self, key: &Pubkey) -> Result<Option<T>, String> {
        let Some(a) = self.c.account(key).map_err(|e| e.to_string())? else { return Ok(None) };
        let mut data: &[u8] = &a.data;
        T::try_deserialize(&mut data).map(Some).map_err(|e| format!("{key}: {e}"))
    }

    pub fn trading_account(&self, trader: &Pubkey, challenge_id: u64) -> Result<TradingAccount, String> {
        self.read::<TradingAccount>(&self.ids.acct(trader, challenge_id))?.ok_or_else(|| format!("account {} does not exist", self.ids.acct(trader, challenge_id)))
    }

    pub fn wallet_profile(&self, trader: &Pubkey) -> Result<Option<WalletProfile>, String> {
        self.read::<WalletProfile>(&self.ids.wallet(trader))
    }

    pub fn config(&self) -> Result<SectorConfig, String> {
        self.read::<SectorConfig>(&self.ids.config())?.ok_or_else(|| "the sector config does not exist".to_string())
    }

    pub fn ruleset(&self, version: u16) -> Result<RuleSet, String> {
        self.read::<RuleSet>(&self.ids.ruleset(version))?.ok_or_else(|| format!("rule set {version} does not exist"))
    }

    pub fn market(&self, id: u16) -> Result<Market, String> {
        self.read::<Market>(&self.ids.market(id))?.ok_or_else(|| format!("market {id} does not exist"))
    }

    /// The amount of an SPL token account (0 when it does not exist).
    pub fn token_balance(&self, key: &Pubkey) -> u64 {
        match self.c.account(key) {
            Ok(Some(a)) if a.data.len() >= 72 => u64::from_le_bytes(a.data[64..72].try_into().unwrap_or([0; 8])),
            _ => 0,
        }
    }

    pub fn sol(&self, name: &str) -> u64 {
        self.c.balance(&self.keys.pubkey(name)).unwrap_or(0)
    }

    pub fn exists(&self, key: &Pubkey) -> bool {
        matches!(self.c.account(key), Ok(Some(_)))
    }

    // ------------------------------------------------------------------------------------------------ accounts of the run

    /// The reference of a named account of the run.
    pub fn acct_ref(&self, name: &str) -> Result<AcctRef, String> {
        let r = self.st.accounts.get(name).ok_or_else(|| format!("no account {name} in the state"))?;
        Ok(AcctRef { name: name.to_string(), trader: r.trader.parse().map_err(|e| format!("{e}"))?, challenge_id: r.challenge_id })
    }

    pub fn trader_name_of(&self, a: &AcctRef) -> Result<String, String> {
        self.keys.name_of(&a.trader).map(str::to_string).ok_or_else(|| format!("{}: trader is not one of the throwaway keys", a.name))
    }

    pub fn record_account(&mut self, name: &str, trader_name: &str, challenge_id: u64, tier: u8) -> Result<(), String> {
        let trader = self.keys.pubkey(trader_name);
        let acct = self.ids.acct(&trader, challenge_id);
        let on_chain = self.trading_account(&trader, challenge_id)?;
        self.st.accounts.insert(
            name.to_string(),
            AcctRec {
                name: name.to_string(),
                trader_name: trader_name.to_string(),
                trader: trader.to_string(),
                challenge_id,
                address: acct.to_string(),
                tier,
                phase_at_open: on_chain.phase,
                created_at: on_chain.created_at,
                mock: false,
                note: String::new(),
            },
        );
        self.save()
    }

    /// A [`Live`] for a recorded account: the evaluator starts at the account's creation second in its opening phase, with the
    /// launch markets (mock proofs) and the story's starting prices. The story code is deterministic, so a resumed run rebuilds
    /// the same evaluator by running the same stories again over what is already on chain.
    pub fn live_for(&self, name: &str, start: &[(u16, u64)]) -> Result<Live, String> {
        let rec = self.st.accounts.get(name).ok_or_else(|| format!("no account {name}"))?;
        let trader = self.keys.get(&rec.trader_name);
        let on_chain = self.trading_account(&trader.pubkey(), rec.challenge_id)?;
        let params = evaluator::AccountParams {
            account: self.ids.acct(&trader.pubkey(), rec.challenge_id).to_bytes(),
            trader: trader.pubkey().to_bytes(),
            challenge_id: rec.challenge_id,
            size_micro: i64::try_from(on_chain.size_s).map_err(|e| e.to_string())?,
            phase: rec.phase_at_open,
            tier_index: rec.tier,
            opened_time: rec.created_at,
        };
        let mut rules = self.launch.rules_cfg(rec.mock);
        rules.ruleset_version = on_chain.ruleset_version;
        Live::new(rules, params, &trader.to_bytes(), start)
    }

    /// Writes the evaluator's own files for an account (the sample archive and the signed intents) next to the state.
    pub fn save_account_files(&self, name: &str, l: &Live) -> Result<(), String> {
        std::fs::create_dir_all(&self.work).map_err(|e| e.to_string())?;
        std::fs::write(self.work.join(format!("{name}.archive.bin")), l.ev.archive_bytes()).map_err(|e| e.to_string())?;
        std::fs::write(self.work.join(format!("{name}.intents.jsonl")), l.ev.intents_bytes()).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn expect_account_fields(a: &TradingAccount, b: &Batch) -> Result<(), String> {
        let e = &b.account_after;
        let mut bad = Vec::new();
        macro_rules! chk {
            ($name:expr, $got:expr, $want:expr) => {
                if $got != $want {
                    bad.push(format!("{}: chain {:?} != evaluator {:?}", $name, $got, $want));
                }
            };
        }
        chk!("b", a.b, e.b);
        chk!("q_tally", a.q_tally, e.q_tally);
        chk!("lock_until", a.lock_until, e.lock_until);
        chk!("last_batch_time", a.last_batch_time, e.last_batch_time);
        chk!("seq", a.seq, e.seq);
        chk!("journal_hash", a.journal_hash, e.journal_hash);
        chk!("trades_closed", a.trades_closed, e.trades_closed);
        chk!("latch", a.latch, u8::from(e.latch));
        chk!("flat", a.flat, u8::from(e.flat));
        chk!("tight_on", a.tight_on, u8::from(e.tight_on));
        chk!("r", a.r, e.r);
        chk!("g_peak", a.g_peak, e.g_peak);
        chk!("l_level", a.l_level, e.l_level);
        if e.failed {
            chk!("status", a.status, 1u8);
        }
        if bad.is_empty() {
            Ok(())
        } else {
            Err(bad.join("; "))
        }
    }
}

impl Net for Rehearsal {
    fn post(&mut self, a: &AcctRef, b: &Batch) -> Result<(), String> {
        let cur = self.trading_account(&a.trader, a.challenge_id)?;
        if cur.seq >= b.account_after.seq {
            // already on chain (a resumed run replays its stories): it must be THE batch
            if cur.seq == b.account_after.seq && cur.journal_hash != b.account_after.journal_hash {
                return Err(format!("{}: the chain head at seq {} differs from the replayed history", a.name, cur.seq));
            }
            return Ok(());
        }
        if b.batch.first_seq != cur.seq + 1 {
            return Err(format!("{}: batch starts at seq {} but the chain is at {}", a.name, b.batch.first_seq, cur.seq));
        }
        self.c.wait_until(b.meta_time).map_err(|e| e.to_string())?;
        let evaluator = self.keys.pubkey("evaluator");
        let bytes = b.batch.to_bytes().map_err(|e| format!("batch encoding: {e:?}"))?;
        // the cluster's own clock decides (see `wait_until_block_time`); a TimeInFuture (6009) is waited out and tried again
        let time_in_future = u32::from(setl8_lev_trading::errors::Setl8Error::TimeInFuture);
        let (ix, label) = match b.kind {
            BatchKind::PostJournal => (
                crate::ix::post_journal(&self.ids, &evaluator, &a.trader, a.challenge_id, cur.ruleset_version, bytes, &b.markets),
                format!("post_journal ev={} pr={} {}", b.batch.events.len(), b.batch.proofs.len(), a.name),
            ),
            BatchKind::ReportFail(r) => (
                crate::ix::report_fail(&self.ids, &evaluator, &a.trader, a.challenge_id, cur.ruleset_version, bytes, r as u8, &b.markets),
                format!("report_fail ev={} pr={} {}", b.batch.events.len(), b.batch.proofs.len(), a.name),
            ),
        };
        let mut attempt = 0;
        let sent = loop {
            self.c.wait_until_block_time(b.meta_time).map_err(|e| e.to_string())?;
            match self.send(&label, std::slice::from_ref(&ix), "evaluator", &[], false) {
                Ok(s) => break s,
                Err(e) if e.code == Some(time_in_future) && attempt < 8 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
                Err(e) => return Err(format!("{} (seq {})", e.text, b.batch.first_seq)),
            }
        };
        if sent.size != b.tx_bytes {
            self.say(&format!("  note: {label}: the transaction is {} bytes, the evaluator's model says {}", sent.size, b.tx_bytes));
        }
        let after = self.trading_account(&a.trader, a.challenge_id)?;
        Self::expect_account_fields(&after, b).map_err(|d| format!("{}: the stored account differs from the evaluator after seq {}: {d}", a.name, b.account_after.seq))
    }

    fn view(&mut self, a: &AcctRef) -> Result<ChainView, String> {
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        Ok(ChainView { seq: acct.seq, journal_hash: acct.journal_hash, b: acct.b, flat: acct.flat == 1, phase: acct.phase })
    }

    fn pass_done(&self, a: &AcctRef, t: i64) -> bool {
        self.st.is_done(&format!("{}:request@{t}", a.name))
    }

    fn request_advance(&mut self, a: &AcctRef, t: i64) -> Result<bool, String> {
        let label = format!("{}:request@{t}", a.name);
        if self.st.is_done(&label) {
            return Ok(true);
        }
        self.c.wait_until(t).map_err(|e| e.to_string())?;
        let trader = self.trader_name_of(a)?;
        let ix = crate::ix::request_advance(&self.ids, &a.trader, a.challenge_id);
        match self.send(&format!("request_advance {}", a.name), &[ix], &trader, &[], false) {
            Ok(_) => {
                self.st.done.insert(label);
                self.save()?;
                Ok(true)
            }
            // TargetNotReached 6021, NotFlat 6020, TightModeOn 6085, PhaseFailed 6086
            Err(e) if matches!(e.code, Some(6020 | 6021 | 6085 | 6086)) => Ok(false),
            Err(e) => Err(e.text),
        }
    }

    fn confirm_pass(&mut self, a: &AcctRef, t: i64) -> Result<(), String> {
        let label = format!("{}:confirm@{t}", a.name);
        if self.st.is_done(&label) {
            return Ok(());
        }
        self.c.wait_until(t).map_err(|e| e.to_string())?;
        let ev = self.keys.pubkey("evaluator");
        let ix = crate::ix::confirm_pass(&self.ids, &ev, &a.trader, a.challenge_id);
        self.send(&format!("confirm_pass {}", a.name), &[ix], "evaluator", &[], false).map_err(|e| e.text)?;
        self.st.done.insert(label);
        self.save()
    }

    fn reject_pass(&mut self, a: &AcctRef, t: i64, reason: u8) -> Result<(), String> {
        let label = format!("{}:reject@{t}", a.name);
        if self.st.is_done(&label) {
            return Ok(());
        }
        self.c.wait_until(t).map_err(|e| e.to_string())?;
        let ev = self.keys.pubkey("evaluator");
        let ix = crate::ix::reject_pass(&self.ids, &ev, &a.trader, a.challenge_id, reason);
        self.send(&format!("reject_pass {}", a.name), &[ix], "evaluator", &[], false).map_err(|e| e.text)?;
        self.st.done.insert(label);
        self.save()
    }

    fn hold(&mut self, a: &AcctRef, t: i64) -> Result<(), String> {
        let label = format!("{}:hold@{t}", a.name);
        if self.st.is_done(&label) {
            return Ok(());
        }
        self.c.wait_until(t).map_err(|e| e.to_string())?;
        let ev = self.keys.pubkey("evaluator");
        let ix = crate::ix::hold_account(&self.ids, &ev, &a.trader, a.challenge_id, [0x5A; 32]);
        self.send(&format!("hold_account {}", a.name), &[ix], "evaluator", &[], false).map_err(|e| e.text)?;
        self.st.done.insert(label);
        self.save()
    }
}

/// Whole-dollar display of micro-USD (6 decimals), for the narration.
pub fn usd(micro: u64) -> String {
    format!("{}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

/// SOL display of lamports.
pub fn sol(lamports: u64) -> String {
    format!("{}.{:09}", lamports / 1_000_000_000, lamports % 1_000_000_000)
}

pub fn started_now(st: &mut State) {
    if st.started_at == 0 {
        st.started_at = wall_now();
    }
}

#[allow(dead_code)]
fn _json_is_used(_: &Value) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_helpers_format_six_and_nine_decimals() {
        assert_eq!(usd(30_000_000), "30.000000");
        assert_eq!(usd(1), "0.000001");
        assert_eq!(sol(1_500_000_000), "1.500000000");
        assert_eq!(sol(5), "0.000000005");
    }

    #[test]
    fn a_preflight_failure_yields_its_program_error() {
        let e = RpcError {
            method: "sendTransaction".into(),
            message: "Transaction simulation failed".into(),
            data: Some(serde_json::json!({"err": {"InstructionError": [0, {"Custom": 6109}]}, "logs": []})),
        };
        assert_eq!(preflight_code(&e), Some((0, 6109)));
        let other = RpcError { method: "x".into(), message: "m".into(), data: None };
        assert_eq!(preflight_code(&other), None);
    }

    #[test]
    fn sha256_hex_is_lowercase_hex() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn a_key_file_that_is_not_a_keypair_is_refused_without_echoing_it() {
        let dir = std::env::temp_dir().join(format!("devnet-rehearsal-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in KEY_NAMES {
            let kp = Keypair::new();
            std::fs::write(dir.join(format!("{n}.json")), serde_json::to_string(&kp.to_bytes().to_vec()).unwrap()).unwrap();
        }
        assert!(Keys::load(&dir).is_ok());
        std::fs::write(dir.join("keeper.json"), "[1,2,3] SECRET-LOOKING-TEXT").unwrap();
        let err = match Keys::load(&dir) {
            Ok(_) => panic!("must refuse"),
            Err(e) => e,
        };
        assert!(err.contains("keeper") && !err.contains("SECRET-LOOKING-TEXT") && !err.contains("[1,2,3]"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
