//! ipmc-app - the application port protocol: what a payload CPU on the application UART can ask
//! the IPMC, and how the IPMC answers.
//!
//! This crate is also the example an engineer extends with their own commands. It carries more
//! prose than the other crates on purpose; read this page top to bottom once and the rest of the
//! crate is easy to follow. It is self-contained: it does not depend on `ipmc-menu`, and it may
//! repeat code of other crates (PLAN decision 27).
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
//! The FRU image reserves room for those fields (the `IPMC_FRU_*_MAX` defines in defs.rs),
//! so an update rewrites its field in place and no area moves. The manufacturing date is
//! `YYYY-MM-DD HH:MM` (UTC), 1996-01-01 00:00 to 9999-12-31 23:59. It lives in the VITA 46.11
//! Alternate Manufacturing Date/Time record, because the standard 3-byte field of the FRU board
//! area ends at 2027-11-24 20:15; the standard field stays 000000h (unspecified). The console
//! `FRU` menu reads and writes the same fields through the same path.
//!
//! # The format (protocol 2): one text, for a person and a payload CPU
//!
//! There is one mode. A person at a terminal reads it as it is; a payload CPU parses it with
//! three rules. The port is plain ASCII at 115200 baud, 8N1 (`IPMC_APP_UART_BAUD`).
//!
//! - The IPMC prints a start-up banner, one line, then the prompt `app> ` (`PROMPT`). It prints
//!   the prompt again after every answer.
//! - It echoes every printable byte it receives, erases with BS SP BS when it receives backspace
//!   (08h or 7Fh), and takes a request line at CR, LF or CR LF (one line end; no empty second
//!   line). A line is at most `IPMC_APP_LINE_MAX` bytes. A line that is empty prints only the
//!   prompt again.
//! - Every request gets an answer: zero or more data lines, then exactly one final line.
//!   - A data line is `KEY: value`: KEY in capitals and underscores, one space after the colon,
//!     the value as the rest of the line (empty after the colon-space when there is none). A
//!     `FRU FIELDS` line for an empty field is `NAME:  (max n)`: the empty value leaves two
//!     blanks between the colon and the parenthesis. The `HELP` rows are the one exception: the
//!     aligned table, each row starting with two spaces.
//!   - The final line is `OK`, or `ERROR <CODE>: <sentence>` with CODE in capitals and
//!     underscores and the sentence written for a person.
//! - A request may start with a tag, `#` and 1 to 8 letters or digits. The final line carries it
//!   after the `OK` or `ERROR`: `OK #a7`, `ERROR #a7 TOO_LONG: ...`. Data lines carry no tag. A
//!   bad tag, or a line that was lost or could not be read (too long, a byte that is not
//!   printable ASCII, bytes lost on the port), is answered without a tag.
//! - Commands are not case sensitive.
//!
//! The banner is not an answer, so it has no final line:
//! `IPMC application port, <firmware name> v<version>, Tier <n>, protocol <n>. Type HELP or ?.`
//! A parser skips everything before its first request.
//!
//! ## The parsing rule for a payload CPU
//!
//! 1. Send the request line (CR LF at the end).
//! 2. Skip every line that starts with `app> ` (the prompt, and the prompt joined with the echo
//!    of your own line).
//! 3. Collect the lines that look like `KEY: value`.
//! 4. The answer ends at the first line that is `OK`, starts with `OK ` or starts with `ERROR `
//!    (`final_line` is this rule in code). `OK` means the data lines are complete; `ERROR` means
//!    ignore them (the IPMC takes its data lines back when a command fails).
//!
//! A payload that sends several requests without waiting matches answers to requests by the tag
//! of the final line; it must not send a request while the prompt of the previous answer has not
//! arrived if it also wants to keep its own echo straight.
//!
//! # Lost bytes
//!
//! The UART counts the bytes it loses (overrun, receive ring full). When the count moves, the
//! firmware discards every byte waiting in the receive ring and calls `LineEditor::damage`;
//! the next line end answers `ERROR LINE_DAMAGED: <n> bytes lost on the port; the line was
//! dropped` (no tag, because the tag may be among the lost bytes), nothing runs for it, and the
//! lines after it run normally. Lines that were complete inside the discarded bytes get no
//! answer at all: a payload CPU treats an answer that does not arrive within its timeout as a
//! lost request and sends it again. A payload that writes a field also compares the
//! `FRU SET` echo with what it sent. With `IPMC_APP_UART_FLOW` 1 the IPMC deasserts RTS only
//! when the data register holds an unread byte, so a sender that has a byte in flight can still
//! overrun by one byte; the overrun counter counts it and the line end is answered
//! `ERROR LINE_DAMAGED`.
//!
//! # A session
//!
//! `>` is not printed; the text below is what the terminal shows (typed text after `app> `).
//!
//! ```text
//! IPMC application port, ipmc-nucleo-f401re v0.1.0, Tier 1, protocol 2. Type HELP or ?.
//! app> STATUS
//! IPMB_ADDRESS: 82h
//! GA: 1
//! FRU_STATE: M4 FRU Active
//! PAYLOAD_POWER: on
//! HEALTH: ok
//! NVMRO: deasserted
//! UPTIME: 0d 00:17:48
//! OK
//! app> #a7 FRU SET BOARD_SERIAL 1234567890123456789012345
//! ERROR #a7 TOO_LONG: BOARD_SERIAL is at most 24 characters
//! app> FRU SET BOARD_SERIAL 00:11:22:33:44:55
//! BOARD_SERIAL: 00:11:22:33:44:55
//! OK
//! app> IPMI 06 01
//! CC: 00h
//! DATA: 20 01 02
//! COMMAND: App Get Device ID
//! OK
//! app>
//! ```
//!
//! The commands are `HELP`, `?`, `INFO`, `STATUS`, `FRU STATE`, `FRU FIELDS`, `FRU GET`,
//! `FRU SET`, `FRU SAVE` and `IPMI`. `HELP` prints the aligned list from the table (`COMMANDS`
//! in commands.rs), so the table is the one description of what the port accepts.
//!
//! # Where a byte goes
//!
//! ```text
//! UART byte -> serve -> LineEditor::push -> respond -> engine::run -> COMMANDS row -> handler -> AppHost
//!              (commands.rs)  (line.rs)     (commands.rs) (engine.rs)   (commands.rs)             (host.rs)
//! ```
//!
//! 1. The firmware reads bytes from the UART receive ring and gives each to `serve`. `serve`
//!    appends the echo of the byte to the output buffer.
//! 2. The `LineEditor` returns `Fed::Line` when a line ends (and `Empty`, `TooLong`, `BadChar`
//!    or `Damaged` for a line that cannot run). `respond` passes a line to `engine::run` and
//!    gives the other states their `ERROR` line; every answer ends with the prompt.
//! 3. `run` strips the optional tag, finds the row of `COMMANDS` whose keywords the line starts
//!    with (the longest keyword sequence wins), splits the rest into the row's parameters and
//!    calls the handler.
//! 4. The handler asks the `AppHost` for whatever it needs and writes its data lines. It returns
//!    `Ok`, and `run` writes `OK`; or it returns a `Fail`, and `run` takes back the data lines
//!    and writes `ERROR <CODE>: <sentence>`.
//! 5. The firmware copies the output buffer to the UART transmit ring as the ring has room.
//!
//! The crate never touches hardware. Everything it needs from the board comes through the
//! `AppHost` trait (host.rs); the firmware implements the trait in `app_port.rs` over the IPMB
//! link layer, and the tests implement it over a `FruStore` (tests.rs).
//!
//! # Add your own command
//!
//! As an example, `UPTIME` answers `SECONDS: <n>`.
//!
//! 1. Write the handler in commands.rs. A handler has the signature of `Handler`: its
//!    arguments, the host, the output. It writes its data lines with `data` and returns `Ok`,
//!    or returns `Err(Fail::new("CODE", format_args!("sentence")))` before it has written
//!    anything it could not take back.
//!
//!    ```text
//!    /// cmd_uptime -- `UPTIME` answers `SECONDS: <n>`.
//!    fn cmd_uptime(_args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
//!        data(out, "SECONDS", format_args!("{}", host.status().uptime_s))
//!    }
//!    ```
//!
//! 2. Add one row to `COMMANDS` in commands.rs: the syntax string, one sentence of help and the
//!    handler. The syntax is the keywords in capitals (one or more words), then `[NAME]` for a
//!    required parameter, `{NAME}` for an optional one and `{NAME...}` for the rest of the line
//!    as one value (last). Optional parameters come last; two rows may not have the same
//!    keywords (the test `the_command_table_is_well_formed` rejects a bad row). A handler reads
//!    its parameters with `args.get(i)` (optional) or `args.required(i)?`, `i` counting from the
//!    first parameter after the keywords. `HELP` lists the row at once, aligned with the rest.
//!
//!    ```text
//!    Row { syntax: "UPTIME", help: "Seconds since reset", handler: cmd_uptime },
//!    ```
//!
//! 3. If the handler needs a value the host does not offer yet, add a method to `AppHost` in
//!    host.rs, implement it in the firmware (`LinkHost` in
//!    `crates/ipmc-nucleo-f401re/src/app_port.rs`) and in the test host (`FakeHost` in
//!    tests.rs). The compiler lists the places that miss it.
//! 4. Add one row to `REQUEST_TABLE` in tests.rs: the request line and the answer the fake host
//!    must produce (`SECONDS: 1068` then `OK`). The test for the HELP table text (`HELP_TABLE`)
//!    gets the new row too. Run `cargo test -p ipmc-app`.
//! 5. Run `cargo clippy --workspace --all-targets -- -D warnings` and
//!    `python -I tools/dupcheck.py` (a handler that repeats another handler's code fails it).
//!
//! # Files
//!
//! - `host.rs`: `AppHost`, the board as the protocol sees it, and the plain types it passes.
//! - `line.rs`: `LineEditor` (bytes to lines, with echo) and `AnswerBuf` (a fixed text buffer).
//! - `engine.rs`: the command engine: `Row`, syntax strings, matching, `Args`, `Fail`, `HELP`,
//!   the final line, `run`.
//! - `commands.rs`: `COMMANDS`, the handlers, the banner, `respond` and `serve`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
// `Fail` holds its sentence inline (no allocator, one fixed buffer); it is returned only on the
// failure path of a command, never in a loop, so its size costs nothing.
#![allow(clippy::result_large_err)]

/// commands -- the command table, the handlers, the banner and the byte-to-answer step.
pub mod commands;
/// engine -- the command engine: syntax strings, matching, arguments, HELP, the final line.
pub mod engine;
/// host -- the `AppHost` trait and the types it passes.
pub mod host;
/// line -- the line editor and the answer buffer.
pub mod line;

#[cfg(test)]
mod tests;

pub use commands::{answer_final_line, banner, respond, serve, Served, COMMANDS, PROTOCOL_VERSION};
pub use engine::{check_table, final_line, run, Args, Fail, Handler, Row, ANSWER_MAX, END, PROMPT};
pub use host::{AppError, AppHost, IpmiData, SaveState, Status};
pub use line::{AnswerBuf, Echo, Edit, Fed, LineEditor};
