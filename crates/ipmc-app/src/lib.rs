//! ipmc-app - the application port protocol: what a payload CPU on the application UART can ask
//! the IPMC, and how the IPMC answers.
//!
//! This crate is also the example an engineer extends with their own commands. It carries more
//! prose than the other crates on purpose; read this page top to bottom once and the rest of the
//! crate is easy to follow.
//!
//! # What the port is for
//!
//! The application UART (APP_UART, USART1 on the NUCLEO-F401RE) connects the IPMC to the payload
//! CPU on the same board. Through it the payload CPU
//!
//! - asks for the IPMC status and the FRU state,
//! - runs IPMI requests locally, as if the Chassis Manager had sent them (customer specific
//!   IPMI to payload commands start here), and
//! - reads and writes the thirteen FRU fields (manufacturer, product name, serial
//!   number, part number and file ID of the board; the manufacturing date; manufacturer, name,
//!   part number, version, serial number, asset tag and file ID of the product) at the first boot
//!   of a real system.
//!
//! The FRU image reserves room for those fields (the `IPMC_FRU_*_MAX` defines in ipmi_defs.h),
//! so an update rewrites its field in place and no area moves. The manufacturing date is
//! `YYYY-MM-DD HH:MM` (UTC), 1996-01-01 00:00 to 9999-12-31 23:59. It lives in the VITA 46.11
//! Alternate Manufacturing Date/Time record, because the standard 3-byte field of the FRU board
//! area ends at 2027-11-24 20:15; the standard field stays 000000h (unspecified). The console
//! `FRU` menu reads and writes the same fields through the same path.
//!
//! # The line format
//!
//! Plain ASCII text, one request per line, CR LF or LF at the end of a line. A line holds at
//! most `IPMC_APP_LINE_MAX` bytes. Every request gets exactly one answer line, which starts with
//! `OK` or `ERR <CODE>`. Commands are not case sensitive. A request may start with a tag, `#`
//! and 1 to 8 letters or digits; the answer then starts with the same tag, so a payload CPU
//! that has several requests in flight can match answers to requests.
//!
//! # Lost bytes
//!
//! The UART counts the bytes it loses (overrun, receive ring full). When the count moves, the
//! firmware discards every byte waiting in the receive ring and calls `LineAssembler::damage`;
//! the next line end answers `ERR LINE_DAMAGED <n> bytes lost on the port` (no tag, because the
//! tag may be among the lost bytes), nothing runs for it, and the lines after it run normally.
//! Lines that were complete inside the discarded bytes get no answer at all: a payload CPU
//! treats an answer that does not arrive within its timeout as a lost request and sends it
//! again. A payload that writes a field also compares the `OK <FIELD> <value>` echo of `SET`
//! with what it sent. With `IPMC_APP_UART_FLOW` 1 the IPMC deasserts RTS only when the data
//! register holds an unread byte, so a sender that has a byte in flight can still overrun by one
//! byte; the overrun counter counts it and the line end is answered `ERR LINE_DAMAGED`.
//!
//! Two worked exchanges (`>` is the request, `<` the answer):
//!
//! ```text
//! > HELLO
//! < OK IPMC-APP 1 ipmc-nucleo-f401re v0.1.0 tier=3
//!
//! > #a7 SET BOARD_SERIAL 00:11:22:33:44:55
//! < #a7 OK BOARD_SERIAL 00:11:22:33:44:55
//! ```
//!
//! The commands are `HELP`, `HELLO`, `STATUS`, `FRU STATE`, `GET <field>`, `SET <field> <value>`,
//! `FIELDS`, `IPMI <netfn> <cmd> [data...]` and `SAVE`. `HELP` prints the list from the table
//! (`COMMANDS` in commands.rs), so the table is the one description of what the port accepts.
//!
//! # Where a request goes
//!
//! ```text
//! UART byte -> LineAssembler::push -> respond -> answer -> COMMANDS row -> handler -> AppHost
//!                  (line.rs)         (commands.rs)                (commands.rs)       (host.rs)
//! ```
//!
//! 1. The firmware reads bytes from the UART receive ring and gives each to
//!    `LineAssembler::push`. A complete line comes back as `Fed::Line`.
//! 2. `respond` passes the line to `answer`. A line that was too long or held a non-ASCII byte
//!    gets its own `ERR` answer there, and so does any line that completes after bytes were lost
//!    (`ERR LINE_DAMAGED`; the line does not run).
//! 3. `answer` strips the optional tag, takes the first word as the command and finds the row of
//!    `COMMANDS` with that keyword.
//! 4. The row's handler asks the `AppHost` for whatever it needs and writes the answer line.
//! 5. The firmware writes the answer and CR LF to the UART transmit ring.
//!
//! The crate never touches hardware. Everything it needs from the board comes through the
//! `AppHost` trait (host.rs); the firmware implements the trait in `app_port.rs` over the IPMB
//! link layer, and the tests implement it over a `FruStore` (tests.rs).
//!
//! # Add your own command
//!
//! As an example, `UPTIME` answers `OK UPTIME <seconds>`.
//!
//! 1. Write the handler in commands.rs. A handler has the signature of `Handler`: the request,
//!    the host, the output. It writes one line, `OK ...` or `ERR <CODE> <text>`, and writes
//!    nothing before it knows which.
//!
//!    ```text
//!    /// cmd_uptime -- `UPTIME` answers `OK UPTIME <seconds>`.
//!    fn cmd_uptime(_request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
//!        write!(out, "OK UPTIME {}", host.status().uptime_s)
//!    }
//!    ```
//!
//! 2. Add one row to `COMMANDS` in commands.rs: the keyword in capitals, the argument summary
//!    (empty for none), one sentence of help and the handler. `HELP` lists the row at once.
//!
//!    ```text
//!    AppCommand { keyword: "UPTIME", args: "", help: "seconds since reset", handler: cmd_uptime },
//!    ```
//!
//! 3. If the handler needs a value the host does not offer yet, add a method to `AppHost` in
//!    host.rs, implement it in the firmware (`LinkHost` in
//!    `crates/ipmc-nucleo-f401re/src/app_port.rs`) and in the test host (`FakeHost` in
//!    tests.rs). The compiler lists the places that miss it.
//! 4. Add one row to `REQUEST_TABLE` in tests.rs: the request line and the answer line the fake
//!    host must produce. Run `cargo test -p ipmc-app`.
//! 5. Run `cargo clippy --workspace --all-targets -- -D warnings` and
//!    `python -I tools/dupcheck.py` (a handler that repeats another handler's code fails it).
//!
//! # Files
//!
//! - `host.rs`: `AppHost`, the board as the protocol sees it, and the plain types it passes.
//! - `line.rs`: `LineAssembler` (bytes to lines) and `AnswerBuf` (a fixed text buffer).
//! - `commands.rs`: `COMMANDS`, the handlers, `answer` and `respond`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// commands -- the command table, the handlers and `answer`.
pub mod commands;
/// host -- the `AppHost` trait and the types it passes.
pub mod host;
/// line -- the line assembler and the answer buffer.
pub mod line;

#[cfg(test)]
mod tests;

pub use commands::{
    answer, respond, AppCommand, Handler, Request, ANSWER_MAX, COMMANDS, PROTOCOL_VERSION,
};
pub use host::{AppError, AppHost, IpmiData, SaveState, Status};
pub use line::{AnswerBuf, Fed, LineAssembler};
