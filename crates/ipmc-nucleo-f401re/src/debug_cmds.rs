// debug_cmds.rs - the DEBUG console group: test switches, the IPMB capture log and local IPMI
// requests.
//
// The state lives in ipmb_link.rs (the capture log, the two trace switches, the link and the
// dispatch context); these handlers switch it, print it and run requests through it. Decoding
// and printing of the log come from ipmc_core::capture (CSV rows and trace rows are the same
// Display types the live I2C trace uses); nothing here reads an IPMB byte layout of its own.
// The words typed for IPMI and INJECT are hex bytes, parsed by ipmc_wire::hex_byte.
//
// Nothing here runs in an interrupt handler.

use core::fmt;

use ipmc_core::capture::{
    CommandName, CsvHeader, CsvRow, DumpCursor, HexBytes, NetFnName, Step, TraceRow, PAYLOAD_MAX,
    TEXT_MAX,
};
use ipmc_core::dispatch::cc;
use ipmc_core::ipmb::{self, IPMB_MAX_LEN, NETFN_SE};
use ipmc_core::link::{LocalError, CM_ADDR8};
use ipmc_core::queue::Bus;
use ipmc_menu::{optional, Args, CmdError, CmdResult};

use crate::bsp::systick;
use crate::bsp::uart::{self, Port};
use crate::console::Context;
use crate::ipmb_link::IpmbLink;

/// CMD_SET_EVENT_RECEIVER -- Set Event Receiver, S/E NetFn (IPMI v2.0 section 29.1).
const CMD_SET_EVENT_RECEIVER: u8 = 0x00;
/// CMD_GET_EVENT_RECEIVER -- Get Event Receiver, S/E NetFn (IPMI v2.0 section 29.2).
const CMD_GET_EVENT_RECEIVER: u8 = 0x01;
/// RECEIVER_OFF -- the Event Receiver address that disables event messages (IPMI v2.0 29.1).
const RECEIVER_OFF: u8 = 0xFF;
/// RECEIVER_LUN_MASK -- the Event Receiver LUN is bits 1:0 of the second data byte.
const RECEIVER_LUN_MASK: u8 = 0x03;

/// on_off -- the switch word.
const fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// reported -- prints one error line and stops the handler.
fn reported(message: fmt::Arguments) -> CmdError {
    out!("Error: {}\n", message);
    CmdError::Reported
}

// ---- hex words ----

/// hex_word -- one typed byte: `06`, `0x06` and `6` are all 06h.
fn hex_word(text: &str) -> Result<u8, CmdError> {
    ipmc_wire::hex_word(text).ok_or_else(|| reported(format_args!("'{text}' is not a hex byte")))
}

/// hex_words -- the words of `args` from index `first` on as bytes in `buf`; the count.
fn hex_words(
    args: &Args<'_>,
    first: usize,
    buf: &mut [u8; IPMB_MAX_LEN],
) -> Result<usize, CmdError> {
    let mut count = 0;
    while let Some(word) = args.get(first + count) {
        let slot = buf
            .get_mut(count)
            .ok_or_else(|| reported(format_args!("more than {IPMB_MAX_LEN} bytes")))?;
        *slot = hex_word(word)?;
        count += 1;
    }
    Ok(count)
}

// ---- local requests ----

/// Reply -- the response frame of a local request (also used by the application port).
pub(crate) struct Reply {
    len: usize,
    bytes: [u8; IPMB_MAX_LEN],
}

impl Reply {
    /// frame -- the parsed response; None when it does not parse (the core built it, so never).
    fn frame(&self) -> Option<ipmb::Frame<'_>> {
        ipmb::parse(self.bytes.get(..self.len)?).ok()
    }

    /// completion_code -- the completion code of the response.
    pub(crate) fn completion_code(&self) -> u8 {
        self.frame()
            .and_then(|f| f.completion_code)
            .unwrap_or(cc::UNSPECIFIED)
    }

    /// data -- the response data after the completion code.
    pub(crate) fn data(&self) -> &[u8] {
        self.frame().map_or(&[], |f| f.data)
    }
}

/// local -- runs a request locally (`IpmbLink::local_request`).
pub(crate) fn local(
    link: &mut IpmbLink,
    netfn: u8,
    cmd: u8,
    data: &[u8],
) -> Result<Reply, LocalError> {
    let (len, bytes) = link.local_request(netfn, cmd, data)?;
    Ok(Reply { len, bytes })
}

// ---- event receiver ----

/// ReceiverState -- what Get Event Receiver answers, for the status line.
enum ReceiverState {
    /// Event messages go to this address and LUN.
    On { addr8: u8, lun: u8 },
    /// Address FFh: event messages are off.
    Off,
    /// The build has no Get Event Receiver (completion code C1h).
    NoEvents,
    /// Another completion code, or a request that could not run.
    Failed(u8),
}

impl fmt::Display for ReceiverState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReceiverState::On { addr8, lun } => write!(f, "{addr8:02X}h LUN {lun}"),
            ReceiverState::Off => write!(f, "off ({RECEIVER_OFF:02X}h)"),
            ReceiverState::NoEvents => {
                write!(f, "none in this build ({:02X}h)", cc::INVALID_CMD)
            }
            ReceiverState::Failed(code) => write!(f, "Get Event Receiver answered CC {code:02X}h"),
        }
    }
}

/// receiver_state -- Get Event Receiver, locally.
fn receiver_state(link: &mut IpmbLink) -> ReceiverState {
    let Ok(reply) = local(link, NETFN_SE, CMD_GET_EVENT_RECEIVER, &[]) else {
        return ReceiverState::Failed(cc::UNSPECIFIED);
    };
    match (reply.completion_code(), reply.data()) {
        (cc::OK, [RECEIVER_OFF, _]) => ReceiverState::Off,
        (cc::OK, [addr8, lun_byte]) => ReceiverState::On {
            addr8: *addr8,
            lun: lun_byte & RECEIVER_LUN_MASK,
        },
        (cc::INVALID_CMD, _) => ReceiverState::NoEvents,
        (code, _) => ReceiverState::Failed(code),
    }
}

/// EventsResult -- how a local Set Event Receiver ended.
pub(crate) enum EventsResult {
    /// The Event Receiver was set.
    Set,
    /// The build sends no event messages (completion code C1h).
    NoEvents,
    /// Another completion code.
    Refused(u8),
    /// The request could not run.
    Failed(LocalError),
}

/// set_events -- Set Event Receiver, locally: the CM address (events on, VITA 46.11 Rule 7-5) or
/// FFh (off), LUN 0. The one function of DEBUG EVENTS and of the start-up with
/// IPMC_EVENTS_AT_BOOT false.
pub(crate) fn set_events(link: &mut IpmbLink, on: bool) -> EventsResult {
    let receiver = if on { CM_ADDR8 } else { RECEIVER_OFF };
    match local(link, NETFN_SE, CMD_SET_EVENT_RECEIVER, &[receiver, 0]) {
        Err(error) => EventsResult::Failed(error),
        Ok(reply) => match reply.completion_code() {
            cc::OK => EventsResult::Set,
            cc::INVALID_CMD => EventsResult::NoEvents,
            code => EventsResult::Refused(code),
        },
    }
}

/// events_off_at_boot -- the start-up with IPMC_EVENTS_AT_BOOT false: sends the IPMC itself Set
/// Event Receiver FFh and says so, unless the build has no event messages (C1h).
pub(crate) fn events_off_at_boot(link: &mut IpmbLink) {
    if ipmc_defs::IPMC_EVENTS_AT_BOOT {
        return;
    }
    match set_events(link, false) {
        EventsResult::NoEvents => {}
        EventsResult::Set => {
            out!("Events: off from start-up (defs.rs IPMC_EVENTS_AT_BOOT false)\n");
        }
        EventsResult::Refused(code) => {
            out!("Events: start-up Set Event Receiver FFh answered CC {code:02X}h\n");
        }
        EventsResult::Failed(error) => {
            out!(
                "Events: start-up Set Event Receiver FFh failed: {}\n",
                error.name()
            );
        }
    }
}

// ---- status ----

/// print_log -- the capture log line of DEBUG STATUS and DEBUG LOG.
fn print_log(link: &IpmbLink) {
    let s = link.capture().stats();
    out!(
        "Capture log:     {}, {} bytes, used {}, {} records, {} overwritten\n",
        on_off(s.enabled),
        s.size,
        s.used,
        s.records,
        s.overwritten
    );
}

/// print_switches -- the two trace switches (DEBUG STATUS and DEBUG TRACE without argument).
fn print_switches(link: &IpmbLink) {
    out!("IPMI console:    trace {}\n", on_off(link.ipmi_console()));
    out!("I2C trace:       {}\n", on_off(link.i2c_trace()));
}

/// cmd_debug_status -- DEBUG STATUS.
pub fn cmd_debug_status(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    print_log(ctx.ipmb);
    print_switches(ctx.ipmb);
    let receiver = receiver_state(ctx.ipmb);
    out!("Event receiver:  {}\n", receiver);
    out!(
        "Duplicate cache: {} entries\n",
        ctx.ipmb.link().duplicate_cache_len()
    );
    Ok(())
}

// ---- capture log ----

/// cmd_debug_log -- DEBUG LOG {STATE}.
pub fn cmd_debug_log(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    if let Some(on) = optional(args.on_off_at(0))? {
        ctx.ipmb.capture_mut().set_enabled(on);
    }
    print_log(ctx.ipmb);
    Ok(())
}

/// DumpFormat -- the two printers of DEBUG DUMP.
#[derive(Clone, Copy)]
enum DumpFormat {
    Csv,
    Trace,
}

/// ROW_MAX -- room one dump step needs in the console TX ring before it prints a row, so a row
/// never waits for the wire. The longest row is the larger of a text record and a frame record,
/// each with its CR LF:
///   text:  index (10) + time (10) + `TEXT` and commas (12) + quotes (2) + the text, which a
///          CSV row may double when every character is a quote, so 2 x IPMC_LOG_TEXT_MAX
///          + 2 for CR LF = 2 x TEXT_MAX + 36
///   frame: fixed columns and the summary (at most 160 with the trace row's padded NetFn and
///          command columns) + 3 characters per frame byte (hex and a space)
///          + 2 for CR LF = 3 x IPMB_MAX_LEN + 162
/// A bigger TX ring is a design input: the const assert below names the define.
const TEXT_ROW_MAX: usize = 2 * TEXT_MAX + 36;
const FRAME_ROW_MAX: usize = 3 * IPMB_MAX_LEN + 162;
const ROW_MAX: usize = if TEXT_ROW_MAX > FRAME_ROW_MAX {
    TEXT_ROW_MAX
} else {
    FRAME_ROW_MAX
};
const _: () = assert!(
    ROW_MAX < ipmc_defs::IPMC_UART_TX_BUF,
    "IPMC_UART_TX_BUF must be larger than the longest dump row (ROW_MAX in debug_cmds.rs)"
);

/// DumpJob -- a DEBUG DUMP in progress. The console holds it and continues it on every
/// `Console::poll` pass (`step`): whole rows while the console TX ring has room for the longest
/// one, so the main loop never waits for the wire. It dumps the records held when it started.
pub struct DumpJob {
    format: DumpFormat,
    cursor: DumpCursor,
}

impl DumpJob {
    /// step -- prints rows while the TX ring holds the longest row; true when the dump is over
    /// (the end marker is printed).
    pub fn step(&mut self, link: &IpmbLink) -> bool {
        let names = link.names();
        let log = link.capture();
        let mut scratch = [0u8; PAYLOAD_MAX];
        while uart::tx_free(Port::Console) >= ROW_MAX {
            match self.cursor.next(log) {
                Step::Row(index) => {
                    if let Some(record) = log.get(index, &mut scratch) {
                        match self.format {
                            DumpFormat::Csv => out!("{}\n", CsvRow(&record, &names)),
                            DumpFormat::Trace => out!("{}\n", TraceRow(&record, &names)),
                        }
                    }
                }
                Step::Overwritten(count) => {
                    out!("# {} records overwritten during the dump\n", count);
                }
                Step::Done => {
                    out!("# end of capture\n");
                    return true;
                }
            }
        }
        false
    }
}

/// cmd_debug_dump -- DEBUG DUMP {FORMAT}: prints the header line and starts the dump job; the
/// console prints the rows and the end line on the following main-loop passes (`DumpJob`).
pub fn cmd_debug_dump(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let format = match optional(args.text_at(0))? {
        None => DumpFormat::Csv,
        Some(word) if word.eq_ignore_ascii_case("CSV") => DumpFormat::Csv,
        Some(word) if word.eq_ignore_ascii_case("TRACE") => DumpFormat::Trace,
        Some(_) => return Err(reported(format_args!("format must be CSV or TRACE"))),
    };
    let link = &*ctx.ipmb;
    let stats = link.capture().stats();
    out!(
        "# IPMC capture: {} v{}, ipmc-core v{}, IPMB address {:02X}h, Tier {}, uptime {} ms, {} records, {} overwritten\n",
        ipmc_defs::IPMC_FW_NAME,
        env!("CARGO_PKG_VERSION"),
        ipmc_core::CORE_VERSION,
        link.dispatch().addr.ipmb_address,
        ipmc_defs::IPMC_TIER,
        systick::millis(),
        stats.records,
        stats.overwritten
    );
    if matches!(format, DumpFormat::Csv) {
        out!("{}\n", CsvHeader);
    }
    ctx.dump = Some(DumpJob {
        format,
        cursor: DumpCursor::start(link.capture()),
    });
    Ok(())
}

/// cmd_debug_clear -- DEBUG CLEAR.
pub fn cmd_debug_clear(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    ctx.ipmb.capture_mut().clear();
    out!("OK: capture log cleared\n");
    Ok(())
}

// ---- trace switches ----

/// cmd_debug_trace -- DEBUG TRACE {WHAT} {STATE}: IPMI (the request, link and event lines on
/// the console; the log keeps them either way) or I2C (one console line per frame).
pub fn cmd_debug_trace(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let Some(what) = optional(args.text_at(0))? else {
        print_switches(ctx.ipmb);
        return Ok(());
    };
    let state = optional(args.on_off_at(1))?;
    if what.eq_ignore_ascii_case("IPMI") {
        if let Some(on) = state {
            ctx.ipmb.set_ipmi_console(on);
        }
    } else if what.eq_ignore_ascii_case("I2C") {
        if let Some(on) = state {
            ctx.ipmb.set_i2c_trace(on);
        }
    } else {
        return Err(reported(format_args!("trace is IPMI or I2C")));
    }
    print_switches(ctx.ipmb);
    Ok(())
}

// ---- local requests ----

/// cmd_debug_events -- DEBUG EVENTS {STATE}: a local Set Event Receiver; no argument prints the
/// Event Receiver.
pub fn cmd_debug_events(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let Some(on) = optional(args.on_off_at(0))? else {
        let receiver = receiver_state(ctx.ipmb);
        out!("Event receiver: {}\n", receiver);
        return Ok(());
    };
    match set_events(ctx.ipmb, on) {
        EventsResult::Set if on => {
            out!("OK: events on (receiver {CM_ADDR8:02X}h LUN 0)\n");
            Ok(())
        }
        EventsResult::Set => {
            out!("OK: events off\n");
            Ok(())
        }
        EventsResult::NoEvents => {
            out!("this build sends no event messages\n");
            Ok(())
        }
        EventsResult::Refused(code) => Err(reported(format_args!(
            "Set Event Receiver answered CC {code:02X}h"
        ))),
        EventsResult::Failed(error) => Err(reported(format_args!(
            "Set Event Receiver did not run: {}",
            error.name()
        ))),
    }
}

/// cmd_debug_ipmi -- DEBUG IPMI [NETFN] [CMD] {DATA...}: one request through the dispatcher as
/// if the Chassis Manager had sent it; nothing goes on the bus. The request and the response go
/// into the capture log as text lines.
pub fn cmd_debug_ipmi(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let netfn = hex_word(args.text_at(0)?)?;
    let cmd = hex_word(args.text_at(1)?)?;
    let mut buf = [0u8; IPMB_MAX_LEN];
    let count = hex_words(args, 2, &mut buf)?;
    let data = buf.get(..count).unwrap_or(&[]);
    ctx.ipmb.log_text(format_args!(
        "console request NetFn {netfn:02X}h cmd {cmd:02X}h data {}",
        HexBytes(data)
    ));
    let reply = local(ctx.ipmb, netfn, cmd, data).map_err(|error| {
        reported(format_args!(
            "the request cannot run ({}): NetFn must be even and at most 3Eh",
            error.name()
        ))
    })?;
    let (code, response) = (reply.completion_code(), reply.data());
    ctx.ipmb.log_text(format_args!(
        "console response CC {code:02X}h, {} bytes: {}",
        response.len(),
        HexBytes(response)
    ));
    let names = ctx.ipmb.names();
    out!(
        "-> CC {code:02X}h, {} bytes: {} ({} {})\n",
        response.len(),
        HexBytes(response),
        NetFnName(netfn),
        CommandName {
            names: &names,
            netfn,
            cmd
        }
    );
    Ok(())
}

/// cmd_debug_inject -- DEBUG INJECT [BUS] {BYTES...}: a whole IPMB frame handed to the link as
/// if the bus had received it. A request the link accepts is answered on the bus as any other.
pub fn cmd_debug_inject(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let bus_word = args.text_at(0)?;
    let bus = if bus_word.eq_ignore_ascii_case("A") {
        Bus::A
    } else if bus_word.eq_ignore_ascii_case("B") {
        Bus::B
    } else {
        return Err(reported(format_args!("bus is A or B")));
    };
    let mut buf = [0u8; IPMB_MAX_LEN];
    let count = hex_words(args, 1, &mut buf)?;
    if count == 0 {
        return Err(CmdError::Arg(ipmc_menu::ArgError::Missing));
    }
    let frame = buf.get(..count).unwrap_or(&[]);
    match ctx.ipmb.inject(bus, frame) {
        Ok(()) => out!(
            "OK: {count} bytes handed to the link as received on IPMB-{}\n",
            bus.name()
        ),
        Err(reason) => out!("Dropped by the link: {}\n", reason.name()),
    }
    Ok(())
}

/// CACHE_ROW -- one DEBUG CACHE row, worst case, CR LF counted as 2: `  rqSA FFh seq FFh ` (19),
/// the NetFn name (9), a blank, the command name (the command tables' names are at most 29 bytes
/// today; 48 allowed), `: 255 bytes, age 4294967295 ms (expired)` (42) and the line end = 119,
/// rounded up to 128.
const CACHE_ROW: usize = 128;

/// CACHE_ROWS_MAX -- rows DEBUG CACHE prints: what the console TX ring holds after 256 bytes for
/// the header line, the "more" line and what the ring still holds ((4095 - 256) / 128 = 29 with the
/// default ring). The cache holds up to IPMB_DUP_CACHE (64) entries, written by whatever the bus
/// sends; the others are counted in the last line.
const CACHE_ROWS_MAX: usize = uart::CONSOLE_TX_USABLE.saturating_sub(256) / CACHE_ROW;
const _: () = assert!(
    CACHE_ROWS_MAX >= 1,
    "IPMC_UART_TX_BUF is too small for DEBUG CACHE (debug_cmds.rs CACHE_ROW)"
);

/// cmd_debug_cache -- DEBUG CACHE {ACTION}: the duplicate cache entries (at most
/// `CACHE_ROWS_MAX`, then a line that counts the rest), or CLEAR.
pub fn cmd_debug_cache(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    match optional(args.text_at(0))? {
        None => {
            let link = &*ctx.ipmb;
            let names = link.names();
            out!(
                "Duplicate cache: {} entries\n",
                link.link().duplicate_cache_len()
            );
            let mut left = 0;
            for (index, e) in link
                .link()
                .duplicate_cache_entries(systick::millis())
                .enumerate()
            {
                if index >= CACHE_ROWS_MAX {
                    left += 1;
                    continue;
                }
                out!(
                    "  rqSA {:02X}h seq {:02X}h {} {}: {} bytes, age {} ms{}\n",
                    e.rq_sa,
                    e.rq_seq,
                    NetFnName(e.netfn),
                    CommandName {
                        names: &names,
                        netfn: e.netfn,
                        cmd: e.cmd
                    },
                    e.response_len,
                    e.age_ms,
                    if e.live { "" } else { " (expired)" }
                );
            }
            if left > 0 {
                out!("  ... {} more entries\n", left);
            }
            Ok(())
        }
        Some(word) if word.eq_ignore_ascii_case("CLEAR") => {
            ctx.ipmb.clear_duplicate_cache();
            out!("OK: duplicate cache cleared\n");
            Ok(())
        }
        Some(_) => Err(reported(format_args!("action is CLEAR"))),
    }
}
