//! Error types. None of them carries data from the input, so decoding untrusted bytes cannot allocate by surprise.

/// Why bytes could not be decoded into a journal type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended before the value was complete, or a length prefix claims more bytes than remain.
    UnexpectedEof,
    /// Bytes remain after the value (canonical encodings have none).
    TrailingBytes,
    /// Event count is 0 or above `MAX_EVENTS_PER_BATCH`.
    BadEventCount,
    /// Proof count is above `MAX_PROOFS_PER_BATCH`.
    BadProofCount,
    /// `signed_update` is longer than `MAX_SIGNED_UPDATE_BYTES`.
    SignedUpdateTooLong,
    /// An event refers to a proof index that is neither `NO_PROOF` nor below the proof count.
    BadProofIndex,
    /// The event variant byte is not 0 to 8.
    UnknownEventTag,
    /// A `side`, `order_kind`, `reason` or `kind` byte is not a defined value.
    UnknownEnumValue,
    /// A boolean byte (`quick`, `flat`) is neither 0 nor 1.
    InvalidBool,
}

/// Why a value cannot be encoded: it breaks a limit that decoding would reject, so every encoding decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    BadEventCount,
    BadProofCount,
    SignedUpdateTooLong,
    BadProofIndex,
}

/// Why a hash-chain step failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainError {
    /// The batch (or proof) cannot be encoded.
    Encode(EncodeError),
    /// `first_seq` was 0 (sequence numbers start at 1; 0 is the genesis state).
    ZeroFirstSeq,
    /// A sequence number would pass `u64::MAX`.
    SeqOverflow,
    /// The event references a proof but none was supplied, or a proof was supplied for an event that references none.
    ProofMismatch,
}

impl From<EncodeError> for ChainError {
    fn from(e: EncodeError) -> Self {
        ChainError::Encode(e)
    }
}

/// Why a price proof was rejected (DESIGN I-4, 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofError {
    /// The proof's `feed_id` is not the market's current feed.
    WrongFeed,
    /// The event price differs from the proof's value converted to `price_e12`.
    PriceMismatch,
    /// `proof_time < event_time - max_age_secs`.
    TooOld,
    /// `proof_time > event_time + 2`.
    InFuture,
    /// `source` is not one of the supported sources (2 GMX tickers, 3 GMX signed, 4 Hyperliquid, 255 mock).
    UnknownSource,
    /// Source 255 (mock) while mocks are not allowed.
    MockNotAllowed,
    /// `signed_update` is longer than `MAX_SIGNED_UPDATE_BYTES`.
    SignedUpdateTooLong,
    /// The quantized value is zero or negative, or floors to a zero price.
    ValueNotPositive,
    /// The converted price does not fit in `u64`.
    ValueTooLarge,
    /// `signed_update` has the wrong shape for its source (decision 47): empty for sources 2 and 4, exactly 32 bytes
    /// (the sha256 of the signed blob) for source 3.
    BadSignedUpdate,
}
