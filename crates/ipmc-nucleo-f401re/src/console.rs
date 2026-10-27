// console.rs - UART command line: the firmware's command tables and handlers on the ipmc-menu engine.
//
// The engine (crates/ipmc-menu) owns line editing, matching, help and the prompt; this file owns
// the command tree and the handlers. Each handler calls the module that owns the behaviour
// (bsp::led, bsp::uart, ipmc_core::addr).
// Command tree (documented in crates/ipmc-menu/MenuDocumentation.md):
//   SYS  : STATUS, RESET, VERSION      LED : ON, OFF, BLINK [HZ], AUTO, STATUS
//   IPMB : ADDR, STATS, STATE
//   SENSORS : SHOW, SDR, TRACE {OFF|EVENTS|ALL}     POWER : ON, OFF, CLEAR, STATUS
//   FRU  : SHOW, DUMP {OFFSET} {COUNT}, STORE, FIELDS, GET [FIELD], and with IPMC_FRU_CONSOLE_WRITE true
//          SET [FIELD] {VALUE...}, RESTORE
//   SIM  : RAIL {N} {MV}, TEMP {DC}, FAULT {NAME} {ON|OFF}, SHOW
//   DEBUG: STATUS, LOG {ON|OFF}, DUMP {CSV|TRACE}, CLEAR, TRACE {IPMI|I2C} {ON|OFF},
//          EVENTS {ON|OFF}, IPMI [NETFN] [CMD] {DATA...}, INJECT [BUS] {BYTES...}, CACHE {CLEAR},
//          BOOTLOADER
//          WATCHDOG
//   top level: STATUS (= SYS STATUS), RESET (= SYS RESET)
// The SENSORS (SHOW, TRACE), POWER and SIM handlers live in sensing.rs, which owns the behaviour; the
// IPMB STATS and IPMB STATE handlers live in ipmb_link.rs; the FRU handlers (SHOW, DUMP, STORE,
// FIELDS, GET, SET, RESTORE) and SENSORS SDR live in data_cmds.rs; the DEBUG handlers live in debug_cmds.rs (STATUS, EVENTS, IPMI, INJECT, CACHE, BOOTLOADER, WATCHDOG) and log_cmds.rs (LOG, DUMP, CLEAR, TRACE).

use core::fmt;

use ipmc_core::addr::{self, AddrInfo};
use ipmc_core::trace::Trace;
use ipmc_menu::{
    report_line_error, Args, CmdError, CmdResult, Command, EditEvent, Group, LineEditor, Menu,
    MenuError, MenuOutput,
};

use crate::app_port::{self, AppPort};
use crate::bsp::clock::{ClockFault, ClockInfo};
use crate::bsp::fault_record::Fault;
use crate::bsp::led::{self, Shown};
use crate::bsp::reset_cause::ResetCause;
use crate::bsp::uart::{self, Port, PortText};
use crate::bsp::{ga, systick};
use crate::data_cmds;
use crate::debug_cmds;
use crate::ipmb_link::{self, IpmbLink};
use crate::log_cmds::{self, ConsoleSink};
use crate::sensing::{self, Sensing};
use ipmc_log::DumpJob;

/// Context -- values captured at start-up that the banner and `SYS STATUS` report. This is the context
/// value every command handler receives.
pub struct Context {
    reset_cause: ResetCause,
    /// The fault the previous run kept (bsp/fault_record.rs), reported next to the reset cause.
    last_fault: Option<Fault>,
    /// The watchdog period set; None while the watchdog is not running (`print_watchdog`).
    pub(crate) watchdog_ms: Option<u32>,
    clock: ClockInfo,
    clock_fault: Option<ClockFault>,
    /// Sensor monitor and its backends; the SENSORS, POWER and SIM handlers use it.
    pub(crate) sensing: Sensing,
    /// IPMB link layer; the IPMB STATS and IPMB STATE handlers read it.
    pub(crate) ipmb: &'static mut IpmbLink,
    /// The DEBUG DUMP in progress, continued by `Console::poll` (log_cmds.rs).
    pub(crate) dump: Option<DumpJob>,
    /// The application port: its line assembler, polled by `Console::service_app`.
    app: AppPort,
}

/// ConsoleTrace -- console sink of the trace hook, shared by the sensors and the IPMB link:
/// one line per call, written whole into the console TX ring when it has room and dropped (and
/// counted) otherwise, so a trace line never holds the main loop (`uart::print_line_or_drop`).
pub(crate) struct ConsoleTrace;

impl Trace for ConsoleTrace {
    fn line(&mut self, args: fmt::Arguments) {
        uart::print_line_or_drop(Port::Console, args);
    }
}

/// Menu output on the console UART. `\n` becomes CR LF inside `uart::print`.
struct UartOut;

impl MenuOutput for UartOut {
    fn write_str(&mut self, s: &str) {
        out!("{}", s);
    }
}

/// Centihertz in one hertz.
const CHZ_PER_HZ: u32 = 100;

type ConsoleCommand = Command<Context>;

// ---- command tables (MenuBuilder FUNCTION:COMMAND:DESCRIPTION) ----

/// Top-level commands: aliases that work without entering a group.
static TOP_COMMANDS: [ConsoleCommand; 2] = [
    Command {
        syntax: "STATUS",
        help: "Alias of SYS STATUS",
        handler: cmd_status,
    },
    Command {
        syntax: "RESET",
        help: "Alias of SYS RESET",
        handler: cmd_reset,
    },
];

static SYS_COMMANDS: [ConsoleCommand; 3] = [
    Command {
        syntax: "STATUS",
        help: "Uptime, clocks, geographic address, LED, last reset cause",
        handler: cmd_status,
    },
    Command {
        syntax: "RESET",
        help: "System reset",
        handler: cmd_reset,
    },
    Command {
        syntax: "VERSION",
        help: "Firmware and core library versions",
        handler: cmd_version,
    },
];

static LED_COMMANDS: [ConsoleCommand; 5] = [
    Command {
        syntax: "ON",
        help: "LD2 on",
        handler: cmd_led_on,
    },
    Command {
        syntax: "OFF",
        help: "LD2 off",
        handler: cmd_led_off,
    },
    Command {
        syntax: "BLINK [HZ]",
        help: "LD2 blink at the given rate in Hz; the limits print with LED STATUS",
        handler: cmd_led_blink,
    },
    Command {
        syntax: "AUTO",
        help: "LD2 back to the heartbeat / IPMB activity pattern",
        handler: cmd_led_auto,
    },
    Command {
        syntax: "STATUS",
        help: "LD2 mode, rate and the accepted blink rate limits",
        handler: cmd_led_status,
    },
];

static IPMB_COMMANDS: [ConsoleCommand; 3] = [
    Command {
        syntax: "ADDR",
        help: "Geographic address, IPMB address, parity and NVMRO with their sources",
        handler: cmd_ipmb_addr,
    },
    Command {
        syntax: "STATS",
        help: "Per-bus receive, error, transmit, resend and replay counters",
        handler: ipmb_link::cmd_ipmb_stats,
    },
    Command {
        syntax: "STATE",
        help:
            "Per-bus fitted, mode and enable state, the transmit policy and the link sensor bytes",
        handler: ipmb_link::cmd_ipmb_state,
    },
];

static SENSORS_COMMANDS: [ConsoleCommand; 3] = [
    Command {
        syntax: "SHOW",
        help: "Rails, die temperature, inputs, power state, FRU state, Tier-1 readings and their data bytes",
        handler: sensing::cmd_sensors_show,
    },
    Command {
        syntax: "SDR",
        help: "The Device SDR records: sensor number, type, event/reading type, ID string",
        handler: data_cmds::cmd_sensors_sdr,
    },
    Command {
        syntax: "TRACE {LEVEL}",
        help: "Sensor trace level OFF, EVENTS (changes) or ALL (changes and samples)",
        handler: sensing::cmd_sensors_trace,
    },
];

static POWER_COMMANDS: [ConsoleCommand; 4] = [
    Command {
        syntax: "ON",
        help: "Assert PWR_EN and wait for PGOOD",
        handler: sensing::cmd_power_on,
    },
    Command {
        syntax: "OFF",
        help: "Release PWR_EN and clear a latched fault",
        handler: sensing::cmd_power_off,
    },
    Command {
        syntax: "CLEAR",
        help: "Clear a latched power fault",
        handler: sensing::cmd_power_clear,
    },
    Command {
        syntax: "STATUS",
        help: "Power state, PWR_EN, latched fault",
        handler: sensing::cmd_power_status,
    },
];

static SIM_COMMANDS: [ConsoleCommand; 4] = [
    Command {
        syntax: "RAIL {N} {MV}",
        help: "Simulated rail N (0 = VDDA) in millivolts; no argument lists them",
        handler: sensing::cmd_sim_rail,
    },
    Command {
        syntax: "TEMP {DC}",
        help: "Simulated die temperature in tenths of a degree C",
        handler: sensing::cmd_sim_temp,
    },
    Command {
        syntax: "FAULT {NAME} {STATE}",
        help: "Assert (ON) or clear (OFF) a simulated input; no argument lists them",
        handler: sensing::cmd_sim_fault,
    },
    Command {
        syntax: "SHOW",
        help: "Source of every group and the simulated values",
        handler: sensing::cmd_sim_show,
    },
];

static FRU_COMMANDS: [ConsoleCommand; 7] = [
    Command {
        syntax: "SHOW",
        help: "FRU image decoded: common header, board and product areas, checksums",
        handler: data_cmds::cmd_fru_show,
    },
    Command {
        syntax: "DUMP {OFFSET} {COUNT}",
        help: "Hex dump of the FRU image; default offset 0 and the rest of the area",
        handler: data_cmds::cmd_fru_dump,
    },
    Command {
        syntax: "STORE",
        help: "Flash log state: records, newest sequence, free bytes, pending write, next erase",
        handler: data_cmds::cmd_fru_store,
    },
    Command {
        syntax: "FIELDS",
        help: "Every FRU field with its maximum length and its current value",
        handler: data_cmds::cmd_fru_fields,
    },
    Command {
        syntax: "GET [FIELD]",
        help: "Current value of one FRU field; FRU FIELDS lists the names",
        handler: data_cmds::cmd_fru_get,
    },
    // The rows below this line are the write rows: they are in the FRU group only when
    // IPMC_FRU_CONSOLE_WRITE is true (`FRU_TABLE`).
    Command {
        syntax: "SET [FIELD] {VALUE...}",
        help: "Write a FRU field (no value clears a text field or the date; MFG_DATE takes YYYY-MM-DD HH:MM); saved to flash after the save delay; refused while NVMRO is asserted",
        handler: data_cmds::cmd_fru_set,
    },
    Command {
        syntax: "RESTORE",
        help:
            "Save the built FRU image as a new log record and put it back; refused while NVMRO is asserted",
        handler: data_cmds::cmd_fru_restore,
    },
];

/// FRU_READ_ROWS -- the rows of `FRU_COMMANDS` that only read (SHOW, DUMP, STORE, FIELDS, GET);
/// the write rows (SET, RESTORE) follow them.
const FRU_READ_ROWS: usize = 5;

/// FRU_TABLE -- the table of the FRU group, chosen at compile time from IPMC_FRU_CONSOLE_WRITE:
/// every row when it is 1, only the read rows when it is 0 (SET and RESTORE do not exist: HELP
/// does not list them and a typed SET is an unknown command).
static FRU_TABLE: &[ConsoleCommand] = if ipmc_defs::IPMC_FRU_CONSOLE_WRITE {
    &FRU_COMMANDS
} else {
    FRU_COMMANDS.split_at(FRU_READ_ROWS).0
};

static DEBUG_COMMANDS: [ConsoleCommand; 11] = [
    Command {
        syntax: "STATUS",
        help: "Debug switches, capture log use, event receiver, duplicate cache",
        handler: debug_cmds::cmd_debug_status,
    },
    Command {
        syntax: "LOG {STATE}",
        help: "Capture log ON or OFF; no argument shows its state",
        handler: log_cmds::cmd_debug_log,
    },
    Command {
        syntax: "DUMP {FORMAT}",
        help: "Print the capture log: CSV (default) or TRACE; any key stops it",
        handler: log_cmds::cmd_debug_dump,
    },
    Command {
        syntax: "CLEAR",
        help: "Empty the capture log",
        handler: log_cmds::cmd_debug_clear,
    },
    Command {
        syntax: "TRACE {WHAT} {STATE}",
        help: "Console trace: IPMI (request, link and event lines) or I2C (one line per frame), ON or OFF",
        handler: log_cmds::cmd_debug_trace,
    },
    Command {
        syntax: "EVENTS {STATE}",
        help: "Event messages ON (receiver 20h) or OFF (receiver FFh)",
        handler: debug_cmds::cmd_debug_events,
    },
    Command {
        syntax: "IPMI [NETFN] [CMD] {DATA...}",
        help: "Run one IPMI request locally, as from 20h; nothing goes on the bus",
        handler: debug_cmds::cmd_debug_ipmi,
    },
    Command {
        syntax: "INJECT [BUS] {BYTES...}",
        help: "Hand a whole IPMB frame to the link as if received on bus A or B",
        handler: debug_cmds::cmd_debug_inject,
    },
    Command {
        syntax: "CACHE {ACTION}",
        help: "Duplicate cache: no argument shows the entries, CLEAR empties it",
        handler: debug_cmds::cmd_debug_cache,
    },
    Command {
        syntax: "BOOTLOADER {CONFIRM}",
        help: "Restart into the bootloader; CONFIRM needed while payload power is on",
        handler: debug_cmds::cmd_debug_bootloader,
    },
    Command {
        syntax: "WATCHDOG {TEST} {CONFIRM}",
        help: "Watchdog state; HANG or PANIC with CONFIRM tests the reset",
        handler: debug_cmds::cmd_debug_watchdog,
    },
];

static GROUPS: [Group<Context>; 8] = [
    Group {
        name: "SYS",
        description: "System status, reset and version",
        commands: &SYS_COMMANDS,
    },
    Group {
        name: "LED",
        description: "LD2 control",
        commands: &LED_COMMANDS,
    },
    Group {
        name: "IPMB",
        description: "IPMB address and bus state",
        commands: &IPMB_COMMANDS,
    },
    Group {
        name: "SENSORS",
        description: "Voltage rails, die temperature, inputs and Tier-1 readings",
        commands: &SENSORS_COMMANDS,
    },
    Group {
        name: "POWER",
        description: "Payload power enable",
        commands: &POWER_COMMANDS,
    },
    Group {
        name: "SIM",
        description: "Simulated sensor inputs",
        commands: &SIM_COMMANDS,
    },
    Group {
        name: "FRU",
        description: "FRU image, its decoded areas, its fields and the flash log",
        commands: FRU_TABLE,
    },
    Group {
        name: "DEBUG",
        description: "Test switches, the IPMB capture log and local IPMI requests",
        commands: &DEBUG_COMMANDS,
    },
];

// A table fault is a build error, not a run-time surprise.
const _: () = assert!(
    Menu::validate_tables(&TOP_COMMANDS, &GROUPS).is_ok(),
    "console command tables are invalid"
);
// The FRU table is valid with every row, so it is valid with the read rows alone: a row taken
// away cannot create a duplicate or an empty group, whatever IPMC_FRU_CONSOLE_WRITE is.
const _: () = assert!(
    Menu::validate_tables(
        &TOP_COMMANDS,
        &[Group {
            name: "FRU",
            description: "",
            commands: &FRU_COMMANDS,
        }]
    )
    .is_ok(),
    "FRU command table is invalid"
);

// ---- the size of HELP ----
//
// A HELP listing is output too: it goes into the console TX ring whole (see data_cmds.rs, "how
// much a console command may print"), so the longest one must fit it. The listing prints for
// each entry two blanks, the syntax padded to the longest syntax of the listing, ` - `, the help
// sentence and CR LF; a group listing has the group's commands and the four built-in entries
// (HELP, ?, UP, ..), a title (at most 40 bytes) and a footer (at most 90); the root listing has
// every group (name and description), the top commands and the two built-in entries. The width
// used below is the longest syntax of any table or group name, which is at least what any one
// listing pads to.

/// help_width -- the longest syntax or group name of the tables.
const fn help_width(top: &[ConsoleCommand], groups: &[Group<Context>]) -> usize {
    let mut width = 4; // the built-in entries are at most "HELP" / ".." / "UP" / "?"
    let mut i = 0;
    while i < top.len() {
        width = max_of(width, top[i].syntax.len());
        i += 1;
    }
    let mut g = 0;
    while g < groups.len() {
        width = max_of(width, groups[g].name.len());
        let mut c = 0;
        while c < groups[g].commands.len() {
            width = max_of(width, groups[g].commands[c].syntax.len());
            c += 1;
        }
        g += 1;
    }
    width
}

/// max_of -- the larger of two sizes.
const fn max_of(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// help_rows -- the bytes of the rows of `commands` at `width`.
const fn help_rows(commands: &[ConsoleCommand], width: usize) -> usize {
    let (mut total, mut i) = (0, 0);
    while i < commands.len() {
        total += 2 + width + 3 + commands[i].help.len() + 2;
        i += 1;
    }
    total
}

/// help_worst -- the bytes of the longest HELP listing: a group's, or the root's.
const fn help_worst(top: &[ConsoleCommand], groups: &[Group<Context>]) -> usize {
    let width = help_width(top, groups);
    // Built-in entries: HELP "List commands", ? "Same as HELP", UP "Leave this group", .. "Same as
    // UP" (at most 16 bytes of text each).
    let builtin = 4 * (2 + width + 3 + 16 + 2);
    let mut worst = 40 + 90 + builtin + help_rows(top, width);
    let mut g = 0;
    while g < groups.len() {
        let group = help_rows(groups[g].commands, width) + 40 + 90 + builtin;
        worst = max_of(worst, group);
        g += 1;
    }
    let mut root = 40 + 90 + help_rows(top, width) + 2 * (2 + width + 3 + 16 + 2);
    g = 0;
    while g < groups.len() {
        root += 2 + width + 3 + groups[g].description.len() + 2;
        g += 1;
    }
    max_of(worst, root)
}

const _: () = assert!(
    help_worst(&TOP_COMMANDS, &GROUPS) + 256 <= uart::CONSOLE_TX_USABLE,
    "IPMC_UART_TX_BUF is too small for the longest HELP listing (console.rs help_worst)"
);

/// Console -- the console: line editor, menu and handler context.
pub struct Console {
    ctx: Context,
    menu: Menu<Context>,
    editor: LineEditor,
}

/// Centihertz printed as hertz: whole hertz alone ("50"), else two decimals ("0.01").
struct Hz(u32);

impl fmt::Display for Hz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (whole, frac) = (self.0 / CHZ_PER_HZ, self.0 % CHZ_PER_HZ);
        if frac == 0 {
            write!(f, "{whole}")
        } else {
            write!(f, "{whole}.{frac:02}")
        }
    }
}

/// Frequency in Hz printed as MHz with two decimals.
struct Mhz(u32);

impl fmt::Display for Mhz {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let centi = (self.0 + 5_000) / 10_000;
        write!(f, "{}.{:02} MHz", centi / 100, centi % 100)
    }
}

impl Console {
    /// new -- a console over the start-up values, the sensor backends and the IPMB link.
    pub fn new(
        reset_cause: ResetCause,
        last_fault: Option<Fault>,
        clock: ClockInfo,
        clock_fault: Option<ClockFault>,
        sensing: Sensing,
        ipmb: &'static mut IpmbLink,
    ) -> Self {
        Console {
            ctx: Context {
                reset_cause,
                last_fault,
                watchdog_ms: None,
                clock,
                clock_fault,
                sensing,
                ipmb,
                dump: None,
                app: AppPort::new(),
            },
            menu: Menu::new(ipmc_defs::IPMC_CONSOLE_PROMPT, &TOP_COMMANDS, &GROUPS),
            editor: LineEditor::new(),
        }
    }

    /// set_watchdog -- records the period the watchdog runs at (`SYS STATUS`, `DEBUG WATCHDOG`).
    pub fn set_watchdog(&mut self, period_ms: Option<u32>) {
        self.ctx.watchdog_ms = period_ms;
    }

    /// start_events -- applies the events-at-boot setting (defs.rs IPMC_EVENTS_AT_BOOT).
    pub fn start_events(&mut self) {
        debug_cmds::events_off_at_boot(self.ctx.ipmb);
    }

    /// start_sensors -- applies the power-at-boot setting (defs.rs IPMC_POWER_AT_BOOT).
    pub fn start_sensors(&mut self) {
        let power_on_check = self.ctx.ipmb.power_on_check();
        self.ctx.sensing.start(power_on_check);
    }

    /// service_sensors -- serves the payload power and payload reset requests of the command
    /// handlers (`Sensing::serve_hand_offs`), takes a sensor sample when the period has elapsed,
    /// and hands every new sample (also one a console command took) to the IPMI handlers and the
    /// FRU state machine.
    pub fn service_sensors(&mut self) {
        let hand_offs = self.ctx.ipmb.take_hand_offs();
        self.ctx.sensing.serve_hand_offs(hand_offs);
        self.ctx.sensing.service();
        if let Some(snapshot) = self.ctx.sensing.take_fresh() {
            self.ctx.ipmb.feed_sensors(&snapshot);
        }
    }

    /// service_app -- one main-loop pass of the application port (`AppPort::poll`): takes the
    /// received bytes and answers at most one request line.
    pub fn service_app(&mut self) {
        let power = self.ctx.sensing.payload_power_on();
        self.ctx.app.poll(self.ctx.ipmb, power);
    }

    /// service_link -- runs the IPMB link layer's main-loop step.
    pub fn service_link(&mut self) {
        self.ctx.ipmb.service();
    }

    /// service_fru_log -- the FRU save step alone, for the stopped evaluation image.
    #[cfg(ipmc_eval)]
    pub fn service_fru_log(&mut self) {
        self.ctx.ipmb.service_fru_log();
    }

    /// stop_ipmb_for_eval -- the evaluation build's end of period: both IPMB slaves off.
    #[cfg(ipmc_eval)]
    pub fn stop_ipmb_for_eval(&mut self) {
        self.ctx.ipmb.stop_for_eval();
    }

    /// banner -- prints the start-up banner (the first prompt follows from `prompt`).
    pub fn banner(&self) {
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        out!("\n");
        print_version();
        out!("Build:        {}\n", profile);
        #[cfg(ipmc_eval)]
        out!(
            "EVALUATION BUILD: the IPMB stops after {} minutes\n",
            ipmc_defs::IPMC_EVAL_MINUTES
        );
        print_reset(&self.ctx);
        print_clock(&self.ctx);
        print_ga(&self.ctx.ipmb.dispatch().addr);
        let [result, detail] = self.ctx.ipmb.dispatch().self_test.bytes();
        out!(
            "Self test:    {:02X}h {:02X}h ({})\n",
            result,
            detail,
            if self.ctx.ipmb.dispatch().self_test.passed() {
                "passed"
            } else {
                "failed"
            }
        );
        out!("{}\n", addr::bootloader_pin_text(ga::bootloader_pin_high()));
        print_app_port();
        out!("Type HELP or ? for commands.\n");
    }

    /// prompt -- prints the root prompt.
    pub fn prompt(&self) {
        self.menu.write_prompt(&mut UartOut);
    }

    /// poll -- consumes received bytes: the editor reports what to echo, the menu runs complete
    /// lines. One call handles at most IPMC_CONSOLE_BYTES_PER_POLL bytes and runs at most one
    /// command line (it returns right after one), so the main loop services sensor sampling
    /// (and, in Phase 6, the IPMB link) every iteration however much the console receives.
    /// Console output goes into the interrupt-driven TX ring (IPMC_UART_TX_BUF), so a command
    /// that prints a little does not hold the loop for the wire time (86.8 us per byte at 115200
    /// baud); output waits only while the ring is full, and a long listing (DEBUG DUMP) is a job
    /// that prints a few rows per pass (`poll_dump`) so the loop stays inside the 227 ms IPMB
    /// response deadline (VITA 46.11 Rule 4.1.1-11). Bytes that arrive meanwhile wait in the UART
    /// RX ring (IPMC_UART_RX_BUF).
    pub fn poll(&mut self) {
        if self.ctx.dump.is_some() {
            self.poll_dump();
            return;
        }
        for _ in 0..ipmc_defs::IPMC_CONSOLE_BYTES_PER_POLL {
            let Some(byte) = uart::getc(Port::Console) else {
                return;
            };
            match self.editor.push_byte(byte) {
                EditEvent::Echo(ch) => out!("{}", char::from(ch)),
                EditEvent::Backspace => out!("\u{8} \u{8}"), // erase the character on screen
                EditEvent::Line(line) => {
                    out!("\n");
                    self.menu.process_line(line, &mut self.ctx, &mut UartOut);
                    if self.ctx.dump.is_none() {
                        self.menu.write_prompt(&mut UartOut); // a dump prints it when it ends
                    }
                    return;
                }
                EditEvent::LineTooLong => {
                    out!("\n");
                    report_line_error(MenuError::LineTooLong, &mut UartOut);
                    self.menu.write_prompt(&mut UartOut);
                    return;
                }
                EditEvent::None => {}
            }
        }
    }
}

impl Console {
    /// poll_dump -- one main-loop pass of a running DEBUG DUMP. A received byte stops the dump
    /// (a line feed does not count: a terminal that ends Enter with CR LF sends it right after
    /// the CR that started the dump); otherwise `DumpJob::step` (ipmc-log) prints the rows the TX ring has room for. New command lines
    /// wait: the menu does not run while the job exists. The prompt follows the end of the job.
    fn poll_dump(&mut self) {
        let stopped = matches!(uart::getc(Port::Console), Some(byte) if byte != b'\n');
        if stopped {
            out!("# dump stopped\n");
        }
        let finished = match self.ctx.dump.as_mut() {
            Some(job) if !stopped => job.step(
                self.ctx.ipmb.log().capture(),
                &self.ctx.ipmb.names(),
                &mut ConsoleSink,
            ),
            _ => true,
        };
        if finished {
            self.ctx.dump = None;
            self.menu.write_prompt(&mut UartOut);
        }
    }
}

fn print_version() {
    out!(
        "{} v{} (ipmc-core v{})\n",
        ipmc_defs::IPMC_FW_NAME,
        env!("CARGO_PKG_VERSION"),
        ipmc_core::CORE_VERSION
    );
    // The image header in flash: version, length and the start of the digest.
    out!("Image:        ");
    crate::app_header::image_line(&mut PortText(Port::Console));
    out!("\n");
}

fn print_clock(ctx: &Context) {
    let c = &ctx.clock;
    out!("Clock source: {}\n", c.source.text());
    if let Some(fault) = ctx.clock_fault {
        out!("Clock fault:  {}\n", fault.text());
    }
    out!("SYSCLK:       {}\n", Mhz(c.sysclk_hz));
    out!("HCLK (AHB):   {}\n", Mhz(c.hclk_hz));
    out!("PCLK1 (APB1): {}\n", Mhz(c.pclk1_hz));
    out!("PCLK2 (APB2): {}\n", Mhz(c.pclk2_hz));
}

/// print_app_port -- the banner line of the application port.
fn print_app_port() {
    if app_port::ENABLED {
        out!(
            "App port:     USART1 {} 8N1, RTS/CTS {}\n",
            ipmc_defs::IPMC_APP_UART_BAUD,
            if ipmc_defs::IPMC_APP_UART_FLOW {
                "on"
            } else {
                "off"
            }
        );
    } else {
        out!("App port:     off\n");
    }
}

fn print_ga(g: &AddrInfo) {
    out!(
        "GA raw pins (at start-up): GA4..GA0={:05b} GAP={} NVMRO={} (1 = high)\n",
        g.ga_pin_levels(),
        u8::from(g.gap_pin_high()),
        u8::from(g.nvmro_pin_high())
    );
    out!(
        "GA value:     {}{} ({})\n",
        g.ga,
        if g.valid { "" } else { " (invalid)" },
        g.ga_source.text()
    );
    out!("HW address:   0x{:02X}\n", g.hw_address);
    out!("IPMB address: 0x{:02X}\n", g.ipmb_address);
    out!("GA parity:    {}\n", if g.parity_ok { "OK" } else { "BAD" });
    out!(
        "NVMRO:        {} ({})\n",
        if g.nvmro_asserted {
            "asserted"
        } else {
            "deasserted"
        },
        g.nvmro_source.text()
    );
}

// ---- handlers ----

/// UART_VALUE_COLUMN -- the column of the value in a SYS STATUS line (the width of `Uptime:       `).
const UART_VALUE_COLUMN: usize = 14;

/// port_name -- the name the firmware prints for a UART (defs.rs IPMC_CONSOLE_UART_NAME and
/// IPMC_APP_UART_NAME); a PC's port number is not known here.
fn port_name(port: Port) -> &'static str {
    match port {
        Port::Console => ipmc_defs::IPMC_CONSOLE_UART_NAME,
        Port::App => ipmc_defs::IPMC_APP_UART_NAME,
    }
}

/// print_uart_label -- `<name> UART:` padded to UART_VALUE_COLUMN (at least one space).
fn print_uart_label(port: Port) {
    let name = port_name(port);
    let pad = UART_VALUE_COLUMN
        .saturating_sub(name.len() + " UART:".len())
        .max(1);
    out!("{} UART:{:pad$}", name, "");
}

/// print_uart -- one line with the failure counters and the free TX bytes of a port.
fn print_uart(port: Port) {
    let e = uart::counters(port);
    print_uart_label(port);
    out!(
        "rx overflow={} tx timeout={} overrun={} line errors={} tx lines dropped={} tx free={}\n",
        e.rx_overflow,
        e.tx_timeout,
        e.overrun,
        e.line_errors,
        e.tx_lines_dropped,
        uart::tx_free(port)
    );
}

fn cmd_status(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let s = systick::seconds();
    out!(
        "Uptime:       {}d {:02}:{:02}:{:02}\n",
        s / 86_400,
        (s / 3_600) % 24,
        (s / 60) % 60,
        s % 60
    );
    print_clock(ctx);
    print_ga(&ctx.ipmb.dispatch().addr);
    print_led();
    print_reset(ctx);
    print_watchdog(ctx);
    print_uart(Port::Console);
    if ipmc_defs::IPMC_APP_UART_ENABLE {
        print_uart(Port::App);
    } else {
        out!("App UART:     off (IPMC_APP_UART_ENABLE false)\n");
    }
    out!("{}\n", addr::bootloader_pin_text(ga::bootloader_pin_high()));
    Ok(())
}

/// print_reset -- the `Last reset:` line and, when the previous run kept a fault, the
/// `Last fault:` line (the words of the fault console lines).
fn print_reset(ctx: &Context) {
    out!("Last reset:   {}\n", ctx.reset_cause.text());
    match ctx.last_fault {
        None => {}
        Some(Fault::HardFault { pc }) => out!("Last fault:   HARDFAULT pc=0x{pc:08X}\n"),
        Some(Fault::UnhandledIrq { irq }) => out!("Last fault:   UNHANDLED IRQ {irq}\n"),
        Some(Fault::Panic { line, column }) => {
            out!("Last fault:   PANIC line {line} column {column}\n")
        }
    }
}

/// print_watchdog -- the `Watchdog:` line (`SYS STATUS` and `DEBUG WATCHDOG`).
pub(crate) fn print_watchdog(ctx: &Context) {
    match (ipmc_defs::IPMC_WATCHDOG_ENABLE, ctx.watchdog_ms) {
        (false, _) => out!("Watchdog:     off (IPMC_WATCHDOG_ENABLE false)\n"),
        (true, Some(ms)) => out!("Watchdog:     on, {ms} ms\n"),
        (true, None) => out!("Watchdog:     not started\n"),
    }
}

fn cmd_reset(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    out!(
        "Resetting
"
    );
    ctx.ipmb.restart(None)
}

fn cmd_version(_ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    print_version();
    Ok(())
}

fn cmd_ipmb_addr(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    print_ga(&ctx.ipmb.dispatch().addr);
    Ok(())
}

/// print_led -- one line with the LED mode and rate (SYS STATUS and LED STATUS).
fn print_led() {
    let shown = led::shown();
    let chz = led::rate_chz(shown);
    let (mode, auto) = match shown {
        Shown::Heartbeat => ("heartbeat", true),
        Shown::Activity => ("activity", true),
        Shown::On => ("on", false),
        Shown::Off => ("off", false),
        Shown::Blink(_) => ("blink", false),
        Shown::Fault(_) => ("fault blink", false),
    };
    out!(
        "LED (LD2):    {} ({})",
        mode,
        if auto { "auto" } else { "manual" }
    );
    if chz != 0 {
        out!(", {} Hz", Hz(chz));
    }
    out!("\n");
}

fn cmd_led_status(_ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    print_led();
    out!(
        "Blink limits: {} to {} Hz (defs.rs IPMC_LED_RATE_MIN_CHZ, IPMC_LED_RATE_MAX_CHZ)\n",
        Hz(led::RATE_MIN_CHZ),
        Hz(led::RATE_MAX_CHZ)
    );
    Ok(())
}

fn cmd_led_auto(_ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    led::set_auto();
    out!("OK: LED auto\n");
    Ok(())
}

fn cmd_led_on(_ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    led::set_on();
    out!("OK: LED on\n");
    Ok(())
}

fn cmd_led_off(_ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    led::set_off();
    out!("OK: LED off\n");
    Ok(())
}

/// Parses "5", "0.5", ".25" into centihertz. Digits past the second decimal are rejected.
fn parse_rate_chz(s: &str) -> Option<u32> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if frac.len() > 2
        || !whole
            .bytes()
            .chain(frac.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole_val = if whole.is_empty() {
        0
    } else {
        whole.parse::<u32>().ok()?
    };
    let frac_val = if frac.is_empty() {
        0
    } else {
        frac.parse::<u32>().ok()?
    };
    let frac_chz = if frac.len() == 1 {
        frac_val * 10
    } else {
        frac_val
    };
    whole_val.checked_mul(100)?.checked_add(frac_chz)
}

fn cmd_led_blink(_ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let arg = args.text_at(0)?;
    match parse_rate_chz(arg) {
        Some(chz) if (led::RATE_MIN_CHZ..=led::RATE_MAX_CHZ).contains(&chz) => {
            led::set_blink_chz(chz);
            out!("OK: LED blink {} Hz\n", Hz(chz));
            Ok(())
        }
        Some(_) => {
            out!(
                "Error: rate must be between {} and {} Hz\n",
                Hz(led::RATE_MIN_CHZ),
                Hz(led::RATE_MAX_CHZ)
            );
            Err(CmdError::Reported)
        }
        None => {
            out!(
                "Error: '{}' is not a number (example: led blink 2.5)\n",
                arg
            );
            Err(CmdError::Reported)
        }
    }
}
