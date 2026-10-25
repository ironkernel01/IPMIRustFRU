//! capture.rs - the capture log: a ring of every whole IPMB frame the IPMC received and sent, and
//! of the trace lines, with the printers that turn it into CSV and a Wireshark-style text trace.
//!
//! A customer enables the log on a target system, reproduces a fault and mails the dump back;
//! the dump reads the same on the console, in a spreadsheet and (through `tools/capture`) in
//! Wireshark. No allocator, no hardware: the firmware owns one `DefaultLog` and the host tests
//! use small rings.
//!
//! Ring layout: one `[u8; N]`. A record is a 6-byte header followed by its payload:
//!
//! ```text
//! byte 0   flags: bit 0 kind (0 frame, 1 text), bit 1 bus (0 A, 1 B), bit 2 dir (0 in, 1 out),
//!                 bits 3..=6 status code (`FrameStatus::code`)
//! byte 1   payload length (a frame: at most IPMB_MAX_LEN; a text line: at most IPMC_LOG_TEXT_MAX)
//! 2..=5    time_ms, u32 little-endian
//! 6..      payload: the IPMB frame as it is on the wire, or the line as ASCII
//! ```
//!
//! The record index is not stored: the i-th held record is `total - records + i`. A record may
//! wrap the end of the ring (it is copied in two parts). A record that does not fit drops the
//! oldest whole records first.

use core::fmt::{self, Arguments, Display, Write};

use crate::commands;
use crate::ext::Extension;
use crate::ipmb::{self, IPMB_MAX_LEN, NETFN_APP, NETFN_GROUP_EXT, NETFN_SE, NETFN_STORAGE};
use crate::queue::Bus;
use crate::trace::Trace;

/// HEADER_LEN -- bytes of a record header (flags, length, time_ms).
pub const HEADER_LEN: usize = 6;
/// TEXT_MAX -- longest trace line kept (ipmi_defs.h IPMC_LOG_TEXT_MAX).
pub const TEXT_MAX: usize = ipmc_defs::IPMC_LOG_TEXT_MAX as usize;
/// LOG_SIZE -- bytes of the firmware's ring (ipmi_defs.h IPMC_LOG_SIZE).
pub const LOG_SIZE: usize = ipmc_defs::IPMC_LOG_SIZE as usize;
/// PAYLOAD_MAX -- the longest payload a record holds: a frame or a text line.
const PAYLOAD_MAX: usize = if IPMB_MAX_LEN > TEXT_MAX {
    IPMB_MAX_LEN
} else {
    TEXT_MAX
};

// The ring holds a few whole frames and at least one whole text line; ipmc-defs guards the
// ranges of the two defines. A smaller ring would drop every record it was asked to keep.
const _: () = assert!(
    LOG_SIZE >= 4 * (HEADER_LEN + ipmc_defs::IPMB_MSG_MAX as usize),
    "IPMC_LOG_SIZE must hold four whole frames"
);
const _: () = assert!(
    LOG_SIZE >= HEADER_LEN + TEXT_MAX,
    "IPMC_LOG_SIZE must hold one whole text line"
);
// The length byte of a record holds the payload length.
const _: () = assert!(
    PAYLOAD_MAX <= 255,
    "a payload length must fit the length byte"
);

/// Flags byte layout (module comment).
const FLAG_TEXT: u8 = 1 << 0;
const FLAG_BUS_B: u8 = 1 << 1;
const FLAG_OUT: u8 = 1 << 2;
const STATUS_SHIFT: u32 = 3;
const STATUS_MASK: u8 = 0x0F;

/// Dir -- which way a frame went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Received from the bus.
    In,
    /// Sent to the bus.
    Out,
}

impl Dir {
    /// name -- the CSV and trace word.
    pub const fn name(self) -> &'static str {
        match self {
            Dir::In => "IN",
            Dir::Out => "OUT",
        }
    }
}

/// TxFault -- why a transmission failed, as the master driver saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TxFault {
    /// The destination address was not acknowledged.
    AddressNack,
    /// A data byte was not acknowledged.
    DataNack,
    /// Another master won the bus.
    ArbitrationLost,
    /// The bus stayed busy.
    BusBusy,
    /// A device held SCL low past the stretch limit.
    StretchTimeout,
    /// SCL or SDA is stuck.
    LineStuck,
    /// No further detail.
    Unspecified,
}

impl TxFault {
    /// ALL -- every fault in code order.
    const ALL: [TxFault; 7] = [
        TxFault::AddressNack,
        TxFault::DataNack,
        TxFault::ArbitrationLost,
        TxFault::BusBusy,
        TxFault::StretchTimeout,
        TxFault::LineStuck,
        TxFault::Unspecified,
    ];

    /// short -- the CSV and trace word.
    pub const fn short(self) -> &'static str {
        match self {
            TxFault::AddressNack => "addr-nack",
            TxFault::DataNack => "data-nack",
            TxFault::ArbitrationLost => "arb-lost",
            TxFault::BusBusy => "bus-busy",
            TxFault::StretchTimeout => "stretch",
            TxFault::LineStuck => "line-stuck",
            TxFault::Unspecified => "failed",
        }
    }

    /// meaning -- the console cause of this fault for a frame addressed to `dest8`.
    pub const fn meaning(self, dest8: u8) -> FaultMeaning {
        FaultMeaning { fault: self, dest8 }
    }
}

/// FaultMeaning -- `TxFault::meaning`: the sentence that names the cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultMeaning {
    fault: TxFault,
    dest8: u8,
}

impl Display for FaultMeaning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dest = self.dest8;
        match self.fault {
            TxFault::AddressNack => write!(
                f,
                "address {dest:02X}h not acknowledged: no device answers at {dest:02X}h"
            ),
            TxFault::DataNack => write!(
                f,
                "a data byte was not acknowledged: the receiver at {dest:02X}h stopped accepting the frame"
            ),
            TxFault::ArbitrationLost => f.write_str("lost arbitration to another master"),
            TxFault::BusBusy => f.write_str("the bus stayed busy (another master held it)"),
            TxFault::StretchTimeout => {
                f.write_str("a device held SCL low past the clock-stretch limit")
            }
            TxFault::LineStuck => f.write_str("SCL or SDA is stuck"),
            TxFault::Unspecified => f.write_str("the transmission failed"),
        }
    }
}

/// FrameStatus -- what became of a frame. In: Received or Injected; Out: Sent or Failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    /// Received from the bus.
    Received,
    /// Handed to the link by the console (DEBUG INJECT).
    Injected,
    /// Transmitted.
    Sent,
    /// The transmission failed.
    Failed(TxFault),
}

impl FrameStatus {
    /// Codes 0..=2 are the three states without a fault; the faults follow from `FAULT_BASE`.
    const FAULT_BASE: u8 = 3;

    /// code -- the 4-bit value in the flags byte.
    const fn code(self) -> u8 {
        match self {
            FrameStatus::Received => 0,
            FrameStatus::Injected => 1,
            FrameStatus::Sent => 2,
            FrameStatus::Failed(fault) => Self::FAULT_BASE + fault as u8,
        }
    }

    /// from_code -- the status of a flags-byte code; a code no writer produces reads as an
    /// unspecified failure.
    fn from_code(code: u8) -> FrameStatus {
        match code {
            0 => FrameStatus::Received,
            1 => FrameStatus::Injected,
            2 => FrameStatus::Sent,
            _ => FrameStatus::Failed(
                TxFault::ALL
                    .get(usize::from(code.saturating_sub(Self::FAULT_BASE)))
                    .copied()
                    .unwrap_or(TxFault::Unspecified),
            ),
        }
    }

    /// short -- the CSV status word.
    pub const fn short(self) -> &'static str {
        match self {
            FrameStatus::Received => "rx",
            FrameStatus::Injected => "inject",
            FrameStatus::Sent => "ok",
            FrameStatus::Failed(fault) => fault.short(),
        }
    }
}

/// Stats -- the state of the ring for the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Ring size in bytes.
    pub size: usize,
    /// Bytes in use.
    pub used: usize,
    /// Records held.
    pub records: u32,
    /// Records dropped to make room, since the last clear.
    pub overwritten: u32,
    /// Records ever added, since the last clear (wrapping).
    pub total: u32,
    /// The log records now.
    pub enabled: bool,
}

/// Kind -- the content of one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    /// A whole IPMB frame, from rsSA/rqSA to chk2.
    Frame {
        /// The bus.
        bus: Bus,
        /// The direction.
        dir: Dir,
        /// What became of it.
        status: FrameStatus,
        /// The bytes as on the wire.
        bytes: &'a [u8],
    },
    /// A trace or fault line.
    Text(&'a str),
}

/// Record -- one held record, as `for_each` hands it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    /// Position in the order records were added since the last clear.
    pub index: u32,
    /// Time the record was added, milliseconds.
    pub time_ms: u32,
    /// The content.
    pub kind: Kind<'a>,
}

/// CaptureLog -- the ring of N bytes.
pub struct CaptureLog<const N: usize> {
    buf: [u8; N],
    /// Offset of the oldest record.
    head: usize,
    /// Bytes in use.
    used: usize,
    records: u32,
    overwritten: u32,
    total: u32,
    enabled: bool,
}

/// DefaultLog -- the ring of the firmware and the emulator (ipmi_defs.h IPMC_LOG_SIZE).
pub type DefaultLog = CaptureLog<LOG_SIZE>;

impl<const N: usize> CaptureLog<N> {
    /// RING_OK -- a ring needs at least one byte (the offsets wrap with `% N`).
    const RING_OK: () = assert!(N > 0, "a capture ring needs at least one byte");

    /// new -- an empty log, recording when `enabled`.
    pub const fn new(enabled: bool) -> Self {
        let () = Self::RING_OK;
        CaptureLog {
            buf: [0; N],
            head: 0,
            used: 0,
            records: 0,
            overwritten: 0,
            total: 0,
            enabled,
        }
    }

    /// set_enabled -- starts or stops recording; the held records stay.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// enabled -- the log records now.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// clear -- empties the ring and zeroes the counters; the enabled flag stays.
    pub fn clear(&mut self) {
        self.head = 0;
        self.used = 0;
        self.records = 0;
        self.overwritten = 0;
        self.total = 0;
    }

    /// stats -- the ring state.
    pub fn stats(&self) -> Stats {
        Stats {
            size: N,
            used: self.used,
            records: self.records,
            overwritten: self.overwritten,
            total: self.total,
            enabled: self.enabled,
        }
    }

    /// frame -- adds a whole frame (cut to IPMB_MAX_LEN) at `time_ms`; nothing while disabled.
    pub fn frame(&mut self, time_ms: u32, bus: Bus, dir: Dir, status: FrameStatus, bytes: &[u8]) {
        let kept = bytes.get(..IPMB_MAX_LEN).unwrap_or(bytes);
        let mut flags = status.code() << STATUS_SHIFT;
        if bus == Bus::B {
            flags |= FLAG_BUS_B;
        }
        if dir == Dir::Out {
            flags |= FLAG_OUT;
        }
        self.add(time_ms, flags, kept);
    }

    /// text -- formats one line into a stack buffer and adds it; a line longer than
    /// IPMC_LOG_TEXT_MAX is cut and ends in `...`. Nothing is formatted while disabled.
    pub fn text(&mut self, time_ms: u32, args: Arguments) {
        if !self.enabled {
            return;
        }
        let mut line = Bounded::<TEXT_MAX>::new();
        // Bounded never fails; a Display impl that fails leaves the part written so far.
        let _ = line.write_fmt(args);
        line.finish();
        self.add(time_ms, FLAG_TEXT, line.bytes());
    }

    /// for_each -- calls `f` for every held record, oldest first.
    pub fn for_each(&self, mut f: impl FnMut(&Record<'_>)) {
        let first = self.total.wrapping_sub(self.records);
        let mut pos = self.head;
        let mut scratch = [0u8; PAYLOAD_MAX];
        for offset in 0..self.records {
            let Some((flags, len, time_ms)) = self.header_at(pos) else {
                return;
            };
            let Some(payload) = scratch.get_mut(..len) else {
                return;
            };
            self.copy_out((pos + HEADER_LEN) % N, payload);
            f(&Record {
                index: first.wrapping_add(offset),
                time_ms,
                kind: decode(flags, payload),
            });
            pos = (pos + HEADER_LEN + len) % N;
        }
    }

    /// add -- appends one record, dropping the oldest whole records until it fits.
    fn add(&mut self, time_ms: u32, flags: u8, payload: &[u8]) {
        if !self.enabled {
            return;
        }
        let Ok(len) = u8::try_from(payload.len()) else {
            return;
        };
        let need = HEADER_LEN + payload.len();
        if need > N {
            return; // a ring smaller than the record (only a test ring)
        }
        while N - self.used < need && self.records > 0 {
            self.drop_oldest();
        }
        let [t0, t1, t2, t3] = time_ms.to_le_bytes();
        let tail = (self.head + self.used) % N;
        self.copy_in(tail, &[flags, len, t0, t1, t2, t3]);
        self.copy_in((tail + HEADER_LEN) % N, payload);
        self.used += need;
        self.records = self.records.saturating_add(1);
        self.total = self.total.wrapping_add(1);
    }

    /// drop_oldest -- removes the oldest whole record and counts it.
    fn drop_oldest(&mut self) {
        let Some((_, len, _)) = self.header_at(self.head) else {
            return;
        };
        let size = HEADER_LEN + len;
        self.head = (self.head + size) % N;
        self.used = self.used.saturating_sub(size);
        self.records = self.records.saturating_sub(1);
        self.overwritten = self.overwritten.saturating_add(1);
    }

    /// header_at -- (flags, payload length, time_ms) of the record at `pos`.
    fn header_at(&self, pos: usize) -> Option<(u8, usize, u32)> {
        let mut header = [0u8; HEADER_LEN];
        self.copy_out(pos, &mut header);
        let [flags, len, t0, t1, t2, t3] = header;
        Some((
            flags,
            usize::from(len),
            u32::from_le_bytes([t0, t1, t2, t3]),
        ))
    }

    /// copy_in -- writes `data` at ring offset `pos` (< N), wrapping the end in two parts.
    fn copy_in(&mut self, pos: usize, data: &[u8]) {
        let first = data.len().min(N.saturating_sub(pos));
        let (head_part, tail_part) = data.split_at(first);
        if let Some(dst) = self.buf.get_mut(pos..pos + first) {
            dst.copy_from_slice(head_part);
        }
        if let Some(dst) = self.buf.get_mut(..tail_part.len()) {
            dst.copy_from_slice(tail_part);
        }
    }

    /// copy_out -- reads `out.len()` bytes from ring offset `pos` (< N), wrapping the end.
    fn copy_out(&self, pos: usize, out: &mut [u8]) {
        for (slot, at) in out.iter_mut().zip(pos..) {
            *slot = self.buf.get(at % N).copied().unwrap_or(0);
        }
    }
}

/// decode -- the content of a record from its flags and payload.
fn decode(flags: u8, payload: &[u8]) -> Kind<'_> {
    if flags & FLAG_TEXT != 0 {
        return Kind::Text(core::str::from_utf8(payload).unwrap_or("?"));
    }
    Kind::Frame {
        bus: if flags & FLAG_BUS_B != 0 {
            Bus::B
        } else {
            Bus::A
        },
        dir: if flags & FLAG_OUT != 0 {
            Dir::Out
        } else {
            Dir::In
        },
        status: FrameStatus::from_code((flags >> STATUS_SHIFT) & STATUS_MASK),
        bytes: payload,
    }
}

/// CUT_MARK -- the end of a line that did not fit.
const CUT_MARK: &str = "...";

/// Bounded -- a `fmt::Write` into a stack buffer of N bytes that stops at the limit. The trace
/// line of the log (IPMC_LOG_TEXT_MAX) and the padded columns of a trace row use it.
struct Bounded<const N: usize> {
    buf: [u8; N],
    len: usize,
    /// A write did not fit.
    cut: bool,
}

impl<const N: usize> Write for Bounded<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.cut {
            return Ok(());
        }
        let mut take = s.len().min(N - self.len);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        if take < s.len() {
            self.cut = true;
        }
        if let (Some(dst), Some(src)) = (
            self.buf.get_mut(self.len..self.len + take),
            s.as_bytes().get(..take),
        ) {
            dst.copy_from_slice(src);
        }
        self.len += take;
        Ok(())
    }
}

impl<const N: usize> Bounded<N> {
    const fn new() -> Self {
        Bounded {
            buf: [0; N],
            len: 0,
            cut: false,
        }
    }

    /// bytes -- what was written.
    fn bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// as_str -- what was written, as text.
    fn as_str(&self) -> &str {
        core::str::from_utf8(self.bytes()).unwrap_or("?")
    }

    /// finish -- a cut line ends in `...` in its last three bytes (fewer, when a character the
    /// cut split is dropped).
    fn finish(&mut self) {
        if !self.cut {
            return;
        }
        let keep = self.len.saturating_sub(CUT_MARK.len());
        let valid = match core::str::from_utf8(self.buf.get(..keep).unwrap_or(&[])) {
            Ok(_) => keep,
            Err(e) => e.valid_up_to(),
        };
        if let Some(dst) = self.buf.get_mut(valid..valid + CUT_MARK.len()) {
            dst.copy_from_slice(CUT_MARK.as_bytes());
        }
        self.len = valid + CUT_MARK.len();
    }
}

// ---- names ----

/// Names -- the name of a command, for the summary and trace columns.
pub trait Names {
    /// command -- the name of the request (netfn even, the request form; cmd), None when unknown.
    fn command(&self, netfn: u8, cmd: u8) -> Option<&'static str>;
}

/// SENT_REQUESTS -- the requests the IPMC sends that no command table answers: (netFn, cmd, name).
/// Platform Event (IPMI v2.0 29.3) goes to the Event Receiver.
const SENT_REQUESTS: &[(u8, u8, &str)] = &[(NETFN_SE, 0x02, "Platform Event")];

/// CoreNames -- the names of this build: the Tier-1 table, then the extension, then the requests
/// the IPMC sends.
pub struct CoreNames<'a>(pub &'a dyn Extension);

impl Names for CoreNames<'_> {
    fn command(&self, netfn: u8, cmd: u8) -> Option<&'static str> {
        commands::find(netfn, cmd)
            .map(|row| row.name)
            .or_else(|| self.0.command_name(netfn, cmd))
            .or_else(|| {
                SENT_REQUESTS
                    .iter()
                    .find(|(n, c, _)| (*n, *c) == (netfn, cmd))
                    .map(|(_, _, name)| *name)
            })
    }
}

/// NETFN_NAMES -- IPMI v2.0 Table 5-1, request forms: (netFn, name).
const NETFN_NAMES: &[(u8, &str)] = &[
    (0x00, "Chassis"),
    (0x02, "Bridge"),
    (NETFN_SE, "S/E"),
    (NETFN_APP, "App"),
    (0x08, "Firmware"),
    (NETFN_STORAGE, "Storage"),
    (0x0C, "Transport"),
    (NETFN_GROUP_EXT, "Group Ext"),
    (0x2E, "OEM/Group"),
];

/// NetFnName -- the Table 5-1 name of a NetFn (either form), `NetFn 3Ah` when it has none.
pub struct NetFnName(pub u8);

impl Display for NetFnName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let request = self.0 & !1;
        match NETFN_NAMES.iter().find(|(n, _)| *n == request) {
            Some((_, name)) => f.write_str(name),
            None => write!(f, "NetFn {request:02X}h"),
        }
    }
}

/// CommandName -- the name of a command, `CMD 5Ah` when `names` has none.
pub struct CommandName<'a> {
    /// Where the names come from.
    pub names: &'a dyn Names,
    /// NetFn (either form).
    pub netfn: u8,
    /// Command code.
    pub cmd: u8,
}

impl Display for CommandName<'_> {
    /// fmt -- honours a width (`{:<22}`): the name goes through `f.pad`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.names.command(self.netfn & !1, self.cmd) {
            Some(name) => f.pad(name),
            None => {
                let mut label = Bounded::<TAIL_MAX>::new();
                let _ = write!(label, "CMD {:02X}h", self.cmd);
                f.pad(label.as_str())
            }
        }
    }
}

// ---- printers ----

/// HexBytes -- bytes as upper-case hex, one space apart.
pub struct HexBytes<'a>(pub &'a [u8]);

impl Display for HexBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

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

/// TAIL_MAX -- bytes of one padded column of a trace row (a `Display` value does not honour a
/// width on its own, so the column is built in a `Bounded` and padded as a `str`).
const TAIL_MAX: usize = 48;

// ---- the trace tee ----

/// CaptureTrace -- a `Trace` that records every line in the log and forwards it to `inner` when
/// `console` is set. The firmware builds one wherever it passes a trace sink to the core.
pub struct CaptureTrace<'a, const N: usize> {
    /// The log the lines go into.
    pub log: &'a mut CaptureLog<N>,
    /// The sink the lines are forwarded to (the console).
    pub inner: &'a mut dyn Trace,
    /// Forward to `inner`.
    pub console: bool,
    /// The time stamped on the record.
    pub now_ms: u32,
}

impl<const N: usize> Trace for CaptureTrace<'_, N> {
    fn line(&mut self, args: Arguments) {
        self.log.text(self.now_ms, args);
        if self.console {
            self.inner.line(args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::NoExtension;
    use crate::trace::RecordingTrace;

    type Small = CaptureLog<64>;

    /// Every record as (index, time, text of the kind) for ring tests.
    fn held<const N: usize>(log: &CaptureLog<N>) -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        log.for_each(|r| {
            let what = match r.kind {
                Kind::Text(t) => t.to_string(),
                Kind::Frame { bytes, .. } => format!("frame {}", bytes.len()),
            };
            out.push((r.index, r.time_ms, what));
        });
        out
    }

    fn request_in() -> Vec<u8> {
        // Get Device ID, 20h -> 82h, seq 1 (the example of the CSV format).
        vec![0x82, 0x18, 0x66, 0x20, 0x04, 0x01, 0xDB]
    }

    fn response_out() -> Vec<u8> {
        vec![0x20, 0x1C, 0xC4, 0x82, 0x04, 0x01, 0x00, 0x79]
    }

    fn platform_event() -> Vec<u8> {
        vec![
            0x20, 0x10, 0xD0, 0x82, 0x00, 0x02, 0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00, 0x78,
        ]
    }

    #[test]
    fn records_come_back_oldest_first() {
        let mut log = Small::new(true);
        // Each text record is 6 + 10 bytes; four fill 64 exactly, so the fifth drops one.
        for i in 0..4u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        assert_eq!(held(&log).len(), 4);
        assert_eq!(log.stats().used, 64);
        log.text(4, format_args!("line {:05}", 4));
        let want: Vec<_> = (1..5u32).map(|i| (i, i, format!("line {i:05}"))).collect();
        assert_eq!(held(&log), want);
    }

    #[test]
    fn the_ring_matches_a_model_across_every_wrap() {
        // The model keeps the same records in a Vec and drops from the front while the bytes
        // would not fit; the ring must hold exactly the same texts after every add, whatever
        // offset (inside a header, inside a payload) the end of the buffer falls on.
        let mut log = Small::new(true);
        let mut model: Vec<(u32, u32, String)> = Vec::new();
        let mut seed = 7u32;
        for n in 0..300u32 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let len = 1 + (seed >> 16) as usize % 20;
            let text: String = (0..len)
                .map(|i| char::from(b'a' + ((n as usize + i) % 26) as u8))
                .collect();
            log.text(n, format_args!("{text}"));
            model.push((n, n, text));
            while model.iter().map(|r| 6 + r.2.len()).sum::<usize>() > 64 {
                model.remove(0);
            }
            assert_eq!(held(&log), model, "after record {n}");
            assert_eq!(log.stats().records as usize, model.len());
        }
        let stats = log.stats();
        assert_eq!(stats.total, 300);
        assert_eq!(stats.overwritten, 300 - stats.records);
    }

    #[test]
    fn one_new_record_overwrites_several_old_ones() {
        let mut log = Small::new(true);
        for i in 0..8u32 {
            log.text(i, format_args!("{i}")); // 7 bytes each, 9 fit
        }
        assert_eq!(log.stats().records, 8);
        // A 40-byte payload needs 46 of 64 bytes: 56 are used, so six 7-byte records go.
        log.text(99, format_args!("{}", "x".repeat(40)));
        let stats = log.stats();
        assert_eq!((stats.records, stats.overwritten, stats.total), (3, 6, 9));
        let got = held(&log);
        assert_eq!(got.iter().map(|r| r.0).collect::<Vec<_>>(), [6, 7, 8]);
        assert_eq!(got[0].2, "6");
    }

    #[test]
    fn disabled_adds_nothing_and_clear_empties() {
        let mut log = Small::new(false);
        log.text(1, format_args!("a"));
        log.frame(2, Bus::A, Dir::In, FrameStatus::Received, &request_in());
        assert_eq!(log.stats().total, 0);
        log.set_enabled(true);
        log.text(3, format_args!("b"));
        assert_eq!((log.stats().records, log.stats().used), (1, 7));
        log.set_enabled(false);
        log.text(4, format_args!("c"));
        assert_eq!(log.stats().records, 1, "the held record stays");
        log.clear();
        let stats = log.stats();
        assert_eq!(
            (stats.records, stats.used, stats.overwritten, stats.total),
            (0, 0, 0, 0)
        );
        assert!(!stats.enabled);
        assert!(held(&log).is_empty());
    }

    #[test]
    fn a_long_text_line_is_cut_with_dots_and_a_long_frame_is_cut() {
        let mut log = DefaultLog::new(true);
        let long = "y".repeat(TEXT_MAX + 30);
        log.text(1, format_args!("{long}"));
        log.frame(
            2,
            Bus::A,
            Dir::In,
            FrameStatus::Received,
            &[0x55; IPMB_MAX_LEN + 5],
        );
        let mut seen = Vec::new();
        log.for_each(|r| match r.kind {
            Kind::Text(t) => seen.push((
                t.len(),
                t.ends_with("..."),
                t[..TEXT_MAX - 3].chars().all(|c| c == 'y'),
            )),
            Kind::Frame { bytes, .. } => seen.push((bytes.len(), false, false)),
        });
        assert_eq!(seen, [(TEXT_MAX, true, true), (IPMB_MAX_LEN, false, false)]);
        // A line that fits exactly is not cut.
        let mut log = DefaultLog::new(true);
        let exact = "z".repeat(TEXT_MAX);
        log.text(1, format_args!("{exact}"));
        log.for_each(|r| assert_eq!(r.kind, Kind::Text(&exact)));
    }

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
    fn the_test_frames_are_the_frames_they_claim_to_be() {
        assert_eq!(request_in(), REQ_IN);
        assert_eq!(response_out(), RSP_OUT);
        assert_eq!(platform_event(), PLATFORM);
        assert!(ipmb::parse(&REQ_IN).is_ok());
        assert!(ipmb::parse(&RSP_OUT).is_ok());
        assert!(ipmb::parse(&PLATFORM).is_ok());
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

    #[test]
    fn fault_words_and_sentences() {
        let cases = [
            (
                TxFault::AddressNack,
                "addr-nack",
                "address 20h not acknowledged: no device answers at 20h",
            ),
            (
                TxFault::DataNack,
                "data-nack",
                "a data byte was not acknowledged: the receiver at 20h stopped accepting the frame",
            ),
            (
                TxFault::ArbitrationLost,
                "arb-lost",
                "lost arbitration to another master",
            ),
            (
                TxFault::BusBusy,
                "bus-busy",
                "the bus stayed busy (another master held it)",
            ),
            (
                TxFault::StretchTimeout,
                "stretch",
                "a device held SCL low past the clock-stretch limit",
            ),
            (TxFault::LineStuck, "line-stuck", "SCL or SDA is stuck"),
            (TxFault::Unspecified, "failed", "the transmission failed"),
        ];
        for (fault, short, meaning) in cases {
            assert_eq!(fault.short(), short);
            assert_eq!(fault.meaning(0x20).to_string(), meaning);
            // The status code round-trips through the flags byte.
            let status = FrameStatus::Failed(fault);
            assert_eq!(FrameStatus::from_code(status.code()), status);
        }
        for status in [
            FrameStatus::Received,
            FrameStatus::Injected,
            FrameStatus::Sent,
        ] {
            assert_eq!(FrameStatus::from_code(status.code()), status);
        }
    }

    #[test]
    fn capture_trace_records_and_forwards_only_on_console() {
        let mut log = DefaultLog::new(true);
        let mut inner = RecordingTrace::default();
        for (console, forwarded) in [(false, 0usize), (true, 1)] {
            let mut tee = CaptureTrace {
                log: &mut log,
                inner: &mut inner,
                console,
                now_ms: 7,
            };
            tee.line(format_args!("hello {}", 5));
            assert_eq!(inner.lines.len(), forwarded);
        }
        assert_eq!(inner.lines, ["hello 5"]);
        assert_eq!(
            held(&log),
            [(0, 7, "hello 5".into()), (1, 7, "hello 5".into())]
        );
    }

    /// An extension that names one command.
    struct Named;

    impl Extension for Named {
        fn command(
            &mut self,
            _ctx: &mut crate::dispatch::Context,
            _req: &ipmb::Frame,
            _out: &mut [u8],
        ) -> Option<(&'static str, Result<usize, u8>)> {
            None
        }

        fn command_name(&self, netfn: u8, cmd: u8) -> Option<&'static str> {
            ((netfn, cmd) == (NETFN_GROUP_EXT, 0x70)).then_some("Ext Thing")
        }
    }

    #[test]
    fn core_names_find_tier1_extension_and_platform_event() {
        let names = CoreNames(&Named);
        // (netfn, cmd, name): the Tier-1 table, the extension, the sent request, a miss.
        let cases = [
            (NETFN_APP, 0x01, Some("Get Device ID")),
            (NETFN_GROUP_EXT, 0x70, Some("Ext Thing")),
            (NETFN_SE, 0x02, Some("Platform Event")),
            (NETFN_SE, 0x55, None),
        ];
        for (netfn, cmd, want) in cases {
            assert_eq!(names.command(netfn, cmd), want, "{netfn:02X}h/{cmd:02X}h");
        }
        assert_eq!(CoreNames(&NoExtension).command(NETFN_GROUP_EXT, 0x70), None);
        let label = |netfn, cmd| {
            CommandName {
                names: &names,
                netfn,
                cmd,
            }
            .to_string()
        };
        assert_eq!(label(NETFN_APP + 1, 0x01), "Get Device ID");
        assert_eq!(label(NETFN_SE, 0x55), "CMD 55h");
        assert_eq!(NetFnName(0x3A).to_string(), "NetFn 3Ah");
        assert_eq!(NetFnName(NETFN_GROUP_EXT + 1).to_string(), "Group Ext");
    }
}
