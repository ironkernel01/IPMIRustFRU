//! run - the generic boot sequence (design 6 and 8): start-up trace, strap pins, boot request,
//! image check, start decision, then the start, the countdown or the stay, and the command session
//! on the port that got `WAKEUP`. Everything a board supplies is `BootBoard` and the ipmc-hal
//! handles of `BootParts`; the F401 binary only builds them. Generics only: the `dyn` types are the
//! `TextOut` of the trace and the `BootHw` of the session, as in the pure modules.
//!
//! Output order on the failure paths is the board's: `init_straps` and `start_ports` park on a pin
//! claim conflict after the lines printed before them. No `core::fmt`: every number goes through
//! `text`.

use ipmc_fru::persist::FlashError;
use ipmc_hal::kept::KeptWords;
use ipmc_hal::serial::SerialPort;
use ipmc_hal::system::{led, ResetCause, StatusLed, Straps};
use ipmc_hal::time::{delay_us, Clock, CycleCounter};
use ipmc_menu::{EditEvent, LineEditor};

use crate::decision::{decide, Countdown, Decision, PortId, Reason, Tick};
use crate::header::{check, ImageSource};
use crate::load::FlashOps;
use crate::request::{self, WORDS};
use crate::session::{is_wakeup, Action, BootHw, Session, StartRecord};
use crate::text::{put_dec, SerialText, TextOut, CRLF};
use crate::trace::{self, PortCounts};

/// BACKSPACE_ERASE -- backspace, space, backspace: erases one character on a terminal.
const BACKSPACE_ERASE: &str = "\x08 \x08";
/// MS_PER_S -- milliseconds per second (cycle counts per millisecond are `hz / 1000`).
const MS_PER_S: u32 = 1000;
/// REQUEST_CHECK_WORD -- the index of the check word of the boot request; clearing it makes a
/// later reset read no request.
const REQUEST_CHECK_WORD: usize = WORDS - 1;

/// BootBoard -- what the boot sequence asks of a board beyond the ipmc-hal handles. A cloned
/// console or application port is another handle to the same ring pair.
pub trait BootBoard {
    type Console: SerialPort + Clone;
    type App: SerialPort + Clone;
    type Clock: Clock;
    type Cycles: CycleCounter;
    type Led: StatusLed;
    type Straps: Straps;
    /// The words of the boot request, kept across a software reset.
    type Request: KeptWords<4>;
    type Flash: FlashOps;
    /// init_straps -- starts the cycle counter and claims the strap and BOOTLOADER pins (a claim
    /// conflict parks), after the first trace lines.
    fn init_straps(&mut self);
    /// start_ports -- starts the 1 ms tick and APP_UART when enabled (a pin conflict parks).
    fn start_ports(&mut self);
    /// stop_port -- stops the port that did not get `WAKEUP`.
    fn stop_port(&mut self, port: PortId);
    /// start_runtime -- the hand-over to the runtime (the ports are flushed before).
    fn start_runtime(&mut self) -> !;
    /// system_reset -- restarts the processor.
    fn system_reset(&mut self) -> !;
    /// wait_for_interrupt -- sleeps until the next interrupt.
    fn wait_for_interrupt(&self);
}

/// BootParts -- the handles and start data a board hands to `boot`.
pub struct BootParts<B: BootBoard> {
    pub console: B::Console,
    pub app: B::App,
    pub clock: B::Clock,
    pub cycles: B::Cycles,
    pub led: B::Led,
    pub straps: B::Straps,
    pub request: B::Request,
    pub flash: B::Flash,
    pub cause: ResetCause,
    pub clock_fault: Option<&'static str>,
    /// The bootloader version text the banner and `INFO` print.
    pub version: &'static str,
}

/// Io -- the handles of `BootParts` while the sequence runs.
struct Io<B: BootBoard> {
    console: B::Console,
    app: B::App,
    clock: B::Clock,
    cycles: B::Cycles,
    led: B::Led,
    straps: B::Straps,
    request: B::Request,
    flash: B::Flash,
}

/// Hw -- the session's view of the board: the flash, the clock, the LED activity and the port
/// counters.
struct Hw<'a, B: BootBoard> {
    flash: &'a mut B::Flash,
    clock: &'a B::Clock,
    led: &'a B::Led,
    console: &'a B::Console,
    app: &'a B::App,
}

impl<B: BootBoard> ImageSource for Hw<'_, B> {
    fn read(&self, addr: u32, out: &mut [u8]) {
        self.flash.read(addr, out);
    }
}

impl<B: BootBoard> FlashOps for Hw<'_, B> {
    fn erase_sector(&mut self, number: u8) -> Result<(), FlashError> {
        self.flash.erase_sector(number)
    }

    fn program(&mut self, addr: u32, bytes: &[u8]) -> Result<(), FlashError> {
        self.flash.program(addr, bytes)
    }
}

impl<B: BootBoard> BootHw for Hw<'_, B> {
    fn now_ms(&self) -> u32 {
        self.clock.millis()
    }

    fn activity(&self) {
        self.led.activity();
    }

    fn port_counts(&self, port: PortId) -> PortCounts {
        let c = match port {
            PortId::Cli => self.console.counters(),
            PortId::App => self.app.counters(),
        };
        PortCounts {
            overrun: c.overrun,
            line_errors: c.line_errors,
            rx_overflow: c.rx_overflow,
            tx_timeout: c.tx_timeout,
        }
    }
}

/// Both -- the countdown's output: the console always, APP_UART when it is up.
struct Both<C: SerialPort, A: SerialPort> {
    console: SerialText<C>,
    app: SerialText<A>,
}

impl<C: SerialPort, A: SerialPort> TextOut for Both<C, A> {
    fn put(&mut self, text: &str) {
        self.console.put(text);
        if self.app.0.is_up() {
            self.app.put(text);
        }
    }
}

/// flush -- waits (bounded) until the TX rings of the console and of APP_UART, when it is up, have
/// sent their last byte.
fn flush<C: SerialPort, A: SerialPort>(console: &mut C, app: &mut A) {
    console.flush();
    if app.is_up() {
        app.flush();
    }
}

/// Editors -- one line editor per port. They keep their partial line from the wait into the
/// session, so a `WAKEUP` that ends in CR and a LF that follows are one line end.
struct Editors {
    console: LineEditor,
    app: LineEditor,
}

impl Editors {
    const fn new() -> Self {
        Editors {
            console: LineEditor::new(),
            app: LineEditor::new(),
        }
    }

    /// wakeup -- feeds the bytes waiting on the ports that are up to their editors; returns the
    /// port of the first complete `WAKEUP` line, the console first. Nothing is echoed and no other
    /// line is answered (design 8.1). Bytes after that line stay in the ring (the session reads
    /// them).
    fn wakeup<C: SerialPort, A: SerialPort>(
        &mut self,
        console: &mut C,
        app: &mut A,
    ) -> Option<PortId> {
        if has_wakeup(console, &mut self.console) {
            Some(PortId::Cli)
        } else if has_wakeup(app, &mut self.app) {
            Some(PortId::App)
        } else {
            None
        }
    }
}

/// has_wakeup -- feeds the bytes waiting on `port` to `editor`; true at the first complete
/// `WAKEUP` line.
fn has_wakeup<P: SerialPort>(port: &mut P, editor: &mut LineEditor) -> bool {
    drain(port, editor, &mut Loss::unwatched(), wakeup_line)
}

/// wakeup_line -- true for a complete `WAKEUP` line.
fn wakeup_line(event: EditEvent<'_>, _loss: &mut Loss) -> bool {
    matches!(event, EditEvent::Line(line) if is_wakeup(line))
}

/// Loss -- the loss watch of the session port: the count of lost or damaged bytes seen at the last
/// check, and whether the line in progress is damaged (the same rule as the runtime application
/// port, PLAN decision 37). `watch` is false for the wake-up drain, which only looks for `WAKEUP`.
struct Loss {
    seen: u32,
    damaged: bool,
    watch: bool,
}

impl Loss {
    const fn unwatched() -> Self {
        Loss {
            seen: 0,
            damaged: false,
            watch: false,
        }
    }

    /// watching -- a watch that starts from the current count of `port`.
    fn watching<P: SerialPort>(port: &P) -> Self {
        Loss {
            seen: port.counters().lost(),
            damaged: false,
            watch: true,
        }
    }
}

/// drain -- pushes the bytes waiting on `port` into `editor` and hands each event to `on` (with the
/// loss state, so it can read and clear `damaged`), which returns true to stop. Returns whether
/// `on` stopped it. When a watched port lost bytes since the last check, the RX ring is discarded,
/// the byte in hand is dropped and the line in progress is marked damaged.
fn drain<P: SerialPort>(
    port: &mut P,
    editor: &mut LineEditor,
    loss: &mut Loss,
    mut on: impl FnMut(EditEvent<'_>, &mut Loss) -> bool,
) -> bool {
    while let Some(byte) = port.getc() {
        if loss.watch {
            let now = port.counters().lost();
            if now != loss.seen {
                port.discard_rx();
                loss.seen = now;
                loss.damaged = true;
                continue;
            }
        }
        if on(editor.push_byte(byte), loss) {
            return true;
        }
    }
    false
}

/// countdown -- prints the countdown line on both ports, then one digit a second (`3 2 1`), then
/// CR LF on both ports. Returns the port of a `WAKEUP` line that completed inside the window, or
/// None when the window of `seconds` is over (the start). A `WAKEUP` whose line end arrives after
/// the window is not answered.
fn countdown<B: BootBoard>(
    board: &B,
    io: &mut Io<B>,
    editors: &mut Editors,
    seconds: u32,
) -> Option<PortId> {
    let mut both = Both {
        console: SerialText(io.console.clone()),
        app: SerialText(io.app.clone()),
    };
    trace::countdown(&mut both, seconds, ipmc_defs::IPMC_APP_UART_ENABLE);
    let mut window = Countdown::new(seconds);
    let started = io.clock.millis();
    let mut first = true;
    let chosen = loop {
        let wakeup = editors.wakeup(&mut io.console, &mut io.app);
        match window.step(io.clock.millis().wrapping_sub(started), wakeup) {
            Tick::Digit(digit) => {
                if !first {
                    both.put(" ");
                }
                first = false;
                put_dec(&mut both, digit);
            }
            Tick::Wait => board.wait_for_interrupt(),
            Tick::Start => break None,
            Tick::Session(port) => break Some(port),
        }
    };
    both.put(CRLF);
    chosen
}

/// wait_for_wakeup -- the stay: waits without a time limit for a `WAKEUP` on either port, the LED
/// blinking (the tick drives it).
fn wait_for_wakeup<B: BootBoard>(board: &B, io: &mut Io<B>, editors: &mut Editors) -> PortId {
    loop {
        if let Some(port) = editors.wakeup(&mut io.console, &mut io.app) {
            return port;
        }
        board.wait_for_interrupt();
    }
}

/// start_ports -- the board starts the tick and APP_UART, then the LED blinks the wait pattern.
fn start_ports<B: BootBoard>(board: &mut B, io: &mut Io<B>) {
    board.start_ports();
    io.led
        .set_mode(led::Mode::Blink(ipmc_defs::IPMC_FWU_LED_WAIT_CHZ));
}

/// StartData -- what the board knows at start: the version text, the reset cause and the clock
/// start-up fault.
#[derive(Clone, Copy)]
struct StartData {
    version: &'static str,
    cause: ResetCause,
    clock_fault: Option<&'static str>,
}

/// sequence -- the start-up of design 6.1 from the first trace line to the choice: the trace, the
/// pins, the boot request, the image check, the decision, and the countdown or the stay. Returns
/// None when the runtime starts, or the record of the session with the port that got `WAKEUP`.
fn sequence<B: BootBoard>(
    board: &mut B,
    io: &mut Io<B>,
    editors: &mut Editors,
    data: &StartData,
) -> Option<StartRecord<'static>> {
    let StartData {
        version,
        cause,
        clock_fault,
    } = *data;
    let mut console = SerialText(io.console.clone());

    // The first trace lines.
    trace::banner(&mut console, version, ipmc_defs::FIELD_BUILD);
    trace::reset_cause(&mut console, cause.text());
    if let Some(fault) = clock_fault {
        trace::clock_fault(&mut console, fault);
    }

    // The strap and BOOTLOADER pins, the settle wait, the one read.
    board.init_straps();
    delay_us(&io.cycles, ipmc_defs::IPMC_FWU_PIN_SETTLE_US);
    let pin_high = io.straps.bootloader_pin_high();
    trace::pin(&mut console, pin_high);

    // The boot request words, then the check word is cleared so a later reset reads none; the
    // request counts only after a software reset.
    let words = io.request.read();
    io.request.clear(REQUEST_CHECK_WORD);
    let request = request::decode(words, matches!(cause, ResetCause::Software));
    trace::request(&mut console, request);

    // The image check, timed with the cycle counter (32-bit arithmetic).
    let started = io.cycles.now();
    let report = check(&io.flash);
    let elapsed = io.cycles.now().wrapping_sub(started);
    let ms = elapsed.checked_div(io.cycles.hz() / MS_PER_S).unwrap_or(0);
    trace::runtime_image(&mut console, &report);
    trace::image_check(&mut console, &report, ms);

    // The decision.
    let decision = decide(
        pin_high,
        request,
        report.result,
        ipmc_defs::IPMC_BOOT_COUNTDOWN,
    );
    trace::decision(&mut console, &decision, ipmc_defs::IPMC_APP_UART_ENABLE);

    // Start, countdown then start or session, or stay and wait for a session.
    let (chosen, decision) = match decision {
        Decision::Start => return None,
        Decision::Countdown(seconds) => {
            start_ports(board, io);
            let port = countdown(board, io, editors, seconds)?;
            let decision = Decision::Stay(Reason::WakeupInCountdown);
            trace::decision(&mut console, &decision, ipmc_defs::IPMC_APP_UART_ENABLE);
            (port, decision)
        }
        Decision::Stay(_) => {
            start_ports(board, io);
            (wait_for_wakeup(board, io, editors), decision)
        }
    };
    Some(StartRecord {
        version,
        reset_cause: cause.text(),
        pin_high,
        request,
        decision,
        port: chosen,
        port_name: match chosen {
            PortId::Cli => ipmc_defs::IPMC_CONSOLE_UART_NAME,
            PortId::App => ipmc_defs::IPMC_APP_UART_NAME,
        },
        baud: match chosen {
            PortId::Cli => ipmc_defs::IPMC_CONSOLE_BAUD,
            PortId::App => ipmc_defs::IPMC_APP_UART_BAUD,
        },
        flow: chosen == PortId::App && ipmc_defs::IPMC_APP_UART_FLOW,
        app_enabled: ipmc_defs::IPMC_APP_UART_ENABLE,
    })
}

/// start -- the last line of the start path, then the hand-over to the runtime.
fn start<B: BootBoard>(board: &mut B, io: &mut Io<B>) -> ! {
    trace::starting(&mut SerialText(io.console.clone()));
    flush(&mut io.console, &mut io.app);
    board.start_runtime()
}

/// session_pass -- one pass of the session on `port`: the waiting bytes through the editor into the
/// session (echo while commands are read), then the session's poll. Returns what the caller does
/// next.
#[inline(never)]
fn session_pass<P: SerialPort>(
    port: &mut P,
    out: &mut SerialText<P>,
    editor: &mut LineEditor,
    session: &mut Session,
    loss: &mut Loss,
    hw: &mut dyn BootHw,
    record: &StartRecord<'_>,
) -> Action {
    let mut action = Action::None;
    drain(port, editor, loss, |event, loss| {
        let echo = session.echo();
        match event {
            EditEvent::Echo(byte) if echo => {
                let _ = out.0.write_all_wait(&[byte]);
            }
            EditEvent::Backspace if echo => out.put(BACKSPACE_ERASE),
            EditEvent::Line(line) => {
                if echo {
                    out.put(CRLF);
                }
                if core::mem::take(&mut loss.damaged) {
                    session.line_damaged(&*hw, out);
                } else {
                    action = session.line(line, hw, record, out);
                }
            }
            EditEvent::LineTooLong => {
                if echo {
                    out.put(CRLF);
                }
                if core::mem::take(&mut loss.damaged) {
                    session.line_damaged(&*hw, out);
                } else {
                    session.line_too_long(out);
                }
            }
            _ => {}
        }
        action != Action::None
    });
    session.poll(&*hw, out);
    action
}

/// serve -- the command session on `port`, the port that got the `WAKEUP`. The other port is
/// stopped first (design 8.1). Never returns: `RUN` starts the runtime, `RESET` restarts the
/// processor, anything else keeps the session open (the LED blinks the wait pattern in command
/// mode and shows the activity pattern while records load).
fn serve<B: BootBoard, P: SerialPort + Clone>(
    board: &mut B,
    mut port: P,
    other: PortId,
    editor: &mut LineEditor,
    io: &mut Io<B>,
    record: &StartRecord<'_>,
) -> ! {
    flush(&mut io.console, &mut io.app);
    board.stop_port(other);
    let mut out = SerialText(port.clone());
    let mut session = Session::new();
    session.open(&mut out);
    let mut commands = true;
    let mut loss = Loss::watching(&port);
    loop {
        let action = {
            let mut hw = Hw::<B> {
                flash: &mut io.flash,
                clock: &io.clock,
                led: &io.led,
                console: &io.console,
                app: &io.app,
            };
            session_pass(
                &mut port,
                &mut out,
                editor,
                &mut session,
                &mut loss,
                &mut hw,
                record,
            )
        };
        match action {
            Action::None => {}
            Action::Run => start(board, io),
            Action::Reset => {
                port.flush();
                board.system_reset();
            }
        }
        if session.echo() != commands {
            commands = session.echo();
            io.led.set_mode(if commands {
                led::Mode::Blink(ipmc_defs::IPMC_FWU_LED_WAIT_CHZ)
            } else {
                led::Mode::Auto
            });
        }
        board.wait_for_interrupt();
    }
}

/// boot -- the bootloader, from the first trace line to the start or the session. Never returns.
pub fn boot<B: BootBoard>(mut board: B, parts: BootParts<B>) -> ! {
    let data = StartData {
        version: parts.version,
        cause: parts.cause,
        clock_fault: parts.clock_fault,
    };
    let mut io = Io::<B> {
        console: parts.console,
        app: parts.app,
        clock: parts.clock,
        cycles: parts.cycles,
        led: parts.led,
        straps: parts.straps,
        request: parts.request,
        flash: parts.flash,
    };
    let mut editors = Editors::new();
    let Some(record) = sequence(&mut board, &mut io, &mut editors, &data) else {
        start(&mut board, &mut io)
    };
    match record.port {
        PortId::Cli => {
            let port = io.console.clone();
            let editor = &mut editors.console;
            serve(&mut board, port, PortId::App, editor, &mut io, &record)
        }
        PortId::App => {
            let port = io.app.clone();
            let editor = &mut editors.app;
            serve(&mut board, port, PortId::Cli, editor, &mut io, &record)
        }
    }
}

#[cfg(test)]
mod tests;
