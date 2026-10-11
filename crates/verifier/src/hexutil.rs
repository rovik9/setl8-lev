//! Tiny hex helpers (no dependency).

pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes an even-length hex string (either case). An empty string is an empty vector.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", b.len()));
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        match (pair.first().copied().and_then(nibble), pair.get(1).copied().and_then(nibble)) {
            (Some(h), Some(l)) => out.push((h << 4) | l),
            _ => return Err("invalid hex digit".to_string()),
        }
    }
    Ok(out)
}

pub fn decode_array<const N: usize>(s: &str) -> Result<[u8; N], String> {
    let v = decode(s)?;
    <[u8; N]>::try_from(v.as_slice()).map_err(|_| format!("expected {N} bytes, got {}", v.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_errors() {
        assert_eq!(encode(&[0, 1, 0xab, 0xff]), "0001abff");
        assert_eq!(decode("0001ABff").unwrap(), vec![0, 1, 0xab, 0xff]);
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
        assert!(decode("abc").is_err());
        assert!(decode("zz").is_err());
        assert!(decode_array::<2>("0102").is_ok());
        assert!(decode_array::<2>("010203").is_err());
    }
}
