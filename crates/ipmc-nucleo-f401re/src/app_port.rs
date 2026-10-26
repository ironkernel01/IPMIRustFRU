// app_port.rs - the firmware side of the application port (APP_UART, USART1): moves bytes between
// the UART rings and the ipmc-app protocol, and gives that protocol its view of the board.
//
// The protocol (line format, commands, answers) lives in crates/ipmc-app and never touches
// hardware. This file does the three things the crate leaves to the firmware:
//   1. `AppPort::poll` takes bytes from `uart::getc(Port::App)`, feeds the line assembler and,
//      for one complete line per pass, runs `ipmc_app::respond` and writes the answer plus CR LF
//      with `uart::write(Port::App, ..)`. It never waits: an answer the TX ring cannot take
//      whole is dropped and counted (`answers_dropped`, shown by SYS STATUS and DEBUG STATUS).
//   2. `LinkHost` implements `ipmc_app::AppHost` over the IPMB link layer: the dispatch context
//      (addresses, FRU image, FRU state, latest sensor snapshot), the flash log status, the live
//      NVMRO pin and the SysTick time. A new application command that needs more board data adds
//      its method to `AppHost` and its body here.
//   3. Every request line and every answer line goes into the capture log as a text line
//      (`app request <line>`, `app answer <line>`), cut as the log cuts any text line.
//
// Ownership: the UART driver (bsp/uart.rs) owns USART1 and its rings; this file only reads and
// writes through its functions. The per-unit FRU fields are written through the same path as a
// Write FRU Data from the Chassis Manager: `IpmbLink::set_unit_field` edits the RAM image and
// marks it dirty, and `IpmbLink::service_fru_log` saves it. There is no second writer.
//
// Nothing here runs in an interrupt handler.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering};

use ipmc_app::{
    AnswerBuf, AppError, AppHost, Fed, IpmiData, LineAssembler, SaveState, Status, ANSWER_MAX,
};
use ipmc_core::fru_state::State;
use ipmc_core::link::LocalError;
use ipmc_fru::{FieldError, UnitField};

use crate::bsp::systick;
use crate::bsp::uart::{self, Port};
use crate::debug_cmds::local;
use crate::ipmb_link::IpmbLink;

/// ENABLED -- the application port is built in (ipmi_defs.h IPMC_APP_UART_ENABLE).
pub const ENABLED: bool = ipmc_defs::IPMC_APP_UART_ENABLE == 1;

/// LINE_MAX -- longest request line, bytes (ipmi_defs.h IPMC_APP_LINE_MAX).
const LINE_MAX: usize = ipmc_defs::IPMC_APP_LINE_MAX as usize;

/// BYTES_PER_POLL -- most bytes one main-loop pass takes (ipmi_defs.h IPMC_APP_BYTES_PER_POLL).
const BYTES_PER_POLL: u32 = ipmc_defs::IPMC_APP_BYTES_PER_POLL;

/// CRLF -- the end of every answer line.
const CRLF: &[u8] = b"\r\n";

/// ANSWERS_DROPPED -- answers the TX ring had no room for. Only the main loop writes it.
static ANSWERS_DROPPED: AtomicU32 = AtomicU32::new(0);

/// answers_dropped -- answers dropped since reset (SYS STATUS, DEBUG STATUS).
pub fn answers_dropped() -> u32 {
    ANSWERS_DROPPED.load(Ordering::Relaxed)
}

/// AppPort -- the line assembler of the port; the main loop calls `poll` every pass.
pub struct AppPort {
    assembler: LineAssembler<LINE_MAX>,
    /// Bytes the port had lost (UART overrun plus RX ring overflow) when the previous line ended.
    /// The baseline is taken at the end of the previous line, not at the first byte of this one,
    /// so a lost first byte of a line is counted against that line.
    lost_at_line_start: u32,
}

/// lost_bytes -- bytes the application port has lost since reset (overrun and RX overflow).
fn lost_bytes() -> u32 {
    let c = uart::counters(Port::App);
    c.overrun.wrapping_add(c.rx_overflow)
}

impl AppPort {
    /// new -- an idle port.
    pub const fn new() -> Self {
        AppPort {
            assembler: LineAssembler::new(),
            lost_at_line_start: 0,
        }
    }

    /// poll -- one main-loop pass. Takes at most BYTES_PER_POLL bytes from the UART RX ring and
    /// answers at most one request line: the pass ends after the answer, so a payload CPU that
    /// sends many requests cannot hold the IPMB link service off. Does nothing when the port is
    /// off or did not start.
    pub fn poll(&mut self, link: &mut IpmbLink) {
        if !ENABLED || !uart::is_up(Port::App) {
            return;
        }
        for _ in 0..BYTES_PER_POLL {
            let Some(byte) = uart::getc(Port::App) else {
                return;
            };
            let fed = self.assembler.push(byte);
            match &fed {
                Fed::Nothing => continue,
                Fed::Line(line) => link.log_text(format_args!("app request {line}")),
                Fed::TooLong => link.log_text(format_args!(
                    "app request (line longer than {LINE_MAX} bytes)"
                )),
                Fed::BadChar => link.log_text(format_args!("app request (not printable ASCII)")),
            }
            // Bytes lost since the previous line ended: the line is answered, not run (R4).
            let lost = lost_bytes().wrapping_sub(self.lost_at_line_start);
            let mut out = AnswerBuf::<ANSWER_MAX>::new();
            ipmc_app::respond(fed, lost, &mut LinkHost { link: &mut *link }, &mut out);
            self.lost_at_line_start = lost_bytes();
            if out.overflowed() {
                out.clear();
                // The fixed text fits the empty buffer.
                let _ = out.write_str("ERR ANSWER_TOO_LONG");
            }
            link.log_text(format_args!("app answer {}", out.as_str()));
            send(out.as_str());
            return;
        }
    }
}

/// send -- writes `answer` and CR LF to the TX ring when both fit; otherwise nothing is written
/// and the drop is counted (a partial answer would confuse the payload CPU).
fn send(answer: &str) {
    let needed = answer.len() + CRLF.len();
    if uart::tx_free(Port::App) < needed {
        ANSWERS_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    uart::write(Port::App, answer.as_bytes());
    uart::write(Port::App, CRLF);
}

/// LinkHost -- the board as `ipmc-app` sees it, over the IPMB link layer.
struct LinkHost<'a> {
    link: &'a mut IpmbLink,
}

impl AppHost for LinkHost<'_> {
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn status(&self) -> Status {
        let dispatch = self.link.dispatch();
        Status {
            ipmb_addr: dispatch.addr.ipmb_address,
            ga: dispatch.addr.ga,
            power: dispatch.sensors.power,
            health: dispatch.sensors.health,
            nvmro: crate::bsp::ga::nvmro_asserted(),
            uptime_s: systick::seconds(),
        }
    }

    fn fru_state(&self) -> State {
        self.link.dispatch().fru_state.state()
    }

    fn unit_field(&self, f: UnitField, out: &mut dyn fmt::Write) -> Result<(), FieldError> {
        ipmc_fru::write_unit_field(self.link.dispatch().fru.bytes(), f, out)
    }

    fn set_unit_field(&mut self, f: UnitField, value: &str) -> Result<(), FieldError> {
        self.link.set_unit_field(f, value)
    }

    fn ipmi(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<(u8, IpmiData), AppError> {
        let reply = local(self.link, netfn, cmd, data).map_err(|error| match error {
            LocalError::TooLong => AppError::TooLong,
            LocalError::Dispatch => AppError::Dispatch,
        })?;
        let data = IpmiData::from_slice(reply.data()).ok_or(AppError::TooLong)?;
        Ok((reply.completion_code(), data))
    }

    fn save_state(&self) -> SaveState {
        if self.link.fru_log_status().is_none() {
            SaveState::LogOff
        } else if self.link.fru_unsaved() {
            SaveState::Pending
        } else {
            SaveState::Saved
        }
    }
}
