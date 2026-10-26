//! hex - the Intel HEX record parser (design 8.4 checks 1 to 4): form, checksum, type, wrap.

use crate::error::BootError;
use ipmc_wire::{hex_byte, zero_checksum};

/// DATA_MAX -- most data bytes in one record (the count field is one byte).
pub const DATA_MAX: usize = 255;
/// OVERHEAD -- count, two address bytes, type and checksum.
const OVERHEAD: usize = 5;
/// DIGITS_MIN -- the shortest record: OVERHEAD bytes of two digits.
const DIGITS_MIN: usize = OVERHEAD * 2;
/// SEGMENT -- the size of the 16-bit offset space; a data record may not cross it.
const SEGMENT: u32 = 0x1_0000;

const TYPE_DATA: u8 = 0x00;
const TYPE_EOF: u8 = 0x01;
const TYPE_EXTENDED_LINEAR: u8 = 0x04;
const TYPE_START_LINEAR: u8 = 0x05;

/// Record -- one parsed record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record<'a> {
    Data { offset: u16, bytes: &'a [u8] },
    Eof,
    ExtendedLinear(u16),
    StartLinear(u32),
}

/// decode_bytes -- the bytes of the hex digits of `digits` into `buf`; None for a digit that is
/// not hex or more bytes than `buf` holds.
fn decode_bytes<'a>(digits: &str, buf: &'a mut [u8; DATA_MAX + OVERHEAD]) -> Option<&'a [u8]> {
    let pairs = digits.as_bytes().as_chunks::<2>().0;
    for (slot, pair) in buf.iter_mut().zip(pairs) {
        *slot = hex_byte(core::str::from_utf8(pair).ok()?)?;
    }
    buf.get(..pairs.len())
}

/// parse -- one line (no line end) into a record; `buf` holds the decoded bytes. The checks run
/// in the design's order: form, checksum, type and its count, wrap. A record of type 01, 04 or
/// 05 with the wrong count is a form error.
pub fn parse<'a>(
    line: &str,
    buf: &'a mut [u8; DATA_MAX + OVERHEAD],
) -> Result<Record<'a>, BootError> {
    let digits = line.strip_prefix(':').ok_or(BootError::Format)?;
    if digits.len() % 2 != 0
        || digits.len() < DIGITS_MIN
        || digits.len() > 2 * buf.len()
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(BootError::Format);
    }
    let bytes = decode_bytes(digits, buf).ok_or(BootError::Format)?;
    let [count, offset_high, offset_low, kind, rest @ ..] = bytes else {
        return Err(BootError::Format);
    };
    let count = *count as usize;
    if rest.len() != count + 1 {
        return Err(BootError::Format);
    }
    if zero_checksum(bytes) != 0 {
        return Err(BootError::Checksum);
    }
    let offset = u16::from_be_bytes([*offset_high, *offset_low]);
    let data = rest.get(..count).ok_or(BootError::Format)?;
    match (*kind, count) {
        (TYPE_DATA, _) => {
            if offset as u32 + count as u32 > SEGMENT {
                return Err(BootError::Wrap);
            }
            Ok(Record::Data {
                offset,
                bytes: data,
            })
        }
        (TYPE_EOF, 0) => Ok(Record::Eof),
        (TYPE_EXTENDED_LINEAR, 2) => Ok(Record::ExtendedLinear(u16::from_be_bytes([
            data.first().copied().unwrap_or(0),
            data.get(1).copied().unwrap_or(0),
        ]))),
        (TYPE_START_LINEAR, 4) => Ok(Record::StartLinear(u32::from_be_bytes([
            data.first().copied().unwrap_or(0),
            data.get(1).copied().unwrap_or(0),
            data.get(2).copied().unwrap_or(0),
            data.get(3).copied().unwrap_or(0),
        ]))),
        (TYPE_EOF | TYPE_EXTENDED_LINEAR | TYPE_START_LINEAR, _) => Err(BootError::Format),
        _ => Err(BootError::Type),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::string::String;

    /// record_line -- the text of a record with a correct checksum.
    pub(crate) fn record_line(offset: u16, kind: u8, data: &[u8]) -> String {
        let mut bytes = std::vec![data.len() as u8, (offset >> 8) as u8, offset as u8, kind];
        bytes.extend_from_slice(data);
        bytes.push(zero_checksum(&bytes));
        let mut line = String::from(":");
        for b in bytes {
            line.push_str(&std::format!("{b:02X}"));
        }
        line
    }

    fn parse_line(line: &str) -> Result<Record<'_>, BootError> {
        // The buffer lives for the call only; copy the result out for comparison.
        let mut buf = [0u8; DATA_MAX + OVERHEAD];
        match parse(line, &mut buf)? {
            Record::Data { offset, bytes } => Ok(Record::Data {
                offset,
                bytes: std::boxed::Box::leak(bytes.to_vec().into_boxed_slice()),
            }),
            Record::Eof => Ok(Record::Eof),
            Record::ExtendedLinear(v) => Ok(Record::ExtendedLinear(v)),
            Record::StartLinear(v) => Ok(Record::StartLinear(v)),
        }
    }

    #[test]
    fn accepted_records() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let rows: [(String, Record<'_>); 5] = [
            (
                record_line(0x0200, 0x00, &data),
                Record::Data {
                    offset: 0x200,
                    bytes: &data,
                },
            ),
            (record_line(0, 0x01, &[]), Record::Eof),
            (
                record_line(0, 0x04, &[0x08, 0x00]),
                Record::ExtendedLinear(0x0800),
            ),
            (
                record_line(0, 0x05, &[0x08, 0x00, 0x81, 0x41]),
                Record::StartLinear(0x0800_8141),
            ),
            (":00000001FF".into(), Record::Eof),
        ];
        for (line, want) in rows {
            assert_eq!(parse_line(&line), Ok(want), "{line}");
        }
    }

    #[test]
    fn refused_records() {
        let mut bad_sum = record_line(0x10, 0, &[1, 2, 3]);
        bad_sum.replace_range(bad_sum.len() - 2.., "00");
        let max = [0u8; 255];
        let rows: [(String, BootError); 19] = [
            (
                record_line(0x10, 0, &[1]).replacen(':', ";", 1),
                BootError::Format,
            ),
            (record_line(0x10, 0, &[1])[..10].into(), BootError::Format),
            (String::from(":0000000"), BootError::Format),
            (String::from(":"), BootError::Format),
            (String::new(), BootError::Format),
            (
                record_line(0x10, 0, &[1]).replacen('0', "G", 1),
                BootError::Format,
            ),
            (String::from(":0100000001"), BootError::Format),
            (String::from(":0200000001FD"), BootError::Format),
            (record_line(0, 0x01, &[1]), BootError::Format),
            (record_line(0, 0x04, &[1, 2, 3]), BootError::Format),
            (record_line(0, 0x05, &[1, 2]), BootError::Format),
            (bad_sum, BootError::Checksum),
            (record_line(0, 0x02, &[0x10, 0x00]), BootError::Type),
            (record_line(0, 0x03, &[0, 0, 0, 0]), BootError::Type),
            (record_line(0, 0x06, &[]), BootError::Type),
            (record_line(0xFFFF, 0, &[1, 2]), BootError::Wrap),
            (record_line(0xFFF0, 0, &[0; 17]), BootError::Wrap),
            (record_line(0xFF80, 0, &max), BootError::Wrap),
            (String::from(":") + &"00".repeat(300), BootError::Format),
        ];
        for (line, want) in rows {
            assert_eq!(parse_line(&line), Err(want), "{line}");
        }
    }

    #[test]
    fn data_to_the_end_of_the_segment_is_accepted() {
        let data = [7u8; 16];
        let line = record_line(0xFFF0, 0, &data);
        assert_eq!(
            parse_line(&line),
            Ok(Record::Data {
                offset: 0xFFF0,
                bytes: &data
            })
        );
        let max = [1u8; 255];
        assert!(parse_line(&record_line(0, 0, &max)).is_ok());
    }
}
