//! ipmc-log - the capture log and the logging logic of the DEBUG menu.
//!
//! One crate for everything a person looks for when a capture or a DEBUG LOG, DUMP, CLEAR or
//! TRACE question comes up: the record ring, the CSV and trace printers, the paced dump, the
//! trace tee and the log switches. No allocator, no hardware; it builds for the host and for
//! thumbv7em-none-eabihf. It depends on ipmc-core (names and the byte ring); the core never
//! depends on it.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// dump -- the paced DEBUG DUMP: cursor, formats, header line, job and sink.
pub mod dump;
/// print -- the CSV and Wireshark-style printers of a record.
pub mod print;
/// record -- the record layout, the ring and the capture log.
pub mod record;
/// switches -- the log state the DEBUG menu switches and its status lines.
pub mod switches;
/// tee -- the trace sink that records every line in the log.
pub mod tee;

pub use dump::{
    DumpCursor, DumpFormat, DumpHeader, DumpJob, DumpSink, Step, FRAME_ROW_MAX, ROW_MAX,
    TEXT_ROW_MAX,
};
pub use print::{CsvHeader, CsvRow, TraceRow};
pub use record::{
    CaptureLog, DefaultLog, Dir, FrameStatus, Kind, Record, Stats, HEADER_LEN, LOG_SIZE,
    PAYLOAD_MAX, TEXT_MAX,
};
pub use switches::{LogLine, LogState, SwitchLines, TraceWhat};
pub use tee::CaptureTrace;

/// word_in -- the value of the table row whose word equals `word`, in any case. The one lookup
/// behind the typed words of DEBUG DUMP (format) and DEBUG TRACE (what).
pub(crate) fn word_in<T: Copy>(word: &str, table: &[(&str, T)]) -> Option<T> {
    table
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(word))
        .map(|(_, value)| *value)
}
