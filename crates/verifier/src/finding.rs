//! Findings, verdicts and the report (stable names and stable JSON).
use serde::Serialize;

/// Every class of finding. The names are a stable interface (`name()`), documented in handover/module-9.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindingClass {
    // ---- stage 1
    HashChain,
    SequenceGap,
    BadGenesis,
    EventDecode,
    UnknownEvent,
    ProofFeed,
    ProofAge,
    ProofShape,
    PriceMismatch,
    ArchiveMissing,
    SamplesHashMismatch,
    IntentsHashMismatch,
    ForgedTrade,
    BadIntentSignature,
    IntentDecode,
    UnknownMarket,
    PositionMismatch,
    PnlMismatch,
    CreditMismatch,
    QuickMismatch,
    FeeMismatch,
    MarginMismatch,
    AvgEntryMismatch,
    FloorMismatch,
    StateMismatch,
    EngineVersion,
    PassInvalid,
    FalseFail,
    // ---- stage 2
    MissedBreach,
    MissedFail,
    FalseBreach,
    MissedTightStart,
    FalseTightStart,
    MissedFloorRaise,
    FalseFloorRaise,
    WrongLLevel,
    TriggerMissed,
    TriggerFalse,
    ArchiveDisagreement,
    // ---- M9b
    PendingFillWithoutMargin,
    // ---- M9c
    FillWithoutMargin,
    FillNotAllowed,
    // ---- P25b (decision 68), appended so the earlier classes keep their order
    ArchiveRecordInvalid,
}

impl FindingClass {
    pub const ALL: [FindingClass; 43] = [
        FindingClass::HashChain,
        FindingClass::SequenceGap,
        FindingClass::BadGenesis,
        FindingClass::EventDecode,
        FindingClass::UnknownEvent,
        FindingClass::ProofFeed,
        FindingClass::ProofAge,
        FindingClass::ProofShape,
        FindingClass::PriceMismatch,
        FindingClass::ArchiveMissing,
        FindingClass::SamplesHashMismatch,
        FindingClass::IntentsHashMismatch,
        FindingClass::ForgedTrade,
        FindingClass::BadIntentSignature,
        FindingClass::IntentDecode,
        FindingClass::UnknownMarket,
        FindingClass::PositionMismatch,
        FindingClass::PnlMismatch,
        FindingClass::CreditMismatch,
        FindingClass::QuickMismatch,
        FindingClass::FeeMismatch,
        FindingClass::MarginMismatch,
        FindingClass::AvgEntryMismatch,
        FindingClass::FloorMismatch,
        FindingClass::StateMismatch,
        FindingClass::EngineVersion,
        FindingClass::PassInvalid,
        FindingClass::FalseFail,
        FindingClass::MissedBreach,
        FindingClass::MissedFail,
        FindingClass::FalseBreach,
        FindingClass::MissedTightStart,
        FindingClass::FalseTightStart,
        FindingClass::MissedFloorRaise,
        FindingClass::FalseFloorRaise,
        FindingClass::WrongLLevel,
        FindingClass::TriggerMissed,
        FindingClass::TriggerFalse,
        FindingClass::ArchiveDisagreement,
        FindingClass::PendingFillWithoutMargin,
        FindingClass::FillWithoutMargin,
        FindingClass::FillNotAllowed,
        FindingClass::ArchiveRecordInvalid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FindingClass::HashChain => "HashChain",
            FindingClass::SequenceGap => "SequenceGap",
            FindingClass::BadGenesis => "BadGenesis",
            FindingClass::EventDecode => "EventDecode",
            FindingClass::UnknownEvent => "UnknownEvent",
            FindingClass::ProofFeed => "ProofFeed",
            FindingClass::ProofAge => "ProofAge",
            FindingClass::ProofShape => "ProofShape",
            FindingClass::PriceMismatch => "PriceMismatch",
            FindingClass::ArchiveMissing => "ArchiveMissing",
            FindingClass::SamplesHashMismatch => "SamplesHashMismatch",
            FindingClass::IntentsHashMismatch => "IntentsHashMismatch",
            FindingClass::ForgedTrade => "ForgedTrade",
            FindingClass::BadIntentSignature => "BadIntentSignature",
            FindingClass::IntentDecode => "IntentDecode",
            FindingClass::UnknownMarket => "UnknownMarket",
            FindingClass::PositionMismatch => "PositionMismatch",
            FindingClass::PnlMismatch => "PnlMismatch",
            FindingClass::CreditMismatch => "CreditMismatch",
            FindingClass::QuickMismatch => "QuickMismatch",
            FindingClass::FeeMismatch => "FeeMismatch",
            FindingClass::MarginMismatch => "MarginMismatch",
            FindingClass::AvgEntryMismatch => "AvgEntryMismatch",
            FindingClass::FloorMismatch => "FloorMismatch",
            FindingClass::StateMismatch => "StateMismatch",
            FindingClass::EngineVersion => "EngineVersion",
            FindingClass::PassInvalid => "PassInvalid",
            FindingClass::FalseFail => "FalseFail",
            FindingClass::MissedBreach => "MissedBreach",
            FindingClass::MissedFail => "MissedFail",
            FindingClass::FalseBreach => "FalseBreach",
            FindingClass::MissedTightStart => "MissedTightStart",
            FindingClass::FalseTightStart => "FalseTightStart",
            FindingClass::MissedFloorRaise => "MissedFloorRaise",
            FindingClass::FalseFloorRaise => "FalseFloorRaise",
            FindingClass::WrongLLevel => "WrongLLevel",
            FindingClass::TriggerMissed => "TriggerMissed",
            FindingClass::TriggerFalse => "TriggerFalse",
            FindingClass::ArchiveDisagreement => "ArchiveDisagreement",
            FindingClass::PendingFillWithoutMargin => "PendingFillWithoutMargin",
            FindingClass::FillWithoutMargin => "FillWithoutMargin",
            FindingClass::FillNotAllowed => "FillNotAllowed",
            FindingClass::ArchiveRecordInvalid => "ArchiveRecordInvalid",
        }
    }
}

impl Serialize for FindingClass {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// One thing the replay found wrong. `expected` is what the verifier computed, `found` what the evidence says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub class: FindingClass,
    /// Chain sequence number of the event concerned, if any.
    pub seq: Option<u64>,
    /// Position of the event in the log (0-based among event lines), if any.
    pub event_index: Option<usize>,
    pub expected: String,
    pub found: String,
    pub message: String,
    /// "primary" for the main archive, "second" for a finding that only the second archive supports.
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Verdict {
    Valid,
    Mismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhaseVerdict {
    pub phase: u8,
    pub first_seq: u64,
    pub last_seq: u64,
    pub events: usize,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassVerdict {
    pub phase: u8,
    /// "requested" or "confirmed".
    pub kind: &'static str,
    pub requested_seq: u64,
    pub valid: bool,
    pub reasons: Vec<String>,
}

/// The replayed account at the end of the replay (numbers the tests and the 4.9 reproduction read).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StateSummary {
    pub b: i64,
    pub q_tally: i64,
    pub latch: bool,
    pub tight_on: bool,
    pub r: i64,
    pub g_peak: i64,
    pub l_level: i64,
    pub lock_until: i64,
    pub flat: bool,
    pub phase: u8,
    pub terminal: bool,
    pub trades_closed: u32,
    pub seq: u64,
    pub journal_hash: String,
    pub open_positions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// True when more than `MAX_FINDINGS` were found and the rest were dropped.
    pub truncated: bool,
    pub per_phase: Vec<PhaseVerdict>,
    pub per_pass: Vec<PassVerdict>,
    pub state: StateSummary,
}

impl Report {
    /// The distinct classes found, sorted (for tests and the `watch` line).
    pub fn classes(&self) -> Vec<FindingClass> {
        let mut v: Vec<FindingClass> = self.findings.iter().map(|f| f.class).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Findings are capped at this many; the verdict stays `Mismatch`.
pub const MAX_FINDINGS: usize = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_all_is_complete() {
        let mut names: Vec<&str> = FindingClass::ALL.iter().map(|c| c.name()).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
        let mut sorted = FindingClass::ALL.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), FindingClass::ALL.len());
    }

    #[test]
    fn class_serialises_as_its_name() {
        assert_eq!(serde_json::to_string(&FindingClass::PnlMismatch).unwrap(), "\"PnlMismatch\"");
    }
}
