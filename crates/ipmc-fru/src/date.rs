//! date.rs - the manufacturing date: minutes since 1996-01-01 00:00 UTC and the calendar text of
//! them.
//!
//! The standard board-area field is 3 bytes of minutes since 1996-01-01 00:00 (FRU spec Table
//! 11-1, 000000h = unspecified). It ends at FFFFFFh minutes, 2027-11-24 20:15, and a Chassis
//! Manager decodes only that field, so it cannot be widened. This IPMC keeps the date in the VITA
//! 46.11 Alternate Manufacturing Date/Time record (VITA 34h, a 32-bit minutes count, MultiRecord
//! area; `records.rs` lays it out, `unit.rs` reads and writes it) and the standard field is
//! 000000h (unspecified) in the built image and after every write; a nonzero standard field that a
//! Chassis Manager wrote is the date in use (VITA 46.11 section 10.3.10). Minute 0 means
//! unspecified, so the dates the text form accepts run from 1996-01-01 00:01 to 9999-12-31 23:59,
//! with the Gregorian leap-year rule. The conversions below are `const fn` because the built image
//! carries the default date.

use core::fmt;

/// TEXT_LEN -- `YYYY-MM-DD HH:MM`.
pub const TEXT_LEN: usize = 16;
/// MINUTES_PER_DAY -- minutes in a day.
const MINUTES_PER_DAY: i64 = 24 * 60;
/// EPOCH_DAYS -- days from 1970-01-01 to 1996-01-01, the FRU date origin (26 years, 6 leap days).
const EPOCH_DAYS: i64 = 9_496;
/// STANDARD_MINUTES_MAX -- the largest value the standard 3-byte field holds (FFFFFFh minutes,
/// 2027-11-24 20:15): why the date lives in the VITA 34h record.
pub const STANDARD_MINUTES_MAX: u32 = 0x00FF_FFFF;
/// MINUTES_MIN -- the first date the text form accepts: 1996-01-01 00:01. Minute 0 is
/// "unspecified" (FRU spec Table 11-1, VITA 34h record), so no date maps to it.
pub const MINUTES_MIN: u32 = 1;
/// MINUTES_MAX -- the last minute the date accepts: 9999-12-31 23:59.
pub const MINUTES_MAX: u32 = minutes_of(9999, 12, 31, 23, 59);

/// days_from_civil -- days since 1970-01-01 of a proleptic Gregorian date (month 1..=12).
pub const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12; // March = 0
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// civil_from_days -- the inverse of `days_from_civil` for a day count after 1996-01-01:
/// (year, month, day) of the day that is `days` days after 1996-01-01.
pub const fn civil_from_days(days: i64) -> (i64, i64, i64) {
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
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// minutes_of -- minutes since 1996-01-01 00:00 of a calendar date that is on or after it and
/// in range (the caller checks the date exists); 0 for a date before the origin.
const fn minutes_of(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> u32 {
    let days = days_from_civil(year, month, day) - EPOCH_DAYS;
    let minutes = days * MINUTES_PER_DAY + hour * 60 + minute;
    if minutes < 0 || minutes > u32::MAX as i64 {
        0
    } else {
        minutes as u32
    }
}

/// put_digits -- `value` as `width` decimal digits at `buf[at..]`.
const fn put_digits(buf: &mut [u8; TEXT_LEN], at: usize, value: i64, width: usize) {
    let mut rest = value;
    let mut i = width;
    while i > 0 {
        i -= 1;
        buf[at + i] = b'0' + (rest % 10) as u8;
        rest /= 10;
    }
}

/// text_of -- `minutes` as the 16 bytes `YYYY-MM-DD HH:MM`; a value past `MINUTES_MAX` prints as
/// `MINUTES_MAX`. The one formatter of the date: `FieldValue`'s Display uses it.
pub const fn text_of(minutes: u32) -> [u8; TEXT_LEN] {
    let minutes = if minutes > MINUTES_MAX {
        MINUTES_MAX
    } else {
        minutes
    };
    let (year, month, day) = civil_from_days((minutes as i64) / MINUTES_PER_DAY);
    let rest = (minutes as i64) % MINUTES_PER_DAY;
    let mut buf = [b'-'; TEXT_LEN];
    put_digits(&mut buf, 0, year, 4);
    put_digits(&mut buf, 5, month, 2);
    put_digits(&mut buf, 8, day, 2);
    buf[10] = b' ';
    put_digits(&mut buf, 11, rest / 60, 2);
    buf[13] = b':';
    put_digits(&mut buf, 14, rest % 60, 2);
    buf
}

/// parse_parts -- `YYYY-MM-DD<sep>HH:MM` as minutes since 1996-01-01 00:00; None for a text of
/// another shape, a date that does not exist, a date before 1996-01-01 00:01 (minute 0 is
/// "unspecified") or after `MINUTES_MAX`. A `const fn`, so the build checks the default date with the same parser.
const fn parse_parts(bytes: &[u8], separator: u8) -> Option<u32> {
    if bytes.len() != TEXT_LEN {
        return None;
    }
    // Separators at 4, 7, 10 and 13; digits everywhere else.
    let mut i = 0;
    while i < TEXT_LEN {
        let expected = match i {
            4 | 7 => b'-',
            10 => separator,
            13 => b':',
            _ => 0,
        };
        let byte = bytes[i];
        if (expected == 0 && !byte.is_ascii_digit()) || (expected != 0 && byte != expected) {
            return None;
        }
        i += 1;
    }
    let (year, month, day) = (
        digits(bytes, 0, 4),
        digits(bytes, 5, 7),
        digits(bytes, 8, 10),
    );
    let (hour, minute) = (digits(bytes, 11, 13), digits(bytes, 14, 16));
    if month < 1 || month > 12 || day < 1 || hour > 23 || minute > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day) - EPOCH_DAYS;
    // A day past the end of the month rolls into the next one: the round trip tells.
    let (y, m, d) = civil_from_days(days);
    if days < 0 || y != year || m != month || d != day {
        return None;
    }
    let minutes = days * MINUTES_PER_DAY + hour * 60 + minute;
    if minutes < MINUTES_MIN as i64 || minutes > MINUTES_MAX as i64 {
        return None;
    }
    Some(minutes as u32)
}

/// digits -- the decimal value of `bytes[from..to]` (the caller checked they are digits).
const fn digits(bytes: &[u8], from: usize, to: usize) -> i64 {
    let mut value = 0i64;
    let mut i = from;
    while i < to {
        value = value * 10 + (bytes[i] - b'0') as i64;
        i += 1;
    }
    value
}

/// parse_text -- `YYYY-MM-DD HH:MM` (UTC) as minutes since 1996-01-01 00:00.
pub const fn parse_text(text: &str) -> Option<u32> {
    parse_parts(text.as_bytes(), b' ')
}

/// DEFAULT_MINUTES -- the default date of the build (defs.rs IPMC_FRU_MFG_DATE), as minutes.
/// The build fails when the define is not a date this module accepts.
pub const DEFAULT_MINUTES: u32 = match parse_text(ipmc_defs::IPMC_FRU_MFG_DATE) {
    Some(minutes) => minutes,
    None => panic!(
        "IPMC_FRU_MFG_DATE must be YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59 (minute 0 is unspecified)"
    ),
};

/// DateRange -- prints the accepted range, `use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to
/// 9999-12-31 23:59`: the one text of the bad-date answer on every port.
#[derive(Clone, Copy, Debug)]
pub struct DateRange;

impl fmt::Display for DateRange {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, last) = (text_of(MINUTES_MIN), text_of(MINUTES_MAX));
        write!(
            out,
            "use YYYY-MM-DD HH:MM (UTC), {} to {}",
            core::str::from_utf8(&first).unwrap_or(""),
            core::str::from_utf8(&last).unwrap_or("")
        )
    }
}
