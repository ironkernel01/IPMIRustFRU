// run.rs - the start-up order and the main loop of the IPMC firmware, generic over the board.
//
// A board's `main` brings up its hardware (clock, pins, UARTs, sensor hardware, the IPMB master,
// the FRU flash driver), fills in `BoardParts` and `SystemParts` and calls `run`, which is `start`
// and then `pass` for ever (a host test calls those two itself). Together they do what every board
// does the same way: the FRU log start, the IPMB buses, the link, the console banner,
// the sensor start, the prompt, the watchdog start and then the loop (watchdog feed, console,
// application port, sensor sampling, IPMB link, sleep). A processor restart is a value the
// console or the link returns; only `run` hands it to the board's reset (`Console::restart`).
//
// Fail-stop: a master that did not start prints a FATAL line and parks (`SystemControl::park`).
// The evaluation build (`cfg(ipmc_eval)`) asks the board's `eval_expired` once per pass and stops
// the IPMB when it answers true; no other build holds any of it.

use ipmc_boot::header::HEADER_LEN;
use ipmc_core::addr;
use ipmc_hal::board::{Board, Platform};
use ipmc_hal::fault::Fault;
use ipmc_hal::serial::SerialPort;
use ipmc_hal::system::{ClockSummary, ResetCause, Straps, SystemControl, Watchdog};
use ipmc_sensors::board_sensors::RAIL_COUNT;
use ipmc_sensors::hal::RailWiring;

use crate::console::{Console, ConsoleArgs, LinkHome};
use crate::fru_log::FruLog;
use crate::kept::kept_sequence;
use crate::link::{start_buses, IpmbLink, LinkArgs, Restart};
use crate::out;
use crate::sensing::Sensing;

/// StartInfo -- what the board found while it started, for the banner and `SYS STATUS`.
#[derive(Clone, Copy)]
pub struct StartInfo {
    /// Why the processor last reset.
    pub reset_cause: ResetCause,
    /// The fault the previous run kept, reported next to the reset cause.
    pub last_fault: Option<Fault>,
    /// The clock tree after its start-up.
    pub clock: ClockSummary,
    /// Why the application port did not start (the text of the error); None when it started or is
    /// not enabled.
    pub app_uart_fault: Option<&'static str>,
    /// The 64 bytes of the runtime image header in flash, read once at start (the banner and
    /// `SYS VERSION` print the version, length and digest from them).
    pub image_header: [u8; HEADER_LEN],
    /// The firmware version text (the version of the firmware crate).
    pub version: &'static str,
    /// The event generator of an extension build starts on (true) or off (false). False makes the
    /// runtime send itself Set Event Receiver FFh at start-up and print the `Events: off` line.
    /// The bench firmware passes `IPMC_EVENTS_AT_BOOT` (false); the emulator and the conformance
    /// runs pass true (PLAN decision 24).
    pub events_at_boot: bool,
    /// The runtime requests payload power at start-up. The firmware passes `IPMC_POWER_AT_BOOT`;
    /// the emulator passes `EmuConfig::power_on_at_start`.
    pub power_at_boot: bool,
}

/// BoardParts -- the services of one IPMC instance, ready for `run`: each part is the board's own
/// handle, started. A part that failed to start is its error text, printed where the old start-up
/// printed it.
pub struct BoardParts<B: Board> {
    pub start: StartInfo,
    pub clock: B::Clock,
    pub cycles: B::Cycles,
    pub console: B::Console,
    pub app: B::App,
    /// The IPMB buses; Err is the text of the master start failure (FATAL, after the FRU lines).
    pub ipmb: Result<B::Ipmb, &'static str>,
    /// The FRU flash driver; Err is the text of why it did not start.
    pub fru: Result<B::Fru, &'static str>,
    /// The analog inputs: None when no sensor group uses them, Err is the text of the start error.
    pub analog: Option<Result<B::Analog, &'static str>>,
    /// The board I/O: None when no sensor group uses it.
    pub io: Option<B::Io>,
    /// The board's rail wiring: the channel and divider of each rail of the sensor table.
    pub rail_wiring: &'static [RailWiring; RAIL_COUNT],
    pub straps: B::Straps,
    pub led: B::Led,
}

/// SystemParts -- the processor services only a processor build has.
pub struct SystemParts<P: Platform> {
    pub system: P::System,
    pub watchdog: P::Watchdog,
    pub kept_seq: P::KeptSeq,
    pub boot_request: P::BootRequest,
    /// The evaluation period is over (the board owns the timer that measures it).
    #[cfg(ipmc_eval)]
    pub eval_expired: fn() -> bool,
}

/// Runtime -- a started IPMC: the console with the link and the sensor service, and the processor
/// services only the loop uses. `start` builds it, `pass` runs one pass of the main loop, `restart`
/// hands a returned restart to the board; `run` is these three and the endless loop, so a host test
/// runs the same start and the same passes without it.
pub struct Runtime<P: Platform, L: LinkHome<P>> {
    console: Console<P, L>,
    #[cfg(ipmc_eval)]
    port: P::Console,
    system: P::System,
    watchdog: P::Watchdog,
    kept_seq: P::KeptSeq,
    boot_request: P::BootRequest,
    #[cfg(ipmc_eval)]
    eval_expired: fn() -> bool,
    #[cfg(ipmc_eval)]
    ipmb_ended: bool,
}

/// start -- the start-up order; the console has printed the banner and the prompt when it returns.
/// 1. The FRU flash log starts (`FruLog::boot`; it may erase a sector, so it runs before the I2C
///    slaves exist). A master that did not start prints the FATAL line and parks.
/// 2. `start_buses` brings the IPMB buses up for the strap address, the kept request sequence
///    state comes back and `home` places the one `IpmbLink` (None, a second call, stops in a
///    sleep loop).
/// 3. The console: banner (and the application port fault), the events and sensor start, the
///    prompt, then the watchdog start (a failure leaves the board running without it).
#[inline(always)]
pub fn start<P: Platform, L: LinkHome<P>>(
    parts: BoardParts<P>,
    sys: SystemParts<P>,
    home: impl FnOnce(LinkArgs<P>) -> Option<L>,
) -> Runtime<P, L> {
    let BoardParts {
        start,
        clock,
        cycles,
        console: mut port,
        app,
        ipmb,
        fru,
        analog,
        io,
        rail_wiring,
        straps,
        led,
    } = parts;
    let SystemParts {
        system,
        mut watchdog,
        mut kept_seq,
        boot_request,
        #[cfg(ipmc_eval)]
        eval_expired,
    } = sys;
    // FRU flash log: loads the stored image (`None`: the built image stays in use) and erases the
    // store sector when it holds only damage and NVMRO is deasserted.
    let (fru_log, fru_image) = FruLog::boot(fru, straps.nvmro_asserted(), &mut port);
    // IPMB drivers: the bus recovery and the I2C slaves, now that the own address is known. A bus
    // that cannot start prints a line and stays disabled.
    let address = addr::resolve(straps.read_raw());
    let ipmb = match ipmb {
        Ok(ipmb) => ipmb,
        Err(error) => {
            // The master takes the pins and the cycle counter; without it the board cannot
            // transmit.
            out!(&mut port, "\nFATAL: IPMB master: {}\n", error);
            port.flush();
            system.park()
        }
    };
    let buses = start_buses(ipmb, &address, &mut port);
    // The one IpmbLink lives in the board's storage; None would mean `run` was entered twice.
    let Some(link) = home(LinkArgs {
        buses,
        fru_log,
        fru_image,
        kept: kept_sequence(&mut kept_seq, start.reset_cause),
        clock: clock.clone(),
        cycles,
        straps: straps.clone(),
        con: port.clone(),
    }) else {
        loop {
            system.wait_for_interrupt();
        }
    };
    let app_uart_fault = start.app_uart_fault;
    let mut console = Console::new(ConsoleArgs {
        start,
        clock,
        console: port.clone(),
        app,
        led,
        straps,
        sensing: Sensing::new(analog, io, rail_wiring),
        ipmb: link,
    });
    // Banner first: start_sensors may print the first power event line.
    console.banner();
    if let Some(error) = app_uart_fault {
        out!(&mut port, "APP_UART not started: {}\n", error);
    }
    console.start_events();
    console.start_sensors();
    console.prompt();
    // The watchdog starts last, just before the loop. A start failure leaves the board running
    // without it.
    if ipmc_defs::IPMC_WATCHDOG_ENABLE {
        match watchdog.start(ipmc_defs::IPMC_WATCHDOG_MS) {
            Ok(ms) => console.set_watchdog(Some(ms)),
            Err(e) => out!(&mut port, "Watchdog not started: {}\n", e.name()),
        }
    }
    Runtime {
        console,
        #[cfg(ipmc_eval)]
        port,
        system,
        watchdog,
        kept_seq,
        boot_request,
        #[cfg(ipmc_eval)]
        eval_expired,
        #[cfg(ipmc_eval)]
        ipmb_ended: false,
    }
}

impl<P: Platform, L: LinkHome<P>> Runtime<P, L> {
    /// console -- the console, for a host test that types into it.
    pub fn console(&mut self) -> &mut Console<P, L> {
        &mut self.console
    }

    /// console_view -- the console, read only (the host emulator reads the link and the sensors).
    pub fn console_view(&self) -> &Console<P, L> {
        &self.console
    }

    /// kept_seq -- the words that keep the request sequence state across a restart (a host
    /// board starts its next run from them).
    pub fn kept_seq(&self) -> &P::KeptSeq {
        &self.kept_seq
    }

    /// pass -- one pass of the main loop: watchdog feed, console, application port, sensor
    /// service, IPMB link, then the sleep until the next interrupt. A processor restart that the
    /// console or the link asks for ends the pass at once and is returned (`restart` performs it).
    #[inline(always)]
    pub fn pass(&mut self) -> Option<Restart> {
        self.watchdog.feed(); // once per pass, first: the only place the watchdog is fed
        if let Some(restart) = self.console.poll() {
            return Some(restart);
        }
        self.console.service_app();
        self.console.service_sensors();
        #[cfg(not(ipmc_eval))]
        if let Some(restart) = self.console.service_link() {
            return Some(restart);
        }
        #[cfg(ipmc_eval)]
        if !self.ipmb_ended {
            if (self.eval_expired)() {
                self.console.stop_ipmb_for_eval();
                out!(
                    &mut self.port,
                    "Evaluation period ended: IPMB stopped (power cycle to restart)\n"
                );
                self.ipmb_ended = true;
            } else if let Some(restart) = self.console.service_link() {
                return Some(restart);
            }
        } else {
            // A Write FRU Data accepted before the stop is still saved; the save never touches
            // the stopped slaves.
            self.console.service_fru_log();
        }
        self.system.wait_for_interrupt(); // sleeps until the next interrupt (UART byte or 1 ms tick)
        None
    }

    /// restart -- performs the processor restart `pass` returned (both ports flushed, the request
    /// sequence kept, the boot request written, the reset); does not return.
    pub fn restart(&mut self, restart: Restart) -> ! {
        self.console.restart(
            &mut self.kept_seq,
            &mut self.boot_request,
            &mut self.system,
            restart,
        )
    }
}

/// run -- the firmware: `start`, then `pass` for ever; a restart a pass returns goes to the
/// board's reset. Never returns.
pub fn run<P: Platform>(
    parts: BoardParts<P>,
    sys: SystemParts<P>,
    home: fn(LinkArgs<P>) -> Option<&'static mut IpmbLink<P>>,
) -> ! {
    let mut runtime = start(parts, sys, home);
    loop {
        if let Some(restart) = runtime.pass() {
            runtime.restart(restart);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::sim::{self, Reset, Sim};
    use ipmc_fru::RamFlash;
    use std::boxed::Box;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::string::String;

    fn started() -> (Runtime<Sim, Box<IpmbLink<Sim>>>, sim::Handles) {
        let (board, system, handles) = sim::parts(RamFlash::new());
        let runtime = start(board, system, |args| Some(Box::new(IpmbLink::new(args))));
        (runtime, handles)
    }

    fn typed(handles: &sim::Handles) -> String {
        handles.console.take_text().replace("\r\n", "\n")
    }

    #[test]
    fn a_start_prints_the_fru_lines_the_banner_and_the_prompt() {
        let (_runtime, handles) = started();
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let expected = std::format!(
            "FRU log: no stored image, built image in use\n\n{} v0.1.0 (ipmc-core v{})\n\
Image:        v0.1.0, 143456 bytes, SHA-256 bd7855a1c66ee421\n\
Build:        {profile}\n\
Last reset:   software reset\n\
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
Type HELP or ? for commands.\n\
event: power Off -> PowerOnRequested\n\
CLI> ",
            ipmc_defs::IPMC_FW_NAME,
            ipmc_core::CORE_VERSION,
        );
        // The line of the events-at-boot setting comes from the extension; a Tier-1 build has none.
        let events_line = "Events: off from start-up (defs.rs IPMC_EVENTS_AT_BOOT false)\n";
        assert_eq!(typed(&handles).replace(events_line, ""), expected);
    }

    #[test]
    fn passes_run_the_console_and_a_reset_command_ends_in_the_board_reset() {
        let (mut runtime, handles) = started();
        typed(&handles);
        // The first pass runs the sensors: the payload power comes on and the FRU state follows.
        assert!(runtime.pass().is_none(), "an idle pass asks for nothing");
        assert_eq!(
            typed(&handles),
            "event: power PowerOnRequested -> On\nFRU state: M0 IPMC Inactive -> M1 FRU Inactive\n"
        );
        assert!(runtime.pass().is_none());
        assert_eq!(typed(&handles), "");
        handles.console.feed("SYS LED OFF\r");
        while handles.console.rx_pending() > 0 {
            assert!(runtime.pass().is_none());
        }
        assert!(typed(&handles).ends_with("CLI> "));
        handles.console.feed("SYS RESET\r");
        let restart = loop {
            if let Some(restart) = runtime.pass() {
                break restart;
            }
        };
        assert_eq!(restart, Restart { boot: None });
        assert!(
            typed(&handles).ends_with("Resetting\n"),
            "no prompt follows"
        );
        let ended = catch_unwind(AssertUnwindSafe(|| runtime.restart(restart)));
        assert!(ended.is_err_and(|payload| payload.is::<Reset>()));
    }
}
