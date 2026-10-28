//! Tests of the generic boot sequence over a test board: the trace bytes, the countdown, the WAKEUP
//! choice, the stay, the boot request and the session I/O. `boot` never returns, so a test board
//! ends it by a panic with a `Stop` payload that the runner catches.

use super::*;
use crate::testflash::{good_image, Ram};
use ipmc_hal::serial::SerialCounters;
use ipmc_hal::testing::{FakePort, TestClock, TestCycles, TestLed, TestStraps};
use std::cell::{Cell, RefCell};
use std::format;
use std::panic::{catch_unwind, panic_any, AssertUnwindSafe};
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

/// Stop -- how a test board ends the sequence: the hand-over, a reset, or the idle limit (the
/// sequence would wait for ever).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Start,
    Reset,
    Idle,
}

/// IDLE_LIMIT -- wait-for-interrupt calls a test board allows (1 ms each): more than the countdown
/// needs.
const IDLE_LIMIT: u32 = 20_000;
/// IMAGE_CHECK_MS -- the time the image check reads as: the test cycle counter steps 1000 counts
/// per millisecond.
const IMAGE_CHECK_MS: u32 = 81;

/// Words -- the boot request words in shared memory.
#[derive(Clone)]
struct Words(Rc<Cell<[u32; 4]>>);

impl KeptWords<4> for Words {
    fn read(&self) -> [u32; 4] {
        self.0.get()
    }
    fn write(&mut self, words: [u32; 4]) {
        self.0.set(words);
    }
    fn clear(&mut self, index: usize) {
        let mut words = self.0.get();
        words[index] = 0;
        self.0.set(words);
    }
}

/// TestBoard -- logs the board calls; one wait-for-interrupt moves the clock by 1 ms.
struct TestBoard {
    clock: TestClock,
    events: Rc<RefCell<Vec<&'static str>>>,
    idle: Cell<u32>,
}

impl BootBoard for TestBoard {
    type Console = FakePort;
    type App = FakePort;
    type Clock = TestClock;
    type Cycles = TestCycles;
    type Led = TestLed;
    type Straps = TestStraps;
    type Request = Words;
    type Flash = Ram;

    fn init_straps(&mut self) {
        self.events.borrow_mut().push("init_straps");
    }
    fn start_ports(&mut self) {
        self.events.borrow_mut().push("start_ports");
    }
    fn stop_port(&mut self, port: PortId) {
        self.events.borrow_mut().push(match port {
            PortId::Cli => "stop_console",
            PortId::App => "stop_app",
        });
    }
    fn start_runtime(&mut self) -> ! {
        self.events.borrow_mut().push("start_runtime");
        panic_any(Stop::Start)
    }
    fn system_reset(&mut self) -> ! {
        self.events.borrow_mut().push("system_reset");
        panic_any(Stop::Reset)
    }
    fn wait_for_interrupt(&self) {
        self.idle.set(self.idle.get() + 1);
        if self.idle.get() > IDLE_LIMIT {
            panic_any(Stop::Idle);
        }
        self.clock.advance(1);
    }
}

/// Scenario -- the start-up conditions of one run.
struct Scenario {
    image: Option<Vec<u8>>,
    cause: ResetCause,
    pin_high: bool,
    words: [u32; 4],
    clock_fault: Option<&'static str>,
    console_in: &'static str,
    app_in: &'static str,
    app_up: bool,
}

/// scenario -- a power-on start with a good image, the BOOTLOADER pin low and nothing received.
fn scenario() -> Scenario {
    Scenario {
        image: Some(good_image()),
        cause: ResetCause::PowerOn,
        pin_high: false,
        words: [0; 4],
        clock_fault: None,
        console_in: "",
        app_in: "",
        app_up: true,
    }
}

/// pinned -- the same with the BOOTLOADER pin high (the bootloader stays).
fn pinned() -> Scenario {
    Scenario {
        pin_high: true,
        ..scenario()
    }
}

/// Outcome -- what a run left behind.
struct Outcome {
    stop: Stop,
    console: FakePort,
    app: FakePort,
    events: Vec<&'static str>,
    led: TestLed,
    words: [u32; 4],
    clock: TestClock,
}

fn run(s: Scenario) -> Outcome {
    let console = FakePort::roomy();
    let app = FakePort::roomy();
    console.feed(s.console_in);
    app.feed(s.app_in);
    app.set_up(s.app_up);
    let clock = TestClock::default();
    let cycles = TestCycles::default();
    cycles.set_step(IMAGE_CHECK_MS * 1000);
    let led = TestLed::default();
    let straps = TestStraps::default();
    straps.set_bootloader_pin(s.pin_high);
    let words = Words(Rc::new(Cell::new(s.words)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let board = TestBoard {
        clock: clock.clone(),
        events: events.clone(),
        idle: Cell::new(0),
    };
    let parts = BootParts::<TestBoard> {
        console: console.clone(),
        app: app.clone(),
        clock: clock.clone(),
        cycles,
        led: led.clone(),
        straps,
        request: words.clone(),
        flash: s
            .image
            .map_or_else(Ram::new, |image| Ram::with_image(&image)),
        cause: s.cause,
        clock_fault: s.clock_fault,
        version: "0.1.0",
    };
    let ended = catch_unwind(AssertUnwindSafe(|| boot(board, parts)));
    let payload = ended.expect_err("boot never returns");
    let stop = *payload.downcast_ref::<Stop>().expect("a Stop payload");
    let events = events.borrow().clone();
    Outcome {
        stop,
        console,
        app,
        events,
        led,
        words: words.0.get(),
        clock,
    }
}

const COUNT: u32 = ipmc_defs::IPMC_BOOT_COUNTDOWN;
const PROMPT: &str = ipmc_defs::IPMC_BOOT_PROMPT;
/// START_LINE -- the last line of the start path.
const START_LINE: &str = "Starting the runtime firmware at 0x0800_C000\r\n";

/// digest8 -- the first 8 bytes of the test image's digest as the trace prints them.
fn digest8() -> String {
    let image = good_image();
    let report = crate::header::check(&crate::header::SliceImage {
        base: crate::geometry::APP_START,
        bytes: &image,
    });
    let mut text = String::new();
    crate::text::put_hex_bytes(&mut text, &report.computed.expect("a digest")[..8]);
    text
}

/// the_trace -- the lines every run prints before the decision, for the good test image.
fn the_trace(cause: &str, pin: &str, request: &str) -> String {
    format!(
        "ipmc-boot v0.1.0 (sectors 0-1)\r\n\
Reset cause:    {cause}\r\n\
BOOTLOADER pin: {pin}\r\n\
Request:        {request}\r\n\
Runtime image:  header at 0x0800_C200: found, v0.1.0, 70000 bytes, flags 00h\r\n\
Image check:    SHA-256 {digest} match, {IMAGE_CHECK_MS} ms\r\n",
        digest = digest8()
    )
}

#[test]
fn power_on_with_a_good_image_counts_down_and_starts() {
    let out = run(scenario());
    assert_eq!(out.stop, Stop::Start);
    let digits = (1..=COUNT)
        .rev()
        .map(|digit| format!("{digit}"))
        .collect::<Vec<_>>()
        .join(" ");
    let wait = format!(
        "Send WAKEUP on CLI or APP within {COUNT} s to stay in the bootloader\r\n{digits}\r\n"
    );
    let want = format!(
        "{}Decision:       countdown {COUNT} s for WAKEUP on CLI or APP\r\n{wait}{START_LINE}",
        the_trace("power-on", "0", "none")
    );
    assert_eq!(out.console.text(), want);
    // APP_UART gets the countdown line and the digits, not the trace.
    assert_eq!(out.app.text(), wait);
    assert_eq!(out.events, ["init_straps", "start_ports", "start_runtime"]);
    assert_eq!(
        out.led.mode(),
        led::Mode::Blink(ipmc_defs::IPMC_FWU_LED_WAIT_CHZ)
    );
    assert!(out.clock.millis() >= COUNT * 1000);
    assert!(out.console.flushes() >= 1 && out.app.flushes() >= 1);
}

#[test]
fn a_down_app_port_gets_no_text_and_no_flush() {
    let out = run(Scenario {
        app_up: false,
        ..scenario()
    });
    assert_eq!(out.stop, Stop::Start);
    assert_eq!(out.app.text(), "");
    assert_eq!(out.app.flushes(), 0);
    assert!(out.console.flushes() >= 1);
}

#[test]
fn wakeup_inside_the_countdown_opens_the_session_on_that_port() {
    for (console_in, app_in, on_console, stopped) in [
        ("WAKEUP\r", "", true, "stop_app"),
        ("", "WAKEUP\r", false, "stop_console"),
        ("WAKEUP\r", "WAKEUP\r", true, "stop_app"),
    ] {
        let out = run(Scenario {
            console_in,
            app_in,
            ..scenario()
        });
        assert_eq!(out.stop, Stop::Idle, "the session waits for a command");
        let (talk, other) = if on_console {
            (&out.console, &out.app)
        } else {
            (&out.app, &out.console)
        };
        let said = talk.text();
        assert!(said.ends_with(&format!("OK\r\n{PROMPT}")), "{said:?}");
        assert!(!other.text().ends_with(PROMPT), "the other port is quiet");
        assert_eq!(out.events, ["init_straps", "start_ports", stopped]);
        if on_console {
            assert!(
                said.contains("Decision:       staying in the bootloader: WAKEUP in countdown\r\n")
            );
        }
    }
}

#[test]
fn the_pin_stays_and_only_wakeup_is_answered() {
    let out = run(Scenario {
        console_in: "HELLO\rWAKEUP\r",
        ..pinned()
    });
    let said = out.console.text();
    let head = the_trace("power-on", "1 (forced)", "none");
    let want = format!(
        "{head}Decision:       staying in the bootloader: BOOTLOADER pin, waiting for WAKEUP on CLI or APP\r\nOK\r\n{PROMPT}"
    );
    assert_eq!(
        said, want,
        "no echo and no answer before WAKEUP, no countdown"
    );
    assert_eq!(out.events, ["init_straps", "start_ports", "stop_app"]);
}

#[test]
fn a_stay_waits_for_wakeup_with_no_end_and_takes_it_in_any_case() {
    let silent = run(pinned());
    assert_eq!(silent.stop, Stop::Idle, "no WAKEUP: the wait has no end");
    assert!(silent.clock.millis() >= IDLE_LIMIT);
    let out = run(Scenario {
        app_in: "wakeup\r",
        ..pinned()
    });
    assert!(out.app.text().ends_with(&format!("OK\r\n{PROMPT}")));
    assert_eq!(out.events, ["init_straps", "start_ports", "stop_console"]);
}

#[test]
fn a_request_counts_after_a_software_reset_only_and_is_cleared() {
    let console_request = crate::request::encode(crate::request::Interface::Console);
    let app_request = crate::request::encode(crate::request::Interface::App);
    for (cause, words, request, stays) in [
        (
            ResetCause::Software,
            console_request,
            "bootloader requested by the runtime firmware (console)",
            true,
        ),
        (
            ResetCause::Software,
            app_request,
            "bootloader requested by the runtime firmware (app port)",
            true,
        ),
        (ResetCause::PowerOn, console_request, "none", false),
    ] {
        let out = run(Scenario {
            cause,
            words,
            console_in: "WAKEUP\r",
            ..scenario()
        });
        let said = out.console.text();
        assert!(
            said.contains(&format!("Request:        {request}\r\n")),
            "{said:?}"
        );
        assert_eq!(
            said.contains("staying in the bootloader: requested"),
            stays,
            "{said:?}"
        );
        assert_eq!(out.words[3], 0, "the check word is cleared");
    }
}

#[test]
fn a_missing_image_and_a_clock_fault_are_traced() {
    let out = run(Scenario {
        image: None,
        clock_fault: Some("HSE did not start"),
        console_in: "WAKEUP\r",
        ..scenario()
    });
    let said = out.console.text();
    let mut want = String::new();
    trace::banner(&mut want, "0.1.0", ipmc_defs::FIELD_BUILD);
    trace::reset_cause(&mut want, "power-on");
    trace::clock_fault(&mut want, "HSE did not start");
    assert!(said.starts_with(&want), "{said:?}");
    assert!(said.contains(
        "Decision:       staying in the bootloader: no image, waiting for WAKEUP on CLI or APP\r\n"
    ));
    assert!(!said.contains("Send WAKEUP"));
}

#[test]
fn run_starts_the_runtime_and_reset_restarts() {
    let out = run(Scenario {
        console_in: "WAKEUP\rRUN\r",
        ..pinned()
    });
    assert_eq!(out.stop, Stop::Start);
    let said = out.console.text();
    assert!(said.contains("OK RUN\r\n"), "{said:?}");
    assert!(said.ends_with(START_LINE));
    assert_eq!(
        out.events,
        ["init_straps", "start_ports", "stop_app", "start_runtime"]
    );

    let out = run(Scenario {
        app_in: "WAKEUP\rRESET\r",
        ..pinned()
    });
    assert_eq!(out.stop, Stop::Reset);
    assert!(
        out.app.text().contains("OK RESET\r\n"),
        "{:?}",
        out.app.text()
    );
    assert!(out.app.flushes() >= 1, "the session port is flushed first");
    assert_eq!(out.events.last(), Some(&"system_reset"));
}

#[test]
fn the_session_echoes_commands_and_reports_port_counts() {
    let out = run(Scenario {
        console_in: "WAKEUP\rINFO\r",
        ..pinned()
    });
    let said = out.console.text();
    let session = &said[said.find("OK\r\n").expect("the session opened")..];
    assert!(session.contains("INFO\r\n"), "echoed: {session:?}");
    assert!(session.contains("overrun 0, line 0, rx overflow 0, tx timeout 0"));
}

#[test]
fn wakeup_reads_the_console_first_and_keeps_partial_lines() {
    let (mut console, mut app) = (FakePort::roomy(), FakePort::roomy());
    let mut editors = Editors::new();
    console.feed("WAKE");
    app.feed("hello\rWAKEUP\r");
    assert_eq!(editors.wakeup(&mut console, &mut app), Some(PortId::App));
    console.feed("UP\r");
    app.feed("WAKEUP\r");
    assert_eq!(
        editors.wakeup(&mut console, &mut app),
        Some(PortId::Cli),
        "the partial line was kept and the console is read first"
    );
    assert_eq!(editors.wakeup(&mut console, &mut app), Some(PortId::App));
    assert_eq!(editors.wakeup(&mut console, &mut app), None);
}

#[test]
fn a_watched_port_that_lost_bytes_drops_them_and_marks_the_line() {
    let mut port = FakePort::roomy();
    let mut editor = LineEditor::new();
    port.feed("AB");
    let mut loss = Loss::watching(&port);
    port.set_counters(SerialCounters {
        overrun: 1,
        ..Default::default()
    });
    let mut events = 0;
    let stopped = drain(&mut port, &mut editor, &mut loss, |_, _| {
        events += 1;
        false
    });
    assert!(!stopped);
    assert_eq!(events, 0, "the bytes in the ring are dropped");
    assert!(loss.damaged);
    assert_eq!(loss.seen, 1);
    assert_eq!(port.rx_pending(), 0);
    // An unwatched drain passes every byte on.
    port.feed("AB");
    let mut seen = String::new();
    drain(
        &mut port,
        &mut editor,
        &mut Loss::unwatched(),
        |event, _| {
            if let EditEvent::Echo(byte) = event {
                seen.push(byte as char);
            }
            false
        },
    );
    assert_eq!(seen, "AB");
}

#[test]
fn the_hw_view_reports_the_counters_of_the_asked_port() {
    let console = FakePort::roomy();
    let app = FakePort::roomy();
    app.set_counters(SerialCounters {
        overrun: 1,
        line_errors: 2,
        rx_overflow: 3,
        tx_timeout: 4,
        tx_lines_dropped: 5,
    });
    let (mut flash, clock, led) = (Ram::new(), TestClock::default(), TestLed::default());
    let hw = Hw::<TestBoard> {
        flash: &mut flash,
        clock: &clock,
        led: &led,
        console: &console,
        app: &app,
    };
    assert_eq!(hw.port_counts(PortId::Cli), PortCounts::default());
    assert_eq!(
        hw.port_counts(PortId::App),
        PortCounts {
            overrun: 1,
            line_errors: 2,
            rx_overflow: 3,
            tx_timeout: 4
        }
    );
    clock.advance(7);
    assert_eq!(hw.now_ms(), 7);
}
