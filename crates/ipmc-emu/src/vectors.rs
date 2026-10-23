//! vectors.rs - the one reader of the vector file format (vectors/sbc_ab.vec).
//!
//! One frame per line: `time_s bus hex...`. `time_s` is the capture time in seconds (decimal),
//! `bus` is `A` or `B`, and the hex bytes are one whole IPMB frame starting with the 8-bit
//! destination address. Lines starting with `#` and blank lines are skipped; they still count
//! in the line numbers of errors. No function in this module panics.

use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::queue::Bus;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

/// LINE_MAX_CHARS -- longest accepted line, from ipmi_defs.h IPMC_EMU_VECTOR_LINE_MAX (guarded
/// there to hold a time, a bus tag and IPMB_MSG_MAX three-character hex tokens).
pub const LINE_MAX_CHARS: usize = ipmc_defs::IPMC_EMU_VECTOR_LINE_MAX as usize;
/// FRAME_MAX_BYTES -- longest accepted frame: the ipmc-core frame maximum (IPMB_MSG_MAX). A
/// longer frame cannot be a valid IPMB frame, so the reader rejects it instead of passing it on.
pub const FRAME_MAX_BYTES: usize = IPMB_MAX_LEN;
/// MS_PER_S -- milliseconds in a second (the file holds seconds, the Link takes milliseconds).
const MS_PER_S: f64 = 1000.0;

/// Vector -- one captured frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// Line number in the file (1-based), for messages.
    pub line: usize,
    /// Capture time in milliseconds, rounded from the seconds in the file.
    pub time_ms: u32,
    /// Bus the frame was captured on.
    pub bus: Bus,
    /// The frame: destination address first, checksums included.
    pub bytes: Vec<u8>,
}

/// VectorErrorKind -- what is wrong with a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorErrorKind {
    /// The line is longer than LINE_MAX_CHARS.
    LineTooLong,
    /// The line has no time field.
    MissingTime,
    /// The time is not a finite non-negative number of seconds that fits 32-bit milliseconds.
    BadTime,
    /// The line ends after the time.
    MissingBus,
    /// The bus tag is not `A` or `B`.
    BadBus,
    /// The line has no frame bytes.
    EmptyFrame,
    /// A byte token is not one or two hex digits.
    BadHex,
    /// The frame is longer than FRAME_MAX_BYTES.
    FrameTooLong,
}

impl VectorErrorKind {
    /// name -- text for messages.
    pub const fn name(self) -> &'static str {
        match self {
            VectorErrorKind::LineTooLong => "line too long",
            VectorErrorKind::MissingTime => "missing time",
            VectorErrorKind::BadTime => "bad time",
            VectorErrorKind::MissingBus => "missing bus tag",
            VectorErrorKind::BadBus => "bus tag is not A or B",
            VectorErrorKind::EmptyFrame => "no frame bytes",
            VectorErrorKind::BadHex => "bad hex byte",
            VectorErrorKind::FrameTooLong => "frame too long",
        }
    }
}

/// VectorError -- a rejected line: its number (1-based) and the fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorError {
    /// Line number in the file, 1-based.
    pub line: usize,
    /// The fault.
    pub kind: VectorErrorKind,
}

impl fmt::Display for VectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.kind.name())
    }
}

impl std::error::Error for VectorError {}

/// LoadError -- `load` failed: the file could not be read, or a line is bad.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Io(io::Error),
    /// A line was rejected.
    Vector(VectorError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::Vector(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// load -- reads the vector file at `path`.
/// Err(Io) when the file cannot be read as text, Err(Vector) for the first bad line.
pub fn load(path: &Path) -> Result<Vec<Vector>, LoadError> {
    let text = fs::read_to_string(path).map_err(LoadError::Io)?;
    parse(&text).map_err(LoadError::Vector)
}

/// parse -- reads the text of a vector file: every frame line in order. Err carries the number
/// and fault of the first bad line.
pub fn parse(text: &str) -> Result<Vec<Vector>, VectorError> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fail = |kind| VectorError { line: number, kind };
        if line.len() > LINE_MAX_CHARS {
            return Err(fail(VectorErrorKind::LineTooLong));
        }
        let mut fields = trimmed.split_whitespace();
        let seconds = fields.next().ok_or(fail(VectorErrorKind::MissingTime))?;
        let time_ms = time_ms_of(seconds).ok_or(fail(VectorErrorKind::BadTime))?;
        let tag = fields.next().ok_or(fail(VectorErrorKind::MissingBus))?;
        let bus = match tag {
            "A" => Bus::A,
            "B" => Bus::B,
            _ => return Err(fail(VectorErrorKind::BadBus)),
        };
        let mut bytes = Vec::new();
        for token in fields {
            if bytes.len() >= FRAME_MAX_BYTES {
                return Err(fail(VectorErrorKind::FrameTooLong));
            }
            bytes.push(ipmc_wire::hex_byte(token).ok_or(fail(VectorErrorKind::BadHex))?);
        }
        if bytes.is_empty() {
            return Err(fail(VectorErrorKind::EmptyFrame));
        }
        out.push(Vector {
            line: number,
            time_ms,
            bus,
            bytes,
        });
    }
    Ok(out)
}

/// time_ms_of -- seconds text to milliseconds; None when it is not a finite, non-negative number
/// that fits 32 bits.
fn time_ms_of(text: &str) -> Option<u32> {
    let seconds: f64 = text.parse().ok()?;
    let ms = (seconds * MS_PER_S).round();
    if !ms.is_finite() || ms < 0.0 || ms > f64::from(u32::MAX) {
        return None;
    }
    // Range checked above: the cast cannot truncate.
    Some(ms as u32)
}
