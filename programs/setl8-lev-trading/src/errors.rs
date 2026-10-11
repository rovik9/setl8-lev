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
    #[msg("pass delay outside 1 to 14 days, or zero cap or zero window")]
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
    }
}
