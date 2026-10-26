//! line.rs - the line editor (bytes in, request lines and echo out) and the answer buffer.
//!
//! The UART delivers single bytes; the protocol works on whole lines. `LineEditor` is what a
//! terminal program expects of a device: it echoes what a person types, erases with backspace,
//! and hands over a line when CR, LF or CR LF ends it. `AnswerBuf` is the fixed buffer the echo,
//! the answer and the prompt are formatted into before they go to the UART.

use core::fmt;

/// Fed -- what one byte did to a `LineEditor`.
#[derive(Debug, PartialEq, Eq)]
pub enum Fed<'a> {
    /// The byte was stored, erased or ignored: nothing to answer yet.
    Nothing,
    /// An empty line ended: the answer is the prompt alone.
    Empty,
    /// A line ended. The text excludes the CR and LF; it stays valid until the next `push`.
    Line(&'a str),
    /// A line ended that was longer than the editor holds; its bytes are dropped.
    TooLong,
    /// A line ended that contained a byte other than printable ASCII (20h..=7Eh) or a tab; its
    /// bytes are dropped.
    BadChar,
    /// The port lost bytes (`LineEditor::damage`) and the line that was being collected, or the
    /// next one to end, cannot be trusted: its end arrived. The count is the bytes lost since the
    /// previous line ended; the line's bytes are dropped and it must not run.
    Damaged(u32),
}

/// Echo -- what the device sends back for one received byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    /// Nothing: the byte was ignored, dropped (line too long, bad byte) or a second line end.
    None,
    /// The printable byte (or tab) itself.
    Byte(u8),
    /// Backspace over one character: BS, SP, BS.
    Erase,
    /// A line end: CR LF.
    Newline,
}

impl Echo {
    /// write_to -- writes the echo bytes to `out`.
    pub fn write_to(self, out: &mut dyn fmt::Write) -> fmt::Result {
        match self {
            Echo::None => Ok(()),
            Echo::Byte(byte) => out.write_char(char::from(byte)),
            Echo::Erase => out.write_str("\u{8} \u{8}"),
            Echo::Newline => out.write_str("\r\n"),
        }
    }
}

/// Edit -- what `LineEditor::push` returns for one byte.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit<'a> {
    /// The bytes to send back.
    pub echo: Echo,
    /// The line state: `Fed::Nothing` until a line ends.
    pub fed: Fed<'a>,
}

/// LineEditor -- collects bytes into lines of at most `N` bytes.
///
/// A printable byte (20h..=7Eh) or a tab is stored and echoed. Backspace (08h or 7Fh) erases the
/// last stored byte and echoes BS SP BS; with nothing stored it does nothing. A line ends at CR
/// or LF; a CR LF or LF CR pair is one line end (the second byte is swallowed), so no empty
/// second line follows, while two bytes alike (CR CR) are two line ends. A line longer than `N`
/// is reported once, when its end arrives, and its bytes are dropped (the editor never keeps a
/// partial line and never clamps); the bytes past `N` are not echoed and backspace does nothing
/// on such a line. A byte outside printable ASCII is not echoed and makes the line `BadChar`.
pub struct LineEditor<const N: usize> {
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
    /// The CR or LF that ended the previous line, 0 after any other byte: a different line end
    /// right behind it is the second half of a CR LF pair.
    last_end: u8,
}

impl<const N: usize> LineEditor<N> {
    /// new -- an empty editor.
    pub const fn new() -> Self {
        LineEditor {
            buf: [0; N],
            len: 0,
            ready: false,
            overflow: false,
            bad: false,
            lost: 0,
            last_end: 0,
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
        self.last_end = 0;
    }

    /// push -- takes one received byte and returns its echo and the line state. Call `push` for
    /// every byte, in order.
    pub fn push(&mut self, byte: u8) -> Edit<'_> {
        if self.ready {
            self.len = 0;
            self.ready = false;
        }
        let (echo, ended) = self.take(byte);
        let fed = if ended {
            self.end_of_line()
        } else {
            Fed::Nothing
        };
        Edit { echo, fed }
    }

    /// take -- the part of `push` that stores or erases a byte: the echo, and whether a line
    /// ended (the caller then runs `end_of_line`).
    fn take(&mut self, byte: u8) -> (Echo, bool) {
        if byte == b'\r' || byte == b'\n' {
            let second_half = self.last_end != 0 && self.last_end != byte;
            self.last_end = if second_half { 0 } else { byte };
            return if second_half {
                (Echo::None, false)
            } else {
                (Echo::Newline, true)
            };
        }
        self.last_end = 0;
        if byte == 0x08 || byte == 0x7F {
            if self.overflow || self.len == 0 {
                return (Echo::None, false);
            }
            self.len -= 1;
            return (Echo::Erase, false);
        }
        if !(byte == b'\t' || (0x20..=0x7E).contains(&byte)) {
            self.bad = true;
            return (Echo::None, false);
        }
        match self.buf.get_mut(self.len) {
            Some(slot) if !self.overflow => {
                *slot = byte;
                self.len += 1;
                (Echo::Byte(byte), false)
            }
            _ => {
                self.overflow = true;
                (Echo::None, false)
            }
        }
    }

    /// end_of_line -- the line state at a line end.
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
        // A line of blanks only is an empty line: the prompt alone.
        let typed = self.buf.get(..self.len).unwrap_or(&[]);
        if typed.iter().all(|byte| *byte == b' ' || *byte == b'\t') {
            self.len = 0;
            return Fed::Empty;
        }
        self.ready = true;
        match self.buf.get(..self.len).map(core::str::from_utf8) {
            Some(Ok(line)) => Fed::Line(line),
            _ => Fed::BadChar,
        }
    }
}

impl<const N: usize> Default for LineEditor<N> {
    fn default() -> Self {
        LineEditor::new()
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

    /// as_bytes -- the bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// len -- bytes written so far.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// truncate -- drops everything written after the first `len` bytes and clears the overflow
    /// flag (a handler that fails takes back the data lines it wrote). A `len` past the end does
    /// nothing.
    pub fn truncate(&mut self, len: usize) {
        if len <= self.len {
            self.len = len;
            self.overflowed = false;
        }
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
