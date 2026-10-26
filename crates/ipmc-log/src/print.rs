//! print.rs - the printers of a record: one CSV line and one Wireshark-style text line, the same
//! `Display` types the live I2C trace and DEBUG DUMP use.

use core::fmt::{self, Display, Write};

use ipmc_core::ipmb;
use ipmc_core::names::{Bounded, CommandName, HexBytes, Names, NetFnName, TAIL_MAX};
use ipmc_core::queue::Bus;

use crate::record::{FrameStatus, Kind, Record};

/// DoubleQuotes -- a `fmt::Write` that doubles every `"` (CSV quoting).
struct DoubleQuotes<'a, 'b>(&'a mut fmt::Formatter<'b>);

impl Write for DoubleQuotes<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for (i, part) in s.split('"').enumerate() {
            if i > 0 {
                self.0.write_str("\"\"")?;
            }
            self.0.write_str(part)?;
        }
        Ok(())
    }
}

/// CsvQuoted -- `value` as one quoted CSV field.
struct CsvQuoted<T: Display>(T);

impl<T: Display> Display for CsvQuoted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        write!(DoubleQuotes(f), "{}", self.0)?;
        f.write_str("\"")
    }
}

/// Summary -- `20h->82h App Req Get Device ID seq 01h` (and ` CC 00h` for a response) of a
/// frame, or `unparsed: HeaderChecksum`.
struct Summary<'a> {
    bytes: &'a [u8],
    names: &'a dyn Names,
}

impl Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let frame = match ipmb::parse(self.bytes) {
            Ok(frame) => frame,
            Err(error) => return write!(f, "unparsed: {}", error.name()),
        };
        let h = &frame.header;
        write!(
            f,
            "{:02X}h->{:02X}h {} {} {} seq {:02X}h",
            h.src,
            h.dest,
            NetFnName(h.netfn),
            request_or_response(h.is_response()),
            CommandName {
                names: self.names,
                netfn: h.netfn,
                cmd: h.cmd
            },
            h.seq
        )?;
        match frame.completion_code {
            Some(cc) => write!(f, " CC {cc:02X}h"),
            None => Ok(()),
        }
    }
}

/// request_or_response -- the `Req` / `Rsp` word.
const fn request_or_response(response: bool) -> &'static str {
    if response {
        "Rsp"
    } else {
        "Req"
    }
}

/// bus_letter -- `A` or `B`.
fn bus_letter(bus: Bus) -> &'static str {
    bus.name()
}

/// CsvHeader -- the column names of `CsvRow`.
pub struct CsvHeader;

impl Display for CsvHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("rec,time_ms,type,bus,dir,status,len,summary,bytes")
    }
}

/// CsvRow -- one record as one CSV line (no line ending).
pub struct CsvRow<'a>(pub &'a Record<'a>, pub &'a dyn Names);

impl Display for CsvRow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Record {
            index,
            time_ms,
            kind,
        } = self.0;
        match kind {
            Kind::Frame {
                bus,
                dir,
                status,
                bytes,
            } => write!(
                f,
                "{index},{time_ms},FRAME,{},{},{},{},{},{}",
                bus_letter(*bus),
                dir.name(),
                status.short(),
                bytes.len(),
                CsvQuoted(Summary {
                    bytes,
                    names: self.1
                }),
                HexBytes(bytes)
            ),
            Kind::Text(text) => write!(f, "{index},{time_ms},TEXT,,,,,{},", CsvQuoted(text)),
        }
    }
}

/// TraceRow -- one record as a Wireshark-style text line (no line ending):
///
/// ```text
/// #12   123.456  IPMB-A  IN   20h -> 82h  App Req       Get Device ID          seq 01h          | 82 18 66 20 04 01 DB
/// #13   123.460  TEXT    <text>
/// ```
pub struct TraceRow<'a>(pub &'a Record<'a>, pub &'a dyn Names);

/// MS_PER_SECOND -- milliseconds in the seconds column.
const MS_PER_SECOND: u32 = 1000;

impl Display for TraceRow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Record {
            index,
            time_ms,
            kind,
        } = self.0;
        write!(
            f,
            "#{index:<5}{:>3}.{:03}  ",
            time_ms / MS_PER_SECOND,
            time_ms % MS_PER_SECOND
        )?;
        let Kind::Frame {
            bus,
            dir,
            status,
            bytes,
        } = kind
        else {
            let Kind::Text(text) = kind else {
                return Ok(());
            };
            return write!(f, "{:<8}{text}", "TEXT");
        };
        write!(f, "IPMB-{:<3}{:<5}", bus_letter(*bus), dir.name())?;
        let frame = match ipmb::parse(bytes) {
            Ok(frame) => frame,
            Err(error) => return write!(f, "unparsed: {} | {}", error.name(), HexBytes(bytes)),
        };
        let h = &frame.header;
        let mut tail = Bounded::<TAIL_MAX>::new();
        // The tail is short by construction (seq, CC, one status word); TailBuf cuts otherwise.
        let _ = write!(tail, "seq {:02X}h", h.seq);
        if let Some(cc) = frame.completion_code {
            let _ = write!(tail, " CC {cc:02X}h");
        }
        if matches!(status, FrameStatus::Injected | FrameStatus::Failed(_)) {
            let _ = write!(tail, " {}", status.short());
        }
        // The NetFn column is 14 wide: the longest name ("NetFn 3Ah Req", "Transport Req",
        // "Group Ext Req") is 13 characters, so one space always follows it.
        let mut netfn = Bounded::<TAIL_MAX>::new();
        let _ = write!(
            netfn,
            "{} {}",
            NetFnName(h.netfn),
            request_or_response(h.is_response())
        );
        write!(
            f,
            "{:02X}h -> {:02X}h  {:<14}{:<22} {:<16} | {}",
            h.src,
            h.dest,
            netfn.as_str(),
            CommandName {
                names: self.1,
                netfn: h.netfn,
                cmd: h.cmd
            },
            tail.as_str(),
            HexBytes(bytes)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DefaultLog, Dir};
    use ipmc_core::ext::NoExtension;
    use ipmc_core::names::{CoreNames, TxFault};

    /// One printed row of each shape: (record, CSV, trace).
    #[test]
    fn rows_print_exactly() {
        let names = CoreNames(&NoExtension);
        let cases: [(Kind, &str, &str); 7] = [
            (
                frame_kind(Bus::A, Dir::In, FrameStatus::Received, &REQ_IN),
                "12,123456,FRAME,A,IN,rx,7,\"20h->82h App Req Get Device ID seq 01h\",82 18 66 20 04 01 DB",
                "#12   123.456  IPMB-A  IN   20h -> 82h  App Req       Get Device ID          seq 01h          | 82 18 66 20 04 01 DB",
            ),
            (
                frame_kind(Bus::B, Dir::Out, FrameStatus::Sent, &RSP_OUT),
                "12,123456,FRAME,B,OUT,ok,8,\"82h->20h App Rsp Get Device ID seq 01h CC 00h\",20 1C C4 82 04 01 00 79",
                "#12   123.456  IPMB-B  OUT  82h -> 20h  App Rsp       Get Device ID          seq 01h CC 00h   | 20 1C C4 82 04 01 00 79",
            ),
            (
                frame_kind(Bus::A, Dir::Out, FrameStatus::Failed(TxFault::AddressNack), &PLATFORM),
                "12,123456,FRAME,A,OUT,addr-nack,14,\"82h->20h S/E Req Platform Event seq 00h\",20 10 D0 82 00 02 04 F0 00 6F A1 00 00 78",
                "#12   123.456  IPMB-A  OUT  82h -> 20h  S/E Req       Platform Event         seq 00h addr-nack | 20 10 D0 82 00 02 04 F0 00 6F A1 00 00 78",
            ),
            (
                frame_kind(Bus::A, Dir::In, FrameStatus::Injected, &REQ_IN),
                "12,123456,FRAME,A,IN,inject,7,\"20h->82h App Req Get Device ID seq 01h\",82 18 66 20 04 01 DB",
                "#12   123.456  IPMB-A  IN   20h -> 82h  App Req       Get Device ID          seq 01h inject   | 82 18 66 20 04 01 DB",
            ),
            (
                frame_kind(Bus::A, Dir::In, FrameStatus::Received, &GROUP_REQ),
                "12,123456,FRAME,A,IN,rx,8,\"20h->82h Group Ext Req Get VSO Capabilities seq 01h\",82 B0 CE 20 04 00 03 D9",
                "#12   123.456  IPMB-A  IN   20h -> 82h  Group Ext Req Get VSO Capabilities   seq 01h          | 82 B0 CE 20 04 00 03 D9",
            ),
            (
                frame_kind(Bus::A, Dir::In, FrameStatus::Received, &BAD_CHK),
                "12,123456,FRAME,A,IN,rx,7,\"unparsed: HeaderChecksum\",82 18 67 20 04 01 DB",
                "#12   123.456  IPMB-A  IN   unparsed: HeaderChecksum | 82 18 67 20 04 01 DB",
            ),
            (
                Kind::Text("say \"hi\", ok"),
                "12,123456,TEXT,,,,,\"say \"\"hi\"\", ok\",",
                "#12   123.456  TEXT    say \"hi\", ok",
            ),
        ];
        for (kind, csv, trace) in cases {
            let record = Record {
                index: 12,
                time_ms: 123_456,
                kind,
            };
            assert_eq!(CsvRow(&record, &names).to_string(), csv);
            assert_eq!(TraceRow(&record, &names).to_string(), trace);
        }
        assert_eq!(
            CsvHeader.to_string(),
            "rec,time_ms,type,bus,dir,status,len,summary,bytes"
        );
    }

    const REQ_IN: [u8; 7] = [0x82, 0x18, 0x66, 0x20, 0x04, 0x01, 0xDB];
    /// A Group Extension request: the longest NetFn column name (13 characters) leaves a space.
    const GROUP_REQ: [u8; 8] = [0x82, 0xB0, 0xCE, 0x20, 0x04, 0x00, 0x03, 0xD9];
    const BAD_CHK: [u8; 7] = [0x82, 0x18, 0x67, 0x20, 0x04, 0x01, 0xDB];
    const RSP_OUT: [u8; 8] = [0x20, 0x1C, 0xC4, 0x82, 0x04, 0x01, 0x00, 0x79];
    const PLATFORM: [u8; 14] = [
        0x20, 0x10, 0xD0, 0x82, 0x00, 0x02, 0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00, 0x78,
    ];

    fn frame_kind<'a>(bus: Bus, dir: Dir, status: FrameStatus, bytes: &'a [u8]) -> Kind<'a> {
        Kind::Frame {
            bus,
            dir,
            status,
            bytes,
        }
    }

    #[test]
    fn the_test_frames_parse() {
        for frame in [&REQ_IN[..], &RSP_OUT[..], &PLATFORM[..]] {
            assert!(ipmb::parse(frame).is_ok());
        }
    }

    #[test]
    fn rows_of_a_ring_carry_the_record_index() {
        let names = CoreNames(&NoExtension);
        let mut log = DefaultLog::new(true);
        log.text(10, format_args!("one"));
        log.frame(20, Bus::A, Dir::In, FrameStatus::Received, &REQ_IN);
        let mut rows = Vec::new();
        log.for_each(|r| rows.push(CsvRow(r, &names).to_string()));
        assert_eq!(rows[0], "0,10,TEXT,,,,,\"one\",");
        assert!(rows[1].starts_with("1,20,FRAME,A,IN,rx,7,"));
    }
}
