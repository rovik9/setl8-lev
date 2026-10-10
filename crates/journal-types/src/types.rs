//! Journal types of DESIGN 8.2. Field order and widths are the wire format; see `codec.rs`.
use crate::enums::{CloseReason, FailReason, FeeKind, OrderKind, Side};

/// One `post_journal` payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalBatch {
    /// Must equal `account.seq + 1`.
    pub first_seq: u64,
    /// Must equal `account.journal_hash`.
    pub prev_hash: [u8; 32],
    /// 1 to `MAX_EVENTS_PER_BATCH` events.
    pub events: Vec<JournalEvent>,
    /// 0 to `MAX_PROOFS_PER_BATCH` proofs, referenced by index from the events.
    pub proofs: Vec<PriceProof>,
}

/// An evaluator-posted event. Variant order is the wire tag (Fill 0 ... Meta 8).
/// `proof` fields hold an index into the batch's proofs, or `NO_PROOF` (255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalEvent {
    Fill {
        market: u16,
        side: Side,
        order_kind: OrderKind,
        qty: u64,
        price_e12: u64,
        time: i64,
        avg_entry_after: u64,
        margin: u64,
        sl: u64,
        tp: u64,
        intent_id: u32,
        proof: u8,
    },
    Close {
        market: u16,
        side: Side,
        qty: u64,
        entry_e12: u64,
        exit_e12: u64,
        time: i64,
        timer_start: i64,
        pnl: i64,
        quick: bool,
        credited: i64,
        reason: CloseReason,
        intent_id: u32,
        proof: u8,
    },
    Fee {
        kind: FeeKind,
        market: u16,
        amount: u64,
        time: i64,
    },
    TightStart {
        time: i64,
        p_open: i64,
        r: i64,
        l_start: i64,
        g_peak: i64,
        l_level: i64,
        proof: u8,
    },
    FloorRaised {
        time: i64,
        g_peak: i64,
        l_level: i64,
        proof: u8,
    },
    TightEnd {
        time: i64,
    },
    Breach {
        time: i64,
        l_level: i64,
        b_after: i64,
        proof: u8,
    },
    Fail {
        time: i64,
        reason: FailReason,
        b_after: i64,
        proof: u8,
    },
    /// Last event of every batch.
    Meta {
        time: i64,
        flat: bool,
        samples_hash: [u8; 32],
        intents_hash: [u8; 32],
        engine_version: u16,
    },
}

impl JournalEvent {
    /// The raw `proof` field (index or `NO_PROOF`) of the variants that have one; `None` for `Fee`, `TightEnd`, `Meta`.
    pub fn proof_ref(&self) -> Option<u8> {
        match self {
            JournalEvent::Fill { proof, .. }
            | JournalEvent::Close { proof, .. }
            | JournalEvent::TightStart { proof, .. }
            | JournalEvent::FloorRaised { proof, .. }
            | JournalEvent::Breach { proof, .. }
            | JournalEvent::Fail { proof, .. } => Some(*proof),
            JournalEvent::Fee { .. } | JournalEvent::TightEnd { .. } | JournalEvent::Meta { .. } => None,
        }
    }

    /// The proof index this event points at, if it points at one (`None` for `NO_PROOF` and for variants without a field).
    pub fn proof_index(&self) -> Option<usize> {
        match self.proof_ref() {
            Some(p) if p != crate::NO_PROOF => Some(usize::from(p)),
            _ => None,
        }
    }

    /// The event time (every variant has one).
    pub fn time(&self) -> i64 {
        match self {
            JournalEvent::Fill { time, .. }
            | JournalEvent::Close { time, .. }
            | JournalEvent::Fee { time, .. }
            | JournalEvent::TightStart { time, .. }
            | JournalEvent::FloorRaised { time, .. }
            | JournalEvent::TightEnd { time }
            | JournalEvent::Breach { time, .. }
            | JournalEvent::Fail { time, .. }
            | JournalEvent::Meta { time, .. } => *time,
        }
    }
}

/// A price sample attached to a decisive event (DESIGN 8.2, decision 46).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceProof {
    /// 0 Stork (not built), 1 Pyth (not built), 2 GMX tickers, 3 GMX signed report, 4 Hyperliquid, 255 mock.
    pub source: u8,
    /// `sha256("SL8FEED" || source || utf8 symbol)`.
    pub feed_id: [u8; 32],
    pub timestamp_ns: u64,
    /// 18 decimals.
    pub quantized_value: i128,
    /// The source's raw signed report (source 3), empty for unsigned sources; at most `MAX_SIGNED_UPDATE_BYTES`.
    pub signed_update: Vec<u8>,
}
