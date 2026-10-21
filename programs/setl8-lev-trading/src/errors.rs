//! Full error list of DESIGN section 12. The ORDER is the wire contract: Anchor numbers
//! `#[error_code]` variants from 6000, so `Unauthorized` = 6000 ... `NotHeld` = 6038.
//! Append only at the end; never reorder or remove.
use anchor_lang::prelude::*;

#[error_code]
pub enum Setl8Error {
    #[msg("wrong signer for an admin or evaluator instruction")]
    Unauthorized,
    #[msg("sector paused")]
    Paused,
    #[msg("5 active accounts already")]
    WalletCapReached,
    #[msg("tier index not in the rule set")]
    InvalidTier,
    #[msg("account status is not Active")]
    AccountNotActive,
    #[msg("exposure-increasing fill while the account is locked")]
    AccountLocked,
    #[msg("market account missing or wrong")]
    MarketUnknown,
    #[msg("first_seq or prev_hash mismatch")]
    BadSequence,
    #[msg("event time before the last batch or the previous event")]
    TimeBackwards,
    #[msg("event time beyond now + 2 s")]
    TimeInFuture,
    #[msg("price proof feed id, value or age check failed")]
    BadPriceProof,
    #[msg("Close.pnl differs from the program's computation")]
    PnlMismatch,
    #[msg("quick or credited amount is wrong")]
    QuickMismatch,
    #[msg("negative fee amount")]
    FeeInvalid,
    #[msg("tight-mode start check failed")]
    TightStartInvalid,
    #[msg("l_level does not follow the formula, or fell")]
    FloorFormulaMismatch,
    #[msg("breach check failed")]
    BreachInvalid,
    #[msg("fail check failed")]
    FailInvalid,
    #[msg("quick profit above the limit without a fail")]
    QuickLimitExceeded,
    #[msg("journal posted while a pass is pending")]
    PassPending,
    #[msg("account is not flat")]
    NotFlat,
    #[msg("balance is below the pass target")]
    TargetNotReached,
    #[msg("no pending pass in the required state")]
    NoPendingPass,
    #[msg("pass delay has not elapsed")]
    PassDelayNotElapsed,
    #[msg("daily payout cap reached")]
    DailyPayoutCapReached,
    #[msg("vault did not accept the payout")]
    PayoutNotAccepted,
    #[msg("vault return data missing or from the wrong program")]
    ReturnDataInvalid,
    #[msg("previous account is not locally Failed")]
    ResetNotAllowed,
    #[msg("operation not valid in this phase")]
    WrongPhase,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("account closed before 7 days in a terminal state")]
    CloseTooEarly,
    #[msg("rule set version not found")]
    RuleSetMissing,
    #[msg("more than 8 events or 3 proofs in a batch")]
    BatchTooLarge,
    #[msg("cancel_pass before 1 hour")]
    CancelTooEarly,
    #[msg("pass delay outside 1 to 6 days, or zero cap or zero window")]
    InvalidLimits,
    #[msg("wallet is banned")]
    WalletBanned,
    #[msg("evidence hash is missing or zero")]
    EvidenceMissing,
    #[msg("account is held for review")]
    AccountHeld,
    #[msg("account is not held")]
    NotHeld,
    // ---- added after DESIGN 12 (append only; the codes above are fixed) ----
    #[msg("a flag byte is not 0 or 1")]
    InvalidFlag,
    #[msg("token account has the wrong owner or mint")]
    InvalidTokenAccount,
    #[msg("vault program account does not match the configured vault")]
    InvalidVaultProgram,
    #[msg("stored account fields disagree with the engine state")]
    EngineStateMismatch,
    // ---- M5b: one variant per rejection reason of post_journal (6043 and up) ----
    #[msg("batch.prev_hash is not the stored journal_hash")]
    BadPrevHash,
    #[msg("the batch has no events")]
    EmptyBatch,
    #[msg("the batch bytes do not decode (truncated, trailing bytes, unknown tag or enum value, bad boolean)")]
    BatchMalformed,
    #[msg("an event refers to a proof index that does not exist")]
    ProofIndexOutOfRange,
    #[msg("a decisive event carries no price proof")]
    ProofMissing,
    #[msg("proof feed id is not the market's current feed")]
    ProofWrongFeed,
    #[msg("proof is older than the market's max age")]
    ProofTooOld,
    #[msg("proof is more than 2 s ahead of the event")]
    ProofInFuture,
    #[msg("proof source is not supported")]
    ProofUnknownSource,
    #[msg("mock proofs are only accepted by the localnet build")]
    ProofMockNotAllowed,
    #[msg("proof signed_update has the wrong shape for its source")]
    ProofBadSignedUpdate,
    #[msg("proof value is not a positive price that fits")]
    ProofValueInvalid,
    #[msg("event price differs from the price derived from the proof and the spread")]
    EventPriceMismatch,
    #[msg("an event price is zero")]
    PriceInvalid,
    #[msg("market is disabled")]
    MarketDisabled,
    #[msg("market is reduce-only: opening or adding is refused")]
    MarketReduceOnly,
    #[msg("market is outside its session windows")]
    MarketClosed,
    #[msg("quantity is zero, off the step or below the minimum")]
    QuantityInvalid,
    #[msg("close timer_start is after the close time")]
    TimerInvalid,
    #[msg("credited amount is wrong")]
    CreditMismatch,
    #[msg("tight mode is already on")]
    TightAlreadyOn,
    #[msg("tight mode is not on")]
    TightNotOn,
    #[msg("open profit is not above 1% of S")]
    TightStartPOpen,
    #[msg("g_peak is negative or above the open profit")]
    TightStartGPeak,
    #[msg("r differs from the closed balance")]
    TightStartR,
    #[msg("l_start differs from min(static floor + 4% S, r)")]
    TightStartLStart,
    #[msg("l_level differs from the tight-start formula")]
    TightStartLLevel,
    #[msg("g_peak is not above the stored peak")]
    FloorNotRaised,
    #[msg("l_level differs from the ratchet formula")]
    FloorLLevel,
    #[msg("TightEnd in a batch that does not end flat")]
    TightEndNotFlat,
    #[msg("batch ends flat but tight mode is still on")]
    TightNotEnded,
    #[msg("breach l_level differs from the stored tight floor")]
    BreachLLevel,
    #[msg("breach b_after differs from the closed balance")]
    BreachBalance,
    #[msg("breach balance is above the tight floor")]
    BreachNotBelowFloor,
    #[msg("breach balance is at or below the static floor (that is a fail)")]
    BreachBelowStaticFloor,
    #[msg("breach in a batch that does not end flat")]
    BreachNotFlat,
    #[msg("Fail events are only posted through report_fail")]
    FailEventNotAllowed,
    #[msg("the batch has no Meta event")]
    MetaMissing,
    #[msg("Meta is not the last event")]
    MetaNotLast,
    #[msg("the batch has more than one Meta event")]
    MetaTwice,
    #[msg("Meta engine_version differs from the program's engine")]
    EngineVersionMismatch,
    // ---- M6: the pass flow (6084 and up) ----
    #[msg("a pass is already requested or confirmed")]
    PassAlreadyPending,
    #[msg("tight mode is on")]
    TightModeOn,
    #[msg("the phase is failed (static floor or quick limit)")]
    PhaseFailed,
    #[msg("reject reason is not 0 (NotFlat), 1 (JournalBehind), 2 (TargetNotReached) or 3 (Other)")]
    RejectReasonInvalid,
    #[msg("the payout tally account does not parse")]
    PayoutTallyInvalid,
    // ---- M7: fail, hold, violation, activity, close (6089 and up) ----
    #[msg("the batch ends flat at or below the static floor: use report_fail")]
    MustReportFail,
    #[msg("fail reason is not 0 (StaticFloor) or 1 (PhaseQuick)")]
    FailReasonInvalid,
    #[msg("the Fail event's reason differs from the argument")]
    FailReasonMismatch,
    #[msg("report_fail needs a Fail event")]
    FailMissing,
    #[msg("the batch has more than one Fail event")]
    FailTwice,
    #[msg("the Fail event is not the one right before Meta")]
    FailNotBeforeMeta,
    #[msg("a failing batch must end flat")]
    FailNotFlat,
    #[msg("Fail.b_after differs from the closed balance")]
    FailBalanceMismatch,
    #[msg("closed balance is above the static floor")]
    FailStaticFloorNotReached,
    #[msg("the quick tally is not above 0.028 S")]
    FailQuickNotExceeded,
    #[msg("the quick tally is above the limit: the fail reason must be PhaseQuick")]
    QuickLimitNeedsPhaseQuick,
    #[msg("the account is already held")]
    AlreadyHeld,
    #[msg("the wallet has a held account")]
    WalletHasHeldAccount,
    #[msg("the account is not in a terminal state")]
    AccountNotTerminal,
    #[msg("vault account is not the owned PDA of this challenge")]
    InvalidVaultAccount,
    #[msg("reset price: not exactly one tier has the failed account's size")]
    ResetTierInvalid,
    #[msg("reset price: the failed phase is beyond the reset ladder")]
    ResetPhaseOutOfRange,
    #[msg("reset price: the price does not fit in a u64")]
    ResetPriceOverflow,
    #[msg("the previous account was already reset")]
    AccountAlreadyReset,
    #[msg("an account failed for a violation can never be reset")]
    ViolationNoReset,
    #[msg("the market has no half-spread in the account's rule set: an exposure-increasing fill is refused")]
    MarketUnpriced,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_design_section_12() {
        assert_eq!(u32::from(Setl8Error::Unauthorized), 6000);
        assert_eq!(u32::from(Setl8Error::DailyPayoutCapReached), 6024);
        assert_eq!(u32::from(Setl8Error::InvalidLimits), 6034);
        assert_eq!(u32::from(Setl8Error::NotHeld), 6038);
        assert_eq!(u32::from(Setl8Error::InvalidFlag), 6039);
        assert_eq!(u32::from(Setl8Error::InvalidTokenAccount), 6040);
        assert_eq!(u32::from(Setl8Error::InvalidVaultProgram), 6041);
        assert_eq!(u32::from(Setl8Error::EngineStateMismatch), 6042);
        assert_eq!(u32::from(Setl8Error::BadPrevHash), 6043);
        assert_eq!(u32::from(Setl8Error::EmptyBatch), 6044);
        assert_eq!(u32::from(Setl8Error::BatchMalformed), 6045);
        assert_eq!(u32::from(Setl8Error::ProofIndexOutOfRange), 6046);
        assert_eq!(u32::from(Setl8Error::ProofMissing), 6047);
        assert_eq!(u32::from(Setl8Error::ProofWrongFeed), 6048);
        assert_eq!(u32::from(Setl8Error::ProofTooOld), 6049);
        assert_eq!(u32::from(Setl8Error::ProofInFuture), 6050);
        assert_eq!(u32::from(Setl8Error::ProofUnknownSource), 6051);
        assert_eq!(u32::from(Setl8Error::ProofMockNotAllowed), 6052);
        assert_eq!(u32::from(Setl8Error::ProofBadSignedUpdate), 6053);
        assert_eq!(u32::from(Setl8Error::ProofValueInvalid), 6054);
        assert_eq!(u32::from(Setl8Error::EventPriceMismatch), 6055);
        assert_eq!(u32::from(Setl8Error::PriceInvalid), 6056);
        assert_eq!(u32::from(Setl8Error::MarketDisabled), 6057);
        assert_eq!(u32::from(Setl8Error::MarketReduceOnly), 6058);
        assert_eq!(u32::from(Setl8Error::MarketClosed), 6059);
        assert_eq!(u32::from(Setl8Error::QuantityInvalid), 6060);
        assert_eq!(u32::from(Setl8Error::TimerInvalid), 6061);
        assert_eq!(u32::from(Setl8Error::CreditMismatch), 6062);
        assert_eq!(u32::from(Setl8Error::TightAlreadyOn), 6063);
        assert_eq!(u32::from(Setl8Error::TightNotOn), 6064);
        assert_eq!(u32::from(Setl8Error::TightStartPOpen), 6065);
        assert_eq!(u32::from(Setl8Error::TightStartGPeak), 6066);
        assert_eq!(u32::from(Setl8Error::TightStartR), 6067);
        assert_eq!(u32::from(Setl8Error::TightStartLStart), 6068);
        assert_eq!(u32::from(Setl8Error::TightStartLLevel), 6069);
        assert_eq!(u32::from(Setl8Error::FloorNotRaised), 6070);
        assert_eq!(u32::from(Setl8Error::FloorLLevel), 6071);
        assert_eq!(u32::from(Setl8Error::TightEndNotFlat), 6072);
        assert_eq!(u32::from(Setl8Error::TightNotEnded), 6073);
        assert_eq!(u32::from(Setl8Error::BreachLLevel), 6074);
        assert_eq!(u32::from(Setl8Error::BreachBalance), 6075);
        assert_eq!(u32::from(Setl8Error::BreachNotBelowFloor), 6076);
        assert_eq!(u32::from(Setl8Error::BreachBelowStaticFloor), 6077);
        assert_eq!(u32::from(Setl8Error::BreachNotFlat), 6078);
        assert_eq!(u32::from(Setl8Error::FailEventNotAllowed), 6079);
        assert_eq!(u32::from(Setl8Error::MetaMissing), 6080);
        assert_eq!(u32::from(Setl8Error::MetaNotLast), 6081);
        assert_eq!(u32::from(Setl8Error::MetaTwice), 6082);
        assert_eq!(u32::from(Setl8Error::EngineVersionMismatch), 6083);
        assert_eq!(u32::from(Setl8Error::PassAlreadyPending), 6084);
        assert_eq!(u32::from(Setl8Error::TightModeOn), 6085);
        assert_eq!(u32::from(Setl8Error::PhaseFailed), 6086);
        assert_eq!(u32::from(Setl8Error::RejectReasonInvalid), 6087);
        assert_eq!(u32::from(Setl8Error::PayoutTallyInvalid), 6088);
        assert_eq!(u32::from(Setl8Error::MustReportFail), 6089);
        assert_eq!(u32::from(Setl8Error::FailReasonInvalid), 6090);
        assert_eq!(u32::from(Setl8Error::FailReasonMismatch), 6091);
        assert_eq!(u32::from(Setl8Error::FailMissing), 6092);
        assert_eq!(u32::from(Setl8Error::FailTwice), 6093);
        assert_eq!(u32::from(Setl8Error::FailNotBeforeMeta), 6094);
        assert_eq!(u32::from(Setl8Error::FailNotFlat), 6095);
        assert_eq!(u32::from(Setl8Error::FailBalanceMismatch), 6096);
        assert_eq!(u32::from(Setl8Error::FailStaticFloorNotReached), 6097);
        assert_eq!(u32::from(Setl8Error::FailQuickNotExceeded), 6098);
        assert_eq!(u32::from(Setl8Error::QuickLimitNeedsPhaseQuick), 6099);
        assert_eq!(u32::from(Setl8Error::AlreadyHeld), 6100);
        assert_eq!(u32::from(Setl8Error::WalletHasHeldAccount), 6101);
        assert_eq!(u32::from(Setl8Error::AccountNotTerminal), 6102);
        assert_eq!(u32::from(Setl8Error::InvalidVaultAccount), 6103);
        assert_eq!(u32::from(Setl8Error::ResetTierInvalid), 6104);
        assert_eq!(u32::from(Setl8Error::ResetPhaseOutOfRange), 6105);
        assert_eq!(u32::from(Setl8Error::ResetPriceOverflow), 6106);
        assert_eq!(u32::from(Setl8Error::AccountAlreadyReset), 6107);
        assert_eq!(u32::from(Setl8Error::ViolationNoReset), 6108);
        assert_eq!(u32::from(Setl8Error::MarketUnpriced), 6109);
    }
}
