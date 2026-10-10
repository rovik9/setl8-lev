//! Journal types, hash chain, price-proof checks and session windows (DESIGN 3.3, 3.4, 8.2, 8.3).
//!
//! No floats, no `std::time`, no unsafe, no panics in non-test code; usable from the Solana program (SBF), the
//! evaluator, the verifier and WASM. Hashing is injected ([`Sha256`]); feature `native-sha2` (default) adds
//! [`NativeSha256`]. The program should depend on this crate with `default-features = false`.
//!
//! # Wire format: canonical Borsh
//! Integers little-endian; fixed arrays raw; `Vec<T>` = u32 LE length + items; enum = u8 variant index in
//! declaration order + fields; `bool` = 0 or 1 (anything else is rejected).
//!
//! | enum / field | values |
//! |---|---|
//! | `JournalEvent` tag | Fill 0, Close 1, Fee 2, TightStart 3, FloorRaised 4, TightEnd 5, Breach 6, Fail 7, Meta 8 |
//! | `order_kind` | Market 0, Limit 1, Stop 2 |
//! | `side` | Long 0, Short 1 |
//! | Close `reason` | Manual 0, StopLoss 1, TakeProfit 2, Flip 3, Breach 4, Fail 5 |
//! | Fee `kind` | Holding 0, Commission 1 |
//! | Fail `reason` | StaticFloor 0, PhaseQuick 1 |
//! | `PriceProof.source` | Stork 0 (not built), Pyth 1 (not built), GMX tickers 2, GMX signed 3, Hyperliquid 4, mock 255 |
//! | event `proof` | index into the batch proofs, or `NO_PROOF` = 255 |
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )
)]

pub mod codec;
pub mod enums;
pub mod error;
pub mod hash;
pub mod proof;
pub mod session;
pub mod types;

pub use enums::{CloseReason, FailReason, FeeKind, OrderKind, Side, UnknownValue};
pub use error::{ChainError, DecodeError, EncodeError, ProofError};
#[cfg(feature = "native-sha2")]
pub use hash::NativeSha256;
pub use hash::{
    apply_batch, feed_id, genesis_hash, next_hash, proof_digest, verify_chain, ChainBreak, ChainBreakKind,
    ChainEntry, Sha256,
};
pub use proof::{check_proof, price_e12_from_quantized, proof_time_secs};
pub use session::{in_session, market_open, minute_of_week, MINUTES_PER_WEEK};
pub use types::{JournalBatch, JournalEvent, PriceProof};

/// Maximum events per `post_journal` batch (DESIGN 8.2).
pub const MAX_EVENTS_PER_BATCH: usize = 8;
/// Maximum price proofs per batch.
pub const MAX_PROOFS_PER_BATCH: usize = 3;
/// Maximum `PriceProof.signed_update` length in bytes.
pub const MAX_SIGNED_UPDATE_BYTES: usize = 512;
/// The `proof` field value of an event that carries no proof.
pub const NO_PROOF: u8 = 255;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_match_design() {
        assert_eq!(MAX_EVENTS_PER_BATCH, 8);
        assert_eq!(MAX_PROOFS_PER_BATCH, 3);
        assert_eq!(MAX_SIGNED_UPDATE_BYTES, 512);
        assert_eq!(NO_PROOF, 255);
    }
}
