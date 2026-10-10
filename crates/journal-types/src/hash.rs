//! Hash chain (DESIGN 8.3). Hashing is injected through [`Sha256`] so the Solana program can pass the
//! `sol_sha256` syscall and links no hashing code; [`NativeSha256`] (feature `native-sha2`) serves everyone else.
use crate::error::{ChainError, EncodeError};
use crate::types::{JournalBatch, JournalEvent, PriceProof};
use crate::NO_PROOF;

/// Prefix of the genesis hash: `h_0 = sha256("SL8LEV" || account pubkey)`.
pub const GENESIS_PREFIX: &[u8] = b"SL8LEV";
/// Prefix of every chain step: `h_n = sha256("SL8LEV1" || h_(n-1) || n u64 LE || borsh(event) || proof_digest)`.
pub const EVENT_PREFIX: &[u8] = b"SL8LEV1";
/// Prefix of feed ids: `sha256("SL8FEED" || source || utf8 symbol)`.
pub const FEED_PREFIX: &[u8] = b"SL8FEED";

/// SHA-256 over the concatenation of the given parts (the shape of the `sol_sha256` syscall).
pub trait Sha256 {
    fn hashv(&self, parts: &[&[u8]]) -> [u8; 32];
}

impl<F: Fn(&[&[u8]]) -> [u8; 32]> Sha256 for F {
    fn hashv(&self, parts: &[&[u8]]) -> [u8; 32] {
        self(parts)
    }
}

/// SHA-256 from the `sha2` crate (feature `native-sha2`).
#[cfg(feature = "native-sha2")]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeSha256;

#[cfg(feature = "native-sha2")]
impl Sha256 for NativeSha256 {
    fn hashv(&self, parts: &[&[u8]]) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().into()
    }
}

/// `h_0 = sha256("SL8LEV" || account pubkey)`.
pub fn genesis_hash<H: Sha256 + ?Sized>(h: &H, account_pubkey: &[u8; 32]) -> [u8; 32] {
    h.hashv(&[GENESIS_PREFIX, account_pubkey])
}

/// `sha256(borsh(proof))`.
pub fn proof_digest<H: Sha256 + ?Sized>(h: &H, proof: &PriceProof) -> Result<[u8; 32], EncodeError> {
    Ok(h.hashv(&[&proof.to_bytes()?]))
}

/// `sha256("SL8LEV1" || prev || seq u64 LE || borsh(event) || proof_digest or 32 zero bytes)`.
/// `proof` must be `Some` exactly when the event references a proof (its `proof` field is not `NO_PROOF`).
pub fn next_hash<H: Sha256 + ?Sized>(
    h: &H,
    prev: &[u8; 32],
    seq: u64,
    event: &JournalEvent,
    proof: Option<&PriceProof>,
) -> Result<[u8; 32], ChainError> {
    let references = matches!(event.proof_ref(), Some(p) if p != NO_PROOF);
    let digest = match (references, proof) {
        (true, Some(p)) => proof_digest(h, p)?,
        (false, None) => [0u8; 32],
        _ => return Err(ChainError::ProofMismatch),
    };
    Ok(h.hashv(&[EVENT_PREFIX, prev, &seq.to_le_bytes(), &event.to_bytes(), &digest]))
}

/// Applies a batch to a chain head: returns `(last_hash, last_seq)`. The first event gets `first_seq`, the next
/// `first_seq + 1`, and so on. Rejects `first_seq == 0`, `u64` overflow of the sequence, and an invalid batch.
/// (`prev_hash` is the account's stored head; the caller has already compared it with `batch.prev_hash`.)
pub fn apply_batch<H: Sha256 + ?Sized>(
    h: &H,
    prev_hash: &[u8; 32],
    first_seq: u64,
    batch: &JournalBatch,
) -> Result<([u8; 32], u64), ChainError> {
    if first_seq == 0 {
        return Err(ChainError::ZeroFirstSeq);
    }
    batch.validate()?;
    let mut hash = *prev_hash;
    let mut seq = first_seq;
    let mut last_seq = first_seq;
    for (i, ev) in batch.events.iter().enumerate() {
        if i > 0 {
            seq = seq.checked_add(1).ok_or(ChainError::SeqOverflow)?;
        }
        let proof = match ev.proof_index() {
            Some(idx) => Some(batch.proofs.get(idx).ok_or(EncodeError::BadProofIndex)?),
            None => None,
        };
        hash = next_hash(h, &hash, seq, ev, proof)?;
        last_seq = seq;
    }
    Ok((hash, last_seq))
}

/// One recorded step of a chain, as a verifier sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainEntry {
    pub seq: u64,
    pub event: JournalEvent,
    /// The proof the event references, if it references one.
    pub proof: Option<PriceProof>,
    /// The chain head recorded after this event (for example from an account snapshot).
    pub hash: [u8; 32],
}

/// What went wrong at a [`ChainBreak`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainBreakKind {
    /// `seq` is not the previous `seq + 1` (or `first_seq` for entry 0): a dropped, inserted, repeated or reordered entry.
    BadSeq,
    /// The recomputed hash differs from the recorded one: an altered event, proof or earlier hash.
    BadHash,
    /// The entry's proof is missing, superfluous or cannot be encoded.
    BadProof,
    /// The sequence number would pass `u64::MAX`.
    SeqOverflow,
}

/// The first entry that does not fit the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainBreak {
    pub index: usize,
    pub kind: ChainBreakKind,
}

/// Recomputes a chain from `start_hash` (the genesis hash, or any earlier head) and compares every step with the
/// recorded entries. Returns the final head on success, otherwise the first index that breaks.
/// The caller still compares the returned head with the account's `journal_hash`: dropping the LAST entry
/// leaves a valid shorter chain.
pub fn verify_chain<H: Sha256 + ?Sized>(
    h: &H,
    start_hash: &[u8; 32],
    first_seq: u64,
    entries: &[ChainEntry],
) -> Result<[u8; 32], ChainBreak> {
    let mut hash = *start_hash;
    let mut expected_seq = first_seq;
    for (index, e) in entries.iter().enumerate() {
        if index > 0 {
            expected_seq = expected_seq
                .checked_add(1)
                .ok_or(ChainBreak { index, kind: ChainBreakKind::SeqOverflow })?;
        }
        if e.seq != expected_seq || e.seq == 0 {
            return Err(ChainBreak { index, kind: ChainBreakKind::BadSeq });
        }
        hash = next_hash(h, &hash, e.seq, &e.event, e.proof.as_ref())
            .map_err(|_| ChainBreak { index, kind: ChainBreakKind::BadProof })?;
        if hash != e.hash {
            return Err(ChainBreak { index, kind: ChainBreakKind::BadHash });
        }
    }
    Ok(hash)
}

/// `sha256("SL8FEED" || [source] || utf8 symbol)`.
pub fn feed_id<H: Sha256 + ?Sized>(h: &H, source: u8, symbol: &str) -> [u8; 32] {
    h.hashv(&[FEED_PREFIX, &[source], symbol.as_bytes()])
}
