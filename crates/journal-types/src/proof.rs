//! Price proof checks (DESIGN I-4, 3.3). Pure functions; the publisher signature is NOT checked here.
use crate::error::ProofError;
use crate::types::PriceProof;
use crate::MAX_SIGNED_UPDATE_BYTES;

/// Proof sources (decisions 43, 44, 46).
pub const SOURCE_STORK: u8 = 0;
pub const SOURCE_PYTH: u8 = 1;
pub const SOURCE_GMX_TICKERS: u8 = 2;
pub const SOURCE_GMX_SIGNED: u8 = 3;
pub const SOURCE_HYPERLIQUID: u8 = 4;
pub const SOURCE_MOCK: u8 = 255;

/// A proof may be at most this many seconds ahead of the event time (I-4: `proof.time <= ev.time + 2`).
pub const MAX_PROOF_FUTURE_SECS: i64 = 2;

/// Source 3 carries the sha256 of the signed blob (decision 47).
pub const SIGNED_UPDATE_DIGEST_BYTES: usize = 32;

const NS_PER_SEC: u64 = 1_000_000_000;
const QUANTIZED_PER_E12: i128 = 1_000_000;

/// `price_e12 = floor(quantized_value / 10^6)` (18-decimal value to the 12-decimal price).
/// Rejects zero and negative values, a value that floors to price 0, and a price above `u64::MAX`.
pub fn price_e12_from_quantized(quantized_value: i128) -> Result<u64, ProofError> {
    if quantized_value <= 0 {
        return Err(ProofError::ValueNotPositive);
    }
    let price = quantized_value / QUANTIZED_PER_E12;
    if price == 0 {
        return Err(ProofError::ValueNotPositive);
    }
    u64::try_from(price).map_err(|_| ProofError::ValueTooLarge)
}

/// Seconds of the proof timestamp, floored.
pub fn proof_time_secs(proof: &PriceProof) -> i64 {
    // u64::MAX / 1e9 is about 1.8e10, always inside i64.
    i64::try_from(proof.timestamp_ns / NS_PER_SEC).unwrap_or(i64::MAX)
}

/// The `signed_update` shape of decision 47: EMPTY for sources 2 and 4, exactly 32 bytes (the sha256 of the signed
/// blob) for source 3. Any other source (mock, unsupported) only needs the length limit, which `check_proof` enforces.
pub fn check_signed_update_shape(proof: &PriceProof) -> Result<(), ProofError> {
    let ok = match proof.source {
        SOURCE_GMX_TICKERS | SOURCE_HYPERLIQUID => proof.signed_update.is_empty(),
        SOURCE_GMX_SIGNED => proof.signed_update.len() == SIGNED_UPDATE_DIGEST_BYTES,
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(ProofError::BadSignedUpdate)
    }
}

/// I-4 for one proof, comparing the proof's price with `event_price_e12`.
/// Checks, in this order: source (`UnknownSource`; `MockNotAllowed` for 255 unless `allow_mock`),
/// `signed_update` length, feed id, value (`price_e12_from_quantized` must equal `event_price_e12`), then the time window
/// `event_time - max_age_secs <= proof_time <= event_time + 2`, both ends inclusive.
/// Supported sources are 2, 3, 4 and 255; 0 and 1 are defined but not built, so they are `UnknownSource`.
pub fn check_proof(
    proof: &PriceProof,
    market_feed_id: &[u8; 32],
    max_age_secs: u32,
    event_time: i64,
    event_price_e12: u64,
    allow_mock: bool,
) -> Result<(), ProofError> {
    check_proof_inner(proof, market_feed_id, max_age_secs, event_time, Some(event_price_e12), allow_mock)
}

/// The same checks without an event price (events that carry a proof only for time and feed: tight start, floor
/// raise, breach). The proof value must still convert to a positive price.
pub fn check_proof_without_price(
    proof: &PriceProof,
    market_feed_id: &[u8; 32],
    max_age_secs: u32,
    event_time: i64,
    allow_mock: bool,
) -> Result<(), ProofError> {
    check_proof_inner(proof, market_feed_id, max_age_secs, event_time, None, allow_mock)
}

fn check_proof_inner(
    proof: &PriceProof,
    market_feed_id: &[u8; 32],
    max_age_secs: u32,
    event_time: i64,
    event_price_e12: Option<u64>,
    allow_mock: bool,
) -> Result<(), ProofError> {
    match proof.source {
        SOURCE_GMX_TICKERS | SOURCE_GMX_SIGNED | SOURCE_HYPERLIQUID => {}
        SOURCE_MOCK if allow_mock => {}
        SOURCE_MOCK => return Err(ProofError::MockNotAllowed),
        _ => return Err(ProofError::UnknownSource),
    }
    if proof.signed_update.len() > MAX_SIGNED_UPDATE_BYTES {
        return Err(ProofError::SignedUpdateTooLong);
    }
    if proof.feed_id != *market_feed_id {
        return Err(ProofError::WrongFeed);
    }
    let price = price_e12_from_quantized(proof.quantized_value)?;
    if event_price_e12.is_some_and(|p| p != price) {
        return Err(ProofError::PriceMismatch);
    }

    let t = i128::from(proof_time_secs(proof));
    let now = i128::from(event_time);
    // i128 holds any i64 +/- u32 exactly, so the saturating forms never actually saturate.
    if t < now.saturating_sub(i128::from(max_age_secs)) {
        return Err(ProofError::TooOld);
    }
    if t > now.saturating_add(i128::from(MAX_PROOF_FUTURE_SECS)) {
        return Err(ProofError::InFuture);
    }
    Ok(())
}
