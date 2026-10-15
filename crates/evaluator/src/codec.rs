//! Byte formats the evaluator writes (handover/verifier-formats.md, version 1): the 48-byte `OrderIntent`, the signed
//! message, the intents file lines, the 68-byte sample record and the 16-byte archive header. The verifier crate has its
//! own readers and writers; the dev tests of this crate check the two against each other byte for byte.

pub const ORDER_LEN: usize = 48;
pub const RECORD_LEN: usize = 68;
pub const HEADER_LEN: usize = 16;

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    s
}

/// The kind byte of an `OrderIntent`.
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

/// What a trader signs (48 bytes, little endian): kind u8 @0 | market u16 @1 | side u8 @3 (0 buy/long, 1 sell/short) |
/// qty u64 @4 (kind close: 0 = the whole position) | price_e12 u64 @12 (limit or stop level) | sl_e12 u64 @20 |
/// tp_e12 u64 @28 | target_nonce u32 @36 (cancel) | client_time i64 @40 (unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderIntent {
    pub kind: IntentKind,
    pub market: u16,
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
        put(&mut b, 0, &[self.kind as u8]);
        put(&mut b, 1, &self.market.to_le_bytes());
        put(&mut b, 3, &[self.side]);
        put(&mut b, 4, &self.qty.to_le_bytes());
        put(&mut b, 12, &self.price_e12.to_le_bytes());
        put(&mut b, 20, &self.sl_e12.to_le_bytes());
        put(&mut b, 28, &self.tp_e12.to_le_bytes());
        put(&mut b, 36, &self.target_nonce.to_le_bytes());
        put(&mut b, 40, &self.client_time.to_le_bytes());
        b
    }

    /// Exactly 48 bytes; an unknown kind or a side above 1 is `None`.
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

fn put(buf: &mut [u8], off: usize, src: &[u8]) {
    if let Some(dst) = buf.get_mut(off..off.saturating_add(src.len())) {
        dst.copy_from_slice(src);
    }
}

/// The exact bytes the trader signs: `b"SL8LEV|" ++ account (32) ++ b"|" ++ nonce u32 LE ++ b"|" ++ order`.
pub fn signing_message(account: &[u8; 32], nonce: u32, order: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(45usize.saturating_add(order.len()));
    m.extend_from_slice(b"SL8LEV|");
    m.extend_from_slice(account);
    m.push(b'|');
    m.extend_from_slice(&nonce.to_le_bytes());
    m.push(b'|');
    m.extend_from_slice(order);
    m
}

/// First line of the intents file.
pub fn intents_header_line() -> &'static str {
    "{\"format\":\"setl8-lev-intents\",\"version\":1}\n"
}

/// One data line of the intents file.
pub fn intents_line(nonce: u32, order: &[u8], sig: &[u8]) -> String {
    format!("{{\"nonce\":{nonce},\"order\":\"{}\",\"sig\":\"{}\"}}\n", hex(order), hex(sig))
}

/// One recorded price update (the 68-byte record of the sample archive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRec {
    pub market: u16,
    pub source: u8,
    /// The evaluator's receive time, ns.
    pub recv_ns: u64,
    /// The source's own timestamp, ns (the `PriceProof.timestamp_ns`).
    pub ts_ns: u64,
    /// 18 decimals (the `PriceProof.quantized_value`).
    pub value: i128,
    /// Source 3: the sha256 of the signed blob; zeros otherwise.
    pub digest: [u8; 32],
}

impl SampleRec {
    /// market u16 @0 | source u8 @2 | flags u8 @3 (0) | recv_ns u64 @4 | ts_ns u64 @12 | value i128 @20 | digest @36.
    pub fn to_bytes(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        put(&mut b, 0, &self.market.to_le_bytes());
        put(&mut b, 2, &[self.source]);
        put(&mut b, 4, &self.recv_ns.to_le_bytes());
        put(&mut b, 12, &self.ts_ns.to_le_bytes());
        put(&mut b, 20, &self.value.to_le_bytes());
        put(&mut b, 36, &self.digest);
        b
    }

    /// Source time in whole seconds (floor).
    pub fn sec(&self) -> i64 {
        i64::try_from(self.ts_ns / 1_000_000_000).unwrap_or(i64::MAX)
    }
}

/// The 16-byte segment header: magic `SL8SAMPL` | version u16 LE = 1 | record_len u16 LE = 68 | reserved u32 = 0.
pub fn archive_header() -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    put(&mut h, 0, b"SL8SAMPL");
    put(&mut h, 8, &1u16.to_le_bytes());
    put(&mut h, 10, &(RECORD_LEN as u16).to_le_bytes());
    h
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
    }

    #[test]
    fn decode_rejects_bad_input() {
        let b = intent().to_bytes();
        for bad_kind in [6u8, 255] {
            let mut x = b;
            x[0] = bad_kind;
            assert_eq!(OrderIntent::from_bytes(&x), None);
        }
        let mut x = b;
        x[3] = 2;
        assert_eq!(OrderIntent::from_bytes(&x), None);
        assert_eq!(OrderIntent::from_bytes(&b[..47]), None);
        let mut long = b.to_vec();
        long.push(0);
        assert_eq!(OrderIntent::from_bytes(&long), None);
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
        assert_eq!(m.len(), 47);
    }

    #[test]
    fn intents_lines() {
        assert_eq!(intents_header_line(), "{\"format\":\"setl8-lev-intents\",\"version\":1}\n");
        assert_eq!(intents_line(7, &[0xAB, 0x01], &[0xFF]), "{\"nonce\":7,\"order\":\"ab01\",\"sig\":\"ff\"}\n");
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0, 15, 16, 255]), "000f10ff");
    }

    #[test]
    fn record_byte_table() {
        let r = SampleRec { market: 0x0102, source: 3, recv_ns: 0x1112131415161718, ts_ns: 0x2122232425262728, value: -5, digest: [0xDD; 32] };
        let b = r.to_bytes();
        assert_eq!(b.len(), 68);
        assert_eq!(&b[0..2], &[0x02, 0x01]);
        assert_eq!(b[2], 3);
        assert_eq!(b[3], 0);
        assert_eq!(&b[4..12], &0x1112131415161718u64.to_le_bytes());
        assert_eq!(&b[12..20], &0x2122232425262728u64.to_le_bytes());
        assert_eq!(&b[20..36], &(-5i128).to_le_bytes());
        assert_eq!(&b[36..68], &[0xDD; 32]);
        assert_eq!(r.sec(), 0x2122232425262728 / 1_000_000_000);
    }

    #[test]
    fn header_byte_table() {
        let h = archive_header();
        assert_eq!(&h[..8], b"SL8SAMPL");
        assert_eq!(&h[8..10], &[1, 0]);
        assert_eq!(&h[10..12], &[68, 0]);
        assert_eq!(&h[12..16], &[0, 0, 0, 0]);
    }
}
