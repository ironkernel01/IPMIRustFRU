//! ipmc-boot - the library of the bootloader (sectors 0-1): everything it decides, host-tested.
//!
//! - `geometry`: RM0368 Table 5 sectors, the region policy of `ERASE` and `LOAD`.
//! - `header`: the runtime image header, the one image check and the build-time patch.
//! - `request`: the boot request words that cross a software reset.
//! - `decision`: the start decision and the boot countdown.
//! - `text`: numbers as text without `core::fmt` (the bootloader must fit 32 KB).
//! - `trace`: the start-up trace lines.
//! - `hex`: the Intel HEX record parser.
//! - `error`: the `ERR` codes of the session.
//! - `load`: erase and the `LOAD` state machine over a `FlashOps` trait.
//! - `commands`: the small command table of the session, its help text and the lookup.
//! - `session`: the session state machine (commands, erase, `LOAD`, `RUN`) over a `BootHw` trait.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod commands;
pub mod decision;
pub mod error;
pub mod geometry;
pub mod header;
pub mod hex;
pub mod load;
pub mod request;
pub mod session;
#[cfg(test)]
mod testflash;
pub mod text;
pub mod trace;

pub use commands::{find, Cmd};
pub use decision::{decide, Countdown, Decision, PortId, Reason, Tick};
pub use error::{put_err, BootError};
pub use geometry::{load_allowed, Sector, Target};
pub use header::{
    check, digest, patch, Header, HeaderField, ImageError, ImageSource, PatchError, Report,
    SliceImage,
};
pub use hex::Record;
pub use load::{erase, FlashOps, LoadStep, Loader};
pub use request::Interface;
pub use session::{is_wakeup, Action, BootHw, Session, StartRecord};
pub use text::TextOut;
