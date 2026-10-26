// app_port.rs - the firmware side of the application port (APP_UART, USART1): moves bytes between
// the UART rings and the ipmc-app protocol, and gives that protocol its view of the board.
//
// The protocol (line editor, commands, answers) lives in crates/ipmc-app and never touches
// hardware. This file does the three things the crate leaves to the firmware:
//   1. `AppPort::poll` takes bytes from `uart::getc(Port::App)` and gives each to
//      `ipmc_app::serve`, which appends the echo and, when a line ends, the answer and the prompt
//      to one output buffer. The buffer drains into the UART TX ring a few bytes per pass, with
//      no wait (see "Time" below). The start-up banner goes out through the same buffer.
//   2. `LinkHost` implements `ipmc_app::AppHost` over the IPMB link layer: the dispatch context
//      (addresses, FRU image, FRU state, latest sensor snapshot), the flash log status, the live
//      NVMRO pin, the command names of the capture log and the SysTick time. A new application
//      command that needs more board data adds its method to `AppHost` and its body here.
//   3. Every request line and the final line of every answer go into the capture log as text
//      lines (`app request <line>`, `app answer <line>`), cut as the log cuts any text line.
//
// Time. A person reads the output, so nothing is dropped, and the main loop never waits for the
// UART: an answer of up to ANSWER_MAX (1280 with the default table) bytes is formatted into `out`, then `drain` copies
// what the TX ring takes (`uart::write`, which never waits) and keeps a cursor for the rest. While
// output is pending the port takes no new bytes, so one request is answered at a time and the
// output order is the request order. One pass does at most IPMC_APP_BYTES_PER_POLL bytes of
// input and stops after one answer. Formatting the answer is the work of that pass, estimated
// from the code (not measured on the board; the main session measures it) at up to about 1 ms for
// HELP or FRU FIELDS, while FRU SET and IPMI run their whole work in their pass; the 111 ms the
// longest answer needs on the wire (1280 bytes at 115200 baud) is spent in the UART interrupt,
// not in the main loop. The IPMB service therefore waits at most one such pass per loop turn.
// With flow control on (IPMC_APP_UART_FLOW) and the payload CPU holding CTS closed, the output
// stays pending and the receive ring fills; a full ring then holds the sender off (RTS
// deasserted), so only the byte already in flight can be lost, and the port counts that loss and
// answers the next line end `LINE_DAMAGED`. Without flow control a sender that fills the ring
// loses bytes, with the same answer.
//
// Ownership: the UART driver (bsp/uart.rs) owns USART1 and its rings; this file only reads and
// writes through its functions. The per-unit FRU fields are written through the same path as a
// Write FRU Data from the Chassis Manager: `IpmbLink::set_unit_field` edits the RAM image and
// marks it dirty, and `IpmbLink::service_fru_log` saves it. There is no second writer.
//
// Nothing here runs in an interrupt handler.

use core::fmt;

use ipmc_app::{
    answer_final_line, banner, serve, AnswerBuf, AppError, AppHost, Fed, IpmiData, LineEditor,
    SaveState, Status, ANSWER_MAX,
};
use ipmc_core::capture::Names;
use ipmc_core::fru_state::State;
use ipmc_core::link::LocalError;
use ipmc_fru::{FieldError, FruField};

use crate::bsp::systick;
use crate::bsp::uart::{self, Port};
use crate::debug_cmds::local;
use crate::ipmb_link::IpmbLink;

/// ENABLED -- the application port is built in (ipmc-defs IPMC_APP_UART_ENABLE).
pub const ENABLED: bool = ipmc_defs::IPMC_APP_UART_ENABLE;

/// LINE_MAX -- longest request line, bytes (ipmc-defs IPMC_APP_LINE_MAX).
const LINE_MAX: usize = ipmc_defs::IPMC_APP_LINE_MAX;

/// BYTES_PER_POLL -- most bytes one main-loop pass takes (ipmc-defs IPMC_APP_BYTES_PER_POLL).
const BYTES_PER_POLL: usize = ipmc_defs::IPMC_APP_BYTES_PER_POLL;

/// AppPort -- the line editor, the output buffer and its cursor; the main loop calls `poll` every
/// pass.
pub struct AppPort {
    editor: LineEditor<LINE_MAX>,
    /// `lost_bytes()` at the last poll that looked: a move means bytes were lost since.
    lost_seen: u32,
    /// Output waiting for the TX ring: the echo, the answer, the prompt, or the banner.
    out: AnswerBuf<ANSWER_MAX>,
    /// Bytes of `out` already copied to the TX ring.
    sent: usize,
    /// The banner went out.
    started: bool,
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
            editor: LineEditor::new(),
            lost_seen: 0,
            out: AnswerBuf::new(),
            sent: 0,
            started: false,
        }
    }

    /// poll -- one main-loop pass. The first pass with the port up queues the banner. When the
    /// port lost bytes since the previous pass, discards everything in the UART RX ring and marks
    /// the editor damaged: the next line end is answered `ERROR LINE_DAMAGED` and not run, lines
    /// after it run normally (ipmc-app lib.rs). Output still pending from an earlier pass is
    /// copied to the TX ring first and, while any is left, no new byte is taken. Then takes at
    /// most BYTES_PER_POLL bytes and answers at most one request line: the pass ends after the
    /// answer, so a payload CPU that sends many requests cannot hold the IPMB link service off.
    /// Does nothing when the port is off or did not start.
    pub fn poll(&mut self, link: &mut IpmbLink) {
        if !ENABLED || !uart::is_up(Port::App) {
            return;
        }
        self.poll_pass(link);
        // A Cold Reset asked for from this port waits for its own answer: the link's reset gate
        // counts the output still in `out` or in the TX ring as "not idle".
        link.set_app_output_pending(self.sent < self.out.len() || uart::tx_pending(Port::App));
    }

    /// poll_pass -- the work of one `poll` pass, with the port up.
    fn poll_pass(&mut self, link: &mut IpmbLink) {
        if !self.started {
            self.started = true;
            // The banner fits the empty buffer.
            let _ = banner(&LinkHost { link: &mut *link }, &mut self.out);
        }
        self.check_loss(link);
        if self.drain() {
            return;
        }
        for _ in 0..BYTES_PER_POLL {
            let Some(byte) = uart::getc(Port::App) else {
                break;
            };
            // The UART interrupt can lose a byte while this loop runs. Looking after every byte
            // taken and before it is served: a loss found now discards the byte in hand as well
            // (it can be the line end of the damaged line, or of a line before it, so it is not
            // trusted) and ends the pass. The editor stays damaged until a line end that arrives
            // after the discard, so the suffix of a damaged line never runs as a request.
            if self.check_loss(link) {
                break;
            }
            let served = serve(
                &mut self.editor,
                byte,
                &mut LinkHost { link: &mut *link },
                &mut self.out,
            );
            match &served.fed {
                Fed::Nothing => continue,
                Fed::Empty => {}
                Fed::Line(line) => link.log_text(format_args!("app request {line}")),
                Fed::TooLong => link.log_text(format_args!(
                    "app request (line longer than {LINE_MAX} bytes)"
                )),
                Fed::BadChar => link.log_text(format_args!("app request (not printable ASCII)")),
                Fed::Damaged(lost) => {
                    link.log_text(format_args!("app request (damaged, {lost} bytes lost)"))
                }
            }
            if let Some(line) = answer_final_line(&self.out, served.answer_at) {
                link.log_text(format_args!("app answer {line}"));
            }
            break;
        }
        self.drain();
    }

    /// check_loss -- when the port lost bytes since the last look, discards everything in the UART
    /// RX ring, marks the editor damaged and returns true. The loss is not tied to a line: the
    /// byte can be anywhere in the ring, so every byte waiting is dropped and the line editor
    /// resynchronises at the first line end that arrives afterwards. A caller that holds a byte
    /// taken before the loss was seen drops it too.
    fn check_loss(&mut self, link: &mut IpmbLink) -> bool {
        let lost_now = lost_bytes();
        let moved = lost_now.wrapping_sub(self.lost_seen);
        if moved > 0 {
            uart::discard_rx(Port::App);
            self.editor.damage(moved);
            self.lost_seen = lost_now;
            link.log_text(format_args!("app port lost {moved} bytes, ring discarded"));
        }
        moved > 0
    }

    /// drain -- copies what the TX ring takes of the pending output (never waits); true while
    /// output is left. The buffer is reused once everything is copied.
    fn drain(&mut self) -> bool {
        let rest = self.out.as_bytes().get(self.sent..).unwrap_or(&[]);
        self.sent += uart::write(Port::App, rest);
        if self.sent < self.out.len() {
            return true;
        }
        self.out.clear();
        self.sent = 0;
        false
    }
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

    fn unit_field(&self, f: FruField, out: &mut dyn fmt::Write) -> Result<(), FieldError> {
        ipmc_fru::write_unit_field(self.link.dispatch().fru.bytes(), f, out)
    }

    fn set_unit_field(&mut self, f: FruField, value: &str) -> Result<(), FieldError> {
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

    fn command_name(&self, netfn: u8, cmd: u8) -> Option<&'static str> {
        self.link.names().command(netfn, cmd)
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
