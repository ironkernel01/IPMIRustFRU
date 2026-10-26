//! record.rs - the capture log: a ring of every whole IPMB frame the IPMC received and sent, and
//! of the trace lines, the records and the ring (the printers are print.rs, the dump dump.rs).
//!
//! A customer enables the log on a target system, reproduces a fault and mails the dump back;
//! the dump reads the same on the console, in a spreadsheet and (through `tools/capture`) in
//! Wireshark. No allocator, no hardware: the firmware owns one `DefaultLog` and the host tests
//! use small rings.
//!
//! Ring layout: one `ByteRing<N>` (ipmc_core::ring; N a power of two, N - 1 bytes usable). A record is a 6-byte header followed by its payload:
//!
//! ```text
//! byte 0   flags: bit 0 kind (0 frame, 1 text), bit 1 bus (0 A, 1 B), bit 2 dir (0 in, 1 out),
//!                 bits 3..=6 status code (`FrameStatus::code`)
//! byte 1   payload length (a frame: at most IPMB_MAX_LEN; a text line: at most IPMC_LOG_TEXT_MAX)
//! 2..=5    time_ms, u32 little-endian
//! 6..      payload: the IPMB frame as it is on the wire, or the line as ASCII
//! ```
//!
//! The record index is not stored: the i-th held record is `total - records + i`. A record that
//! does not fit drops the oldest whole records first (`ByteRing::discard`).

use core::fmt::Arguments;

use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::names::{format_cut, TxFault};
use ipmc_core::queue::Bus;
use ipmc_core::ring::ByteRing;

/// HEADER_LEN -- bytes of a record header (flags, length, time_ms).
pub const HEADER_LEN: usize = 6;
/// TEXT_MAX -- longest trace line kept (defs.rs IPMC_LOG_TEXT_MAX).
pub const TEXT_MAX: usize = ipmc_defs::IPMC_LOG_TEXT_MAX;
/// LOG_SIZE -- bytes of the firmware's ring (defs.rs IPMC_LOG_SIZE).
pub const LOG_SIZE: usize = ipmc_defs::IPMC_LOG_SIZE;
/// PAYLOAD_MAX -- the longest payload a record holds: a frame or a text line.
pub const PAYLOAD_MAX: usize = if IPMB_MAX_LEN > TEXT_MAX {
    IPMB_MAX_LEN
} else {
    TEXT_MAX
};

// The ring holds a few whole frames and at least one whole text line; ipmc-defs guards the
// ranges of the two defines. A smaller ring would drop every record it was asked to keep. The
// usable size is LOG_SIZE - 1 (a ByteRing keeps one slot empty), and LOG_SIZE is a power of two.
const _: () = assert!(
    LOG_SIZE.is_power_of_two(),
    "IPMC_LOG_SIZE must be a power of two"
);
const _: () = assert!(
    ByteRing::<LOG_SIZE>::capacity() >= 4 * (HEADER_LEN + ipmc_defs::IPMB_MSG_MAX),
    "IPMC_LOG_SIZE - 1 must hold four whole frames"
);
const _: () = assert!(
    ByteRing::<LOG_SIZE>::capacity() >= HEADER_LEN + TEXT_MAX,
    "IPMC_LOG_SIZE - 1 must hold one whole text line"
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
    pub(crate) const fn code(self) -> u8 {
        match self {
            FrameStatus::Received => 0,
            FrameStatus::Injected => 1,
            FrameStatus::Sent => 2,
            FrameStatus::Failed(fault) => Self::FAULT_BASE + fault as u8,
        }
    }

    /// from_code -- the status of a flags-byte code; a code no writer produces reads as an
    /// unspecified failure.
    pub(crate) fn from_code(code: u8) -> FrameStatus {
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
    /// Usable ring size in bytes (the ring size - 1).
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
    ring: ByteRing<N>,
    records: u32,
    overwritten: u32,
    total: u32,
    enabled: bool,
}

/// DefaultLog -- the ring of the firmware and the emulator (defs.rs IPMC_LOG_SIZE).
pub type DefaultLog = CaptureLog<LOG_SIZE>;

impl<const N: usize> CaptureLog<N> {
    /// new -- an empty log, recording when `enabled`.
    pub const fn new(enabled: bool) -> Self {
        CaptureLog {
            ring: ByteRing::new(),
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
        self.ring.clear();
        self.records = 0;
        self.overwritten = 0;
        self.total = 0;
    }

    /// stats -- the ring state.
    pub fn stats(&self) -> Stats {
        Stats {
            size: ByteRing::<N>::capacity(),
            used: self.ring.len(),
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
        let line = format_cut::<TEXT_MAX>(args);
        self.add(time_ms, FLAG_TEXT, line.bytes());
    }

    /// for_each -- calls `f` for every held record, oldest first.
    pub fn for_each(&self, mut f: impl FnMut(&Record<'_>)) {
        let first = self.total.wrapping_sub(self.records);
        let mut pos = 0;
        let mut scratch = [0u8; PAYLOAD_MAX];
        for offset in 0..self.records {
            let Some((record, next)) =
                self.record_at(pos, first.wrapping_add(offset), &mut scratch)
            else {
                return;
            };
            f(&record);
            pos = next;
        }
    }

    /// get -- the held record with `index` (the number `Record::index` carries), None when it
    /// was overwritten or has not been added. `scratch` holds the payload the record borrows.
    pub fn get<'s>(&self, index: u32, scratch: &'s mut [u8; PAYLOAD_MAX]) -> Option<Record<'s>> {
        let first = self.total.wrapping_sub(self.records);
        let wanted = index.wrapping_sub(first);
        if wanted >= self.records {
            return None;
        }
        let mut pos = 0;
        for _ in 0..wanted {
            let (_, len, _) = self.header_at(pos)?;
            pos += HEADER_LEN + len;
        }
        self.record_at(pos, index, scratch)
            .map(|(record, _)| record)
    }

    /// record_at -- the record whose header starts at ring offset `pos` (0 is the oldest byte),
    /// copied into `scratch`, and the offset of the next record.
    fn record_at<'s>(
        &self,
        pos: usize,
        index: u32,
        scratch: &'s mut [u8; PAYLOAD_MAX],
    ) -> Option<(Record<'s>, usize)> {
        let (flags, len, time_ms) = self.header_at(pos)?;
        let payload = scratch.get_mut(..len)?;
        self.copy_out(pos + HEADER_LEN, payload);
        Some((
            Record {
                index,
                time_ms,
                kind: decode(flags, payload),
            },
            pos + HEADER_LEN + len,
        ))
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
        if need > ByteRing::<N>::capacity() {
            return; // a ring smaller than the record (only a test ring)
        }
        while self.ring.free() < need && self.records > 0 {
            self.drop_oldest();
        }
        let [t0, t1, t2, t3] = time_ms.to_le_bytes();
        let stored =
            self.ring.push_slice(&[flags, len, t0, t1, t2, t3]) && self.ring.push_slice(payload);
        if !stored {
            return; // cannot happen: the room was made above
        }
        self.records = self.records.saturating_add(1);
        self.total = self.total.wrapping_add(1);
    }

    /// drop_oldest -- removes the oldest whole record and counts it.
    fn drop_oldest(&mut self) {
        let Some((_, len, _)) = self.header_at(0) else {
            return;
        };
        self.ring.discard(HEADER_LEN + len);
        self.records = self.records.saturating_sub(1);
        self.overwritten = self.overwritten.saturating_add(1);
    }

    /// header_at -- (flags, payload length, time_ms) of the record at ring offset `pos`.
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

    /// copy_out -- reads `out.len()` bytes from ring offset `pos`, the oldest byte being 0.
    fn copy_out(&self, pos: usize, out: &mut [u8]) {
        for (slot, at) in out.iter_mut().zip(pos..) {
            *slot = self.ring.peek_at(at).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;

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

    const REQUEST_IN: [u8; 7] = [0x82, 0x18, 0x66, 0x20, 0x04, 0x01, 0xDB];

    #[test]
    fn records_come_back_oldest_first() {
        let mut log = Small::new(true);
        // Each text record is 6 + 10 bytes; the ring of 64 holds 63 usable bytes, so three fill
        // 48 and the fourth drops one.
        for i in 0..3u32 {
            log.text(i, format_args!("line {i:05}"));
        }
        assert_eq!(held(&log).len(), 3);
        assert_eq!(log.stats().used, 48);
        log.text(3, format_args!("line {:05}", 3));
        let want: Vec<_> = (1..4u32).map(|i| (i, i, format!("line {i:05}"))).collect();
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
            while model.iter().map(|r| 6 + r.2.len()).sum::<usize>() > 63 {
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
            log.text(i, format_args!("{i}")); // 7 bytes each, 9 fit in 63
        }
        assert_eq!(log.stats().records, 8);
        // A 40-byte payload needs 46 of 63 bytes: 56 are used, so six 7-byte records go.
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
        log.frame(2, Bus::A, Dir::In, FrameStatus::Received, &REQUEST_IN);
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

    #[test]
    fn the_status_code_round_trips_through_the_flags_byte() {
        for fault in TxFault::ALL {
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
}
