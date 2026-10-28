// console.rs - UART command line: the command tables and handlers on the ipmc-menu engine, generic
// over the board.
//
// The engine (crates/ipmc-menu) owns line editing, matching, help and the prompt; this file owns
// the command tree and the handlers of the SYS and LED groups, the banner and the main-loop steps
// of the console. Every other handler lives in the module that owns the behaviour. The console
// writes through the handle each part holds (`Context::con`, `Console::con`; clones of one board
// console), so no process global is involved.
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
// The SENSORS (SHOW, TRACE), POWER and SIM handlers live in sensing_cmds.rs; the IPMB STATS and
// IPMB STATE handlers live in ipmb_cmds.rs; the FRU handlers (SHOW, DUMP, STORE, FIELDS, GET, SET,
// RESTORE) and SENSORS SDR live in data_cmds.rs; the DEBUG handlers live in debug_cmds.rs (STATUS,
// EVENTS, IPMI, INJECT, CACHE, BOOTLOADER, WATCHDOG) and log_cmds.rs (LOG, DUMP, CLEAR, TRACE).
//
// The tables are associated consts of `Console<B, L>`: a `static` table cannot be generic over the
// board. `Console::new` evaluates `TABLES_VALID` and `HELP_FITS`, so a table fault or a TX ring too
// small for the longest HELP listing is a build error of every board that builds a console.

use core::fmt;
use core::ops::DerefMut;

use ipmc_boot::header::{self, HEADER_LEN};
use ipmc_boot::text::{put_dec, put_hex_bytes, put_version, TextOut};
use ipmc_core::addr::{self, AddrInfo};
use ipmc_hal::board::{Board, Platform};
use ipmc_hal::fault::Fault;
use ipmc_hal::serial::{SerialCounters, SerialPort, CONSOLE_TX_USABLE};
use ipmc_hal::system::{ClockSummary, StatusLed, Straps};
use ipmc_hal::time::Clock;
use ipmc_log::DumpJob;
use ipmc_menu::{
    report_line_error, Args, CmdError, CmdResult, Command, EditEvent, Group, LineEditor, Menu,
    MenuError,
};
use ipmc_sensors::eval::led::{rate_chz, Mode, Shown};

use crate::app_port::{self, AppPort};
use crate::link::{self, IpmbLink, Restart};
use crate::run::StartInfo;
use crate::sensing::BoardSensing;
use crate::text::{ConsoleText, DumpOut, MenuOut};
use crate::{data_cmds, debug_cmds, ipmb_cmds, log_cmds, out, sensing_cmds};

/// LinkHome -- where the console finds the one IPMB link: the firmware passes a `&'static mut`
/// reference to the link in static storage, the emulator a `Box`. `'static` because the menu holds
/// its context type in `'static` tables.
pub trait LinkHome<B: Board>: DerefMut<Target = IpmbLink<B>> + 'static {}

impl<B: Board, T: DerefMut<Target = IpmbLink<B>> + 'static> LinkHome<B> for T {}

/// Context -- the value every command handler receives: the handles and start-up values the
/// handlers report, the sensor service and the IPMB link.
pub struct Context<B: Board, L: LinkHome<B>> {
    /// The console port the handlers print on.
    pub(crate) con: B::Console,
    /// LD2, for the LED group and `SYS STATUS`.
    pub(crate) led: B::Led,
    /// The strap pins, for the BOOTLOADER pin line.
    pub(crate) straps: B::Straps,
    /// The 1 ms tick, for uptimes and sample times.
    pub(crate) clock: B::Clock,
    /// The watchdog period set; None while the watchdog is not running (`print_watchdog`).
    pub(crate) watchdog_ms: Option<u32>,
    /// What the board found at start-up (banner and `SYS STATUS`).
    pub(crate) start: StartInfo,
    /// A processor restart a handler asked for (`SYS RESET`); `Console::poll` returns it.
    pub(crate) restart: Option<Restart>,
    /// Sensor monitor and its backends; the SENSORS, POWER and SIM handlers use it.
    pub(crate) sensing: BoardSensing<B>,
    /// IPMB link layer; the IPMB STATS and IPMB STATE handlers read it.
    pub(crate) ipmb: L,
    /// The DEBUG DUMP in progress, continued by `Console::poll` (log_cmds.rs).
    pub(crate) dump: Option<DumpJob>,
    /// The application port: its line assembler, polled by `Console::service_app`.
    pub(crate) app: AppPort<B>,
}

/// Centihertz in one hertz.
const CHZ_PER_HZ: u32 = 100;

/// FRU_READ_ROWS -- the rows of `FRU_COMMANDS` that only read (SHOW, DUMP, STORE, FIELDS, GET);
/// the write rows (SET, RESTORE) follow them.
const FRU_READ_ROWS: usize = 5;

/// Console -- the console: line editor, menu and handler context.
pub struct Console<B: Board, L: LinkHome<B>> {
    ctx: Context<B, L>,
    menu: Menu<Context<B, L>>,
    editor: LineEditor,
    /// The console port for the echo and the menu output (a clone of `ctx.con`: the menu holds the
    /// context while it prints).
    con: B::Console,
}

/// ConsoleArgs -- what `Console::new` takes: the start-up values, the handles and the services.
pub struct ConsoleArgs<B: Board, L: LinkHome<B>> {
    pub start: StartInfo,
    pub clock: B::Clock,
    pub console: B::Console,
    pub app: B::App,
    pub led: B::Led,
    pub straps: B::Straps,
    pub sensing: BoardSensing<B>,
    pub ipmb: L,
}

impl<B: Board, L: LinkHome<B>> Console<B, L> {
    // ---- command tables (MenuBuilder FUNCTION:COMMAND:DESCRIPTION) ----

    /// Top-level commands: aliases that work without entering a group.
    const TOP_COMMANDS: &'static [Command<Context<B, L>>] = &[
        Command {
            syntax: "STATUS",
            help: "Alias of SYS STATUS",
            handler: cmd_status::<B, L>,
        },
        Command {
            syntax: "RESET",
            help: "Alias of SYS RESET",
            handler: cmd_reset::<B, L>,
        },
    ];

    const SYS_COMMANDS: &'static [Command<Context<B, L>>] = &[
        Command {
            syntax: "STATUS",
            help: "Uptime, clocks, geographic address, LED, last reset cause",
            handler: cmd_status::<B, L>,
        },
        Command {
            syntax: "RESET",
            help: "System reset",
            handler: cmd_reset::<B, L>,
        },
        Command {
            syntax: "VERSION",
            help: "Firmware and core library versions",
            handler: cmd_version::<B, L>,
        },
    ];

    const LED_COMMANDS: &'static [Command<Context<B, L>>] = &[
        Command {
            syntax: "ON",
            help: "LD2 on",
            handler: cmd_led_on::<B, L>,
        },
        Command {
            syntax: "OFF",
            help: "LD2 off",
            handler: cmd_led_off::<B, L>,
        },
        Command {
            syntax: "BLINK [HZ]",
            help: "LD2 blink at the given rate in Hz; the limits print with LED STATUS",
            handler: cmd_led_blink::<B, L>,
        },
        Command {
            syntax: "AUTO",
            help: "LD2 back to the heartbeat / IPMB activity pattern",
            handler: cmd_led_auto::<B, L>,
        },
        Command {
            syntax: "STATUS",
            help: "LD2 mode, rate and the accepted blink rate limits",
            handler: cmd_led_status::<B, L>,
        },
    ];

    const IPMB_COMMANDS: &'static [Command<Context<B, L>>] = &[
    Command {
        syntax: "ADDR",
        help: "Geographic address, IPMB address, parity and NVMRO with their sources",
        handler: cmd_ipmb_addr::<B, L>,
    },
    Command {
        syntax: "STATS",
        help: "Per-bus receive, error, transmit, resend and replay counters",
        handler: ipmb_cmds::cmd_ipmb_stats::<B, L>,
    },
    Command {
        syntax: "STATE",
        help:
            "Per-bus fitted, mode and enable state, the transmit policy and the link sensor bytes",
        handler: ipmb_cmds::cmd_ipmb_state::<B, L>,
    },
];

    const SENSORS_COMMANDS: &'static [Command<Context<B, L>>] = &[
    Command {
        syntax: "SHOW",
        help: "Rails, die temperature, inputs, power state, FRU state, Tier-1 readings and their data bytes",
        handler: sensing_cmds::cmd_sensors_show::<B, L>,
    },
    Command {
        syntax: "SDR",
        help: "The Device SDR records: sensor number, type, event/reading type, ID string",
        handler: data_cmds::cmd_sensors_sdr::<B, L>,
    },
    Command {
        syntax: "TRACE {LEVEL}",
        help: "Sensor trace level OFF, EVENTS (changes) or ALL (changes and samples)",
        handler: sensing_cmds::cmd_sensors_trace::<B, L>,
    },
];

    const POWER_COMMANDS: &'static [Command<Context<B, L>>] = &[
        Command {
            syntax: "ON",
            help: "Assert PWR_EN and wait for PGOOD",
            handler: sensing_cmds::cmd_power_on::<B, L>,
        },
        Command {
            syntax: "OFF",
            help: "Release PWR_EN and clear a latched fault",
            handler: sensing_cmds::cmd_power_off::<B, L>,
        },
        Command {
            syntax: "CLEAR",
            help: "Clear a latched power fault",
            handler: sensing_cmds::cmd_power_clear::<B, L>,
        },
        Command {
            syntax: "STATUS",
            help: "Power state, PWR_EN, latched fault",
            handler: sensing_cmds::cmd_power_status::<B, L>,
        },
    ];

    const SIM_COMMANDS: &'static [Command<Context<B, L>>] = &[
        Command {
            syntax: "RAIL {N} {MV}",
            help: "Simulated rail N (0 = VDDA) in millivolts; no argument lists them",
            handler: sensing_cmds::cmd_sim_rail::<B, L>,
        },
        Command {
            syntax: "TEMP {DC}",
            help: "Simulated die temperature in tenths of a degree C",
            handler: sensing_cmds::cmd_sim_temp::<B, L>,
        },
        Command {
            syntax: "FAULT {NAME} {STATE}",
            help: "Assert (ON) or clear (OFF) a simulated input; no argument lists them",
            handler: sensing_cmds::cmd_sim_fault::<B, L>,
        },
        Command {
            syntax: "SHOW",
            help: "Source of every group and the simulated values",
            handler: sensing_cmds::cmd_sim_show::<B, L>,
        },
    ];

    const FRU_COMMANDS: &'static [Command<Context<B, L>>] = &[
    Command {
        syntax: "SHOW",
        help: "FRU image decoded: common header, board and product areas, checksums",
        handler: data_cmds::cmd_fru_show::<B, L>,
    },
    Command {
        syntax: "DUMP {OFFSET} {COUNT}",
        help: "Hex dump of the FRU image; default offset 0 and the rest of the area",
        handler: data_cmds::cmd_fru_dump::<B, L>,
    },
    Command {
        syntax: "STORE",
        help: "Flash log state: records, newest sequence, free bytes, pending write, next erase",
        handler: data_cmds::cmd_fru_store::<B, L>,
    },
    Command {
        syntax: "FIELDS",
        help: "Every FRU field with its maximum length and its current value",
        handler: data_cmds::cmd_fru_fields::<B, L>,
    },
    Command {
        syntax: "GET [FIELD]",
        help: "Current value of one FRU field; FRU FIELDS lists the names",
        handler: data_cmds::cmd_fru_get::<B, L>,
    },
    // The rows below this line are the write rows: they are in the FRU group only when
    // IPMC_FRU_CONSOLE_WRITE is true (`FRU_TABLE`).
    Command {
        syntax: "SET [FIELD] {VALUE...}",
        help: "Write a FRU field (no value clears a text field or the date; MFG_DATE takes YYYY-MM-DD HH:MM); saved to flash after the save delay; refused while NVMRO is asserted",
        handler: data_cmds::cmd_fru_set::<B, L>,
    },
    Command {
        syntax: "RESTORE",
        help:
            "Save the built FRU image as a new log record and put it back; refused while NVMRO is asserted",
        handler: data_cmds::cmd_fru_restore::<B, L>,
    },
];

    /// FRU_TABLE -- the table of the FRU group, chosen at compile time from IPMC_FRU_CONSOLE_WRITE:
    /// every row when it is 1, only the read rows when it is 0 (SET and RESTORE do not exist: HELP
    /// does not list them and a typed SET is an unknown command).
    const FRU_TABLE: &'static [Command<Context<B, L>>] = if ipmc_defs::IPMC_FRU_CONSOLE_WRITE {
        Self::FRU_COMMANDS
    } else {
        Self::FRU_COMMANDS.split_at(FRU_READ_ROWS).0
    };

    const DEBUG_COMMANDS: &'static [Command<Context<B, L>>] = &[
    Command {
        syntax: "STATUS",
        help: "Debug switches, capture log use, event receiver, duplicate cache",
        handler: debug_cmds::cmd_debug_status::<B, L>,
    },
    Command {
        syntax: "LOG {STATE}",
        help: "Capture log ON or OFF; no argument shows its state",
        handler: log_cmds::cmd_debug_log::<B, L>,
    },
    Command {
        syntax: "DUMP {FORMAT}",
        help: "Print the capture log: CSV (default) or TRACE; any key stops it",
        handler: log_cmds::cmd_debug_dump::<B, L>,
    },
    Command {
        syntax: "CLEAR",
        help: "Empty the capture log",
        handler: log_cmds::cmd_debug_clear::<B, L>,
    },
    Command {
        syntax: "TRACE {WHAT} {STATE}",
        help: "Console trace: IPMI (request, link and event lines) or I2C (one line per frame), ON or OFF",
        handler: log_cmds::cmd_debug_trace::<B, L>,
    },
    Command {
        syntax: "EVENTS {STATE}",
        help: "Event messages ON (receiver 20h) or OFF (receiver FFh)",
        handler: debug_cmds::cmd_debug_events::<B, L>,
    },
    Command {
        syntax: "IPMI [NETFN] [CMD] {DATA...}",
        help: "Run one IPMI request locally, as from 20h; nothing goes on the bus",
        handler: debug_cmds::cmd_debug_ipmi::<B, L>,
    },
    Command {
        syntax: "INJECT [BUS] {BYTES...}",
        help: "Hand a whole IPMB frame to the link as if received on bus A or B",
        handler: debug_cmds::cmd_debug_inject::<B, L>,
    },
    Command {
        syntax: "CACHE {ACTION}",
        help: "Duplicate cache: no argument shows the entries, CLEAR empties it",
        handler: debug_cmds::cmd_debug_cache::<B, L>,
    },
    Command {
        syntax: "BOOTLOADER {CONFIRM}",
        help: "Restart into the bootloader; CONFIRM needed while payload power is on",
        handler: debug_cmds::cmd_debug_bootloader::<B, L>,
    },
    Command {
        syntax: "WATCHDOG {TEST} {CONFIRM}",
        help: "Watchdog state; HANG or PANIC with CONFIRM tests the reset",
        handler: debug_cmds::cmd_debug_watchdog::<B, L>,
    },
];

    const GROUPS: &'static [Group<Context<B, L>>] = &[
        Group {
            name: "SYS",
            description: "System status, reset and version",
            commands: Self::SYS_COMMANDS,
        },
        Group {
            name: "LED",
            description: "LD2 control",
            commands: Self::LED_COMMANDS,
        },
        Group {
            name: "IPMB",
            description: "IPMB address and bus state",
            commands: Self::IPMB_COMMANDS,
        },
        Group {
            name: "SENSORS",
            description: "Voltage rails, die temperature, inputs and Tier-1 readings",
            commands: Self::SENSORS_COMMANDS,
        },
        Group {
            name: "POWER",
            description: "Payload power enable",
            commands: Self::POWER_COMMANDS,
        },
        Group {
            name: "SIM",
            description: "Simulated sensor inputs",
            commands: Self::SIM_COMMANDS,
        },
        Group {
            name: "FRU",
            description: "FRU image, its decoded areas, its fields and the flash log",
            commands: Self::FRU_TABLE,
        },
        Group {
            name: "DEBUG",
            description: "Test switches, the IPMB capture log and local IPMI requests",
            commands: Self::DEBUG_COMMANDS,
        },
    ];

    /// TABLES_VALID -- a table fault is a build error, not a run-time surprise.
    const TABLES_VALID: () = {
        assert!(
            Menu::validate_tables(Self::TOP_COMMANDS, Self::GROUPS).is_ok(),
            "console command tables are invalid"
        );
        // The FRU table is valid with every row, so it is valid with the read rows alone: a row
        // taken away cannot create a duplicate or an empty group, whatever
        // IPMC_FRU_CONSOLE_WRITE is.
        assert!(
            Menu::validate_tables(
                Self::TOP_COMMANDS,
                &[Group {
                    name: "FRU",
                    description: "",
                    commands: Self::FRU_COMMANDS,
                }]
            )
            .is_ok(),
            "FRU command table is invalid"
        );
    };

    /// HELP_FITS -- the longest HELP listing fits the console TX ring.
    const HELP_FITS: () = assert!(
        help_worst(Self::TOP_COMMANDS, Self::GROUPS) + 256 <= CONSOLE_TX_USABLE,
        "IPMC_UART_TX_BUF is too small for the longest HELP listing (console.rs help_worst)"
    );

    /// new -- a console over the start-up values, the handles, the sensor service and the IPMB
    /// link.
    pub fn new(args: ConsoleArgs<B, L>) -> Self {
        let () = Self::TABLES_VALID;
        let () = Self::HELP_FITS;
        let ConsoleArgs {
            start,
            clock,
            console,
            app,
            led,
            straps,
            sensing,
            ipmb,
        } = args;
        let version = start.version;
        Console {
            ctx: Context {
                con: console.clone(),
                led,
                straps: straps.clone(),
                clock: clock.clone(),
                watchdog_ms: None,
                start,
                restart: None,
                sensing,
                ipmb,
                dump: None,
                app: AppPort::new(app, clock, straps, version),
            },
            menu: Menu::new(
                ipmc_defs::IPMC_CONSOLE_PROMPT,
                Self::TOP_COMMANDS,
                Self::GROUPS,
            ),
            editor: LineEditor::new(),
            con: console,
        }
    }

    /// link -- the IPMB link service.
    pub fn link(&self) -> &IpmbLink<B> {
        &self.ctx.ipmb
    }

    /// link_mut -- the IPMB link service, to change.
    pub fn link_mut(&mut self) -> &mut IpmbLink<B> {
        &mut self.ctx.ipmb
    }

    /// sensing -- the sensor service.
    pub fn sensing(&self) -> &BoardSensing<B> {
        &self.ctx.sensing
    }

    /// sensing_mut -- the sensor service, to change.
    pub fn sensing_mut(&mut self) -> &mut BoardSensing<B> {
        &mut self.ctx.sensing
    }

    /// set_watchdog -- records the period the watchdog runs at (`SYS STATUS`, `DEBUG WATCHDOG`).
    pub fn set_watchdog(&mut self, period_ms: Option<u32>) {
        self.ctx.watchdog_ms = period_ms;
    }

    /// start_events -- applies the events-at-boot setting (`StartInfo::events_at_boot`).
    pub fn start_events(&mut self) {
        debug_cmds::events_off_at_boot(
            self.ctx.start.events_at_boot,
            &mut *self.ctx.ipmb,
            &mut self.ctx.con,
        );
    }

    /// start_sensors -- applies the power-at-boot setting (`StartInfo::power_at_boot`).
    pub fn start_sensors(&mut self) {
        let power_on_check = self.ctx.ipmb.power_on_check();
        let now_ms = self.ctx.clock.millis();
        self.ctx.sensing.start(
            self.ctx.start.power_at_boot,
            power_on_check,
            &mut self.ctx.con,
            now_ms,
        );
    }

    /// service_sensors -- serves the payload power and payload reset requests of the command
    /// handlers (`Sensing::serve_hand_offs`), takes a sensor sample when the period has elapsed,
    /// and hands every new sample (also one a console command took) to the IPMI handlers and the
    /// FRU state machine.
    pub fn service_sensors(&mut self) {
        let hand_offs = self.ctx.ipmb.take_hand_offs();
        let now_ms = self.ctx.clock.millis();
        self.ctx
            .sensing
            .serve_hand_offs(hand_offs, &mut self.ctx.con, now_ms);
        let now_ms = self.ctx.clock.millis();
        self.ctx.sensing.service(&mut self.ctx.con, now_ms);
        if let Some(snapshot) = self.ctx.sensing.take_fresh() {
            self.ctx.ipmb.feed_sensors(&snapshot);
        }
    }

    /// service_app -- one main-loop pass of the application port (`AppPort::poll`): takes the
    /// received bytes and answers at most one request line.
    // inline(never): the step's locals (the link's receive and save buffers, the line parser, the
    // application port's answer) live in its own frame while it runs and not in the frame of
    // `run`, which holds the console for as long as the firmware runs.
    #[inline(never)]
    pub fn service_app(&mut self) {
        let power = self.ctx.sensing.payload_power_on();
        self.ctx.app.poll(&mut *self.ctx.ipmb, power);
    }

    /// service_link -- runs the IPMB link layer's main-loop step; Some when the link asks for a
    /// processor restart (a Cold Reset or a restart into the bootloader).
    // inline(never): the step's locals (the link's receive and save buffers, the line parser, the
    // application port's answer) live in its own frame while it runs and not in the frame of
    // `run`, which holds the console for as long as the firmware runs.
    #[inline(never)]
    pub fn service_link(&mut self) -> Option<Restart> {
        self.ctx.ipmb.service()
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
    pub fn banner(&mut self) {
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let ctx = &mut self.ctx;
        let con = &mut ctx.con;
        out!(con, "\n");
        print_version(con, &ctx.start);
        out!(con, "Build:        {}\n", profile);
        #[cfg(ipmc_eval)]
        out!(
            con,
            "EVALUATION BUILD: the IPMB stops after {} minutes\n",
            ipmc_defs::IPMC_EVAL_MINUTES
        );
        print_reset(con, &ctx.start);
        print_clock(con, &ctx.start.clock);
        print_ga(con, &ctx.ipmb.dispatch().addr);
        let [result, detail] = ctx.ipmb.dispatch().self_test.bytes();
        out!(
            con,
            "Self test:    {:02X}h {:02X}h ({})\n",
            result,
            detail,
            if ctx.ipmb.dispatch().self_test.passed() {
                "passed"
            } else {
                "failed"
            }
        );
        out!(
            con,
            "{}\n",
            addr::bootloader_pin_text(ctx.straps.bootloader_pin_high())
        );
        print_app_port::<B>(con);
        out!(con, "Type HELP or ? for commands.\n");
    }

    /// prompt -- prints the root prompt.
    pub fn prompt(&mut self) {
        self.menu.write_prompt(&mut MenuOut(&mut self.con));
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
    /// RX ring (IPMC_UART_RX_BUF). Returns the restart a command asked for (`SYS RESET`) without
    /// writing the prompt; the board performs it.
    // inline(never): the step's locals (the link's receive and save buffers, the line parser, the
    // application port's answer) live in its own frame while it runs and not in the frame of
    // `run`, which holds the console for as long as the firmware runs.
    #[inline(never)]
    pub fn poll(&mut self) -> Option<Restart> {
        if self.ctx.dump.is_some() {
            self.poll_dump();
            return None;
        }
        for _ in 0..ipmc_defs::IPMC_CONSOLE_BYTES_PER_POLL {
            let byte = self.con.getc()?;
            match self.editor.push_byte(byte) {
                EditEvent::Echo(ch) => out!(&mut self.con, "{}", char::from(ch)),
                EditEvent::Backspace => out!(&mut self.con, "\u{8} \u{8}"), // erase the character on screen
                EditEvent::Line(line) => {
                    out!(&mut self.con, "\n");
                    self.menu
                        .process_line(line, &mut self.ctx, &mut MenuOut(&mut self.con));
                    if let Some(restart) = self.ctx.restart.take() {
                        return Some(restart);
                    }
                    if self.ctx.dump.is_none() {
                        self.menu.write_prompt(&mut MenuOut(&mut self.con));
                        // a dump prints it when it ends
                    }
                    return None;
                }
                EditEvent::LineTooLong => {
                    out!(&mut self.con, "\n");
                    report_line_error(MenuError::LineTooLong, &mut MenuOut(&mut self.con));
                    self.menu.write_prompt(&mut MenuOut(&mut self.con));
                    return None;
                }
                EditEvent::None => {}
            }
        }
        None
    }

    /// poll_dump -- one main-loop pass of a running DEBUG DUMP. A received byte stops the dump
    /// (a line feed does not count: a terminal that ends Enter with CR LF sends it right after
    /// the CR that started the dump); otherwise `DumpJob::step` (ipmc-log) prints the rows the TX ring has room for. New command lines
    /// wait: the menu does not run while the job exists. The prompt follows the end of the job.
    fn poll_dump(&mut self) {
        let stopped = matches!(self.con.getc(), Some(byte) if byte != b'\n');
        if stopped {
            out!(&mut self.con, "# dump stopped\n");
        }
        let finished = match self.ctx.dump.as_mut() {
            Some(job) if !stopped => job.step(
                self.ctx.ipmb.log().capture(),
                &self.ctx.ipmb.names(),
                &mut DumpOut(&mut self.con),
            ),
            _ => true,
        };
        if finished {
            self.ctx.dump = None;
            self.menu.write_prompt(&mut MenuOut(&mut self.con));
        }
    }
}

impl<P: Platform, L: LinkHome<P>> Console<P, L> {
    /// restart -- performs the processor restart `poll` or `service_link` returned
    /// (`link::restart`: both ports flushed, the request sequence kept, the boot request words
    /// written, the reset); does not return.
    pub fn restart(
        &mut self,
        kept: &mut P::KeptSeq,
        boot_request: &mut P::BootRequest,
        system: &mut P::System,
        restart: Restart,
    ) -> ! {
        link::restart::<P>(
            &*self.ctx.ipmb,
            &mut self.ctx.con,
            &mut self.ctx.app.port,
            kept,
            boot_request,
            system,
            restart,
        )
    }
}

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
const fn help_width<C: 'static>(top: &[Command<C>], groups: &[Group<C>]) -> usize {
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
const fn help_rows<C>(commands: &[Command<C>], width: usize) -> usize {
    let (mut total, mut i) = (0, 0);
    while i < commands.len() {
        total += 2 + width + 3 + commands[i].help.len() + 2;
        i += 1;
    }
    total
}

/// help_worst -- the bytes of the longest HELP listing: a group's, or the root's.
const fn help_worst<C: 'static>(top: &[Command<C>], groups: &[Group<C>]) -> usize {
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

/// DIGEST_SHOWN -- digest bytes the image line prints (16 hex digits).
const DIGEST_SHOWN: usize = 8;

/// image_line -- `v<x.y.z>, <n> bytes, SHA-256 <first 16 hex digits>` from the 64 header bytes
/// the board read at start (the text after the `Image:` label of the banner and `SYS VERSION`).
fn image_line(raw: &[u8; HEADER_LEN], out: &mut dyn TextOut) {
    let image = header::decode(raw);
    put_version(out, image.version);
    out.put(", ");
    put_dec(out, image.length);
    out.put(" bytes, SHA-256 ");
    put_hex_bytes(out, image.digest.get(..DIGEST_SHOWN).unwrap_or(&[]));
}

fn print_version(con: &mut impl ConsoleText, start: &StartInfo) {
    out!(
        con,
        "{} v{} (ipmc-core v{})\n",
        ipmc_defs::IPMC_FW_NAME,
        start.version,
        ipmc_core::CORE_VERSION
    );
    // The image header in flash: version, length and the start of the digest.
    out!(con, "Image:        ");
    image_line(&start.image_header, &mut ConsoleTextOut(&mut *con));
    out!(con, "\n");
}

/// ConsoleTextOut -- a console as the bootloader's `TextOut`, for `image_line`.
struct ConsoleTextOut<'a, C: ConsoleText>(&'a mut C);

impl<C: ConsoleText> TextOut for ConsoleTextOut<'_, C> {
    fn put(&mut self, text: &str) {
        self.0.print(format_args!("{}", text));
    }
}

fn print_clock(con: &mut impl ConsoleText, clock: &ClockSummary) {
    out!(con, "Clock source: {}\n", clock.source);
    if let Some(fault) = clock.fault {
        out!(con, "Clock fault:  {}\n", fault);
    }
    for domain in clock.domains.iter().flatten() {
        out!(con, "{}{}\n", domain.label, Mhz(domain.hz));
    }
}

/// print_app_port -- the banner line of the application port.
fn print_app_port<B: Board>(con: &mut impl ConsoleText) {
    if app_port::ENABLED {
        out!(
            con,
            "App port:     {} {} 8N1, RTS/CTS {}\n",
            B::APP_UART_PERIPHERAL,
            ipmc_defs::IPMC_APP_UART_BAUD,
            if ipmc_defs::IPMC_APP_UART_FLOW {
                "on"
            } else {
                "off"
            }
        );
    } else {
        out!(con, "App port:     off\n");
    }
}

fn print_ga(con: &mut impl ConsoleText, g: &AddrInfo) {
    out!(
        con,
        "GA raw pins (at start-up): GA4..GA0={:05b} GAP={} NVMRO={} (1 = high)\n",
        g.ga_pin_levels(),
        u8::from(g.gap_pin_high()),
        u8::from(g.nvmro_pin_high())
    );
    out!(
        con,
        "GA value:     {}{} ({})\n",
        g.ga,
        if g.valid { "" } else { " (invalid)" },
        g.ga_source.text()
    );
    out!(con, "HW address:   0x{:02X}\n", g.hw_address);
    out!(con, "IPMB address: 0x{:02X}\n", g.ipmb_address);
    out!(
        con,
        "GA parity:    {}\n",
        if g.parity_ok { "OK" } else { "BAD" }
    );
    out!(
        con,
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

/// print_uart -- one line with the failure counters and the free TX bytes of a port; `name` is the
/// name the firmware prints for the UART (defs.rs IPMC_CONSOLE_UART_NAME and IPMC_APP_UART_NAME).
fn print_uart(con: &mut impl ConsoleText, name: &str, e: SerialCounters, tx_free: usize) {
    // `<name> UART:` padded to UART_VALUE_COLUMN (at least one space).
    let pad = UART_VALUE_COLUMN
        .saturating_sub(name.len() + " UART:".len())
        .max(1);
    out!(con, "{} UART:{:pad$}", name, "");
    out!(
        con,
        "rx overflow={} tx timeout={} overrun={} line errors={} tx lines dropped={} tx free={}\n",
        e.rx_overflow,
        e.tx_timeout,
        e.overrun,
        e.line_errors,
        e.tx_lines_dropped,
        tx_free
    );
}

fn cmd_status<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    let con = &mut ctx.con;
    let s = ctx.clock.seconds();
    out!(
        con,
        "Uptime:       {}d {:02}:{:02}:{:02}\n",
        s / 86_400,
        (s / 3_600) % 24,
        (s / 60) % 60,
        s % 60
    );
    print_clock(con, &ctx.start.clock);
    print_ga(con, &ctx.ipmb.dispatch().addr);
    print_led(con, ctx.led.shown());
    print_reset(con, &ctx.start);
    print_watchdog(con, ctx.watchdog_ms);
    let (counters, tx_free) = (con.counters(), SerialPort::tx_free(&*con));
    print_uart(con, ipmc_defs::IPMC_CONSOLE_UART_NAME, counters, tx_free);
    if ipmc_defs::IPMC_APP_UART_ENABLE {
        let port = &ctx.app.port;
        print_uart(
            con,
            ipmc_defs::IPMC_APP_UART_NAME,
            port.counters(),
            SerialPort::tx_free(port),
        );
    } else {
        out!(con, "App UART:     off (IPMC_APP_UART_ENABLE false)\n");
    }
    out!(
        con,
        "{}\n",
        addr::bootloader_pin_text(ctx.straps.bootloader_pin_high())
    );
    Ok(())
}

/// print_reset -- the `Last reset:` line and, when the previous run kept a fault, the
/// `Last fault:` line (the words of the fault console lines).
fn print_reset(con: &mut impl ConsoleText, start: &StartInfo) {
    out!(con, "Last reset:   {}\n", start.reset_cause.text());
    match start.last_fault {
        None => {}
        Some(Fault::HardFault { pc }) => out!(con, "Last fault:   HARDFAULT pc=0x{pc:08X}\n"),
        Some(Fault::UnhandledIrq { irq }) => out!(con, "Last fault:   UNHANDLED IRQ {irq}\n"),
        Some(Fault::Panic { line, column }) => {
            out!(con, "Last fault:   PANIC line {line} column {column}\n")
        }
    }
}

/// print_watchdog -- the `Watchdog:` line (`SYS STATUS` and `DEBUG WATCHDOG`); `watchdog_ms` is
/// the period set, None while the watchdog is not running.
pub(crate) fn print_watchdog(con: &mut impl ConsoleText, watchdog_ms: Option<u32>) {
    match (ipmc_defs::IPMC_WATCHDOG_ENABLE, watchdog_ms) {
        (false, _) => out!(con, "Watchdog:     off (IPMC_WATCHDOG_ENABLE false)\n"),
        (true, Some(ms)) => out!(con, "Watchdog:     on, {ms} ms\n"),
        (true, None) => out!(con, "Watchdog:     not started\n"),
    }
}

/// cmd_reset -- `SYS RESET`: says so and leaves the restart for `Console::poll` to return.
fn cmd_reset<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    out!(&mut ctx.con, "Resetting\n");
    ctx.restart = Some(Restart { boot: None });
    Ok(())
}

fn cmd_version<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    print_version(&mut ctx.con, &ctx.start);
    Ok(())
}

fn cmd_ipmb_addr<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    print_ga(&mut ctx.con, &ctx.ipmb.dispatch().addr);
    Ok(())
}

/// print_led -- one line with the LED mode and rate (SYS STATUS and LED STATUS).
fn print_led(con: &mut impl ConsoleText, shown: Shown) {
    let chz = rate_chz(shown);
    let (mode, auto) = match shown {
        Shown::Heartbeat => ("heartbeat", true),
        Shown::Activity => ("activity", true),
        Shown::On => ("on", false),
        Shown::Off => ("off", false),
        Shown::Blink(_) => ("blink", false),
        Shown::Fault(_) => ("fault blink", false),
    };
    out!(
        con,
        "LED (LD2):    {} ({})",
        mode,
        if auto { "auto" } else { "manual" }
    );
    if chz != 0 {
        out!(con, ", {} Hz", Hz(chz));
    }
    out!(con, "\n");
}

fn cmd_led_status<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    print_led(con, ctx.led.shown());
    out!(
        con,
        "Blink limits: {} to {} Hz (defs.rs IPMC_LED_RATE_MIN_CHZ, IPMC_LED_RATE_MAX_CHZ)\n",
        Hz(ipmc_defs::IPMC_LED_RATE_MIN_CHZ),
        Hz(ipmc_defs::IPMC_LED_RATE_MAX_CHZ)
    );
    Ok(())
}

/// set_led -- switches LD2 to `mode` and says so: `OK: LED <what>`.
fn set_led<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, mode: Mode, what: &str) -> CmdResult {
    ctx.led.set_mode(mode);
    out!(&mut ctx.con, "OK: LED {}\n", what);
    Ok(())
}

fn cmd_led_auto<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    set_led(ctx, Mode::Auto, "auto")
}

fn cmd_led_on<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    set_led(ctx, Mode::On, "on")
}

fn cmd_led_off<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, _args: &Args<'_>) -> CmdResult {
    set_led(ctx, Mode::Off, "off")
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

fn cmd_led_blink<B: Board, L: LinkHome<B>>(ctx: &mut Context<B, L>, args: &Args<'_>) -> CmdResult {
    let con = &mut ctx.con;
    let arg = args.text_at(0)?;
    match parse_rate_chz(arg) {
        Some(chz)
            if (ipmc_defs::IPMC_LED_RATE_MIN_CHZ..=ipmc_defs::IPMC_LED_RATE_MAX_CHZ)
                .contains(&chz) =>
        {
            ctx.led.set_mode(Mode::Blink(chz));
            out!(con, "OK: LED blink {} Hz\n", Hz(chz));
            Ok(())
        }
        Some(_) => {
            out!(
                con,
                "Error: rate must be between {} and {} Hz\n",
                Hz(ipmc_defs::IPMC_LED_RATE_MIN_CHZ),
                Hz(ipmc_defs::IPMC_LED_RATE_MAX_CHZ)
            );
            Err(CmdError::Reported)
        }
        None => {
            out!(
                con,
                "Error: '{}' is not a number (example: led blink 2.5)\n",
                arg
            );
            Err(CmdError::Reported)
        }
    }
}

#[cfg(test)]
mod tests;
