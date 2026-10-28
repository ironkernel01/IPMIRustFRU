//! text - numbers as text without `core::fmt`: the bootloader must fit 32 KB, and the number
//! writers here are the only way a number reaches a UART.

use ipmc_hal::serial::SerialPort;

/// TextOut -- where the bootloader's text goes (a UART in the firmware, a String in tests).
pub trait TextOut {
    fn put(&mut self, text: &str);
}

/// SerialText -- a serial port as a `TextOut`: the text goes into the TX ring whole, with no
/// `core::fmt` (the fault handlers and the bootloader's trace use it). A failed write drops the
/// rest of the text; the port has counted the timeout.
pub struct SerialText<S: SerialPort>(pub S);

impl<S: SerialPort> TextOut for SerialText<S> {
    fn put(&mut self, text: &str) {
        let _ = self.0.write_all_wait(text.as_bytes());
    }
}

pub const CRLF: &str = "\r\n";

const DIGITS_UPPER: &str = "0123456789ABCDEF";
const DIGITS_LOWER: &str = "0123456789abcdef";
/// DEC_DIGITS_MAX -- digits in u32::MAX.
const DEC_DIGITS_MAX: usize = 10;
/// HEX_DIGITS_MAX -- hex digits of a u32.
const HEX_DIGITS_MAX: u32 = 8;

/// put_digit -- the digit `value` (0..=15) of `table`.
fn put_digit(out: &mut dyn TextOut, table: &str, value: u32) {
    let at = value as usize;
    out.put(table.get(at..at + 1).unwrap_or("?"));
}

/// put_dec -- `value` in decimal.
pub fn put_dec(out: &mut dyn TextOut, value: u32) {
    let mut digits = [0u8; DEC_DIGITS_MAX];
    let mut count = 0;
    let mut rest = value;
    loop {
        if let Some(slot) = digits.get_mut(count) {
            *slot = (rest % 10) as u8;
        }
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for at in (0..count).rev() {
        put_digit(
            out,
            DIGITS_UPPER,
            digits.get(at).copied().unwrap_or(0) as u32,
        );
    }
}

/// put_hex -- `digits` hex digits (1..=8), upper case, leading zeros.
pub fn put_hex(out: &mut dyn TextOut, value: u32, digits: u32) {
    for at in (0..digits.clamp(1, HEX_DIGITS_MAX)).rev() {
        put_digit(out, DIGITS_UPPER, (value >> (4 * at)) & 0xF);
    }
}

/// put_hex_bytes -- each byte as two lower-case hex digits, no separator (digest text).
pub fn put_hex_bytes(out: &mut dyn TextOut, bytes: &[u8]) {
    for &byte in bytes {
        put_digit(out, DIGITS_LOWER, (byte >> 4) as u32);
        put_digit(out, DIGITS_LOWER, (byte & 0xF) as u32);
    }
}

/// put_addr -- a flash or RAM address as `0x0800_C000` (upper case digits, underscore between
/// the halves).
pub fn put_addr(out: &mut dyn TextOut, addr: u32) {
    out.put("0x");
    put_hex(out, addr >> 16, 4);
    out.put("_");
    put_hex(out, addr & 0xFFFF, 4);
}

/// put_version -- a version `x.y.z` from its first three bytes, with a leading `v`.
pub fn put_version(out: &mut dyn TextOut, version: [u8; 4]) {
    out.put("v");
    for (i, part) in version.iter().take(3).enumerate() {
        if i > 0 {
            out.put(".");
        }
        put_dec(out, *part as u32);
    }
}

#[cfg(test)]
impl TextOut for std::string::String {
    fn put(&mut self, text: &str) {
        self.push_str(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipmc_hal::testing::FakePort;
    use std::string::String;

    #[test]
    fn serial_text_writes_whole_and_drops_on_a_full_port() {
        let port = FakePort::new(8);
        let mut out = SerialText(port.clone());
        put_dec(&mut out, 1234);
        out.put("-ok");
        assert_eq!(port.text(), "1234-ok");
        out.put("overflow"); // only the byte that fits is kept; the rest is dropped, no panic
        assert_eq!(port.text(), "1234-oko");
    }

    #[test]
    fn decimal_rows() {
        for (value, text) in [
            (0, "0"),
            (1, "1"),
            (9, "9"),
            (10, "10"),
            (81, "81"),
            (100, "100"),
            (u32::MAX, "4294967295"),
        ] {
            let mut out = String::new();
            put_dec(&mut out, value);
            assert_eq!(out, text);
        }
    }

    #[test]
    fn hex_padding_rows() {
        for (value, digits, text) in [
            (0, 2, "00"),
            (0xA, 1, "A"),
            (0x1F, 4, "001F"),
            (0xBEEF, 2, "EF"),
            (0xDEAD_BEEF, 8, "DEADBEEF"),
            (0x1, 0, "1"),
            (0x1, 20, "00000001"),
        ] {
            let mut out = String::new();
            put_hex(&mut out, value, digits);
            assert_eq!(out, text, "{value:#x} {digits}");
        }
    }

    #[test]
    fn hex_bytes_are_lower_case_pairs() {
        let mut out = String::new();
        put_hex_bytes(&mut out, &[0x00, 0x1F, 0xAB, 0xFF]);
        assert_eq!(out, "001fabff");
        let mut empty = String::new();
        put_hex_bytes(&mut empty, &[]);
        assert_eq!(empty, "");
    }

    #[test]
    fn address_and_version_rows() {
        let mut out = String::new();
        put_addr(&mut out, 0x0800_C200);
        out.push(' ');
        put_version(&mut out, [0, 1, 20, 0]);
        assert_eq!(out, "0x0800_C200 v0.1.20");
    }
}
