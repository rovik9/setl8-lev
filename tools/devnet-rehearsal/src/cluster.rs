//! A thin JSON-RPC client over `ureq` (no solana-client): the calls the rehearsal needs, a transaction sender that records what
//! the runtime did (signature, compute units, size, error), and the cluster clock.
//!
//! The RPC URL is NEVER printed (it may carry an API key); errors name the method only. Nothing here reads or prints a key.
use std::time::{Duration, Instant};

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey};
use base64::Engine as _;
use serde_json::{json, Value};
use solana_keypair::Keypair;
use solana_message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::{versioned::VersionedTransaction, Transaction};

pub const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";

/// Which cluster the driver is allowed to talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterKind {
    Devnet,
    /// A local `solana-test-validator` (the selftest): 127.0.0.1 or localhost only.
    Local,
}

/// Refuses every RPC URL that is not clearly the intended cluster. Devnet: the URL must contain "devnet" and not "mainnet".
/// Local: only `http://127.0.0.1:<port>` or `http://localhost:<port>`.
pub fn check_url(kind: ClusterKind, url: &str) -> Result<(), String> {
    let u = url.to_ascii_lowercase();
    match kind {
        ClusterKind::Devnet => {
            if u.contains("mainnet") {
                return Err("refusing: the RPC URL contains 'mainnet'".into());
            }
            if !u.contains("devnet") {
                return Err("refusing: the RPC URL must contain 'devnet'".into());
            }
            Ok(())
        }
        ClusterKind::Local => {
            let ok = u.starts_with("http://127.0.0.1:") || u.starts_with("http://localhost:");
            if ok {
                Ok(())
            } else {
                Err("refusing: the selftest talks to a local validator only (http://127.0.0.1:<port>)".into())
            }
        }
    }
}

/// Refuses a node whose genesis hash is not the expected one (devnet's, or the local validator's, given on the command line).
pub fn check_genesis(kind: ClusterKind, got: &str, expect: &str) -> Result<(), String> {
    if kind == ClusterKind::Devnet && expect != DEVNET_GENESIS {
        return Err(format!("refusing: --expect-genesis is {expect}, devnet's is {DEVNET_GENESIS}"));
    }
    if got != expect {
        return Err(format!("refusing: the node's genesis hash is {got}, not {expect}"));
    }
    Ok(())
}

pub struct Rpc {
    url: String,
    agent: ureq::Agent,
    pub calls: std::cell::Cell<u64>,
}

#[derive(Debug, Clone)]
pub struct RpcError {
    pub method: String,
    pub message: String,
    /// The JSON-RPC `error.data` (simulation failures carry `err` and `logs` here).
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rpc {}: {}", self.method, self.message)
    }
}

impl Rpc {
    pub fn new(url: &str) -> Rpc {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(40)).build();
        Rpc { url: url.to_string(), agent, calls: std::cell::Cell::new(0) }
    }

    /// One JSON-RPC call; retries on HTTP 429 / 5xx and on transport errors with a growing pause.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut last = String::new();
        for attempt in 0..8u64 {
            self.calls.set(self.calls.get() + 1);
            match self.agent.post(&self.url).set("content-type", "application/json").send_json(body.clone()) {
                Ok(resp) => {
                    let v: Value = resp.into_json().map_err(|e| RpcError { method: method.into(), message: format!("bad JSON: {e}"), data: None })?;
                    if let Some(err) = v.get("error") {
                        let msg = err.get("message").and_then(Value::as_str).unwrap_or("error").to_string();
                        return Err(RpcError { method: method.into(), message: msg, data: err.get("data").cloned() });
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(ureq::Error::Status(code, _)) if code == 429 || code >= 500 => {
                    last = format!("HTTP {code}");
                }
                Err(ureq::Error::Status(code, _)) => {
                    return Err(RpcError { method: method.into(), message: format!("HTTP {code}"), data: None });
                }
                Err(ureq::Error::Transport(t)) => {
                    // the transport error text can contain the URL: keep only its kind
                    last = format!("transport error ({:?})", t.kind());
                }
            }
            std::thread::sleep(Duration::from_millis(400 * (attempt + 1)));
        }
        Err(RpcError { method: method.into(), message: format!("gave up: {last}"), data: None })
    }
}

/// What the cluster reports about one executed transaction (the parts the rehearsal reads).
#[derive(Clone, Debug, Default)]
pub struct TxInfo {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    /// The `err` of the transaction status, as JSON (`null` = success).
    pub err: Value,
    pub fee: u64,
    pub cu: u64,
    pub logs: Vec<String>,
    /// All account keys of the message (static keys), in message order.
    pub keys: Vec<Pubkey>,
    /// Top-level instructions: (program id, account indexes, data).
    pub ixs: Vec<RawIx>,
    /// Inner instructions: (index of the top-level instruction, the inner ones).
    pub inner: Vec<(usize, Vec<RawIx>)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawIx {
    pub program: Pubkey,
    pub accounts: Vec<usize>,
    pub data: Vec<u8>,
}

impl TxInfo {
    pub fn ok(&self) -> bool {
        self.err.is_null()
    }

    /// `Some(code)` when the transaction failed with `InstructionError(_, Custom(code))`.
    pub fn custom_error(&self) -> Option<(usize, u32)> {
        parse_custom_error(&self.err)
    }
}

/// `{"InstructionError":[idx,{"Custom":code}]}` -> `(idx, code)`.
pub fn parse_custom_error(err: &Value) -> Option<(usize, u32)> {
    let arr = err.get("InstructionError")?.as_array()?;
    let idx = usize::try_from(arr.first()?.as_u64()?).ok()?;
    let code = u32::try_from(arr.get(1)?.get("Custom")?.as_u64()?).ok()?;
    Some((idx, code))
}

fn pk(s: &str) -> Result<Pubkey, String> {
    s.parse::<Pubkey>().map_err(|e| format!("bad pubkey {s}: {e}"))
}

fn raw_ix(keys: &[Pubkey], v: &Value) -> Result<RawIx, String> {
    let pi = usize::try_from(v.get("programIdIndex").and_then(Value::as_u64).ok_or("programIdIndex")?).map_err(|e| e.to_string())?;
    let program = *keys.get(pi).ok_or("programIdIndex out of range")?;
    let accounts = v.get("accounts").and_then(Value::as_array).ok_or("accounts")?.iter().filter_map(|a| a.as_u64().and_then(|x| usize::try_from(x).ok())).collect();
    let data = bs58::decode(v.get("data").and_then(Value::as_str).ok_or("data")?).into_vec().map_err(|e| format!("data: {e}"))?;
    Ok(RawIx { program, accounts, data })
}

/// Parses the result of `getTransaction` (encoding `json`, version 0 allowed). Address-table lookups are not used by this tool; their
/// loaded addresses are appended after the static keys if the node reports them.
pub fn parse_transaction(sig: &str, v: &Value) -> Result<TxInfo, String> {
    let msg = v.pointer("/transaction/message").ok_or("no message")?;
    let mut keys: Vec<Pubkey> = Vec::new();
    for k in msg.get("accountKeys").and_then(Value::as_array).ok_or("accountKeys")? {
        keys.push(pk(k.as_str().ok_or("key")?)?);
    }
    for part in ["writable", "readonly"] {
        if let Some(a) = v.pointer(&format!("/meta/loadedAddresses/{part}")).and_then(Value::as_array) {
            for k in a {
                keys.push(pk(k.as_str().ok_or("loaded key")?)?);
            }
        }
    }
    let mut ixs = Vec::new();
    for i in msg.get("instructions").and_then(Value::as_array).ok_or("instructions")? {
        ixs.push(raw_ix(&keys, i)?);
    }
    let mut inner = Vec::new();
    if let Some(groups) = v.pointer("/meta/innerInstructions").and_then(Value::as_array) {
        for g in groups {
            let idx = usize::try_from(g.get("index").and_then(Value::as_u64).ok_or("inner index")?).map_err(|e| e.to_string())?;
            let mut list = Vec::new();
            for i in g.get("instructions").and_then(Value::as_array).ok_or("inner instructions")? {
                list.push(raw_ix(&keys, i)?);
            }
            inner.push((idx, list));
        }
    }
    Ok(TxInfo {
        signature: sig.to_string(),
        slot: v.get("slot").and_then(Value::as_u64).unwrap_or(0),
        block_time: v.get("blockTime").and_then(Value::as_i64),
        err: v.pointer("/meta/err").cloned().unwrap_or(Value::Null),
        fee: v.pointer("/meta/fee").and_then(Value::as_u64).unwrap_or(0),
        cu: v.pointer("/meta/computeUnitsConsumed").and_then(Value::as_u64).unwrap_or(0),
        logs: v.pointer("/meta/logMessages").and_then(Value::as_array).map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        keys,
        ixs,
        inner,
    })
}

#[derive(Clone, Debug)]
pub struct AccountData {
    pub lamports: u64,
    pub owner: Pubkey,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// What a sent transaction came to.
#[derive(Clone, Debug)]
pub struct Sent {
    pub info: TxInfo,
    /// Serialized size of the signed transaction in bytes.
    pub size: usize,
    /// It was a version-0 transaction with address lookup tables.
    pub v0: bool,
}

pub struct Cluster {
    pub rpc: Rpc,
    /// Cluster time minus wall-clock time at the last refresh (seconds).
    clock_offset: std::cell::Cell<Option<i64>>,
}

pub fn wall_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| i64::try_from(d.as_secs()).unwrap_or(0)).unwrap_or(0)
}

impl Cluster {
    pub fn new(url: &str) -> Cluster {
        Cluster { rpc: Rpc::new(url), clock_offset: std::cell::Cell::new(None) }
    }

    pub fn genesis_hash(&self) -> Result<String, RpcError> {
        Ok(self.rpc.call("getGenesisHash", json!([]))?.as_str().unwrap_or_default().to_string())
    }

    pub fn balance(&self, key: &Pubkey) -> Result<u64, RpcError> {
        Ok(self.rpc.call("getBalance", json!([key.to_string(), {"commitment": "confirmed"}]))?.get("value").and_then(Value::as_u64).unwrap_or(0))
    }

    pub fn min_rent(&self, len: usize) -> Result<u64, RpcError> {
        Ok(self.rpc.call("getMinimumBalanceForRentExemption", json!([len]))?.as_u64().unwrap_or(0))
    }

    pub fn airdrop(&self, key: &Pubkey, lamports: u64) -> Result<String, RpcError> {
        Ok(self.rpc.call("requestAirdrop", json!([key.to_string(), lamports]))?.as_str().unwrap_or_default().to_string())
    }

    pub fn account(&self, key: &Pubkey) -> Result<Option<AccountData>, RpcError> {
        let v = self.rpc.call("getAccountInfo", json!([key.to_string(), {"encoding": "base64", "commitment": "confirmed"}]))?;
        let Some(val) = v.get("value").filter(|x| !x.is_null()) else { return Ok(None) };
        let data_b64 = val.pointer("/data/0").and_then(Value::as_str).unwrap_or_default();
        let data = base64::engine::general_purpose::STANDARD.decode(data_b64).map_err(|e| RpcError { method: "getAccountInfo".into(), message: format!("bad base64: {e}"), data: None })?;
        let owner = val.get("owner").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or_default();
        Ok(Some(AccountData { lamports: val.get("lamports").and_then(Value::as_u64).unwrap_or(0), owner, data, executable: val.get("executable").and_then(Value::as_bool).unwrap_or(false) }))
    }

    /// The cluster's own time: the block time of the newest confirmed slot, remembered as an offset from the wall clock.
    pub fn refresh_clock(&self) -> Result<i64, RpcError> {
        let slot = self.rpc.call("getSlot", json!([{"commitment": "confirmed"}]))?.as_u64().unwrap_or(0);
        for back in 0..8u64 {
            if let Ok(t) = self.rpc.call("getBlockTime", json!([slot.saturating_sub(back)])) {
                if let Some(t) = t.as_i64() {
                    self.clock_offset.set(Some(t - wall_now()));
                    return Ok(t);
                }
            }
        }
        Err(RpcError { method: "getBlockTime".into(), message: "no block time for the newest slots".into(), data: None })
    }

    /// The cluster time now (an estimate from the last refresh; refreshed when it is stale or unknown).
    pub fn now(&self) -> Result<i64, RpcError> {
        if self.clock_offset.get().is_none() {
            self.refresh_clock()?;
        }
        Ok(wall_now() + self.clock_offset.get().unwrap_or(0))
    }

    /// Waits until the cluster time is at least `t` (re-measuring the clock about every 20 s so a drifting clock cannot hang us).
    pub fn wait_until(&self, t: i64) -> Result<(), RpcError> {
        let mut last_refresh = Instant::now();
        loop {
            let now = self.now()?;
            if now >= t {
                return Ok(());
            }
            let gap = u64::try_from(t - now).unwrap_or(1);
            std::thread::sleep(Duration::from_millis((gap * 1000).clamp(200, 2_000)));
            if last_refresh.elapsed() > Duration::from_secs(20) {
                self.refresh_clock()?;
                last_refresh = Instant::now();
            }
        }
    }

    /// Waits until the MEASURED block time of the newest confirmed slot has reached `t` (two requests per look, no cached offset). The
    /// program refuses an event dated more than 2 s after the block time (I-3), and a validator under load can fall behind the wall
    /// clock, so a time-sensitive transaction is only sent when the cluster's own clock says so.
    pub fn wait_until_block_time(&self, t: i64) -> Result<(), RpcError> {
        loop {
            if self.refresh_clock()? >= t {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    }

    pub fn latest_blockhash(&self) -> Result<solana_hash::Hash, RpcError> {
        let v = self.rpc.call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))?;
        let s = v.pointer("/value/blockhash").and_then(Value::as_str).unwrap_or_default();
        s.parse().map_err(|_| RpcError { method: "getLatestBlockhash".into(), message: "bad blockhash".into(), data: None })
    }

    pub fn transaction(&self, sig: &str) -> Result<Option<TxInfo>, RpcError> {
        let v = self.rpc.call("getTransaction", json!([sig, {"encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0}]))?;
        if v.is_null() {
            return Ok(None);
        }
        parse_transaction(sig, &v).map(Some).map_err(|e| RpcError { method: "getTransaction".into(), message: e, data: None })
    }

    /// Every signature that touches `addr`, oldest first: (signature, slot). Pages of 1,000 from the newest back.
    pub fn signatures_for(&self, addr: &Pubkey) -> Result<Vec<(String, u64)>, RpcError> {
        let mut out: Vec<(String, u64)> = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let mut cfg = json!({"limit": 1000, "commitment": "confirmed"});
            if let Some(b) = &before {
                cfg["before"] = json!(b);
            }
            let v = self.rpc.call("getSignaturesForAddress", json!([addr.to_string(), cfg]))?;
            let arr = v.as_array().cloned().unwrap_or_default();
            if arr.is_empty() {
                break;
            }
            for e in &arr {
                out.push((e.get("signature").and_then(Value::as_str).unwrap_or_default().to_string(), e.get("slot").and_then(Value::as_u64).unwrap_or(0)));
            }
            before = out.last().map(|x| x.0.clone());
            if arr.len() < 1000 {
                break;
            }
        }
        out.reverse();
        Ok(out)
    }

    /// Signs, sends and waits for `confirmed`. `skip_preflight` lands a transaction the simulation would refuse (the attacks of S3 are
    /// executed by the real runtime, not by a simulation). A transaction that fails on chain returns `Ok` with `info.err` set.
    pub fn send(&self, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair], skip_preflight: bool) -> Result<Sent, RpcError> {
        self.send_with(ixs, payer, signers, skip_preflight, &[])
    }

    /// `send`, as a version-0 transaction that loads accounts from the given address lookup tables when there are any.
    pub fn send_with(&self, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair], skip_preflight: bool, tables: &[AddressLookupTableAccount]) -> Result<Sent, RpcError> {
        let bh = self.latest_blockhash()?;
        let sign_err = |e: String| RpcError { method: "sign".into(), message: e, data: None };
        let bytes = if tables.is_empty() {
            let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &bh);
            let mut tx = Transaction::new_unsigned(msg);
            let mut all: Vec<&Keypair> = vec![payer];
            for s in signers {
                if s.pubkey() != payer.pubkey() {
                    all.push(s);
                }
            }
            // sign only with the keys the message asks for
            let n = usize::from(tx.message.header.num_required_signatures);
            let required: Vec<Pubkey> = tx.message.account_keys[..n].to_vec();
            let sigs: Vec<&Keypair> = all.into_iter().filter(|k| required.contains(&k.pubkey())).collect();
            tx.try_partial_sign(&sigs, bh).map_err(|e| sign_err(e.to_string()))?;
            bincode::serialize(&tx).map_err(|e| sign_err(e.to_string()))?
        } else {
            let msg = v0::Message::try_compile(&payer.pubkey(), ixs, tables, bh).map_err(|e| sign_err(format!("compile v0: {e}")))?;
            let n = usize::from(msg.header.num_required_signatures);
            let required: Vec<Pubkey> = msg.account_keys[..n].to_vec();
            let mut all: Vec<&Keypair> = vec![payer];
            all.extend(signers.iter().filter(|s| s.pubkey() != payer.pubkey()));
            // the signatures in the order of the message's signer keys
            let ordered: Vec<&Keypair> = required.iter().filter_map(|k| all.iter().find(|s| s.pubkey() == *k).copied()).collect();
            let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &ordered).map_err(|e| sign_err(e.to_string()))?;
            bincode::serialize(&tx).map_err(|e| sign_err(e.to_string()))?
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let sig = self
            .rpc
            .call("sendTransaction", json!([b64, {"encoding": "base64", "skipPreflight": skip_preflight, "preflightCommitment": "confirmed", "maxRetries": 5}]))?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let info = self.confirm(&sig, Duration::from_secs(90), &bytes)?;
        Ok(Sent { info, size: bytes.len(), v0: !tables.is_empty() })
    }

    /// The finalized slot (a slot the `SlotHashes` sysvar certainly holds).
    pub fn finalized_slot(&self) -> Result<u64, RpcError> {
        Ok(self.rpc.call("getSlot", json!([{"commitment": "finalized"}]))?.as_u64().unwrap_or(0))
    }

    pub fn confirmed_slot(&self) -> Result<u64, RpcError> {
        Ok(self.rpc.call("getSlot", json!([{"commitment": "confirmed"}]))?.as_u64().unwrap_or(0))
    }

    fn confirm(&self, sig: &str, timeout: Duration, raw: &[u8]) -> Result<TxInfo, RpcError> {
        let start = Instant::now();
        let mut resent = 0;
        loop {
            let v = self.rpc.call("getSignatureStatuses", json!([[sig], {"searchTransactionHistory": true}]))?;
            if let Some(st) = v.pointer("/value/0").filter(|x| !x.is_null()) {
                let level = st.get("confirmationStatus").and_then(Value::as_str).unwrap_or("");
                if level == "confirmed" || level == "finalized" {
                    // the transaction record can take a moment to be served
                    for _ in 0..20 {
                        if let Some(t) = self.transaction(sig)? {
                            return Ok(t);
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    return Err(RpcError { method: "getTransaction".into(), message: "confirmed but not served".into(), data: None });
                }
            }
            if start.elapsed() > timeout {
                return Err(RpcError { method: "confirm".into(), message: format!("not confirmed within {}s", timeout.as_secs()), data: None });
            }
            // a dropped transaction is sent again (same signature: harmless if it landed)
            if start.elapsed() > Duration::from_secs(20 * (resent + 1)) {
                resent += 1;
                let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
                let _ = self.rpc.call("sendTransaction", json!([b64, {"encoding": "base64", "skipPreflight": true, "maxRetries": 3}]));
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devnet_urls_are_checked_and_mainnet_is_refused() {
        assert!(check_url(ClusterKind::Devnet, "https://api.devnet.solana.com").is_ok());
        assert!(check_url(ClusterKind::Devnet, "https://devnet.helius-rpc.com/?api-key=x").is_ok());
        assert!(check_url(ClusterKind::Devnet, "https://api.mainnet-beta.solana.com").is_err());
        assert!(check_url(ClusterKind::Devnet, "https://devnet.mainnet.example").is_err(), "mainnet anywhere in the URL refuses");
        assert!(check_url(ClusterKind::Devnet, "https://rpc.example.com").is_err());
        assert!(check_url(ClusterKind::Devnet, "http://127.0.0.1:8899").is_err());
    }

    #[test]
    fn the_local_selftest_talks_to_localhost_only() {
        assert!(check_url(ClusterKind::Local, "http://127.0.0.1:8899").is_ok());
        assert!(check_url(ClusterKind::Local, "http://localhost:8899").is_ok());
        assert!(check_url(ClusterKind::Local, "https://api.devnet.solana.com").is_err());
        assert!(check_url(ClusterKind::Local, "http://127.0.0.1.evil.example:8899").is_err());
    }

    #[test]
    fn genesis_must_match_and_devnet_pins_its_own_hash() {
        assert!(check_genesis(ClusterKind::Devnet, DEVNET_GENESIS, DEVNET_GENESIS).is_ok());
        assert!(check_genesis(ClusterKind::Devnet, "other", DEVNET_GENESIS).is_err());
        assert!(check_genesis(ClusterKind::Devnet, "other", "other").is_err(), "devnet cannot be told to expect another genesis");
        assert!(check_genesis(ClusterKind::Local, "abc", "abc").is_ok());
        assert!(check_genesis(ClusterKind::Local, DEVNET_GENESIS, "abc").is_err());
    }

    #[test]
    fn custom_errors_are_read_from_the_status() {
        let e = json!({"InstructionError": [1, {"Custom": 6109}]});
        assert_eq!(parse_custom_error(&e), Some((1, 6109)));
        assert_eq!(parse_custom_error(&json!({"InstructionError": [0, "InvalidAccountData"]})), None);
        assert_eq!(parse_custom_error(&Value::Null), None);
    }

    #[test]
    fn a_transaction_record_is_parsed_with_its_inner_instructions() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let data = bs58::encode([1u8, 2, 3]).into_string();
        let v = json!({
            "slot": 12, "blockTime": 1_700_000_000,
            "transaction": {"message": {"accountKeys": [a.to_string(), b.to_string()],
                "instructions": [{"programIdIndex": 1, "accounts": [0], "data": data}]}},
            "meta": {"err": null, "fee": 5000, "computeUnitsConsumed": 777, "logMessages": ["x"],
                "innerInstructions": [{"index": 0, "instructions": [{"programIdIndex": 1, "accounts": [0, 1], "data": data}]}]}
        });
        let t = parse_transaction("sig", &v).unwrap();
        assert!(t.ok());
        assert_eq!((t.slot, t.block_time, t.fee, t.cu), (12, Some(1_700_000_000), 5000, 777));
        assert_eq!(t.ixs[0].program, b);
        assert_eq!(t.ixs[0].data, vec![1, 2, 3]);
        assert_eq!(t.inner[0].0, 0);
        assert_eq!(t.inner[0].1[0].accounts, vec![0, 1]);
        assert!(parse_transaction("sig", &json!({"transaction": {"message": {}}})).is_err());
    }
}
