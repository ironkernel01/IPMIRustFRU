// text.rs - the console as text: formatted output over a `SerialPort`, with no process global.
//
// `ConsoleText` is implemented for every `SerialPort`, so a board's console handle prints
// directly and the host tests print into a fake port. The three adapters (`ConsoleTrace`,
// `MenuOut`, `DumpOut`) give the trace hook, the menu engine and the capture dump their sink over
// any `ConsoleText`. Output is complete (`print`, waits only while the TX ring is full) or
// whole-line-or-dropped (`line_or_drop`, `row_or_drop`, never waits).

use core::fmt;

use ipmc_core::names::format_cut;
use ipmc_core::trace::Trace;
use ipmc_hal::serial::SerialPort;
use ipmc_log::DumpSink;
use ipmc_menu::MenuOutput;

/// ROW_TEXT_MAX -- longest I2C trace row, bytes: the columns of a `TraceRow` (up to 104 characters
/// with the longest names) plus IPMB_MSG_MAX three-character hex bytes (96), rounded up.
pub const ROW_TEXT_MAX: usize = 224;
const _: () = assert!(
    ROW_TEXT_MAX + 2 <= ipmc_defs::IPMC_UART_TX_BUF,
    "the console TX ring must hold one whole I2C trace row"
);

/// CRLF -- the end of every console line.
const CRLF: &[u8] = b"\r\n";

/// ConsoleText -- the console as text: complete writes, and lines that are dropped when the ring
/// is full.
pub trait ConsoleText {
    /// print -- formatted output, complete (waits only while the TX ring is full); `\n` becomes
    /// CR LF. A write error aborts the rest of the message.
    fn print(&mut self, args: fmt::Arguments);
    /// line_or_drop -- one trace line of at most IPMC_LOG_TEXT_MAX bytes (the length the capture
    /// log keeps), written whole when the ring has room for it and CR LF, else dropped and
    /// counted; never a partial line and never a wait. A longer line is cut and ends in `...`.
    fn line_or_drop(&mut self, args: fmt::Arguments);
    /// row_or_drop -- the same for an I2C trace row (longer than a log text line, ROW_TEXT_MAX).
    fn row_or_drop(&mut self, args: fmt::Arguments);
    /// tx_free -- bytes the TX ring takes now.
    fn tx_free(&self) -> usize;
}

/// out -- formatted write to a console: `out!(&mut console, "text {}", value)`; a newline
/// becomes CR LF (`ConsoleText::print`).
#[macro_export]
macro_rules! out {
    ($console:expr, $($arg:tt)*) => {
        $crate::text::ConsoleText::print($console, format_args!($($arg)*))
    };
}

/// Writer -- `fmt::Write` into the TX ring of a port; `\n` becomes CR LF.
struct Writer<'a, S: SerialPort>(&'a mut S);

impl<S: SerialPort> fmt::Write for Writer<'_, S> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for (i, part) in s.split('\n').enumerate() {
            // A failed write aborts the rest of the message: one timeout per message, not per
            // byte.
            if i > 0 {
                self.0.write_all_wait(CRLF).map_err(|_| fmt::Error)?;
            }
            self.0
                .write_all_wait(part.as_bytes())
                .map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

/// print_cut_or_drop -- one line of at most N bytes plus CR LF, written whole when the TX ring
/// has room for all of it, else dropped and counted (`count_line_dropped`).
fn print_cut_or_drop<S: SerialPort, const N: usize>(port: &mut S, args: fmt::Arguments) {
    let line = format_cut::<N>(args);
    let text = line.bytes();
    // Only the main loop writes a port and the interrupt only drains the ring, so the room found
    // here is still there for the two writes.
    if SerialPort::tx_free(port) >= text.len() + CRLF.len() {
        port.write(text);
        port.write(CRLF);
    } else {
        port.count_line_dropped();
    }
}

impl<S: SerialPort> ConsoleText for S {
    fn print(&mut self, args: fmt::Arguments) {
        // A write error means bytes were dropped; the timeout is already counted and the console
        // has no other channel to report it on.
        let _ = fmt::Write::write_fmt(&mut Writer(self), args);
    }

    fn line_or_drop(&mut self, args: fmt::Arguments) {
        print_cut_or_drop::<S, { ipmc_defs::IPMC_LOG_TEXT_MAX }>(self, args);
    }

    fn row_or_drop(&mut self, args: fmt::Arguments) {
        print_cut_or_drop::<S, ROW_TEXT_MAX>(self, args);
    }

    fn tx_free(&self) -> usize {
        SerialPort::tx_free(self)
    }
}

/// ConsoleTrace -- console sink of the trace hook, shared by the sensors and the IPMB link: one
/// line per call, written whole when the ring has room and dropped (and counted) otherwise, so a
/// trace line never holds the main loop.
pub struct ConsoleTrace<'a, C: ConsoleText>(pub &'a mut C);

impl<C: ConsoleText> Trace for ConsoleTrace<'_, C> {
    fn line(&mut self, args: fmt::Arguments) {
        self.0.line_or_drop(args);
    }
}

/// MenuOut -- menu output on the console; `\n` becomes CR LF inside `print`.
pub struct MenuOut<'a, C: ConsoleText>(pub &'a mut C);

impl<C: ConsoleText> MenuOutput for MenuOut<'_, C> {
    fn write_str(&mut self, s: &str) {
        self.0.print(format_args!("{}", s));
    }
}

/// DumpOut -- the console TX ring as the sink of a capture dump.
pub struct DumpOut<'a, C: ConsoleText>(pub &'a mut C);

impl<C: ConsoleText> DumpSink for DumpOut<'_, C> {
    fn free(&self) -> usize {
        self.0.tx_free()
    }

    fn line(&mut self, args: fmt::Arguments) {
        self.0.print(args);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::sim::SimSerial;

    #[test]
    fn print_maps_newline_to_crlf() {
        let mut port = SimSerial::new(64);
        port.print(format_args!("a\nb {}\n", 7));
        assert_eq!(port.text(), "a\r\nb 7\r\n");
    }

    #[test]
    fn a_ring_without_room_aborts_a_print_and_drops_a_line() {
        // (room, what is printed, the bytes in the ring, the dropped count)
        let print_cases: [(usize, &str, &str, u32); 2] =
            [(3, "abcd\nefg", "abc", 0), (64, "ab\ncd", "ab\r\ncd", 0)];
        for (room, text, sent, dropped) in print_cases {
            let mut port = SimSerial::new(room);
            port.print(format_args!("{}", text));
            assert_eq!((port.text().as_str(), port.dropped()), (sent, dropped));
        }
        // "hello" and CR LF need 7 bytes: exact room writes whole, one byte less drops the line.
        let line_cases: [(usize, &str, u32); 2] = [(7, "hello\r\n", 0), (6, "", 1)];
        for (room, sent, dropped) in line_cases {
            let mut port = SimSerial::new(room);
            port.line_or_drop(format_args!("hello"));
            assert_eq!((port.text().as_str(), port.dropped()), (sent, dropped));
        }
    }

    #[test]
    fn a_long_line_is_cut_and_ends_in_three_dots() {
        let mut port = SimSerial::roomy();
        let long = "x".repeat(ipmc_defs::IPMC_LOG_TEXT_MAX + 40);
        port.line_or_drop(format_args!("{}", long));
        let text = port.text();
        assert_eq!(text.len(), ipmc_defs::IPMC_LOG_TEXT_MAX + 2);
        assert!(text.ends_with("...\r\n"));
    }

    #[test]
    fn a_row_keeps_what_a_log_line_cuts() {
        let width = ipmc_defs::IPMC_LOG_TEXT_MAX + 40;
        assert!(width <= ROW_TEXT_MAX);
        let mut port = SimSerial::roomy();
        port.row_or_drop(format_args!("{}", "y".repeat(width)));
        assert_eq!(port.sent_len(), width + 2);
        assert!(!port.text().contains("..."));
    }

    #[test]
    fn the_adapters_reach_the_port() {
        let mut port = SimSerial::new(64);
        ConsoleTrace(&mut port).line(format_args!("t"));
        MenuOut(&mut port).write_str("m\n");
        DumpOut(&mut port).line(format_args!("d\n"));
        assert_eq!(port.text(), "t\r\nm\r\nd\r\n");
        assert_eq!(DumpOut(&mut port).free(), 64 - 9);
    }
}
