//! records.rs - the FRU Information records of FRU 0: layouts, the built image, the decoder and
//! `verify`.
//!
//! Layouts are from the IPMI Platform Management FRU Information Storage Definition v1.0 rev 1.3
//! ("FRU spec" below): Table 8-1 common header, Table 11-1 board info area, Table 12-1 product
//! info area, section 13 type/length byte. VITA 46.11 rules: Common Header, Board Info Area and
//! Product Info Area are present (Rule 8.1-2), the Chassis Info offset is 00h (Rule 8.1-3), the
//! Internal Use and MultiRecord areas are absent (offset 00h, Permission 8.1-1 and 8.1-4).
//!
//! ```text
//! offset 00h  common header, 8 bytes
//!        08h  board info area, a multiple of 8 bytes
//!        ..   product info area, a multiple of 8 bytes
//!        ..   00h padding up to IPMC_FRU_SIZE
//! ```
//!
//! The image is built at compile time from the IPMC_FRU_* defines of ipmi_defs.h: a string
//! the area cannot hold, or an image larger than IPMC_FRU_SIZE, fails the build. `verify`
//! checks a (possibly rewritten) image against the same layouts; the startup self test and
//! every accepted Write FRU Data call it through ipmc-core `selftest`.

use crate::store::FruError;
use ipmc_wire::{zero_checksum as checksum, Builder, TYPE_ASCII, TYPE_MASK};

/// SIZE -- the FRU inventory area size in bytes (ipmi_defs.h IPMC_FRU_SIZE).
pub const SIZE: usize = ipmc_defs::IPMC_FRU_SIZE as usize;

/// AREA_ALIGN -- areas start at, and are, multiples of 8 bytes (FRU spec section 8, Table 8-1).
pub const AREA_ALIGN: usize = 8;
/// COMMON_HEADER_LEN -- the common header is 8 bytes (FRU spec Table 8-1).
pub const COMMON_HEADER_LEN: usize = 8;
/// FORMAT_VERSION -- format version 1h in the common header and in every area (FRU spec sections 8, 11, 12).
const FORMAT_VERSION: u8 = 1;
/// A version byte is compared whole against FORMAT_VERSION: bits 3:0 are the version and bits 7:4
/// are reserved, written 0 (FRU spec Tables 8-1, 11-1, 12-1), so any other value is wrong.
/// MULTIRECORD_VERSION_MASK -- bits 3:0 of a MultiRecord header's second byte are the record
/// format version; bit 7 is end-of-list and bits 6:4 are reserved, written 0 (Table 16-1).
const MULTIRECORD_VERSION_MASK: u8 = 0x7F;
/// LANGUAGE_ENGLISH -- language code 0, English (FRU spec Table 15-1: index 0 and 25 both mean English).
const LANGUAGE_ENGLISH: u8 = 0;
/// LENGTH_MASK -- bits 5:0 of a type/length byte, the data length in bytes (FRU spec section 13).
const LENGTH_MASK: u8 = 0x3F;
/// END_OF_FIELDS -- type/length byte C1h: no more fields (FRU spec section 13).
const END_OF_FIELDS: u8 = 0xC1;
/// FIELD_LEN_MAX -- longest field a 6-bit length holds.
const FIELD_LEN_MAX: usize = 63;
/// MFG_DATE_LEN -- the board manufacturing date/time is 3 bytes, LS byte first (FRU spec Table 11-1).
const MFG_DATE_LEN: usize = 3;
/// Bytes of an area before its first type/length field: version, length, language code, and for
/// the board area the 3-byte manufacturing date (FRU spec Tables 11-1 and 12-1).
const PRODUCT_HEADER_LEN: usize = 3;
const BOARD_HEADER_LEN: usize = PRODUCT_HEADER_LEN + MFG_DATE_LEN;

/// Common header byte positions (FRU spec Table 8-1).
const HDR_VERSION: usize = 0;
const HDR_INTERNAL: usize = 1;
const HDR_CHASSIS: usize = 2;
const HDR_BOARD: usize = 3;
const HDR_PRODUCT: usize = 4;
const HDR_MULTIRECORD: usize = 5;
/// Byte 6 is the pad (00h), byte 7 the zero checksum of bytes 0..=6.
const HDR_PAD: usize = 6;
const HDR_CHECKSUM: usize = 7;

/// Image -- the FRU bytes and how many of them the areas use (`ipmc_wire::Builder`).
pub(crate) type Image = Builder<SIZE>;

/// push_text -- one type/length field holding `text` as 8-bit ASCII. An empty string is the
/// "no data" field 00h in bits 5:0 (FRU spec section 5); a length of 1 is reserved for
/// type 11b (section 13: "at least two bytes of data"), so `push_type_length_ascii` refuses it.
const fn push_text(img: &mut Image, text: &str) {
    img.push_type_length_ascii(text, 0, FIELD_LEN_MAX);
}

/// finish_area -- closes the area that began at `start`: the C1h end-of-fields byte, 00h
/// padding to a multiple of 8 bytes that leaves one byte for the checksum, the area length
/// in byte 1 (in 8-byte units) and the zero checksum.
const fn finish_area(img: &mut Image, start: usize) {
    img.push(END_OF_FIELDS);
    while !(img.len() - start + 1).is_multiple_of(AREA_ALIGN) {
        img.push(0);
    }
    let units = (img.len() - start + 1) / AREA_ALIGN;
    assert!(units <= u8::MAX as usize, "FRU area longer than 255 units");
    img.bytes[start + 1] = units as u8;
    let (_, tail) = img.bytes.split_at(start);
    let (area, _) = tail.split_at(img.len() - start);
    let sum = checksum(area);
    img.push(sum);
}

/// start_area -- version byte and the length placeholder; returns the area's offset.
const fn start_area(img: &mut Image) -> usize {
    let start = img.len();
    assert!(
        start.is_multiple_of(AREA_ALIGN),
        "FRU areas start on 8-byte boundaries"
    );
    img.push(FORMAT_VERSION);
    img.push(0);
    start
}

/// build -- the FRU image from the IPMC_FRU_* defines.
const fn build() -> Image {
    let mut img = Image::with_len(COMMON_HEADER_LEN);

    // Board Info Area (FRU spec Table 11-1).
    let board = start_area(&mut img);
    img.push(LANGUAGE_ENGLISH);
    let date = ipmc_defs::IPMC_FRU_MFG_MINUTES.to_le_bytes();
    img.push(date[0]);
    img.push(date[1]);
    img.push(date[2]);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_MFR);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_PRODUCT);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_SERIAL);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_PART);
    push_text(&mut img, ipmc_defs::IPMC_FRU_FILE_ID);
    finish_area(&mut img, board);

    // Product Info Area (FRU spec Table 12-1). The product area has its own FRU File ID field;
    // owner decision 2 defines one file ID (the board's), so this one is the empty field.
    let product = start_area(&mut img);
    img.push(LANGUAGE_ENGLISH);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_MFR);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_NAME);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_PART);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_VERSION);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_SERIAL);
    push_text(&mut img, ipmc_defs::IPMC_FRU_ASSET_TAG);
    push_text(&mut img, "");
    finish_area(&mut img, product);

    // Common header (FRU spec Table 8-1): Internal Use, Chassis Info and MultiRecord are absent.
    img.bytes[HDR_VERSION] = FORMAT_VERSION;
    img.bytes[HDR_INTERNAL] = 0;
    img.bytes[HDR_CHASSIS] = 0;
    img.bytes[HDR_BOARD] = (board / AREA_ALIGN) as u8;
    img.bytes[HDR_PRODUCT] = (product / AREA_ALIGN) as u8;
    img.bytes[HDR_MULTIRECORD] = 0;
    let (head, _) = img.bytes.split_at(HDR_CHECKSUM);
    img.bytes[HDR_CHECKSUM] = checksum(head);
    img
}

/// IMAGE -- the built image, evaluated at compile time; `FruStore::new` copies it.
pub(crate) static IMAGE: Image = build();

/// IMAGE_LEN -- bytes of SIZE the built areas use.
pub const IMAGE_LEN: usize = build().len();
const _: () = assert!(
    SIZE >= IMAGE_LEN,
    "IPMC_FRU_SIZE in ipmi_defs.h is smaller than the built FRU image"
);
const _: () = assert!(
    SIZE.is_multiple_of(AREA_ALIGN),
    "IPMC_FRU_SIZE must be a multiple of 8"
);

// ---- decoder (FRU SHOW, tests) ----

/// CommonHeader -- the decoded common header (FRU spec Table 8-1); offsets in 8-byte units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommonHeader {
    /// Internal Use Area offset, 0 = absent.
    pub internal: u8,
    /// Chassis Info Area offset, 0 = absent (VITA Rule 8.1-3: 0).
    pub chassis: u8,
    /// Board Info Area offset.
    pub board: u8,
    /// Product Info Area offset.
    pub product: u8,
    /// MultiRecord Area offset, 0 = absent.
    pub multirecord: u8,
    /// Pad byte (byte 6), 00h in a valid header.
    pub pad: u8,
    /// The 8 header bytes sum to 0 (zero checksum).
    pub checksum_ok: bool,
}

/// common_header -- decodes the first 8 bytes of `image`.
/// Err(TooShort) with fewer than 8 bytes, Err(BadVersion) when the version is not 1h.
pub fn common_header(image: &[u8]) -> Result<CommonHeader, FruError> {
    let head = image.get(..COMMON_HEADER_LEN).ok_or(FruError::TooShort)?;
    if head.get(HDR_VERSION) != Some(&FORMAT_VERSION) {
        return Err(FruError::BadVersion);
    }
    let at = |i: usize| head.get(i).copied().ok_or(FruError::TooShort);
    Ok(CommonHeader {
        internal: at(HDR_INTERNAL)?,
        chassis: at(HDR_CHASSIS)?,
        board: at(HDR_BOARD)?,
        product: at(HDR_PRODUCT)?,
        multirecord: at(HDR_MULTIRECORD)?,
        pad: at(HDR_PAD)?,
        checksum_ok: checksum(head) == 0,
    })
}

/// AreaKind -- which information area a decode reads; the first field follows a different
/// fixed prefix in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaKind {
    /// Board Info Area (Table 11-1): version, length, language, 3-byte date, then fields.
    Board,
    /// Product Info Area (Table 12-1): version, length, language, then fields.
    Product,
}

impl AreaKind {
    const fn header_len(self) -> usize {
        match self {
            AreaKind::Board => BOARD_HEADER_LEN,
            AreaKind::Product => PRODUCT_HEADER_LEN,
        }
    }
}

/// Area -- one decoded information area, borrowing the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area<'a> {
    /// Board or Product.
    pub kind: AreaKind,
    /// The whole area, version byte to checksum byte.
    pub bytes: &'a [u8],
    /// The area's bytes sum to 0 (zero checksum).
    pub checksum_ok: bool,
}

/// area -- decodes the area at `offset_units` (an 8-byte unit offset from the common header).
/// Err(TooShort), Err(BadVersion) or Err(BadLength) when the bytes are not an area.
pub fn area(image: &[u8], kind: AreaKind, offset_units: u8) -> Result<Area<'_>, FruError> {
    let start = usize::from(offset_units) * AREA_ALIGN;
    let rest = image.get(start..).ok_or(FruError::TooShort)?;
    let [version, units, ..] = rest else {
        return Err(FruError::TooShort);
    };
    if *version != FORMAT_VERSION {
        return Err(FruError::BadVersion);
    }
    let len = usize::from(*units) * AREA_ALIGN;
    if len < kind.header_len() {
        return Err(FruError::BadLength);
    }
    let bytes = rest.get(..len).ok_or(FruError::BadLength)?;
    Ok(Area {
        kind,
        bytes,
        checksum_ok: checksum(bytes) == 0,
    })
}

impl<'a> Area<'a> {
    /// mfg_minutes -- the board manufacturing date as minutes since 1996-01-01 00:00 (Table
    /// 11-1); None for a product area or a truncated one.
    pub fn mfg_minutes(&self) -> Option<u32> {
        if self.kind != AreaKind::Board {
            return None;
        }
        let date = self.bytes.get(PRODUCT_HEADER_LEN..BOARD_HEADER_LEN)?;
        let [a, b, c] = date else { return None };
        Some(u32::from_le_bytes([*a, *b, *c, 0]))
    }

    /// fields -- the type/length fields in order, up to the C1h end-of-fields byte. The area's
    /// final checksum byte is not part of the field list.
    pub fn fields(&self) -> Fields<'a> {
        let list = self
            .bytes
            .split_last()
            .and_then(|(_, list)| list.get(self.kind.header_len()..))
            .unwrap_or(&[]);
        Fields {
            rest: list,
            closed: false,
        }
    }

    /// has_end_marker -- a C1h end-of-fields byte closes the field list inside the area.
    pub fn has_end_marker(&self) -> bool {
        let mut fields = self.fields();
        while fields.next().is_some() {}
        fields.closed
    }
}

/// Field -- one type/length field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field<'a> {
    /// Type code, bits 7:6 of the type/length byte (11b = 8-bit ASCII + Latin 1).
    pub type_code: u8,
    /// The data bytes; empty for a "no data" field.
    pub data: &'a [u8],
}

impl Field<'_> {
    /// is_ascii -- the field is 8-bit ASCII + Latin 1.
    pub const fn is_ascii(&self) -> bool {
        self.type_code == TYPE_ASCII >> 6
    }
}

/// Fields -- iterator over the fields of an area; ends at C1h or at a field that runs past the area.
#[derive(Clone, Copy, Debug)]
pub struct Fields<'a> {
    rest: &'a [u8],
    /// The iteration stopped at a C1h end-of-fields byte, not at the end of the bytes.
    closed: bool,
}

impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Field<'a>> {
        let (&tl, after) = self.rest.split_first()?;
        if tl == END_OF_FIELDS {
            self.rest = &[];
            self.closed = true;
            return None;
        }
        let len = usize::from(tl & LENGTH_MASK);
        let Some(data) = after.get(..len) else {
            self.rest = &[];
            return None;
        };
        self.rest = after.get(len..).unwrap_or(&[]);
        Some(Field {
            type_code: (tl & TYPE_MASK) >> 6,
            data,
        })
    }
}

// ---- verify ----

/// MULTIRECORD_HEADER_LEN -- a MultiRecord header is 5 bytes: type ID, end-of-list bit and format
/// version, record length, record checksum, header checksum (FRU spec Table 16-1).
const MULTIRECORD_HEADER_LEN: usize = 5;
/// MULTIRECORD_VERSION -- record format version 2h (FRU spec section 16.2.3); bits 3:0.
const MULTIRECORD_VERSION: u8 = 2;
/// MULTIRECORD_END_OF_LIST -- bit 7 of the second header byte marks the last record (section 16.2.2).
const MULTIRECORD_END_OF_LIST: u8 = 0x80;

/// verify -- checks `image` against the FRU layouts; the startup self test and every accepted
/// Write FRU Data run it.
/// 1. The common header: version byte exactly 01h (BadVersion), zero checksum (BadChecksum),
///    pad byte 00h (BadPad, Table 8-1) and Chassis Info offset 00h (ChassisAreaPresent, VITA
///    Rule 8.1-3).
/// 2. The board and product areas are present (Rule 8.1-2; an offset of 00h is BadLength), and
///    each has version 1h (BadVersion), a nonzero length inside the image (BadLength), a zero
///    checksum (BadChecksum) and a C1h end-of-fields byte inside its field list (NoEndMarker).
/// 3. An Internal Use Area, when its offset is nonzero, starts with version 1h (BadVersion;
///    Table 9-1: the area has no length or checksum of its own).
/// 4. A MultiRecord Area, when its offset is nonzero, is a chain of records, each with format
///    version 2h (BadVersion), a zero header checksum and a zero data checksum (BadChecksum), data
///    inside the image (BadLength) and an end-of-list record before the image ends (NoEndMarker).
///    The chain walk is bounded by the image length.
///
/// This image has neither of the last two areas; a Chassis Manager may add one (VITA
/// Permissions 8.1-1 and 8.1-4), and then the layouts above are what it must follow.
pub fn verify(image: &[u8]) -> Result<(), FruError> {
    let head = common_header(image)?;
    if !head.checksum_ok {
        return Err(FruError::BadChecksum);
    }
    if head.pad != 0 {
        return Err(FruError::BadPad);
    }
    if head.chassis != 0 {
        return Err(FruError::ChassisAreaPresent);
    }
    for (units, kind) in [
        (head.board, AreaKind::Board),
        (head.product, AreaKind::Product),
    ] {
        if units == 0 {
            return Err(FruError::BadLength);
        }
        let found = area(image, kind, units)?;
        if !found.checksum_ok {
            return Err(FruError::BadChecksum);
        }
        if !found.has_end_marker() {
            return Err(FruError::NoEndMarker);
        }
    }
    if head.internal != 0 {
        verify_internal_use(image, head.internal)?;
    }
    if head.multirecord != 0 {
        verify_multirecord(image, head.multirecord)?;
    }
    Ok(())
}

/// verify_internal_use -- the Internal Use Area at `units` starts with format version 1h.
fn verify_internal_use(image: &[u8], units: u8) -> Result<(), FruError> {
    let version = image
        .get(usize::from(units) * AREA_ALIGN)
        .ok_or(FruError::TooShort)?;
    if *version != FORMAT_VERSION {
        return Err(FruError::BadVersion);
    }
    Ok(())
}

/// verify_multirecord -- walks the MultiRecord chain from `units` to its end-of-list record.
/// Every record takes at least MULTIRECORD_HEADER_LEN bytes, so fewer than `image.len()` records
/// fit; that bounds the loop.
fn verify_multirecord(image: &[u8], units: u8) -> Result<(), FruError> {
    let mut offset = usize::from(units) * AREA_ALIGN;
    for _ in 0..image.len() {
        let header_end = offset
            .checked_add(MULTIRECORD_HEADER_LEN)
            .ok_or(FruError::TooShort)?;
        let Some([type_id, flags, length, record_checksum, header_checksum]) =
            image.get(offset..header_end)
        else {
            return Err(FruError::TooShort);
        };
        if checksum(&[*type_id, *flags, *length, *record_checksum]) != *header_checksum {
            return Err(FruError::BadChecksum);
        }
        if flags & MULTIRECORD_VERSION_MASK != MULTIRECORD_VERSION {
            return Err(FruError::BadVersion);
        }
        let data_end = header_end
            .checked_add(usize::from(*length))
            .ok_or(FruError::BadLength)?;
        let data = image.get(header_end..data_end).ok_or(FruError::BadLength)?;
        if checksum(data) != *record_checksum {
            return Err(FruError::BadChecksum);
        }
        if flags & MULTIRECORD_END_OF_LIST != 0 {
            return Ok(());
        }
        offset = data_end;
    }
    Err(FruError::NoEndMarker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FruStore;

    // The expected bytes are written out from the FRU spec layouts (Tables 8-1, 11-1, 12-1,
    // section 13) for the default strings of ipmi_defs.h, with every checksum worked out apart
    // from `build`. A change to a default string changes them and fails these tests on purpose.

    /// Common header: version 01h; internal 00h; chassis 00h (VITA Rule 8.1-3); board at unit 1
    /// (offset 08h); product at unit 0Bh (offset 58h, the board area is 80 bytes); multirecord
    /// 00h; pad 00h; checksum 100h - (01h + 01h + 0Bh) = F3h.
    const HEADER: [u8; 8] = [0x01, 0x00, 0x00, 0x01, 0x0B, 0x00, 0x00, 0xF3];

    /// Board area at 08h: version 01h, length 0Ah (80 bytes), language 00h, date 16204320 =
    /// F74220h -> 20 42 F7, "IronKernel" (CAh), "VPX Tier-1 IPMC" (CFh), "IK-T1-000001" (CCh),
    /// "IK-IPMC-T1-NUC" (CEh), "ipmc-fru-v1" (CBh), C1h, 00h pad, checksum 51h.
    const BOARD: [u8; 80] = [
        0x01, 0x0A, 0x00, 0x20, 0x42, 0xF7, 0xCA, 0x49, 0x72, 0x6F, 0x6E, 0x4B, 0x65, 0x72, 0x6E,
        0x65, 0x6C, 0xCF, 0x56, 0x50, 0x58, 0x20, 0x54, 0x69, 0x65, 0x72, 0x2D, 0x31, 0x20, 0x49,
        0x50, 0x4D, 0x43, 0xCC, 0x49, 0x4B, 0x2D, 0x54, 0x31, 0x2D, 0x30, 0x30, 0x30, 0x30, 0x30,
        0x31, 0xCE, 0x49, 0x4B, 0x2D, 0x49, 0x50, 0x4D, 0x43, 0x2D, 0x54, 0x31, 0x2D, 0x4E, 0x55,
        0x43, 0xCB, 0x69, 0x70, 0x6D, 0x63, 0x2D, 0x66, 0x72, 0x75, 0x2D, 0x76, 0x31, 0xC1, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x51,
    ];

    /// Product area at 58h: version 01h, length 09h (72 bytes), language 00h, "IronKernel" (CAh),
    /// "Tier-1 IPMC Reference" (D5h), "IK-IPMC-T1" (CAh), "0.1" (C3h), "IK-T1-000001" (CCh), empty
    /// asset tag (C0h), empty file ID (C0h), C1h, 00h pad, checksum 1Bh.
    const PRODUCT: [u8; 72] = [
        0x01, 0x09, 0x00, 0xCA, 0x49, 0x72, 0x6F, 0x6E, 0x4B, 0x65, 0x72, 0x6E, 0x65, 0x6C, 0xD5,
        0x54, 0x69, 0x65, 0x72, 0x2D, 0x31, 0x20, 0x49, 0x50, 0x4D, 0x43, 0x20, 0x52, 0x65, 0x66,
        0x65, 0x72, 0x65, 0x6E, 0x63, 0x65, 0xCA, 0x49, 0x4B, 0x2D, 0x49, 0x50, 0x4D, 0x43, 0x2D,
        0x54, 0x31, 0xC3, 0x30, 0x2E, 0x31, 0xCC, 0x49, 0x4B, 0x2D, 0x54, 0x31, 0x2D, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x31, 0xC0, 0xC0, 0xC1, 0x00, 0x00, 0x00, 0x00, 0x1B,
    ];

    const BOARD_AT: usize = 0x08;
    const PRODUCT_AT: usize = 0x58;

    #[test]
    fn image_is_byte_exact() {
        let store = FruStore::new();
        assert_eq!(store.read(0, 8).unwrap(), &HEADER);
        assert_eq!(store.read(BOARD_AT, BOARD.len()).unwrap(), &BOARD);
        assert_eq!(store.read(PRODUCT_AT, PRODUCT.len()).unwrap(), &PRODUCT);
        assert_eq!(IMAGE_LEN, 8 + 80 + 72);
        // Everything after the areas is 00h padding.
        assert!(store.bytes()[IMAGE_LEN..].iter().all(|b| *b == 0));
        assert_eq!(store.bytes().len(), SIZE);
    }

    #[test]
    fn areas_are_multiples_of_8_and_checksums_sum_to_zero() {
        let sum = |bytes: &[u8]| bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        assert_eq!(sum(&HEADER), 0);
        assert_eq!(sum(&BOARD), 0);
        assert_eq!(sum(&PRODUCT), 0);
        assert_eq!(BOARD.len() % 8, 0);
        assert_eq!(PRODUCT.len() % 8, 0);
        assert_eq!(usize::from(BOARD[1]) * 8, BOARD.len());
        assert_eq!(usize::from(PRODUCT[1]) * 8, PRODUCT.len());
        assert_eq!(SIZE % 8, 0);
    }

    #[test]
    fn mfg_date_is_minutes_from_1996_to_2026_10_23() {
        // (30 years x 365 + 8 leap days) + 295 days = 11253 days; x 1440 minutes.
        assert_eq!(11_253u32 * 1_440, ipmc_defs::IPMC_FRU_MFG_MINUTES);
        assert_eq!(ipmc_defs::IPMC_FRU_MFG_MINUTES, 0x00F7_4220);
    }

    #[test]
    fn decoder_reads_the_image_back() {
        let store = FruStore::new();
        let head = common_header(store.bytes()).unwrap();
        assert_eq!(
            head,
            CommonHeader {
                internal: 0,
                chassis: 0,
                board: 1,
                product: 0x0B,
                multirecord: 0,
                pad: 0,
                checksum_ok: true
            }
        );
        let board = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        assert!(board.checksum_ok);
        assert_eq!(board.bytes, &BOARD);
        assert_eq!(board.mfg_minutes(), Some(16_204_320));
        let names: Vec<&[u8]> = board.fields().map(|f| f.data).collect();
        assert_eq!(
            names,
            [
                &b"IronKernel"[..],
                b"VPX Tier-1 IPMC",
                b"IK-T1-000001",
                b"IK-IPMC-T1-NUC",
                b"ipmc-fru-v1"
            ]
        );
        assert!(board.fields().all(|f| f.is_ascii()));
        let product = area(store.bytes(), AreaKind::Product, head.product).unwrap();
        assert!(product.checksum_ok);
        assert_eq!(product.mfg_minutes(), None);
        let fields: Vec<&[u8]> = product.fields().map(|f| f.data).collect();
        assert_eq!(fields.len(), 7);
        assert_eq!(fields[1], b"Tier-1 IPMC Reference");
        assert_eq!(fields[3], b"0.1");
        assert!(fields[5].is_empty() && fields[6].is_empty());
    }

    #[test]
    fn decoder_rejects_damaged_structures() {
        assert_eq!(common_header(&[1, 0, 0]), Err(FruError::TooShort));
        assert_eq!(
            common_header(&[2, 0, 0, 1, 11, 0, 0, 0]),
            Err(FruError::BadVersion)
        );
        let mut bytes = FruStore::new().bytes().to_vec();
        bytes[8 + 79] ^= 0x01;
        let board = area(&bytes, AreaKind::Board, 1).unwrap();
        assert!(!board.checksum_ok);
        bytes[9] = 0xFF; // length past the image
        assert_eq!(area(&bytes, AreaKind::Board, 1), Err(FruError::BadLength));
        bytes[9] = 0;
        assert_eq!(area(&bytes, AreaKind::Board, 1), Err(FruError::BadLength));
        bytes[8] = 0x02;
        assert_eq!(area(&bytes, AreaKind::Board, 1), Err(FruError::BadVersion));
        assert_eq!(area(&bytes, AreaKind::Board, 200), Err(FruError::TooShort));
    }

    #[test]
    fn fields_stop_at_a_field_that_overruns_the_area() {
        // version, length 1 unit, language, then a field claiming 5 bytes with 2 present.
        let bytes = [0x01, 0x01, 0x00, 0xC5, 0x41, 0x42, 0x00, 0x00];
        let a = Area {
            kind: AreaKind::Product,
            bytes: &bytes,
            checksum_ok: false,
        };
        assert_eq!(a.fields().count(), 0);
    }

    // ---- verify ----

    /// An image copy with `change` applied.
    fn altered(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut bytes = FruStore::new().bytes().to_vec();
        change(&mut bytes);
        bytes
    }

    #[test]
    fn the_built_image_verifies() {
        assert_eq!(verify(FruStore::new().bytes()), Ok(()));
    }

    #[test]
    fn a_flipped_byte_in_each_checksummed_part_is_a_bad_checksum() {
        // Header pad byte 6, a board string byte, a product string byte.
        for at in [6, BOARD_AT + 20, PRODUCT_AT + 20] {
            let bytes = altered(|b| b[at] ^= 0x01);
            assert_eq!(verify(&bytes), Err(FruError::BadChecksum), "byte {at}");
        }
    }

    /// Fault -- one edit of the built image (named) and the error `verify` must report for it.
    struct Fault(&'static str, fn(&mut Vec<u8>), FruError);

    #[test]
    fn header_and_area_faults_are_each_named() {
        // FRU Table 8-1: version 01h exactly. F1h keeps the version nibble but sets reserved
        // bits 7:4; this header sums to 0, so only the version check can reject it.
        let reserved = [0xF1, 0x00, 0x00, 0x01, 0x0B, 0x00, 0x00, 0x03];
        assert_eq!(checksum(&reserved), 0);
        // Each case edits the image; the header checksum is rewritten after the edit so only
        // the named fault remains.
        let cases: [Fault; 8] = [
            Fault(
                "reserved version bits set",
                |b| b[0] = 0xF1,
                FruError::BadVersion,
            ),
            Fault(
                "Rule 8.1-3: no Chassis Info Area",
                |b| b[2] = 1,
                FruError::ChassisAreaPresent,
            ),
            Fault(
                "Table 8-1: the pad byte is 00h",
                |b| b[6] = 1,
                FruError::BadPad,
            ),
            Fault(
                "Table 11-1: reserved bits 7:4 of the board version",
                |b| b[BOARD_AT] = 0x11,
                FruError::BadVersion,
            ),
            Fault(
                "Table 12-1: reserved bits 7:4 of the product version",
                |b| b[PRODUCT_AT] = 0x81,
                FruError::BadVersion,
            ),
            Fault(
                "Rule 8.1-2: the board area is present",
                |b| b[3] = 0,
                FruError::BadLength,
            ),
            Fault("header version 2", |b| b[0] = 2, FruError::BadVersion),
            Fault(
                "board area version 2",
                |b| b[BOARD_AT] = 2,
                FruError::BadVersion,
            ),
        ];
        for Fault(name, edit, want) in cases {
            let bytes = altered(|b| {
                edit(b);
                b[7] = checksum(&b[..7]);
            });
            assert_eq!(verify(&bytes), Err(want), "{name}");
        }
        assert_eq!(FruError::BadPad.name(), "BadPad");
        assert_eq!(FruError::ChassisAreaPresent.name(), "ChassisAreaPresent");
    }

    #[test]
    fn a_zeroed_area_length_is_a_bad_length() {
        let bytes = altered(|b| b[BOARD_AT + 1] = 0);
        assert_eq!(verify(&bytes), Err(FruError::BadLength));
        let bytes = altered(|b| b[PRODUCT_AT + 1] = 0);
        assert_eq!(verify(&bytes), Err(FruError::BadLength));
        // A length that runs past the image.
        let bytes = altered(|b| b[BOARD_AT + 1] = 0xFF);
        assert_eq!(verify(&bytes), Err(FruError::BadLength));
    }

    #[test]
    fn an_area_without_the_c1_marker_has_no_end_marker() {
        // Replace the board's C1h by C0h (an empty field) and fix the area checksum.
        let marker = BOARD_AT + 73;
        let last = BOARD_AT + BOARD.len() - 1;
        let bytes = altered(|b| {
            assert_eq!(b[marker], 0xC1);
            b[marker] = 0xC0;
            b[last] = checksum(&b[BOARD_AT..last]);
        });
        assert_eq!(verify(&bytes), Err(FruError::NoEndMarker));
    }

    /// Image with a MultiRecord area at unit 14h (offset A0h, after the product area): one OEM
    /// record, type C0h, end of list and version 2 (82h), 2 data bytes AAh 55h, record checksum
    /// 01h (AAh + 55h = FFh), header checksum -(C0h + 82h + 02h + 01h) = BBh.
    fn with_multirecord(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        altered(|b| {
            b[5] = 0x14;
            b[7] = checksum(&b[..7]);
            b[IMAGE_LEN..IMAGE_LEN + 7]
                .copy_from_slice(&[0xC0, 0x82, 0x02, 0x01, 0xBB, 0xAA, 0x55]);
            change(b);
        })
    }

    #[test]
    fn a_multirecord_chain_is_walked() {
        assert_eq!(verify(&with_multirecord(|_| {})), Ok(()));
        let a = IMAGE_LEN;
        // Header checksum, data checksum, version, data past the image, no end-of-list bit.
        assert_eq!(
            verify(&with_multirecord(|b| b[a + 4] ^= 1)),
            Err(FruError::BadChecksum)
        );
        assert_eq!(
            verify(&with_multirecord(|b| b[a + 5] ^= 1)),
            Err(FruError::BadChecksum)
        );
        assert_eq!(
            verify(&with_multirecord(|b| {
                b[a + 1] = 0x83;
                b[a + 4] = checksum(&b[a..a + 4]);
            })),
            Err(FruError::BadVersion)
        );
        assert_eq!(
            verify(&with_multirecord(|b| {
                b[a + 2] = 0xFF;
                b[a + 4] = checksum(&b[a..a + 4]);
            })),
            Err(FruError::BadLength)
        );
        // Table 16-1: reserved bits 6:4 of the format byte are 0 (92h = end of list, version 2h,
        // bit 4 set).
        assert_eq!(
            verify(&with_multirecord(|b| {
                b[a + 1] = 0x92;
                b[a + 4] = checksum(&b[a..a + 4]);
            })),
            Err(FruError::BadVersion)
        );
        // Without the end-of-list bit the walk continues into the zero padding, whose header
        // does not check out as version 2h.
        assert_eq!(
            verify(&with_multirecord(|b| {
                b[a + 1] = 0x02;
                b[a + 4] = checksum(&b[a..a + 4]);
            })),
            Err(FruError::BadVersion)
        );
    }

    #[test]
    fn an_internal_use_area_needs_version_1() {
        let with_internal = |version: u8| {
            altered(|b| {
                b[1] = 0x14;
                b[7] = checksum(&b[..7]);
                b[IMAGE_LEN] = version;
            })
        };
        assert_eq!(verify(&with_internal(1)), Ok(()));
        assert_eq!(verify(&with_internal(2)), Err(FruError::BadVersion));
        assert_eq!(verify(&with_internal(0x11)), Err(FruError::BadVersion));
    }
}
