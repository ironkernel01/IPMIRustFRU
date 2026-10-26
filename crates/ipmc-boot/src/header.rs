//! header - the runtime image header (design section 4), its one check and the build-time patch.

use crate::geometry::{APP_END, APP_LEN_MAX, APP_START, NOINIT_END, RAM_END};
use ipmc_wire::Sha256;

/// HEADER_OFFSET -- where the header sits, from APP_START.
pub const HEADER_OFFSET: u32 = 0x200;
pub const HEADER_ADDR: u32 = APP_START + HEADER_OFFSET;
pub const HEADER_LEN: usize = 64;
/// MAGIC -- the bytes "IPMC" in memory order.
pub const MAGIC: u32 = 0x434D_5049;
pub const FORMAT: u16 = 1;
/// DIGEST_OFFSET -- the 32 digest bytes, inside the header.
pub const DIGEST_OFFSET: usize = 0x20;
const DIGEST_LEN: usize = 32;
/// LEN_MIN -- the vector table area and the header.
pub const LEN_MIN: u32 = HEADER_OFFSET + HEADER_LEN as u32;
pub const FLAG_TIER23: u32 = 1 << 0;
pub const FLAG_EVAL: u32 = 1 << 1;

const OFF_FORMAT: usize = 0x04;
const OFF_SIZE: usize = 0x06;
const OFF_LENGTH: usize = 0x08;
const OFF_VERSION: usize = 0x0C;
const OFF_FLAGS: usize = 0x10;
const OFF_VECTORS: usize = 0x14;
/// ERASED_WORD -- an erased flash word.
const ERASED_WORD: u32 = 0xFFFF_FFFF;
/// READ_CHUNK -- most bytes the digest reads from the source at once.
const READ_CHUNK: usize = 256;

/// Header -- the 64 header bytes decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,
    pub format: u16,
    pub size: u16,
    pub length: u32,
    pub version: [u8; 4],
    pub flags: u32,
    pub vectors: u32,
    pub digest: [u8; 32],
}

/// version_bytes -- "x.y.z" (a crate version) as [x, y, z]; const so the runtime static can use it
/// with env!("CARGO_PKG_VERSION"). A part above 255 or a malformed text is a compile error when
/// used in a const (panic in const context is allowed here, nowhere else).
pub const fn version_bytes(text: &str) -> [u8; 3] {
    let bytes = text.as_bytes();
    let mut parts = [0u32; 3];
    let mut part = 0;
    let mut digits = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' {
            assert!(digits > 0 && part < 2, "malformed version text");
            part += 1;
            digits = 0;
        } else {
            assert!(b.is_ascii_digit(), "malformed version text");
            parts[part] = parts[part] * 10 + (b - b'0') as u32;
            assert!(parts[part] <= 255, "version part above 255");
            digits += 1;
        }
        i += 1;
    }
    assert!(digits > 0 && part == 2, "malformed version text");
    [parts[0] as u8, parts[1] as u8, parts[2] as u8]
}

/// put_le -- copies `value` little-endian into `bytes` at `at` (const).
const fn put_le(bytes: &mut [u8; HEADER_LEN], at: usize, value: &[u8]) {
    let mut i = 0;
    while i < value.len() {
        bytes[at + i] = value[i];
        i += 1;
    }
}

/// placeholder -- the 64 header bytes the runtime links: every field filled, length and digest
/// FFh (fwimage patch fills them).
pub const fn placeholder(version: [u8; 3], flags: u32) -> [u8; HEADER_LEN] {
    let mut bytes = [0u8; HEADER_LEN];
    put_le(&mut bytes, 0, &MAGIC.to_le_bytes());
    put_le(&mut bytes, OFF_FORMAT, &FORMAT.to_le_bytes());
    put_le(&mut bytes, OFF_SIZE, &(HEADER_LEN as u16).to_le_bytes());
    put_le(&mut bytes, OFF_LENGTH, &ERASED_WORD.to_le_bytes());
    put_le(
        &mut bytes,
        OFF_VERSION,
        &[version[0], version[1], version[2], 0],
    );
    put_le(&mut bytes, OFF_FLAGS, &flags.to_le_bytes());
    put_le(&mut bytes, OFF_VECTORS, &APP_START.to_le_bytes());
    let mut i = DIGEST_OFFSET;
    while i < DIGEST_OFFSET + DIGEST_LEN {
        bytes[i] = 0xFF;
        i += 1;
    }
    bytes
}

/// le_bytes -- the N bytes at `at` (zeros past the end of `bytes`).
fn le_bytes<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes
        .get(at..at + N)
        .and_then(|b| <[u8; N]>::try_from(b).ok())
        .unwrap_or([0; N])
}

/// word_at -- the little-endian u32 at `at`.
fn word_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(le_bytes(bytes, at))
}

/// half_at -- the little-endian u16 at `at`.
fn half_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(le_bytes(bytes, at))
}

/// decode -- the header fields of 64 bytes, all little-endian.
pub fn decode(bytes: &[u8; HEADER_LEN]) -> Header {
    let mut digest = [0u8; DIGEST_LEN];
    if let Some(src) = bytes.get(DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_LEN) {
        digest.copy_from_slice(src);
    }
    Header {
        magic: word_at(bytes, 0),
        format: half_at(bytes, OFF_FORMAT),
        size: half_at(bytes, OFF_SIZE),
        length: word_at(bytes, OFF_LENGTH),
        version: le_bytes(bytes, OFF_VERSION),
        flags: word_at(bytes, OFF_FLAGS),
        vectors: word_at(bytes, OFF_VECTORS),
        digest,
    }
}

/// HeaderField -- which field a `bad header` names in the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderField {
    Format,
    Size,
    Vectors,
    Length,
}

impl HeaderField {
    pub const fn name(self) -> &'static str {
        match self {
            HeaderField::Format => "format",
            HeaderField::Size => "size",
            HeaderField::Vectors => "vector address",
            HeaderField::Length => "length",
        }
    }
}

/// ImageError -- why the runtime image is not startable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    Missing,
    BadMagic,
    BadHeader(HeaderField),
    BadVectorTable,
    DigestMismatch,
}

impl ImageError {
    pub const fn name(self) -> &'static str {
        match self {
            ImageError::Missing => "missing",
            ImageError::BadMagic => "bad magic",
            ImageError::BadHeader(_) => "bad header",
            ImageError::BadVectorTable => "bad vector table",
            ImageError::DigestMismatch => "digest mismatch",
        }
    }
}

/// ImageSource -- reads bytes of flash by absolute address. The firmware reads its flash; the
/// host reads a buffer. Bytes outside what the source holds read FFh (erased).
pub trait ImageSource {
    fn read(&self, addr: u32, out: &mut [u8]);
}

/// SliceImage -- an ImageSource over a buffer that starts at `base`.
pub struct SliceImage<'a> {
    pub base: u32,
    pub bytes: &'a [u8],
}

impl ImageSource for SliceImage<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = (addr as usize)
                .checked_add(i)
                .and_then(|a| a.checked_sub(self.base as usize))
                .and_then(|offset| self.bytes.get(offset))
                .copied()
                .unwrap_or(0xFF);
        }
    }
}

/// Report -- the image check result with what the trace and INFO print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub header: Option<Header>,
    pub computed: Option<[u8; 32]>,
    pub result: Result<(), ImageError>,
}

/// vectors_ok -- the vector table rule (check step 3) for the initial stack pointer and the reset
/// vector of an image of `length` bytes: SP 8-byte aligned inside RAM above NOINIT; reset vector
/// odd (Thumb) and, with bit 0 cleared, after the header and inside the image.
fn vectors_ok(sp: u32, reset: u32, length: u32) -> bool {
    let target = reset & !1;
    sp.is_multiple_of(8)
        && (NOINIT_END..=RAM_END).contains(&sp)
        && reset & 1 == 1
        && target >= HEADER_ADDR + HEADER_LEN as u32
        && target < APP_START.saturating_add(length)
}

/// check -- the one image check (design section 4, steps 1-4, in that order). `header` is Some
/// once the magic matched; `computed` is Some once steps 1-3 passed.
pub fn check(src: &dyn ImageSource) -> Report {
    let mut raw = [0u8; HEADER_LEN];
    src.read(HEADER_ADDR, &mut raw);
    let fail = |header: Option<Header>, error: ImageError| Report {
        header,
        computed: None,
        result: Err(error),
    };
    let header = decode(&raw);
    if header.magic == ERASED_WORD {
        return fail(None, ImageError::Missing);
    }
    if header.magic != MAGIC {
        return fail(None, ImageError::BadMagic);
    }
    let field = if header.format != FORMAT {
        Some(HeaderField::Format)
    } else if header.size as usize != HEADER_LEN {
        Some(HeaderField::Size)
    } else if header.vectors != APP_START {
        Some(HeaderField::Vectors)
    } else if header.length < LEN_MIN
        || header.length > APP_LEN_MAX
        || !header.length.is_multiple_of(4)
    {
        Some(HeaderField::Length)
    } else {
        None
    };
    if let Some(field) = field {
        return fail(Some(header), ImageError::BadHeader(field));
    }
    let mut table = [0u8; 8];
    src.read(APP_START, &mut table);
    if !vectors_ok(word_at(&table, 0), word_at(&table, 4), header.length) {
        return fail(Some(header), ImageError::BadVectorTable);
    }
    let computed = digest(src, header.length);
    let result = if computed == header.digest {
        Ok(())
    } else {
        Err(ImageError::DigestMismatch)
    };
    Report {
        header: Some(header),
        computed: Some(computed),
        result,
    }
}

/// hash_range -- feeds `from..to` of `src` into `hash`, at most READ_CHUNK bytes per read.
fn hash_range(src: &dyn ImageSource, hash: &mut Sha256, from: u32, to: u32) {
    let mut at = from;
    let mut chunk = [0u8; READ_CHUNK];
    while at < to {
        let take = (to - at).min(READ_CHUNK as u32);
        if let Some(part) = chunk.get_mut(..take as usize) {
            src.read(at, part);
            hash.update(part);
        }
        at += take;
    }
}

/// digest -- SHA-256 over APP_START..APP_START+length minus the 32 digest bytes (the hashed
/// range of section 4), read in chunks of at most 256 bytes from `src`.
pub fn digest(src: &dyn ImageSource, length: u32) -> [u8; 32] {
    let end = APP_START.saturating_add(length).min(APP_END);
    let digest_start = HEADER_ADDR + DIGEST_OFFSET as u32;
    let digest_end = digest_start + DIGEST_LEN as u32;
    let mut hash = Sha256::new();
    hash_range(src, &mut hash, APP_START, end.min(digest_start));
    hash_range(src, &mut hash, digest_end, end);
    hash.finish()
}

/// PatchError -- why `patch` refuses an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchError {
    TooShort,
    TooLong,
    NotAligned,
    NoPlaceholder,
    BadVectorTable,
}

impl PatchError {
    pub const fn name(self) -> &'static str {
        match self {
            PatchError::TooShort => "image shorter than the vector table and header",
            PatchError::TooLong => "image longer than the APP region",
            PatchError::NotAligned => "image length not a multiple of 4",
            PatchError::NoPlaceholder => "no header placeholder at offset 0x200",
            PatchError::BadVectorTable => "bad vector table at offset 0",
        }
    }
}

/// is_placeholder -- the header bytes are what `placeholder` links: magic, format, size and
/// vector address filled, length and digest FFh.
fn is_placeholder(raw: &[u8; HEADER_LEN]) -> bool {
    let h = decode(raw);
    h.magic == MAGIC
        && h.format == FORMAT
        && h.size as usize == HEADER_LEN
        && h.vectors == APP_START
        && h.length == ERASED_WORD
        && h.digest.iter().all(|&b| b == 0xFF)
}

/// patch -- fwimage's step: `image` is the APP image starting at APP_START. Checks the
/// placeholder at HEADER_OFFSET and the vector table words at offset 0 (the rules of check step
/// 3), writes length (`image.len()`, which the caller has padded to a multiple of 4) and then the
/// digest. Returns the 64 patched header bytes.
pub fn patch(image: &mut [u8]) -> Result<[u8; HEADER_LEN], PatchError> {
    let length = u32::try_from(image.len()).map_err(|_| PatchError::TooLong)?;
    if length < LEN_MIN {
        return Err(PatchError::TooShort);
    }
    if length > APP_LEN_MAX {
        return Err(PatchError::TooLong);
    }
    if !length.is_multiple_of(4) {
        return Err(PatchError::NotAligned);
    }
    let at = HEADER_OFFSET as usize;
    let mut raw = [0u8; HEADER_LEN];
    raw.copy_from_slice(image.get(at..at + HEADER_LEN).ok_or(PatchError::TooShort)?);
    if !is_placeholder(&raw) {
        return Err(PatchError::NoPlaceholder);
    }
    if !vectors_ok(word_at(image, 0), word_at(image, 4), length) {
        return Err(PatchError::BadVectorTable);
    }
    // The length goes in first: it is inside the hashed range.
    let put = |image: &mut [u8], field: usize, value: &[u8]| {
        image
            .get_mut(at + field..at + field + value.len())
            .map(|room| room.copy_from_slice(value))
            .ok_or(PatchError::TooShort)
    };
    put(image, OFF_LENGTH, &length.to_le_bytes())?;
    let sum = digest(
        &SliceImage {
            base: APP_START,
            bytes: image,
        },
        length,
    );
    put(image, DIGEST_OFFSET, &sum)?;
    raw.copy_from_slice(image.get(at..at + HEADER_LEN).ok_or(PatchError::TooShort)?);
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// SP_OK -- a valid initial stack pointer.
    const SP_OK: u32 = 0x2001_8000;
    /// RESET_OK -- a valid reset vector (Thumb bit set, after the header).
    const RESET_OK: u32 = HEADER_ADDR + HEADER_LEN as u32 + 0x41;

    /// image -- an APP image of `len` bytes: vector words, the placeholder, a counting pattern.
    fn image(len: usize, sp: u32, reset: u32) -> Vec<u8> {
        let mut bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
        bytes[0..4].copy_from_slice(&sp.to_le_bytes());
        bytes[4..8].copy_from_slice(&reset.to_le_bytes());
        let at = HEADER_OFFSET as usize;
        bytes[at..at + HEADER_LEN].copy_from_slice(&placeholder([1, 2, 3], FLAG_EVAL));
        bytes
    }

    /// patched -- a good image after `patch`.
    fn patched(len: usize) -> Vec<u8> {
        let mut bytes = image(len, SP_OK, RESET_OK);
        patch(&mut bytes).unwrap();
        bytes
    }

    fn check_bytes(bytes: &[u8]) -> Report {
        check(&SliceImage {
            base: APP_START,
            bytes,
        })
    }

    #[test]
    fn placeholder_bytes_exactly() {
        let mut want = vec![0u8; HEADER_LEN];
        want[0..4].copy_from_slice(b"IPMC");
        want[4..6].copy_from_slice(&[1, 0]);
        want[6..8].copy_from_slice(&[64, 0]);
        want[8..12].copy_from_slice(&[0xFF; 4]);
        want[12..16].copy_from_slice(&[1, 2, 3, 0]);
        want[16..20].copy_from_slice(&[2, 0, 0, 0]);
        want[20..24].copy_from_slice(&[0x00, 0xC0, 0x00, 0x08]);
        want[32..64].copy_from_slice(&[0xFF; 32]);
        assert_eq!(placeholder([1, 2, 3], FLAG_EVAL).to_vec(), want);
    }

    #[test]
    fn version_text_to_bytes() {
        assert_eq!(version_bytes("0.1.0"), [0, 1, 0]);
        assert_eq!(version_bytes("12.255.7"), [12, 255, 7]);
    }

    #[test]
    fn patch_then_check_is_ok_with_an_independent_digest() {
        let len = 4000;
        let bytes = patched(len);
        let report = check_bytes(&bytes);
        assert_eq!(report.result, Ok(()));
        let mut hashed = bytes[..0x220].to_vec();
        hashed.extend_from_slice(&bytes[0x240..]);
        let want = ipmc_wire::sha256(&hashed);
        assert_eq!(report.computed, Some(want));
        let header = report.header.unwrap();
        assert_eq!(header.digest, want);
        assert_eq!(header.length as usize, len);
        assert_eq!(header.version, [1, 2, 3, 0]);
        assert_eq!(header.flags, FLAG_EVAL);
    }

    #[test]
    fn patch_returns_the_header_it_wrote() {
        let mut bytes = image(1000, SP_OK, RESET_OK);
        let header = patch(&mut bytes).unwrap();
        assert_eq!(header.to_vec(), bytes[0x200..0x240].to_vec());
    }

    #[test]
    fn header_fields_corrupted_give_their_reason() {
        let good = patched(1000);
        // (offset, bytes written, expected)
        let rows: [(usize, [u8; 4], ImageError); 9] = [
            (0x200, [0xFF; 4], ImageError::Missing),
            (0x200, [0x11, 0x22, 0x33, 0x44], ImageError::BadMagic),
            (
                0x204,
                [2, 0, 64, 0],
                ImageError::BadHeader(HeaderField::Format),
            ),
            (
                0x204,
                [1, 0, 65, 0],
                ImageError::BadHeader(HeaderField::Size),
            ),
            (
                0x214,
                [0, 0, 0, 8],
                ImageError::BadHeader(HeaderField::Vectors),
            ),
            (
                0x208,
                [0x3F, 2, 0, 0],
                ImageError::BadHeader(HeaderField::Length),
            ),
            (
                0x208,
                [0, 0, 8, 0],
                ImageError::BadHeader(HeaderField::Length),
            ),
            (
                0x208,
                [0xEB, 3, 0, 0],
                ImageError::BadHeader(HeaderField::Length),
            ),
            (
                0x208,
                [0, 0, 0, 0],
                ImageError::BadHeader(HeaderField::Length),
            ),
        ];
        for (offset, patch_bytes, want) in rows {
            let mut bytes = good.clone();
            bytes[offset..offset + 4].copy_from_slice(&patch_bytes);
            let report = check_bytes(&bytes);
            assert_eq!(
                report.result,
                Err(want),
                "offset {offset:#x} {patch_bytes:?}"
            );
            assert_eq!(report.computed, None);
        }
    }

    #[test]
    fn erased_source_is_missing_without_a_header() {
        let report = check_bytes(&[]);
        assert_eq!(report.result, Err(ImageError::Missing));
        assert_eq!(report.header, None);
        assert_eq!(report.computed, None);
    }

    #[test]
    fn the_digest_bytes_are_outside_the_hash() {
        let good = patched(2000);
        // (offset, expected result)
        let rows: [(usize, Result<(), ImageError>); 6] = [
            (0x220, Err(ImageError::DigestMismatch)),
            (0x23F, Err(ImageError::DigestMismatch)),
            (0x100, Err(ImageError::DigestMismatch)),
            (0x240, Err(ImageError::DigestMismatch)),
            (1999, Err(ImageError::DigestMismatch)),
            (0x21F, Err(ImageError::DigestMismatch)),
        ];
        for (offset, want) in rows {
            let mut bytes = good.clone();
            bytes[offset] ^= 0x5A;
            assert_eq!(check_bytes(&bytes).result, want, "offset {offset:#x}");
        }
        // A byte past `length` is outside the hash.
        let mut longer = good.clone();
        longer.extend_from_slice(&[0x12; 64]);
        assert_eq!(check_bytes(&longer).result, Ok(()));
    }

    #[test]
    fn vector_table_rows() {
        // (sp, reset, expected)
        let rows: [(u32, u32, Result<(), ImageError>); 7] = [
            (SP_OK, RESET_OK, Ok(())),
            (SP_OK - 4, RESET_OK, Err(ImageError::BadVectorTable)),
            (NOINIT_END - 8, RESET_OK, Err(ImageError::BadVectorTable)),
            (RAM_END + 8, RESET_OK, Err(ImageError::BadVectorTable)),
            (SP_OK, RESET_OK - 1, Err(ImageError::BadVectorTable)),
            (SP_OK, HEADER_ADDR + 0x3F, Err(ImageError::BadVectorTable)),
            (SP_OK, APP_START + 0x1001, Err(ImageError::BadVectorTable)),
        ];
        for (sp, reset, want) in rows {
            let mut bytes = image(4096, sp, reset);
            // Bypass patch (which checks the table too): write length and digest by hand.
            bytes[0x208..0x20C].copy_from_slice(&4096u32.to_le_bytes());
            let sum = digest(
                &SliceImage {
                    base: APP_START,
                    bytes: &bytes,
                },
                4096,
            );
            bytes[0x220..0x240].copy_from_slice(&sum);
            assert_eq!(
                check_bytes(&bytes).result,
                want,
                "sp {sp:#x} reset {reset:#x}"
            );
        }
    }

    #[test]
    fn patch_refuses_each_case() {
        let mut no_placeholder = image(1000, SP_OK, RESET_OK);
        no_placeholder[0x204] = 0;
        let mut already = patched(1000);
        already[0x208] = 0;
        // (image, expected)
        let rows: [(Vec<u8>, PatchError); 7] = [
            (
                image(0x240, SP_OK, RESET_OK)[..0x23C].to_vec(),
                PatchError::TooShort,
            ),
            (vec![0; 475_140], PatchError::TooLong),
            (image(1001, SP_OK, RESET_OK), PatchError::NotAligned),
            (no_placeholder, PatchError::NoPlaceholder),
            (already, PatchError::NoPlaceholder),
            (image(1000, SP_OK + 4, RESET_OK), PatchError::BadVectorTable),
            (image(1000, SP_OK, APP_START), PatchError::BadVectorTable),
        ];
        for (mut bytes, want) in rows {
            assert_eq!(patch(&mut bytes), Err(want), "{}", want.name());
        }
    }
}
