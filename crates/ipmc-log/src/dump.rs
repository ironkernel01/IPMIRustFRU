//! dump.rs - the paced DEBUG DUMP: the cursor that walks the held records while the log keeps
//! recording, the two formats and the job that prints a few rows per main-loop pass into a sink
//! (the console TX ring on the board, a string in the tests).

use core::fmt::{self, Arguments, Display};

use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::names::Names;

use crate::print::{CsvHeader, CsvRow, TraceRow};
use crate::record::{CaptureLog, Stats, PAYLOAD_MAX, TEXT_MAX};

/// first_index -- the index of the oldest held record.
fn first_index<const N: usize>(log: &CaptureLog<N>) -> u32 {
    let stats = log.stats();
    stats.total.wrapping_sub(stats.records)
}

/// Step -- what the dump does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Print the record with this index (`CaptureLog::get`).
    Row(u32),
    /// This many records of the dump were overwritten before they were printed; the dump
    /// continues with the oldest record still held.
    Overwritten(u32),
    /// Every record that was held at the start has been printed (or lost).
    Done,
}

/// DumpCursor -- the position of a dump that runs a few rows per main-loop pass while the log
/// keeps recording. It dumps the records held when it started, no later ones: `start` fixes the
/// end, `next` walks towards it, and a record the ring overwrote meanwhile is skipped once,
/// with a count. Record indices wrap with `total`, so every comparison is a wrapping distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpCursor {
    /// Index of the next record to print.
    next: u32,
    /// Records of the dump still to print or to count as lost.
    remaining: u32,
}

impl DumpCursor {
    /// start -- a dump of the records held now.
    pub fn start<const N: usize>(log: &CaptureLog<N>) -> Self {
        DumpCursor {
            next: first_index(log),
            remaining: log.stats().records,
        }
    }

    /// next -- the next step. A cleared log (its indices restart) ends the dump.
    pub fn next<const N: usize>(&mut self, log: &CaptureLog<N>) -> Step {
        if self.remaining == 0 {
            return Step::Done;
        }
        let first = first_index(log);
        // Signed wrapping distances: positive when the log is ahead of the cursor.
        let behind = first.wrapping_sub(self.next) as i32;
        if behind > 0 {
            let lost = behind.unsigned_abs().min(self.remaining);
            self.next = self.next.wrapping_add(lost);
            self.remaining -= lost;
            return Step::Overwritten(lost);
        }
        if (log.stats().total.wrapping_sub(self.next) as i32) <= 0 {
            self.remaining = 0; // the log was cleared: nothing left to print
            return Step::Done;
        }
        let index = self.next;
        self.next = self.next.wrapping_add(1);
        self.remaining -= 1;
        Step::Row(index)
    }
}

/// DumpFormat -- the two printers of DEBUG DUMP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpFormat {
    /// One CSV line per record, after a column header.
    Csv,
    /// One Wireshark-style text line per record.
    Trace,
}

impl DumpFormat {
    /// parse -- the typed word, any case: CSV or TRACE; None for another word.
    pub fn parse(word: &str) -> Option<DumpFormat> {
        crate::word_in(
            word,
            &[("CSV", DumpFormat::Csv), ("TRACE", DumpFormat::Trace)],
        )
    }

    /// column_header -- the line printed after the `DumpHeader`: the CSV column names; the trace
    /// format has none.
    pub fn column_header(self) -> Option<CsvHeader> {
        match self {
            DumpFormat::Csv => Some(CsvHeader),
            DumpFormat::Trace => None,
        }
    }
}

/// ROW_MAX -- room one dump step needs in the sink before it prints a row, so a row never waits
/// for the wire. The longest row is the larger of a text record and a frame record, each with
/// its CR LF:
///   text:  index (10) + time (10) + `TEXT` and commas (12) + quotes (2) + the text, which a
///          CSV row may double when every character is a quote, so 2 x IPMC_LOG_TEXT_MAX
///          + 2 for CR LF = 2 x TEXT_MAX + 36
///   frame: fixed columns and the summary (at most 160 with the trace row's padded NetFn and
///          command columns) + 3 characters per frame byte (hex and a space)
///          + 2 for CR LF = 3 x IPMB_MAX_LEN + 162
/// A bigger TX ring is a design input: the const assert below names the define.
pub const TEXT_ROW_MAX: usize = 2 * TEXT_MAX + 36;
/// FRAME_ROW_MAX -- the longest frame row (see ROW_MAX).
pub const FRAME_ROW_MAX: usize = 3 * IPMB_MAX_LEN + 162;
/// ROW_MAX -- the longer of the two.
pub const ROW_MAX: usize = if TEXT_ROW_MAX > FRAME_ROW_MAX {
    TEXT_ROW_MAX
} else {
    FRAME_ROW_MAX
};
const _: () = assert!(
    ROW_MAX < ipmc_defs::IPMC_UART_TX_BUF,
    "IPMC_UART_TX_BUF must be larger than the longest dump row (ROW_MAX in ipmc-log dump.rs)"
);

/// DumpHeader -- the first line of a dump (no line ending): who made it and what the log holds.
pub struct DumpHeader<'a> {
    /// Firmware name (defs.rs IPMC_FW_NAME).
    pub firmware: &'a str,
    /// Firmware version.
    pub version: &'a str,
    /// ipmc-core version.
    pub core_version: &'a str,
    /// The IPMB address, 8-bit form.
    pub ipmb_address: u8,
    /// The Tier the build reports.
    pub tier: u32,
    /// Milliseconds since start-up.
    pub uptime_ms: u32,
    /// The ring state.
    pub stats: Stats,
}

impl Display for DumpHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "# IPMC capture: {} v{}, ipmc-core v{}, IPMB address {:02X}h, Tier {}, uptime {} ms, {} records, {} overwritten",
            self.firmware,
            self.version,
            self.core_version,
            self.ipmb_address,
            self.tier,
            self.uptime_ms,
            self.stats.records,
            self.stats.overwritten
        )
    }
}

/// DumpSink -- where a dump prints: the free bytes it may fill without waiting, and a line.
pub trait DumpSink {
    /// free -- bytes the sink takes without waiting.
    fn free(&self) -> usize;
    /// line -- prints the formatted text (it carries its own line ending).
    fn line(&mut self, args: Arguments);
}

/// DumpJob -- a DEBUG DUMP in progress. The console holds it and continues it on every
/// `Console::poll` pass (`step`): whole rows while the sink has room for the longest one, so the
/// main loop never waits for the wire. It dumps the records held when it started.
pub struct DumpJob {
    format: DumpFormat,
    cursor: DumpCursor,
}

impl DumpJob {
    /// start -- a dump of the records held now.
    pub fn start<const N: usize>(format: DumpFormat, log: &CaptureLog<N>) -> Self {
        DumpJob {
            format,
            cursor: DumpCursor::start(log),
        }
    }

    /// step -- prints rows while the sink holds the longest row; true when the dump is over
    /// (the end marker is printed).
    pub fn step<const N: usize>(
        &mut self,
        log: &CaptureLog<N>,
        names: &dyn Names,
        sink: &mut dyn DumpSink,
    ) -> bool {
        let mut scratch = [0u8; PAYLOAD_MAX];
        while sink.free() >= ROW_MAX {
            match self.cursor.next(log) {
                Step::Row(index) => {
                    if let Some(record) = log.get(index, &mut scratch) {
                        match self.format {
                            DumpFormat::Csv => {
                                sink.line(format_args!("{}\n", CsvRow(&record, names)));
                            }
                            DumpFormat::Trace => {
                                sink.line(format_args!("{}\n", TraceRow(&record, names)));
                            }
                        }
                    }
                }
                Step::Overwritten(count) => {
                    sink.line(format_args!(
                        "# {} records overwritten during the dump\n",
                        count
                    ));
                }
                Step::Done => {
                    sink.line(format_args!("# end of capture\n"));
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DefaultLog, Dir, FrameStatus};
    use ipmc_core::ext::NoExtension;
    use ipmc_core::names::CoreNames;
    use ipmc_core::queue::Bus;

    type Small = CaptureLog<64>;

    #[test]
    fn a_dump_cursor_walks_the_held_records_and_ignores_later_ones() {
        let mut log = Small::new(true);
        for i in 0..3u32 {
            log.text(i, format_args!("r{i}"));
        }
        let mut cursor = DumpCursor::start(&log);
        assert_eq!(cursor.next(&log), Step::Row(0));
        log.text(9, format_args!("added during the dump"));
        assert_eq!(cursor.next(&log), Step::Row(1));
        assert_eq!(cursor.next(&log), Step::Row(2));
        assert_eq!(cursor.next(&log), Step::Done);
        assert_eq!(cursor.next(&log), Step::Done);
        let mut scratch = [0u8; PAYLOAD_MAX];
        assert_eq!(log.get(1, &mut scratch).map(|r| r.time_ms), Some(1));
        assert!(log.get(7, &mut scratch).is_none());
    }

    #[test]
    fn a_dump_cursor_counts_records_the_ring_overwrote_and_goes_on() {
        let mut log = Small::new(true);
        for i in 0..3u32 {
            log.text(i, format_args!("line {i:05}")); // 16 bytes each, 3 fit
        }
        let mut cursor = DumpCursor::start(&log);
        assert_eq!(cursor.next(&log), Step::Row(0));
        // The ring holds three of these: two more records push record 1 out.
        log.text(3, format_args!("line {:05}", 3));
        log.text(4, format_args!("line {:05}", 4));
        let first = log.stats().total - log.stats().records;
        assert_eq!(first, 2);
        assert_eq!(cursor.next(&log), Step::Overwritten(1));
        assert_eq!(cursor.next(&log), Step::Row(2));
        assert_eq!(
            cursor.next(&log),
            Step::Done,
            "records 3 and 4 came after the start"
        );
    }

    #[test]
    fn a_dump_cursor_whose_records_are_all_gone_reports_them_once_then_ends() {
        let mut log = Small::new(true);
        for i in 0..3u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        let mut cursor = DumpCursor::start(&log);
        for i in 3..9u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        assert_eq!(cursor.next(&log), Step::Overwritten(3));
        assert_eq!(cursor.next(&log), Step::Done);
    }

    #[test]
    fn a_dump_cursor_of_an_empty_or_cleared_log_ends() {
        let mut log = Small::new(true);
        assert_eq!(DumpCursor::start(&log).next(&log), Step::Done);
        log.text(1, format_args!("a"));
        log.text(2, format_args!("b"));
        let mut cursor = DumpCursor::start(&log);
        assert_eq!(cursor.next(&log), Step::Row(0));
        log.clear();
        assert_eq!(cursor.next(&log), Step::Done);
    }

    /// A sink that collects the text and offers `room` bytes.
    struct StringSink {
        room: usize,
        text: String,
    }

    impl DumpSink for StringSink {
        fn free(&self) -> usize {
            self.room
        }

        fn line(&mut self, args: Arguments) {
            self.text.push_str(&std::fmt::format(args));
        }
    }

    #[test]
    fn a_dump_job_prints_the_rows_the_overwrite_note_and_the_end_line() {
        let names = CoreNames(&NoExtension);
        let mut log = DefaultLog::new(true);
        log.text(5, format_args!("one"));
        log.frame(
            6,
            Bus::A,
            Dir::In,
            FrameStatus::Received,
            &[0x82, 0x18, 0x66, 0x20, 0x04, 0x01, 0xDB],
        );
        let mut job = DumpJob::start(DumpFormat::Csv, &log);
        let mut sink = StringSink {
            room: ROW_MAX - 1,
            text: String::new(),
        };
        assert!(
            !job.step(&log, &names, &mut sink),
            "no room: nothing printed"
        );
        assert!(sink.text.is_empty());
        sink.room = ROW_MAX;
        assert!(job.step(&log, &names, &mut sink));
        assert_eq!(
            sink.text,
            "0,5,TEXT,,,,,\"one\",\n\
             1,6,FRAME,A,IN,rx,7,\"20h->82h App Req Get Device ID seq 01h\",82 18 66 20 04 01 DB\n\
             # end of capture\n"
        );
        // A dump of a log that lost its records says so once.
        let mut log = Small::new(true);
        for i in 0..3u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        let mut job = DumpJob::start(DumpFormat::Trace, &log);
        for i in 3..9u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        let mut sink = StringSink {
            room: ROW_MAX,
            text: String::new(),
        };
        assert!(job.step(&log, &names, &mut sink));
        assert_eq!(
            sink.text,
            "# 3 records overwritten during the dump\n# end of capture\n"
        );
    }

    #[test]
    fn the_format_word_and_the_header_line() {
        for (word, want) in [
            ("csv", Some(DumpFormat::Csv)),
            ("Trace", Some(DumpFormat::Trace)),
            ("json", None),
        ] {
            assert_eq!(DumpFormat::parse(word), want);
        }
        assert!(DumpFormat::Csv.column_header().is_some());
        assert!(DumpFormat::Trace.column_header().is_none());
        let log = Small::new(true);
        let header = DumpHeader {
            firmware: "fw",
            version: "1.2.3",
            core_version: "0.1.0",
            ipmb_address: 0x82,
            tier: 1,
            uptime_ms: 99,
            stats: log.stats(),
        };
        assert_eq!(
            header.to_string(),
            "# IPMC capture: fw v1.2.3, ipmc-core v0.1.0, IPMB address 82h, Tier 1, uptime 99 ms, 0 records, 0 overwritten"
        );
    }
}
