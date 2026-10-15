//! Public data types of the evaluator's output side: queue entries, batches, snapshots, pass handling.
use engine::{AccountState, FailReason, Position};
use journal_types::{JournalBatch, JournalEvent};

use crate::codec::{OrderIntent, SampleRec};

/// One open position and its protective levels (0 = none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub p: Position,
    pub sl: u64,
    pub tp: u64,
}

/// A live limit or stop order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub nonce: u32,
    pub intent: OrderIntent,
}

/// A recorded sample with the numbers derived from it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub rec: SampleRec,
    /// `floor(value / 10^6)`.
    pub mid: u64,
    pub bid: u64,
    pub ask: u64,
    /// Source second of the last CHANGE of this market's value, as of this sample (decision 49).
    pub last_change: i64,
}

impl Stored {
    pub fn sec(&self) -> i64 {
        self.rec.sec()
    }
}

/// The account as it stands after an event (what the program stores once the batch holding it is applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub st: AccountState,
    pub trades_closed: u32,
    /// Time of the event (the program's `last_batch_time` after the batch ends with it).
    pub last_time: i64,
    /// All positions closed after this event.
    pub flat: bool,
}

/// One queued journal event: the event with a placeholder proof index, the sample that justifies it, and the state after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QEvent {
    pub ev: JournalEvent,
    /// Index into the evaluator's sample list (the proof), if the event carries one.
    pub proof: Option<usize>,
    pub after: Snap,
}

/// Events that must stay in one batch (a flip's close and fill, the last close with its fee and the `Breach`, `TightEnd`
/// or `Fail` that follows it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub events: Vec<QEvent>,
}

/// Which instruction posts the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchKind {
    PostJournal,
    /// `report_fail(batch, reason)`: the batch ends `Fail, Meta`.
    ReportFail(FailReason),
}

/// The fields of `TradingAccount` the journal decides, as they must read after the batch is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub b: i64,
    pub q_tally: i64,
    pub lock_until: i64,
    pub last_batch_time: i64,
    pub seq: u64,
    pub journal_hash: [u8; 32],
    pub trades_closed: u32,
    pub latch: bool,
    pub flat: bool,
    pub tight_on: bool,
    pub r: i64,
    pub g_peak: i64,
    pub l_level: i64,
    /// The batch is a `report_fail`: the account ends `Failed`.
    pub failed: bool,
}

/// One transaction's worth of journal: the exact `JournalBatch`, what the transaction needs around it, and what the
/// account must look like afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub kind: BatchKind,
    pub batch: JournalBatch,
    /// Distinct market ids the transaction must pass as remaining accounts (events' markets and the markets of floating proofs), ascending.
    pub markets: Vec<u16>,
    /// `Meta.time`.
    pub meta_time: i64,
    /// Number of archive records `Meta.samples_hash` commits to, and the number of bytes of the intents file `Meta.intents_hash` commits to.
    pub samples_committed: usize,
    pub intents_committed_bytes: usize,
    pub account_after: AccountSnapshot,
    /// Modelled size of the signed transaction and its compute units (see `batch.rs`).
    pub tx_bytes: usize,
    pub cu: u64,
}

/// Limits and options of `drain_batches`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchPolicy {
    /// Events per batch including the `Meta` (at most 8).
    pub max_events: usize,
    /// Distinct proofs per batch (at most 3).
    pub max_proofs: usize,
    /// Signed transaction size (at most 1,232).
    pub max_tx_bytes: usize,
    /// Compute units (default limit 200,000).
    pub max_cu: u64,
    /// Count a `ComputeBudget` instruction in the transaction size.
    pub compute_budget_ix: bool,
    /// Emit a `Meta`-only batch when nothing is queued but time has moved on since the last `Meta`.
    pub heartbeat: bool,
}

impl BatchPolicy {
    /// The chain's limits: 8 events, 3 proofs, 1,232 bytes, 200,000 CU, no budget instruction, no heartbeat.
    pub const MAX: BatchPolicy =
        BatchPolicy { max_events: 8, max_proofs: 3, max_tx_bytes: 1_232, max_cu: 200_000, compute_budget_ix: false, heartbeat: false };

    pub fn with_max_events(self, n: usize) -> BatchPolicy {
        BatchPolicy { max_events: n, ..self }
    }

    pub fn with_heartbeat(self, on: bool) -> BatchPolicy {
        BatchPolicy { heartbeat: on, ..self }
    }
}

/// The chain's `TradingAccount` as far as a pass decision needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainView {
    pub seq: u64,
    pub journal_hash: [u8; 32],
    pub b: i64,
    pub flat: bool,
    pub phase: u8,
}

/// The outcome of the watchdog replay the caller ran (the public verifier) before confirming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayCheck {
    Skipped,
    Valid,
    Mismatch,
}

/// What to send after `request_advance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassDecision {
    /// `confirm_pass`.
    Confirm,
    /// `reject_pass(reason as u8)`.
    Reject(crate::config::PassReject),
}

/// Where the account stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    /// The journal ended with a `Fail`.
    Failed(FailReason),
}
