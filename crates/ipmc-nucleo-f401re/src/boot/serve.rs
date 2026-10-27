// serve.rs - the serial side of the bootloader: the two ports before the choice, the WAKEUP rule,
// and the command session on the chosen port (Documentation/ipmc_boot_plan.md 6.5 and 8). Every
// decision is the `ipmc-boot` crate (`is_wakeup`, `Session`, `Countdown`); this file moves bytes
// between the UART rings, the line editors and the session, and performs the `Run` and `Reset`
// the session asks for.
//
// Ownership: this module owns the two `LineEditor`s (one per port) and the session loop. It
// reaches the UARTs only through `bsp::uart` (getc, write_all_wait, flush, stop) and the LED only
// through `bsp::led`. No IPMI code and no flash decision lives here.

use cortex_m::peripheral::{NVIC, SYST};
use ipmc_boot::decision::PortId;
use ipmc_boot::session::{Action, BootHw, Session, StartRecord};
use ipmc_boot::text::{TextOut, CRLF};
use ipmc_boot::trace::PortCounts;
use ipmc_boot::{is_wakeup, trace};
use ipmc_menu::{EditEvent, LineEditor};
use ipmc_nucleo_f401re::bsp::reset_cause;
use ipmc_nucleo_f401re::bsp::uart::{self, Port, PortText, UartError, UartPeripheral};
use ipmc_nucleo_f401re::bsp::{fault, led, systick};
use stm32f4::stm32f401::{RCC, USART1};

use crate::console;
use crate::flash_image::FlashImage;
use crate::start;

/// BACKSPACE_ERASE -- backspace, space, backspace: erases one character on a terminal.
const BACKSPACE_ERASE: &str = "\x08 \x08";

/// Hardware -- what the wait starts: SysTick, and APP_UART when enabled.
pub struct Hardware<'a> {
    pub syst: SYST,
    pub hclk_hz: u32,
    pub usart1: USART1,
    pub rcc: &'a RCC,
    pub nvic: &'a mut NVIC,
    pub pclk2_hz: u32,
}

/// start_ports -- starts SysTick (the 1 ms clock), the LD2 wait blink, and APP_UART when
/// `IPMC_APP_UART_ENABLE` is 1. A port that does not start stays out of the wait (the listener
/// reads a port that is up); a pin conflict is fatal, as in the runtime.
pub fn start_ports(hw: Hardware<'_>) {
    systick::init(hw.syst, hw.hclk_hz);
    led::set_blink_chz(ipmc_defs::IPMC_FWU_LED_WAIT_CHZ);
    if ipmc_defs::IPMC_APP_UART_ENABLE {
        if let Err(UartError::Pins(e)) = uart::init(
            Port::App,
            hw.rcc,
            UartPeripheral::U1(hw.usart1),
            hw.nvic,
            hw.pclk2_hz,
        ) {
            fault::claim_failed(e);
        }
    }
}

/// Editors -- one line editor per port. They keep their partial line from the wait into the
/// session, so a WAKEUP that ends in CR and a LF that follows are one line end.
pub struct Editors {
    console: LineEditor,
    app: LineEditor,
}

impl Editors {
    pub const fn new() -> Self {
        Editors {
            console: LineEditor::new(),
            app: LineEditor::new(),
        }
    }

    fn of(&mut self, port: Port) -> &mut LineEditor {
        match port {
            Port::Console => &mut self.console,
            Port::App => &mut self.app,
        }
    }

    /// wakeup -- feeds the bytes waiting on the ports that are up to their editors; returns the
    /// port of the first complete `WAKEUP` line. Nothing is echoed and no other line is
    /// answered (design 8.1). Bytes after that line stay in the ring (the session reads them).
    pub fn wakeup(&mut self) -> Option<PortId> {
        [(Port::Console, PortId::Cli), (Port::App, PortId::App)]
            .into_iter()
            .find(|&(port, _)| {
                drain(
                    port,
                    self.of(port),
                    &mut Loss::unwatched(),
                    |event, _| matches!(event, EditEvent::Line(line) if is_wakeup(line)),
                )
            })
            .map(|(_, id)| id)
    }
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
    fn watching(port: Port) -> Self {
        Loss {
            seen: lost_bytes(port),
            damaged: false,
            watch: true,
        }
    }
}

/// lost_bytes -- overrun + RX ring overflow + line errors of `port` (wrapping).
fn lost_bytes(port: Port) -> u32 {
    let c = uart::counters(port);
    c.overrun
        .wrapping_add(c.rx_overflow)
        .wrapping_add(c.line_errors)
}

/// drain -- pushes the bytes waiting on `port` into `editor` and hands each event to `on` (with the
/// loss state, so it can read and clear `damaged`), which returns true to stop. Returns whether
/// `on` stopped it. When a watched port lost bytes since the last check, the RX ring is discarded,
/// the byte in hand is dropped and the line in progress is marked damaged.
fn drain(
    port: Port,
    editor: &mut LineEditor,
    loss: &mut Loss,
    mut on: impl FnMut(EditEvent<'_>, &mut Loss) -> bool,
) -> bool {
    while let Some(byte) = uart::getc(port) {
        if loss.watch {
            let now = lost_bytes(port);
            if now != loss.seen {
                uart::discard_rx(port);
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

/// wait_for_wakeup -- the stay: waits without a time limit for a `WAKEUP` on either port, LD2
/// blinking (the SysTick tick drives it).
pub fn wait_for_wakeup(editors: &mut Editors) -> PortId {
    loop {
        if let Some(port) = editors.wakeup() {
            return port;
        }
        cortex_m::asm::wfi();
    }
}

/// The board as the session sees it: the flash, SysTick for the time, and the LED activity.
impl BootHw for FlashImage {
    fn now_ms(&self) -> u32 {
        systick::millis()
    }

    fn activity(&self) {
        led::activity();
    }

    fn port_counts(&self, port: PortId) -> PortCounts {
        let c = uart::counters(match port {
            PortId::Cli => Port::Console,
            PortId::App => Port::App,
        });
        PortCounts {
            overrun: c.overrun,
            line_errors: c.line_errors,
            rx_overflow: c.rx_overflow,
            tx_timeout: c.tx_timeout,
        }
    }
}

/// session -- the command session on the port that got the `WAKEUP`. The other port is stopped
/// first (design 8.1). Never returns: `RUN` starts the runtime, `RESET` restarts the processor,
/// anything else keeps the session open (LD2 blinks the wait pattern in command mode and shows
/// the activity pattern while records load).
pub fn session(
    chosen: PortId,
    editors: &mut Editors,
    flash: &mut FlashImage,
    record: &StartRecord,
    rcc: &RCC,
) -> ! {
    let (port, other) = match chosen {
        PortId::Cli => (Port::Console, Port::App),
        PortId::App => (Port::App, Port::Console),
    };
    console::flush();
    uart::stop(other, rcc);
    let mut out = PortText(port);
    let mut session = Session::new();
    session.open(&mut out);
    let mut commands = true;
    let mut loss = Loss::watching(port);
    loop {
        let mut action = Action::None;
        drain(port, editors.of(port), &mut loss, |event, loss| {
            let echo = session.echo();
            match event {
                EditEvent::Echo(byte) if echo => {
                    let _ = uart::write_all_wait(port, &[byte]);
                }
                EditEvent::Backspace if echo => out.put(BACKSPACE_ERASE),
                EditEvent::Line(line) => {
                    if echo {
                        out.put(CRLF);
                    }
                    if core::mem::take(&mut loss.damaged) {
                        session.line_damaged(flash, &mut out);
                    } else {
                        action = session.line(line, flash, record, &mut out);
                    }
                }
                EditEvent::LineTooLong => {
                    if echo {
                        out.put(CRLF);
                    }
                    if core::mem::take(&mut loss.damaged) {
                        session.line_damaged(flash, &mut out);
                    } else {
                        session.line_too_long(&mut out);
                    }
                }
                _ => {}
            }
            action != Action::None
        });
        session.poll(flash, &mut out);
        match action {
            Action::None => {}
            Action::Run => {
                trace::starting(&mut PortText(Port::Console));
                start::start_runtime();
            }
            Action::Reset => {
                uart::flush(port);
                reset_cause::system_reset();
            }
        }
        if session.echo() != commands {
            commands = session.echo();
            if commands {
                led::set_blink_chz(ipmc_defs::IPMC_FWU_LED_WAIT_CHZ);
            } else {
                led::set_auto();
            }
        }
        cortex_m::asm::wfi();
    }
}
