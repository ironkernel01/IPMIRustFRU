//! ipmc-wire - the IPMI byte-level primitives shared by IPMB frames, FRU records and SDR records.
//!
//! The crate has no hardware access and no allocator, and builds for the host and for
//! thumbv7em-none-eabihf. It holds one implementation of each primitive:
//!
//! - `hex_byte` / `hex_word`: the hex byte tokens of vector files, console lines and application
//!   port lines.
//! - `zero_checksum`: the two's-complement checksum of IPMB frames and FRU areas.
//! - `Builder`: the const byte-record builder of the SDR records and the FRU image.
//! - `Builder::push_type_length_ascii`: the IPMI type/length field of 8-bit ASCII text.
//! - `crc32_update` / `crc32`: the CRC-32 of the stored FRU record (ipmc-fru persist.rs).
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

/// hex_word -- one typed byte of a console or application port line: `06`, `0x06`, `0X06` and
/// `6` are all 06h. The optional prefix is removed, then `hex_byte` decides; None for anything
/// else.
pub fn hex_word(text: &str) -> Option<u8> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    hex_byte(digits)
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

/// CRC32_POLY_REFLECTED -- the CRC-32 polynomial 04C11DB7h bit-reversed (IEEE 802.3, the reflected
/// form that shifts right).
const CRC32_POLY_REFLECTED: u32 = 0xEDB8_8320;
/// CRC32_INIT -- the register value before the first byte (IEEE 802.3: all ones). Also the final
/// XOR value.
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;
/// CRC32_XOROUT -- the value XORed into the register after the last byte.
pub const CRC32_XOROUT: u32 = 0xFFFF_FFFF;

/// crc32_update -- one byte folded into a CRC-32 register (IEEE 802.3: reflected, polynomial
/// 04C11DB7h, bits shifted out LS first). Start from `CRC32_INIT`, finish with `^ CRC32_XOROUT`;
/// `crc32` does both. Bitwise, no table: the stored FRU record is a few hundred bytes, read at
/// boot and written after a Write FRU Data, so speed does not matter and flash is saved.
pub const fn crc32_update(crc: u32, byte: u8) -> u32 {
    let mut crc = crc ^ byte as u32;
    let mut bit = 0;
    while bit < u8::BITS {
        crc = if crc & 1 != 0 {
            (crc >> 1) ^ CRC32_POLY_REFLECTED
        } else {
            crc >> 1
        };
        bit += 1;
    }
    crc
}

/// crc32 -- the CRC-32 (IEEE 802.3) of `bytes`: init FFFFFFFFh, reflected, XOR out FFFFFFFFh.
/// The check value of the ASCII bytes "123456789" is CBF43926h.
pub const fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = CRC32_INIT;
    let mut i = 0;
    while i < bytes.len() {
        crc = crc32_update(crc, bytes[i]);
        i += 1;
    }
    crc ^ CRC32_XOROUT
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
    fn crc32_matches_the_standard_check_value_and_empty_input() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
        // Streaming the same bytes through the byte primitive gives the same register.
        let streamed = b"123456789"
            .iter()
            .fold(CRC32_INIT, |crc, &b| crc32_update(crc, b))
            ^ CRC32_XOROUT;
        assert_eq!(streamed, 0xCBF4_3926);
    }

    #[test]
    fn hex_tokens_parse_with_and_without_the_prefix() {
        // (text, hex_byte, hex_word): `hex_word` also takes a 0x prefix.
        let cases: [(&str, Option<u8>, Option<u8>); 13] = [
            ("0A", Some(0x0A), Some(0x0A)),
            ("a", Some(0x0A), Some(0x0A)),
            ("ff", Some(0xFF), Some(0xFF)),
            ("", None, None),
            ("123", None, None),
            ("+1", None, None),
            ("0x1", None, Some(1)),
            ("0X1f", None, Some(0x1F)),
            ("0x", None, None),
            ("0x123", None, None),
            ("x06", None, None),
            ("1h", None, None),
            ("G0", None, None),
        ];
        for (text, byte, word) in cases {
            assert_eq!(hex_byte(text), byte, "hex_byte {text:?}");
            assert_eq!(hex_word(text), word, "hex_word {text:?}");
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
