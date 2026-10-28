// console/tests.rs - golden console transcripts over the host test board: the banner, SYS STATUS and
// VERSION, the LED group, HELP, FRU SHOW and DUMP, DEBUG STATUS and WATCHDOG, IPMB STATE, the
// restart paths (SYS RESET returns a restart and no prompt; DEBUG BOOTLOADER and the application
// port BOOTLOADER leave a boot request) and the build-time table checks.
//
// The expected text is the transcript the board printed before the console moved into this crate
// (the first lines of each case are the typed command and its echo), with the values of the test
// board where they differ: the clock lines of the summary below, the readings, the transmit-ring
// room. Values that come from ipmc-defs are the defaults.

extern crate std;

use super::*;
use crate::fru_log::FruLog;
use crate::link::{start_buses, LinkArgs};
use crate::sensing::Sensing;
use crate::sim::{
    start_info, Reset, Sim, SimClock, SimCycles, SimIpmb, SimLed, SimPins, SimSerial, SimStraps,
    SimSystem, GOLDEN_RAW,
};
use ipmc_boot::request::{self, Interface};
use ipmc_fru::RamFlash;

use ipmc_sensors::drivers::sim::SIM_RAIL_WIRING;
use std::boxed::Box;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::string::{String, ToString};
use std::vec::Vec;

/// Rig -- a console over the test board and the handles a test reads.
struct Rig {
    console: Console<Sim, Box<IpmbLink<Sim>>>,
    con: SimSerial,
    app: SimSerial,
    clock: SimClock,
}

/// rig -- a started console: IPMB-A up, an erased FRU log, the straps of `GOLDEN_RAW`. Nothing the
/// start-up prints is left on the console port.
fn rig() -> Rig {
    let con = SimSerial::new(ipmc_defs::IPMC_UART_TX_BUF - 1);
    let app = SimSerial::roomy();
    let (clock, straps) = (SimClock::default(), SimStraps::default());
    straps.set(GOLDEN_RAW);
    let mut port = con.clone();
    let (fru_log, fru_image) = FruLog::boot(Ok(RamFlash::new()), false, &mut port);
    let address = addr::resolve(straps.read_raw());
    let buses = start_buses(SimIpmb::default(), &address, &mut port);
    let link = Box::new(IpmbLink::new(LinkArgs {
        buses,
        fru_log,
        fru_image,
        kept: (0, 0),
        clock: clock.clone(),
        cycles: SimCycles::default(),
        straps: straps.clone(),
        con: con.clone(),
    }));
    let console = Console::new(ConsoleArgs {
        start: start_info(),
        clock: clock.clone(),
        console: con.clone(),
        app: app.clone(),
        led: SimLed::default(),
        straps,
        sensing: Sensing::new(None, Some(SimPins::new()), &SIM_RAIL_WIRING),
        ipmb: link,
    });
    con.take_text();
    Rig {
        console,
        con,
        app,
        clock,
    }
}

impl Rig {
    /// text -- the console text since the last call, line ends as LF.
    fn text(&self) -> String {
        self.con.take_text().replace("\r\n", "\n")
    }

    /// type_line -- types `line` and Enter, polls until the console has taken every byte, and
    /// returns the text it printed (echo included) and the restart it returned.
    fn type_line(&mut self, line: &str) -> (String, Option<Restart>) {
        self.con.feed(line);
        self.con.feed("\r");
        let mut restart = None;
        while self.con.rx_pending() > 0 && restart.is_none() {
            restart = self.console.poll();
        }
        (self.text(), restart)
    }

    /// run -- `type_line` for a command that does not restart.
    fn run(&mut self, line: &str) -> String {
        let (text, restart) = self.type_line(line);
        assert!(restart.is_none(), "{line} asked for a restart");
        text
    }
}

/// golden -- the expected transcript: the echo of `line` on its own line, `body`, the prompt.
fn golden(line: &str, body: &str) -> String {
    std::format!("{line}\n{body}CLI> ")
}

/// no_tx_free -- `text` with the transmit-ring room of the UART lines replaced by N (it depends on
/// what was printed before).
fn no_tx_free(text: &str) -> String {
    const KEY: &str = "tx free=";
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(KEY) {
        let (head, tail) = rest.split_at(at + KEY.len());
        out.push_str(head);
        out.push('N');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

#[test]
fn the_banner_is_the_captured_text() {
    let mut rig = rig();
    rig.console.banner();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let tail = "Last reset:   software reset\n\
Clock source: HSE from ST-LINK MCO (bypass), PLL\n\
SYSCLK:       84.00 MHz\n\
HCLK (AHB):   84.00 MHz\n\
PCLK1 (APB1): 42.00 MHz\n\
PCLK2 (APB2): 84.00 MHz\n\
GA raw pins (at start-up): GA4..GA0=11111 GAP=1 NVMRO=1 (1 = high)\n\
GA value:     1 (forced by defs.rs)\n\
HW address:   0x41\n\
IPMB address: 0x82\n\
GA parity:    OK\n\
NVMRO:        deasserted (forced by defs.rs)\n\
Self test:    55h 00h (passed)\n\
BOOTLOADER pin: 0\n\
App port:     SIM 115200 8N1, RTS/CTS off\n\
Type HELP or ? for commands.\n";
    let expected = std::format!(
        "\n{} v0.1.0 (ipmc-core v{})\n\
Image:        v0.1.0, 143456 bytes, SHA-256 bd7855a1c66ee421\n\
Build:        {profile}\n{tail}",
        ipmc_defs::IPMC_FW_NAME,
        ipmc_core::CORE_VERSION,
    );
    assert_eq!(rig.text(), expected);
    rig.console.prompt();
    assert_eq!(rig.text(), "CLI> ");
}

#[test]
fn sys_status_reports_uptime_clocks_led_watchdog_and_uarts() {
    let mut rig = rig();
    rig.clock.advance(2_000);
    rig.console.set_watchdog(Some(4_000));
    let text = no_tx_free(&rig.run("SYS STATUS"));
    let expected = golden(
        "SYS STATUS",
        "Uptime:       0d 00:00:02\n\
Clock source: HSE from ST-LINK MCO (bypass), PLL\n\
SYSCLK:       84.00 MHz\n\
HCLK (AHB):   84.00 MHz\n\
PCLK1 (APB1): 42.00 MHz\n\
PCLK2 (APB2): 84.00 MHz\n\
GA raw pins (at start-up): GA4..GA0=11111 GAP=1 NVMRO=1 (1 = high)\n\
GA value:     1 (forced by defs.rs)\n\
HW address:   0x41\n\
IPMB address: 0x82\n\
GA parity:    OK\n\
NVMRO:        deasserted (forced by defs.rs)\n\
LED (LD2):    heartbeat (auto), 1 Hz\n\
Last reset:   software reset\n\
Watchdog:     on, 4000 ms\n\
CLI UART:     rx overflow=0 tx timeout=0 overrun=0 line errors=0 tx lines dropped=0 tx free=N\n\
APP UART:     rx overflow=0 tx timeout=0 overrun=0 line errors=0 tx lines dropped=0 tx free=N\n\
BOOTLOADER pin: 0\n",
    );
    assert_eq!(text, expected);
    // The same command by its top-level alias.
    let again = no_tx_free(&rig.run("STATUS"));
    assert_eq!(again.replacen("STATUS", "SYS STATUS", 1), text);
}

#[test]
fn sys_version_prints_the_firmware_and_the_image() {
    let mut rig = rig();
    assert_eq!(
        rig.run("SYS VERSION"),
        golden(
            "SYS VERSION",
            &std::format!(
                "{} v0.1.0 (ipmc-core v{})\n\
Image:        v0.1.0, 143456 bytes, SHA-256 bd7855a1c66ee421\n",
                ipmc_defs::IPMC_FW_NAME,
                ipmc_core::CORE_VERSION
            )
        )
    );
}

#[test]
fn the_led_group_sets_and_reports_the_mode() {
    let mut rig = rig();
    let limits =
        "Blink limits: 0.01 to 50 Hz (defs.rs IPMC_LED_RATE_MIN_CHZ, IPMC_LED_RATE_MAX_CHZ)\n";
    // (typed line, what it prints)
    let cases: [(&str, String); 10] = [
        (
            "LED STATUS",
            std::format!("LED (LD2):    heartbeat (auto), 1 Hz\n{limits}"),
        ),
        ("LED ON", "OK: LED on\n".to_string()),
        (
            "LED STATUS",
            std::format!("LED (LD2):    on (manual)\n{limits}"),
        ),
        ("LED OFF", "OK: LED off\n".to_string()),
        ("LED BLINK 2.5", "OK: LED blink 2.50 Hz\n".to_string()),
        (
            "LED STATUS",
            std::format!("LED (LD2):    blink (manual), 2.50 Hz\n{limits}"),
        ),
        (
            "LED BLINK 99",
            "Error: rate must be between 0.01 and 50 Hz\n".to_string(),
        ),
        (
            "LED BLINK x",
            "Error: 'x' is not a number (example: led blink 2.5)\n".to_string(),
        ),
        ("LED AUTO", "OK: LED auto\n".to_string()),
        (
            "LED STATUS",
            std::format!("LED (LD2):    heartbeat (auto), 1 Hz\n{limits}"),
        ),
    ];
    for (line, body) in cases {
        assert_eq!(rig.run(line), golden(line, &body), "{line}");
    }
}

#[test]
fn help_lists_the_groups_and_the_top_commands() {
    let mut rig = rig();
    let rows = [
        "Groups and commands:",
        "  SYS     - System status, reset and version",
        "  LED     - LD2 control",
        "  IPMB    - IPMB address and bus state",
        "  SENSORS - Voltage rails, die temperature, inputs and Tier-1 readings",
        "  POWER   - Payload power enable",
        "  SIM     - Simulated sensor inputs",
        "  FRU     - FRU image, its decoded areas, its fields and the flash log",
        "  DEBUG   - Test switches, the IPMB capture log and local IPMI requests",
        "  STATUS  - Alias of SYS STATUS",
        "  RESET   - Alias of SYS RESET",
        "  HELP    - List commands",
        "  ?       - Same as HELP",
        "Enter a group by name, or type <GROUP> HELP. Full commands work from any level.",
    ];
    assert_eq!(
        rig.run("HELP"),
        golden(
            "HELP",
            &(rows.join(
                "
"
            ) + "
")
        )
    );
}

#[test]
fn fru_show_and_dump_print_the_image() {
    let mut rig = rig();
    let text = rig.run("FRU SHOW");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "FRU SHOW");
    assert!(
        lines[1].starts_with("FRU image: 512 bytes, "),
        "{}",
        lines[1]
    );
    assert!(
        lines[1].ends_with(", kept in the flash log (FRU STORE), NVMRO deasserted"),
        "{}",
        lines[1]
    );
    assert!(lines[2].starts_with("Common header: internal 00h, chassis 00h, board 01h, product "));
    assert!(lines[2].ends_with(", checksum OK"));
    for prefix in [
        "Board area at 08h: ",
        "Product area at ",
        "MultiRecord area at ",
    ] {
        assert!(
            lines.iter().any(|line| line.starts_with(prefix)),
            "no line starts with {prefix}"
        );
    }
    assert_eq!(lines.last(), Some(&"CLI> "));
    // The first bytes of the image: the common header, format 1, no internal or chassis area,
    // the board area at 8 bytes.
    assert_eq!(
        rig.run("FRU DUMP 0 4"),
        golden("FRU DUMP 0 4", "0000: 01 00 00 01\n")
    );
    assert_eq!(
        rig.run("FRU DUMP 600 4"),
        golden(
            "FRU DUMP 600 4",
            "Error: offset 600 count 4 is outside the 512-byte FRU area\n"
        )
    );
}

#[test]
fn debug_status_and_watchdog_print_their_lines() {
    let mut rig = rig();
    let text = rig.run("DEBUG STATUS");
    let lines: Vec<&str> = text.lines().collect();
    let prefixes = [
        "DEBUG STATUS",
        "Capture log:     ",
        "IPMI console:    ",
        "I2C trace:       ",
        "Event receiver:  ",
        "Duplicate cache: 0 entries",
        "CLI> ",
    ];
    assert_eq!(lines.len(), prefixes.len(), "{text}");
    for (line, prefix) in lines.iter().zip(prefixes) {
        assert!(line.starts_with(prefix), "{line} / {prefix}");
    }
    rig.console.set_watchdog(Some(4_000));
    assert_eq!(
        rig.run("DEBUG WATCHDOG"),
        golden("DEBUG WATCHDOG", "Watchdog:     on, 4000 ms\n")
    );
    // A test needs CONFIRM and says what it would do.
    assert_eq!(
        rig.run("DEBUG WATCHDOG HANG"),
        golden(
            "DEBUG WATCHDOG HANG",
            "DEBUG WATCHDOG HANG hangs the main loop, then the watchdog resets the board\n\
Send DEBUG WATCHDOG HANG CONFIRM to run it\n"
        )
    );
}

#[test]
fn ipmb_state_lists_the_buses_the_policy_and_the_link_sensor() {
    let mut rig = rig();
    let text = rig.run("IPMB STATE");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "IPMB STATE");
    assert_eq!(
        lines[1],
        "IPMB-A: fitted, local control, enabled, local status none"
    );
    assert!(lines[2].starts_with("IPMB-B: "));
    assert!(lines[3].starts_with("TX policy: "));
    assert!(lines[3].ends_with(" (from ipmc-defs IPMB_COMPLIANCE)"));
    assert!(lines[4].starts_with("Link sensor data (sensor 1): "));
    assert_eq!(lines[5], "CLI> ");
}

#[test]
fn sys_reset_returns_a_restart_and_no_prompt() {
    let mut rig = rig();
    let (text, restart) = rig.type_line("SYS RESET");
    assert_eq!(text, "SYS RESET\nResetting\n");
    assert_eq!(restart, Some(Restart { boot: None }));
    // The board performs it: both ports flush, the sequence is kept, no boot request is written,
    // the processor resets.
    let (mut kept, mut boot_request, mut system) = ([0u32; 4], [0u32; 4], SimSystem);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rig.console.restart(
            &mut kept,
            &mut boot_request,
            &mut system,
            Restart { boot: None },
        )
    }));
    assert!(outcome.unwrap_err().is::<Reset>(), "the reset ended it");
    assert_eq!((rig.con.flushes(), rig.app.flushes()), (1, 1));
    assert_ne!(kept, [0; 4], "the request sequence state is kept");
    assert_eq!(boot_request, [0; 4]);
}

/// restart_of -- passes of the link step (the clock advancing) until it returns a restart.
fn restart_of(rig: &mut Rig) -> Restart {
    for _ in 0..200 {
        rig.console.service_app();
        if let Some(restart) = rig.console.service_link() {
            return restart;
        }
        rig.clock.advance(10);
    }
    panic!("no restart came");
}

/// reboot -- performs `restart` on fresh words and returns the boot request words it wrote.
fn reboot(rig: &mut Rig, restart: Restart) -> [u32; 4] {
    let (mut kept, mut boot_request, mut system) = ([0u32; 4], [0u32; 4], SimSystem);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rig.console
            .restart(&mut kept, &mut boot_request, &mut system, restart)
    }));
    assert!(outcome.unwrap_err().is::<Reset>(), "the reset ended it");
    boot_request
}

#[test]
fn debug_bootloader_restarts_into_the_bootloader_by_the_console() {
    let mut rig = rig();
    let text = rig.run("DEBUG BOOTLOADER");
    assert_eq!(
        text,
        golden("DEBUG BOOTLOADER", "Restarting into the bootloader\n")
    );
    let restart = restart_of(&mut rig);
    assert_eq!(
        restart,
        Restart {
            boot: Some(Interface::Console)
        }
    );
    assert_eq!(
        reboot(&mut rig, restart),
        request::encode(Interface::Console)
    );
}

#[test]
fn the_application_port_bootloader_command_restarts_into_the_bootloader() {
    let mut rig = rig();
    rig.console.service_app();
    rig.app.feed("BOOTLOADER\r");
    let restart = restart_of(&mut rig);
    let sent = rig.app.take_text();
    assert!(sent.contains("BOOTLOADER: restarting"), "{sent}");
    assert_eq!(
        restart,
        Restart {
            boot: Some(Interface::App)
        }
    );
    assert_eq!(reboot(&mut rig, restart), request::encode(Interface::App));
}

#[test]
fn an_unknown_command_is_reported_by_the_engine() {
    let mut rig = rig();
    let text = rig.run("NOSUCH");
    assert!(text.starts_with("NOSUCH\n"));
    assert!(text.ends_with("CLI> "));
    assert!(text.lines().count() > 2, "the engine says what is wrong");
}

#[test]
fn the_power_sim_and_sensors_commands_run_over_the_sensor_service() {
    let mut rig = rig();
    // (typed line, a line its answer holds)
    let cases: [(&str, &str); 8] = [
        ("POWER ON", "Power: On, PWR_EN on, no fault latched"),
        ("POWER STATUS", "Power: On, PWR_EN on, no fault latched"),
        ("POWER OFF", "Power: Off, PWR_EN off, no fault latched"),
        ("POWER CLEAR", "Error: no fault latched"),
        ("SENSORS TRACE ALL", "Sensor trace: all"),
        ("SENSORS TRACE", "Sensor trace: all"),
        ("SENSORS TRACE LOUD", "Error: 'LOUD' is not a trace level (OFF, EVENTS or ALL)"),
        (
            "SIM TEMP",
            "Not simulated: the die temperature source is hardware (ipmc-defs IPMC_ADC_INTERNAL_SOURCE)",
        ),
    ];
    for (line, expected) in cases {
        let text = rig.run(line);
        assert!(
            text.lines().any(|row| row == expected),
            "{line} did not print {expected}: {text}"
        );
        assert!(text.ends_with("CLI> "), "{line}");
    }
    let sdr = rig.run("SENSORS SDR");
    assert!(sdr.starts_with(
        "SENSORS SDR
Device SDR repository: "
    ));
    assert!(sdr.contains("\"FRU State\""));
}
