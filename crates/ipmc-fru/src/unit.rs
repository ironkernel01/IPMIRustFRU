//! unit.rs - the FRU fields: every text field of the board and product areas and the
//! manufacturing date, readable and writable one at a time, and the editor that rewrites one
//! field in place.
//!
//! `records.rs` builds each area with room for these fields (the `IPMC_FRU_*_MAX` defines of
//! ipmi_defs.h): after the C1h end-of-fields byte the area holds 00h bytes up to its length. An
//! update rewrites only its own area. It decodes the fields, writes them back in the same order
//! with the one field replaced, then C1h, 00h up to the area's old length and the zero checksum.
//! The area length never changes, so no other area moves. When the new fields do not fit (an
//! image a Chassis Manager wrote may have no spare room) the update is refused with `NoRoom`
//! and the image stays as it was.
//!
//! The date takes and returns the text `YYYY-MM-DD HH:MM` (UTC), 1996-01-01 00:00 to 9999-12-31
//! 23:59. The standard 3-byte board field (minutes since 1996-01-01 00:00, FRU spec section 11)
//! ends at 2027-11-24 20:15, so the date lives in the VITA 46.11 Alternate Manufacturing
//! Date/Time record (VITA 34h) of the MultiRecord area, a 32-bit minutes count that a write
//! changes in place with both checksums; the standard field is set to 000000h (unspecified).

use crate::date;
use crate::records::{
    area, common_header, multirecords, verify, Area, AreaKind, MultiRecord, ALT_MFG_MINUTES_AT,
    AREA_ALIGN, BOARD_HEADER_LEN, END_OF_FIELDS, MULTIRECORD_HEADER_LEN, PRODUCT_HEADER_LEN, SIZE,
};
use crate::store::FruStore;
use core::fmt;
use ipmc_wire::{zero_checksum, TYPE_ASCII};

/// FruField -- one field of the FRU image the console and the application port read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FruField {
    /// Board area: manufacturer.
    BoardMfr,
    /// Board area: product name.
    BoardProduct,
    /// Board area: serial number.
    BoardSerial,
    /// Board area: part number.
    BoardPart,
    /// Board area: FRU file ID.
    BoardFileId,
    /// Manufacturing date and time (the VITA 34h record).
    MfgDate,
    /// Product area: manufacturer.
    ProductMfr,
    /// Product area: product name.
    ProductName,
    /// Product area: part or model number.
    ProductPart,
    /// Product area: version.
    ProductVersion,
    /// Product area: serial number.
    ProductSerial,
    /// Product area: asset tag.
    AssetTag,
    /// Product area: FRU file ID.
    ProductFileId,
}

impl FruField {
    /// ALL -- every field, in the order FIELDS lists them: the board area, then the product area.
    pub const ALL: [FruField; 13] = [
        FruField::BoardMfr,
        FruField::BoardProduct,
        FruField::BoardSerial,
        FruField::BoardPart,
        FruField::BoardFileId,
        FruField::MfgDate,
        FruField::ProductMfr,
        FruField::ProductName,
        FruField::ProductPart,
        FruField::ProductVersion,
        FruField::ProductSerial,
        FruField::AssetTag,
        FruField::ProductFileId,
    ];

    /// name -- the upper-case name used on the application port and in the console.
    pub const fn name(self) -> &'static str {
        match self {
            FruField::BoardMfr => "BOARD_MFR",
            FruField::BoardProduct => "BOARD_PRODUCT",
            FruField::BoardSerial => "BOARD_SERIAL",
            FruField::BoardPart => "BOARD_PART",
            FruField::BoardFileId => "BOARD_FILE_ID",
            FruField::MfgDate => "MFG_DATE",
            FruField::ProductMfr => "PRODUCT_MFR",
            FruField::ProductName => "PRODUCT_NAME",
            FruField::ProductPart => "PRODUCT_PART",
            FruField::ProductVersion => "PRODUCT_VERSION",
            FruField::ProductSerial => "PRODUCT_SERIAL",
            FruField::AssetTag => "ASSET_TAG",
            FruField::ProductFileId => "PRODUCT_FILE_ID",
        }
    }

    /// max_len -- the longest value in bytes: the room the `IPMC_FRU_*_MAX` define reserves, or
    /// the 16 characters of the date text.
    pub const fn max_len(self) -> usize {
        (match self {
            FruField::BoardMfr => ipmc_defs::IPMC_FRU_BOARD_MFR_MAX,
            FruField::BoardProduct => ipmc_defs::IPMC_FRU_BOARD_PRODUCT_MAX,
            FruField::BoardSerial => ipmc_defs::IPMC_FRU_BOARD_SERIAL_MAX,
            FruField::BoardPart => ipmc_defs::IPMC_FRU_BOARD_PART_MAX,
            FruField::BoardFileId => ipmc_defs::IPMC_FRU_BOARD_FILE_ID_MAX,
            FruField::MfgDate => date::TEXT_LEN as u32,
            FruField::ProductMfr => ipmc_defs::IPMC_FRU_PRODUCT_MFR_MAX,
            FruField::ProductName => ipmc_defs::IPMC_FRU_PRODUCT_NAME_MAX,
            FruField::ProductPart => ipmc_defs::IPMC_FRU_PRODUCT_PART_MAX,
            FruField::ProductVersion => ipmc_defs::IPMC_FRU_PRODUCT_VERSION_MAX,
            FruField::ProductSerial => ipmc_defs::IPMC_FRU_PRODUCT_SERIAL_MAX,
            FruField::AssetTag => ipmc_defs::IPMC_FRU_ASSET_TAG_MAX,
            FruField::ProductFileId => ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID_MAX,
        }) as usize
    }

    /// from_name -- the field called `name`, upper or lower case; None for an unknown name.
    pub fn from_name(name: &str) -> Option<FruField> {
        FruField::ALL
            .into_iter()
            .find(|field| field.name().eq_ignore_ascii_case(name))
    }

    /// location -- the area the field is in and its position in that area's field list (None
    /// for the date, which is not a type/length field).
    const fn location(self) -> (AreaKind, Option<usize>) {
        match self {
            // Board fields: manufacturer 0, product name 1, serial 2, part number 3, file ID 4.
            FruField::BoardMfr => (AreaKind::Board, Some(0)),
            FruField::BoardProduct => (AreaKind::Board, Some(1)),
            FruField::BoardSerial => (AreaKind::Board, Some(2)),
            FruField::BoardPart => (AreaKind::Board, Some(3)),
            FruField::BoardFileId => (AreaKind::Board, Some(4)),
            FruField::MfgDate => (AreaKind::Board, None),
            // Product fields: manufacturer 0, name 1, part 2, version 3, serial 4, asset tag 5,
            // file ID 6.
            FruField::ProductMfr => (AreaKind::Product, Some(0)),
            FruField::ProductName => (AreaKind::Product, Some(1)),
            FruField::ProductPart => (AreaKind::Product, Some(2)),
            FruField::ProductVersion => (AreaKind::Product, Some(3)),
            FruField::ProductSerial => (AreaKind::Product, Some(4)),
            FruField::AssetTag => (AreaKind::Product, Some(5)),
            FruField::ProductFileId => (AreaKind::Product, Some(6)),
        }
    }
}

/// FieldError -- why a read or an update of a field was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// The image has no such area (its common header offset is 00h or past the image).
    AreaMissing,
    /// The area or the field is damaged or absent (checksum, end marker, field list too short),
    /// or the rewritten image would not pass `verify`.
    Corrupt,
    /// The value, or the stored field, is not printable ASCII (20h..=7Eh).
    NotAscii,
    /// A text of one byte: a type/length field holds 0 or at least 2 bytes (FRU spec section 13).
    BadLength,
    /// The value is longer than `FruField::max_len`.
    TooLong,
    /// The rewritten fields do not fit in the area's existing length, or the MultiRecord area
    /// has no Alternate Mfg Date/Time record to write the date into.
    NoRoom,
    /// NVMRO is asserted (VITA 46.11 Rule 8-3). The store cannot see the pin: the firmware
    /// checks it before the update and reports this.
    Nvmro,
    /// The date text is malformed or outside 1996-01-01 00:00 .. 9999-12-31 23:59 (`DateRange`).
    BadDate,
}

impl FieldError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            FieldError::AreaMissing => "AreaMissing",
            FieldError::Corrupt => "Corrupt",
            FieldError::NotAscii => "NotAscii",
            FieldError::BadLength => "BadLength",
            FieldError::TooLong => "TooLong",
            FieldError::NoRoom => "NoRoom",
            FieldError::Nvmro => "Nvmro",
            FieldError::BadDate => "BadDate",
        }
    }
}

/// FieldValue -- a field as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldValue<'a> {
    /// A text field; empty for the "no data" field.
    Text(&'a str),
    /// The manufacturing date: minutes since 1996-01-01 00:00 (at most `date::MINUTES_MAX`).
    Minutes(u32),
    /// The manufacturing date is not set: the VITA 34h record is absent, damaged or holds
    /// 00000000h.
    Unspecified,
}

impl fmt::Display for FieldValue<'_> {
    /// Text prints as it is, the date as `YYYY-MM-DD HH:MM`, an unset date as `unspecified`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldValue::Text(text) => out.write_str(text),
            FieldValue::Minutes(minutes) => {
                out.write_str(core::str::from_utf8(&date::text_of(*minutes)).unwrap_or(""))
            }
            FieldValue::Unspecified => out.write_str("unspecified"),
        }
    }
}

/// parse_date -- `YYYY-MM-DD HH:MM` (UTC) as minutes since 1996-01-01 00:00. Err(BadDate) for a
/// text of another shape, a date that does not exist (month 13, February 30, 2100-02-29), a date
/// before 1996-01-01 00:00 and a date after 9999-12-31 23:59.
pub fn parse_date(text: &str) -> Result<u32, FieldError> {
    date::parse_text(text).ok_or(FieldError::BadDate)
}

// ---- reading ----

/// printable -- the bytes are 20h..=7Eh.
fn printable(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| (0x20..=0x7E).contains(byte))
}

/// find_area -- the area that holds `field`, decoded from `image`, checksum checked.
fn find_area(image: &[u8], kind: AreaKind) -> Result<Area<'_>, FieldError> {
    let head = common_header(image).map_err(|_| FieldError::Corrupt)?;
    let units = match kind {
        AreaKind::Board => head.board,
        AreaKind::Product => head.product,
    };
    if units == 0 {
        return Err(FieldError::AreaMissing);
    }
    let found = area(image, kind, units).map_err(|_| FieldError::Corrupt)?;
    if !found.checksum_ok || !found.has_end_marker() {
        return Err(FieldError::Corrupt);
    }
    Ok(found)
}

/// AltDate -- the Alternate Mfg Date/Time record (VITA 34h) as found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltDate {
    /// The image has no MultiRecord area, or none of its records is the Alternate Mfg Date/Time
    /// record.
    Absent,
    /// The record is there but its header or data checksum is wrong.
    BadChecksum,
    /// The record's minutes since 1996-01-01 00:00; 0 is unspecified.
    Minutes(u32),
}

/// MfgDate -- the manufacturing date as stored in both places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MfgDate {
    /// The standard 3-byte board field: minutes since 1996-01-01 00:00, 0 = unspecified (what
    /// this IPMC writes). A Chassis Manager may have written another value; it is not read for
    /// the date.
    pub standard: u32,
    /// The VITA 34h record.
    pub alt: AltDate,
}

impl MfgDate {
    /// effective -- the date: the minutes of a sound record that are set and in range; None
    /// (unspecified) otherwise.
    pub fn effective(&self) -> Option<u32> {
        match self.alt {
            AltDate::Minutes(minutes) if minutes != 0 && minutes <= date::MINUTES_MAX => {
                Some(minutes)
            }
            _ => None,
        }
    }
}

/// alt_record -- the first Alternate Mfg Date/Time record of the MultiRecord area of `image`
/// (shape only: the checksums are the caller's to read), with its minutes.
fn alt_record(image: &[u8]) -> Result<Option<(MultiRecord<'_>, u32)>, FieldError> {
    let head = common_header(image).map_err(|_| FieldError::Corrupt)?;
    Ok(multirecords(image, head.multirecord)
        .find_map(|record| record.alt_mfg_minutes().map(|minutes| (record, minutes))))
}

/// mfg_date -- both stored forms of the manufacturing date in `image`. Err(AreaMissing) without
/// a board area, Err(Corrupt) for a damaged one. A missing or damaged record is `AltDate`, not
/// an error.
pub fn mfg_date(image: &[u8]) -> Result<MfgDate, FieldError> {
    let standard = find_area(image, AreaKind::Board)?
        .mfg_minutes()
        .ok_or(FieldError::Corrupt)?;
    let alt = match alt_record(image)? {
        None => AltDate::Absent,
        Some((record, _)) if !(record.header_ok && record.data_ok) => AltDate::BadChecksum,
        Some((_, minutes)) => AltDate::Minutes(minutes),
    };
    Ok(MfgDate { standard, alt })
}

/// unit_field -- the value of `field` in `image`. Err(AreaMissing) without its area,
/// Err(Corrupt) for a damaged area or a field list too short, Err(NotAscii) for a text field
/// that is not printable 8-bit ASCII. The date reads from the VITA 34h record; without a sound
/// one that is set it is `Unspecified` (`MfgDate::effective`).
pub fn unit_field(image: &[u8], field: FruField) -> Result<FieldValue<'_>, FieldError> {
    let (kind, index) = field.location();
    let Some(index) = index else {
        return Ok(mfg_date(image)?
            .effective()
            .map_or(FieldValue::Unspecified, FieldValue::Minutes));
    };
    let found = find_area(image, kind)?;
    let stored = found.fields().nth(index).ok_or(FieldError::Corrupt)?;
    if !stored.is_ascii() || !printable(stored.data) {
        return Err(FieldError::NotAscii);
    }
    core::str::from_utf8(stored.data)
        .map(FieldValue::Text)
        .map_err(|_| FieldError::NotAscii)
}

/// write_unit_field -- writes the value of `field` in `image` to `out` as text: the text of a
/// text field (nothing for an empty one), `YYYY-MM-DD HH:MM` for the date. The one function the
/// application port and the console use to print a field. Err as `unit_field`; a full `out` is
/// `TooLong`.
pub fn write_unit_field(
    image: &[u8],
    field: FruField,
    out: &mut dyn fmt::Write,
) -> Result<(), FieldError> {
    write!(out, "{}", unit_field(image, field)?).map_err(|_| FieldError::TooLong)
}

// ---- writing ----

/// AreaWriter -- the rewritten area, filled front to back; a byte past `limit` is `NoRoom`.
struct AreaWriter {
    bytes: [u8; SIZE],
    pos: usize,
    /// First byte the fields may not reach: the area's checksum byte.
    limit: usize,
}

impl AreaWriter {
    fn push(&mut self, byte: u8) -> Result<(), FieldError> {
        if self.pos >= self.limit {
            return Err(FieldError::NoRoom);
        }
        *self.bytes.get_mut(self.pos).ok_or(FieldError::NoRoom)? = byte;
        self.pos += 1;
        Ok(())
    }

    fn push_all(&mut self, bytes: &[u8]) -> Result<(), FieldError> {
        bytes.iter().try_for_each(|byte| self.push(*byte))
    }
}

/// check_text -- `value` is a legal text for `field`: printable ASCII, 0 or 2.. bytes, at most
/// `max_len`.
fn check_text(field: FruField, value: &str) -> Result<(), FieldError> {
    if !printable(value.as_bytes()) {
        return Err(FieldError::NotAscii);
    }
    if value.len() > field.max_len() {
        return Err(FieldError::TooLong);
    }
    if value.len() == 1 {
        return Err(FieldError::BadLength);
    }
    Ok(())
}

/// rewrite_area -- the bytes of `old` with the text field at list position `index` set to
/// `value`: same length, every other field copied byte for byte, then C1h, 00h up to the
/// checksum byte and the zero checksum.
fn rewrite_area(old: &Area<'_>, index: usize, value: &str) -> Result<[u8; SIZE], FieldError> {
    let header_len = match old.kind {
        AreaKind::Board => BOARD_HEADER_LEN,
        AreaKind::Product => PRODUCT_HEADER_LEN,
    };
    let mut out = AreaWriter {
        bytes: [0; SIZE],
        pos: 0,
        limit: old.bytes.len() - 1,
    };
    out.push_all(old.bytes.get(..header_len).ok_or(FieldError::Corrupt)?)?;
    if index >= old.fields().count() {
        return Err(FieldError::Corrupt);
    }
    for (position, stored) in old.fields().enumerate() {
        if index == position {
            out.push(TYPE_ASCII | value.len() as u8)?;
            out.push_all(value.as_bytes())?;
        } else {
            out.push((stored.type_code << 6) | stored.data.len() as u8)?;
            out.push_all(stored.data)?;
        }
    }
    out.push(END_OF_FIELDS)?;
    while out.pos < out.limit {
        out.push(0)?;
    }
    let body = out.bytes.get(..out.limit).ok_or(FieldError::Corrupt)?;
    let sum = zero_checksum(body);
    *out.bytes.get_mut(out.limit).ok_or(FieldError::Corrupt)? = sum;
    Ok(out.bytes)
}

impl FruStore {
    /// set_unit_field -- sets `field` to `value` (text, or `YYYY-MM-DD HH:MM` for the date,
    /// which changes the minutes of the VITA 34h record in place, rewrites both of its checksums
    /// and sets the standard board field to 000000h). The store changes only when the whole
    /// update succeeds, then it is marked dirty so the flash log saves it, as it does after
    /// Write FRU Data. The caller checks NVMRO first (VITA 46.11 Rule 8-3).
    ///
    /// Errors: `NotAscii`, `TooLong` and `BadLength` for the text, `BadDate` for the date,
    /// `AreaMissing` and `Corrupt` for an image that has no such field, `NoRoom` when the new
    /// fields do not fit the area's length or the image has no VITA 34h record. A refused
    /// update leaves the image unchanged.
    pub fn set_unit_field(&mut self, field: FruField, value: &str) -> Result<(), FieldError> {
        let (kind, index) = field.location();
        let image = *self.image();
        let next = match index {
            None => with_mfg_date(&image, parse_date(value)?)?,
            Some(index) => {
                check_text(field, value)?;
                with_text(&image, kind, index, value)?
            }
        };
        verify(&next).map_err(|_| FieldError::Corrupt)?;
        self.replace_image(next);
        Ok(())
    }
}

/// area_start -- the offset of the board or product area of `image`.
fn area_start(image: &[u8], kind: AreaKind) -> Result<usize, FieldError> {
    let head = common_header(image).map_err(|_| FieldError::Corrupt)?;
    let units = match kind {
        AreaKind::Board => head.board,
        AreaKind::Product => head.product,
    };
    Ok(usize::from(units) * AREA_ALIGN)
}

/// with_text -- `image` with the text field at list position `index` of the `kind` area set.
fn with_text(
    image: &[u8; SIZE],
    kind: AreaKind,
    index: usize,
    value: &str,
) -> Result<[u8; SIZE], FieldError> {
    let old = find_area(image, kind)?;
    let rewritten = rewrite_area(&old, index, value)?;
    let start = area_start(image, kind)?;
    let area_len = old.bytes.len();
    let mut next = *image;
    next.get_mut(start..start + area_len)
        .ok_or(FieldError::Corrupt)?
        .copy_from_slice(rewritten.get(..area_len).ok_or(FieldError::Corrupt)?);
    Ok(next)
}

/// with_mfg_date -- `image` with the minutes of the VITA 34h record set to `minutes`, the data
/// checksum and the header checksum rewritten, and the standard board field set to 000000h
/// (the board area checksum rewritten). Err(AreaMissing) without a MultiRecord area,
/// Err(NoRoom) when it has no such record; nothing moves.
fn with_mfg_date(image: &[u8; SIZE], minutes: u32) -> Result<[u8; SIZE], FieldError> {
    let board = find_area(image, AreaKind::Board)?;
    let head = common_header(image).map_err(|_| FieldError::Corrupt)?;
    if head.multirecord == 0 {
        return Err(FieldError::AreaMissing);
    }
    let (record, _) = alt_record(image)?.ok_or(FieldError::NoRoom)?;
    let mut next = *image;
    // The record: the minutes, then the data checksum (header byte 3) and the header checksum
    // (header byte 4) over the new bytes.
    let data_at = record.offset + MULTIRECORD_HEADER_LEN;
    let minutes_at = data_at + ALT_MFG_MINUTES_AT;
    next.get_mut(minutes_at..minutes_at + 4)
        .ok_or(FieldError::Corrupt)?
        .copy_from_slice(&minutes.to_le_bytes());
    let data = next
        .get(data_at..data_at + record.data.len())
        .ok_or(FieldError::Corrupt)?;
    let data_sum = zero_checksum(data);
    *next.get_mut(record.offset + 3).ok_or(FieldError::Corrupt)? = data_sum;
    let header = next
        .get(record.offset..record.offset + 4)
        .ok_or(FieldError::Corrupt)?;
    let header_sum = zero_checksum(header);
    *next.get_mut(record.offset + 4).ok_or(FieldError::Corrupt)? = header_sum;
    // The standard board field, 000000h, and the board checksum over it.
    let start = area_start(image, AreaKind::Board)?;
    let end = start + board.bytes.len();
    next.get_mut(start + PRODUCT_HEADER_LEN..start + BOARD_HEADER_LEN)
        .ok_or(FieldError::Corrupt)?
        .fill(0);
    let area_sum = zero_checksum(next.get(start..end - 1).ok_or(FieldError::Corrupt)?);
    *next.get_mut(end - 1).ok_or(FieldError::Corrupt)? = area_sum;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{COMMON_HEADER_LEN, IMAGE_LEN};

    const MAC: &str = "00:11:22:33:44:55";

    /// area_range -- offset and length of the board or product area of `image`.
    fn area_range(image: &[u8], kind: AreaKind) -> (usize, usize) {
        let head = common_header(image).unwrap();
        let units = match kind {
            AreaKind::Board => head.board,
            AreaKind::Product => head.product,
        };
        let found = area(image, kind, units).unwrap();
        (usize::from(units) * AREA_ALIGN, found.bytes.len())
    }

    /// text -- the text of `field` in `store`.
    fn text(store: &FruStore, field: FruField) -> String {
        format!("{}", unit_field(store.bytes(), field).unwrap())
    }

    /// with_header_checksum -- the image with its common header checksum rewritten.
    fn with_header_checksum(mut image: [u8; SIZE]) -> [u8; SIZE] {
        image[COMMON_HEADER_LEN - 1] = zero_checksum(&image[..COMMON_HEADER_LEN - 1]);
        image
    }

    /// default_fields -- the five text fields of the built board area.
    fn default_fields() -> Vec<Vec<u8>> {
        [
            ipmc_defs::IPMC_FRU_BOARD_MFR,
            ipmc_defs::IPMC_FRU_BOARD_PRODUCT,
            ipmc_defs::IPMC_FRU_BOARD_SERIAL,
            ipmc_defs::IPMC_FRU_BOARD_PART,
            ipmc_defs::IPMC_FRU_FILE_ID,
        ]
        .iter()
        .map(|text| text.as_bytes().to_vec())
        .collect()
    }

    /// board_with -- the built image with the board area rewritten from `fields` (type/length
    /// 8-bit ASCII), `standard` as the 3-byte date and `units` 8-byte units long. The other
    /// areas stay where they were, so `units` is at most the built length.
    fn board_with(fields: &[Vec<u8>], standard: [u8; 3], units: usize) -> [u8; SIZE] {
        let mut image = *FruStore::new().image();
        let (at, built_len) = area_range(&image, AreaKind::Board);
        let len = units * AREA_ALIGN;
        assert!(len <= built_len);
        let mut body = vec![1, units as u8, 0];
        body.extend(standard);
        for field in fields {
            body.push(TYPE_ASCII | field.len() as u8);
            body.extend(field);
        }
        body.push(END_OF_FIELDS);
        assert!(body.len() < len, "the fields fill the area");
        body.resize(len - 1, 0);
        body.push(zero_checksum(&body));
        image[at..at + built_len].fill(0);
        image[at..at + len].copy_from_slice(&body);
        image
    }

    /// without_reserve -- the board area cut back to its fields: 74 bytes up to C1h, the area
    /// 10 units (5 bytes of slack after C1h).
    fn without_reserve() -> [u8; SIZE] {
        board_with(&default_fields(), [0; 3], 10)
    }

    /// without_record -- the built image with the MultiRecord area removed (header byte 5 = 0).
    fn without_record() -> [u8; SIZE] {
        let mut image = *FruStore::new().image();
        image[5] = 0;
        with_header_checksum(image)
    }

    /// record_at -- offset of the VITA 34h record of the built image.
    fn record_at() -> usize {
        usize::from(common_header(FruStore::new().bytes()).unwrap().multirecord) * AREA_ALIGN
    }

    #[test]
    fn each_field_is_set_read_back_and_leaves_the_rest_alone() {
        // (field, value to set).
        let rows: [(FruField, &str); 17] = [
            (FruField::BoardMfr, "ACME-MFR"),
            (FruField::BoardProduct, "Board product name"),
            (FruField::BoardSerial, MAC),
            (FruField::BoardPart, "PN-0001-REV-B"),
            (FruField::BoardFileId, "file-id-2"),
            (FruField::MfgDate, "2026-10-25 08:30"),
            (FruField::ProductMfr, "Other maker"),
            (FruField::ProductName, "Product name"),
            (FruField::ProductPart, "MODEL-X"),
            (FruField::ProductVersion, "1.25"),
            (FruField::ProductSerial, MAC),
            (FruField::AssetTag, "ASSET 12345 (rack 7)"),
            (FruField::AssetTag, ""),
            (FruField::ProductFileId, "pf-1"),
            (FruField::BoardSerial, "AB"),
            (FruField::BoardPart, "123456789012345678901234"),
            (FruField::MfgDate, "9999-12-31 23:59"),
        ];
        for (field, value) in rows {
            let mut store = FruStore::new();
            let before = *store.image();
            store.set_unit_field(field, value).unwrap();
            assert!(store.is_dirty(), "{} marks the store dirty", field.name());
            assert_eq!(text(&store, field), value, "{}", field.name());
            assert_eq!(verify(store.bytes()), Ok(()), "{}", field.name());
            // The image keeps its size and layout: the area offsets are the same.
            assert_eq!(common_header(store.bytes()), common_header(&before));
            for kind in [AreaKind::Board, AreaKind::Product] {
                assert_eq!(area_range(store.image(), kind), area_range(&before, kind));
            }
            // Every other field reads as before.
            for other in FruField::ALL.into_iter().filter(|other| *other != field) {
                assert_eq!(
                    unit_field(store.bytes(), other),
                    unit_field(&before, other),
                    "{} after {}",
                    other.name(),
                    field.name()
                );
            }
        }
    }

    #[test]
    fn a_text_field_edit_changes_only_its_area() {
        let before = *FruStore::new().image();
        let mut store = FruStore::new();
        store.set_unit_field(FruField::AssetTag, "tag-77").unwrap();
        let (at, len) = area_range(&before, AreaKind::Product);
        assert_eq!(zero_checksum(&store.bytes()[at..at + len]), 0, "checksum");
        assert_eq!(&store.bytes()[..at], &before[..at]);
        assert_eq!(&store.bytes()[at + len..], &before[at + len..]);
    }

    #[test]
    fn every_text_field_takes_a_value_of_its_full_reserved_room() {
        for field in FruField::ALL
            .into_iter()
            .filter(|field| *field != FruField::MfgDate)
        {
            let mut store = FruStore::new();
            let longest = "Y".repeat(field.max_len());
            store.set_unit_field(field, &longest).unwrap();
            assert_eq!(text(&store, field), longest, "{}", field.name());
            assert_eq!(verify(store.bytes()), Ok(()), "{}", field.name());
            let too_long = "Y".repeat(field.max_len() + 1);
            assert_eq!(
                store.set_unit_field(field, &too_long),
                Err(FieldError::TooLong)
            );
        }
        // All of them at once: the pooled room of an area holds every field at its maximum.
        let mut store = FruStore::new();
        for field in FruField::ALL
            .into_iter()
            .filter(|field| *field != FruField::MfgDate)
        {
            store
                .set_unit_field(field, &"Z".repeat(field.max_len()))
                .unwrap();
        }
        assert_eq!(verify(store.bytes()), Ok(()));
        store
            .set_unit_field(FruField::MfgDate, "9999-12-31 23:59")
            .unwrap();
        assert_eq!(verify(store.bytes()), Ok(()));
    }

    #[test]
    fn a_second_edit_with_a_shorter_value_leaves_zero_filled_room() {
        let mut store = FruStore::new();
        store.set_unit_field(FruField::BoardSerial, MAC).unwrap();
        store.set_unit_field(FruField::BoardSerial, "AB").unwrap();
        assert_eq!(text(&store, FruField::BoardSerial), "AB");
        assert_eq!(verify(store.bytes()), Ok(()));
        store.set_unit_field(FruField::BoardSerial, MAC).unwrap();
        assert_eq!(text(&store, FruField::BoardSerial), MAC);
    }

    #[test]
    fn a_custom_field_after_the_file_id_is_kept() {
        let mut fields = default_fields();
        fields.push(b"CUSTOM-1".to_vec());
        let mut store = FruStore::from_image(board_with(&fields, [0; 3], 14));
        assert_eq!(verify(store.bytes()), Ok(()));
        store.set_unit_field(FruField::BoardSerial, MAC).unwrap();
        let head = common_header(store.bytes()).unwrap();
        let found = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        let datas: Vec<&[u8]> = found.fields().map(|f| f.data).collect();
        assert_eq!(datas.len(), 6);
        assert_eq!(datas[2], MAC.as_bytes());
        assert_eq!(datas[5], b"CUSTOM-1");
        assert_eq!(verify(store.bytes()), Ok(()));
    }

    #[test]
    fn refused_values_name_their_error_and_change_nothing() {
        let too_long = "X".repeat(FruField::BoardSerial.max_len() + 1);
        let rows: [(FruField, &str, FieldError); 11] = [
            (FruField::BoardSerial, &too_long, FieldError::TooLong),
            (FruField::AssetTag, &too_long, FieldError::TooLong),
            (FruField::BoardPart, "A", FieldError::BadLength),
            (FruField::ProductPart, "caf\u{e9}", FieldError::NotAscii),
            (FruField::ProductSerial, "a\tb", FieldError::NotAscii),
            (FruField::ProductSerial, "a\u{7f}b", FieldError::NotAscii),
            (FruField::MfgDate, "2026-13-01 00:00", FieldError::BadDate),
            (FruField::MfgDate, "yesterday", FieldError::BadDate),
            (FruField::MfgDate, "2100-02-29 00:00", FieldError::BadDate),
            (FruField::MfgDate, "1995-12-31 23:59", FieldError::BadDate),
            (FruField::MfgDate, "9999-12-31 24:00", FieldError::BadDate),
        ];
        for (field, value, want) in rows {
            let mut store = FruStore::new();
            assert_eq!(store.set_unit_field(field, value), Err(want), "{value:?}");
            assert_eq!(store.image(), FruStore::new().image(), "{value:?}");
            assert!(!store.is_dirty(), "{value:?}");
        }
    }

    #[test]
    fn an_image_without_reserve_gives_no_room_and_stays_as_it_was() {
        let mut store = FruStore::from_image(without_reserve());
        assert_eq!(verify(store.bytes()), Ok(()));
        let before = *store.image();
        // The area has 5 bytes of alignment slack after C1h: 17 bytes for the 12-byte serial
        // fit, 18 do not.
        assert_eq!(
            store.set_unit_field(FruField::BoardSerial, "IK-T1-000001-ABCDEF"),
            Err(FieldError::NoRoom)
        );
        assert_eq!(store.image(), &before);
        assert!(!store.is_dirty());
        store
            .set_unit_field(FruField::BoardSerial, "IK-T1-000001-ABCDE")
            .unwrap_err();
        store
            .set_unit_field(FruField::BoardSerial, "IK-T1-000001-ABCD")
            .unwrap();
        store
            .set_unit_field(FruField::BoardSerial, "IK-T1-000002")
            .unwrap();
        // The date lives in the record, not in the board area: no room is needed there.
        store
            .set_unit_field(FruField::MfgDate, "2026-10-25 08:30")
            .unwrap();
        assert_eq!(verify(store.bytes()), Ok(()));
        assert_eq!(text(&store, FruField::BoardSerial), "IK-T1-000002");
        assert_eq!(text(&store, FruField::MfgDate), "2026-10-25 08:30");
    }

    #[test]
    fn a_missing_or_damaged_area_is_refused() {
        let mut image = *FruStore::new().image();
        image[3] = 0; // no board area
        let mut store = FruStore::from_image(with_header_checksum(image));
        assert_eq!(
            store.set_unit_field(FruField::BoardSerial, MAC),
            Err(FieldError::AreaMissing)
        );
        assert_eq!(
            store.set_unit_field(FruField::MfgDate, "2026-10-25 08:30"),
            Err(FieldError::AreaMissing)
        );
        assert_eq!(
            unit_field(store.bytes(), FruField::BoardPart),
            Err(FieldError::AreaMissing)
        );
        assert_eq!(
            unit_field(store.bytes(), FruField::MfgDate),
            Err(FieldError::AreaMissing)
        );
        // A bad area checksum is damage.
        let mut image = *FruStore::new().image();
        let (at, len) = area_range(&image, AreaKind::Product);
        image[at + len - 1] ^= 1;
        let mut store = FruStore::from_image(image);
        assert_eq!(
            store.set_unit_field(FruField::AssetTag, MAC),
            Err(FieldError::Corrupt)
        );
        assert_eq!(
            unit_field(&[0u8; 4], FruField::AssetTag),
            Err(FieldError::Corrupt)
        );
    }

    #[test]
    fn a_stored_field_that_is_not_printable_ascii_reads_as_not_ascii() {
        let mut image = *FruStore::new().image();
        let (at, len) = area_range(&image, AreaKind::Board);
        // The serial data starts after header 6, manufacturer 11, product name 16 and the
        // serial's own type/length byte: offset 34.
        image[at + 34] = 0x07;
        image[at + len - 1] = zero_checksum(&image[at..at + len - 1]);
        assert_eq!(
            unit_field(&image, FruField::BoardSerial),
            Err(FieldError::NotAscii)
        );
    }

    #[test]
    fn names_parse_in_either_case_and_cover_every_field() {
        assert_eq!(FruField::ALL.len(), 13);
        for (position, field) in FruField::ALL.into_iter().enumerate() {
            assert_eq!(FruField::from_name(field.name()), Some(field));
            assert_eq!(
                FruField::from_name(&field.name().to_ascii_lowercase()),
                Some(field)
            );
            assert!(field.max_len() >= 3);
            // No name twice.
            assert!(FruField::ALL[position + 1..]
                .iter()
                .all(|other| other.name() != field.name()));
        }
        assert_eq!(FruField::from_name("SERIAL"), None);
        assert_eq!(FruField::MfgDate.max_len(), 16);
        assert!(FruField::BoardSerial.max_len() >= 17);
        assert!(FruField::ProductSerial.max_len() >= 17);
        assert_eq!(FieldError::NoRoom.name(), "NoRoom");
    }

    #[test]
    fn the_date_converts_both_ways() {
        // (text, minutes since 1996-01-01 00:00)
        let rows: [(&str, u32); 9] = [
            ("1996-01-01 00:00", 0),
            ("1996-01-01 00:01", 1),
            // 2000-02-29 12:00: 1461 days to 2000-01-01 (1996 is a leap year), 59 more days
            // (31 + 28) to the leap day: 1520 days x 1440 + 720 minutes.
            ("2000-02-29 12:00", 2_189_520),
            // 2026-10-23 00:00: 11253 days x 1440 (the default of ipmi_defs.h).
            ("2026-10-23 00:00", 16_204_320),
            // The last value the standard 3-byte field holds: 11650 days and 1215 minutes.
            ("2027-11-24 20:15", 0x00FF_FFFF),
            ("2027-11-24 20:16", 0x0100_0000),
            // 2100 is not a leap year: 2100-03-01 follows 2100-02-28.
            ("2100-02-28 23:59", 54_784_799),
            ("2100-03-01 00:00", 54_784_800),
            ("9999-12-31 23:59", 4_209_697_439),
        ];
        for (date_text, minutes) in rows {
            assert_eq!(parse_date(date_text), Ok(minutes), "{date_text}");
            assert_eq!(FieldValue::Minutes(minutes).to_string(), date_text);
        }
        assert_eq!(date::MINUTES_MAX, 4_209_697_439);
        assert_eq!(date::STANDARD_MINUTES_MAX, 0x00FF_FFFF);
        for bad in [
            "9999-12-31 24:00",  // hour 24
            "1995-12-31 23:59",  // before the origin
            "2001-02-29 00:00",  // not a leap year
            "2100-02-29 00:00",  // 2100 is not a leap year
            "2000-02-30 00:00",  // no such day
            "2000-04-31 00:00",  // April has 30 days
            "2000-00-10 00:00",  // month 0
            "2000-01-00 00:00",  // day 0
            "2000-01-01 00:60",  // minute 60
            "2000-01-01T00:00",  // wrong separator
            "2000-1-1 00:00",    // short
            "2000-01-01 00:00 ", // long
            "10000-01-01 00:00", // five-digit year
            "20a0-01-01 00:00",  // not a digit
            "",
        ] {
            assert_eq!(parse_date(bad), Err(FieldError::BadDate), "{bad:?}");
        }
        assert_eq!(FieldValue::Unspecified.to_string(), "unspecified");
        assert_eq!(
            date::DateRange.to_string(),
            "use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:00 to 9999-12-31 23:59"
        );
    }

    #[test]
    fn the_built_record_is_the_vita_34h_record_with_the_default_date() {
        let image = *FruStore::new().image();
        let at = record_at();
        // 14 bytes: C0h, 82h, 09h, data checksum, header checksum, ACh 81h 00h, 34h, 00h and the
        // minutes of 2026-10-23 00:00 (16204320 = 00F74220h) LS byte first.
        let data = [0xAC, 0x81, 0x00, 0x34, 0x00, 0x20, 0x42, 0xF7, 0x00];
        let head = [0xC0, 0x82, 0x09, zero_checksum(&data)];
        let mut want = head.to_vec();
        want.push(zero_checksum(&head));
        want.extend(data);
        assert_eq!(&image[at..at + 14], &want[..]);
        assert_eq!(IMAGE_LEN, at + 14);
        // The standard board field is 000000h, and the record reads back.
        let found = mfg_date(&image).unwrap();
        assert_eq!(found.standard, 0);
        assert_eq!(found.alt, AltDate::Minutes(16_204_320));
        assert_eq!(found.effective(), Some(16_204_320));
        assert_eq!(date::DEFAULT_MINUTES, 16_204_320);
    }

    #[test]
    fn a_date_changes_the_record_in_place_with_both_checksums_and_leaves_the_standard_field_zero() {
        for date_text in [
            "1996-01-01 00:01",
            "2026-10-25 08:30",
            "2027-11-24 20:15",
            "2027-11-24 20:16",
            "2100-03-01 00:00",
            "9999-12-31 23:59",
        ] {
            let before = *FruStore::new().image();
            let mut store = FruStore::from_image(before);
            store.set_unit_field(FruField::MfgDate, date_text).unwrap();
            assert_eq!(text(&store, FruField::MfgDate), date_text);
            let minutes = parse_date(date_text).unwrap();
            let found = mfg_date(store.bytes()).unwrap();
            assert_eq!(found.standard, 0, "{date_text}");
            assert_eq!(found.alt, AltDate::Minutes(minutes));
            // Only the minutes bytes and the two checksums of the record changed.
            let at = record_at();
            let changed: Vec<usize> = (0..SIZE)
                .filter(|i| store.bytes()[*i] != before[*i])
                .collect();
            assert!(
                changed.iter().all(|i| (at + 3..at + 14).contains(i)),
                "{date_text}: {changed:?}"
            );
            assert_eq!(&store.bytes()[at + 10..at + 14], &minutes.to_le_bytes());
            let record = multirecords(store.bytes(), (at / AREA_ALIGN) as u8)
                .next()
                .unwrap();
            assert!(record.header_ok && record.data_ok);
            assert_eq!(verify(store.bytes()), Ok(()));
        }
    }

    #[test]
    fn set_writes_zero_over_a_nonzero_standard_field_and_reads_ignore_it() {
        // A Chassis Manager wrote 16204320 minutes (F74220h) into the standard field only.
        let mut image = board_with(&default_fields(), [0x20, 0x42, 0xF7], 17);
        let at = record_at();
        // And the record holds 00000000h (unspecified).
        image[at + 10..at + 14].fill(0);
        image[at + 3] = zero_checksum(&image[at + 5..at + 14]);
        image[at + 4] = zero_checksum(&image[at..at + 4]);
        let mut store = FruStore::from_image(image);
        assert_eq!(verify(store.bytes()), Ok(()));
        assert_eq!(text(&store, FruField::MfgDate), "unspecified");
        let found = mfg_date(store.bytes()).unwrap();
        assert_eq!(
            (found.standard, found.alt),
            (16_204_320, AltDate::Minutes(0))
        );
        assert_eq!(found.effective(), None);
        store
            .set_unit_field(FruField::MfgDate, "2031-05-12 08:00")
            .unwrap();
        let found = mfg_date(store.bytes()).unwrap();
        assert_eq!(found.standard, 0);
        assert_eq!(found.effective(), Some(18_598_080));
    }

    #[test]
    fn a_damaged_or_missing_record_reads_unspecified() {
        // A wrong data checksum: BadChecksum, the date reads unspecified, a SET repairs it.
        let mut image = *FruStore::new().image();
        let at = record_at();
        image[at + 10] ^= 1;
        let mut store = FruStore::from_image(image);
        assert_eq!(mfg_date(store.bytes()).unwrap().alt, AltDate::BadChecksum);
        assert_eq!(text(&store, FruField::MfgDate), "unspecified");
        // verify refuses such an image (the self test fails); a write re-verifies the new one.
        assert!(verify(store.bytes()).is_err());
        store
            .set_unit_field(FruField::MfgDate, "2026-10-25 08:30")
            .unwrap();
        assert_eq!(text(&store, FruField::MfgDate), "2026-10-25 08:30");
        // No MultiRecord area: unspecified, and a SET names AreaMissing.
        let mut store = FruStore::from_image(without_record());
        assert_eq!(mfg_date(store.bytes()).unwrap().alt, AltDate::Absent);
        assert_eq!(text(&store, FruField::MfgDate), "unspecified");
        assert_eq!(
            store.set_unit_field(FruField::MfgDate, "2026-10-25 08:30"),
            Err(FieldError::AreaMissing)
        );
        // A MultiRecord area with another record only: no room for the date.
        let mut image = *FruStore::new().image();
        image[at + 5] = 0x00; // another manufacturer ID, not the VITA record
        image[at + 3] = zero_checksum(&image[at + 5..at + 14]);
        image[at + 4] = zero_checksum(&image[at..at + 4]);
        let mut store = FruStore::from_image(image);
        assert_eq!(mfg_date(store.bytes()).unwrap().alt, AltDate::Absent);
        let before = *store.image();
        assert_eq!(
            store.set_unit_field(FruField::MfgDate, "2026-10-25 08:30"),
            Err(FieldError::NoRoom)
        );
        assert_eq!(store.image(), &before);
    }

    #[test]
    fn the_built_image_exposes_the_defaults() {
        let store = FruStore::new();
        for (field, default) in [
            (FruField::BoardMfr, ipmc_defs::IPMC_FRU_BOARD_MFR),
            (FruField::BoardProduct, ipmc_defs::IPMC_FRU_BOARD_PRODUCT),
            (FruField::BoardSerial, ipmc_defs::IPMC_FRU_BOARD_SERIAL),
            (FruField::BoardPart, ipmc_defs::IPMC_FRU_BOARD_PART),
            (FruField::BoardFileId, ipmc_defs::IPMC_FRU_FILE_ID),
            (FruField::MfgDate, ipmc_defs::IPMC_FRU_MFG_DATE),
            (FruField::ProductMfr, ipmc_defs::IPMC_FRU_PRODUCT_MFR),
            (FruField::ProductName, ipmc_defs::IPMC_FRU_PRODUCT_NAME),
            (FruField::ProductPart, ipmc_defs::IPMC_FRU_PRODUCT_PART),
            (
                FruField::ProductVersion,
                ipmc_defs::IPMC_FRU_PRODUCT_VERSION,
            ),
            (FruField::ProductSerial, ipmc_defs::IPMC_FRU_PRODUCT_SERIAL),
            (FruField::AssetTag, ipmc_defs::IPMC_FRU_ASSET_TAG),
            (FruField::ProductFileId, ipmc_defs::IPMC_FRU_PRODUCT_FILE_ID),
        ] {
            assert_eq!(text(&store, field), default, "{}", field.name());
        }
        assert_eq!(mfg_date(store.bytes()).unwrap().standard, 0);
    }
}
