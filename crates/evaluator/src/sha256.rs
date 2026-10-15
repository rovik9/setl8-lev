//! SHA-256 (FIPS 180-4), written out so the evaluator core has no crates.io dependency and the prefix hashes of the
//! sample archive and the intents file can be extended record by record (`Sha256State` is `Clone`: a midstate).
//! Tested against the NIST vectors and, in the dev tests, against the `sha2` crate.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
    0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
    0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INIT: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

/// An incremental SHA-256. `Clone` copies the midstate, so a caller can finalize a copy and keep appending.
#[derive(Clone, Debug)]
pub struct Sha256State {
    h: [u32; 8],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Default for Sha256State {
    fn default() -> Self {
        Self::new()
    }
}

fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, chunk) in block.chunks_exact(4).enumerate() {
        let mut word = [0u8; 4];
        word.copy_from_slice(chunk);
        if let Some(slot) = w.get_mut(i) {
            *slot = u32::from_be_bytes(word);
        }
    }
    for i in 16..64 {
        let w15 = w[i - 15];
        let w2 = w[i - 2];
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *slot = slot.wrapping_add(v);
    }
}

impl Sha256State {
    pub fn new() -> Self {
        Sha256State { h: INIT, block: [0; 64], filled: 0, total: 0 }
    }

    /// Appends bytes.
    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.filled > 0 {
            let need = 64 - self.filled;
            let take = need.min(data.len());
            let (head, rest) = data.split_at(take);
            if let Some(dst) = self.block.get_mut(self.filled..self.filled + take) {
                dst.copy_from_slice(head);
            }
            self.filled += take;
            data = rest;
            if self.filled == 64 {
                let block = self.block;
                compress(&mut self.h, &block);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let (head, rest) = data.split_at(64);
            let mut block = [0u8; 64];
            block.copy_from_slice(head);
            compress(&mut self.h, &block);
            data = rest;
        }
        if !data.is_empty() {
            if let Some(dst) = self.block.get_mut(..data.len()) {
                dst.copy_from_slice(data);
            }
            self.filled = data.len();
        }
    }

    /// The digest of everything appended so far; `self` is unchanged (the padding is applied to a copy).
    pub fn finalize(&self) -> [u8; 32] {
        let mut s = self.clone();
        let bit_len = s.total.wrapping_mul(8);
        s.update_padding(bit_len);
        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(s.h.iter()) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn update_padding(&mut self, bit_len: u64) {
        // 0x80, zeros up to 56 mod 64, then the 64-bit big-endian length
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let pad_len = if self.filled < 56 { 56 - self.filled } else { 120 - self.filled };
        let mut tail = [0u8; 72];
        tail[..pad_len].copy_from_slice(&pad[..pad_len]);
        tail[pad_len..pad_len + 8].copy_from_slice(&bit_len.to_be_bytes());
        let total = self.total;
        self.update(&tail[..pad_len + 8]);
        self.total = total;
    }
}

/// SHA-256 of the concatenation of `parts` (the shape of `journal_types::Sha256::hashv`).
pub fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut s = Sha256State::new();
    for p in parts {
        s.update(p);
    }
    s.finalize()
}

/// SHA-256 of one slice.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    sha256_parts(&[data])
}

/// The hasher handed to `journal_types` (`Sha256` is implemented for closures of this shape).
pub fn chain_hasher() -> impl Fn(&[&[u8]]) -> [u8; 32] {
    |parts: &[&[u8]]| sha256_parts(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; 32]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn nist_vectors() {
        assert_eq!(hex(sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex(sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million_a = vec![b'a'; 1_000_000];
        assert_eq!(hex(sha256(&million_a)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn padding_boundaries() {
        // lengths around the 55/56/63/64 byte padding edges, against known digests computed with sha2 (see the dev test)
        for n in [0usize, 1, 54, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 129] {
            let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
            let mut inc = Sha256State::new();
            for chunk in data.chunks(7) {
                inc.update(chunk);
            }
            assert_eq!(inc.finalize(), sha256(&data), "incremental vs one shot at {n}");
        }
    }

    #[test]
    fn finalize_does_not_consume_the_state() {
        let mut s = Sha256State::new();
        s.update(b"ab");
        let first = s.finalize();
        assert_eq!(first, sha256(b"ab"));
        s.update(b"c");
        assert_eq!(s.finalize(), sha256(b"abc"));
        assert_eq!(s.finalize(), sha256(b"abc"));
    }

    #[test]
    fn parts_equal_the_concatenation() {
        assert_eq!(sha256_parts(&[b"a", b"", b"bc"]), sha256(b"abc"));
        let h = chain_hasher();
        assert_eq!(h(&[b"abc"]), sha256(b"abc"));
    }
}
