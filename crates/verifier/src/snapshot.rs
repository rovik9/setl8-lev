//! The account snapshot (format version 1): the stored `TradingAccount` fields the replay is compared with.
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub b: i64,
    pub q_tally: i64,
    pub latch: u8,
    pub tight_on: u8,
    pub r: i64,
    pub g_peak: i64,
    pub l_level: i64,
    pub lock_until: i64,
    pub seq: u64,
    /// Hex, 32 bytes.
    pub journal_hash: String,
    pub flat: u8,
    pub phase: u8,
    pub status: u8,
    pub trades_closed: u32,
}

impl Snapshot {
    pub fn parse(bytes: &[u8]) -> Result<Snapshot, String> {
        let s: Snapshot = serde_json::from_slice(bytes).map_err(|e| format!("snapshot: {e}"))?;
        if s.version != 1 {
            return Err(format!("snapshot version {} is not supported (1)", s.version));
        }
        crate::hexutil::decode_array::<32>(&s.journal_hash).map_err(|e| format!("snapshot journal_hash: {e}"))?;
        Ok(s)
    }

    pub fn hash(&self) -> [u8; 32] {
        crate::hexutil::decode_array::<32>(&self.journal_hash).unwrap_or([0u8; 32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snapshot() {
        let h = crate::hexutil::encode(&[1u8; 32]);
        let j = format!(r#"{{"version":1,"b":5,"q_tally":0,"latch":0,"tight_on":0,"r":0,"g_peak":0,"l_level":0,"lock_until":0,"seq":3,"journal_hash":"{h}","flat":1,"phase":2,"status":0,"trades_closed":1}}"#);
        let s = Snapshot::parse(j.as_bytes()).unwrap();
        assert_eq!((s.b, s.seq, s.phase), (5, 3, 2));
        assert_eq!(s.hash(), [1u8; 32]);
        assert!(Snapshot::parse(j.replace("\"version\":1", "\"version\":2").as_bytes()).is_err());
        assert!(Snapshot::parse(j.replace(&h, "00").as_bytes()).is_err());
        assert!(Snapshot::parse(b"{}").is_err());
    }
}
