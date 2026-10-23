// lib.rs - ipmc-menu: command-menu engine (MenuBuilder rules in Rust) shared by every command line.
//
// The engine holds no hardware and no heap. The caller supplies the command tables, a context
// value handed to every handler, and a `MenuOutput` sink. Byte input goes through `LineEditor`;
// complete lines go to `Menu::process_line`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod args;
mod line;
mod menu;
mod syntax;

pub use args::{optional, ArgError, Args};
pub use line::{report_line_error, EditEvent, LineEditor, MenuError};
pub use menu::{Group, Menu, Outcome};
pub use syntax::{Kind, SynToken, TableError};

/// MENU_LINE_MAX -- longest accepted input line in bytes (ipmi_defs.h IPMC_MENU_LINE_MAX). The firmware RX ring
/// (IPMC_UART_RX_BUF) is sized against this value: ipmc-defs guards that a line that fills it
/// still fits. A longer line is rejected with `MenuError::LineTooLong`.
pub const MENU_LINE_MAX: usize = ipmc_defs::IPMC_MENU_LINE_MAX as usize;

/// MENU_TOKENS_MAX -- most whitespace-separated tokens in one line, and in one syntax string
/// (ipmi_defs.h IPMC_MENU_TOKENS_MAX). A line with more is
/// rejected with `MenuError::TooManyTokens`; a syntax with more fails table validation
/// (`TableError::SyntaxTooLong`), because no input could ever match it.
pub const MENU_TOKENS_MAX: usize = ipmc_defs::IPMC_MENU_TOKENS_MAX as usize;

/// CmdResult -- result of a command handler. `Ok` means the command ran; `Err` names why it did not.
pub type CmdResult = Result<(), CmdError>;

/// CmdError -- why a handler stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdError {
    /// A parameter was missing or malformed; the engine prints the fault and the usage line.
    Arg(ArgError),
    /// The handler already printed its own message; the engine prints nothing more.
    Reported,
}

impl From<ArgError> for CmdError {
    fn from(e: ArgError) -> Self {
        CmdError::Arg(e)
    }
}

/// MenuOutput -- output sink supplied by the caller. `\n` is a line end; the sink maps it to its line
/// terminator. The engine never assumes a UART.
pub trait MenuOutput {
    fn write_str(&mut self, s: &str);
}

/// Command -- one command: MenuBuilder `FUNCTION:COMMAND:DESCRIPTION`. `handler` is FUNCTION, `syntax` is
/// COMMAND and `help` is DESCRIPTION.
///
/// `syntax` is whitespace-separated: keywords (plain text, case-insensitive, exact),
/// `[REQUIRED]` parameters, `{OPTIONAL}` parameters, optional ones last. The first token is a
/// keyword. In a group table the syntax is relative to the group (`ON`, not `LED ON`).
pub struct Command<C> {
    pub syntax: &'static str,
    pub help: &'static str,
    pub handler: fn(&mut C, &Args<'_>) -> CmdResult,
}
