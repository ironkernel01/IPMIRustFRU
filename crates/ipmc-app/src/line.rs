//! line.rs - bytes in, request lines out, and the answer buffer.
//!
//! The UART delivers single bytes; the protocol works on whole lines. `LineAssembler` collects
//! bytes until a line ends and hands the line over. `AnswerBuf` is the fixed buffer an answer is
//! formatted into before it goes to the UART.

use core::fmt;

/// Fed -- what one byte did to a `LineAssembler`.
#[derive(Debug, PartialEq, Eq)]
pub enum Fed<'a> {
    /// The byte was stored, or it ended an empty line: nothing to answer yet.
    Nothing,
    /// A line ended. The text excludes the CR and LF; it stays valid until the next `push`.
    Line(&'a str),
    /// A line ended that was longer than the assembler holds; its bytes are dropped.
    TooLong,
    /// A line ended that contained a byte other than printable ASCII (20h..=7Eh) or a tab; its
    /// bytes are dropped.
    BadChar,
    /// The port lost bytes (`LineAssembler::damage`) and the line that was being collected, or the
    /// next one to end, cannot be trusted: its end arrived. The count is the bytes lost since the
    /// previous line ended; the line's bytes are dropped and it must not run.
    Damaged(u32),
}

/// LineAssembler -- collects bytes into lines of at most `N` bytes.
///
/// A line ends at CR or LF. A CR LF pair therefore produces the line at the CR and an empty line
/// at the LF; an empty line is not a request, so it produces nothing. A line longer than `N` is
/// reported once, when its end arrives, and its bytes are dropped (the assembler never keeps a
/// partial line and never clamps).
pub struct LineAssembler<const N: usize> {
    buf: [u8; N],
    len: usize,
    /// A line is waiting to be taken: the next `push` starts a new one.
    ready: bool,
    /// Bytes past `N` arrived in the current line.
    overflow: bool,
    /// A byte that is not printable ASCII arrived in the current line.
    bad: bool,
    /// Bytes the port lost since the previous line ended; above 0 the next line end answers
    /// `Fed::Damaged` (see `damage`).
    lost: u32,
}

impl<const N: usize> LineAssembler<N> {
    /// new -- an empty assembler.
    pub const fn new() -> Self {
        LineAssembler {
            buf: [0; N],
            len: 0,
            ready: false,
            overflow: false,
            bad: false,
            lost: 0,
        }
    }

    /// damage -- the port lost `lost` bytes (above 0) and the firmware discarded what was waiting
    /// in its receive ring. The partial line is dropped, and the next line end (the first CR or
    /// LF `push` takes afterwards) returns `Fed::Damaged` instead of a line, so a request that
    /// lost a byte never runs. Lines that were complete inside the discarded bytes get no
    /// answer: the payload's answer timeout covers them. Lines after the damaged one run
    /// normally. Losses that arrive before that line end add up.
    pub fn damage(&mut self, lost: u32) {
        self.lost = self.lost.saturating_add(lost);
        self.len = 0;
        self.ready = false;
        self.overflow = false;
        self.bad = false;
    }

    /// push -- takes one received byte. Returns `Fed::Line` when the byte ends a request line,
    /// `Fed::TooLong` or `Fed::BadChar` when it ends a line that cannot be a request, and
    /// `Fed::Nothing` otherwise. Call `push` for every byte, in order.
    pub fn push(&mut self, byte: u8) -> Fed<'_> {
        if self.ready {
            self.len = 0;
            self.ready = false;
        }
        if byte == b'\r' || byte == b'\n' {
            return self.end_of_line();
        }
        if byte == b'\t' || (0x20..=0x7E).contains(&byte) {
            match self.buf.get_mut(self.len) {
                Some(slot) => {
                    *slot = byte;
                    self.len += 1;
                }
                None => self.overflow = true,
            }
        } else {
            self.bad = true;
        }
        Fed::Nothing
    }

    /// end_of_line -- the CR or LF case of `push`.
    fn end_of_line(&mut self) -> Fed<'_> {
        let (overflow, bad) = (self.overflow, self.bad);
        self.overflow = false;
        self.bad = false;
        if self.lost > 0 {
            let lost = self.lost;
            self.lost = 0;
            self.len = 0;
            return Fed::Damaged(lost);
        }
        if overflow {
            self.len = 0;
            return Fed::TooLong;
        }
        if bad {
            self.len = 0;
            return Fed::BadChar;
        }
        if self.len == 0 {
            return Fed::Nothing;
        }
        self.ready = true;
        match self.buf.get(..self.len).map(core::str::from_utf8) {
            Some(Ok(line)) => Fed::Line(line),
            _ => Fed::BadChar,
        }
    }
}

impl<const N: usize> Default for LineAssembler<N> {
    fn default() -> Self {
        LineAssembler::new()
    }
}

/// AnswerBuf -- a fixed text buffer of `N` bytes that implements `fmt::Write`. A write that does
/// not fit stores nothing, sets the overflow flag and returns an error, so the buffer never
/// holds half an item.
pub struct AnswerBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
    overflowed: bool,
}

impl<const N: usize> AnswerBuf<N> {
    /// new -- an empty buffer.
    pub const fn new() -> Self {
        AnswerBuf {
            buf: [0; N],
            len: 0,
            overflowed: false,
        }
    }

    /// clear -- empties the buffer and the overflow flag.
    pub fn clear(&mut self) {
        self.len = 0;
        self.overflowed = false;
    }

    /// as_str -- the text written so far (all ASCII).
    pub fn as_str(&self) -> &str {
        self.buf
            .get(..self.len)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }

    /// is_empty -- nothing has been written.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// overflowed -- a write did not fit since the last `clear`.
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl<const N: usize> Default for AnswerBuf<N> {
    fn default() -> Self {
        AnswerBuf::new()
    }
}

impl<const N: usize> fmt::Write for AnswerBuf<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len.saturating_add(text.len());
        match self.buf.get_mut(self.len..end) {
            Some(room) => {
                room.copy_from_slice(text.as_bytes());
                self.len = end;
                Ok(())
            }
            None => {
                self.overflowed = true;
                Err(fmt::Error)
            }
        }
    }
}
