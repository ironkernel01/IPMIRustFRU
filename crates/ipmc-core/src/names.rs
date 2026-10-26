//! names.rs - the names and bounded text the link layer and the printers share: the transmit
//! fault words, the command and NetFn names, the bounded line formatter and the hex bytes.

use core::fmt::{self, Arguments, Display, Write};

use crate::commands;
use crate::ext::Extension;
use crate::ipmb::{NETFN_APP, NETFN_GROUP_EXT, NETFN_SE, NETFN_STORAGE};

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
    pub const ALL: [TxFault; 7] = [
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

/// format_cut -- formats `args` into a stack buffer of N bytes. A line longer than N is cut and
/// ends in `...`. The log's text records and the console's trace lines (the firmware UART
/// driver) are formatted by this one function.
pub fn format_cut<const N: usize>(args: Arguments) -> Bounded<N> {
    let mut line = Bounded::<N>::new();
    // Bounded never fails; a Display impl that fails leaves the part written so far.
    let _ = line.write_fmt(args);
    line.finish();
    line
}

/// CUT_MARK -- the end of a line that did not fit.
const CUT_MARK: &str = "...";

/// Bounded -- a `fmt::Write` into a stack buffer of N bytes that stops at the limit. The trace
/// line of the log (IPMC_LOG_TEXT_MAX) and the padded columns of a trace row use it.
pub struct Bounded<const N: usize> {
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

impl<const N: usize> Default for Bounded<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Bounded<N> {
    pub const fn new() -> Self {
        Bounded {
            buf: [0; N],
            len: 0,
            cut: false,
        }
    }

    /// bytes -- what was written.
    pub fn bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// as_str -- what was written, as text.
    pub fn as_str(&self) -> &str {
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

/// TAIL_MAX -- bytes of one padded column of a trace row (a `Display` value does not honour a
/// width on its own, so the column is built in a `Bounded` and padded as a `str`).
pub const TAIL_MAX: usize = 48;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::NoExtension;
    use crate::ipmb;

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
        }
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
