//! sha256 - FIPS 180-4 SHA-256, compact: the 64-word constant table and a rolled 64-round loop
//! over a 16-word circular message schedule (the bootloader image check and `fwimage` share it).

/// BLOCK_LEN -- bytes in one message block.
const BLOCK_LEN: usize = 64;
/// LENGTH_FIELD_AT -- where the padding writes the 64-bit message length in bits (big-endian).
const LENGTH_FIELD_AT: usize = BLOCK_LEN - 8;

/// INITIAL -- the initial hash value H(0) (FIPS 180-4 section 5.3.3).
const INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// K -- the 64 round constants (FIPS 180-4 section 4.2.2).
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Sha256 -- FIPS 180-4 SHA-256, streaming: `new`, `update` any number of times, `finish`.
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; BLOCK_LEN],
    used: usize,
    /// Message bytes so far.
    length: u64,
}

impl Sha256 {
    /// new -- a hash of the empty message so far.
    pub const fn new() -> Self {
        Sha256 {
            state: INITIAL,
            block: [0; BLOCK_LEN],
            used: 0,
            length: 0,
        }
    }

    /// update -- appends `data` to the message.
    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let mut rest = data;
        if self.used > 0 {
            let take = (BLOCK_LEN - self.used).min(rest.len());
            let (head, tail) = rest.split_at(take);
            self.fill(head);
            rest = tail;
            if self.used < BLOCK_LEN {
                return;
            }
            compress(&mut self.state, &self.block);
            self.used = 0;
        }
        let (blocks, tail) = rest.as_chunks::<BLOCK_LEN>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.fill(tail);
    }

    /// fill -- copies `bytes` (which fit) into the block buffer after the used part.
    fn fill(&mut self, bytes: &[u8]) {
        if let Some(room) = self.block.get_mut(self.used..self.used + bytes.len()) {
            room.copy_from_slice(bytes);
            self.used += bytes.len();
        }
    }

    /// finish -- pads the message (80h, zeros, the bit length big-endian) and returns the digest.
    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != LENGTH_FIELD_AT {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut digest = [0u8; 32];
        let (words, _) = digest.as_chunks_mut::<4>();
        for (out, word) in words.iter_mut().zip(self.state) {
            *out = word.to_be_bytes();
        }
        digest
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// sha256 -- the digest of one buffer.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(data);
    hash.finish()
}

/// compress -- one 64-byte block into `state` (FIPS 180-4 section 6.2.2), the schedule kept in 16
/// circular words: word i replaces word i - 16 in place.
fn compress(state: &mut [u32; 8], block: &[u8; BLOCK_LEN]) {
    let mut w = [0u32; 16];
    for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (i, k) in K.iter().enumerate() {
        if i >= 16 {
            let s0 = w[(i + 1) & 15].rotate_right(7)
                ^ w[(i + 1) & 15].rotate_right(18)
                ^ (w[(i + 1) & 15] >> 3);
            let s1 = w[(i + 14) & 15].rotate_right(17)
                ^ w[(i + 14) & 15].rotate_right(19)
                ^ (w[(i + 14) & 15] >> 10);
            w[i & 15] = w[i & 15]
                .wrapping_add(s0)
                .wrapping_add(w[(i + 9) & 15])
                .wrapping_add(s1);
        }
        let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(big_s1)
            .wrapping_add(choose)
            .wrapping_add(*k)
            .wrapping_add(w[i & 15]);
        let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = big_s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use sha2::Digest;
    use std::{format, string::String, vec, vec::Vec};

    /// hex -- lower-case hex text of a digest.
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// pattern -- `len` bytes of a fixed, non-repeating-looking pattern.
    fn pattern(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i.wrapping_mul(31) ^ (i >> 8).wrapping_mul(7) ^ 0xA5) as u8)
            .collect()
    }

    /// reference -- the digest of `data` by the independent implementation.
    fn reference(data: &[u8]) -> [u8; 32] {
        sha2::Sha256::digest(data).into()
    }

    #[test]
    fn fips_180_4_example_messages() {
        let million_a = vec![b'a'; 1_000_000];
        let rows: [(&[u8], &str); 4] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                &million_a,
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (message, digest) in rows {
            assert_eq!(hex(&sha256(message)), digest, "{} bytes", message.len());
        }
    }

    #[test]
    fn every_length_to_300_matches_the_reference() {
        let data = pattern(300);
        for len in 0..=300 {
            assert_eq!(sha256(&data[..len]), reference(&data[..len]), "len {len}");
        }
    }

    #[test]
    fn a_480_kb_buffer_matches_the_reference() {
        let data = pattern(480 * 1024);
        assert_eq!(sha256(&data), reference(&data));
    }

    #[test]
    fn chunked_feeding_equals_one_call() {
        let data = pattern(1000);
        let whole = sha256(&data);
        for chunk in [1usize, 3, 63, 64, 65] {
            let mut hash = Sha256::default();
            for piece in data.chunks(chunk) {
                hash.update(piece);
            }
            assert_eq!(hash.finish(), whole, "chunk {chunk}");
        }
    }
}
