//! The sample archive (format version 1): an append-only logical stream of fixed-width little-endian records.
//!
//! File = one or more SEGMENTS, each `header (16 bytes)` + `records`. The logical stream, and what `Meta.samples_hash`
//! commits to, is the concatenation of the RECORD bytes only (headers excluded). A record never starts with the
//! header magic because its `flags` byte (offset 3) is always 0 and the magic's fourth byte is not.
//!
//! Header (16): magic `SL8SAMPL` (8) | version u16 LE = 1 | record_len u16 LE = 68 | reserved u32 = 0.
//!
//! Record (68): market u16 @0 | source u8 @2 | flags u8 @3 (0) | recv_ns u64 @4 | source_ts_ns u64 @12 |
//! quantized_value i128 @20 (18 decimals, the number a `PriceProof` carries) | blob_digest [32] @36 (source 3: sha256 of
//! the signed blob; zeros otherwise).
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const MAGIC: &[u8; 8] = b"SL8SAMPL";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 16;
pub const RECORD_LEN: usize = 68;

/// One recorded price update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub market: u16,
    pub source: u8,
    /// The evaluator's receive time, ns.
    pub recv_ns: u64,
    /// The source's own timestamp, ns (the `PriceProof.timestamp_ns`).
    pub ts_ns: u64,
    pub value: i128,
    pub digest: [u8; 32],
}

impl Sample {
    pub fn to_bytes(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        let mut put = |off: usize, src: &[u8]| {
            if let Some(dst) = b.get_mut(off..off + src.len()) {
                dst.copy_from_slice(src);
            }
        };
        put(0, &self.market.to_le_bytes());
        put(2, &[self.source]);
        put(4, &self.recv_ns.to_le_bytes());
        put(12, &self.ts_ns.to_le_bytes());
        put(20, &self.value.to_le_bytes());
        put(36, &self.digest);
        b
    }

    /// Decodes one record; `None` for a wrong length or a non-zero `flags` byte.
    pub fn from_bytes(b: &[u8]) -> Option<Sample> {
        if b.len() != RECORD_LEN || *b.get(3)? != 0 {
            return None;
        }
        Some(Sample {
            market: u16::from_le_bytes(b.get(0..2)?.try_into().ok()?),
            source: *b.get(2)?,
            recv_ns: u64::from_le_bytes(b.get(4..12)?.try_into().ok()?),
            ts_ns: u64::from_le_bytes(b.get(12..20)?.try_into().ok()?),
            value: i128::from_le_bytes(b.get(20..36)?.try_into().ok()?),
            digest: b.get(36..68)?.try_into().ok()?,
        })
    }

    /// Source time in whole seconds (floor).
    pub fn ts_sec(&self) -> i64 {
        i64::try_from(self.ts_ns / 1_000_000_000).unwrap_or(i64::MAX)
    }

    /// `floor(value / 10^6)`, the e12 mid; `None` for a non-positive value or one beyond `u64`.
    pub fn mid_e12(&self) -> Option<u64> {
        journal_types::price_e12_from_quantized(self.value).ok()
    }
}

pub fn header_bytes() -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_le_bytes());
    h[10..12].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    h
}

/// Appends records (and segment headers) the way the evaluator does. M10 uses this writer.
#[derive(Clone, Debug)]
pub struct ArchiveWriter {
    buf: Vec<u8>,
}

impl Default for ArchiveWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveWriter {
    /// A new archive with its first segment header.
    pub fn new() -> Self {
        ArchiveWriter { buf: header_bytes().to_vec() }
    }

    /// Starts a new segment (for example a new UTC day): writes a header; the logical stream is unaffected.
    pub fn start_segment(&mut self) {
        self.buf.extend_from_slice(&header_bytes());
    }

    pub fn push(&mut self, s: &Sample) {
        self.buf.extend_from_slice(&s.to_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    MissingHeader,
    BadHeader(String),
    Truncated { offset: usize },
    BadRecord { offset: usize },
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::MissingHeader => write!(f, "archive does not start with the SL8SAMPL header"),
            ArchiveError::BadHeader(s) => write!(f, "bad archive header: {s}"),
            ArchiveError::Truncated { offset } => write!(f, "archive truncated at byte {offset}"),
            ArchiveError::BadRecord { offset } => write!(f, "bad archive record at byte {offset}"),
        }
    }
}

/// A parsed archive.
#[derive(Clone, Debug)]
pub struct Archive {
    pub samples: Vec<Sample>,
    /// The logical stream: the record bytes concatenated, headers removed (what `samples_hash` hashes).
    pub stream: Vec<u8>,
    /// Sample indexes per market, in stream order.
    pub by_market: BTreeMap<u16, Vec<usize>>,
    index: HashMap<(u16, u64, i128), usize>,
}

impl Archive {
    pub fn parse(bytes: &[u8]) -> Result<Archive, ArchiveError> {
        if !bytes.starts_with(MAGIC) {
            return Err(ArchiveError::MissingHeader);
        }
        let mut samples = Vec::new();
        let mut stream = Vec::new();
        let mut pos = 0usize;
        while pos < bytes.len() {
            let rest = bytes.get(pos..).unwrap_or(&[]);
            if rest.starts_with(MAGIC) {
                let h = rest.get(..HEADER_LEN).ok_or(ArchiveError::Truncated { offset: pos })?;
                let version = u16::from_le_bytes([h[8], h[9]]);
                let rec_len = u16::from_le_bytes([h[10], h[11]]);
                let reserved = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
                if version != VERSION || usize::from(rec_len) != RECORD_LEN || reserved != 0 {
                    return Err(ArchiveError::BadHeader(format!("version {version}, record_len {rec_len}, reserved {reserved}")));
                }
                pos += HEADER_LEN;
                continue;
            }
            let rec = rest.get(..RECORD_LEN).ok_or(ArchiveError::Truncated { offset: pos })?;
            let s = Sample::from_bytes(rec).ok_or(ArchiveError::BadRecord { offset: pos })?;
            samples.push(s);
            stream.extend_from_slice(rec);
            pos += RECORD_LEN;
        }
        let mut by_market: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        let mut index = HashMap::new();
        for (i, s) in samples.iter().enumerate() {
            by_market.entry(s.market).or_default().push(i);
            index.entry((s.market, s.ts_ns, s.value)).or_insert(i);
        }
        Ok(Archive { samples, stream, by_market, index })
    }

    /// The first sample of `market` with this source timestamp and value.
    pub fn find(&self, market: u16, ts_ns: u64, value: i128) -> Option<usize> {
        self.index.get(&(market, ts_ns, value)).copied()
    }

    /// The last sample of `market` (in stream order) whose source second is `<= sec`.
    pub fn last_at_or_before(&self, market: u16, sec: i64) -> Option<usize> {
        let list = self.by_market.get(&market)?;
        let n = list.partition_point(|&i| self.samples.get(i).is_some_and(|s| s.ts_sec() <= sec));
        n.checked_sub(1).and_then(|k| list.get(k)).copied()
    }

    /// The first sample of `market` whose source timestamp is `>= ts_ns` (used to anchor a proof the second archive lacks).
    pub fn first_at_or_after(&self, market: u16, ts_ns: u64) -> Option<usize> {
        let list = self.by_market.get(&market)?;
        let n = list.partition_point(|&i| self.samples.get(i).is_some_and(|s| s.ts_ns < ts_ns));
        list.get(n).copied()
    }
}

fn hash_of(h: &Sha256) -> [u8; 32] {
    h.clone().finalize().into()
}

/// One pass over the stream: for every record boundary (0 records included) test whether the sha256 of the prefix is one
/// of `wanted`. Returns hash -> number of records. O(n) hashing, O(|wanted|) memory; stops early when all are found.
pub fn find_prefixes(stream: &[u8], wanted: &HashSet<[u8; 32]>) -> HashMap<[u8; 32], usize> {
    let mut found = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    let mut h = Sha256::new();
    let empty = hash_of(&h);
    if wanted.contains(&empty) {
        found.insert(empty, 0);
    }
    for (n, rec) in stream.chunks_exact(RECORD_LEN).enumerate() {
        if found.len() == wanted.len() {
            break;
        }
        h.update(rec);
        let d = hash_of(&h);
        if wanted.contains(&d) {
            found.entry(d).or_insert(n + 1);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(market: u16, ts: u64, v: i128) -> Sample {
        Sample { market, source: 2, recv_ns: ts + 5, ts_ns: ts, value: v, digest: [0; 32] }
    }

    #[test]
    fn record_byte_table() {
        let r = Sample { market: 0x0102, source: 3, recv_ns: 0x1112131415161718, ts_ns: 0x2122232425262728, value: -5, digest: [0xDD; 32] };
        let b = r.to_bytes();
        assert_eq!(b.len(), 68);
        assert_eq!(&b[0..2], &[0x02, 0x01]);
        assert_eq!(b[2], 3);
        assert_eq!(b[3], 0);
        assert_eq!(&b[4..12], &0x1112131415161718u64.to_le_bytes());
        assert_eq!(&b[12..20], &0x2122232425262728u64.to_le_bytes());
        assert_eq!(&b[20..36], &(-5i128).to_le_bytes());
        assert_eq!(&b[36..68], &[0xDD; 32]);
        assert_eq!(Sample::from_bytes(&b), Some(r));
        let mut bad = b;
        bad[3] = 1;
        assert_eq!(Sample::from_bytes(&bad), None);
        assert_eq!(Sample::from_bytes(&b[..67]), None);
    }

    #[test]
    fn header_byte_table() {
        let h = header_bytes();
        assert_eq!(&h[..8], b"SL8SAMPL");
        assert_eq!(&h[8..10], &[1, 0]);
        assert_eq!(&h[10..12], &[68, 0]);
        assert_eq!(&h[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn segments_concatenate_and_headers_are_not_hashed() {
        let mut w = ArchiveWriter::new();
        w.push(&s(0, 1_000_000_000, 5));
        w.start_segment();
        w.push(&s(1, 2_000_000_000, 6));
        let a = Archive::parse(&w.into_bytes()).unwrap();
        assert_eq!(a.samples.len(), 2);
        assert_eq!(a.stream.len(), 2 * RECORD_LEN);
        assert_eq!(a.by_market.get(&0), Some(&vec![0]));
        assert_eq!(a.find(1, 2_000_000_000, 6), Some(1));
        assert_eq!(a.find(1, 2_000_000_000, 7), None);
    }

    #[test]
    fn parse_errors() {
        assert_eq!(Archive::parse(&[]).unwrap_err(), ArchiveError::MissingHeader);
        assert_eq!(Archive::parse(b"nonsense").unwrap_err(), ArchiveError::MissingHeader);
        let mut w = ArchiveWriter::new();
        w.push(&s(0, 1, 1));
        let good = w.into_bytes();
        assert!(matches!(Archive::parse(&good[..good.len() - 1]), Err(ArchiveError::Truncated { .. })));
        assert!(matches!(Archive::parse(&good[..10]), Err(ArchiveError::Truncated { .. })));
        let mut bad = good.clone();
        bad[HEADER_LEN + 3] = 9;
        assert!(matches!(Archive::parse(&bad), Err(ArchiveError::BadRecord { .. })));
        let mut bad = good;
        bad[8] = 2;
        assert!(matches!(Archive::parse(&bad), Err(ArchiveError::BadHeader(_))));
    }

    #[test]
    fn mark_lookup_by_time() {
        let mut w = ArchiveWriter::new();
        for (i, t) in [10u64, 20, 20, 30].iter().enumerate() {
            w.push(&s(0, t * 1_000_000_000, i as i128 + 1));
        }
        let a = Archive::parse(&w.into_bytes()).unwrap();
        assert_eq!(a.last_at_or_before(0, 9), None);
        assert_eq!(a.last_at_or_before(0, 10), Some(0));
        assert_eq!(a.last_at_or_before(0, 20), Some(2), "the LAST sample of that second");
        assert_eq!(a.last_at_or_before(0, 99), Some(3));
        assert_eq!(a.last_at_or_before(7, 99), None);
        assert_eq!(a.first_at_or_after(0, 20_000_000_000), Some(1));
        assert_eq!(a.first_at_or_after(0, 31_000_000_000), None);
    }

    #[test]
    fn prefix_search_finds_every_boundary_in_one_pass() {
        let mut w = ArchiveWriter::new();
        for t in 1..=5u64 {
            w.push(&s(0, t * 1_000_000_000, t as i128));
        }
        let a = Archive::parse(&w.into_bytes()).unwrap();
        let mut wanted = HashSet::new();
        let at = |n: usize| -> [u8; 32] { Sha256::digest(&a.stream[..n * RECORD_LEN]).into() };
        for n in [0usize, 1, 3, 5] {
            wanted.insert(at(n));
        }
        let junk = [7u8; 32];
        wanted.insert(junk);
        let got = find_prefixes(&a.stream, &wanted);
        assert_eq!(got.get(&at(0)), Some(&0));
        assert_eq!(got.get(&at(1)), Some(&1));
        assert_eq!(got.get(&at(3)), Some(&3));
        assert_eq!(got.get(&at(5)), Some(&5));
        assert_eq!(got.get(&junk), None);
        assert!(find_prefixes(&a.stream, &HashSet::new()).is_empty());
    }
}
