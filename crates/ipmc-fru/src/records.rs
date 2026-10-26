//! records.rs - the FRU Information records of FRU 0: layouts, the built image, the decoder and
//! `verify`.
//!
//! Layouts are from the IPMI Platform Management FRU Information Storage Definition v1.0 rev 1.3
//! ("FRU spec" below): Table 8-1 common header, Table 11-1 board info area, Table 12-1 product
//! info area, section 13 type/length byte. VITA 46.11 rules: Common Header, Board Info Area and
//! Product Info Area are present (Rule 8.1-2), the Chassis Info offset is 00h (Rule 8.1-3) and the
//! Internal Use area is absent (offset 00h, Permission 8.1-1). The MultiRecord area (Permission
//! 8.1-4) holds one record, the VITA 46.11 Alternate Manufacturing Date/Time record (Table
//! 10.3.10-1): the standard 3-byte board date ends at 2027-11-24 20:15, the record's 32-bit
//! minutes count does not.
//!
//! ```text
//! offset 00h  common header, 8 bytes
//!        08h  board info area, a multiple of 8 bytes (fields, C1h, room for the per-unit fields)
//!        ..   product info area, a multiple of 8 bytes (the same)
//!        ..   MultiRecord area: the VITA 46.11 Alternate Mfg Date/Time record (14 bytes)
//!        ..   00h padding up to IPMC_FRU_SIZE
//! ```
//!
//! The image is built at compile time from the IPMC_FRU_* defines of ipmi_defs.h: a string
//! the area cannot hold, or an image larger than IPMC_FRU_SIZE, fails the build. `verify`
//! checks a (possibly rewritten) image against the same layouts; the startup self test and
//! every accepted Write FRU Data call it through ipmc-core `selftest`.

use crate::date;
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
pub(crate) const END_OF_FIELDS: u8 = 0xC1;
/// FIELD_LEN_MAX -- longest field a 6-bit length holds.
const FIELD_LEN_MAX: usize = 63;
/// MFG_DATE_LEN -- the board manufacturing date/time is 3 bytes, LS byte first (FRU spec Table 11-1).
pub(crate) const MFG_DATE_LEN: usize = 3;
/// Bytes of an area before its first type/length field: version, length, language code, and for
/// the board area the 3-byte manufacturing date (FRU spec Tables 11-1 and 12-1).
pub(crate) const PRODUCT_HEADER_LEN: usize = 3;
pub(crate) const BOARD_HEADER_LEN: usize = PRODUCT_HEADER_LEN + MFG_DATE_LEN;

/// MR_TYPE_OEM -- MultiRecord type ID C0h, an OEM record (FRU spec Table 16-1; VITA 46.11
/// Table 10.3.10-1).
pub(crate) const MR_TYPE_OEM: u8 = 0xC0;
/// MR_FLAGS_LAST -- the end-of-list bit of the second MultiRecord header byte.
const MR_FLAGS_LAST: u8 = 0x80;
/// VITA_MFR_ID -- the VITA manufacturer ID 0081ACh, LS byte first (VITA 46.11 Table 10.3.10-1).
pub(crate) const VITA_MFR_ID: [u8; 3] = [0xAC, 0x81, 0x00];
/// VITA_ALT_MFG_ID -- VITA record ID 34h, the Alternate Manufacturing Date/Time record.
pub(crate) const VITA_ALT_MFG_ID: u8 = 0x34;
/// VITA_ALT_MFG_VERSION -- the record's format version, 00h.
pub(crate) const VITA_ALT_MFG_VERSION: u8 = 0;
/// ALT_MFG_DATA_LEN -- data bytes of the record: manufacturer ID 3, record ID 1, version 1,
/// minutes 4.
pub(crate) const ALT_MFG_DATA_LEN: usize = 9;
/// ALT_MFG_MINUTES_AT -- offset of the 4-byte minutes count in the record data.
pub(crate) const ALT_MFG_MINUTES_AT: usize = 5;

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

/// reserve -- the bytes one field leaves free in the built area: the room the define reserves
/// for it (`max`) less the length of its default string. An update up to `max` bytes then
/// rewrites the field in place and no area moves (`unit.rs`). The guards in `ipmc-defs` keep the
/// default within `max`. An area's pad is the sum of this over its fields.
const fn reserve(max: u32, default: &str) -> usize {
    max as usize - default.len()
}

/// finish_area -- closes the area that began at `start`: the C1h end-of-fields byte, `reserve`
/// bytes of 00h (the room for the fields, FRU spec section 10 allows 00h padding after C1h up
/// to the area length), 00h padding to a multiple of 8 bytes that leaves one byte for the
/// checksum, the area length in byte 1 (in 8-byte units) and the zero checksum.
const fn finish_area(img: &mut Image, start: usize, reserve: usize) {
    img.push(END_OF_FIELDS);
    let mut left = reserve;
    while left > 0 {
        img.push(0);
        left -= 1;
    }
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

/// BOARD_RESERVE -- the pad of the built board area: the room of its five text fields.
const BOARD_RESERVE: usize = reserve(
    ipmc_defs::IPMC_FRU_BOARD_MFR_MAX,
    ipmc_defs::IPMC_FRU_BOARD_MFR,
) + reserve(
    ipmc_defs::IPMC_FRU_BOARD_PRODUCT_MAX,
    ipmc_defs::IPMC_FRU_BOARD_PRODUCT,
) + reserve(
    ipmc_defs::IPMC_FRU_BOARD_SERIAL_MAX,
    ipmc_defs::IPMC_FRU_BOARD_SERIAL,
) + reserve(
    ipmc_defs::IPMC_FRU_BOARD_PART_MAX,
    ipmc_defs::IPMC_FRU_BOARD_PART,
) + reserve(
    ipmc_defs::IPMC_FRU_BOARD_FILE_ID_MAX,
    ipmc_defs::IPMC_FRU_FILE_ID,
);

/// PRODUCT_RESERVE -- the pad of the built product area: the room of its seven text fields.
const PRODUCT_RESERVE: usize = reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_MFR_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_MFR,
) + reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_NAME_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_NAME,
) + reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_PART_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_PART,
) + reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_VERSION_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_VERSION,
) + reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_SERIAL_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_SERIAL,
) + reserve(
    ipmc_defs::IPMC_FRU_ASSET_TAG_MAX,
    ipmc_defs::IPMC_FRU_ASSET_TAG,
) + reserve(
    ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID_MAX,
    ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID,
);

/// push_alt_mfg_record -- the VITA 46.11 Alternate Manufacturing Date/Time record (Table
/// 10.3.10-1), 14 bytes: header type C0h (OEM), 82h (end of list, record format version 2), data
/// length 09h, the data checksum and the header checksum (FRU spec Table 16-1), then the data:
/// the VITA manufacturer ID 0081ACh LS byte first, record ID 34h, format version 00h and the
/// date as 32-bit minutes since 1996-01-01 00:00 UTC, LS byte first (00000000h = unspecified).
const fn push_alt_mfg_record(img: &mut Image, minutes: u32) {
    let minutes = minutes.to_le_bytes();
    let data: [u8; ALT_MFG_DATA_LEN] = [
        VITA_MFR_ID[0],
        VITA_MFR_ID[1],
        VITA_MFR_ID[2],
        VITA_ALT_MFG_ID,
        VITA_ALT_MFG_VERSION,
        minutes[0],
        minutes[1],
        minutes[2],
        minutes[3],
    ];
    let data_sum = checksum(&data);
    let head = [
        MR_TYPE_OEM,
        MR_FLAGS_LAST | MULTIRECORD_VERSION,
        ALT_MFG_DATA_LEN as u8,
        data_sum,
    ];
    img.push_all(&head);
    img.push(checksum(&head));
    img.push_all(&data);
}

/// build -- the FRU image from the IPMC_FRU_* defines.
const fn build() -> Image {
    let mut img = Image::with_len(COMMON_HEADER_LEN);

    // Board Info Area (FRU spec Table 11-1). The standard 3-byte Mfg Date/Time is 000000h
    // (unspecified): the date lives in the Alternate Mfg Date/Time record below (date.rs).
    let board = start_area(&mut img);
    img.push(LANGUAGE_ENGLISH);
    let mut i = 0;
    while i < MFG_DATE_LEN {
        img.push(0);
        i += 1;
    }
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_MFR);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_PRODUCT);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_SERIAL);
    push_text(&mut img, ipmc_defs::IPMC_FRU_BOARD_PART);
    push_text(&mut img, ipmc_defs::IPMC_FRU_FILE_ID);
    finish_area(&mut img, board, BOARD_RESERVE);

    // Product Info Area (FRU spec Table 12-1).
    let product = start_area(&mut img);
    img.push(LANGUAGE_ENGLISH);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_MFR);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_NAME);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_PART);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_VERSION);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_SERIAL);
    push_text(&mut img, ipmc_defs::IPMC_FRU_ASSET_TAG);
    push_text(&mut img, ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID);
    finish_area(&mut img, product, PRODUCT_RESERVE);

    // MultiRecord Area: the Alternate Mfg Date/Time record, the only record, so the last.
    let multirecord = img.len();
    assert!(
        multirecord.is_multiple_of(AREA_ALIGN),
        "the MultiRecord area starts on an 8-byte boundary"
    );
    push_alt_mfg_record(&mut img, date::DEFAULT_MINUTES);

    // Common header (FRU spec Table 8-1): Internal Use, Chassis Info and MultiRecord are absent.
    img.bytes[HDR_VERSION] = FORMAT_VERSION;
    img.bytes[HDR_INTERNAL] = 0;
    img.bytes[HDR_CHASSIS] = 0;
    img.bytes[HDR_BOARD] = (board / AREA_ALIGN) as u8;
    img.bytes[HDR_PRODUCT] = (product / AREA_ALIGN) as u8;
    img.bytes[HDR_MULTIRECORD] = (multirecord / AREA_ALIGN) as u8;
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
pub(crate) const MULTIRECORD_HEADER_LEN: usize = 5;
/// MULTIRECORD_VERSION -- record format version 2h (FRU spec section 16.2.3); bits 3:0.
const MULTIRECORD_VERSION: u8 = 2;
/// MULTIRECORD_END_OF_LIST -- bit 7 of the second header byte marks the last record (section 16.2.2).
const MULTIRECORD_END_OF_LIST: u8 = 0x80;

/// MultiRecord -- one decoded MultiRecord (FRU spec Table 16-1), borrowing the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiRecord<'a> {
    /// Offset of the record header in the image.
    pub offset: usize,
    /// Record type ID.
    pub type_id: u8,
    /// Second header byte: end-of-list bit 7, format version in bits 3:0.
    pub flags: u8,
    /// The record data (header excluded).
    pub data: &'a [u8],
    /// The header checksum byte is the zero checksum of the first four header bytes.
    pub header_ok: bool,
    /// The record checksum byte is the zero checksum of the data.
    pub data_ok: bool,
}

impl MultiRecord<'_> {
    /// is_last -- the end-of-list bit is set.
    pub const fn is_last(&self) -> bool {
        self.flags & MULTIRECORD_END_OF_LIST != 0
    }

    /// alt_mfg_minutes -- the date of a VITA 46.11 Alternate Manufacturing Date/Time record (OEM
    /// type C0h, manufacturer ID 0081ACh, record ID 34h, format version 00h, four bytes of
    /// minutes since 1996-01-01 00:00): None for any other record or a malformed one. The
    /// checksums are not part of the shape (`header_ok`, `data_ok`).
    pub fn alt_mfg_minutes(&self) -> Option<u32> {
        let [m0, m1, m2, m3] = *self.data.get(ALT_MFG_MINUTES_AT..)?.first_chunk::<4>()?;
        let shape_ok = self.type_id == MR_TYPE_OEM
            && self.flags & MULTIRECORD_VERSION_MASK == MULTIRECORD_VERSION
            && self.data.len() == ALT_MFG_DATA_LEN
            && self.data.starts_with(&VITA_MFR_ID)
            && self.data.get(VITA_MFR_ID.len()) == Some(&VITA_ALT_MFG_ID)
            && self.data.get(VITA_MFR_ID.len() + 1) == Some(&VITA_ALT_MFG_VERSION);
        shape_ok.then(|| u32::from_le_bytes([m0, m1, m2, m3]))
    }
}

/// MultiRecords -- iterator over the MultiRecord chain from `multirecords`; it ends after the
/// end-of-list record, at a record that runs past the image, and after at most one record per
/// image byte.
#[derive(Clone, Copy, Debug)]
pub struct MultiRecords<'a> {
    image: &'a [u8],
    offset: usize,
    left: usize,
}

/// multirecords -- the MultiRecord chain that starts at `units` (8-byte units, the common header
/// byte 5). Checksums are reported per record, not enforced (`verify` enforces them).
pub fn multirecords(image: &[u8], units: u8) -> MultiRecords<'_> {
    MultiRecords {
        image,
        offset: usize::from(units) * AREA_ALIGN,
        left: if units == 0 { 0 } else { image.len() },
    }
}

impl<'a> Iterator for MultiRecords<'a> {
    type Item = MultiRecord<'a>;

    fn next(&mut self) -> Option<MultiRecord<'a>> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let header_end = self.offset.checked_add(MULTIRECORD_HEADER_LEN)?;
        let [type_id, flags, length, record_checksum, header_checksum] = *self
            .image
            .get(self.offset..header_end)?
            .first_chunk::<5>()?;
        let data_end = header_end.checked_add(usize::from(length))?;
        let data = self.image.get(header_end..data_end)?;
        let found = MultiRecord {
            offset: self.offset,
            type_id,
            flags,
            data,
            header_ok: checksum(&[type_id, flags, length, record_checksum]) == header_checksum,
            data_ok: checksum(data) == record_checksum,
        };
        self.left = if found.is_last() { 0 } else { self.left };
        self.offset = data_end;
        Some(found)
    }
}

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
/// The built image has no Internal Use area and one MultiRecord record (the Alternate Mfg
/// Date/Time record); a Chassis Manager may add an Internal Use area (VITA Permission 8.1-1) or
/// records (Permission 8.1-4), and then the layouts above are what it must follow.
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

    /// Room the defaults leave, per field (MAX - default length): board area 14 (manufacturer),
    /// 17 (product name), 12 (serial), 10 (part), 5 (file ID) = 58; product area 14, 11, 14,
    /// 13 (version), 12, 24 (asset tag), 16 (file ID) = 104.
    ///
    /// Board area bytes: 6 header + (1+10) + (1+15) + (1+12) + (1+14) + (1+11) = 73 field bytes,
    /// C1h = 74, + 58 reserved = 132, + 1 checksum = 133, rounded up to 8 = 136.
    /// Product area bytes: 3 header + (1+10) + (1+21) + (1+10) + (1+3) + (1+12) + 1 + 1 = 66,
    /// C1h = 67, + 104 reserved = 171, + 1 checksum = 172, rounded up to 8 = 176.
    const BOARD_LEN: usize = 136;
    const PRODUCT_LEN: usize = 176;

    /// Common header: version 01h; internal 00h; chassis 00h (VITA Rule 8.1-3); board at unit 1
    /// (offset 08h); product at unit 12h (offset 90h, the board area is 136 bytes); multirecord
    /// at unit 28h (offset 140h, after the 176-byte product area); pad 00h; checksum
    /// 100h - (01h + 01h + 12h + 28h) = C4h.
    const HEADER: [u8; 8] = [0x01, 0x00, 0x00, 0x01, 0x12, 0x28, 0x00, 0xC4];

    /// Board area at 08h up to and including the C1h byte: version 01h, length 11h (136 bytes),
    /// language 00h, the standard date 000000h (unspecified), "IronKernel" (CAh), "VPX Tier-1
    /// IPMC" (CFh), "IK-T1-000001" (CCh), "IK-IPMC-T1-NUC" (CEh), "ipmc-fru-v1" (CBh), C1h. After
    /// it come 61 bytes of 00h (58 reserved, 3 alignment) and the checksum.
    const BOARD_BODY: [u8; 74] = [
        0x01, 0x11, 0x00, 0x00, 0x00, 0x00, 0xCA, 0x49, 0x72, 0x6F, 0x6E, 0x4B, 0x65, 0x72, 0x6E,
        0x65, 0x6C, 0xCF, 0x56, 0x50, 0x58, 0x20, 0x54, 0x69, 0x65, 0x72, 0x2D, 0x31, 0x20, 0x49,
        0x50, 0x4D, 0x43, 0xCC, 0x49, 0x4B, 0x2D, 0x54, 0x31, 0x2D, 0x30, 0x30, 0x30, 0x30, 0x30,
        0x31, 0xCE, 0x49, 0x4B, 0x2D, 0x49, 0x50, 0x4D, 0x43, 0x2D, 0x54, 0x31, 0x2D, 0x4E, 0x55,
        0x43, 0xCB, 0x69, 0x70, 0x6D, 0x63, 0x2D, 0x66, 0x72, 0x75, 0x2D, 0x76, 0x31, 0xC1,
    ];

    /// Product area at 90h up to and including the C1h byte: version 01h, length 16h (176 bytes),
    /// language 00h, "IronKernel" (CAh), "Tier-1 IPMC Reference" (D5h), "IK-IPMC-T1" (CAh),
    /// "0.1" (C3h), "IK-T1-000001" (CCh), empty asset tag (C0h), empty file ID (C0h), C1h.
    /// After it come 108 bytes of 00h (104 reserved, 4 alignment) and the checksum.
    const PRODUCT_BODY: [u8; 67] = [
        0x01, 0x16, 0x00, 0xCA, 0x49, 0x72, 0x6F, 0x6E, 0x4B, 0x65, 0x72, 0x6E, 0x65, 0x6C, 0xD5,
        0x54, 0x69, 0x65, 0x72, 0x2D, 0x31, 0x20, 0x49, 0x50, 0x4D, 0x43, 0x20, 0x52, 0x65, 0x66,
        0x65, 0x72, 0x65, 0x6E, 0x63, 0x65, 0xCA, 0x49, 0x4B, 0x2D, 0x49, 0x50, 0x4D, 0x43, 0x2D,
        0x54, 0x31, 0xC3, 0x30, 0x2E, 0x31, 0xCC, 0x49, 0x4B, 0x2D, 0x54, 0x31, 0x2D, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x31, 0xC0, 0xC0, 0xC1,
    ];

    /// MultiRecord area at 140h: the VITA 46.11 Alternate Mfg Date/Time record (Table
    /// 10.3.10-1), 14 bytes. Header: C0h (OEM), 82h (end of list, version 2), 09h data length,
    /// data checksum, header checksum = 100h - (C0h + 82h + 09h + data checksum). Data: ACh 81h
    /// 00h (manufacturer ID 0081ACh), 34h (record ID), 00h (format version) and 16204320 minutes
    /// (2026-10-23 00:00) = 00F74220h LS byte first. Data checksum 100h - (ACh + 81h + 34h + 20h
    /// + 42h + F7h) mod 256 = 46h.
    const RECORD: [u8; 14] = [
        0xC0, 0x82, 0x09, 0x46, 0x6F, 0xAC, 0x81, 0x00, 0x34, 0x00, 0x20, 0x42, 0xF7, 0x00,
    ];

    const BOARD_AT: usize = 0x08;
    const PRODUCT_AT: usize = 0x90;
    const RECORD_AT: usize = 0x140;
    /// SPARE_AT -- the first 8-byte boundary after the built image: where the tests place an
    /// Internal Use area or a MultiRecord area of their own.
    const SPARE_AT: usize = 0x150;

    /// sum8 -- the bytes added modulo 256.
    fn sum8(bytes: &[u8]) -> u8 {
        bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
    }

    #[test]
    fn image_is_byte_exact() {
        let store = FruStore::new();
        assert_eq!(store.read(0, 8).unwrap(), &HEADER);
        for (at, len, body) in [
            (BOARD_AT, BOARD_LEN, &BOARD_BODY[..]),
            (PRODUCT_AT, PRODUCT_LEN, &PRODUCT_BODY[..]),
        ] {
            let area = store.read(at, len).unwrap();
            assert_eq!(&area[..body.len()], body);
            // Reserved room and alignment padding are 00h up to the checksum byte.
            assert!(area[body.len()..len - 1].iter().all(|b| *b == 0));
            assert_eq!(sum8(area), 0);
        }
        assert_eq!(&store.bytes()[RECORD_AT..RECORD_AT + 14], &RECORD);
        assert_eq!(IMAGE_LEN, RECORD_AT + RECORD.len());
        assert_eq!(RECORD_AT, 8 + BOARD_LEN + PRODUCT_LEN);
        // Everything after the areas is 00h padding.
        assert!(store.bytes()[IMAGE_LEN..].iter().all(|b| *b == 0));
        assert_eq!(store.bytes().len(), SIZE);
    }

    #[test]
    fn built_areas_have_exactly_the_reserved_length() {
        // Each reserve is MAX - default length (ipmi_defs.h IPMC_FRU_*_MAX); the area length is
        // the field bytes with the defaults, C1h, the reserves and the checksum, rounded up to 8.
        let rooms = |pairs: &[(u32, &str)]| -> (usize, usize) {
            (
                pairs.iter().map(|(_, text)| 1 + text.len()).sum::<usize>(),
                pairs
                    .iter()
                    .map(|(max, text)| *max as usize - text.len())
                    .sum::<usize>(),
            )
        };
        let (board_fields, board_room) = rooms(&[
            (
                ipmc_defs::IPMC_FRU_BOARD_MFR_MAX,
                ipmc_defs::IPMC_FRU_BOARD_MFR,
            ),
            (
                ipmc_defs::IPMC_FRU_BOARD_PRODUCT_MAX,
                ipmc_defs::IPMC_FRU_BOARD_PRODUCT,
            ),
            (
                ipmc_defs::IPMC_FRU_BOARD_SERIAL_MAX,
                ipmc_defs::IPMC_FRU_BOARD_SERIAL,
            ),
            (
                ipmc_defs::IPMC_FRU_BOARD_PART_MAX,
                ipmc_defs::IPMC_FRU_BOARD_PART,
            ),
            (
                ipmc_defs::IPMC_FRU_BOARD_FILE_ID_MAX,
                ipmc_defs::IPMC_FRU_FILE_ID,
            ),
        ]);
        let (product_fields, product_room) = rooms(&[
            (
                ipmc_defs::IPMC_FRU_PRODUCT_MFR_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_MFR,
            ),
            (
                ipmc_defs::IPMC_FRU_PRODUCT_NAME_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_NAME,
            ),
            (
                ipmc_defs::IPMC_FRU_PRODUCT_PART_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_PART,
            ),
            (
                ipmc_defs::IPMC_FRU_PRODUCT_VERSION_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_VERSION,
            ),
            (
                ipmc_defs::IPMC_FRU_PRODUCT_SERIAL_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_SERIAL,
            ),
            (
                ipmc_defs::IPMC_FRU_ASSET_TAG_MAX,
                ipmc_defs::IPMC_FRU_ASSET_TAG,
            ),
            (
                ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID_MAX,
                ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID,
            ),
        ]);
        // fields + C1h + room + checksum, up to a multiple of 8.
        let rounded = |used: usize| (used + 2).next_multiple_of(AREA_ALIGN);
        let board_used = BOARD_HEADER_LEN + board_fields + board_room;
        let product_used = PRODUCT_HEADER_LEN + product_fields + product_room;
        let store = FruStore::new();
        let head = common_header(store.bytes()).unwrap();
        let board = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        let product = area(store.bytes(), AreaKind::Product, head.product).unwrap();
        assert_eq!(board.bytes.len(), rounded(board_used));
        assert_eq!(product.bytes.len(), rounded(product_used));
        assert_eq!(
            (board.bytes.len(), product.bytes.len()),
            (BOARD_LEN, PRODUCT_LEN)
        );
    }

    #[test]
    fn areas_are_multiples_of_8_and_checksums_sum_to_zero() {
        assert_eq!(sum8(&HEADER), 0);
        let store = FruStore::new();
        assert_eq!(usize::from(store.bytes()[BOARD_AT + 1]) * 8, BOARD_LEN);
        assert_eq!(usize::from(store.bytes()[PRODUCT_AT + 1]) * 8, PRODUCT_LEN);
        assert_eq!(BOARD_LEN % 8, 0);
        assert_eq!(PRODUCT_LEN % 8, 0);
        assert_eq!(SIZE % 8, 0);
    }

    #[test]
    fn the_built_date_is_the_alt_record_and_the_standard_field_is_zero() {
        // 2026-10-23 00:00: (30 years x 365 + 8 leap days) + 295 days = 11253 days x 1440
        // minutes; the define is the text, the standard field is 000000h.
        assert_eq!(ipmc_defs::IPMC_FRU_MFG_DATE, "2026-10-23 00:00");
        assert_eq!(date::DEFAULT_MINUTES, 11_253u32 * 1_440);
        let store = FruStore::new();
        let head = common_header(store.bytes()).unwrap();
        let board = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        assert_eq!(board.mfg_minutes(), Some(0));
        let record = multirecords(store.bytes(), head.multirecord)
            .next()
            .unwrap();
        assert_eq!(record.alt_mfg_minutes(), Some(date::DEFAULT_MINUTES));
        assert!(record.is_last() && record.header_ok && record.data_ok);
        assert_eq!(multirecords(store.bytes(), head.multirecord).count(), 1);
        assert_eq!(multirecords(store.bytes(), 0).count(), 0);
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
                product: 0x12,
                multirecord: 0x28,
                pad: 0,
                checksum_ok: true
            }
        );
        let board = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        assert!(board.checksum_ok);
        assert_eq!(board.bytes.len(), BOARD_LEN);
        assert_eq!(board.mfg_minutes(), Some(0));
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
        bytes[BOARD_AT + BOARD_LEN - 1] ^= 0x01;
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
        let reserved = [0xF1, 0x00, 0x00, 0x01, 0x0E, 0x00, 0x00, 0x00];
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
        let last = BOARD_AT + BOARD_LEN - 1;
        let bytes = altered(|b| {
            assert_eq!(b[marker], 0xC1);
            b[marker] = 0xC0;
            b[last] = checksum(&b[BOARD_AT..last]);
        });
        assert_eq!(verify(&bytes), Err(FruError::NoEndMarker));
    }

    /// Image with a MultiRecord area at unit SPARE_AT / 8 (after the built record): one OEM
    /// record, type C0h, end of list and version 2 (82h), 2 data bytes AAh 55h, record checksum
    /// 01h (AAh + 55h = FFh), header checksum -(C0h + 82h + 02h + 01h) = BBh.
    fn with_multirecord(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        altered(|b| {
            b[5] = (SPARE_AT / AREA_ALIGN) as u8;
            b[7] = checksum(&b[..7]);
            b[SPARE_AT..SPARE_AT + 7].copy_from_slice(&[0xC0, 0x82, 0x02, 0x01, 0xBB, 0xAA, 0x55]);
            change(b);
        })
    }

    #[test]
    fn a_multirecord_chain_is_walked() {
        assert_eq!(verify(&with_multirecord(|_| {})), Ok(()));
        let a = SPARE_AT;
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
                b[1] = (SPARE_AT / AREA_ALIGN) as u8;
                b[7] = checksum(&b[..7]);
                b[SPARE_AT] = version;
            })
        };
        assert_eq!(verify(&with_internal(1)), Ok(()));
        assert_eq!(verify(&with_internal(2)), Err(FruError::BadVersion));
        assert_eq!(verify(&with_internal(0x11)), Err(FruError::BadVersion));
    }
}
