//! ipmc-wire - the IPMI byte-level primitives shared by IPMB frames, FRU records and SDR records.
//!
//! The crate has no hardware access and no allocator, and builds for the host and for
//! thumbv7em-none-eabihf. It holds one implementation of each primitive:
//!
//! - `zero_checksum`: the two's-complement checksum of IPMB frames and FRU areas.
//! - `Builder`: the const byte-record builder of the SDR records and the FRU image.
//! - `Builder::push_type_length_ascii`: the IPMI type/length field of 8-bit ASCII text.
#![no_std]
#![forbid(unsafe_code)]

/// TYPE_ASCII -- type/length byte bits 7:6 = 11b, 8-bit ASCII + Latin 1 (IPMI v2.0 section 43.15,
/// FRU spec section 13).
pub const TYPE_ASCII: u8 = 0xC0;
/// TYPE_MASK -- bits 7:6 of a type/length byte.
pub const TYPE_MASK: u8 = 0xC0;
/// TEXT_LEN_MAX -- the longest text a 6-bit type/length length holds (bits 5:0).
pub const TEXT_LEN_MAX: usize = 63;

/// HEX_DIGITS_MAX -- a byte token is one or two hex digits.
const HEX_DIGITS_MAX: usize = 2;

/// hex_byte -- one or two hex digits to a byte; None for anything else (empty, longer, sign,
/// prefix or suffix). The one hex-byte parser of the vector files and the command document.
pub fn hex_byte(token: &str) -> Option<u8> {
    if token.len() > HEX_DIGITS_MAX || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u8::from_str_radix(token, 16).ok()
}

/// zero_checksum -- two's-complement checksum; returns the byte that makes `bytes` plus itself sum to 0.
/// The IPMB frame checksums (IPMI v2.0 section 13.8) and the FRU zero checksums (Platform
/// Management FRU Information Storage Definition, sections 8, 11 and 12) are this one function;
/// `const` so the FRU image can be built at compile time.
pub const fn zero_checksum(bytes: &[u8]) -> u8 {
    let mut sum = 0u8;
    let mut i = 0;
    while i < bytes.len() {
        sum = sum.wrapping_add(bytes[i]);
        i += 1;
    }
    0u8.wrapping_sub(sum)
}

/// Builder -- a fixed-capacity byte record built in const context: the bytes and how many of
/// them are used. A push past the capacity is a compile error when the record is built in a
/// const item. `bytes` is public so a caller patches a length or checksum byte in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builder<const N: usize> {
    /// The record storage; bytes past `len()` are 0.
    pub bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Builder<N> {
    /// with_len -- an all-zero builder whose first `len` bytes count as used (a reserved prefix
    /// the caller fills in later through `bytes`).
    pub const fn with_len(len: usize) -> Self {
        assert!(len <= N, "the reserved prefix is longer than the capacity");
        Builder { bytes: [0; N], len }
    }

    /// len -- bytes used.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// is_empty -- true while no byte is used.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// capacity -- bytes the record can hold.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// as_bytes -- the used bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    /// push -- appends one byte; a record past its capacity is a compile error.
    pub const fn push(&mut self, byte: u8) {
        assert!(self.len < N, "a record is longer than its capacity");
        self.bytes[self.len] = byte;
        self.len += 1;
    }

    /// push_all -- appends `bytes`.
    pub const fn push_all(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            self.push(bytes[i]);
            i += 1;
        }
    }

    /// push_type_length_ascii -- one type/length field holding `text` as 8-bit ASCII: the byte
    /// `TYPE_ASCII | length`, then the text. The caller states its own length rule as
    /// `min_len..=max_len` (FRU text: 0..=63; SDR ID string: 2..=16); the encoder asserts it
    /// together with the two rules of the field itself: the 6-bit length caps the text at
    /// TEXT_LEN_MAX, and a length of 1 is reserved (FRU spec section 13: type 11b holds "at
    /// least two bytes of data"; 0 is the "no data" field).
    pub const fn push_type_length_ascii(&mut self, text: &str, min_len: usize, max_len: usize) {
        let data = text.as_bytes();
        assert!(
            data.len() >= min_len
                && data.len() <= max_len
                && data.len() <= TEXT_LEN_MAX
                && data.len() != 1,
            "a type/length text breaks its length rule"
        );
        self.push(TYPE_ASCII | data.len() as u8);
        self.push_all(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_known_values() {
        assert_eq!(zero_checksum(&[]), 0);
        assert_eq!(zero_checksum(&[0x82, 0x18]), 0x66);
        assert_eq!(zero_checksum(&[0x01]), 0xFF);
        assert_eq!(zero_checksum(&[0xFF, 0x01]), 0x00);
    }

    #[test]
    fn hex_byte_takes_one_or_two_digits_only() {
        let cases: [(&str, Option<u8>); 9] = [
            ("0A", Some(0x0A)),
            ("a", Some(0x0A)),
            ("ff", Some(0xFF)),
            ("", None),
            ("123", None),
            ("+1", None),
            ("0x1", None),
            ("1h", None),
            ("G0", None),
        ];
        for (token, want) in cases {
            assert_eq!(hex_byte(token), want, "token {token:?}");
        }
    }

    #[test]
    fn builder_pushes_and_reports_length() {
        let mut b = Builder::<8>::with_len(1);
        assert_eq!((b.len(), b.capacity(), b.is_empty()), (1, 8, false));
        b.push(0xAA);
        b.push_all(&[1, 2]);
        assert_eq!(b.as_bytes(), &[0, 0xAA, 1, 2]);
    }

    #[test]
    fn type_length_field_is_c0_or_length_then_text() {
        let mut b = Builder::<8>::with_len(0);
        b.push_type_length_ascii("ab", 0, TEXT_LEN_MAX);
        b.push_type_length_ascii("", 0, TEXT_LEN_MAX);
        assert_eq!(b.as_bytes(), &[0xC2, b'a', b'b', 0xC0]);
    }
}
