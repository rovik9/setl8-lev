//! Canonical Borsh, hand-written so the decoder controls every length check.
//! Layout rules (identical to Borsh as Anchor 0.32 produces): integers little-endian, fixed arrays raw,
//! `Vec<T>` = u32 LE length + items, enum = u8 variant index in declaration order + fields, bool = 0 or 1.
use crate::enums::{CloseReason, FailReason, FeeKind, OrderKind, Side, UnknownValue};
use crate::error::{DecodeError, EncodeError};
use crate::types::{JournalBatch, JournalEvent, PriceProof};
use crate::{MAX_EVENTS_PER_BATCH, MAX_PROOFS_PER_BATCH, MAX_SIGNED_UPDATE_BYTES, NO_PROOF};

// ------------------------------------------------------------------ writing

fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i128(out: &mut Vec<u8>, v: i128) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_bool(out: &mut Vec<u8>, v: bool) {
    out.push(u8::from(v));
}

impl JournalEvent {
    /// Appends the canonical Borsh bytes of the event.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            JournalEvent::Fill {
                market,
                side,
                order_kind,
                qty,
                price_e12,
                time,
                avg_entry_after,
                margin,
                sl,
                tp,
                intent_id,
                proof,
            } => {
                put_u8(out, 0);
                put_u16(out, *market);
                put_u8(out, side.as_u8());
                put_u8(out, order_kind.as_u8());
                put_u64(out, *qty);
                put_u64(out, *price_e12);
                put_i64(out, *time);
                put_u64(out, *avg_entry_after);
                put_u64(out, *margin);
                put_u64(out, *sl);
                put_u64(out, *tp);
                put_u32(out, *intent_id);
                put_u8(out, *proof);
            }
            JournalEvent::Close {
                market,
                side,
                qty,
                entry_e12,
                exit_e12,
                time,
                timer_start,
                pnl,
                quick,
                credited,
                reason,
                intent_id,
                proof,
            } => {
                put_u8(out, 1);
                put_u16(out, *market);
                put_u8(out, side.as_u8());
                put_u64(out, *qty);
                put_u64(out, *entry_e12);
                put_u64(out, *exit_e12);
                put_i64(out, *time);
                put_i64(out, *timer_start);
                put_i64(out, *pnl);
                put_bool(out, *quick);
                put_i64(out, *credited);
                put_u8(out, reason.as_u8());
                put_u32(out, *intent_id);
                put_u8(out, *proof);
            }
            JournalEvent::Fee { kind, market, amount, time } => {
                put_u8(out, 2);
                put_u8(out, kind.as_u8());
                put_u16(out, *market);
                put_u64(out, *amount);
                put_i64(out, *time);
            }
            JournalEvent::TightStart { time, p_open, r, l_start, g_peak, l_level, proof } => {
                put_u8(out, 3);
                put_i64(out, *time);
                put_i64(out, *p_open);
                put_i64(out, *r);
                put_i64(out, *l_start);
                put_i64(out, *g_peak);
                put_i64(out, *l_level);
                put_u8(out, *proof);
            }
            JournalEvent::FloorRaised { time, g_peak, l_level, proof } => {
                put_u8(out, 4);
                put_i64(out, *time);
                put_i64(out, *g_peak);
                put_i64(out, *l_level);
                put_u8(out, *proof);
            }
            JournalEvent::TightEnd { time } => {
                put_u8(out, 5);
                put_i64(out, *time);
            }
            JournalEvent::Breach { time, l_level, b_after, proof } => {
                put_u8(out, 6);
                put_i64(out, *time);
                put_i64(out, *l_level);
                put_i64(out, *b_after);
                put_u8(out, *proof);
            }
            JournalEvent::Fail { time, reason, b_after, proof } => {
                put_u8(out, 7);
                put_i64(out, *time);
                put_u8(out, reason.as_u8());
                put_i64(out, *b_after);
                put_u8(out, *proof);
            }
            JournalEvent::Meta { time, flat, samples_hash, intents_hash, engine_version } => {
                put_u8(out, 8);
                put_i64(out, *time);
                put_bool(out, *flat);
                out.extend_from_slice(samples_hash);
                out.extend_from_slice(intents_hash);
                put_u16(out, *engine_version);
            }
        }
    }

    /// The canonical Borsh bytes of the event.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    /// Decodes exactly one event; trailing bytes are an error.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let ev = r.event()?;
        r.finish()?;
        Ok(ev)
    }
}

impl PriceProof {
    /// Appends the canonical Borsh bytes. Fails if `signed_update` exceeds the limit, so every encoding decodes.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        if self.signed_update.len() > MAX_SIGNED_UPDATE_BYTES {
            return Err(EncodeError::SignedUpdateTooLong);
        }
        let len = u32::try_from(self.signed_update.len()).map_err(|_| EncodeError::SignedUpdateTooLong)?;
        put_u8(out, self.source);
        out.extend_from_slice(&self.feed_id);
        put_u64(out, self.timestamp_ns);
        put_i128(out, self.quantized_value);
        put_u32(out, len);
        out.extend_from_slice(&self.signed_update);
        Ok(())
    }

    /// The canonical Borsh bytes of the proof.
    pub fn to_bytes(&self) -> Result<Vec<u8>, EncodeError> {
        let mut out = Vec::new();
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Decodes exactly one proof; trailing bytes are an error.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let p = r.proof()?;
        r.finish()?;
        Ok(p)
    }
}

impl JournalBatch {
    /// The limits a decodable batch satisfies: 1 to 8 events, 0 to 3 proofs, every proof index resolvable.
    /// (Proofs no event refers to are allowed.)
    pub fn validate(&self) -> Result<(), EncodeError> {
        if self.events.is_empty() || self.events.len() > MAX_EVENTS_PER_BATCH {
            return Err(EncodeError::BadEventCount);
        }
        if self.proofs.len() > MAX_PROOFS_PER_BATCH {
            return Err(EncodeError::BadProofCount);
        }
        if self.proofs.iter().any(|p| p.signed_update.len() > MAX_SIGNED_UPDATE_BYTES) {
            return Err(EncodeError::SignedUpdateTooLong);
        }
        for ev in &self.events {
            if let Some(p) = ev.proof_ref() {
                if p != NO_PROOF && usize::from(p) >= self.proofs.len() {
                    return Err(EncodeError::BadProofIndex);
                }
            }
        }
        Ok(())
    }

    /// The canonical Borsh bytes of the batch (after `validate`).
    pub fn to_bytes(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate()?;
        let mut out = Vec::new();
        put_u64(&mut out, self.first_seq);
        out.extend_from_slice(&self.prev_hash);
        let n_events = u32::try_from(self.events.len()).map_err(|_| EncodeError::BadEventCount)?;
        put_u32(&mut out, n_events);
        for ev in &self.events {
            ev.encode_into(&mut out);
        }
        let n_proofs = u32::try_from(self.proofs.len()).map_err(|_| EncodeError::BadProofCount)?;
        put_u32(&mut out, n_proofs);
        for p in &self.proofs {
            p.encode_into(&mut out)?;
        }
        Ok(out)
    }

    /// Decodes untrusted bytes. Rejects trailing bytes, event count 0 or above 8, proof count above 3,
    /// `signed_update` above 512 bytes, an unresolvable proof index, unknown enum values and non-0/1 booleans.
    /// Lengths are checked before anything is allocated, and it never panics.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let first_seq = r.u64()?;
        let prev_hash = r.array32()?;

        let n_events = r.u32()?;
        if n_events == 0 || usize::try_from(n_events).map_or(true, |n| n > MAX_EVENTS_PER_BATCH) {
            return Err(DecodeError::BadEventCount);
        }
        let mut events = Vec::new();
        for _ in 0..n_events {
            events.push(r.event()?);
        }

        let n_proofs = r.u32()?;
        if usize::try_from(n_proofs).map_or(true, |n| n > MAX_PROOFS_PER_BATCH) {
            return Err(DecodeError::BadProofCount);
        }
        let mut proofs = Vec::new();
        for _ in 0..n_proofs {
            proofs.push(r.proof()?);
        }
        r.finish()?;

        let batch = JournalBatch { first_seq, prev_hash, events, proofs };
        batch.validate().map_err(|e| match e {
            EncodeError::BadProofIndex => DecodeError::BadProofIndex,
            EncodeError::BadEventCount => DecodeError::BadEventCount,
            EncodeError::BadProofCount => DecodeError::BadProofCount,
            EncodeError::SignedUpdateTooLong => DecodeError::SignedUpdateTooLong,
        })?;
        Ok(batch)
    }
}

// ------------------------------------------------------------------ reading

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    fn finish(&self) -> Result<(), DecodeError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn array32(&mut self) -> Result<[u8; 32], DecodeError> {
        self.array::<32>()
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(u8::from_le_bytes(self.array::<1>()?))
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array::<2>()?))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.array::<8>()?))
    }
    fn i128(&mut self) -> Result<i128, DecodeError> {
        Ok(i128::from_le_bytes(self.array::<16>()?))
    }
    fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidBool),
        }
    }
    fn side(&mut self) -> Result<Side, DecodeError> {
        Side::try_from(self.u8()?).map_err(enum_err)
    }

    fn event(&mut self) -> Result<JournalEvent, DecodeError> {
        let tag = self.u8()?;
        Ok(match tag {
            0 => JournalEvent::Fill {
                market: self.u16()?,
                side: self.side()?,
                order_kind: OrderKind::try_from(self.u8()?).map_err(enum_err)?,
                qty: self.u64()?,
                price_e12: self.u64()?,
                time: self.i64()?,
                avg_entry_after: self.u64()?,
                margin: self.u64()?,
                sl: self.u64()?,
                tp: self.u64()?,
                intent_id: self.u32()?,
                proof: self.u8()?,
            },
            1 => JournalEvent::Close {
                market: self.u16()?,
                side: self.side()?,
                qty: self.u64()?,
                entry_e12: self.u64()?,
                exit_e12: self.u64()?,
                time: self.i64()?,
                timer_start: self.i64()?,
                pnl: self.i64()?,
                quick: self.bool()?,
                credited: self.i64()?,
                reason: CloseReason::try_from(self.u8()?).map_err(enum_err)?,
                intent_id: self.u32()?,
                proof: self.u8()?,
            },
            2 => JournalEvent::Fee {
                kind: FeeKind::try_from(self.u8()?).map_err(enum_err)?,
                market: self.u16()?,
                amount: self.u64()?,
                time: self.i64()?,
            },
            3 => JournalEvent::TightStart {
                time: self.i64()?,
                p_open: self.i64()?,
                r: self.i64()?,
                l_start: self.i64()?,
                g_peak: self.i64()?,
                l_level: self.i64()?,
                proof: self.u8()?,
            },
            4 => JournalEvent::FloorRaised {
                time: self.i64()?,
                g_peak: self.i64()?,
                l_level: self.i64()?,
                proof: self.u8()?,
            },
            5 => JournalEvent::TightEnd { time: self.i64()? },
            6 => JournalEvent::Breach {
                time: self.i64()?,
                l_level: self.i64()?,
                b_after: self.i64()?,
                proof: self.u8()?,
            },
            7 => JournalEvent::Fail {
                time: self.i64()?,
                reason: FailReason::try_from(self.u8()?).map_err(enum_err)?,
                b_after: self.i64()?,
                proof: self.u8()?,
            },
            8 => JournalEvent::Meta {
                time: self.i64()?,
                flat: self.bool()?,
                samples_hash: self.array32()?,
                intents_hash: self.array32()?,
                engine_version: self.u16()?,
            },
            _ => return Err(DecodeError::UnknownEventTag),
        })
    }

    fn proof(&mut self) -> Result<PriceProof, DecodeError> {
        let source = self.u8()?;
        let feed_id = self.array32()?;
        let timestamp_ns = self.u64()?;
        let quantized_value = self.i128()?;
        let len = self.u32()?;
        if usize::try_from(len).map_or(true, |n| n > MAX_SIGNED_UPDATE_BYTES) {
            return Err(DecodeError::SignedUpdateTooLong);
        }
        let signed_update = self.take(usize::try_from(len).map_err(|_| DecodeError::UnexpectedEof)?)?.to_vec();
        Ok(PriceProof { source, feed_id, timestamp_ns, quantized_value, signed_update })
    }
}

fn enum_err(_: UnknownValue) -> DecodeError {
    DecodeError::UnknownEnumValue
}
