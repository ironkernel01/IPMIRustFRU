// debug_cmds.rs - the DEBUG console group: STATUS, EVENTS, IPMI, INJECT, CACHE, BOOTLOADER and
// WATCHDOG (local IPMI requests, the duplicate cache, the restart into the bootloader and the
// watchdog tests).
//
// The state lives in ipmc_runtime::link (the link and the dispatch context); these handlers switch it,
// print it and run requests through it. The logging handlers (LOG, DUMP, CLEAR, TRACE) are in
// log_cmds.rs and the ipmc-log crate; STATUS prints the log lines through ipmc-log. Nothing here
// reads an IPMB byte layout of its own.
// The words typed for IPMI and INJECT are hex bytes, parsed by ipmc_wire::hex_byte.
//
// Nothing here runs in an interrupt handler.

use core::fmt;

use ipmc_boot::request::Interface;
use ipmc_core::dispatch::cc;
use ipmc_core::ipmb::{IPMB_MAX_LEN, NETFN_SE};
use ipmc_core::link::{LocalError, CM_ADDR8};
use ipmc_core::names::{CommandName, HexBytes, NetFnName};
use ipmc_core::queue::Bus;
use ipmc_log::{LogLine, SwitchLines};
use ipmc_menu::{eq_ignore_case, optional, Args, CmdError, CmdResult};

use ipmc_hal::board::Board;
use ipmc_hal::serial::{SerialPort, CONSOLE_TX_USABLE};
use ipmc_hal::time::Clock;

use crate::console::{print_watchdog, Context, LinkHome};
use crate::link::IpmbLink;
use crate::out;
use crate::text::ConsoleText;

/// CMD_SET_EVENT_RECEIVER -- Set Event Receiver, S/E NetFn (IPMI v2.0 section 29.1).
pub(crate) const CMD_SET_EVENT_RECEIVER: u8 = 0x00;
/// CMD_GET_EVENT_RECEIVER -- Get Event Receiver, S/E NetFn (IPMI v2.0 section 29.2).
const CMD_GET_EVENT_RECEIVER: u8 = 0x01;
/// RECEIVER_OFF -- the Event Receiver address that disables event messages (IPMI v2.0 29.1).
pub(crate) const RECEIVER_OFF: u8 = 0xFF;
/// RECEIVER_LUN_MASK -- the Event Receiver LUN is bits 1:0 of the second data byte.
const RECEIVER_LUN_MASK: u8 = 0x03;

/// reported -- prints one error line and stops the handler.
pub(crate) fn reported(con: &mut impl ConsoleText, message: fmt::Arguments) -> CmdError {
    out!(con, "Error: {}\n", message);
    CmdError::Reported
}

// ---- hex words ----

/// hex_word -- one typed byte: `06`, `0x06` and `6` are all 06h.
fn hex_word(con: &mut impl ConsoleText, text: &str) -> Result<u8, CmdError> {
    ipmc_wire::hex_word(text)
        .ok_or_else(|| reported(con, format_args!("'{text}' is not a hex byte")))
}

/// hex_words -- the words of `args` from index `first` on as bytes in `buf`; the count.
fn hex_words(
    con: &mut impl ConsoleText,
    args: &Args<'_>,
    first: usize,
    buf: &mut [u8; IPMB_MAX_LEN],
) -> Result<usize, CmdError> {
    let mut count = 0;
    while let Some(word) = args.get(first + count) {
        let slot = buf
            .get_mut(count)
            .ok_or_else(|| reported(con, format_args!("more than {IPMB_MAX_LEN} bytes")))?;
        *slot = hex_word(con, word)?;
        count += 1;
    }
    Ok(count)
}

// ---- local requests ----

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
fn receiver_state<B: Board>(link: &mut IpmbLink<B>) -> ReceiverState {
    let Ok(reply) = link.local(NETFN_SE, CMD_GET_EVENT_RECEIVER, &[]) else {
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
/// the start-up with events off.
pub(crate) fn set_events<B: Board>(link: &mut IpmbLink<B>, on: bool) -> EventsResult {
    let receiver = if on { CM_ADDR8 } else { RECEIVER_OFF };
    match link.local(NETFN_SE, CMD_SET_EVENT_RECEIVER, &[receiver, 0]) {
        Err(error) => EventsResult::Failed(error),
        Ok(reply) => match reply.completion_code() {
            cc::OK => EventsResult::Set,
            cc::INVALID_CMD => EventsResult::NoEvents,
            code => EventsResult::Refused(code),
        },
    }
}

/// events_off_at_boot -- the start-up with `events_at_boot` false: sends the IPMC itself Set
/// Event Receiver FFh and says so, unless the build has no event messages (C1h). With
/// `events_at_boot` true it sends nothing.
pub(crate) fn events_off_at_boot<B: Board>(
    events_at_boot: bool,
    link: &mut IpmbLink<B>,
    con: &mut impl ConsoleText,
) {
    if events_at_boot {
        return;
    }
    match set_events(link, false) {
        EventsResult::NoEvents => {}
        EventsResult::Set => {
            out!(
                con,
                "Events: off from start-up (defs.rs IPMC_EVENTS_AT_BOOT false)\n"
            );
        }
        EventsResult::Refused(code) => {
            out!(
                con,
                "Events: start-up Set Event Receiver FFh answered CC {code:02X}h\n"
            );
        }
        EventsResult::Failed(error) => {
            out!(
                con,
                "Events: start-up Set Event Receiver FFh failed: {}\n",
                error.name()
            );
        }
    }
}

// ---- status ----

/// cmd_debug_status -- DEBUG STATUS.
pub fn cmd_debug_status<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    out!(
        con,
        "{}{}",
        LogLine(ctx.ipmb.log()),
        SwitchLines(ctx.ipmb.log())
    );
    let receiver = receiver_state(&mut *ctx.ipmb);
    out!(con, "Event receiver:  {}\n", receiver);
    out!(
        con,
        "Duplicate cache: {} entries\n",
        ctx.ipmb.link().duplicate_cache_len()
    );
    Ok(())
}

// ---- local requests ----

/// cmd_debug_events -- DEBUG EVENTS {STATE}: a local Set Event Receiver; no argument prints the
/// Event Receiver.
pub fn cmd_debug_events<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let Some(on) = optional(args.on_off_at(0))? else {
        let receiver = receiver_state(&mut *ctx.ipmb);
        out!(con, "Event receiver: {}\n", receiver);
        return Ok(());
    };
    match set_events(&mut *ctx.ipmb, on) {
        EventsResult::Set if on => {
            out!(con, "OK: events on (receiver {CM_ADDR8:02X}h LUN 0)\n");
            Ok(())
        }
        EventsResult::Set => {
            out!(con, "OK: events off\n");
            Ok(())
        }
        EventsResult::NoEvents => {
            out!(con, "this build sends no event messages\n");
            Ok(())
        }
        EventsResult::Refused(code) => Err(reported(
            con,
            format_args!("Set Event Receiver answered CC {code:02X}h"),
        )),
        EventsResult::Failed(error) => Err(reported(
            con,
            format_args!("Set Event Receiver did not run: {}", error.name()),
        )),
    }
}

/// cmd_debug_ipmi -- DEBUG IPMI [NETFN] [CMD] {DATA...}: one request through the dispatcher as
/// if the Chassis Manager had sent it; nothing goes on the bus. The request and the response go
/// into the capture log as text lines.
pub fn cmd_debug_ipmi<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let netfn = hex_word(con, args.text_at(0)?)?;
    let cmd = hex_word(con, args.text_at(1)?)?;
    let mut buf = [0u8; IPMB_MAX_LEN];
    let count = hex_words(con, args, 2, &mut buf)?;
    let data = buf.get(..count).unwrap_or(&[]);
    ctx.ipmb.log_text(format_args!(
        "console request NetFn {netfn:02X}h cmd {cmd:02X}h data {}",
        HexBytes(data)
    ));
    let reply = ctx.ipmb.local(netfn, cmd, data).map_err(|error| {
        reported(
            con,
            format_args!(
                "the request cannot run ({}): NetFn must be even and at most 3Eh",
                error.name()
            ),
        )
    })?;
    let (code, response) = (reply.completion_code(), reply.data());
    ctx.ipmb.log_text(format_args!(
        "console response CC {code:02X}h, {} bytes: {}",
        response.len(),
        HexBytes(response)
    ));
    let names = ctx.ipmb.names();
    out!(
        con,
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
pub fn cmd_debug_inject<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let bus_word = args.text_at(0)?;
    let bus = if bus_word.eq_ignore_ascii_case("A") {
        Bus::A
    } else if bus_word.eq_ignore_ascii_case("B") {
        Bus::B
    } else {
        return Err(reported(con, format_args!("bus is A or B")));
    };
    let mut buf = [0u8; IPMB_MAX_LEN];
    let count = hex_words(con, args, 1, &mut buf)?;
    if count == 0 {
        return Err(CmdError::Arg(ipmc_menu::ArgError::Missing));
    }
    let frame = buf.get(..count).unwrap_or(&[]);
    match ctx.ipmb.inject(bus, frame) {
        Ok(()) => out!(
            con,
            "OK: {count} bytes handed to the link as received on IPMB-{}\n",
            bus.name()
        ),
        Err(reason) => out!(con, "Dropped by the link: {}\n", reason.name()),
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
const CACHE_ROWS_MAX: usize = CONSOLE_TX_USABLE.saturating_sub(256) / CACHE_ROW;
const _: () = assert!(
    CACHE_ROWS_MAX >= 1,
    "IPMC_UART_TX_BUF is too small for DEBUG CACHE (debug_cmds.rs CACHE_ROW)"
);

/// cmd_debug_cache -- DEBUG CACHE {ACTION}: the duplicate cache entries (at most
/// `CACHE_ROWS_MAX`, then a line that counts the rest), or CLEAR.
pub fn cmd_debug_cache<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    match optional(args.text_at(0))? {
        None => {
            let link = &*ctx.ipmb;
            let names = link.names();
            out!(
                con,
                "Duplicate cache: {} entries\n",
                link.link().duplicate_cache_len()
            );
            let mut left = 0;
            for (index, e) in link
                .link()
                .duplicate_cache_entries(ctx.clock.millis())
                .enumerate()
            {
                if index >= CACHE_ROWS_MAX {
                    left += 1;
                    continue;
                }
                out!(
                    con,
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
                out!(con, "  ... {} more entries\n", left);
            }
            Ok(())
        }
        Some(word) if word.eq_ignore_ascii_case("CLEAR") => {
            ctx.ipmb.clear_duplicate_cache();
            out!(con, "OK: duplicate cache cleared\n");
            Ok(())
        }
        Some(_) => Err(reported(con, format_args!("action is CLEAR"))),
    }
}

// ---- BOOTLOADER ----

/// cmd_debug_bootloader -- `DEBUG BOOTLOADER {CONFIRM}` restarts the processor into the bootloader
/// (local_only/Documentation/ipmc_boot_plan.md 7.1). While PWR_EN is asserted the payload loses power at the
/// reset (the reset releases PWR_EN), so the command only warns and waits for `CONFIRM` (any
/// letter case); with power off, or with `CONFIRM`, it prints the restart line and restarts. The
/// reset is the one Cold Reset path (`IpmbLink::request_bootloader`): the console is flushed, an
/// accepted FRU write is saved first, and the request words are written just before the restart.
pub fn cmd_debug_bootloader<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let confirmed = match optional(args.text_at(0))? {
        None => false,
        Some(word) if eq_ignore_case(word, "CONFIRM") => true,
        Some(_) => return Err(reported(con, format_args!("argument is CONFIRM"))),
    };
    let power_on = ctx.sensing.payload_power_on();
    if power_on && !confirmed {
        out!(
            con,
            "Payload power is on and drops at the reset into the bootloader
Send DEBUG BOOTLOADER CONFIRM to restart
"
        );
        return Ok(());
    }
    out!(
        con,
        "Restarting into the bootloader
"
    );
    if power_on {
        out!(
            con,
            "Payload power drops at the reset
"
        );
    }
    ctx.ipmb.request_bootloader(Interface::Console);
    Ok(())
}

// ---- WATCHDOG ----

/// cmd_debug_watchdog -- `DEBUG WATCHDOG {HANG|PANIC} {CONFIRM}`. No argument prints the
/// `Watchdog:` line (as `SYS STATUS`). `HANG CONFIRM` spins with interrupts on and feeds nothing,
/// and `PANIC CONFIRM` panics; the watchdog resets the processor in both, and the next start
/// prints the reset cause (and the kept fault for a panic). Without `CONFIRM` the command only
/// says what it does. A watchdog that is not running refuses both tests (a hang would never end).
/// Argument words are any letter case.
pub fn cmd_debug_watchdog<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let Some(test) = optional(args.text_at(0))? else {
        print_watchdog(con, ctx.watchdog_ms);
        return Ok(());
    };
    let (hang, what) = if eq_ignore_case(test, "HANG") {
        (true, "hangs the main loop")
    } else if eq_ignore_case(test, "PANIC") {
        (false, "panics")
    } else {
        return Err(reported(con, format_args!("test is HANG or PANIC")));
    };
    let confirmed = match optional(args.text_at(1))? {
        None => false,
        Some(word) if eq_ignore_case(word, "CONFIRM") => true,
        Some(_) => return Err(reported(con, format_args!("argument is CONFIRM"))),
    };
    let Some(ms) = ctx.watchdog_ms else {
        return Err(reported(con, format_args!("the watchdog is not running")));
    };
    let name = if hang { "HANG" } else { "PANIC" };
    if !confirmed {
        out!(
            con,
            "DEBUG WATCHDOG {} {}, then the watchdog resets the board\n",
            name,
            what
        );
        out!(con, "Send DEBUG WATCHDOG {} CONFIRM to run it\n", name);
        return Ok(());
    }
    if hang {
        out!(
            con,
            "Hanging: the watchdog resets the board within {} ms\n",
            ms
        );
        con.flush();
        loop {
            core::hint::spin_loop();
        }
    }
    out!(
        con,
        "Panicking: the watchdog resets the board within {} ms\n",
        ms
    );
    con.flush();
    panic!("DEBUG WATCHDOG PANIC")
}
