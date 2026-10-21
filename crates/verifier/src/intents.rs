//! Signed order intents (format version 1).
//!
//! File: JSON Lines. Line 1 is the header `{"format":"setl8-lev-intents","version":1}`; every other line is
//! `{"nonce":u32,"order":hex,"sig":hex}`. `Meta.intents_hash` is the sha256 of a PREFIX of the file's bytes that ends at a
//! line boundary (just after a `\n`, or the end of the file).
//!
//! `order` is the canonical `OrderIntent` (48 bytes, little-endian):
//! kind u8 @0 (0 market, 1 limit, 2 stop, 3 close, 4 cancel, 5 set_sl_tp) | market u16 @1 | side u8 @3 (0 buy/long,
//! 1 sell/short) | qty u64 @4 | price_e12 u64 @12 (limit or stop level) | sl_e12 u64 @20 | tp_e12 u64 @28 |
//! target_nonce u32 @36 (cancel: the order to cancel) | client_time i64 @40 (unix seconds).
//!
//! The signed message is `b"SL8LEV|" ++ account (32) ++ b"|" ++ nonce u32 LE ++ b"|" ++ order bytes`; the signature is
//! ed25519 by the TRADER's key. Nonce 0 is reserved ("no intent" in the journal).
use ed25519_dalek::{PublicKey, Signature};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const ORDER_LEN: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentKind {
    Market = 0,
    Limit = 1,
    Stop = 2,
    Close = 3,
    Cancel = 4,
    SetSlTp = 5,
}

impl IntentKind {
    pub fn from_u8(v: u8) -> Option<IntentKind> {
        Some(match v {
            0 => IntentKind::Market,
            1 => IntentKind::Limit,
            2 => IntentKind::Stop,
            3 => IntentKind::Close,
            4 => IntentKind::Cancel,
            5 => IntentKind::SetSlTp,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderIntent {
    pub kind: IntentKind,
    pub market: u16,
    /// 0 buy/long, 1 sell/short.
    pub side: u8,
    pub qty: u64,
    pub price_e12: u64,
    pub sl_e12: u64,
    pub tp_e12: u64,
    pub target_nonce: u32,
    pub client_time: i64,
}

impl OrderIntent {
    pub fn to_bytes(&self) -> [u8; ORDER_LEN] {
        let mut b = [0u8; ORDER_LEN];
        let mut put = |off: usize, src: &[u8]| {
            if let Some(dst) = b.get_mut(off..off + src.len()) {
                dst.copy_from_slice(src);
            }
        };
        put(0, &[self.kind as u8]);
        put(1, &self.market.to_le_bytes());
        put(3, &[self.side]);
        put(4, &self.qty.to_le_bytes());
        put(12, &self.price_e12.to_le_bytes());
        put(20, &self.sl_e12.to_le_bytes());
        put(28, &self.tp_e12.to_le_bytes());
        put(36, &self.target_nonce.to_le_bytes());
        put(40, &self.client_time.to_le_bytes());
        b
    }

    /// Decodes exactly 48 bytes; an unknown kind or a side above 1 is `None`.
    pub fn from_bytes(b: &[u8]) -> Option<OrderIntent> {
        if b.len() != ORDER_LEN {
            return None;
        }
        let side = *b.get(3)?;
        if side > 1 {
            return None;
        }
        Some(OrderIntent {
            kind: IntentKind::from_u8(*b.first()?)?,
            market: u16::from_le_bytes(b.get(1..3)?.try_into().ok()?),
            side,
            qty: u64::from_le_bytes(b.get(4..12)?.try_into().ok()?),
            price_e12: u64::from_le_bytes(b.get(12..20)?.try_into().ok()?),
            sl_e12: u64::from_le_bytes(b.get(20..28)?.try_into().ok()?),
            tp_e12: u64::from_le_bytes(b.get(28..36)?.try_into().ok()?),
            target_nonce: u32::from_le_bytes(b.get(36..40)?.try_into().ok()?),
            client_time: i64::from_le_bytes(b.get(40..48)?.try_into().ok()?),
        })
    }
}

/// The exact bytes the trader signs.
pub fn signing_message(account: &[u8; 32], nonce: u32, order: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(7 + 32 + 1 + 4 + 1 + order.len());
    m.extend_from_slice(b"SL8LEV|");
    m.extend_from_slice(account);
    m.push(b'|');
    m.extend_from_slice(&nonce.to_le_bytes());
    m.push(b'|');
    m.extend_from_slice(order);
    m
}

/// Verifies the ed25519 signature of an intent by `trader`.
pub fn verify_signature(trader: &[u8; 32], account: &[u8; 32], nonce: u32, order: &[u8], sig: &[u8]) -> bool {
    let Ok(pk) = PublicKey::from_bytes(trader) else { return false };
    let Ok(sig) = Signature::from_bytes(sig) else { return false };
    // verify_strict: rejects a small-order key and a non-canonical signature (M12 findings VRF-2, V-15)
    pk.verify_strict(&signing_message(account, nonce, order), &sig).is_ok()
}

/// One data line of the intents file.
#[derive(Clone, Debug)]
pub struct IntentLine {
    pub nonce: u32,
    pub order: Vec<u8>,
    pub sig: Vec<u8>,
    /// Byte offset just after this line (and its newline).
    pub end_offset: usize,
    /// The decoded order, or why the line is unusable.
    pub intent: Result<OrderIntent, String>,
}

#[derive(Clone, Debug)]
pub struct IntentFile {
    pub lines: Vec<IntentLine>,
    pub raw: Vec<u8>,
    /// Byte offset just after the header line.
    pub header_end: usize,
    /// First occurrence of each nonce (an index into `lines`).
    pub by_nonce: HashMap<u32, usize>,
    /// Indexes of lines whose nonce repeats an earlier one.
    pub duplicates: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentFileError(pub String);

impl IntentFile {
    pub fn parse(bytes: &[u8]) -> Result<IntentFile, IntentFileError> {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut header_end = None;
        for raw_line in bytes.split_inclusive(|&b| b == b'\n') {
            let end = offset + raw_line.len();
            let text = std::str::from_utf8(raw_line).map(|t| t.trim()).unwrap_or("\u{fffd}");
            offset = end;
            if text.is_empty() {
                continue;
            }
            if header_end.is_none() {
                let v: Value = serde_json::from_str(text).map_err(|e| IntentFileError(format!("intents header: {e}")))?;
                let ok = v.get("format").and_then(Value::as_str) == Some("setl8-lev-intents") && v.get("version").and_then(Value::as_u64) == Some(1);
                if !ok {
                    return Err(IntentFileError("intents header must be {\"format\":\"setl8-lev-intents\",\"version\":1}".into()));
                }
                header_end = Some(end);
                continue;
            }
            lines.push(parse_line(text, end));
        }
        let header_end = header_end.ok_or_else(|| IntentFileError("intents file has no header line".into()))?;
        let mut by_nonce = HashMap::new();
        let mut duplicates = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if by_nonce.insert(l.nonce, i).is_some() {
                duplicates.push(i);
                // keep the FIRST occurrence
                if let Some(first) = lines.iter().position(|x| x.nonce == l.nonce) {
                    by_nonce.insert(l.nonce, first);
                }
            }
        }
        Ok(IntentFile { lines, raw: bytes.to_vec(), header_end, by_nonce, duplicates })
    }

    /// Number of data lines inside the first `len` bytes.
    pub fn lines_in_prefix(&self, len: usize) -> usize {
        self.lines.iter().take_while(|l| l.end_offset <= len).count()
    }

    pub fn get(&self, nonce: u32) -> Option<(usize, &IntentLine)> {
        let i = *self.by_nonce.get(&nonce)?;
        self.lines.get(i).map(|l| (i, l))
    }
}

fn parse_line(text: &str, end_offset: usize) -> IntentLine {
    let bad = |why: String| IntentLine { nonce: 0, order: vec![], sig: vec![], end_offset, intent: Err(why) };
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return bad(format!("not JSON: {e}")),
    };
    let nonce = match v.get("nonce").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()) {
        Some(n) => n,
        None => return bad("missing or invalid nonce".into()),
    };
    let hex_field = |name: &str| v.get(name).and_then(Value::as_str).map(crate::hexutil::decode);
    let order = match hex_field("order") {
        Some(Ok(o)) => o,
        _ => return IntentLine { nonce, order: vec![], sig: vec![], end_offset, intent: Err("missing or invalid order hex".into()) },
    };
    let sig = match hex_field("sig") {
        Some(Ok(s)) => s,
        _ => return IntentLine { nonce, order, sig: vec![], end_offset, intent: Err("missing or invalid sig hex".into()) },
    };
    let intent = OrderIntent::from_bytes(&order).ok_or_else(|| "order bytes are not a valid OrderIntent".to_string());
    IntentLine { nonce, order, sig, end_offset, intent }
}

/// Writes an intents file the way the evaluator does (M10 uses it).
pub fn header_line() -> String {
    "{\"format\":\"setl8-lev-intents\",\"version\":1}\n".to_string()
}

pub fn line_for(nonce: u32, order: &[u8], sig: &[u8]) -> String {
    format!("{{\"nonce\":{nonce},\"order\":\"{}\",\"sig\":\"{}\"}}\n", crate::hexutil::encode(order), crate::hexutil::encode(sig))
}

/// One pass over the file: for every line boundary (offset 0 and the header end included, and the end of file) test
/// whether the sha256 of the prefix is in `wanted`. Returns hash -> prefix length in BYTES.
pub fn find_prefixes(raw: &[u8], wanted: &HashSet<[u8; 32]>) -> HashMap<[u8; 32], usize> {
    let mut found = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    let mut h = Sha256::new();
    let check = |h: &Sha256, len: usize, found: &mut HashMap<[u8; 32], usize>| {
        let d: [u8; 32] = h.clone().finalize().into();
        if wanted.contains(&d) {
            found.entry(d).or_insert(len);
        }
    };
    check(&h, 0, &mut found);
    let mut len = 0usize;
    for chunk in raw.split_inclusive(|&b| b == b'\n') {
        if found.len() == wanted.len() {
            break;
        }
        h.update(chunk);
        len += chunk.len();
        check(&h, len, &mut found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> OrderIntent {
        OrderIntent { kind: IntentKind::Limit, market: 0x0102, side: 1, qty: 0x1112131415161718, price_e12: 7, sl_e12: 8, tp_e12: 9, target_nonce: 0x0A0B0C0D, client_time: -5 }
    }

    #[test]
    fn order_byte_table() {
        let b = intent().to_bytes();
        assert_eq!(b.len(), 48);
        assert_eq!(b[0], 1);
        assert_eq!(&b[1..3], &[0x02, 0x01]);
        assert_eq!(b[3], 1);
        assert_eq!(&b[4..12], &0x1112131415161718u64.to_le_bytes());
        assert_eq!(&b[12..20], &7u64.to_le_bytes());
        assert_eq!(&b[20..28], &8u64.to_le_bytes());
        assert_eq!(&b[28..36], &9u64.to_le_bytes());
        assert_eq!(&b[36..40], &0x0A0B0C0Du32.to_le_bytes());
        assert_eq!(&b[40..48], &(-5i64).to_le_bytes());
        assert_eq!(OrderIntent::from_bytes(&b), Some(intent()));
        for bad_kind in [6u8, 255] {
            let mut x = b;
            x[0] = bad_kind;
            assert_eq!(OrderIntent::from_bytes(&x), None);
        }
        let mut x = b;
        x[3] = 2;
        assert_eq!(OrderIntent::from_bytes(&x), None);
        assert_eq!(OrderIntent::from_bytes(&b[..47]), None);
    }

    #[test]
    fn every_kind_round_trips() {
        for k in 0u8..=5 {
            let kind = IntentKind::from_u8(k).unwrap();
            assert_eq!(kind as u8, k);
            let mut i = intent();
            i.kind = kind;
            assert_eq!(OrderIntent::from_bytes(&i.to_bytes()), Some(i));
        }
        assert_eq!(IntentKind::from_u8(6), None);
    }

    #[test]
    fn signing_message_layout() {
        let m = signing_message(&[0xAA; 32], 0x01020304, &[9, 9]);
        assert_eq!(&m[..7], b"SL8LEV|");
        assert_eq!(&m[7..39], &[0xAA; 32]);
        assert_eq!(m[39], b'|');
        assert_eq!(&m[40..44], &[4, 3, 2, 1]);
        assert_eq!(m[44], b'|');
        assert_eq!(&m[45..], &[9, 9]);
    }

    #[test]
    fn signature_round_trip_and_tamper() {
        use ed25519_dalek::{Keypair, Signer};
        let secret = ed25519_dalek::SecretKey::from_bytes(&[5u8; 32]).unwrap();
        let public: PublicKey = (&secret).into();
        let kp = Keypair { secret, public };
        let account = [3u8; 32];
        let order = intent().to_bytes();
        let sig = kp.sign(&signing_message(&account, 7, &order)).to_bytes();
        let pk = public.to_bytes();
        assert!(verify_signature(&pk, &account, 7, &order, &sig));
        assert!(!verify_signature(&pk, &account, 8, &order, &sig), "nonce is signed");
        assert!(!verify_signature(&pk, &[4u8; 32], 7, &order, &sig), "account is signed");
        let mut other = order;
        other[4] ^= 1;
        assert!(!verify_signature(&pk, &account, 7, &other, &sig));
        assert!(!verify_signature(&pk, &account, 7, &order, &sig[..63]));
        assert!(!verify_signature(&[0u8; 32], &account, 7, &order, &sig));
    }

    #[test]
    fn file_parse_prefixes_and_errors() {
        let o = intent().to_bytes();
        let text = format!("{}{}{}", header_line(), line_for(1, &o, &[0; 64]), line_for(2, &o, &[0; 64]));
        let f = IntentFile::parse(text.as_bytes()).unwrap();
        assert_eq!(f.lines.len(), 2);
        assert_eq!(f.get(2).unwrap().0, 1);
        assert_eq!(f.lines_in_prefix(f.header_end), 0);
        assert_eq!(f.lines_in_prefix(f.lines[0].end_offset), 1);
        assert_eq!(f.lines_in_prefix(text.len()), 2);
        let mut wanted = HashSet::new();
        let h = |n: usize| -> [u8; 32] { Sha256::digest(&text.as_bytes()[..n]).into() };
        wanted.insert(h(f.lines[0].end_offset));
        wanted.insert(h(text.len()));
        wanted.insert(h(0));
        wanted.insert(h(5));
        let got = find_prefixes(text.as_bytes(), &wanted);
        assert_eq!(got.get(&h(0)), Some(&0));
        assert_eq!(got.get(&h(f.lines[0].end_offset)), Some(&f.lines[0].end_offset));
        assert_eq!(got.get(&h(text.len())), Some(&text.len()));
        assert_eq!(got.get(&h(5)), None, "not a line boundary");
        assert!(IntentFile::parse(b"").is_err());
        assert!(IntentFile::parse(b"{\"format\":\"x\",\"version\":1}\n").is_err());
        let dup = format!("{}{}{}", header_line(), line_for(1, &o, &[0; 64]), line_for(1, &o, &[0; 64]));
        let f = IntentFile::parse(dup.as_bytes()).unwrap();
        assert_eq!(f.duplicates, vec![1]);
        let junk = format!("{}not json\n{{\"nonce\":1}}\n", header_line());
        let f = IntentFile::parse(junk.as_bytes()).unwrap();
        assert!(f.lines.iter().all(|l| l.intent.is_err()));
    }
}
