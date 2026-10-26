//! unit.rs - the per-unit FRU fields: the values a person writes into each unit at its first
//! boot (serial numbers, part numbers, asset tag, manufacturing date) and the editor that
//! rewrites one field in place.
//!
//! `records.rs` builds each area with room for these fields (the `IPMC_FRU_*_MAX` defines of
//! ipmi_defs.h): after the C1h end-of-fields byte the area holds 00h bytes up to its length. An
//! update rewrites only its own area. It decodes the fields, writes them back in the same order
//! with the one field replaced, then C1h, 00h up to the area's old length and the zero checksum.
//! The area length never changes, so no other area moves. When the new fields do not fit (an
//! image a Chassis Manager wrote may have no spare room) the update is refused with `NoRoom`
//! and the image stays as it was.
//!
//! The date takes and returns the text `YYYY-MM-DD HH:MM` (UTC); the image holds minutes since
//! 1996-01-01 00:00 in 3 bytes, least significant byte first (FRU spec section 11).

use crate::records::{
    area, common_header, verify, Area, AreaKind, AREA_ALIGN, BOARD_HEADER_LEN, END_OF_FIELDS,
    MFG_DATE_LEN, PRODUCT_HEADER_LEN, SIZE,
};
use crate::store::FruStore;
use core::fmt;
use ipmc_wire::{zero_checksum, TYPE_ASCII};

/// UnitField -- one per-unit field of the FRU image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitField {
    /// Board area: serial number.
    BoardSerial,
    /// Board area: part number.
    BoardPart,
    /// Board area: manufacturing date and time (3 bytes, minutes since 1996-01-01).
    MfgDate,
    /// Product area: part or model number.
    ProductPart,
    /// Product area: serial number.
    ProductSerial,
    /// Product area: asset tag.
    AssetTag,
}

impl UnitField {
    /// ALL -- every field, in the order the application port lists them.
    pub const ALL: [UnitField; 6] = [
        UnitField::BoardSerial,
        UnitField::BoardPart,
        UnitField::MfgDate,
        UnitField::ProductPart,
        UnitField::ProductSerial,
        UnitField::AssetTag,
    ];

    /// name -- the upper-case name used on the application port and in the console.
    pub const fn name(self) -> &'static str {
        match self {
            UnitField::BoardSerial => "BOARD_SERIAL",
            UnitField::BoardPart => "BOARD_PART",
            UnitField::MfgDate => "MFG_DATE",
            UnitField::ProductPart => "PRODUCT_PART",
            UnitField::ProductSerial => "PRODUCT_SERIAL",
            UnitField::AssetTag => "ASSET_TAG",
        }
    }

    /// max_len -- the longest value in bytes: the room the `IPMC_FRU_*_MAX` define reserves, or
    /// the 16 characters of the date text.
    pub const fn max_len(self) -> usize {
        match self {
            UnitField::BoardSerial => ipmc_defs::IPMC_FRU_BOARD_SERIAL_MAX as usize,
            UnitField::BoardPart => ipmc_defs::IPMC_FRU_BOARD_PART_MAX as usize,
            UnitField::MfgDate => DATE_TEXT_LEN,
            UnitField::ProductPart => ipmc_defs::IPMC_FRU_PRODUCT_PART_MAX as usize,
            UnitField::ProductSerial => ipmc_defs::IPMC_FRU_PRODUCT_SERIAL_MAX as usize,
            UnitField::AssetTag => ipmc_defs::IPMC_FRU_ASSET_TAG_MAX as usize,
        }
    }

    /// from_name -- the field called `name`, upper or lower case; None for an unknown name.
    pub fn from_name(name: &str) -> Option<UnitField> {
        UnitField::ALL
            .into_iter()
            .find(|field| field.name().eq_ignore_ascii_case(name))
    }

    /// location -- the area the field is in and its position in that area's field list (None
    /// for the date, which is not a type/length field).
    const fn location(self) -> (AreaKind, Option<usize>) {
        match self {
            // Board fields: manufacturer 0, product name 1, serial 2, part number 3, file ID 4.
            UnitField::BoardSerial => (AreaKind::Board, Some(2)),
            UnitField::BoardPart => (AreaKind::Board, Some(3)),
            UnitField::MfgDate => (AreaKind::Board, None),
            // Product fields: manufacturer 0, name 1, part 2, version 3, serial 4, asset tag 5.
            UnitField::ProductPart => (AreaKind::Product, Some(2)),
            UnitField::ProductSerial => (AreaKind::Product, Some(4)),
            UnitField::AssetTag => (AreaKind::Product, Some(5)),
        }
    }
}

/// FieldError -- why a read or an update of a per-unit field was refused.
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
    /// The value is longer than `UnitField::max_len`.
    TooLong,
    /// The rewritten fields do not fit in the area's existing length.
    NoRoom,
    /// NVMRO is asserted (VITA 46.11 Rule 8-3). The store cannot see the pin: the firmware
    /// checks it before the update and reports this.
    Nvmro,
    /// The date text is malformed or outside 1996-01-01 00:00 .. the 3-byte limit.
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

/// FieldValue -- a per-unit field as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldValue<'a> {
    /// A text field; empty for the "no data" field.
    Text(&'a str),
    /// The manufacturing date: minutes since 1996-01-01 00:00.
    Minutes(u32),
}

impl fmt::Display for FieldValue<'_> {
    /// Text prints as it is, the date as `YYYY-MM-DD HH:MM`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldValue::Text(text) => out.write_str(text),
            FieldValue::Minutes(minutes) => {
                let (year, month, day) = civil_from_days(i64::from(minutes / MINUTES_PER_DAY));
                let rest = minutes % MINUTES_PER_DAY;
                write!(
                    out,
                    "{year:04}-{month:02}-{day:02} {:02}:{:02}",
                    rest / 60,
                    rest % 60
                )
            }
        }
    }
}

// ---- date conversion ----

/// DATE_TEXT_LEN -- `YYYY-MM-DD HH:MM`.
const DATE_TEXT_LEN: usize = 16;
const MINUTES_PER_DAY: u32 = 24 * 60;
/// MINUTES_MAX -- the largest value 3 bytes hold.
const MINUTES_MAX: u32 = 0x00FF_FFFF;
/// EPOCH_DAYS -- days from 1970-01-01 to 1996-01-01, the FRU date origin (26 years, 6 leap days).
const EPOCH_DAYS: i64 = 9_496;

/// days_from_civil -- days since 1970-01-01 of a proleptic Gregorian date (month 1..=12).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12; // March = 0
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// civil_from_days -- the inverse of `days_from_civil`: (year, month, day) of the day that is
/// `days` days after 1996-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + EPOCH_DAYS + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// parse_date -- `YYYY-MM-DD HH:MM` (UTC) as minutes since 1996-01-01 00:00. Err(BadDate) for a
/// text of another shape, a date that does not exist (month 13, February 30), a date before
/// 1996-01-01 and a date past the largest 3-byte value.
pub fn parse_date(text: &str) -> Result<u32, FieldError> {
    let bytes = text.as_bytes();
    if bytes.len() != DATE_TEXT_LEN {
        return Err(FieldError::BadDate);
    }
    // Separators at 4, 7, 10 and 13; digits everywhere else.
    let digits = |from: usize, to: usize| -> Result<i64, FieldError> {
        let part = bytes.get(from..to).ok_or(FieldError::BadDate)?;
        part.iter().try_fold(0i64, |value, byte| {
            if byte.is_ascii_digit() {
                Ok(value * 10 + i64::from(byte - b'0'))
            } else {
                Err(FieldError::BadDate)
            }
        })
    };
    for (at, separator) in [(4, b'-'), (7, b'-'), (10, b' '), (13, b':')] {
        if bytes.get(at) != Some(&separator) {
            return Err(FieldError::BadDate);
        }
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute) = (digits(11, 13)?, digits(14, 16)?);
    if !(1..=12).contains(&month) || day < 1 || hour > 23 || minute > 59 {
        return Err(FieldError::BadDate);
    }
    let days = days_from_civil(year, month, day) - EPOCH_DAYS;
    // A day past the end of the month rolls into the next one: the round trip tells.
    if days < 0 || civil_from_days(days) != (year, month, day) {
        return Err(FieldError::BadDate);
    }
    let minutes = days * i64::from(MINUTES_PER_DAY) + hour * 60 + minute;
    u32::try_from(minutes)
        .ok()
        .filter(|value| *value <= MINUTES_MAX)
        .ok_or(FieldError::BadDate)
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

/// unit_field -- the value of `field` in `image`. Err(AreaMissing) without its area,
/// Err(Corrupt) for a damaged area or a field list too short, Err(NotAscii) for a text field
/// that is not printable 8-bit ASCII.
pub fn unit_field(image: &[u8], field: UnitField) -> Result<FieldValue<'_>, FieldError> {
    let (kind, index) = field.location();
    let found = find_area(image, kind)?;
    let Some(index) = index else {
        return found
            .mfg_minutes()
            .map(FieldValue::Minutes)
            .ok_or(FieldError::Corrupt);
    };
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
/// application port hosts use to print a field. Err as `unit_field`; a full `out` is `TooLong`.
pub fn write_unit_field(
    image: &[u8],
    field: UnitField,
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
fn check_text(field: UnitField, value: &str) -> Result<(), FieldError> {
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

/// rewrite_area -- the bytes of `old` with `field` set to `value`: same length, every other
/// field copied byte for byte, then C1h, 00h up to the checksum byte and the zero checksum.
/// `index` is the field's position in the field list, None for the date in the area header.
fn rewrite_area(
    old: &Area<'_>,
    field: UnitField,
    index: Option<usize>,
    value: &str,
) -> Result<[u8; SIZE], FieldError> {
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
    match index {
        None => {
            // The date: three bytes in the area header, after version, length and language.
            let minutes = parse_date(value)?.to_le_bytes();
            let date = out
                .bytes
                .get_mut(PRODUCT_HEADER_LEN..BOARD_HEADER_LEN)
                .ok_or(FieldError::Corrupt)?;
            date.copy_from_slice(minutes.get(..MFG_DATE_LEN).ok_or(FieldError::BadDate)?);
        }
        Some(_) => check_text(field, value)?,
    }
    if index.is_some_and(|target| target >= old.fields().count()) {
        return Err(FieldError::Corrupt);
    }
    for (position, stored) in old.fields().enumerate() {
        if index == Some(position) {
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
    /// set_unit_field -- sets `field` to `value` (text, or `YYYY-MM-DD HH:MM` for the date).
    /// The store changes only when the whole update succeeds, then it is marked dirty so the
    /// flash log saves it, as it does after Write FRU Data. The caller checks NVMRO first
    /// (VITA 46.11 Rule 8-3).
    ///
    /// Errors: `NotAscii`, `TooLong` and `BadLength` for the text, `BadDate` for the date,
    /// `AreaMissing` and `Corrupt` for an image that has no such field, `NoRoom` when the new
    /// fields do not fit the area's length. A refused update leaves the image unchanged.
    pub fn set_unit_field(&mut self, field: UnitField, value: &str) -> Result<(), FieldError> {
        let (kind, index) = field.location();
        let image = *self.image();
        let old = find_area(&image, kind)?;
        let rewritten = rewrite_area(&old, field, index, value)?;
        // The area starts where the common header says; find_area proved the header decodes.
        let head = common_header(&image).map_err(|_| FieldError::Corrupt)?;
        let units = match kind {
            AreaKind::Board => head.board,
            AreaKind::Product => head.product,
        };
        let start = usize::from(units) * AREA_ALIGN;
        let area_len = old.bytes.len();
        let mut next = image;
        next.get_mut(start..start + area_len)
            .ok_or(FieldError::Corrupt)?
            .copy_from_slice(rewritten.get(..area_len).ok_or(FieldError::Corrupt)?);
        verify(&next).map_err(|_| FieldError::Corrupt)?;
        self.replace_image(next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::COMMON_HEADER_LEN;

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
    fn text(store: &FruStore, field: UnitField) -> String {
        format!("{}", unit_field(store.bytes(), field).unwrap())
    }

    /// with_header_checksum -- the image with its common header checksum rewritten.
    fn with_header_checksum(mut image: [u8; SIZE]) -> [u8; SIZE] {
        image[COMMON_HEADER_LEN - 1] = zero_checksum(&image[..COMMON_HEADER_LEN - 1]);
        image
    }

    /// without_reserve -- the built image with both areas cut back to their fields (the layout
    /// of an image written by a Chassis Manager that keeps no spare room): the board area holds
    /// 74 field bytes + 1 checksum rounded up to 80, the product area 67 + 1 rounded up to 72.
    fn without_reserve() -> [u8; SIZE] {
        let built = *FruStore::new().image();
        let (board_at, _) = area_range(&built, AreaKind::Board);
        let (product_at, _) = area_range(&built, AreaKind::Product);
        let mut image = [0u8; SIZE];
        image[..COMMON_HEADER_LEN].copy_from_slice(&built[..COMMON_HEADER_LEN]);
        image[board_at..board_at + 74].copy_from_slice(&built[board_at..board_at + 74]);
        image[board_at + 1] = 10;
        image[board_at + 79] = zero_checksum(&image[board_at..board_at + 79]);
        let product_new = board_at + 80;
        image[product_new..product_new + 67].copy_from_slice(&built[product_at..product_at + 67]);
        image[product_new + 1] = 9;
        image[product_new + 71] = zero_checksum(&image[product_new..product_new + 71]);
        image[4] = (product_new / AREA_ALIGN) as u8;
        with_header_checksum(image)
    }

    /// with_custom_field -- the built image with the 8-byte custom field "CUSTOM-1" after the
    /// board's FRU File ID (a custom field follows the standard ones, FRU spec Table 11-1).
    fn with_custom_field() -> [u8; SIZE] {
        let mut image = *FruStore::new().image();
        let (at, len) = area_range(&image, AreaKind::Board);
        let custom = [&[TYPE_ASCII | 8][..], b"CUSTOM-1"].concat();
        // The standard fields end at the C1h byte, offset 73 in the area.
        image[at + 73..at + 73 + custom.len()].copy_from_slice(&custom);
        image[at + 73 + custom.len()] = END_OF_FIELDS;
        image[at + len - 1] = zero_checksum(&image[at..at + len - 1]);
        image
    }

    #[test]
    fn each_field_is_set_read_back_and_leaves_the_rest_alone() {
        // (field, value to set). The first two rows set both serials to a MAC address text.
        let rows: [(UnitField, &str); 9] = [
            (UnitField::BoardSerial, MAC),
            (UnitField::ProductSerial, MAC),
            (UnitField::BoardPart, "PN-0001-REV-B"),
            (UnitField::ProductPart, "MODEL-X"),
            (UnitField::AssetTag, "ASSET 12345 (rack 7)"),
            (UnitField::AssetTag, ""),
            (UnitField::BoardSerial, "AB"),
            (UnitField::BoardPart, "123456789012345678901234"),
            (UnitField::MfgDate, "2026-10-25 08:30"),
        ];
        for (field, value) in rows {
            let mut store = FruStore::new();
            let before = *store.image();
            store.set_unit_field(field, value).unwrap();
            assert!(store.is_dirty(), "{} marks the store dirty", field.name());
            assert_eq!(text(&store, field), value, "{}", field.name());
            assert_eq!(verify(store.bytes()), Ok(()), "{}", field.name());
            let (kind, _) = field.location();
            let (at, len) = area_range(&before, kind);
            assert_eq!(area_range(store.image(), kind), (at, len), "area length");
            assert_eq!(zero_checksum(&store.bytes()[at..at + len]), 0, "checksum");
            // Everything outside the edited area is byte for byte the same.
            assert_eq!(&store.bytes()[..at], &before[..at]);
            assert_eq!(&store.bytes()[at + len..], &before[at + len..]);
            // Every other field reads as before.
            for other in UnitField::ALL.into_iter().filter(|other| *other != field) {
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
    fn a_second_edit_with_a_shorter_value_leaves_zero_filled_room() {
        let mut store = FruStore::new();
        store.set_unit_field(UnitField::BoardSerial, MAC).unwrap();
        store.set_unit_field(UnitField::BoardSerial, "AB").unwrap();
        assert_eq!(text(&store, UnitField::BoardSerial), "AB");
        assert_eq!(verify(store.bytes()), Ok(()));
        store.set_unit_field(UnitField::BoardSerial, MAC).unwrap();
        assert_eq!(text(&store, UnitField::BoardSerial), MAC);
    }

    #[test]
    fn a_custom_field_after_the_file_id_is_kept() {
        let mut store = FruStore::from_image(with_custom_field());
        assert_eq!(verify(store.bytes()), Ok(()));
        store.set_unit_field(UnitField::BoardSerial, MAC).unwrap();
        let (at, len) = area_range(store.image(), AreaKind::Board);
        let head = common_header(store.bytes()).unwrap();
        let found = area(store.bytes(), AreaKind::Board, head.board).unwrap();
        let fields: Vec<&[u8]> = found.fields().map(|f| f.data).collect();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[2], MAC.as_bytes());
        assert_eq!(fields[4], b"ipmc-fru-v1");
        assert_eq!(fields[5], b"CUSTOM-1");
        assert_eq!(zero_checksum(&store.bytes()[at..at + len]), 0);
        assert_eq!(verify(store.bytes()), Ok(()));
    }

    #[test]
    fn refused_values_name_their_error_and_change_nothing() {
        let too_long = "X".repeat(UnitField::BoardSerial.max_len() + 1);
        let rows: [(UnitField, &str, FieldError); 8] = [
            (UnitField::BoardSerial, &too_long, FieldError::TooLong),
            (UnitField::AssetTag, &too_long, FieldError::TooLong),
            (UnitField::BoardPart, "A", FieldError::BadLength),
            (UnitField::ProductPart, "caf\u{e9}", FieldError::NotAscii),
            (UnitField::ProductSerial, "a\tb", FieldError::NotAscii),
            (UnitField::ProductSerial, "a\u{7f}b", FieldError::NotAscii),
            (UnitField::MfgDate, "2026-13-01 00:00", FieldError::BadDate),
            (UnitField::MfgDate, "yesterday", FieldError::BadDate),
        ];
        for (field, value, want) in rows {
            let mut store = FruStore::new();
            assert_eq!(store.set_unit_field(field, value), Err(want), "{value:?}");
            assert_eq!(store.image(), FruStore::new().image(), "{value:?}");
            assert!(!store.is_dirty(), "{value:?}");
        }
        // The longest value for the room is accepted.
        let mut store = FruStore::new();
        let longest = "Y".repeat(UnitField::BoardSerial.max_len());
        assert_eq!(
            store.set_unit_field(UnitField::BoardSerial, &longest),
            Ok(())
        );
    }

    #[test]
    fn an_image_without_reserve_gives_no_room_and_stays_as_it_was() {
        let mut store = FruStore::from_image(without_reserve());
        assert_eq!(verify(store.bytes()), Ok(()));
        let before = *store.image();
        // The area has 5 bytes of alignment slack after C1h: 17 bytes for the 12-byte field
        // fit, 18 do not.
        assert_eq!(
            store.set_unit_field(UnitField::BoardSerial, "IK-T1-000001-ABCDE"),
            Err(FieldError::NoRoom)
        );
        assert_eq!(store.image(), &before);
        assert!(!store.is_dirty());
        // The slack takes 17 bytes, the same length fits, and so does the date (3 fixed bytes).
        store
            .set_unit_field(UnitField::BoardSerial, "IK-T1-000001-ABCD")
            .unwrap();
        store
            .set_unit_field(UnitField::BoardSerial, "IK-T1-000002")
            .unwrap();
        store
            .set_unit_field(UnitField::MfgDate, "2026-10-25 08:30")
            .unwrap();
        assert_eq!(verify(store.bytes()), Ok(()));
        assert_eq!(text(&store, UnitField::BoardSerial), "IK-T1-000002");
    }

    #[test]
    fn a_missing_or_damaged_area_is_refused() {
        let mut image = *FruStore::new().image();
        image[3] = 0; // no board area
        let mut store = FruStore::from_image(with_header_checksum(image));
        assert_eq!(
            store.set_unit_field(UnitField::BoardSerial, MAC),
            Err(FieldError::AreaMissing)
        );
        assert_eq!(
            unit_field(store.bytes(), UnitField::BoardPart),
            Err(FieldError::AreaMissing)
        );
        // A bad area checksum is damage.
        let mut image = *FruStore::new().image();
        let (at, len) = area_range(&image, AreaKind::Product);
        image[at + len - 1] ^= 1;
        let mut store = FruStore::from_image(image);
        assert_eq!(
            store.set_unit_field(UnitField::AssetTag, MAC),
            Err(FieldError::Corrupt)
        );
        assert_eq!(
            unit_field(&[0u8; 4], UnitField::AssetTag),
            Err(FieldError::Corrupt)
        );
    }

    #[test]
    fn a_stored_field_that_is_not_printable_ascii_reads_as_not_ascii() {
        let mut image = *FruStore::new().image();
        let (at, len) = area_range(&image, AreaKind::Board);
        // The serial data starts after header 6, manufacturer 11, product 16 and the serial's
        // own type/length byte: offset 34.
        image[at + 34] = 0x07;
        image[at + len - 1] = zero_checksum(&image[at..at + len - 1]);
        assert_eq!(
            unit_field(&image, UnitField::BoardSerial),
            Err(FieldError::NotAscii)
        );
    }

    #[test]
    fn names_parse_in_either_case_and_cover_every_field() {
        for field in UnitField::ALL {
            assert_eq!(UnitField::from_name(field.name()), Some(field));
            assert_eq!(
                UnitField::from_name(&field.name().to_ascii_lowercase()),
                Some(field)
            );
            assert!(field.max_len() >= 3);
        }
        assert_eq!(UnitField::from_name("SERIAL"), None);
        assert_eq!(UnitField::MfgDate.max_len(), 16);
        assert!(UnitField::BoardSerial.max_len() >= 17);
        assert!(UnitField::ProductSerial.max_len() >= 17);
        assert_eq!(FieldError::NoRoom.name(), "NoRoom");
    }

    #[test]
    fn the_date_converts_both_ways() {
        // (text, minutes since 1996-01-01 00:00)
        let rows: [(&str, u32); 5] = [
            ("1996-01-01 00:00", 0),
            ("1996-01-01 00:01", 1),
            // 2000-02-29 12:00: 1461 days to 2000-01-01 (1996 is a leap year), 59 more days
            // (31 + 28) to the leap day: 1520 days x 1440 + 720 minutes.
            ("2000-02-29 12:00", 2_189_520),
            // 2026-10-23 00:00: 11253 days x 1440 (the default of ipmi_defs.h).
            ("2026-10-23 00:00", 16_204_320),
            // The largest 3-byte value: 11650 days and 1215 minutes (20:15).
            ("2027-11-24 20:15", 0x00FF_FFFF),
        ];
        for (date, minutes) in rows {
            assert_eq!(parse_date(date), Ok(minutes), "{date}");
            assert_eq!(FieldValue::Minutes(minutes).to_string(), date);
        }
        for bad in [
            "2027-11-24 20:16",  // past the 3-byte limit
            "1995-12-31 23:59",  // before the origin
            "2001-02-29 00:00",  // not a leap year
            "2000-02-30 00:00",  // no such day
            "2000-04-31 00:00",  // April has 30 days
            "2000-00-10 00:00",  // month 0
            "2000-01-00 00:00",  // day 0
            "2000-01-01 24:00",  // hour 24
            "2000-01-01 00:60",  // minute 60
            "2000-01-01T00:00",  // wrong separator
            "2000-1-1 00:00",    // short
            "2000-01-01 00:00 ", // long
            "20a0-01-01 00:00",  // not a digit
            "",
        ] {
            assert_eq!(parse_date(bad), Err(FieldError::BadDate), "{bad:?}");
        }
    }

    #[test]
    fn the_built_image_exposes_the_defaults() {
        let store = FruStore::new();
        for (field, default) in [
            (UnitField::BoardSerial, ipmc_defs::IPMC_FRU_BOARD_SERIAL),
            (UnitField::BoardPart, ipmc_defs::IPMC_FRU_BOARD_PART),
            (UnitField::ProductPart, ipmc_defs::IPMC_FRU_PRODUCT_PART),
            (UnitField::ProductSerial, ipmc_defs::IPMC_FRU_PRODUCT_SERIAL),
            (UnitField::AssetTag, ipmc_defs::IPMC_FRU_ASSET_TAG),
            (UnitField::MfgDate, "2026-10-23 00:00"),
        ] {
            assert_eq!(text(&store, field), default, "{}", field.name());
        }
    }
}
