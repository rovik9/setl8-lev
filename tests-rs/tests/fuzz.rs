//! M12 item 6 (second half): a LiteSVM FUZZ LOOP against the real program (the localnet `.so`) and the real vault fixture.
//!
//! What one iteration does: pick one of the 25 instructions (the prompt says 26, the program has 25: see the fragment),
//! build a call (well-formed from the state of the world, a near miss, a call with permuted/substituted accounts and flags,
//! random instruction data, or pure random), send it, and assert:
//!   (1) no panic or abort of the program (`ProgramFailedToComplete`, `BorshIoError`, a `panicked` log line, an access
//!       violation): a named Anchor / Setl8Error code is the only acceptable way to fail;
//!   (2) a FAILING transaction leaves EVERY account of the SVM byte-identical (whole-world digest over the SVM's account
//!       map, lamports and owners included; executable accounts and sysvars excluded);
//!   (3) a SUCCEEDING transaction keeps the account-level invariants of `check_static` / `check_transition` (the I-1..I-12
//!       of DESIGN 4.4 that can be read from account bytes, plus the wallet / config / tally / vault-link relations), the
//!       hash chain recomputed from the emitted `EventLog`s, and the authorisation oracle (a success needs the signers the
//!       DESIGN asks for, judged on the PRE-state config).
//!
//! Signer model: the fee payer signs; an account may only be flagged as a signer if the harness holds its key (payer,
//! the two test admins, traders, evaluators, attackers). A PDA is never flagged. The harness runs with signature
//! verification OFF (the vault harness needs it), so this discipline IS the signature check.
//!
//! Knobs: `FUZZ_SEED` (default below), `FUZZ_ITERS` (default 50,000), `FUZZ_THREADS` (default min(cores, 10)),
//! `FUZZ_EPOCH` (iterations per world, default 700). Every worker uses seed `FUZZ_SEED + worker * 0x9E37_79B9`, printed.
mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use anchor_lang::solana_program::{
    instruction::{error::InstructionError, AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token;
use common::journal::*;
use common::lifecycle::ResetAccts;
use common::pass::{claim_pda, event_authority, FinalizeAccts};
use common::vault::*;
use common::*;
use journal_types::{genesis_hash, next_hash, JournalBatch, JournalEvent, NativeSha256, PriceProof};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use setl8_lev_trading::{
    accounts, constants as c, instruction,
    instructions::{CreateMarketArgs, UpdateMarketArgs},
    state::{Market, RuleSet, SectorConfig, TradingAccount, WalletProfile},
};
use setl8_shared_interfaces as si;
use sha2::{Digest, Sha256};
use solana_account::ReadableAccount;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;

const DEFAULT_SEED: u64 = 0x5E7_1EB0_F022_2026;
const DEFAULT_ITERS: u64 = 50_000;
const DEFAULT_EPOCH: u64 = 250;

// ====================================================================================================== rng + env

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn idx(&mut self, len: usize) -> usize {
        usize::try_from(self.below(len as u64)).unwrap()
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.idx(v.len())]
    }
    /// Inclusive range.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + i64::try_from(self.below(u64::try_from(hi - lo + 1).unwrap())).unwrap()
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 24) as u8).collect()
    }
    /// Random bytes of a random length below `max`.
    fn rbytes(&mut self, max: u64) -> Vec<u8> {
        let n = usize::try_from(self.below(max)).unwrap();
        self.bytes(n)
    }
    fn weighted(&mut self, w: &[u64]) -> usize {
        let total: u64 = w.iter().sum();
        let mut x = self.below(total);
        for (i, v) in w.iter().enumerate() {
            if x < *v {
                return i;
            }
            x -= v;
        }
        w.len() - 1
    }
    fn hash32(&mut self) -> [u8; 32] {
        let mut h = [0u8; 32];
        h.copy_from_slice(&self.bytes(32));
        if h == [0u8; 32] {
            h[0] = 1;
        }
        h
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(s) if !s.trim().is_empty() => {
            let s = s.trim();
            let parsed = if let Some(hex) = s.strip_prefix("0x") { u64::from_str_radix(hex, 16) } else { s.parse::<u64>() };
            parsed.unwrap_or_else(|_| panic!("{name} must be a number, got {s:?}"))
        }
        _ => default,
    }
}

// ====================================================================================================== the 25 instructions

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
enum Op {
    InitConfig,
    SetEvaluator,
    SetLimits,
    SetPaused,
    CreateRuleset,
    CreateMarket,
    UpdateMarket,
    BanWallet,
    UnbanWallet,
    OpenAccount,
    ResetAccount,
    PostJournal,
    RequestAdvance,
    CancelPass,
    ConfirmPass,
    RejectPass,
    VetoPass,
    FinalizePass,
    ReportFail,
    HoldAccount,
    ReleaseHold,
    FailForViolation,
    TouchActivity,
    ReleaseAccount,
    CloseAccount,
}

use Op::*;

const OPS: [Op; 25] = [
    InitConfig, SetEvaluator, SetLimits, SetPaused, CreateRuleset, CreateMarket, UpdateMarket, BanWallet, UnbanWallet, OpenAccount, ResetAccount,
    PostJournal, RequestAdvance, CancelPass, ConfirmPass, RejectPass, VetoPass, FinalizePass, ReportFail, HoldAccount, ReleaseHold, FailForViolation,
    TouchActivity, ReleaseAccount, CloseAccount,
];

const NAMES: [&str; 25] = [
    "init_config", "set_evaluator", "set_limits", "set_paused", "create_ruleset", "create_market", "update_market", "ban_wallet", "unban_wallet",
    "open_account", "reset_account", "post_journal", "request_advance", "cancel_pass", "confirm_pass", "reject_pass", "veto_pass", "finalize_pass",
    "report_fail", "hold_account", "release_hold", "fail_for_violation", "touch_activity", "release_account", "close_account",
];

impl Op {
    fn name(self) -> &'static str {
        NAMES[self as usize]
    }
    fn disc(self) -> [u8; 8] {
        let d: &[u8] = match self {
            InitConfig => instruction::InitConfig::DISCRIMINATOR,
            SetEvaluator => instruction::SetEvaluator::DISCRIMINATOR,
            SetLimits => instruction::SetLimits::DISCRIMINATOR,
            SetPaused => instruction::SetPaused::DISCRIMINATOR,
            CreateRuleset => instruction::CreateRuleset::DISCRIMINATOR,
            CreateMarket => instruction::CreateMarket::DISCRIMINATOR,
            UpdateMarket => instruction::UpdateMarket::DISCRIMINATOR,
            BanWallet => instruction::BanWallet::DISCRIMINATOR,
            UnbanWallet => instruction::UnbanWallet::DISCRIMINATOR,
            OpenAccount => instruction::OpenAccount::DISCRIMINATOR,
            ResetAccount => instruction::ResetAccount::DISCRIMINATOR,
            PostJournal => instruction::PostJournal::DISCRIMINATOR,
            RequestAdvance => instruction::RequestAdvance::DISCRIMINATOR,
            CancelPass => instruction::CancelPass::DISCRIMINATOR,
            ConfirmPass => instruction::ConfirmPass::DISCRIMINATOR,
            RejectPass => instruction::RejectPass::DISCRIMINATOR,
            VetoPass => instruction::VetoPass::DISCRIMINATOR,
            FinalizePass => instruction::FinalizePass::DISCRIMINATOR,
            ReportFail => instruction::ReportFail::DISCRIMINATOR,
            HoldAccount => instruction::HoldAccount::DISCRIMINATOR,
            ReleaseHold => instruction::ReleaseHold::DISCRIMINATOR,
            FailForViolation => instruction::FailForViolation::DISCRIMINATOR,
            TouchActivity => instruction::TouchActivity::DISCRIMINATOR,
            ReleaseAccount => instruction::ReleaseAccount::DISCRIMINATOR,
            CloseAccount => instruction::CloseAccount::DISCRIMINATOR,
        };
        d.try_into().unwrap()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Good,
    Near,
    Permuted,
    Garbage,
    Random,
}
const KINDS: [Kind; 5] = [Kind::Good, Kind::Near, Kind::Permuted, Kind::Garbage, Kind::Random];
const KIND_NAMES: [&str; 5] = ["good", "near-miss", "permuted", "garbage-data", "random"];

#[derive(Clone)]
struct Call {
    /// The instruction the harness aimed at (`exec` replaces it with the one the discriminator really selects).
    op: Option<Op>,
    /// The one external action (the vault's permissionless `mark_abandoned`).
    ext: bool,
    ix: Instruction,
    kind: Kind,
}

/// The instruction the data really selects (the mutation layer may have replaced the discriminator).
fn eff_op(call: &Call) -> Option<Op> {
    call.ix.data.get(..8).and_then(|d| OPS.iter().copied().find(|o| o.disc() == d))
}

fn op_label(call: &Call) -> &'static str {
    if call.ext {
        "ext"
    } else {
        call.op.map_or("unknown-discriminator", Op::name)
    }
}

// ====================================================================================================== digest + classification

/// Per-account sha256(key, lamports, owner, data) of every non-executable, non-sysvar account of the SVM with lamports > 0.
fn world_map(svm: &litesvm::LiteSVM, payer: &Pubkey) -> HashMap<Pubkey, [u8; 32]> {
    let sysvar_owner = anchor_lang::solana_program::sysvar::ID;
    let mut m = HashMap::new();
    for (k, a) in svm.accounts_db().inner.iter() {
        if a.executable() || *a.owner() == sysvar_owner || a.lamports() == 0 {
            continue;
        }
        let mut h = Sha256::new();
        h.update(k.as_ref());
        // a failed transaction still pays its fee (LiteSVM, like a cluster): the fee payer's lamports are checked separately
        h.update(if k == payer { 0 } else { a.lamports() }.to_le_bytes());
        h.update(a.owner().as_ref());
        h.update((a.data().len() as u64).to_le_bytes());
        h.update(a.data());
        m.insert(*k, h.finalize().into());
    }
    m
}

fn map_diff(a: &HashMap<Pubkey, [u8; 32]>, b: &HashMap<Pubkey, [u8; 32]>) -> Vec<Pubkey> {
    let mut d: Vec<Pubkey> = a.iter().filter(|(k, v)| b.get(*k) != Some(*v)).map(|(k, _)| *k).collect();
    d.extend(b.keys().filter(|k| !a.contains_key(*k)).copied());
    d
}

/// Name of the error: the Anchor error name from the logs for custom codes, else the InstructionError variant.
fn err_label(f: &FailedTransactionMetadata) -> String {
    match &f.err {
        TransactionError::InstructionError(_, InstructionError::Custom(code)) => {
            for l in f.meta.logs.iter().rev() {
                if let Some(p) = l.find("Error Code: ") {
                    let name: String = l[p + 12..].chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
            format!("Custom({code})")
        }
        TransactionError::InstructionError(_, e) => format!("{e:?}").split('(').next().unwrap_or("?").to_string(),
        other => format!("tx:{other:?}"),
    }
}

/// A panic / abort class failure and the program it happened in (the program stack is read from the invoke/success lines).
fn panic_of(f: &FailedTransactionMetadata, our: &Pubkey) -> Option<(bool, String)> {
    let mut stack: Vec<String> = Vec::new();
    let ours = our.to_string();
    for l in &f.meta.logs {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() >= 4 && w[0] == "Program" && w[2] == "invoke" {
            stack.push(w[1].to_string());
        } else if w.len() >= 3 && w[0] == "Program" && (w[2] == "success" || w[2] == "failed:" || w[2] == "failed") && !l.starts_with("Program log") {
            stack.pop();
        }
        let bad = l.contains("panicked") || l.contains("failed to complete") || l.contains("Access violation") || l.contains("stack overflow") || l.contains("Stack overflow");
        if bad {
            let who = stack.last().cloned().unwrap_or_default();
            return Some((who == ours || who.is_empty(), format!("{l}  [in program {who}]")));
        }
    }
    if let TransactionError::InstructionError(_, e) = &f.err {
        if matches!(
            e,
            InstructionError::ProgramFailedToComplete
                | InstructionError::BorshIoError(_)
                | InstructionError::ProgramFailedToCompile
                | InstructionError::ArithmeticOverflow
                | InstructionError::ProgramEnvironmentSetupFailure
        ) {
            return Some((true, format!("{e:?}")));
        }
    }
    None
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ====================================================================================================== snapshots + invariants

type Viol = (&'static str, String);

#[derive(Clone)]
struct Snap {
    clock: i64,
    cfg: SectorConfig,
    tally: (u64, u64),
    accts: BTreeMap<Pubkey, TradingAccount>,
    /// raw bytes of the trading accounts (for exact frame checks)
    wallets: BTreeMap<Pubkey, WalletProfile>,
    wallets_raw: BTreeMap<Pubkey, Vec<u8>>,
    /// per trading-account key: (payout_count, status, account_size) of the vault record
    vts: BTreeMap<Pubkey, (u64, u8, u64)>,
    rulesets: BTreeMap<u16, RuleSet>,
    /// `max_payout_count` the vault holds for this product (the program assumes 4: one payout per phase 2 to 5)
    vault_max_payout: u64,
}

fn fmt_acct(a: &TradingAccount) -> String {
    format!(
        "{{cid {} S {} b {} q {} latch {} flat {} tight {} r {} g {} l {} lock {} st {} ph {} pp {} held {} seq {} lbt {} created {} req_at {} prop_at {} nri {} veto {}}}",
        a.challenge_id, a.size_s, a.b, a.q_tally, a.latch, a.flat, a.tight_on, a.r, a.g_peak, a.l_level, a.lock_until, a.status, a.phase, a.pass_pending,
        a.held, a.seq, a.last_batch_time, a.created_at, a.pass_requested_at, a.pass_proposed_at, a.next_request_id, a.vetoes
    )
}

/// Config relations that hold after `init_config` (`vault` is the vault program the config must carry).
fn check_config(cfg: &SectorConfig, clock: i64, vault: &Pubkey) -> Vec<Viol> {
    let mut v: Vec<Viol> = vec![];
    macro_rules! chk {
        ($id:expr, $cond:expr, $($arg:tt)*) => {
            if !($cond) {
                v.push(($id, format!($($arg)*)));
            }
        };
    }
    chk!("C-1", (c::MIN_PASS_DELAY_SECS..=c::MAX_PASS_DELAY_SECS).contains(&cfg.pass_delay_secs), "pass_delay_secs {} outside 1..14 days", cfg.pass_delay_secs);
    chk!("C-2", cfg.max_payout_per_window > 0 && cfg.window_secs > 0, "cap {} / window {} must be non-zero", cfg.max_payout_per_window, cfg.window_secs);
    chk!("C-3", cfg.paused <= 1, "paused byte {}", cfg.paused);
    chk!("C-4", cfg.market_count <= c::MAX_MARKETS, "market_count {} above 128", cfg.market_count);
    chk!("C-5", cfg.admin_a == c::SL8_ADMIN && cfg.admin_b == c::ROV_ADMIN, "admin keys changed");
    chk!("C-6", cfg.vault_program == *vault, "vault_program changed to {}", cfg.vault_program);
    chk!("C-7", cfg.reserved == [0u8; 64], "reserved bytes not zero");
    chk!("C-8", cfg.window_start >= 0 && cfg.window_start <= clock, "window_start {} outside 0..=clock {}", cfg.window_start, clock);
    v
}

/// Invariants of one snapshot. Accounts of tainted traders (garbage injected by the fuzzer itself) are skipped.
fn check_static(s: &Snap, tainted: &BTreeSet<Pubkey>) -> Vec<Viol> {
    let mut v = check_config(&s.cfg, s.clock, &VAULT_ID);
    macro_rules! chk {
        ($id:expr, $cond:expr, $($arg:tt)*) => {
            if !($cond) {
                v.push(($id, format!($($arg)*)));
            }
        };
    }
    chk!("C-4", s.cfg.current_ruleset >= 1 && s.rulesets.contains_key(&s.cfg.current_ruleset), "current_ruleset {} has no rule set", s.cfg.current_ruleset);
    for (k, a) in &s.accts {
        if tainted.contains(&a.trader) {
            continue;
        }
        let ctx = format!("{k} {}", fmt_acct(a));
        let sz = i128::from(a.size_s);
        let (b, q) = (i128::from(a.b), i128::from(a.q_tally));
        chk!("T-1", a.latch <= 1 && a.flat <= 1 && a.tight_on <= 1 && a.held <= 1, "flag byte out of range {ctx}");
        chk!("T-2", a.status <= 3 && (1..=5).contains(&a.phase) && a.pass_pending <= 3, "status/phase/pass_pending out of range {ctx}");
        chk!("T-3a", a.pass_pending <= 2 || (a.pass_pending == 3 && a.status == c::STATUS_FAILED), "pass_pending 3 on a non-Failed account {ctx}");
        chk!("T-3b", !(matches!(a.pass_pending, 1 | 2) && a.status != c::STATUS_ACTIVE), "pending pass on a terminal account {ctx}");
        chk!("T-3c", !(a.held == 1 && a.status != c::STATUS_ACTIVE), "held on a terminal account {ctx}");
        let tier_ok = s.rulesets.get(&a.ruleset_version).is_some_and(|rs| rs.tier_size.iter().take(usize::from(rs.tier_count)).any(|t| *t == a.size_s));
        chk!("T-4", a.size_s > 0 && a.size_s <= i64::MAX as u64 && tier_ok, "size_s is not a tier of rule set {} {ctx}", a.ruleset_version);
        chk!("T-4b", a.ruleset_version >= 1 && a.ruleset_version <= s.cfg.current_ruleset, "ruleset_version {} outside 1..=current {ctx}", a.ruleset_version);
        // I-7: b >= 1.04 S latches
        chk!("T-5", a.latch == 1 || b * 100 < sz * 104, "b >= 1.04 S but latch is 0 {ctx}");
        // I-8: the quick tally never rests above 0.028 S on a live (non-Failed) account
        chk!("T-6a", q >= 0, "negative q_tally {ctx}");
        chk!("T-6b", a.status == c::STATUS_FAILED || q * 1000 <= sz * 28, "q_tally above 0.028 S on a non-Failed account {ctx}");
        // I-11 / MustReportFail: an Active account that is flat rests above its static floor
        let floor = sz * if a.latch == 1 { 96 } else { 92 } / 100;
        chk!("T-7", !(a.status == c::STATUS_ACTIVE && a.flat == 1) || b > floor, "Active + flat + b {} at or below the static floor {floor} {ctx}", a.b);
        // tight mode consistency
        if a.tight_on == 0 {
            chk!("T-8a", a.r == 0 && a.g_peak == 0 && a.l_level == 0, "tight off but r/g_peak/l_level not zero {ctx}");
        } else if a.status == c::STATUS_ACTIVE {
            let slack = sz / 100;
            let l_start = i128::from(a.l_level) - (i128::from(a.g_peak) - slack).max(0);
            let r = i128::from(a.r);
            chk!("T-8b", a.flat == 0, "tight on while flat {ctx}");
            chk!("T-8c", a.r > 0 && a.g_peak >= 0, "tight on with r <= 0 or g_peak < 0 {ctx}");
            chk!(
                "T-8d",
                l_start == r.min(sz * 96 / 100) || l_start == r.min(sz),
                "derived l_start {l_start} is neither min(0.96 S, r) nor min(1.00 S, r) {ctx}"
            );
        }
        chk!("T-9", a.lock_until >= 0, "negative lock_until {ctx}");
        chk!(
            "T-10",
            a.created_at <= s.clock
                && a.last_batch_time >= a.created_at
                && a.last_batch_time <= s.clock + 2
                && a.last_activity_pushed >= a.created_at
                && a.last_activity_pushed <= s.clock,
            "timestamps out of order (clock {}) {ctx}",
            s.clock
        );
        if a.seq == 0 {
            chk!("T-11a", a.journal_hash == genesis_hash(&NativeSha256, &k.to_bytes()), "seq 0 but journal_hash is not the genesis hash {ctx}");
            chk!(
                "T-11b",
                a.trades_closed == 0 && a.last_batch_time == a.created_at && b == sz && a.q_tally == 0 && a.latch == 0 && a.tight_on == 0 && a.lock_until == 0 && a.flat == 1,
                "seq 0 but the account is not in its fresh state {ctx}"
            );
        } else {
            chk!("T-11c", a.journal_hash != genesis_hash(&NativeSha256, &k.to_bytes()), "seq > 0 but journal_hash is still the genesis hash {ctx}");
        }
        if a.status == c::STATUS_ACTIVE && a.pass_pending == 1 {
            chk!("T-12a", a.pass_requested_at >= a.created_at && a.pass_requested_at <= s.clock, "bad pass_requested_at {ctx}");
        }
        if a.status == c::STATUS_ACTIVE && a.pass_pending == 2 {
            chk!("T-12b", a.pass_requested_at <= a.pass_proposed_at && a.pass_proposed_at >= a.created_at && a.pass_proposed_at <= s.clock, "bad pass_proposed_at {ctx}");
        }
        if a.status != c::STATUS_ACTIVE {
            chk!("T-12c", a.pass_proposed_at >= a.created_at && a.pass_proposed_at <= s.clock, "bad terminal_at {ctx}");
            let reset_ok = a.pass_requested_at == 0 || (a.status == c::STATUS_FAILED && a.pass_requested_at >= a.pass_proposed_at);
            chk!("T-12d", reset_ok, "bad reset_at / pass_requested_at on a terminal account {ctx}");
        }
        chk!("T-13", (1..=5).contains(&a.next_request_id), "next_request_id {} outside 1..=5 {ctx}", a.next_request_id);
        match s.vts.get(k) {
            None => v.push(("V-0", format!("no vault record for {ctx}"))),
            Some((count, status, vsize)) => {
                chk!("V-1", count + 1 == a.next_request_id, "next_request_id != vault payout_count + 1 (vault count {count}) {ctx}");
                chk!("V-2", *vsize == a.size_s, "vault account_size {vsize} != size_s {ctx}");
                // vault TraderStatus: Active 0, Graduated 1, Failed 2, Abandoned 3
                let want: &[u8] = match a.status {
                    c::STATUS_ACTIVE => &[0, 3],
                    c::STATUS_FAILED => &[2],
                    c::STATUS_ABANDONED => &[3],
                    _ => &[1],
                };
                // the vault auto-graduates at ITS max_payout_count, the program graduates after phase 5 whatever the vault holds
                // (LIMITED, see test limited_vault_max_payout_count_is_not_read_by_the_program): compared only while it is 4
                chk!("V-3", s.vault_max_payout != 4 || want.contains(status), "local status {} but vault status {status} {ctx}", a.status);
            }
        }
    }
    for (tr, w) in &s.wallets {
        if tainted.contains(tr) {
            continue;
        }
        let mine: Vec<&TradingAccount> = s.accts.values().filter(|a| a.trader == *tr).collect();
        let active = u8::try_from(mine.iter().filter(|a| a.status == c::STATUS_ACTIVE).count()).unwrap_or(255);
        let held = u8::try_from(mine.iter().filter(|a| a.held == 1).count()).unwrap_or(255);
        chk!("W-1", w.banned <= 1 && w.trader == *tr && w.pad == [0u8; 4], "wallet of {tr}: banned {} / trader field / pad", w.banned);
        chk!("W-2", w.active_count == active && w.active_count <= c::MAX_ACTIVE_ACCOUNTS, "wallet of {tr}: active_count {} but {active} Active accounts", w.active_count);
        chk!("W-3", w.held_count == held, "wallet of {tr}: held_count {} but {held} held accounts", w.held_count);
        let max_id = mine.iter().map(|a| a.challenge_id + 1).max().unwrap_or(0);
        chk!("W-4", w.next_challenge_id >= max_id, "wallet of {tr}: next_challenge_id {} <= an existing challenge id", w.next_challenge_id);
    }
    v
}

fn allowed_ops_for(field: &str) -> &'static [Op] {
    match field {
        "b" | "q_tally" | "latch" | "tight_on" | "r" | "g_peak" | "l_level" | "flat" | "lock_until" | "trades_closed" | "seq" | "journal_hash" | "last_batch_time" => {
            &[PostJournal, ReportFail]
        }
        "last_activity_pushed" => &[RequestAdvance, TouchActivity, ReleaseAccount],
        "next_request_id" => &[FinalizePass],
        "status" => &[ReportFail, FailForViolation, FinalizePass, RequestAdvance, TouchActivity, ReleaseAccount],
        "phase" => &[ConfirmPass, FinalizePass],
        "pass_pending" | "pass_requested_at" | "pass_proposed_at" => {
            &[RequestAdvance, CancelPass, ConfirmPass, RejectPass, VetoPass, FinalizePass, ReportFail, FailForViolation, TouchActivity, ReleaseAccount, ResetAccount]
        }
        "vetoes" => &[VetoPass],
        "held" => &[HoldAccount, ReleaseHold, FailForViolation, ReportFail, FinalizePass, RequestAdvance, TouchActivity, ReleaseAccount],
        _ => &[],
    }
}

fn diff_fields(p: &TradingAccount, n: &TradingAccount) -> Vec<&'static str> {
    let mut d = vec![];
    macro_rules! f {
        ($($name:ident),*) => { $( if p.$name != n.$name { d.push(stringify!($name)); } )* };
    }
    f!(
        trader, challenge_id, size_s, created_at, b, q_tally, lock_until, last_batch_time, last_activity_pushed, next_request_id, seq, journal_hash, trades_closed,
        ruleset_version, status, phase, latch, flat, pass_pending, vetoes, bump, tight_on, held, r, g_peak, l_level, pass_requested_at, pass_proposed_at
    );
    d
}

fn cfg_diff(p: &SectorConfig, n: &SectorConfig) -> Vec<&'static str> {
    let mut d = vec![];
    macro_rules! f {
        ($($name:ident),*) => { $( if p.$name != n.$name { d.push(stringify!($name)); } )* };
    }
    f!(admin_a, admin_b, evaluator, vault_program, sl8_wallet, current_ruleset, market_count, paused, bump, pass_delay_secs, max_payout_per_window, window_secs, window_start, window_total, reserved);
    d
}

/// Transition invariants: what a successful call `op` may have changed between two snapshots, and how.
fn check_transition(p: &Snap, n: &Snap, op: Option<Op>, tainted: &BTreeSet<Pubkey>, new_batch_events: Option<usize>) -> Vec<Viol> {
    check_transition_ops(p, n, op.as_slice(), tainted, new_batch_events)
}

/// The same for a transaction of several instructions (`ops` in order; empty = an outside event): every allowance is the union.
fn check_transition_ops(p: &Snap, n: &Snap, ops: &[Op], tainted: &BTreeSet<Pubkey>, new_batch_events: Option<usize>) -> Vec<Viol> {
    let mut v: Vec<Viol> = vec![];
    let opname = if ops.is_empty() { "external".to_string() } else { ops.iter().map(|o| o.name()).collect::<Vec<_>>().join("+") };
    let cnt = |o: Op| ops.iter().filter(|x| **x == o).count();
    let multi = ops.len() > 1;
    macro_rules! chk {
        ($id:expr, $cond:expr, $($arg:tt)*) => {
            if !($cond) {
                v.push(($id, format!($($arg)*)));
            }
        };
    }
    let is = |o: Op| cnt(o) > 0;
    // ---- config
    for f in cfg_diff(&p.cfg, &n.cfg) {
        let ok = match f {
            "evaluator" => is(SetEvaluator),
            "paused" => is(SetPaused),
            "pass_delay_secs" | "max_payout_per_window" | "window_secs" => is(SetLimits),
            "current_ruleset" => n.cfg.current_ruleset > p.cfg.current_ruleset && usize::from(n.cfg.current_ruleset - p.cfg.current_ruleset) <= cnt(CreateRuleset),
            "market_count" => n.cfg.market_count > p.cfg.market_count && usize::from(n.cfg.market_count - p.cfg.market_count) <= cnt(CreateMarket),
            "window_start" | "window_total" => is(FinalizePass) && n.cfg.window_start >= p.cfg.window_start,
            _ => false,
        };
        chk!("X-C1", ok, "{opname} changed config field {f}");
    }
    if n.cfg.window_start == p.cfg.window_start && n.cfg.window_total != p.cfg.window_total {
        chk!("X-C2", n.cfg.window_total > p.cfg.window_total, "window_total fell inside the same window ({} -> {})", p.cfg.window_total, n.cfg.window_total);
    }
    // ---- payout tally
    let (dc, dt) = (n.tally.0.wrapping_sub(p.tally.0), n.tally.1.wrapping_sub(p.tally.1));
    chk!("X-T1", n.tally.0 >= p.tally.0 && n.tally.1 >= p.tally.1, "payout tally fell {:?} -> {:?}", p.tally, n.tally);
    chk!("X-T2", (dc == 0 && dt == 0) || is(FinalizePass), "{opname} changed the payout tally");
    // ---- accounts
    let mut vanished = 0;
    let mut finalized: Vec<u64> = vec![];
    for (k, pa) in &p.accts {
        if tainted.contains(&pa.trader) {
            vanished += usize::from(!n.accts.contains_key(k));
            continue;
        }
        let Some(na) = n.accts.get(k) else {
            vanished += 1;
            chk!("X-A1", is(CloseAccount), "{opname} made account {k} disappear");
            chk!("X-A2", pa.status != c::STATUS_ACTIVE && n.clock >= pa.pass_proposed_at + c::TERMINAL_RETENTION_SECS, "account {k} closed while {} / before 7 days", pa.status);
            continue;
        };
        let changed = diff_fields(pa, na);
        let phase_up = na.phase == pa.phase + 1;
        for f in &changed {
            let mut ok = allowed_ops_for(f).iter().any(|o| is(*o));
            if phase_up && (is(ConfirmPass) || is(FinalizePass)) && matches!(*f, "b" | "q_tally" | "latch" | "tight_on" | "r" | "g_peak" | "l_level" | "flat") {
                ok = true;
            }
            chk!("X-A3", ok, "{opname} changed field {f} of {k}: {} -> {}", fmt_acct(pa), fmt_acct(na));
        }
        chk!("X-A4", na.seq >= pa.seq && na.last_batch_time >= pa.last_batch_time && na.trades_closed >= pa.trades_closed, "journal counters moved backwards on {k}");
        chk!("X-A5", (na.seq == pa.seq) == (na.journal_hash == pa.journal_hash), "seq and journal_hash do not move together on {k}");
        chk!("X-A6", na.phase == pa.phase || usize::from(na.phase.saturating_sub(pa.phase)) <= cnt(ConfirmPass) + cnt(FinalizePass) && na.phase > pa.phase, "phase jumped {} -> {} on {k}", pa.phase, na.phase);
        chk!("X-A7", pa.status == c::STATUS_ACTIVE || na.status == pa.status, "terminal status {} changed to {} on {k}", pa.status, na.status);
        chk!("X-A8", na.lock_until >= pa.lock_until, "lock_until fell on {k}");
        if na.phase == pa.phase {
            chk!("X-A9", na.q_tally >= pa.q_tally && na.latch >= pa.latch, "q_tally or latch fell inside one phase on {k}");
        }
        chk!("X-A10", na.vetoes == pa.vetoes || (is(VetoPass) && na.vetoes >= pa.vetoes && usize::from(na.vetoes - pa.vetoes) <= cnt(VetoPass)), "vetoes {} -> {} by {opname}", pa.vetoes, na.vetoes);
        chk!("X-A11", na.next_request_id == pa.next_request_id || (na.next_request_id > pa.next_request_id && usize::try_from(na.next_request_id - pa.next_request_id).is_ok_and(|d| d <= cnt(FinalizePass))), "next_request_id {} -> {} by {opname}", pa.next_request_id, na.next_request_id);
        if na.next_request_id != pa.next_request_id {
            finalized.push(pa.size_s);
        }
        if pa.held != na.held {
            chk!("X-A12", multi || na.held == 1 || !is(HoldAccount), "hold_account cleared a hold on {k}");
            chk!("X-A13", na.held != 1 || is(HoldAccount), "{opname} set held on {k}");
        }
        if pa.pass_pending != na.pass_pending && !multi {
            let (from, to) = (pa.pass_pending, na.pass_pending);
            let ok = match to {
                1 => is(RequestAdvance) && from == 0,
                2 => is(ConfirmPass) && from == 1,
                3 => is(FailForViolation),
                _ => match from {
                    1 => is(CancelPass) || is(RejectPass) || is(VetoPass) || is(ConfirmPass) || is(ReportFail) || is(TouchActivity) || is(ReleaseAccount) || is(RequestAdvance),
                    2 => is(VetoPass) || is(FinalizePass) || is(TouchActivity) || is(ReleaseAccount),
                    _ => false,
                },
            };
            chk!("X-A14", ok, "{opname} moved pass_pending {from} -> {to} on {k}");
        }
        if !multi && (is(ReportFail) || is(PostJournal)) {
            chk!("X-A15", pa.status == c::STATUS_ACTIVE || changed.is_empty(), "{opname} touched a terminal account {k}");
        }
    }
    chk!("X-A16", if multi { vanished <= cnt(CloseAccount) } else { vanished == cnt(CloseAccount) }, "{opname} closed {vanished} accounts");
    let new_keys: Vec<&Pubkey> = n.accts.keys().filter(|k| !p.accts.contains_key(k)).collect();
    if tainted.is_empty() {
        let made = cnt(OpenAccount) + cnt(ResetAccount);
        chk!("X-A17", if multi { new_keys.len() <= made } else { new_keys.len() == made }, "{opname} created {} accounts", new_keys.len());
    }
    for k in new_keys {
        let a = &n.accts[k];
        let (want, bump) = Pubkey::find_program_address(&[c::ACCT_SEED, a.trader.as_ref(), &a.challenge_id.to_le_bytes()], &PROGRAM_ID);
        chk!("X-A18", want == *k && bump == a.bump, "new account {k} is not the canonical PDA of ({}, {})", a.trader, a.challenge_id);
    }
    // ---- who changed the seq: only post_journal / report_fail, exactly one account, by the number of emitted events
    let seq_moved: Vec<(&Pubkey, u64)> = n.accts.iter().filter_map(|(k, na)| p.accts.get(k).filter(|pa| pa.seq != na.seq).map(|pa| (k, na.seq.wrapping_sub(pa.seq)))).collect();
    if let Some(events) = new_batch_events {
        chk!("X-J1", seq_moved.len() == 1, "{opname} succeeded but {} accounts changed seq", seq_moved.len());
        if let Some((_, d)) = seq_moved.first() {
            chk!("X-J2", usize::try_from(*d).ok() == Some(events), "seq advanced by {d} but {events} EventLogs were emitted");
        }
    }
    // ---- tally amount
    if dc > 0 && tainted.is_empty() && multi {
        chk!("X-T3", usize::try_from(dc).is_ok_and(|d| d <= cnt(FinalizePass)) && dc == finalized.len() as u64, "tally moved by ({dc}, {dt}) but finalized accounts of sizes {finalized:?}");
    } else if dc > 0 && tainted.is_empty() {
        chk!("X-T3", dc == 1 && finalized.len() == 1 && i128::from(dt) == i128::from(finalized[0]) * 8 / 100, "tally moved by ({dc}, {dt}) but finalized accounts of sizes {finalized:?}");
    } else if dc == 0 {
        chk!("X-T4", dt == 0, "tally total moved without the count");
    }
    // ---- wallets
    for (tr, pw) in &p.wallets {
        if tainted.contains(tr) {
            continue;
        }
        let Some(nw) = n.wallets.get(tr) else {
            chk!("X-W0", false, "wallet of {tr} disappeared");
            continue;
        };
        if p.wallets_raw[tr] != n.wallets_raw[tr] {
            let may = [OpenAccount, ResetAccount, BanWallet, UnbanWallet, ReportFail, FinalizePass, FailForViolation, HoldAccount, ReleaseHold, RequestAdvance, TouchActivity, ReleaseAccount];
            chk!("X-W1", may.iter().any(|o| is(*o)), "{opname} changed the wallet of {tr}");
        }
        chk!("X-W2", nw.next_challenge_id == pw.next_challenge_id || (nw.next_challenge_id > pw.next_challenge_id && usize::try_from(nw.next_challenge_id - pw.next_challenge_id).is_ok_and(|d| d <= cnt(OpenAccount) + cnt(ResetAccount))), "next_challenge_id {} -> {} by {opname}", pw.next_challenge_id, nw.next_challenge_id);
        chk!("X-W3", nw.banned == pw.banned || is(BanWallet) || is(UnbanWallet) || is(FailForViolation), "{opname} changed banned of {tr}");
        chk!("X-W4", nw.trader == pw.trader && nw.bump == pw.bump, "wallet trader/bump changed for {tr}");
    }
    // ---- vault records
    for (k, (pc, ps, _)) in &p.vts {
        if let Some((nc, ns, _)) = n.vts.get(k) {
            chk!("X-V1", *nc == *pc || (*nc > *pc && usize::try_from(*nc - *pc).is_ok_and(|d| d <= cnt(FinalizePass))), "vault payout_count {pc} -> {nc} by {opname} on {k}");
            let status_ok = ns == ps || (*ps == 0 && matches!(*ns, 1..=3));
            chk!("X-V2", status_ok, "vault status {ps} -> {ns} on {k}");
        }
    }
    v
}

/// Authorisation oracle on the PRE-state: the signers the DESIGN demands for `op` were present in the transaction.
fn check_auth(op: Op, ix: &Instruction, pre: &Snap) -> Vec<Viol> {
    let signers: BTreeSet<Pubkey> = ix.accounts.iter().filter(|m| m.is_signer).map(|m| m.pubkey).collect();
    let cfg = &pre.cfg;
    let mut v: Vec<Viol> = vec![];
    let mut need = |id: &'static str, ok: bool, what: String| {
        if !ok {
            v.push((id, format!("{} succeeded without {what}", op.name())));
        }
    };
    match op {
        PostJournal | ReportFail | ConfirmPass | RejectPass | HoldAccount | TouchActivity => {
            need("A-EV", signers.contains(&cfg.evaluator), format!("a signature of config.evaluator {}", cfg.evaluator))
        }
        SetPaused if ix.data.get(8) == Some(&1) => {
            need("A-ANY", signers.contains(&cfg.admin_a) || signers.contains(&cfg.admin_b), "an admin signing (pause)".to_string())
        }
        SetEvaluator | SetLimits | SetPaused | CreateRuleset | BanWallet | UnbanWallet | VetoPass | ReleaseHold | FailForViolation => {
            need("A-BOTH", signers.contains(&cfg.admin_a) && signers.contains(&cfg.admin_b), "both admins signing".to_string())
        }
        CreateMarket | UpdateMarket => need("A-ANY", signers.contains(&cfg.admin_a) || signers.contains(&cfg.admin_b), "an admin signing".to_string()),
        InitConfig => need("A-INIT", signers.contains(&c::SL8_ADMIN) && signers.contains(&c::ROV_ADMIN), "both constant admins signing".to_string()),
        RequestAdvance | CancelPass | ReleaseAccount | CloseAccount => {
            // the trader of the trading account in account slot 1 must be a signer
            let trader = ix.accounts.get(1).and_then(|m| pre.accts.get(&m.pubkey)).map(|a| a.trader);
            need("A-TR", trader.is_some_and(|t| signers.contains(&t)), "the trader of the trading account (slot 1) signing".to_string());
        }
        OpenAccount | ResetAccount => {
            let t = ix.accounts.first().map(|m| m.pubkey).unwrap_or_default();
            need("A-TR", signers.contains(&t), "the trader (account 0) signing".to_string());
        }
        FinalizePass => {}
    }
    v
}


/// Success preconditions of DESIGN 10 / 6, judged on the PRE-state: a call that succeeds on an account that did not meet them is a
/// hole (a permuted call that reached the wrong account, a check that is missing). The target of a call is the account(s) it changed.
fn check_pre(op: Op, ix: &Instruction, pre: &Snap, post: &Snap, tainted: &BTreeSet<Pubkey>) -> Vec<Viol> {
    let mut v: Vec<Viol> = vec![];
    let id: &'static str = match op {
        RequestAdvance => "P-request",
        CancelPass => "P-cancel",
        ConfirmPass => "P-confirm",
        RejectPass => "P-reject",
        VetoPass => "P-veto",
        FinalizePass => "P-finalize",
        HoldAccount => "P-hold",
        ReleaseHold => "P-release_hold",
        FailForViolation => "P-violation",
        TouchActivity => "P-touch",
        ReleaseAccount => "P-release_account",
        ResetAccount => "P-reset",
        OpenAccount => "P-open",
        PostJournal => "P-post",
        ReportFail => "P-report_fail",
        BanWallet => "P-ban",
        CreateRuleset => "P-create_ruleset",
        CreateMarket => "P-create_market",
        _ => return v,
    };
    let cfg = &pre.cfg;
    let mut need = |ok: bool, what: String| {
        if !ok {
            v.push((id, what));
        }
    };
    for (k, pa) in &pre.accts {
        if tainted.contains(&pa.trader) {
            continue;
        }
        let Some(na) = post.accts.get(k) else { continue };
        if pa.pass_pending == na.pass_pending
            && pa.status == na.status
            && pa.held == na.held
            && pa.phase == na.phase
            && pa.seq == na.seq
            && pa.next_request_id == na.next_request_id
            && pa.pass_requested_at == na.pass_requested_at
            && pa.vetoes == na.vetoes
            && pa.last_activity_pushed == na.last_activity_pushed
        {
            continue; // not the target
        }
        let now = post.clock;
        let size = i128::from(pa.size_s);
        let target = i128::from(pa.b) * 100 >= size * 108;
        let who = format!("{k} {}", fmt_acct(pa));
        match op {
            RequestAdvance => need(cfg.paused == 0 && pa.status == 0 && pa.held == 0 && pa.pass_pending == 0 && pa.tight_on == 0 && pa.flat == 1 && target, format!("request_advance succeeded on an account that is not ready (or while paused {}): {who}", cfg.paused)),
            ConfirmPass => need(pa.status == 0 && pa.pass_pending == 1 && pa.held == 0 && pa.flat == 1 && target, format!("confirm_pass succeeded on an account that is not in state 1, held, not flat or below the target: {who}")),
            RejectPass => need(pa.pass_pending == 1, format!("reject_pass succeeded outside state 1: {who}")),
            CancelPass => need(pa.pass_pending == 1 && now >= pa.pass_requested_at + c::CANCEL_PASS_AFTER_SECS, format!("cancel_pass succeeded outside state 1 or before 1 hour: {who}")),
            VetoPass => need(matches!(pa.pass_pending, 1 | 2), format!("veto_pass succeeded without a pending pass: {who}")),
            FinalizePass => {
                let mut ok = cfg.paused == 0 && pa.status == 0 && pa.pass_pending == 2 && pa.held == 0 && now >= pa.pass_proposed_at + i64::from(cfg.pass_delay_secs) && target;
                if na.next_request_id != pa.next_request_id {
                    // a payout: the daily window of 10.6 (roll first, then total + amount <= cap)
                    let amount = u64::try_from(size * 8 / 100).unwrap_or(u64::MAX);
                    let total = if now >= cfg.window_start + i64::from(cfg.window_secs) { 0 } else { cfg.window_total };
                    ok = ok && total.saturating_add(amount) <= cfg.max_payout_per_window;
                }
                need(ok, format!("finalize_pass succeeded outside its preconditions (paused {}, state 2, held 0, delay {} s, target, daily cap): {who}", cfg.paused, cfg.pass_delay_secs))
            }
            HoldAccount => need(pa.status == 0 && pa.held == 0, format!("hold_account succeeded on a terminal or already held account: {who}")),
            ReleaseHold => need(pa.held == 1, format!("release_hold succeeded on an account that was not held: {who}")),
            FailForViolation => need(pa.status == 0 && pa.held == 1, format!("fail_for_violation succeeded on an account that was not held and Active: {who}")),
            TouchActivity | ReleaseAccount => need(pa.status == 0, format!("{} succeeded on a terminal account: {who}", op.name())),
            ResetAccount => need(cfg.paused == 0 && pa.status == 1 && pa.pass_pending != 3 && pa.pass_requested_at == 0, format!("reset_account succeeded on an account that was not a resettable Failed one (paused {}): {who}", cfg.paused)),
            PostJournal | ReportFail => need(pa.status == 0 && pa.pass_pending != 2, format!("{} succeeded on a terminal account or while a pass is confirmed: {who}", op.name())),
            _ => {}
        }
    }
    // checks that need the wallet or the config rather than an account
    match op {
        OpenAccount | ResetAccount => {
            let t = ix.accounts.first().map(|m| m.pubkey).unwrap_or_default();
            if let Some(w) = pre.wallets.get(&t) {
                if !tainted.contains(&t) {
                    need(w.banned == 0 && w.active_count < c::MAX_ACTIVE_ACCOUNTS && cfg.paused == 0, format!("{} succeeded for a banned / full wallet or while paused: banned {} active {} paused {}", op.name(), w.banned, w.active_count, cfg.paused));
                }
            } else {
                need(op == OpenAccount && cfg.paused == 0, format!("{} succeeded without a wallet profile or while paused", op.name()));
            }
        }
        BanWallet => {
            for (tr, pw) in &pre.wallets {
                if post.wallets.get(tr).is_some_and(|nw| nw.banned == 1) && pw.banned == 0 && !tainted.contains(tr) {
                    need(pw.held_count == 0, format!("ban_wallet succeeded on a wallet with {} held accounts", pw.held_count));
                }
            }
        }
        CreateRuleset => need(post.cfg.current_ruleset == cfg.current_ruleset + 1, "create_ruleset did not advance the version by one".to_string()),
        CreateMarket => need(post.cfg.market_count == cfg.market_count + 1, "create_market did not advance market_count by one".to_string()),
        _ => {}
    }
    v
}

// ====================================================================================================== stats

#[derive(Default)]
struct Stats {
    iters: u64,
    tries: [u64; 25],
    ok: [u64; 25],
    kind_tries: [u64; 5],
    kind_ok: [u64; 5],
    ext_tries: u64,
    ext_ok: u64,
    multi_tries: u64,
    multi_ok: u64,
    hist: BTreeMap<String, u64>,
    invariant_runs: u64,
    chain_checks: u64,
    budget_errs: u64,
    vault_panics: Vec<String>,
    violations: Vec<String>,
    viol_count: BTreeMap<String, u64>,
    known_jrn1: u64,
    ev_hist: BTreeMap<&'static str, u64>,
    cov: BTreeMap<&'static str, u64>,
    states: BTreeMap<u32, u64>,
    cu_max: [u64; 25],
    sends: u64,
    send_nanos: u128,
    other_nanos: u128,
}

impl Stats {
    fn merge(&mut self, o: Stats) {
        self.iters += o.iters;
        for i in 0..25 {
            self.tries[i] += o.tries[i];
            self.ok[i] += o.ok[i];
        }
        for i in 0..5 {
            self.kind_tries[i] += o.kind_tries[i];
            self.kind_ok[i] += o.kind_ok[i];
        }
        self.multi_tries += o.multi_tries;
        self.multi_ok += o.multi_ok;
        self.ext_tries += o.ext_tries;
        self.ext_ok += o.ext_ok;
        for (k, n) in o.hist {
            *self.hist.entry(k).or_default() += n;
        }
        self.invariant_runs += o.invariant_runs;
        self.chain_checks += o.chain_checks;
        self.budget_errs += o.budget_errs;
        self.vault_panics.extend(o.vault_panics);
        self.violations.extend(o.violations);
        for (k, n) in o.viol_count {
            *self.viol_count.entry(k).or_default() += n;
        }
        self.known_jrn1 += o.known_jrn1;
        for (k, n) in o.ev_hist {
            *self.ev_hist.entry(k).or_default() += n;
        }
        for (k, n) in o.cov {
            *self.cov.entry(k).or_default() += n;
        }
        for (k, n) in o.states {
            *self.states.entry(k).or_default() += n;
        }
        for i in 0..25 {
            self.cu_max[i] = self.cu_max[i].max(o.cu_max[i]);
        }
        self.sends += o.sends;
        self.send_nanos += o.send_nanos;
        self.other_nanos += o.other_nanos;
    }
}

// ====================================================================================================== the world

/// A trading account the fuzzer may aim a call at (it may be closed: `a` is None then).
#[derive(Clone)]
struct Tgt {
    ti: usize,
    trader: Pubkey,
    cid: u64,
    acct: Pubkey,
    vts: Pubkey,
    wallet: Pubkey,
    a: Option<TradingAccount>,
}

struct Pool {
    all: Vec<Pubkey>,
    by_cat: Vec<Vec<Pubkey>>,
    cat_of: HashMap<Pubkey, usize>,
}

const CAT_ACCT: usize = 0;
const CAT_WALLET: usize = 1;
const CAT_VTS: usize = 2;
const CAT_CLAIM: usize = 3;
const CAT_MARKET: usize = 4;
const CAT_RULESET: usize = 5;
const CAT_TOKEN: usize = 6;
const CAT_SIGNER: usize = 7;
const CAT_PROGRAM: usize = 8;
const CAT_OTHER: usize = 9;

struct World {
    j: JEnv,
    traders: Vec<Keypair>,
    evs: Vec<Keypair>,
    attackers: Vec<Keypair>,
    held: BTreeSet<Pubkey>,
    clock: i64,
    snap: Snap,
    dmap: HashMap<Pubkey, [u8; 32]>,
    tainted: BTreeSet<Pubkey>,
    tg: Vec<Tgt>,
    pool: Pool,
    pda_cache: HashMap<(Pubkey, u64), (Pubkey, Pubkey)>,
    wallet_cache: HashMap<Pubkey, Pubkey>,
    next_market_feed: u8,
    /// Makes the next `gen_post_batch` use this template (the scripted pass cycles).
    force_tpl: Option<usize>,
}

fn spread_of(rs: &RuleSet) -> u32 {
    u32::from(rs.spread_e6[usize::from(MARKET_EUR)])
}

impl World {
    /// The template world: rule set 1 with spreads, markets 0 to 4, four traders with a few accounts, one ghost trader.
    fn build(rng: &mut Rng) -> World {
        let mut j = JEnv::with_trader(8, Some(0));
        let mut traders = vec![dup(&j.trader)];
        for s in 1..=4u32 {
            traders.push(j.v.new_trader_seeded(s));
        }
        for (ti, tier) in [(1usize, 0u8), (1, 3), (2, 6), (2, 1), (3, 4)] {
            let k = dup(&traders[ti]);
            assert!(j.v.open(&k, tier).is_ok(), "template open_account");
        }
        j.set_limits(c::MIN_PASS_DELAY_SECS, 100_000_000_000, 86_400);
        let evs = vec![dup(&j.v.evaluator), Keypair::new_from_array([0xE2; 32])];
        let attackers = vec![Keypair::new_from_array([0xA1; 32]), Keypair::new_from_array([0xA2; 32])];
        let mut held: BTreeSet<Pubkey> = BTreeSet::new();
        for k in traders.iter().chain(evs.iter()).chain(attackers.iter()).chain([&j.v.e.sl8, &j.v.e.rov, &j.v.e.payer]) {
            held.insert(k.pubkey());
        }
        let _ = rng;
        let clock = j.v.e.svm.get_sysvar::<anchor_lang::solana_program::clock::Clock>().unix_timestamp;
        let placeholder = Snap {
            clock,
            cfg: j.v.e.config(),
            tally: (0, 0),
            accts: BTreeMap::new(),
            wallets: BTreeMap::new(),
            wallets_raw: BTreeMap::new(),
            vts: BTreeMap::new(),
            rulesets: BTreeMap::new(),
            vault_max_payout: 4,
        };
        let mut w = World {
            j,
            traders,
            evs,
            attackers,
            held,
            clock,
            snap: placeholder,
            dmap: HashMap::new(),
            tainted: BTreeSet::new(),
            tg: vec![],
            pool: Pool { all: vec![], by_cat: vec![], cat_of: HashMap::new() },
            pda_cache: HashMap::new(),
            wallet_cache: HashMap::new(),
            next_market_feed: 100,
            force_tpl: None,
        };
        w.refresh();
        w
    }

    fn fork(&self) -> World {
        let v = &self.j.v;
        let e2 = Env { svm: v.e.svm.clone(), sl8: dup(&v.e.sl8), rov: dup(&v.e.rov), payer: dup(&v.e.payer), last_tx: TxShape::default() };
        let v2 = VaultEnv {
            e: e2,
            usdc: v.usdc,
            usdt: v.usdt,
            vault_state: v.vault_state,
            usdc_pool: v.usdc_pool,
            usdt_pool: v.usdt_pool,
            vault_sl8_usdc: v.vault_sl8_usdc,
            fee_wallet: v.fee_wallet,
            fee_usdc: v.fee_usdc,
            ruleset: v.ruleset.clone(),
            evaluator: dup(&v.evaluator),
            tracked: v.tracked.clone(),
            traders: v.traders.clone(),
        };
        let j2 = JEnv { v: v2, trader: dup(&self.j.trader), acct: self.j.acct, t0: self.j.t0 };
        World {
            j: j2,
            traders: self.traders.iter().map(dup).collect(),
            evs: self.evs.iter().map(dup).collect(),
            attackers: self.attackers.iter().map(dup).collect(),
            held: self.held.clone(),
            clock: self.clock,
            snap: self.snap.clone(),
            dmap: self.dmap.clone(),
            tainted: BTreeSet::new(),
            tg: self.tg.clone(),
            pool: Pool { all: self.pool.all.clone(), by_cat: self.pool.by_cat.clone(), cat_of: self.pool.cat_of.clone() },
            pda_cache: self.pda_cache.clone(),
            wallet_cache: self.wallet_cache.clone(),
            next_market_feed: self.next_market_feed,
            force_tpl: None,
        }
    }

    fn cache_pdas(&mut self, trader: &Pubkey, cid: u64) -> (Pubkey, Pubkey) {
        *self.pda_cache.entry((*trader, cid)).or_insert_with(|| (acct_pda(trader, cid), vault_trader_state_pda(trader, cid)))
    }

    /// Reads every account of the world into a `Snap`. `Err` if the config or the tally cannot be parsed.
    fn take_snap(&mut self) -> Result<Snap, String> {
        let svm = &self.j.v.e.svm;
        let cfg_acc = svm.get_account(&config_pda()).ok_or("config account missing")?;
        let cfg = SectorConfig::try_deserialize(&mut cfg_acc.data.as_slice()).map_err(|e| format!("config unreadable: {e}"))?;
        let tally_acc = svm.get_account(&tally_pda()).ok_or("tally account missing")?;
        let t = si::PayoutTally::parse(&tally_acc.data).map_err(|e| format!("tally unreadable: {e:?}"))?;
        let mut rulesets = BTreeMap::new();
        for ver in 1..=cfg.current_ruleset {
            if let Some(acc) = svm.get_account(&ruleset_pda(ver)) {
                if let Ok(rs) = RuleSet::try_deserialize(&mut acc.data.as_slice()) {
                    rulesets.insert(ver, rs);
                }
            }
        }
        let mut s = Snap {
            clock: self.clock,
            cfg,
            tally: (t.requested_count, t.requested_total),
            accts: BTreeMap::new(),
            wallets: BTreeMap::new(),
            wallets_raw: BTreeMap::new(),
            vts: BTreeMap::new(),
            rulesets,
            vault_max_payout: self.j.v.registry().max_payout_count,
        };
        let pks: Vec<Pubkey> = self.traders.iter().map(|k| k.pubkey()).collect();
        let mut tg = vec![];
        for (ti, tr) in pks.iter().enumerate() {
            let wpda = *self.wallet_cache.entry(*tr).or_insert_with(|| wallet_pda(tr));
            let Some(wacc) = self.j.v.e.svm.get_account(&wpda) else { continue };
            if wacc.owner != PROGRAM_ID {
                continue;
            }
            let Ok(w) = WalletProfile::try_deserialize(&mut wacc.data.as_slice()) else { continue };
            for cid in 0..w.next_challenge_id.min(64) {
                let (akey, vkey) = self.cache_pdas(tr, cid);
                let svm = &self.j.v.e.svm;
                let a = svm.get_account(&akey).filter(|x| x.owner == PROGRAM_ID && x.data.len() == 207).and_then(|x| TradingAccount::try_deserialize(&mut x.data.as_slice()).ok());
                if let Some(a) = &a {
                    s.accts.insert(akey, a.clone());
                    if let Some(vt) = self.j.v.vault_trader_state(tr, cid) {
                        s.vts.insert(akey, (vt.payout_count, vt.status, vt.account_size));
                    }
                }
                tg.push(Tgt { ti, trader: *tr, cid, acct: akey, vts: vkey, wallet: wpda, a });
            }
            s.wallets_raw.insert(*tr, wacc.data.clone());
            s.wallets.insert(*tr, w);
        }
        self.tg = tg;
        Ok(s)
    }

    /// Re-reads the snapshot, the digest and the account pools after an out-of-band change.
    fn refresh(&mut self) {
        self.snap = self.take_snap().expect("world snapshot");
        self.dmap = world_map(&self.j.v.e.svm, &self.j.v.e.payer.pubkey());
        self.rebuild_pool();
    }

    fn rebuild_pool(&mut self) {
        let mut by_cat: Vec<Vec<Pubkey>> = vec![vec![]; 10];
        let mut cat_of = HashMap::new();
        let mut add = |cat: usize, k: Pubkey, by_cat: &mut Vec<Vec<Pubkey>>| {
            if let std::collections::hash_map::Entry::Vacant(e) = cat_of.entry(k) {
                e.insert(cat);
                by_cat[cat].push(k);
            }
        };
        for t in &self.tg {
            add(CAT_ACCT, t.acct, &mut by_cat);
            add(CAT_WALLET, t.wallet, &mut by_cat);
            add(CAT_VTS, t.vts, &mut by_cat);
            for rid in 1..=3u64 {
                add(CAT_CLAIM, claim_pda(&t.vts, rid), &mut by_cat);
            }
        }
        for tr in &self.traders {
            let tk = tr.pubkey();
            let w = *self.wallet_cache.entry(tk).or_insert_with(|| wallet_pda(&tk));
            add(CAT_WALLET, w, &mut by_cat);
            // ghost / not yet opened: the next account and record of the trader
            let (a0, v0) = (acct_pda(&tk, 0), vault_trader_state_pda(&tk, 0));
            add(CAT_ACCT, a0, &mut by_cat);
            add(CAT_VTS, v0, &mut by_cat);
            if let Some(ta) = self.j.v.traders.get(&tk) {
                add(CAT_TOKEN, *ta, &mut by_cat);
            }
        }
        for m in 0..=(self.snap.cfg.market_count.min(8)) {
            add(CAT_MARKET, market_pda(m), &mut by_cat);
        }
        for r in 1..=3u16 {
            add(CAT_RULESET, ruleset_pda(r), &mut by_cat);
        }
        for k in [self.j.v.usdc_pool, self.j.v.usdt_pool, self.j.v.vault_sl8_usdc, self.j.v.fee_usdc, self.j.v.usdc, self.j.v.usdt] {
            add(CAT_TOKEN, k, &mut by_cat);
        }
        // in a fixed order (the `held` set is ordered by key, which is random for the payer and the evaluator): replays must match
        let signers: Vec<Pubkey> = self
            .traders
            .iter()
            .chain(self.evs.iter())
            .chain(self.attackers.iter())
            .chain([&self.j.v.e.sl8, &self.j.v.e.rov, &self.j.v.e.payer])
            .map(|k| k.pubkey())
            .collect();
        for k in signers {
            add(CAT_SIGNER, k, &mut by_cat);
        }
        for k in [
            system_program::ID,
            spl_token::ID,
            VAULT_ID,
            PROGRAM_ID,
            anchor_lang::solana_program::sysvar::clock::ID,
            anchor_lang::solana_program::sysvar::rent::ID,
            Pubkey::default(),
            anchor_lang::solana_program::sysvar::ID,
        ] {
            add(CAT_PROGRAM, k, &mut by_cat);
        }
        for k in [
            config_pda(),
            tally_pda(),
            registry_pda(),
            self.j.v.vault_state,
            sector_authority_pda(),
            event_authority(),
            VAULT_SL8_ADMIN,
            VAULT_ROV_ADMIN,
            self.j.v.fee_wallet,
            Pubkey::new_from_array([0x77; 32]),
            Pubkey::new_from_array([0x78; 32]),
        ] {
            add(CAT_OTHER, k, &mut by_cat);
        }
        let all: Vec<Pubkey> = by_cat.iter().flatten().copied().collect();
        self.pool = Pool { all, by_cat, cat_of };
    }

    fn set_clock_to(&mut self, t: i64) {
        if t > self.clock {
            self.clock = t;
            self.j.v.e.set_clock(t);
        }
    }

    fn advance(&mut self, secs: i64) {
        let t = self.clock + secs;
        self.set_clock_to(t);
    }

    fn cur_eval(&self) -> Pubkey {
        let e = self.snap.cfg.evaluator;
        if self.held.contains(&e) {
            e
        } else {
            self.evs[0].pubkey()
        }
    }

    fn payer(&self) -> Pubkey {
        self.j.v.e.payer.pubkey()
    }

    fn pick(&self, rng: &mut Rng, pred: impl Fn(&Tgt) -> bool) -> Option<Tgt> {
        let c: Vec<&Tgt> = self.tg.iter().filter(|t| pred(t)).collect();
        if c.is_empty() {
            None
        } else {
            Some(c[rng.idx(c.len())].clone())
        }
    }

    fn pick_any(&self, rng: &mut Rng, pred: impl Fn(&Tgt) -> bool) -> Tgt {
        let any_target = if rng.chance(90) { self.pick(rng, |t| t.a.is_some()) } else { self.pick(rng, |_| true) };
        self.pick(rng, &pred).or(any_target).unwrap_or_else(|| {
            let tr = self.traders[0].pubkey();
            Tgt { ti: 0, trader: tr, cid: 0, acct: acct_pda(&tr, 0), vts: vault_trader_state_pda(&tr, 0), wallet: wallet_pda(&tr), a: None }
        })
    }

    /// An instruction that moves some account along its life (confirm a requested pass, finalize a confirmed one, request a
    /// ready one, release a hold, reset a failed account, close a terminal one), chosen by what the world holds right now.
    fn progress_op(&self, rng: &mut Rng) -> Option<Op> {
        let (mut p1, mut p2, mut ready, mut held, mut failed, mut term, mut active) = (0, 0, 0, 0, 0, 0, 0);
        for a in self.tg.iter().filter_map(|t| t.a.as_ref()) {
            if a.status == 0 {
                active += 1;
                match a.pass_pending {
                    1 => p1 += 1,
                    2 => p2 += 1,
                    _ => {
                        if a.held == 0 && a.tight_on == 0 && a.flat == 1 && i128::from(a.b) * 100 >= i128::from(a.size_s) * 108 {
                            ready += 1;
                        }
                    }
                }
                held += usize::from(a.held == 1);
            } else {
                term += 1;
                failed += usize::from(a.status == c::STATUS_FAILED && a.pass_requested_at == 0 && a.pass_pending != 3);
            }
        }
        let mut cands: Vec<(Op, u64)> = vec![];
        if p1 > 0 {
            cands.extend([(ConfirmPass, 6), (RejectPass, 1), (CancelPass, 1), (VetoPass, 1)]);
        }
        if p2 > 0 {
            cands.extend([(FinalizePass, 6), (VetoPass, 1)]);
        }
        if ready > 0 {
            cands.push((RequestAdvance, 5));
        }
        if held > 0 {
            cands.extend([(ReleaseHold, 3), (FailForViolation, 1)]);
        }
        if failed > 0 {
            cands.push((ResetAccount, 3));
        }
        if term > 0 {
            cands.push((CloseAccount, 1));
        }
        if active > 0 {
            cands.extend([(TouchActivity, 1), (ReleaseAccount, 1)]);
            if rng.chance(25) {
                cands.push((HoldAccount, 1));
            }
        }
        if cands.is_empty() {
            return None;
        }
        let w: Vec<u64> = cands.iter().map(|c| c.1).collect();
        Some(cands[rng.weighted(&w)].0)
    }

    fn admins(&self) -> Admins {
        Admins::both(&self.j.v.e)
    }
}

// ---------------------------------------------------------------------------------------------------- instruction builders

fn finalize_ix(w: &World, t: &Tgt, payer: Pubkey) -> Instruction {
    let rid = t.a.as_ref().map_or(1, |a| a.next_request_id);
    FinalizeAccts {
        payer,
        trading_account: t.acct,
        config: config_pda(),
        payout_tally: tally_pda(),
        wallet: t.wallet,
        sector_authority: sector_authority_pda(),
        vault_program: VAULT_ID,
        product_registry: registry_pda(),
        vault_trader_state: t.vts,
        vault_state: w.j.v.vault_state,
        payout_claim: claim_pda(&t.vts, rid),
        system_program: system_program::ID,
    }
    .to_ix()
}

fn reset_ix(w: &World, t: &Tgt, prev_cid: u64) -> Instruction {
    let next_id = w.snap.wallets.get(&t.trader).map_or(0, |x| x.next_challenge_id);
    let new_acct = acct_pda(&t.trader, next_id);
    ResetAccts {
        trader: t.trader,
        payer: t.trader,
        config: config_pda(),
        ruleset: ruleset_pda(w.snap.cfg.current_ruleset.max(1)),
        wallet: t.wallet,
        prev_trading_account: t.acct,
        trading_account: new_acct,
        sector_authority: sector_authority_pda(),
        vault_program: VAULT_ID,
        product_registry: registry_pda(),
        vault_trader_state: vault_trader_state_pda(&t.trader, next_id),
        prev_vault_trader_state: t.vts,
        vault_state: w.j.v.vault_state,
        trader_token_account: w.j.v.trader_token(&t.trader),
        mint: w.j.v.usdc,
        pool_token_account: w.j.v.usdc_pool,
        vault_sl8_token_account: w.j.v.vault_sl8_usdc,
        fee_token_account: w.j.v.fee_usdc,
        token_program: spl_token::ID,
        system_program: system_program::ID,
        trader_signs: true,
        payer_signs: true,
    }
    .to_ix(prev_cid)
}

/// The admin accounts are `UncheckedAccount`s (no signer flag from the generated metas): flag the two admin keys.
fn sign_admins(ad: Admins, mut ix: Instruction) -> Instruction {
    for m in ix.accounts.iter_mut() {
        if m.pubkey == ad.a || m.pubkey == ad.b {
            m.is_signer = true;
        }
    }
    ix
}

fn meta_ix(accts: Vec<AccountMeta>, data: Vec<u8>) -> Instruction {
    Instruction { program_id: PROGRAM_ID, accounts: accts, data }
}

impl World {
    fn ix_request(&self, t: &Tgt) -> Instruction {
        meta_ix(
            accounts::RequestAdvance {
                trader: t.trader,
                trading_account: t.acct,
                wallet: t.wallet,
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: t.vts,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::RequestAdvance {}.data(),
        )
    }
    fn ix_cancel(&self, t: &Tgt) -> Instruction {
        meta_ix(accounts::CancelPass { trader: t.trader, trading_account: t.acct, event_authority: event_authority_pda(), program: PROGRAM_ID }.to_account_metas(None), instruction::CancelPass {}.data())
    }
    fn ix_confirm(&self, t: &Tgt) -> Instruction {
        meta_ix(
            accounts::ConfirmPass { evaluator: self.cur_eval(), trading_account: t.acct, config: config_pda(), event_authority: event_authority(), program: PROGRAM_ID }
                .to_account_metas(None),
            instruction::ConfirmPass {}.data(),
        )
    }
    fn ix_reject(&self, t: &Tgt, reason: u8) -> Instruction {
        meta_ix(
            accounts::RejectPass { evaluator: self.cur_eval(), trading_account: t.acct, config: config_pda(), event_authority: event_authority(), program: PROGRAM_ID }
                .to_account_metas(None),
            instruction::RejectPass { reason }.data(),
        )
    }
    fn ix_veto(&self, t: &Tgt) -> Instruction {
        let ad = self.admins();
        sign_admins(ad, meta_ix(
            accounts::VetoPass { config: config_pda(), admin_a: ad.a, admin_b: ad.b, trading_account: t.acct, event_authority: event_authority(), program: PROGRAM_ID }
                .to_account_metas(None),
            instruction::VetoPass {}.data(),
        ))
    }
    fn ix_hold(&self, t: &Tgt, hash: [u8; 32]) -> Instruction {
        meta_ix(
            accounts::HoldAccount {
                evaluator: self.cur_eval(),
                trading_account: t.acct,
                wallet: t.wallet,
                config: config_pda(),
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::HoldAccount { evidence_hash: hash }.data(),
        )
    }
    fn ix_release_hold(&self, t: &Tgt) -> Instruction {
        let ad = self.admins();
        sign_admins(ad, meta_ix(
            accounts::ReleaseHold {
                config: config_pda(),
                admin_a: ad.a,
                admin_b: ad.b,
                trading_account: t.acct,
                wallet: t.wallet,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::ReleaseHold {}.data(),
        ))
    }
    fn ix_violation(&self, t: &Tgt, hash: [u8; 32]) -> Instruction {
        let ad = self.admins();
        sign_admins(ad, meta_ix(
            accounts::FailForViolation {
                config: config_pda(),
                admin_a: ad.a,
                admin_b: ad.b,
                trading_account: t.acct,
                wallet: t.wallet,
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: t.vts,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::FailForViolation { evidence_hash: hash }.data(),
        ))
    }
    fn ix_touch(&self, t: &Tgt) -> Instruction {
        meta_ix(
            accounts::TouchActivity {
                evaluator: self.cur_eval(),
                trading_account: t.acct,
                wallet: t.wallet,
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: t.vts,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::TouchActivity {}.data(),
        )
    }
    fn ix_release_acct(&self, t: &Tgt) -> Instruction {
        meta_ix(
            accounts::ReleaseAccount {
                trader: t.trader,
                trading_account: t.acct,
                wallet: t.wallet,
                config: config_pda(),
                sector_authority: sector_authority_pda(),
                vault_program: VAULT_ID,
                product_registry: registry_pda(),
                vault_trader_state: t.vts,
                event_authority: event_authority(),
                program: PROGRAM_ID,
            }
            .to_account_metas(None),
            instruction::ReleaseAccount {}.data(),
        )
    }
    fn ix_close(&self, t: &Tgt) -> Instruction {
        meta_ix(accounts::CloseAccount { trader: t.trader, trading_account: t.acct }.to_account_metas(None), instruction::CloseAccount {}.data())
    }
    fn ix_post(&self, t: &Tgt, bytes: Vec<u8>, markets: &[u16]) -> Instruction {
        let rv = t.a.as_ref().map_or(1, |a| a.ruleset_version);
        let mut metas = accounts::PostJournal {
            evaluator: self.cur_eval(),
            trading_account: t.acct,
            config: config_pda(),
            ruleset: ruleset_pda(rv),
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(JEnv::market_metas(markets));
        meta_ix(metas, instruction::PostJournal { batch_bytes: bytes }.data())
    }
    fn ix_report_fail(&self, t: &Tgt, bytes: Vec<u8>, reason: u8, markets: &[u16]) -> Instruction {
        let rv = t.a.as_ref().map_or(1, |a| a.ruleset_version);
        let mut metas = accounts::ReportFail {
            evaluator: self.cur_eval(),
            trading_account: t.acct,
            config: config_pda(),
            ruleset: ruleset_pda(rv),
            wallet: t.wallet,
            sector_authority: sector_authority_pda(),
            vault_program: VAULT_ID,
            product_registry: registry_pda(),
            vault_trader_state: t.vts,
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        metas.extend(JEnv::market_metas(markets));
        meta_ix(metas, instruction::ReportFail { batch_bytes: bytes, reason }.data())
    }
}

// ---------------------------------------------------------------------------------------------------- journal batch generation

/// The evaluator-side picture of an account while a batch is being built (balance, quick tally, latch, tight mode).
#[derive(Clone)]
struct Sim {
    s: i64,
    b: i64,
    q: i64,
    latch: bool,
    tight: bool,
    g: i64,
    l: i64,
    lock: i64,
}

impl Sim {
    fn of(a: Option<&TradingAccount>) -> Sim {
        match a {
            Some(a) => Sim {
                s: i64::try_from(a.size_s).unwrap_or(100_000_000_000),
                b: a.b,
                q: a.q_tally,
                latch: a.latch == 1,
                tight: a.tight_on == 1,
                g: a.g_peak,
                l: a.l_level,
                lock: a.lock_until,
            },
            None => Sim { s: 100_000_000_000, b: 100_000_000_000, q: 0, latch: false, tight: false, g: 0, l: 0, lock: 0 },
        }
    }
    fn floor(&self) -> i64 {
        self.s / 100 * if self.latch { 96 } else { 92 }
    }
    fn limit(&self) -> i64 {
        self.s / 1000 * 28
    }
    fn l_start(&self) -> i64 {
        self.l - (self.g - self.s / 100).max(0)
    }
    fn apply(&mut self, ev: &JournalEvent) {
        match ev {
            JournalEvent::Close { credited, quick, pnl, .. } => {
                self.b = self.b.saturating_add(*credited);
                if *quick && *pnl > 0 {
                    self.q = self.q.saturating_add(*credited);
                }
                if i128::from(self.b) * 100 >= i128::from(self.s) * 104 {
                    self.latch = true;
                }
            }
            JournalEvent::Fee { amount, .. } => self.b -= i64::try_from(*amount).unwrap_or(0),
            _ => {}
        }
    }
}

fn ev_close(h: u32, t: i64, ts: i64, pnl: i64) -> Option<JournalEvent> {
    let exit = engine::bid_from_mid(MID, h).ok()?;
    let entry = u64::try_from(i128::from(exit) - i128::from(pnl)).ok()?;
    if entry == 0 {
        return None;
    }
    Some(close(MARKET_EUR, true, MEUR, entry, exit, t, ts, 0))
}

impl World {
    fn spread_for(&self, a: Option<&TradingAccount>) -> u32 {
        a.and_then(|a| self.snap.rulesets.get(&a.ruleset_version)).map_or(H_EUR, spread_of)
    }

    fn event_time(&self, rng: &mut Rng, a: Option<&TradingAccount>, noisy: bool) -> i64 {
        let lbt = a.map_or(self.clock, |a| a.last_batch_time);
        let mut t = lbt.max(self.clock - rng.below(3) as i64).min(self.clock + 2);
        if noisy && rng.chance(8) {
            t = match rng.below(3) {
                0 => lbt - 1 - rng.below(5) as i64,
                1 => self.clock + 3 + rng.below(10) as i64,
                _ => self.clock - 100 - rng.below(1000) as i64,
            };
        }
        t
    }

    /// A batch for `post_journal`: a template picked by weight, falling back to random trades when the template does not apply.
    fn gen_post_batch(&mut self, rng: &mut Rng, t: &Tgt, noisy: bool) -> (JournalBatch, Vec<u16>) {
        let a = t.a.clone();
        let h = self.spread_for(a.as_ref());
        let tm = self.event_time(rng, a.as_ref(), noisy);
        let mut sim = Sim::of(a.as_ref());
        let s = sim.s;
        let mut events: Vec<JournalEvent> = vec![];
        let mut extra_markets: Vec<u16> = vec![];
        let mut flat = rng.chance(50);
        let mut template = self.force_tpl.take().unwrap_or_else(|| rng.weighted(&[24, 10, 12, 10, 5, 8, 6, 2]));
        // template applicability
        if (matches!(template, 3..=5) && !sim.tight) || (template == 2 && (sim.tight || sim.b <= sim.floor())) {
            template = 0;
        }
        if template == 5 && sim.l <= sim.floor() + 1 {
            template = 0;
        }
        match template {
            0 => {
                for _ in 0..rng.range(1, 3) {
                    match rng.below(100) {
                        0..=44 => {
                            let quick = rng.chance(30);
                            let mag = (s / 200).max(1);
                            let pnl = rng.range(-mag, mag);
                            let ts = if quick { tm - rng.below(46) as i64 } else { tm - 46 - rng.below(500) as i64 };
                            if let Some(ev) = ev_close(h, tm, ts, pnl) {
                                let mut probe = sim.clone();
                                probe.apply(&ev);
                                let over = probe.q > sim.limit();
                                let ev = if over && !noisy { ev_close(h, tm, tm - 100, pnl).unwrap_or(ev) } else { ev };
                                sim.apply(&ev);
                                events.push(ev);
                            }
                        }
                        45..=69 => {
                            let amount = rng.below(u64::try_from((s / 2000).max(1)).unwrap());
                            let ev = fee(MARKET_EUR, amount, tm);
                            sim.apply(&ev);
                            events.push(ev);
                        }
                        _ => {
                            if tm >= sim.lock || noisy {
                                // mostly EURUSD; sometimes BTC, the reduce-only, the disabled or the one-minute-session market
                                let m = if rng.chance(70) { MARKET_EUR } else { rng.below(5) as u16 };
                                let long = rng.chance(50);
                                let hm = if m == MARKET_EUR {
                                    h
                                } else {
                                    a.as_ref().and_then(|a| self.snap.rulesets.get(&a.ruleset_version)).map_or(H_OTHER, |rs| u32::from(rs.spread_e6[usize::from(m)]))
                                };
                                let pidx = if m == MARKET_EUR {
                                    0
                                } else if let Some(i) = extra_markets.iter().position(|x| *x == m) {
                                    i + 1
                                } else if extra_markets.len() < 2 {
                                    extra_markets.push(m);
                                    extra_markets.len()
                                } else {
                                    0
                                };
                                let price = if long { engine::ask_from_mid(MID, hm) } else { engine::bid_from_mid(MID, hm) }.unwrap_or(MID);
                                events.push(fill(m, long, MEUR, price, tm, pidx as u8));
                            }
                        }
                    }
                }
                flat = flat && !sim.tight && sim.b > sim.floor();
            }
            1 => {
                // profit run: a non-quick close that lifts b to 1.08 S or more
                if sim.tight {
                    events.push(JournalEvent::TightEnd { time: tm });
                    sim.tight = false;
                }
                let want = s / 100 * 108 + 1 + rng.below(u64::try_from((s / 200).max(1)).unwrap()) as i64;
                let pnl = (want - sim.b).max(1);
                if let Some(ev) = ev_close(h, tm, tm - 100 - rng.below(300) as i64, pnl) {
                    sim.apply(&ev);
                    events.push(ev);
                }
                flat = true;
            }
            2 => {
                let p_open = if noisy && rng.chance(15) { s / 100 } else { s / 100 + 1 + rng.below(u64::try_from((s / 25).max(1)).unwrap()) as i64 };
                let g = rng.below(u64::try_from(p_open + 1).unwrap()) as i64;
                let l_start = (sim.floor() + s / 100 * 4).min(sim.b);
                let l_level = l_start + (g - s / 100).max(0);
                events.push(JournalEvent::TightStart { time: tm, p_open, r: sim.b, l_start, g_peak: g, l_level, proof: 0 });
                flat = false;
            }
            3 => {
                let g = sim.g + 1 + rng.below(u64::try_from((s / 100).max(1)).unwrap()) as i64;
                let l = sim.l_start() + (g - s / 100).max(0);
                events.push(JournalEvent::FloorRaised { time: tm, g_peak: g, l_level: l, proof: 0 });
                flat = false;
            }
            4 => {
                events.push(JournalEvent::TightEnd { time: tm });
                flat = true;
            }
            5 => {
                let target = sim.floor() + 1 + rng.below(u64::try_from(sim.l - sim.floor()).unwrap_or(1).max(1)) as i64;
                let target = target.min(sim.l);
                // boundaries: one above the tight floor, at the static floor
                let target = if noisy && rng.chance(12) { sim.l + 1 } else if noisy && rng.chance(12) { sim.floor() } else { target };
                let pnl = target - sim.b;
                if pnl != 0 {
                    if let Some(ev) = ev_close(h, tm, tm - 100, pnl) {
                        sim.apply(&ev);
                        events.push(ev);
                    }
                }
                events.push(JournalEvent::Breach { time: tm, l_level: sim.l, b_after: sim.b, proof: 0 });
                flat = true;
            }
            6 => {
                let pnl = -(sim.b - sim.floor()) + rng.range(-(s / 100), s / 50);
                if let Some(ev) = ev_close(h, tm, tm - 100, pnl) {
                    sim.apply(&ev);
                    events.push(ev);
                }
                flat = flat && sim.b > sim.floor() && !sim.tight;
            }
            _ => {
                for _ in 0..rng.range(1, 4) {
                    events.push(match rng.below(6) {
                        0 => fee(MARKET_EUR, rng.next(), tm),
                        1 => JournalEvent::TightEnd { time: tm },
                        2 => JournalEvent::FloorRaised { time: tm, g_peak: rng.next() as i64, l_level: rng.next() as i64, proof: rng.below(4) as u8 },
                        3 => JournalEvent::Breach { time: tm, l_level: rng.next() as i64, b_after: rng.next() as i64, proof: 0 },
                        4 => JournalEvent::Fail { time: tm, reason: journal_types::FailReason::StaticFloor, b_after: sim.b, proof: 0 },
                        _ => fill(rng.below(8) as u16, rng.chance(50), rng.next(), rng.next(), tm, rng.below(3) as u8),
                    });
                }
            }
        }
        events.push(meta(tm, flat));
        let (first_seq, prev) = a.as_ref().map_or((1, [0u8; 32]), |a| (a.seq.saturating_add(1), a.journal_hash));
        let mut proofs = vec![proof(MARKET_EUR, tm, MID)];
        let mut markets = vec![MARKET_EUR];
        for m in &extra_markets {
            proofs.push(proof(*m, tm, MID));
            markets.push(*m);
        }
        let batch = JournalBatch { first_seq, prev_hash: prev, events, proofs };
        (batch, markets)
    }

    /// A `report_fail` batch: a closing loss that reaches the static floor, or a quick profit above the limit, then Fail, Meta.
    fn gen_fail_batch(&mut self, rng: &mut Rng, t: &Tgt, noisy: bool) -> (JournalBatch, Vec<u16>, u8) {
        let a = t.a.clone();
        let h = self.spread_for(a.as_ref());
        let tm = self.event_time(rng, a.as_ref(), noisy);
        let mut sim = Sim::of(a.as_ref());
        let s = sim.s;
        let mut events = vec![];
        let quick_fail = rng.chance(35);
        let reason = if quick_fail {
            let need = sim.limit() - sim.q + 1 + rng.below(u64::try_from((s / 200).max(1)).unwrap()) as i64;
            let pnl = 2 * need + rng.below(2) as i64;
            if let Some(ev) = ev_close(h, tm, tm - rng.below(46) as i64, pnl) {
                sim.apply(&ev);
                events.push(ev);
            }
            events.push(JournalEvent::Fail { time: tm, reason: journal_types::FailReason::PhaseQuick, b_after: sim.b, proof: 0 });
            1u8
        } else {
            let mut pnl = sim.floor() - sim.b - rng.below(u64::try_from((s / 100).max(1)).unwrap()) as i64;
            if noisy && rng.chance(12) {
                pnl = sim.floor() + 1 - sim.b; // one micro above the static floor: not a fail
            }
            if noisy && rng.chance(10) {
                // a quick overshoot that is reported as a static-floor fail
                let need = sim.limit() - sim.q + 1;
                if let Some(ev) = ev_close(h, tm, tm - rng.below(46) as i64, 2 * need) {
                    sim.apply(&ev);
                    events.push(ev);
                }
            }
            if pnl < 0 {
                if let Some(ev) = ev_close(h, tm, tm - 100, pnl) {
                    sim.apply(&ev);
                    events.push(ev);
                }
            }
            events.push(JournalEvent::Fail { time: tm, reason: journal_types::FailReason::StaticFloor, b_after: sim.b, proof: 0 });
            0u8
        };
        events.push(meta(tm, true));
        let (first_seq, prev) = a.as_ref().map_or((1, [0u8; 32]), |a| (a.seq.saturating_add(1), a.journal_hash));
        let batch = JournalBatch { first_seq, prev_hash: prev, events, proofs: vec![proof(MARKET_EUR, tm, MID)] };
        let reason = if noisy && rng.chance(25) { reason ^ 1 } else { reason };
        (batch, vec![MARKET_EUR], reason)
    }
}

/// A small structural near miss in a batch: one number off by one, a wrong sequence, a dropped or doubled event.
fn tweak_batch(rng: &mut Rng, b: &mut JournalBatch) {
    match rng.below(10) {
        0 => b.first_seq = b.first_seq.wrapping_add(1 + rng.below(2)),
        1 => b.prev_hash[rng.idx(32)] ^= 1 << rng.below(8),
        2 => {
            if b.events.len() > 1 {
                let i = rng.idx(b.events.len() - 1);
                b.events.remove(i);
            }
        }
        3 => {
            let i = rng.idx(b.events.len());
            let e = b.events[i].clone();
            b.events.insert(i, e);
        }
        4 => {
            let n = b.events.len();
            if n > 2 {
                b.events.swap(0, n - 2);
            }
        }
        5 => {
            if rng.chance(50) {
                b.proofs.clear();
            } else {
                // beyond the A-12 caps: 9+ events, 4+ proofs
                let evs = b.events.clone();
                while b.events.len() < 10 {
                    let at = b.events.len() - 1;
                    b.events.insert(at, evs[rng.idx(evs.len())].clone());
                }
                let p = b.proofs[0].clone();
                while b.proofs.len() < 5 {
                    b.proofs.push(p.clone());
                }
            }
        }
        6 => {
            let p = b.proofs[0].clone();
            b.proofs.push(PriceProof { timestamp_ns: p.timestamp_ns + 1_000_000_000, ..p });
            // structural: no Meta, Meta first, no events, a second Meta
            match rng.below(5) {
                0 => {
                    b.events.pop();
                }
                1 => {
                    let m = b.events.pop().unwrap();
                    b.events.insert(0, m);
                }
                2 => b.events.clear(),
                3 => {
                    let m = b.events.last().cloned().unwrap();
                    b.events.push(m);
                }
                _ => {}
            }
        }
        _ => {
            let i = rng.idx(b.events.len());
            let d = *rng.pick(&[1i64, -1, 2, 1000, i64::MAX / 2]);
            match &mut b.events[i] {
                JournalEvent::Close { pnl, credited, qty, quick, timer_start, entry_e12, exit_e12, market, side, .. } => match rng.below(9) {
                    0 => *pnl = pnl.wrapping_add(d),
                    1 => *credited = credited.wrapping_add(d),
                    2 => *qty = qty.wrapping_add(d as u64),
                    3 => *quick = !*quick,
                    4 => *timer_start = timer_start.wrapping_add(d),
                    5 => *entry_e12 = entry_e12.wrapping_add(d as u64),
                    6 => *exit_e12 = if rng.chance(50) { 0 } else { exit_e12.wrapping_add(d as u64) },
                    7 => *market = rng.below(6) as u16,
                    _ => *side = if *side == journal_types::Side::Long { journal_types::Side::Short } else { journal_types::Side::Long },
                },
                JournalEvent::Fill { price_e12, qty, time, proof, .. } => match rng.below(4) {
                    0 => *price_e12 = price_e12.wrapping_add(d as u64),
                    1 => *qty = qty.wrapping_add(d as u64),
                    2 => *proof = *rng.pick(&[255u8, 9, 1]),
                    _ => *time = time.wrapping_add(d),
                },
                JournalEvent::Fee { amount, .. } => *amount = amount.wrapping_add(d as u64),
                JournalEvent::TightStart { p_open, r, l_start, g_peak, l_level, .. } => match rng.below(5) {
                    0 => *p_open = p_open.wrapping_add(d),
                    1 => *r = r.wrapping_add(d),
                    2 => *l_start = l_start.wrapping_add(d),
                    3 => *g_peak = g_peak.wrapping_add(d),
                    _ => *l_level = l_level.wrapping_add(d),
                },
                JournalEvent::FloorRaised { g_peak, l_level, .. } => {
                    if rng.chance(50) {
                        *g_peak = g_peak.wrapping_add(d)
                    } else {
                        *l_level = l_level.wrapping_add(d)
                    }
                }
                JournalEvent::Breach { l_level, b_after, .. } => {
                    if rng.chance(50) {
                        *l_level = l_level.wrapping_add(d)
                    } else {
                        *b_after = b_after.wrapping_add(d)
                    }
                }
                JournalEvent::Fail { b_after, .. } => *b_after = b_after.wrapping_add(d),
                JournalEvent::Meta { time, flat, engine_version, .. } => match rng.below(3) {
                    0 => *time = time.wrapping_add(d),
                    1 => *flat = !*flat,
                    _ => *engine_version = engine_version.wrapping_add(1),
                },
                JournalEvent::TightEnd { time } => *time = time.wrapping_add(d),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------- good calls

impl World {
    /// The flagship generator: a call that is well formed for the current state of the world (when `noisy`, some arguments
    /// are made wrong on purpose). Returns `None` only when the instruction cannot be built at all.
    fn good(&mut self, rng: &mut Rng, op: Op, noisy: bool) -> Option<Instruction> {
        let noise = |rng: &mut Rng| noisy && rng.chance(35);
        let ix = match op {
            InitConfig => {
                let mut args = default_args();
                if noise(rng) {
                    args.pass_delay_secs = 5;
                }
                self.j.v.e.init_ix(&args)
            }
            SetEvaluator => {
                let held_now = self.held.contains(&self.snap.cfg.evaluator);
                let key = if !held_now && rng.chance(90) || rng.chance(70) {
                    self.evs[rng.idx(2)].pubkey()
                } else if rng.chance(50) {
                    self.attackers[0].pubkey()
                } else {
                    Pubkey::new_from_array([0x55; 32])
                };
                let ad = self.admins();
                let mut metas = admin_metas((ad.a, true), (ad.b, true));
                if noise(rng) {
                    metas.swap(1, 2);
                }
                set_evaluator_ix(metas, key)
            }
            SetLimits => {
                let ad = self.admins();
                let (mut d, mut cap, mut win) = (
                    *rng.pick(&[c::MIN_PASS_DELAY_SECS, c::MAX_PASS_DELAY_SECS, c::DEFAULT_PASS_DELAY_SECS, 300_000, 500_000]),
                    *rng.pick(&[20_000_000_000u64, 100_000_000_000, 100_000_000_000, 5_000_000_000, 1, u64::MAX]),
                    *rng.pick(&[86_400u32, 86_400, 3_600, 604_800, 1, u32::MAX]),
                );
                if noise(rng) {
                    d = *rng.pick(&[0, 1, c::MIN_PASS_DELAY_SECS - 1, c::MAX_PASS_DELAY_SECS + 1, u32::MAX]);
                    cap = *rng.pick(&[0u64, 1]);
                    win = *rng.pick(&[0u32, 1]);
                }
                set_limits_ix(admin_metas((ad.a, true), (ad.b, true)), d, cap, win)
            }
            SetPaused => {
                let ad = self.admins();
                let p = if self.snap.cfg.paused == 1 { rng.chance(40) } else { rng.chance(10) };
                set_paused_ix(admin_metas((ad.a, true), (ad.b, true)), p)
            }
            CreateRuleset => {
                let mut args = spread_ruleset();
                let next = self.snap.cfg.current_ruleset + 1;
                args.version = if noise(rng) { next + rng.below(3) as u16 } else { next };
                args.fee_bps = *rng.pick(&[0u16, 40, 40, 1000, 1001]);
                args.spread_e6[usize::from(MARKET_EUR)] = *rng.pick(&[30u16, 30, 30, 1, 10, 0]);
                if noise(rng) {
                    args.tier_count = *rng.pick(&[0u8, 11, 9]);
                    args.ladder_len = *rng.pick(&[0u8, 6, 4]);
                }
                create_ruleset_ix(self.payer(), self.admins(), &args)
            }
            CreateMarket => {
                let id = if noise(rng) { rng.below(140) as u16 } else { self.snap.cfg.market_count };
                self.next_market_feed = self.next_market_feed.wrapping_add(1).max(100);
                let mut a: CreateMarketArgs = create_market_args(id, rng.below(8) as u8, self.next_market_feed);
                if noise(rng) {
                    a.params.status = 3;
                }
                create_market_ix(self.payer(), self.admins(), &a)
            }
            UpdateMarket => {
                let id = rng.below(u64::from(self.snap.cfg.market_count.max(1))) as u16;
                let cur: Option<Market> = self.j.v.e.svm.get_account(&market_pda(id)).and_then(|x| Market::try_deserialize(&mut x.data.as_slice()).ok());
                let mut params = market_params(7);
                if let Some(m) = &cur {
                    params.feed_id = m.feed_id;
                    params.status = if id == MARKET_EUR && rng.chance(90) { 1 } else { rng.below(3) as u8 };
                    params.sessions = m.sessions;
                    params.session_count = m.session_count;
                    params.max_age_secs = m.max_age_secs;
                    params.qty_step = m.qty_step;
                    params.qty_min = m.qty_min;
                    params.max_notional_usd = m.max_notional_usd;
                    params.quote_conv_market = m.quote_conv_market;
                }
                if rng.chance(15) {
                    params.qty_min += params.qty_step * rng.below(5);
                }
                if noise(rng) {
                    match rng.below(4) {
                        0 => params.feed_id = [0; 32],
                        1 => params.status = 9,
                        2 => params.max_age_secs = 0,
                        _ => params.quote_conv_market = id,
                    }
                }
                update_market_ix(self.admins(), &UpdateMarketArgs { market_id: id, params })
            }
            BanWallet => {
                let who = if rng.chance(80) { self.traders[rng.idx(self.traders.len())].pubkey() } else { Pubkey::new_from_array([0x66; 32]) };
                let hash = if noise(rng) { [0u8; 32] } else { rng.hash32() };
                ban_wallet_ix(self.payer(), self.admins(), who, hash)
            }
            UnbanWallet => {
                let who = if rng.chance(90) { self.traders[rng.idx(self.traders.len())].pubkey() } else { Pubkey::new_from_array([0x66; 32]) };
                unban_wallet_ix(self.admins(), who)
            }
            OpenAccount => {
                let ti = rng.idx(self.traders.len());
                let tr = self.traders[ti].pubkey();
                let mut oa = self.j.v.open_accounts(&tr);
                oa.ruleset = ruleset_pda(self.snap.cfg.current_ruleset.max(1));
                let tier = if noise(rng) { rng.below(14) as u8 } else { rng.below(10) as u8 };
                if noise(rng) && rng.chance(30) {
                    oa.payer = self.traders[(ti + 1) % self.traders.len()].pubkey();
                }
                oa.to_ix(tier)
            }
            ResetAccount => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == c::STATUS_FAILED && a.pass_requested_at == 0 && a.pass_pending != 3));
                let prev = if noise(rng) { t.cid + 1 } else { t.cid };
                reset_ix(self, &t, prev)
            }
            PostJournal => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.held == 0 && a.pass_pending != 2));
                let (mut batch, mut markets) = self.gen_post_batch(rng, &t, noisy);
                if noise(rng) {
                    tweak_batch(rng, &mut batch);
                }
                if noisy && rng.chance(10) {
                    markets = (0..self.snap.cfg.market_count.min(12)).collect();
                }
                let bytes = batch.to_bytes().unwrap_or_else(|_| rng.bytes(40));
                self.ix_post(&t, bytes, &markets)
            }
            ReportFail => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.pass_pending != 2));
                let (mut batch, markets, reason) = self.gen_fail_batch(rng, &t, noisy);
                if noise(rng) {
                    tweak_batch(rng, &mut batch);
                }
                let bytes = batch.to_bytes().unwrap_or_else(|_| rng.bytes(40));
                self.ix_report_fail(&t, bytes, reason, &markets)
            }
            RequestAdvance => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.pass_pending == 0 && a.held == 0 && a.tight_on == 0 && a.flat == 1 && i128::from(a.b) * 100 >= i128::from(a.size_s) * 108));
                self.ix_request(&t)
            }
            CancelPass => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.pass_pending == 1));
                if let Some(a) = &t.a {
                    if rng.chance(75) {
                        self.set_clock_to(a.pass_requested_at + c::CANCEL_PASS_AFTER_SECS + rng.below(2) as i64 - i64::from(noisy && rng.chance(30)));
                    }
                }
                self.ix_cancel(&t)
            }
            ConfirmPass => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.pass_pending == 1 && a.held == 0));
                self.ix_confirm(&t)
            }
            RejectPass => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.pass_pending == 1));
                let reason = if noise(rng) { rng.below(256) as u8 } else { rng.below(4) as u8 };
                self.ix_reject(&t, reason)
            }
            VetoPass => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| matches!(a.pass_pending, 1 | 2)));
                self.ix_veto(&t)
            }
            FinalizePass => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.pass_pending == 2 && a.held == 0));
                if let Some(a) = &t.a {
                    if rng.chance(85) {
                        let delay = i64::from(self.snap.cfg.pass_delay_secs);
                        self.set_clock_to(a.pass_proposed_at + delay + rng.below(2) as i64 - i64::from(noisy && rng.chance(30)));
                    }
                }
                let payer = if rng.chance(80) { self.payer() } else { self.traders[rng.idx(self.traders.len())].pubkey() };
                finalize_ix(self, &t, payer)
            }
            HoldAccount => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.held == 0));
                let hash = if noise(rng) { [0u8; 32] } else { rng.hash32() };
                self.ix_hold(&t, hash)
            }
            ReleaseHold => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.held == 1));
                self.ix_release_hold(&t)
            }
            FailForViolation => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.held == 1 && a.status == 0));
                let hash = if noise(rng) { [0u8; 32] } else { rng.hash32() };
                self.ix_violation(&t, hash)
            }
            TouchActivity => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0));
                if let Some(a) = &t.a {
                    if rng.chance(60) {
                        self.set_clock_to(a.last_activity_pushed + c::ACTIVITY_THROTTLE_SECS + rng.below(3) as i64);
                    }
                }
                self.ix_touch(&t)
            }
            ReleaseAccount => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0));
                if let Some(a) = &t.a {
                    if rng.chance(60) {
                        self.set_clock_to(a.last_activity_pushed + c::ACTIVITY_THROTTLE_SECS + rng.below(3) as i64);
                    }
                }
                self.ix_release_acct(&t)
            }
            CloseAccount => {
                let t = self.pick_any(rng, |t| t.a.as_ref().is_some_and(|a| a.status != 0));
                if let Some(a) = &t.a {
                    if rng.chance(75) {
                        self.set_clock_to(a.pass_proposed_at + c::TERMINAL_RETENTION_SECS + rng.below(2) as i64 - i64::from(noisy && rng.chance(30)));
                    }
                }
                self.ix_close(&t)
            }
        };
        Some(ix)
    }
}

// ---------------------------------------------------------------------------------------------------- mutation layer

fn mutate_data(rng: &mut Rng, d: &mut Vec<u8>) {
    match rng.below(10) {
        0 => {
            for _ in 0..rng.range(1, 3) {
                if !d.is_empty() {
                    let i = rng.idx(d.len());
                    d[i] ^= 1 << rng.below(8);
                }
            }
        }
        1 => d.truncate(rng.idx(d.len() + 1)),
        2 => {
            let extra = rng.rbytes(64);
            d.extend(extra)
        }
        3 => {
            d.truncate(8.min(d.len()));
            let extra = rng.rbytes(200);
            d.extend(extra);
        }
        4 => {
            let disc = rng.pick(&OPS).disc();
            d.splice(0..8.min(d.len()), disc);
        }
        5 => {
            let junk = rng.bytes(8);
            d.splice(0..8.min(d.len()), junk);
        }
        6 => {
            d.truncate(8.min(d.len()));
            d.extend(std::iter::repeat_n(0xFFu8, rng.below(100) as usize));
        }
        7 => {
            d.truncate(8.min(d.len()));
            d.extend(std::iter::repeat_n(0u8, rng.below(100) as usize));
        }
        8 => {
            // a Borsh length prefix that promises far more than there is
            d.truncate(8.min(d.len()));
            let n = *rng.pick(&[u32::MAX, 1 << 16, 1 << 31, 10_000, 1_000_000]);
            d.extend(n.to_le_bytes());
            let extra = rng.rbytes(40);
            d.extend(extra);
        }
        _ => {
            // flip a whole byte somewhere in the arguments
            if d.len() > 8 {
                let i = 8 + rng.idx(d.len() - 8);
                d[i] = (rng.next() >> 8) as u8;
            }
        }
    }
}

impl World {
    fn mutate_accounts(&self, rng: &mut Rng, ix: &mut Instruction) {
        let n = ix.accounts.len();
        match rng.below(10) {
            0..=2 => {
                if n > 0 {
                    let i = rng.idx(n);
                    let key = *rng.pick(&self.pool.all);
                    ix.accounts[i].pubkey = key;
                }
            }
            3 | 4 => {
                // same-category substitution (another trader's account in the account slot, a wrong token account, ...)
                if n > 0 {
                    let i = rng.idx(n);
                    if let Some(cat) = self.pool.cat_of.get(&ix.accounts[i].pubkey) {
                        let list = &self.pool.by_cat[*cat];
                        if !list.is_empty() {
                            ix.accounts[i].pubkey = *rng.pick(list);
                        }
                    }
                }
            }
            5 => {
                if n > 1 {
                    let (i, j) = (rng.idx(n), rng.idx(n));
                    ix.accounts.swap(i, j);
                }
            }
            6 => {
                if n > 0 {
                    let i = rng.idx(n);
                    ix.accounts.remove(i);
                }
            }
            7 => {
                if n > 0 && n < 40 {
                    let m = ix.accounts[rng.idx(n)].clone();
                    let at = rng.idx(n + 1);
                    ix.accounts.insert(at, m);
                }
            }
            8 => {
                if n < 40 {
                    for _ in 0..rng.range(1, 3) {
                        let key = *rng.pick(&self.pool.all);
                        ix.accounts.push(AccountMeta { pubkey: key, is_signer: false, is_writable: rng.chance(50) });
                    }
                }
            }
            _ => {
                if n > 0 {
                    let i = rng.idx(n);
                    if rng.chance(50) {
                        ix.accounts[i].is_writable = !ix.accounts[i].is_writable;
                    } else {
                        ix.accounts[i].is_signer = !ix.accounts[i].is_signer;
                    }
                }
            }
        }
    }

    /// A signer flag is only legal on a key the harness holds; a re-flag of a held key in a substituted slot is allowed.
    fn fix_signers(&self, rng: &mut Rng, ix: &mut Instruction, kind: Kind) {
        for m in ix.accounts.iter_mut() {
            if m.is_signer && !self.held.contains(&m.pubkey) {
                m.is_signer = false;
            }
            if kind != Kind::Good && !m.is_signer && self.held.contains(&m.pubkey) && m.pubkey != self.payer() && rng.chance(30) {
                m.is_signer = true;
            }
        }
    }

    fn random_call(&self, rng: &mut Rng) -> Call {
        let op = *rng.pick(&OPS);
        let mut data = if rng.chance(50) { op.disc().to_vec() } else { rng.bytes(8) };
        let extra = rng.rbytes(120);
        data.extend(extra);
        let n = rng.below(24) as usize;
        let accounts = (0..n)
            .map(|_| {
                let key = *rng.pick(&self.pool.all);
                AccountMeta { pubkey: key, is_signer: rng.chance(30), is_writable: rng.chance(50) }
            })
            .collect();
        Call { op: Some(op), ext: false, ix: Instruction { program_id: PROGRAM_ID, accounts, data }, kind: Kind::Random }
    }

    fn make_call(&mut self, rng: &mut Rng, kind: Kind, op: Op) -> Call {
        // time passes
        match rng.below(100) {
            0..=11 => self.advance(1 + rng.below(120) as i64),
            12 => self.advance(3_600 + rng.below(90_000) as i64),
            _ => {}
        }
        if kind == Kind::Random {
            let mut c = self.random_call(rng);
            self.fix_signers(rng, &mut c.ix, Kind::Random);
            return c;
        }
        let noisy = kind != Kind::Good;
        let Some(mut ix) = self.good(rng, op, noisy) else { return self.random_call(rng) };
        match kind {
            Kind::Near => {
                if rng.chance(60) {
                    self.mutate_accounts(rng, &mut ix);
                }
            }
            Kind::Permuted => {
                for _ in 0..rng.range(1, 3) {
                    self.mutate_accounts(rng, &mut ix);
                }
                if rng.chance(20) {
                    mutate_data(rng, &mut ix.data);
                }
            }
            Kind::Garbage => {
                for _ in 0..rng.range(1, 3) {
                    mutate_data(rng, &mut ix.data);
                }
                if rng.chance(30) {
                    self.mutate_accounts(rng, &mut ix);
                }
            }
            _ => {}
        }
        self.fix_signers(rng, &mut ix, kind);
        Call { op: Some(op), ext: false, ix, kind }
    }

    /// Outside events on the vault side (the vault admins are the public test keys, signature verification is off): the
    /// permissionless `mark_abandoned`, `pause_product`, `reactivate_product` and `update_product_config` (same tiers, other
    /// ladder / payout count / fee split). None of them may change anything of OUR program's accounts.
    fn ext_call(&mut self, rng: &mut Rng) -> Option<Call> {
        let ix = match rng.weighted(&[60, 12, 14, 14]) {
            0 => {
                let t = self.pick(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0))?;
                let a = t.a.as_ref()?;
                if rng.chance(70) {
                    let want = a.last_activity_pushed + 604_801;
                    self.set_clock_to(want);
                }
                si::mark_abandoned(
                    VAULT_ID,
                    self.payer(),
                    registry_pda(),
                    &[AccountMeta::new(t.vts, false)],
                    si::MarkAbandonedArgs { trader_wallet: t.trader, product_program_id: PROGRAM_ID, challenge_id: t.cid },
                )
            }
            1 => si::pause_product(VAULT_ID, VAULT_SL8_ADMIN, VAULT_ROV_ADMIN, registry_pda(), &[], si::PauseProductArgs { product_program_id: PROGRAM_ID }),
            2 => si::reactivate_product(VAULT_ID, VAULT_SL8_ADMIN, VAULT_ROV_ADMIN, registry_pda(), &[], si::ReactivateProductArgs { product_program_id: PROGRAM_ID }),
            _ => {
                let cur = self.j.v.registry();
                let mut ladder = cur.reset_price_bps.clone();
                if rng.chance(50) {
                    ladder = vec![10_000, 11_500, 12_000, 12_750, 13_000];
                } else if let Some(x) = ladder.get_mut(rng.idx(5)) {
                    *x = 5_000 + rng.below(10_000) as u16;
                }
                let mut tiers: Vec<si::ChallengeSize> = cur.challenge_sizes.iter().map(|(size, cost)| si::ChallengeSize { size: *size, cost: *cost }).collect();
                if rng.chance(12) {
                    // the vault and the rule set disagree about the tiers from now on
                    match rng.below(3) {
                        0 => {
                            tiers.pop();
                        }
                        1 => tiers[0].cost += 1_000_000,
                        _ => tiers[1].size += 1_000_000,
                    }
                }
                si::update_product_config(
                    VAULT_ID,
                    VAULT_SL8_ADMIN,
                    VAULT_ROV_ADMIN,
                    registry_pda(),
                    &[],
                    si::UpdateProductConfigArgs {
                        product_program_id: PROGRAM_ID,
                        challenge_sizes: tiers,
                        fee_split_bps: if rng.chance(70) { cur.fee_split_bps } else { *rng.pick(&[0u16, 5_000, 6_500, 10_000]) },
                        max_payout_count: if rng.chance(80) { cur.max_payout_count } else { *rng.pick(&[1u64, 4, 5]) },
                        reset_price_bps: ladder,
                    },
                )
            }
        };
        Some(Call { op: None, ext: true, ix, kind: Kind::Good })
    }

    /// Writes garbage into a trading account and marks its trader as tainted: only no-panic and failure-atomicity are
    /// asserted for that trader from then on.
    fn inject(&mut self, rng: &mut Rng) {
        let Some(t) = self.pick(rng, |t| t.a.is_some()) else { return };
        let Some(mut acc) = self.j.v.e.svm.get_account(&t.acct) else { return };
        if acc.data.len() != 207 {
            return;
        }
        // offsets of the flag bytes, status, phase, pass_pending, held and the numeric fields
        let off = *rng.pick(&[158usize, 159, 160, 161, 162, 163, 165, 166, 64, 72, 80, 167, 175, 183, 191, 199, 104, 112, 152]);
        let n = if !(158..=166).contains(&off) { 8 } else { 1 };
        let val = match rng.below(4) {
            0 => vec![0xFF; n],
            1 => vec![0; n],
            2 => vec![2 + rng.below(250) as u8; n],
            _ => rng.bytes(n),
        };
        acc.data[off..off + n].copy_from_slice(&val);
        self.j.v.e.svm.set_account(t.acct, acc).unwrap();
        self.tainted.insert(t.trader);
        self.dmap = world_map(&self.j.v.e.svm, &self.j.v.e.payer.pubkey());
        if let Ok(s) = self.take_snap() {
            self.snap = s;
        }
    }
}

// ---------------------------------------------------------------------------------------------------- execution + checks

/// FINDING JRN-1 (found earlier by journal_attacks.rs): a lying outer Borsh length prefix of the `batch_bytes` argument of
/// post_journal / report_fail aborts the program with "memory allocation failed". Counted, not re-reported.
fn is_known_jrn1(ix: &Instruction, f: &FailedTransactionMetadata) -> bool {
    let d = &ix.data;
    let is_journal = d.len() >= 12 && (d[..8] == PostJournal.disc() || d[..8] == ReportFail.disc());
    is_journal && u32::from_le_bytes([d[8], d[9], d[10], d[11]]) >= 4096 && f.meta.logs.iter().any(|l| l.contains("memory allocation failed"))
}

fn describe_key(w: &World, k: &Pubkey) -> String {
    let label = |k: &Pubkey| -> String {
        if *k == config_pda() {
            "config".into()
        } else if *k == tally_pda() {
            "tally".into()
        } else if *k == w.payer() {
            "payer".into()
        } else if let Some(i) = w.traders.iter().position(|t| t.pubkey() == *k) {
            format!("trader{i}")
        } else if let Some(i) = w.evs.iter().position(|t| t.pubkey() == *k) {
            format!("evaluator{i}")
        } else if let Some(i) = w.attackers.iter().position(|t| t.pubkey() == *k) {
            format!("attacker{i}")
        } else if *k == c::SL8_ADMIN {
            "sl8_admin".into()
        } else if *k == c::ROV_ADMIN {
            "rov_admin".into()
        } else if let Some(t) = w.tg.iter().find(|t| t.acct == *k) {
            format!("acct(trader{} #{})", t.ti, t.cid)
        } else if let Some(t) = w.tg.iter().find(|t| t.wallet == *k) {
            format!("wallet(trader{})", t.ti)
        } else if let Some(t) = w.tg.iter().find(|t| t.vts == *k) {
            format!("vault_record(trader{} #{})", t.ti, t.cid)
        } else {
            let s = k.to_string();
            format!("{}..", &s[..6])
        }
    };
    label(k)
}

fn describe(w: &World, call: &Call) -> String {
    let label = |k: &Pubkey| describe_key(w, k);
    let accs: Vec<String> = call.ix.accounts.iter().map(|m| format!("{}{}{}", label(&m.pubkey), if m.is_signer { "[S]" } else { "" }, if m.is_writable { "[W]" } else { "" })).collect();
    let data = hexs(&call.ix.data[..call.ix.data.len().min(96)]);
    format!(
        "{} kind={} clock={} accounts=[{}] data({} bytes)={}{}",
        if call.ext { "vault.mark_abandoned" } else { call.op.map_or("unknown-discriminator", Op::name) },
        KIND_NAMES[KINDS.iter().position(|k| *k == call.kind).unwrap()],
        w.clock,
        accs.join(", "),
        call.ix.data.len(),
        data,
        if call.ix.data.len() > 96 { "..." } else { "" }
    )
}

impl World {
    /// Sends one call and applies every check. Violations are appended to `st.violations` (the loop goes on).
    fn exec(&mut self, call: Call, st: &mut Stats, tag: &str) {
        self.exec_many(vec![call], st, tag)
    }

    /// Sends ONE transaction holding all `calls` (several instructions, the attack class of a transaction that mixes or repeats
    /// instructions) and applies the checks that hold for any sequence: no panic, failure atomicity, frame, the invariants, the
    /// union of allowed changes, the hash chain. A single call also gets the authorisation and precondition oracles.
    fn exec_many(&mut self, calls: Vec<Call>, st: &mut Stats, tag: &str) {
        let mut calls = calls;
        for c in calls.iter_mut() {
            if !c.ext {
                c.op = eff_op(c);
            }
        }
        let multi = calls.len() > 1;
        let ixs: Vec<Instruction> = calls.iter().map(|c| c.ix.clone()).collect();
        let label_op: &'static str = if multi { "multi" } else { op_label(&calls[0]) };
        let kind_i = KINDS.iter().position(|k| *k == calls[0].kind).unwrap();
        let payer_before = self.j.v.e.svm.get_balance(&self.payer()).unwrap_or(0);
        let t_send = Instant::now();
        let res = self.j.v.e.send(&ixs, &[]);
        st.send_nanos += t_send.elapsed().as_nanos();
        st.sends += 1;
        if multi {
            st.multi_tries += 1;
        } else {
            match calls[0].op {
                Some(op) => st.tries[op as usize] += 1,
                None => st.ext_tries += u64::from(calls[0].ext),
            }
            st.kind_tries[kind_i] += 1;
        }
        let desc = |w: &World| calls.iter().map(|c| describe(w, c)).collect::<Vec<_>>().join("\n         + ");
        let report = |st: &mut Stats, id: &str, msg: String, w: &World| {
            let key = format!("{id} / {label_op}");
            let n = st.viol_count.entry(key).or_default();
            *n += 1;
            if *n <= 3 {
                st.violations.push(format!("[{tag}] VIOLATION {id}: {msg}\n      call: {}", desc(w)));
            }
        };
        match res {
            Err(f) => {
                let label = err_label(&f);
                *st.hist.entry(format!("{label_op} -> {label}")).or_default() += 1;
                if label == "ComputationalBudgetExceeded" {
                    st.budget_errs += 1;
                }
                if let Some((ours, line)) = panic_of(&f, &PROGRAM_ID) {
                    if ours && ixs.iter().any(|ix| is_known_jrn1(ix, &f)) {
                        st.known_jrn1 += 1;
                    } else if ours {
                        report(st, "PANIC", format!("the program panicked or aborted: {line}\n      err: {:?}", f.err), self);
                    } else {
                        st.vault_panics.push(format!("[{tag}] {line} <- {}", desc(self)));
                    }
                }
                let after = world_map(&self.j.v.e.svm, &self.j.v.e.payer.pubkey());
                let paid = payer_before.saturating_sub(self.j.v.e.svm.get_balance(&self.payer()).unwrap_or(0));
                if paid != 5_000 * self.j.v.e.last_tx.signatures as u64 {
                    report(st, "FEE", format!("a failing transaction moved the fee payer's lamports by {paid}, not 5000 x {} signatures", self.j.v.e.last_tx.signatures), self);
                }
                if after != self.dmap {
                    let who: Vec<String> = map_diff(&self.dmap, &after).iter().map(|k| describe_key(self, k)).collect();
                    report(st, "ATOMICITY", format!("a FAILING transaction ({label}) changed the world; accounts changed: {who:?}"), self);
                    self.dmap = after;
                }
            }
            Ok(meta) => {
                *st.hist.entry(format!("{label_op} -> OK")).or_default() += 1;
                if multi {
                    st.multi_ok += 1;
                } else {
                    match calls[0].op {
                        Some(op) => {
                            st.ok[op as usize] += 1;
                            st.cu_max[op as usize] = st.cu_max[op as usize].max(meta.compute_units_consumed);
                        }
                        None => st.ext_ok += u64::from(calls[0].ext),
                    }
                    st.kind_ok[kind_i] += 1;
                    if calls[0].op.is_none() && !calls[0].ext {
                        report(st, "DISC", "a transaction with an unknown instruction discriminator succeeded".to_string(), self);
                    }
                }
                let t_b = Instant::now();
                self.after_success(&calls, &meta, st, &report);
                st.other_nanos += t_b.elapsed().as_nanos();
            }
        }
    }

    fn after_success(&mut self, calls: &[Call], meta: &TransactionMetadata, st: &mut Stats, report: &dyn Fn(&mut Stats, &str, String, &World)) {
        let new_map = world_map(&self.j.v.e.svm, &self.j.v.e.payer.pubkey());
        // frame: only accounts the transaction named may differ afterwards
        let named: BTreeSet<Pubkey> = calls.iter().flat_map(|c| c.ix.accounts.iter().map(|m| m.pubkey)).collect();
        for k in map_diff(&self.dmap, &new_map) {
            if !named.contains(&k) && k != self.payer() {
                report(st, "FRAME", format!("an account the transaction did not name changed: {}", describe_key(self, &k)), self);
            }
        }
        self.dmap = new_map;
        let prev = self.snap.clone();
        let cur = match self.take_snap() {
            Ok(s) => s,
            Err(e) => {
                report(st, "SNAPSHOT", e, self);
                return;
            }
        };
        let ops: Vec<Op> = calls.iter().filter_map(|c| c.op).collect();
        let single = calls.len() == 1;
        let logs = event_logs(meta);
        let any_journal = ops.iter().any(|o| matches!(o, PostJournal | ReportFail));
        let mut viols: Vec<Viol> = vec![];
        st.invariant_runs += 1;
        viols.extend(check_static(&cur, &self.tainted));
        viols.extend(check_transition_ops(&prev, &cur, &ops, &self.tainted, if single && any_journal { Some(logs.len()) } else { None }));
        if let (true, Some(op)) = (single, calls[0].op) {
            viols.extend(check_auth(op, &calls[0].ix, &prev));
            viols.extend(check_pre(op, &calls[0].ix, &prev, &cur, &self.tainted));
        }
        // the hash chain, recomputed from the emitted EventLogs
        if any_journal {
            st.chain_checks += 1;
            let mut by_acct: BTreeMap<Pubkey, Vec<&setl8_lev_trading::events::EventLog>> = BTreeMap::new();
            for l in &logs {
                by_acct.entry(l.account).or_default().push(l);
            }
            for (k, ls) in by_acct {
                let (Some(pa), Some(na)) = (prev.accts.get(&k), cur.accts.get(&k)) else {
                    viols.push(("J-0", format!("EventLogs for {k}, which is not a known account")));
                    continue;
                };
                if self.tainted.contains(&pa.trader) {
                    continue;
                }
                let mut head = pa.journal_hash;
                for (i, l) in ls.iter().enumerate() {
                    if l.seq != pa.seq + 1 + i as u64 {
                        viols.push(("J-1", format!("EventLog seq {} where {} was expected", l.seq, pa.seq + 1 + i as u64)));
                    }
                    let ev = JournalEvent::decode(&l.event_bytes);
                    let pr = if l.proof_bytes.is_empty() { Ok(None) } else { PriceProof::decode(&l.proof_bytes).map(Some) };
                    match (ev, pr) {
                        (Ok(ev), Ok(pr)) => match next_hash(&NativeSha256, &head, l.seq, &ev, pr.as_ref()) {
                            Ok(h) => head = h,
                            Err(e) => viols.push(("J-2", format!("next_hash failed on an emitted log: {e:?}"))),
                        },
                        _ => viols.push(("J-3", "an emitted EventLog does not decode".to_string())),
                    }
                }
                if head != na.journal_hash || na.seq != pa.seq + ls.len() as u64 {
                    viols.push(("J-4", format!("stored (seq {}, head {}) differs from the chain recomputed from the {} emitted logs (seq {}, head {})", na.seq, hexs(&na.journal_hash[..6]), ls.len(), pa.seq + ls.len() as u64, hexs(&head[..6]))));
                }
            }
        }
        // coverage: which states and events the successful calls left behind
        for l in &logs {
            if let Ok(ev) = JournalEvent::decode(&l.event_bytes) {
                let n = match ev {
                    JournalEvent::Fill { .. } => "Fill",
                    JournalEvent::Close { .. } => "Close",
                    JournalEvent::Fee { .. } => "Fee",
                    JournalEvent::TightStart { .. } => "TightStart",
                    JournalEvent::FloorRaised { .. } => "FloorRaised",
                    JournalEvent::TightEnd { .. } => "TightEnd",
                    JournalEvent::Breach { .. } => "Breach",
                    JournalEvent::Fail { .. } => "Fail",
                    JournalEvent::Meta { .. } => "Meta",
                };
                *st.ev_hist.entry(n).or_default() += 1;
            }
        }
        for a in cur.accts.values() {
            if self.tainted.contains(&a.trader) {
                continue;
            }
            let key = u32::from(a.status) | (u32::from(a.phase) << 2) | (u32::from(a.pass_pending) << 5) | (u32::from(a.tight_on) << 7) | (u32::from(a.latch) << 8) | (u32::from(a.held) << 9) | (u32::from(a.flat) << 10) | (u32::from(a.lock_until > cur.clock) << 11) | (u32::from(a.q_tally > 0) << 12);
            *st.states.entry(key).or_default() += 1;
            let mut hit = |n: &'static str, b: bool| {
                if b {
                    *st.cov.entry(n).or_default() += 1;
                }
            };
            hit("account observations", true);
            hit("tight_on", a.tight_on == 1);
            hit("tight_on and latch", a.tight_on == 1 && a.latch == 1);
            hit("latch", a.latch == 1);
            hit("locked (breach lock running)", a.lock_until > cur.clock);
            hit("q_tally > 0", a.q_tally > 0);
            hit("held", a.held == 1);
            hit("pass_pending 1", a.pass_pending == 1);
            hit("pass_pending 2", a.pass_pending == 2);
            hit("pass_pending 3 (violation marker)", a.pass_pending == 3);
            hit("phase 2", a.phase == 2);
            hit("phase 3", a.phase == 3);
            hit("phase 4", a.phase == 4);
            hit("phase 5", a.phase == 5);
            hit("status Failed", a.status == 1);
            hit("status Abandoned", a.status == 2);
            hit("status Graduated", a.status == 3);
            hit("vetoes > 0", a.vetoes > 0);
            hit("b >= 1.08 S and flat (pass-ready)", i128::from(a.b) * 100 >= i128::from(a.size_s) * 108 && a.flat == 1);
            hit("b below static floor (open position)", i128::from(a.b) * 100 <= i128::from(a.size_s) * if a.latch == 1 { 96 } else { 92 });
        }
        for (id, msg) in viols {
            report(st, id, msg, self);
        }
        self.snap = cur;
        if ops.iter().any(|o| matches!(o, OpenAccount | ResetAccount)) {
            self.rebuild_pool();
        }
    }
}

// ---------------------------------------------------------------------------------------------------- runner

fn op_weights() -> [u64; 25] {
    // order of OPS
    [
        1,  // init_config
        2,  // set_evaluator
        3,  // set_limits
        2,  // set_paused
        2,  // create_ruleset
        2,  // create_market
        3,  // update_market
        2,  // ban_wallet
        3,  // unban_wallet
        6,  // open_account
        6,  // reset_account
        28, // post_journal
        8,  // request_advance
        3,  // cancel_pass
        8,  // confirm_pass
        3,  // reject_pass
        3,  // veto_pass
        9,  // finalize_pass
        3,  // report_fail
        2,  // hold_account
        3,  // release_hold
        1,  // fail_for_violation
        4,  // touch_activity
        3,  // release_account
        3,  // close_account
    ]
}

impl World {
    fn run_step(&mut self, rng: &mut Rng, st: &mut Stats, tag: &str, op: Op, ix: Instruction, steps: &mut u64) -> bool {
        let mut ix = ix;
        self.fix_signers(rng, &mut ix, Kind::Good);
        let before = st.ok[op as usize];
        self.exec(Call { op: Some(op), ext: false, ix, kind: Kind::Good }, st, tag);
        *steps += 1;
        st.iters += 1;
        st.ok[op as usize] > before
    }

    fn tgt_of(&self, key: &Pubkey) -> Option<Tgt> {
        self.tg.iter().find(|t| t.acct == *key).cloned()
    }

    /// Scripted real flow (every step is an ordinary checked call): profit run, request, confirm and, from phase 2 on,
    /// the delay and `finalize_pass`, for up to `cycles` phases. Gets the rare states (finalize, graduation) into the run.
    fn script_cycles(&mut self, rng: &mut Rng, st: &mut Stats, tag: &str, cycles: u32) -> u64 {
        let mut steps = 0u64;
        let Some(t0) = self.pick(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.held == 0 && a.pass_pending == 0)) else { return 0 };
        let key = t0.acct;
        // prerequisites: not paused, a held evaluator, a cap that fits an 8 % payout
        if self.snap.cfg.paused == 1 {
            let ad = self.admins();
            let ix = sign_admins(ad, set_paused_ix(admin_metas((ad.a, true), (ad.b, true)), false));
            self.run_step(rng, st, tag, SetPaused, ix, &mut steps);
        }
        if !self.held.contains(&self.snap.cfg.evaluator) {
            let ad = self.admins();
            let ix = sign_admins(ad, set_evaluator_ix(admin_metas((ad.a, true), (ad.b, true)), self.evs[0].pubkey()));
            self.run_step(rng, st, tag, SetEvaluator, ix, &mut steps);
        }
        if self.snap.cfg.max_payout_per_window < 40_000_000_000 || self.snap.cfg.window_secs < 60 {
            let ad = self.admins();
            let ix = sign_admins(ad, set_limits_ix(admin_metas((ad.a, true), (ad.b, true)), c::MIN_PASS_DELAY_SECS, 100_000_000_000, 86_400));
            self.run_step(rng, st, tag, SetLimits, ix, &mut steps);
        }
        for _ in 0..cycles {
            let Some(t) = self.tgt_of(&key) else { break };
            let Some(a) = t.a.clone() else { break };
            if a.status != 0 || a.held == 1 || a.pass_pending != 0 || self.tainted.contains(&a.trader) {
                break;
            }
            if i128::from(a.b) * 100 < i128::from(a.size_s) * 108 || a.flat != 1 || a.tight_on != 0 {
                self.force_tpl = Some(1);
                let (batch, markets) = self.gen_post_batch(rng, &t, false);
                let ix = self.ix_post(&t, batch.to_bytes().unwrap(), &markets);
                if !self.run_step(rng, st, tag, PostJournal, ix, &mut steps) {
                    break;
                }
            }
            let Some(t) = self.tgt_of(&key) else { break };
            if !self.run_step(rng, st, tag, RequestAdvance, self.ix_request(&t), &mut steps) {
                break;
            }
            self.advance(rng.below(30) as i64);
            if !self.run_step(rng, st, tag, ConfirmPass, self.ix_confirm(&t), &mut steps) {
                break;
            }
            let Some(t) = self.tgt_of(&key) else { break };
            if t.a.as_ref().is_some_and(|a| a.pass_pending == 2) {
                if rng.chance(8) {
                    // a veto of the confirmed pass, then the cycle restarts from the profit run
                    self.run_step(rng, st, tag, VetoPass, self.ix_veto(&t), &mut steps);
                    continue;
                }
                let delay = i64::from(self.snap.cfg.pass_delay_secs);
                self.advance(delay + rng.below(2) as i64);
                let payer = self.payer();
                if !self.run_step(rng, st, tag, FinalizePass, finalize_ix(self, &t, payer), &mut steps) {
                    break;
                }
            }
        }
        steps
    }

    /// Scripted: fail an account through `report_fail`, reset it, and (sometimes) close the failed one after the retention.
    fn script_fail_reset(&mut self, rng: &mut Rng, st: &mut Stats, tag: &str) -> u64 {
        let mut steps = 0u64;
        let Some(t) = self.pick(rng, |t| t.a.as_ref().is_some_and(|a| a.status == 0 && a.pass_pending != 2 && a.tight_on == 0)) else { return 0 };
        let key = t.acct;
        let (batch, markets, reason) = self.gen_fail_batch(rng, &t, false);
        let ix = self.ix_report_fail(&t, batch.to_bytes().unwrap(), reason, &markets);
        if !self.run_step(rng, st, tag, ReportFail, ix, &mut steps) {
            return steps;
        }
        let Some(t) = self.tgt_of(&key) else { return steps };
        if self.snap.cfg.paused == 0 && rng.chance(80) && self.run_step(rng, st, tag, ResetAccount, reset_ix(self, &t, t.cid), &mut steps) {
            self.advance(1 + rng.below(100) as i64);
        }
        if rng.chance(50) {
            if let Some(a) = self.tgt_of(&key).and_then(|t| t.a) {
                self.set_clock_to(a.pass_proposed_at + c::TERMINAL_RETENTION_SECS + rng.below(2) as i64);
                let t = self.tgt_of(&key).unwrap();
                self.run_step(rng, st, tag, CloseAccount, self.ix_close(&t), &mut steps);
            }
        }
        steps
    }
}

/// A small pre-initialisation world: `init_config` succeeds here (in the full worlds it already ran), then repeats, wrong
/// signers and garbage; the whole-world digest and the config invariants are checked.
fn bare_epoch(rng: &mut Rng, st: &mut Stats, tag: &str, n: u64) {
    let mut e = Env::new_with(false);
    let mut dmap = world_map(&e.svm, &e.payer.pubkey());
    let mut inited = false;
    for i in 0..n {
        let args = {
            let mut a = default_args();
            if rng.chance(30) {
                a.max_payout_per_window = rng.below(3);
                a.window_secs = rng.below(3) as u32;
                a.pass_delay_secs = *rng.pick(&[0, c::MIN_PASS_DELAY_SECS, c::MAX_PASS_DELAY_SECS, c::MAX_PASS_DELAY_SECS + 1]);
            }
            a
        };
        let (kind, ix) = match rng.below(6) {
            0..=2 => (Kind::Good, init_ix_with(e.payer.pubkey(), e.sl8.pubkey(), e.rov.pubkey(), &args)),
            3 => {
                // one admin only / a stranger
                let mut ix = init_ix_with(e.payer.pubkey(), e.sl8.pubkey(), e.rov.pubkey(), &args);
                let k = rng.idx(ix.accounts.len());
                ix.accounts[k].is_signer = false;
                (Kind::Near, ix)
            }
            4 => {
                let mut ix = init_ix_with(e.payer.pubkey(), Pubkey::new_unique(), e.rov.pubkey(), &args);
                ix.accounts[1].is_signer = false;
                (Kind::Permuted, ix)
            }
            _ => {
                let mut ix = init_ix_with(e.payer.pubkey(), e.sl8.pubkey(), e.rov.pubkey(), &args);
                for _ in 0..rng.range(1, 3) {
                    mutate_data(rng, &mut ix.data);
                }
                (Kind::Garbage, ix)
            }
        };
        // flags are only legal on keys held (payer and the two test admins)
        let held = [e.payer.pubkey(), e.sl8.pubkey(), e.rov.pubkey()];
        let mut ix = ix;
        for m in ix.accounts.iter_mut() {
            if m.is_signer && !held.contains(&m.pubkey) {
                m.is_signer = false;
            }
        }
        let kind_i = KINDS.iter().position(|k| *k == kind).unwrap();
        st.tries[InitConfig as usize] += 1;
        st.kind_tries[kind_i] += 1;
        st.iters += 1;
        let signers: BTreeSet<Pubkey> = ix.accounts.iter().filter(|m| m.is_signer).map(|m| m.pubkey).collect();
        let data_dbg = ix.data.clone();
        let ix_dbg = ix.clone();
        let payer_before = e.svm.get_balance(&e.payer.pubkey()).unwrap_or(0);
        let res = e.send(&[ix], &[]);
        let after = world_map(&e.svm, &e.payer.pubkey());
        match res {
            Err(f) => {
                let label = err_label(&f);
                *st.hist.entry(format!("init_config(bare) -> {label}")).or_default() += 1;
                if let Some((ours, line)) = panic_of(&f, &PROGRAM_ID) {
                    if ours && is_known_jrn1(&ix_dbg, &f) {
                        st.known_jrn1 += 1;
                    } else if ours {
                        st.violations.push(format!("[{tag} bare #{i}] VIOLATION PANIC: {line}\n      data({})={}\n      logs: {}", data_dbg.len(), hexs(&data_dbg[..data_dbg.len().min(120)]), f.meta.logs.join(" | ")));
                    }
                }
                let paid = payer_before.saturating_sub(e.svm.get_balance(&e.payer.pubkey()).unwrap_or(0));
                if paid != 5_000 * e.last_tx.signatures as u64 {
                    st.violations.push(format!("[{tag} bare #{i}] VIOLATION FEE: a failing transaction moved the fee payer's lamports by {paid}"));
                }
                if after != dmap {
                    st.violations.push(format!("[{tag} bare #{i}] VIOLATION ATOMICITY: a failing init_config changed the world: {:?} (payer {}, sl8 {}, rov {})", map_diff(&dmap, &after), e.payer.pubkey(), e.sl8.pubkey(), e.rov.pubkey()));
                }
            }
            Ok(_) => {
                st.ok[InitConfig as usize] += 1;
                st.kind_ok[kind_i] += 1;
                *st.hist.entry("init_config(bare) -> OK".to_string()).or_default() += 1;
                st.invariant_runs += 1;
                if inited {
                    st.violations.push(format!("[{tag} bare #{i}] VIOLATION: init_config succeeded a second time"));
                }
                inited = true;
                if !(signers.contains(&c::SL8_ADMIN) && signers.contains(&c::ROV_ADMIN)) {
                    st.violations.push(format!("[{tag} bare #{i}] VIOLATION A-INIT: init_config succeeded without both constant admins signing"));
                }
                let cfg = e.config();
                let clock = e.svm.get_sysvar::<anchor_lang::solana_program::clock::Clock>().unix_timestamp;
                for (id, msg) in check_config(&cfg, clock, &default_args().vault_program) {
                    st.violations.push(format!("[{tag} bare #{i}] VIOLATION {id}: {msg}"));
                }
            }
        }
        dmap = after;
    }
}

fn run_worker(widx: u64, seed: u64, iters: u64, epoch_len: u64) -> Stats {
    let tag = format!("w{widx} seed {seed:#x}");
    let mut rng = Rng(seed);
    let mut st = Stats::default();
    let t0 = Instant::now();
    bare_epoch(&mut rng, &mut st, &tag, 40.min(iters / 4 + 1));
    let template = World::build(&mut rng);
    let build_ms = t0.elapsed().as_millis();
    let weights = op_weights();
    let kind_w = [45u64, 20, 20, 10, 5];
    let mut done = st.iters;
    let mut epoch = 0u64;
    while done < iters {
        epoch += 1;
        let mut w = template.fork();
        let n = epoch_len.min(iters - done);
        let start = done;
        while done - start < n && done < iters {
            let k = done - start;
            let it_tag = format!("w{widx} epoch {epoch} it {k} (seed {seed:#x})");
            if rng.below(1000) == 0 {
                w.inject(&mut rng);
            }
            if rng.below(150) == 0 {
                if let Some(call) = w.ext_call(&mut rng) {
                    st.iters += 1;
                    done += 1;
                    w.exec(call, &mut st, &it_tag);
                    continue;
                }
            }
            if rng.below(40) == 0 {
                let used = if rng.chance(25) { w.script_fail_reset(&mut rng, &mut st, &it_tag) } else { { let cyc = 1 + rng.below(5) as u32; w.script_cycles(&mut rng, &mut st, &it_tag, cyc) } };
                done += used;
                if used > 0 {
                    continue;
                }
            }
            st.iters += 1;
            done += 1;
            if rng.below(100) < 4 {
                // one transaction with 2 or 3 instructions (or the same instruction twice)
                let n = 2 + rng.idx(2);
                let mut calls: Vec<Call> = vec![];
                for _ in 0..n {
                    let kind = *rng.pick(&[Kind::Good, Kind::Good, Kind::Near, Kind::Permuted]);
                    let op = if rng.chance(50) { w.progress_op(&mut rng).unwrap_or_else(|| OPS[rng.weighted(&weights)]) } else { OPS[rng.weighted(&weights)] };
                    calls.push(w.make_call(&mut rng, kind, op));
                }
                if rng.chance(20) {
                    calls[1] = calls[0].clone();
                }
                w.exec_many(calls, &mut st, &it_tag);
                continue;
            }
            let kind = KINDS[rng.weighted(&kind_w)];
            let op = if rng.chance(40) { w.progress_op(&mut rng).unwrap_or_else(|| OPS[rng.weighted(&weights)]) } else { OPS[rng.weighted(&weights)] };
            let call = w.make_call(&mut rng, kind, op);
            w.exec(call, &mut st, &it_tag);
        }
    }
    // a checksum of everything the worker did (iterations, successes per instruction, the error histogram): a replay must print the same
    let mut h = Sha256::new();
    h.update(st.iters.to_le_bytes());
    for i in 0..25 {
        h.update(st.tries[i].to_le_bytes());
        h.update(st.ok[i].to_le_bytes());
    }
    for (k, n) in &st.hist {
        h.update(k.as_bytes());
        h.update(n.to_le_bytes());
    }
    let sum = hexs(&h.finalize()[..6]);
    println!("[fuzz w{widx}] seed {seed:#x}: {} iterations in {} epochs, setup {build_ms} ms, total {} ms, checksum {sum}", st.iters, epoch, t0.elapsed().as_millis());
    st
}

fn report_run(seed: u64, threads: u64, iters: u64, st: &Stats, secs: f64) {
    println!("\n==================== FUZZ REPORT ====================");
    println!("master seed {seed:#x}  iterations {} (asked {iters})  threads {threads}  wall {secs:.1}s  ({:.0} it/s)", st.iters, st.iters as f64 / secs.max(0.001));
    println!("sends {}  avg send {:.0} us  avg post-success checks {:.0} us", st.sends, st.send_nanos as f64 / st.sends.max(1) as f64 / 1000.0, st.other_nanos as f64 / (st.kind_ok.iter().sum::<u64>().max(1)) as f64 / 1000.0);
    let total_ok: u64 = st.ok.iter().sum::<u64>() + st.ext_ok;
    println!("successes {total_ok} / {} = {:.1}%   (ext vault mark_abandoned: {} of {})", st.sends, 100.0 * total_ok as f64 / st.sends.max(1) as f64, st.ext_ok, st.ext_tries);
    println!("multi-instruction transactions: {} sent, {} succeeded", st.multi_tries, st.multi_ok);
    println!("invariant evaluations (check_static + check_transition + auth on a success) {}   hash-chain recomputations {}", st.invariant_runs, st.chain_checks);
    println!("by kind:");
    for (i, name) in KIND_NAMES.iter().enumerate() {
        println!("  {:<13} tries {:>7}  ok {:>6}  ({:.1}%)", name, st.kind_tries[i], st.kind_ok[i], 100.0 * st.kind_ok[i] as f64 / st.kind_tries[i].max(1) as f64);
    }
    println!("by instruction (tries / successes):");
    for (i, op) in OPS.iter().enumerate() {
        println!("  {:<18} {:>7} / {:>6}   max CU {:>6}{}", op.name(), st.tries[i], st.ok[i], st.cu_max[i], if st.ok[i] == 0 { "   <-- NEVER SUCCEEDED" } else { "" });
    }
    println!("error histogram (instruction -> result: count):");
    let mut h: Vec<(&String, &u64)> = st.hist.iter().collect();
    h.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (k, n) in h {
        println!("  {n:>7}  {k}");
    }
    // which program errors the run produced (the variant names come from the program's own errors.rs, read as text)
    let src = include_str!("../../programs/setl8-lev-trading/src/errors.rs");
    let body = src.split("pub enum Setl8Error {").nth(1).and_then(|b| b.split("\n}").next()).unwrap_or("");
    let variants: Vec<&str> = body.lines().filter_map(|l| l.strip_prefix("    ").and_then(|l| l.strip_suffix(',')).filter(|n| n.chars().all(|ch| ch.is_alphanumeric()) && !n.is_empty())).collect();
    let seen: BTreeSet<&str> = st.hist.keys().filter_map(|k| k.rsplit(" -> ").next()).collect();
    let missing: Vec<&&str> = variants.iter().filter(|v| !seen.contains(**v)).collect();
    println!("Setl8Error variants: {} total, {} produced by the run, never produced: {:?}", variants.len(), variants.len() - missing.len(), missing);
    println!("journal events accepted on chain (EventLogs of successful post_journal / report_fail):");
    for (k, n) in &st.ev_hist {
        println!("  {k:<12} {n}");
    }
    println!("state coverage ({} distinct (status, phase, pass_pending, tight, latch, held, flat, locked, q>0) combinations seen on successes):", st.states.len());
    for (k, n) in &st.cov {
        println!("  {n:>8}  {k}");
    }
    println!("compute-budget errors: {}", st.budget_errs);
    println!("panics inside the VAULT reached through our program: {}", st.vault_panics.len());
    for p in st.vault_panics.iter().take(5) {
        println!("  {p}");
    }
    println!("known FINDING JRN-1 aborts (outer length prefix beyond the heap, not counted as violations): {}", st.known_jrn1);
    println!("violations by id / instruction (total {}):", st.viol_count.values().sum::<u64>());
    for (k, n) in &st.viol_count {
        println!("  {n:>6}  {k}");
    }
    println!("first examples (replay one worker: FUZZ_SEED=<the seed in the brackets> FUZZ_THREADS=1 FUZZ_ITERS=<iterations of that worker>):");
    for v in st.violations.iter().take(30) {
        println!("{v}");
    }
    println!("=====================================================");
}

#[test]
fn fuzz_loop_50k_iterations_fixed_seed() {
    let seed = env_u64("FUZZ_SEED", DEFAULT_SEED);
    let iters = env_u64("FUZZ_ITERS", DEFAULT_ITERS);
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u64);
    let threads = env_u64("FUZZ_THREADS", cores.min(10)).max(1);
    let epoch = env_u64("FUZZ_EPOCH", DEFAULT_EPOCH).max(10);
    println!("FUZZ seed {seed:#x} iterations {iters} threads {threads} epoch {epoch} (set FUZZ_SEED / FUZZ_ITERS / FUZZ_THREADS / FUZZ_EPOCH to change)");
    let t0 = Instant::now();
    let mut total = Stats::default();
    std::thread::scope(|sc| {
        let mut handles = vec![];
        for wi in 0..threads {
            let n = iters / threads + if wi == threads - 1 { iters % threads } else { 0 };
            let ws = seed.wrapping_add(wi.wrapping_mul(0x9E37_79B9));
            handles.push(std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(sc, move || run_worker(wi, ws, n, epoch)).unwrap());
        }
        for h in handles {
            total.merge(h.join().expect("fuzz worker panicked (a harness bug or an abort inside the SVM)"));
        }
    });
    report_run(seed, threads, iters, &total, t0.elapsed().as_secs_f64());
    assert!(total.violations.is_empty(), "{} violations, see the report above (seed {seed:#x})", total.violations.len());
    if iters >= 20_000 {
        let missing: Vec<&str> = OPS.iter().filter(|o| total.ok[**o as usize] == 0).map(|o| o.name()).collect();
        assert!(missing.is_empty(), "instructions that never succeeded in the run: {missing:?}");
    }
}

// ====================================================================================================== controls: does the checker bite?
//
// A fuzz loop that reports "0 violations" proves nothing unless the checker is seen to fail on a corrupted world. These tests
// corrupt a valid snapshot one field at a time and demand the right invariant id, then feed the transition checker changes it must
// refuse, the authorisation oracle calls without the signers DESIGN asks for, and the panic classifier synthetic aborts.

fn fresh() -> (World, Snap, Pubkey) {
    let mut rng = Rng(7);
    let w = World::build(&mut rng);
    let key = acct_pda(&w.traders[0].pubkey(), 0);
    let s = w.snap.clone();
    assert!(s.accts.contains_key(&key));
    (w, s, key)
}

fn has(v: &[Viol], id: &str) -> bool {
    v.iter().any(|x| x.0 == id)
}

fn ids(v: &[Viol]) -> Vec<&'static str> {
    v.iter().map(|x| x.0).collect()
}

#[test]
fn control_the_untouched_world_satisfies_every_static_invariant() {
    let (_, s, _) = fresh();
    let v = check_static(&s, &BTreeSet::new());
    assert!(v.is_empty(), "a freshly built world must be clean, got {v:?}");
    assert!(s.accts.len() >= 6 && s.wallets.len() >= 4 && s.vts.len() == s.accts.len());
}

#[test]
fn control_every_static_invariant_fires_on_its_corruption() {
    let (w, s, k) = fresh();
    let t0 = w.traders[0].pubkey();
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Box<dyn Fn(&mut Snap)>)> = vec![
        ("T-1", Box::new(move |s| s.accts.get_mut(&k).unwrap().flat = 7)),
        ("T-2", Box::new(move |s| s.accts.get_mut(&k).unwrap().phase = 6)),
        ("T-3a", Box::new(move |s| s.accts.get_mut(&k).unwrap().pass_pending = 3)),
        ("T-3b", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.status = 1;
            a.pass_pending = 1;
        })),
        ("T-3c", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.status = 1;
            a.held = 1;
        })),
        ("T-4", Box::new(move |s| s.accts.get_mut(&k).unwrap().size_s = 123_000_000)),
        ("T-4b", Box::new(move |s| s.accts.get_mut(&k).unwrap().ruleset_version = 9)),
        ("T-5", Box::new(move |s| s.accts.get_mut(&k).unwrap().b = 105_000_000_000)),
        ("T-6a", Box::new(move |s| s.accts.get_mut(&k).unwrap().q_tally = -1)),
        ("T-6b", Box::new(move |s| s.accts.get_mut(&k).unwrap().q_tally = 2_800_000_001)),
        ("T-7", Box::new(move |s| s.accts.get_mut(&k).unwrap().b = 92_000_000_000)),
        ("T-8a", Box::new(move |s| s.accts.get_mut(&k).unwrap().r = 5)),
        ("T-8b", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            (a.tight_on, a.flat, a.r, a.l_level) = (1, 1, 100_000_000_000, 96_000_000_000);
        })),
        ("T-8c", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            (a.tight_on, a.flat, a.r) = (1, 0, 0);
        })),
        ("T-8d", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            (a.tight_on, a.flat, a.r, a.g_peak, a.l_level) = (1, 0, 100_000_000_000, 0, 93_000_000_000);
        })),
        ("T-9", Box::new(move |s| s.accts.get_mut(&k).unwrap().lock_until = -5)),
        ("T-10", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.last_batch_time = a.created_at - 1;
        })),
        ("T-11a", Box::new(move |s| s.accts.get_mut(&k).unwrap().journal_hash = [9; 32])),
        ("T-11b", Box::new(move |s| s.accts.get_mut(&k).unwrap().trades_closed = 1)),
        ("T-11c", Box::new(move |s| s.accts.get_mut(&k).unwrap().seq = 4)),
        ("T-12a", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.pass_pending = 1;
            a.pass_requested_at = a.created_at - 5;
        })),
        ("T-12b", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.pass_pending = 2;
            a.pass_requested_at = a.created_at + 50;
            a.pass_proposed_at = a.created_at + 10;
        })),
        ("T-12c", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.status = 2;
            a.pass_proposed_at = a.created_at - 1;
        })),
        ("T-12d", Box::new(move |s| {
            let a = s.accts.get_mut(&k).unwrap();
            a.status = 2;
            a.pass_proposed_at = a.created_at;
            a.pass_requested_at = 5;
        })),
        ("T-13", Box::new(move |s| s.accts.get_mut(&k).unwrap().next_request_id = 9)),
        ("V-0", Box::new(move |s| {
            s.vts.remove(&k);
        })),
        ("V-1", Box::new(move |s| s.vts.get_mut(&k).unwrap().0 = 3)),
        ("V-2", Box::new(move |s| s.vts.get_mut(&k).unwrap().2 = 5)),
        ("V-3", Box::new(move |s| s.accts.get_mut(&k).unwrap().status = 1)),
        ("W-1", Box::new(move |s| s.wallets.get_mut(&t0).unwrap().banned = 2)),
        ("W-2", Box::new(move |s| s.wallets.get_mut(&t0).unwrap().active_count = 4)),
        ("W-3", Box::new(move |s| s.wallets.get_mut(&t0).unwrap().held_count = 2)),
        ("W-4", Box::new(move |s| s.wallets.get_mut(&t0).unwrap().next_challenge_id = 0)),
        ("C-1", Box::new(|s| s.cfg.pass_delay_secs = 5)),
        ("C-2", Box::new(|s| s.cfg.window_secs = 0)),
        ("C-3", Box::new(|s| s.cfg.paused = 2)),
        ("C-4", Box::new(|s| s.cfg.market_count = 200)),
        ("C-5", Box::new(|s| s.cfg.admin_a = Pubkey::new_unique())),
        ("C-6", Box::new(|s| s.cfg.vault_program = Pubkey::default())),
        ("C-7", Box::new(|s| s.cfg.reserved[3] = 1)),
        ("C-8", Box::new(|s| s.cfg.window_start = s.clock + 100)),
    ];
    for (id, corrupt) in &cases {
        let mut bad = s.clone();
        corrupt(&mut bad);
        let v = check_static(&bad, &BTreeSet::new());
        assert!(has(&v, id), "corruption for {id} was not reported, got {:?}", ids(&v));
        // and the same corruption on a tainted trader is skipped for account-level ids (the fuzzer's own garbage injection)
        if id.starts_with('T') {
            let tainted: BTreeSet<Pubkey> = [t0].into_iter().collect();
            assert!(!has(&check_static(&bad, &tainted), id), "{id} must be skipped for a tainted trader");
        }
    }
    assert_eq!(cases.len(), 41, "the table of static invariants changed: update the fragment");
}

#[test]
fn control_every_transition_invariant_fires_on_its_violation() {
    let (w, s, k) = fresh();
    let t0 = w.traders[0].pubkey();
    let none = BTreeSet::new();
    let mut base = s.clone();
    base.tally = (3, 24_000_000_000);
    base.cfg.window_total = 10;
    {
        let a = base.accts.get_mut(&k).unwrap();
        a.seq = 5;
        a.trades_closed = 3;
        a.journal_hash = [1; 32];
    }
    // (id, op, events, mutation of the "after" snapshot)
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Op, Option<usize>, Box<dyn Fn(&mut Snap)>)> = vec![
        ("X-C1", SetLimits, None, Box::new(|n| n.cfg.evaluator = Pubkey::new_unique())),
        ("X-C1", SetPaused, None, Box::new(|n| n.cfg.market_count += 1)),
        ("X-C1", CreateRuleset, None, Box::new(|n| n.cfg.current_ruleset += 2)),
        ("X-C2", FinalizePass, None, Box::new(|n| n.cfg.window_total = 5)),
        ("X-T1", PostJournal, None, Box::new(|n| n.tally.0 -= 1)),
        ("X-T2", PostJournal, None, Box::new(|n| n.tally.1 += 5)),
        ("X-T3", FinalizePass, None, Box::new(move |n| {
            n.tally = (n.tally.0 + 1, n.tally.1 + 12_345);
            n.accts.get_mut(&k).unwrap().next_request_id += 1;
        })),
        ("X-A1", SetPaused, None, Box::new(move |n| {
            n.accts.remove(&k);
        })),
        ("X-A2", CloseAccount, None, Box::new(move |n| {
            n.accts.remove(&k);
        })),
        ("X-A3", SetPaused, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().b += 1)),
        ("X-A3", PostJournal, Some(0), Box::new(move |n| n.accts.get_mut(&k).unwrap().last_activity_pushed += 1)),
        ("X-A4", PostJournal, None, Box::new(move |n| {
            let a = n.accts.get_mut(&k).unwrap();
            a.trades_closed = 0;
            a.seq = 0;
        })),
        ("X-A5", PostJournal, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().journal_hash = [4; 32])),
        ("X-A6", ConfirmPass, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().phase += 2)),
        ("X-A8", PostJournal, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().lock_until = -1)),
        ("X-A10", PostJournal, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().vetoes += 1)),
        ("X-A11", PostJournal, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().next_request_id += 1)),
        ("X-A13", PostJournal, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().held = 1)),
        ("X-A14", ConfirmPass, None, Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 1)),
        ("X-A16", SetPaused, None, Box::new(move |n| {
            n.accts.remove(&k);
        })),
        ("X-A17", PostJournal, None, Box::new(move |n| {
            let a = n.accts[&k].clone();
            n.accts.insert(Pubkey::new_unique(), a);
        })),
        ("X-A18", OpenAccount, None, Box::new(move |n| {
            let a = n.accts[&k].clone();
            n.accts.insert(Pubkey::new_unique(), a);
        })),
        ("X-J1", PostJournal, Some(2), Box::new(|_| {})),
        ("X-J2", PostJournal, Some(3), Box::new(move |n| n.accts.get_mut(&k).unwrap().seq += 1)),
        ("X-W1", PostJournal, None, Box::new(move |n| n.wallets_raw.get_mut(&t0).unwrap()[0] ^= 1)),
        ("X-W2", OpenAccount, None, Box::new(move |n| n.wallets.get_mut(&t0).unwrap().next_challenge_id += 2)),
        ("X-W3", PostJournal, None, Box::new(move |n| n.wallets.get_mut(&t0).unwrap().banned = 1)),
        ("X-V1", PostJournal, None, Box::new(move |n| n.vts.get_mut(&k).unwrap().0 += 1)),
    ];
    for (id, op, events, change) in &cases {
        let mut after = base.clone();
        change(&mut after);
        // X-A2 needs an Active account closed; X-A7 below needs a terminal one
        let v = check_transition(&base, &after, Some(*op), &none, *events);
        assert!(has(&v, id), "{id} under {op:?}: the change was not reported, got {:?}", ids(&v));
    }
    // X-A7: a terminal status cannot change; X-A9: q_tally and latch do not fall inside one phase
    let mut term = base.clone();
    term.accts.get_mut(&k).unwrap().status = 1;
    let mut back = term.clone();
    back.accts.get_mut(&k).unwrap().status = 0;
    assert!(has(&check_transition(&term, &back, Some(ReportFail), &none, None), "X-A7"));
    let mut hi = base.clone();
    hi.accts.get_mut(&k).unwrap().q_tally = 100;
    let mut lo = hi.clone();
    lo.accts.get_mut(&k).unwrap().q_tally = 50;
    assert!(has(&check_transition(&hi, &lo, Some(PostJournal), &none, None), "X-A9"));
    // X-V2: the vault status only moves forward (Active 0 -> Graduated 1 / Failed 2 / Abandoned 3)
    let mut failed_v = base.clone();
    failed_v.vts.get_mut(&k).unwrap().1 = 2;
    let mut back_v = failed_v.clone();
    back_v.vts.get_mut(&k).unwrap().1 = 0;
    assert!(has(&check_transition(&failed_v, &back_v, Some(PostJournal), &none, None), "X-V2"));
    assert!(!has(&check_transition(&base, &failed_v, Some(PostJournal), &none, None), "X-V2"), "0 -> 2 is a legal forward move");
    // the identity (nothing changed) is clean for every op, and an honest change is accepted
    for op in OPS {
        let v = check_transition(&base, &base.clone(), Some(op), &none, None);
        if matches!(op, OpenAccount | ResetAccount | CloseAccount) {
            // a success of these must create / remove exactly one account
            assert!(has(&v, "X-A17") || has(&v, "X-A16"), "{op:?}: {v:?}");
        } else {
            assert!(v.is_empty(), "{op:?}: {v:?}");
        }
    }
    let mut honest = base.clone();
    {
        let a = honest.accts.get_mut(&k).unwrap();
        a.b += 5;
        a.seq += 2;
        a.journal_hash = [5; 32];
        a.last_batch_time += 1;
        a.trades_closed += 1;
    }
    let v = check_transition(&base, &honest, Some(PostJournal), &none, Some(2));
    assert!(v.is_empty(), "an honest journal step must pass: {v:?}");
    assert_eq!(cases.len(), 28, "the table of transition checks changed: update the fragment");
}

#[test]
fn control_the_authorisation_oracle_demands_the_signers_of_the_design() {
    let (w, s, k) = fresh();
    let cfg = &s.cfg;
    let t0 = w.traders[0].pubkey();
    let signer = |p: Pubkey| AccountMeta::new_readonly(p, true);
    let plain = |p: Pubkey| AccountMeta::new_readonly(p, false);
    let ix = |op: Op, accounts: Vec<AccountMeta>, extra: &[u8]| Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: [op.disc().to_vec(), extra.to_vec()].concat(),
    };
    // evaluator ops
    for op in [PostJournal, ReportFail, ConfirmPass, RejectPass, HoldAccount, TouchActivity] {
        assert!(has(&check_auth(op, &ix(op, vec![plain(cfg.evaluator), plain(k)], &[]), &s), "A-EV"), "{op:?} without evaluator");
        assert!(has(&check_auth(op, &ix(op, vec![signer(w.attackers[0].pubkey()), plain(k)], &[]), &s), "A-EV"), "{op:?} stranger");
        assert!(check_auth(op, &ix(op, vec![signer(cfg.evaluator), plain(k)], &[]), &s).is_empty(), "{op:?}");
    }
    // both-admin ops
    for op in [SetEvaluator, SetLimits, CreateRuleset, BanWallet, UnbanWallet, VetoPass, ReleaseHold, FailForViolation] {
        for one in [vec![signer(cfg.admin_a), plain(cfg.admin_b)], vec![plain(cfg.admin_a), signer(cfg.admin_b)], vec![plain(cfg.admin_a)]] {
            assert!(has(&check_auth(op, &ix(op, one, &[]), &s), "A-BOTH"), "{op:?} with one admin");
        }
        assert!(check_auth(op, &ix(op, vec![signer(cfg.admin_a), signer(cfg.admin_b)], &[]), &s).is_empty(), "{op:?}");
    }
    // pause needs one admin, unpause both
    assert!(has(&check_auth(SetPaused, &ix(SetPaused, vec![plain(cfg.admin_a)], &[1]), &s), "A-ANY"));
    assert!(check_auth(SetPaused, &ix(SetPaused, vec![signer(cfg.admin_b)], &[1]), &s).is_empty());
    assert!(has(&check_auth(SetPaused, &ix(SetPaused, vec![signer(cfg.admin_b)], &[0]), &s), "A-BOTH"));
    // either-admin ops
    for op in [CreateMarket, UpdateMarket] {
        assert!(has(&check_auth(op, &ix(op, vec![plain(cfg.admin_a), plain(cfg.admin_b)], &[]), &s), "A-ANY"));
        assert!(check_auth(op, &ix(op, vec![signer(cfg.admin_a)], &[]), &s).is_empty());
    }
    assert!(has(&check_auth(InitConfig, &ix(InitConfig, vec![signer(c::SL8_ADMIN)], &[]), &s), "A-INIT"));
    // trader ops: the trader of the account in slot 1 must sign
    for op in [RequestAdvance, CancelPass, ReleaseAccount, CloseAccount] {
        assert!(has(&check_auth(op, &ix(op, vec![plain(t0), plain(k)], &[]), &s), "A-TR"));
        assert!(has(&check_auth(op, &ix(op, vec![signer(w.traders[1].pubkey()), plain(k)], &[]), &s), "A-TR"), "{op:?}: another trader signs");
        assert!(check_auth(op, &ix(op, vec![signer(t0), plain(k)], &[]), &s).is_empty());
    }
    for op in [OpenAccount, ResetAccount] {
        assert!(has(&check_auth(op, &ix(op, vec![plain(t0)], &[]), &s), "A-TR"));
        assert!(check_auth(op, &ix(op, vec![signer(t0)], &[]), &s).is_empty());
    }
    assert!(check_auth(FinalizePass, &ix(FinalizePass, vec![], &[]), &s).is_empty(), "finalize_pass is permissionless");
}

#[test]
fn control_the_panic_classifier_and_the_known_finding_filter() {
    let failed = |logs: &[&str], e: InstructionError| FailedTransactionMetadata {
        err: TransactionError::InstructionError(0, e),
        meta: TransactionMetadata { logs: logs.iter().map(|l| l.to_string()).collect(), ..Default::default() },
    };
    let us = PROGRAM_ID.to_string();
    let vault = VAULT_ID.to_string();
    // a panic in our program is "ours"
    let f = failed(&[&format!("Program {us} invoke [1]"), "Program log: panicked at src/x.rs:1:1:\nboom", &format!("Program {us} failed: SBF program panicked")], InstructionError::ProgramFailedToComplete);
    assert!(panic_of(&f, &PROGRAM_ID).is_some_and(|(ours, _)| ours));
    // a panic in the vault reached through a CPI is not ours
    let f = failed(
        &[&format!("Program {us} invoke [1]"), &format!("Program {vault} invoke [2]"), "Program log: panicked at vault.rs:1:1:\nboom", &format!("Program {vault} failed: SBF program panicked"), &format!("Program {us} failed: SBF program panicked")],
        InstructionError::ProgramFailedToComplete,
    );
    assert!(panic_of(&f, &PROGRAM_ID).is_some_and(|(ours, _)| !ours));
    // BorshIoError and ProgramFailedToComplete are aborts even without a log line
    assert!(panic_of(&failed(&[], InstructionError::BorshIoError("x".into())), &PROGRAM_ID).is_some());
    assert!(panic_of(&failed(&[], InstructionError::ProgramFailedToComplete), &PROGRAM_ID).is_some());
    // a named program error, an Anchor error and a budget error are not panics
    assert!(panic_of(&failed(&["Program log: AnchorError thrown. Error Code: Unauthorized."], InstructionError::Custom(6000)), &PROGRAM_ID).is_none());
    assert!(panic_of(&failed(&[], InstructionError::ComputationalBudgetExceeded), &PROGRAM_ID).is_none());
    assert_eq!(err_label(&failed(&["Program log: AnchorError thrown in x.rs:3. Error Code: MustReportFail. Error Number: 6089."], InstructionError::Custom(6089))), "MustReportFail");
    // the known finding JRN-1 is recognised only for post_journal / report_fail with a big length prefix AND the allocation log
    let alloc = failed(&["Program log: Error: memory allocation failed, out of memory"], InstructionError::ProgramFailedToComplete);
    let mk = |op: Op, prefix: u32| Instruction { program_id: PROGRAM_ID, accounts: vec![], data: [op.disc().to_vec(), prefix.to_le_bytes().to_vec(), vec![1, 2, 3]].concat() };
    assert!(is_known_jrn1(&mk(PostJournal, 65_536), &alloc));
    assert!(is_known_jrn1(&mk(ReportFail, u32::MAX), &alloc));
    assert!(!is_known_jrn1(&mk(PostJournal, 100), &alloc), "a small prefix is not JRN-1");
    assert!(!is_known_jrn1(&mk(SetLimits, 65_536), &alloc), "another instruction aborting is NOT the known finding");
    assert!(!is_known_jrn1(&mk(PostJournal, 65_536), &failed(&["Program log: panicked at x"], InstructionError::ProgramFailedToComplete)));
}

#[test]
fn control_the_world_map_sees_data_lamports_and_owner_but_not_the_fee_payer_balance_or_sysvars() {
    let (mut w, _, k) = fresh();
    let payer = w.payer();
    let base = world_map(&w.j.v.e.svm, &payer);
    // a byte of a trading account
    let mut acc = w.j.v.e.svm.get_account(&k).unwrap();
    acc.data[100] ^= 1;
    w.j.v.e.svm.set_account(k, acc.clone()).unwrap();
    assert_eq!(map_diff(&base, &world_map(&w.j.v.e.svm, &payer)), vec![k]);
    acc.data[100] ^= 1;
    w.j.v.e.svm.set_account(k, acc.clone()).unwrap();
    assert!(base == world_map(&w.j.v.e.svm, &payer));
    // lamports and owner of the same account
    acc.lamports += 1;
    w.j.v.e.svm.set_account(k, acc.clone()).unwrap();
    assert_eq!(map_diff(&base, &world_map(&w.j.v.e.svm, &payer)), vec![k]);
    acc.lamports -= 1;
    acc.owner = system_program::ID;
    w.j.v.e.svm.set_account(k, acc).unwrap();
    assert_eq!(map_diff(&base, &world_map(&w.j.v.e.svm, &payer)), vec![k]);
    // the fee payer's lamports and the clock do not count
    let mut w = fresh().0;
    let base = world_map(&w.j.v.e.svm, &w.payer());
    let mut pacc = w.j.v.e.svm.get_account(&w.payer()).unwrap();
    pacc.lamports -= 15_000;
    w.j.v.e.svm.set_account(w.payer(), pacc).unwrap();
    w.j.v.e.set_clock(w.clock + 99);
    assert!(base == world_map(&w.j.v.e.svm, &w.payer()), "the fee payer's lamports and the clock must not count");
}

/// The wiring from `exec` to the checks: a change made behind the harness's back is reported by the next call, whether the call fails
/// (atomicity) or succeeds (transition invariants).
#[test]
fn control_exec_reports_a_world_that_changed_behind_its_back() {
    // 1. a FAILING call after an out-of-band write: the digest differs although the call changed nothing
    let (mut w, _, k) = fresh();
    let mut st = Stats::default();
    let mut acc = w.j.v.e.svm.get_account(&k).unwrap();
    acc.data[100] ^= 1;
    w.j.v.e.svm.set_account(k, acc).unwrap();
    let t = w.tg.iter().find(|t| t.acct == k).cloned().unwrap();
    // an unauthorised close_account: refused, nothing changes
    let bad = Call { op: Some(CloseAccount), ext: false, ix: w.ix_close(&Tgt { trader: w.traders[1].pubkey(), ..t.clone() }), kind: Kind::Good };
    let mut call = bad;
    call.ix.accounts[0].is_signer = false;
    w.exec(call, &mut st, "control");
    assert!(st.viol_count.keys().any(|k| k.starts_with("ATOMICITY")), "got {:?}", st.viol_count);
    // 2. a SUCCEEDING call after an out-of-band write to a field the call may not change
    let (mut w, _, k) = fresh();
    let mut st = Stats::default();
    let mut acc = w.j.v.e.svm.get_account(&k).unwrap();
    acc.data[64..72].copy_from_slice(&5i64.to_le_bytes()); // b
    w.j.v.e.svm.set_account(k, acc).unwrap();
    let t = w.tg.iter().find(|t| t.acct == k).cloned().unwrap();
    w.advance(c::ACTIVITY_THROTTLE_SECS + 5);
    let call = Call { op: Some(TouchActivity), ext: false, ix: w.ix_touch(&t), kind: Kind::Good };
    let mut call = call;
    w.fix_signers(&mut Rng(1), &mut call.ix, Kind::Good);
    w.exec(call, &mut st, "control");
    assert_eq!(st.ok[TouchActivity as usize], 1, "the touch itself succeeds: {:?}", st.hist);
    assert!(st.viol_count.keys().any(|k| k.starts_with("X-A3")), "the change of b behind the back must be seen: {:?}", st.viol_count);
}

#[test]
fn control_every_success_precondition_fires_when_the_pre_state_does_not_meet_it() {
    let (w, s, k) = fresh();
    let t0 = w.traders[0].pubkey();
    let none = BTreeSet::new();
    let mut base = s.clone();
    base.clock = 1_800_000_000;
    let ix_of = |op: Op, first: Pubkey| Instruction { program_id: PROGRAM_ID, accounts: vec![AccountMeta::new_readonly(first, true)], data: op.disc().to_vec() };
    let target = 108_000_000_000i64;
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Op, Box<dyn Fn(&mut Snap)>, Box<dyn Fn(&mut Snap)>)> = vec![
        ("P-request", RequestAdvance, Box::new(move |p| p.accts.get_mut(&k).unwrap().b = 100_000_000_000), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 1)),
        ("P-request", RequestAdvance, Box::new(|p| p.cfg.paused = 1), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 1)),
        ("P-confirm", ConfirmPass, Box::new(move |p| {
            let a = p.accts.get_mut(&k).unwrap();
            (a.b, a.pass_pending, a.held) = (target, 1, 1);
        }), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 2)),
        ("P-reject", RejectPass, Box::new(move |p| p.accts.get_mut(&k).unwrap().pass_pending = 2), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 0)),
        ("P-cancel", CancelPass, Box::new(move |p| {
            let a = p.accts.get_mut(&k).unwrap();
            (a.pass_pending, a.pass_requested_at) = (1, 1_800_000_000 - 10);
        }), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_pending = 0)),
        ("P-veto", VetoPass, Box::new(|_| {}), Box::new(move |n| n.accts.get_mut(&k).unwrap().vetoes = 1)),
        ("P-finalize", FinalizePass, Box::new(move |p| {
            let a = p.accts.get_mut(&k).unwrap();
            (a.b, a.pass_pending, a.pass_proposed_at) = (target, 2, 1_800_000_000 - 10);
        }), Box::new(move |n| {
            let a = n.accts.get_mut(&k).unwrap();
            (a.pass_pending, a.next_request_id) = (0, 2);
        })),
        ("P-finalize", FinalizePass, Box::new(move |p| {
            let a = p.accts.get_mut(&k).unwrap();
            (a.b, a.pass_pending, a.pass_proposed_at, a.phase) = (target, 2, 1_700_000_000, 2);
            (p.cfg.window_start, p.cfg.window_secs, p.cfg.window_total, p.cfg.max_payout_per_window) = (1_799_999_000, 86_400, 95_000_000_000, 100_000_000_000);
        }), Box::new(move |n| {
            let a = n.accts.get_mut(&k).unwrap();
            (a.pass_pending, a.next_request_id) = (0, 2);
        })),
        ("P-hold", HoldAccount, Box::new(move |p| p.accts.get_mut(&k).unwrap().status = 1), Box::new(move |n| n.accts.get_mut(&k).unwrap().held = 1)),
        ("P-violation", FailForViolation, Box::new(|_| {}), Box::new(move |n| {
            let a = n.accts.get_mut(&k).unwrap();
            (a.status, a.pass_pending) = (1, 3);
        })),
        ("P-touch", TouchActivity, Box::new(move |p| p.accts.get_mut(&k).unwrap().status = 2), Box::new(move |n| n.accts.get_mut(&k).unwrap().last_activity_pushed += 5)),
        ("P-reset", ResetAccount, Box::new(|_| {}), Box::new(move |n| n.accts.get_mut(&k).unwrap().pass_requested_at = 5)),
        ("P-post", PostJournal, Box::new(move |p| p.accts.get_mut(&k).unwrap().status = 1), Box::new(move |n| n.accts.get_mut(&k).unwrap().seq += 1)),
        ("P-post", PostJournal, Box::new(move |p| p.accts.get_mut(&k).unwrap().pass_pending = 2), Box::new(move |n| n.accts.get_mut(&k).unwrap().seq += 1)),
        ("P-open", OpenAccount, Box::new(move |p| p.wallets.get_mut(&t0).unwrap().banned = 1), Box::new(|_| {})),
        ("P-open", OpenAccount, Box::new(move |p| p.wallets.get_mut(&t0).unwrap().active_count = 5), Box::new(|_| {})),
        ("P-ban", BanWallet, Box::new(move |p| p.wallets.get_mut(&t0).unwrap().held_count = 1), Box::new(move |n| n.wallets.get_mut(&t0).unwrap().banned = 1)),
        ("P-create_ruleset", CreateRuleset, Box::new(|_| {}), Box::new(|n| n.cfg.current_ruleset += 2)),
        ("P-create_market", CreateMarket, Box::new(|_| {}), Box::new(|n| n.cfg.market_count += 3)),
    ];
    for (id, op, pre_mut, post_mut) in &cases {
        let mut pre = base.clone();
        pre_mut(&mut pre);
        let mut post = pre.clone();
        post_mut(&mut post);
        let v = check_pre(*op, &ix_of(*op, t0), &pre, &post, &none);
        assert!(has(&v, id), "{id}: the unmet precondition was not reported, got {:?}", ids(&v));
    }
    // an honest request passes
    let mut pre = base.clone();
    {
        let a = pre.accts.get_mut(&k).unwrap();
        (a.b, a.flat, a.tight_on, a.pass_pending, a.held) = (target, 1, 0, 0, 0);
    }
    let mut post = pre.clone();
    post.accts.get_mut(&k).unwrap().pass_pending = 1;
    assert!(check_pre(RequestAdvance, &ix_of(RequestAdvance, t0), &pre, &post, &none).is_empty());
    assert_eq!(cases.len(), 19, "the table of success preconditions changed: update the fragment");
}

// ====================================================================================================== LIMITED: vault max_payout_count drift

fn set_vault_max_payout(w: &mut JEnv, n: u64) {
    let cur = w.v.registry();
    let ix = si::update_product_config(
        VAULT_ID,
        VAULT_SL8_ADMIN,
        VAULT_ROV_ADMIN,
        registry_pda(),
        &[],
        si::UpdateProductConfigArgs {
            product_program_id: PROGRAM_ID,
            challenge_sizes: cur.challenge_sizes.iter().map(|(size, cost)| si::ChallengeSize { size: *size, cost: *cost }).collect(),
            fee_split_bps: cur.fee_split_bps,
            max_payout_count: n,
            reset_price_bps: cur.reset_price_bps,
        },
    );
    assert_ok(w.v.e.send(&[ix], &[]));
}

/// LIMITED (found by the fuzz loop as a V-3 mismatch, see the fragment): the program hard-codes "payouts in phases 2 to 5, then
/// Graduated" and never reads the vault's `max_payout_count`. With 5 the local account Graduates after phase 5 while the vault
/// record stays Active. Both vault admins set that number (DESIGN 10.4 registers 4); this pins what happens when they do not.
#[test]
fn limited_vault_max_payout_count_5_graduates_locally_while_the_vault_record_stays_active() {
    let mut w = JEnv::new();
    set_vault_max_payout(&mut w, 5);
    w.set_limits(86_400, 100_000_000_000, 86_400);
    let payer = w.new_payer();
    let mut at = w.t0 + 1_000;
    for phase in 2..=5u8 {
        w.ready(phase);
        w.set_pending(c::PASS_CONFIRMED, at, at);
        at += 86_400;
        w.ok(|w| w.finalize(at, &payer));
        at += 1;
    }
    let a = w.account();
    assert_eq!((a.status, a.phase, a.next_request_id), (c::STATUS_GRADUATED, 5, 5));
    let v = w.vault_status();
    assert_eq!((v.status, v.payout_count), (0, 4), "the vault record is still Active after 4 payouts when its max_payout_count is 5");
    assert_eq!(w.wallet_profile().active_count, 0, "the local wallet slot is freed");
}

/// LIMITED: with 1 the vault Graduates the record after the first payout; the local account stays Active, and the next finalize is
/// refused by the vault (the whole transaction reverts, the pass stays confirmed until both admins veto it).
#[test]
fn limited_vault_max_payout_count_1_strands_the_second_payout_with_a_vault_error() {
    let mut w = JEnv::new();
    set_vault_max_payout(&mut w, 1);
    w.set_limits(86_400, 100_000_000_000, 86_400);
    let payer = w.new_payer();
    let mut at = w.t0 + 1_000;
    w.ready(2);
    w.set_pending(c::PASS_CONFIRMED, at, at);
    at += 86_400;
    w.ok(|w| w.finalize(at, &payer));
    assert_eq!((w.account().phase, w.account().status), (3, c::STATUS_ACTIVE));
    assert_eq!(w.vault_status().status, 1, "the vault record is Graduated after the first payout");
    at += 10;
    w.ready(3);
    w.set_pending(c::PASS_CONFIRMED, at, at);
    at += 86_400;
    let before = w.pass_digest();
    let r = w.finalize(at, &payer);
    let f = r.as_ref().expect_err("the vault refuses the second payout");
    assert_eq!(f.err, TransactionError::InstructionError(0, InstructionError::Custom(6005)), "the vault's InvalidTraderStatus: {}", f.meta.logs.join("\n"));
    assert_eq!(w.pass_digest(), before, "the refusal reverts everything");
    assert_eq!((w.account().pass_pending, w.account().status, w.account().phase), (c::PASS_CONFIRMED, c::STATUS_ACTIVE, 3));
    // the way out is the admins' veto
    assert_ok(w.veto(at + 1));
    assert_eq!(w.account().pass_pending, 0);
}
