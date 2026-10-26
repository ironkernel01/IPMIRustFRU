//! host.rs - what the application protocol needs from the board.
//!
//! `AppHost` is the only door between the protocol code (`commands.rs`) and the rest of the
//! firmware. The protocol never reads a sensor, a flash page or an IPMB frame itself: it asks the
//! host. The firmware implements the trait over its link layer (`app_port.rs` in the firmware
//! crate); the tests implement it over a plain `FruStore` (`tests.rs`). A new command that needs
//! board data adds one method here, one implementation in each of those two places.

use core::fmt;

use ipmc_core::fru_state::State;
use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::sensor::{PowerState, Summary};
use ipmc_fru::{FieldError, FruField};

/// Status -- the values the `STATUS` answer reports, read at the moment of the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// The IPMB address of this controller in 8-bit form (40h + the geographic address * 2 for a
    /// VPX slot; `82h` for GA 1).
    pub ipmb_addr: u8,
    /// The geographic address, 0..=31.
    pub ga: u8,
    /// The payload power state machine.
    pub power: PowerState,
    /// The board health summary (the FRU Health sensor).
    pub health: Summary,
    /// The NVMRO pin is asserted: the FRU is write protected.
    pub nvmro: bool,
    /// Seconds since reset.
    pub uptime_s: u32,
}

/// SaveState -- whether the FRU image in RAM is in the flash log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    /// The flash log holds the image in RAM: nothing waits to be saved.
    Saved,
    /// A change waits to be saved (the quiet time, a quiet bus, or NVMRO clearing).
    Pending,
    /// The flash log is off: the image lives in RAM only and a reset restores the built one.
    LogOff,
}

/// AppError -- why a local IPMI request produced no response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppError {
    /// The request does not fit one IPMB frame.
    TooLong,
    /// The dispatcher produced no response (a NetFn that is odd or above 3Eh, for one).
    Dispatch,
}

impl AppError {
    /// name -- the word that follows `ERR IPMI_FAILED` in the answer.
    pub const fn name(self) -> &'static str {
        match self {
            AppError::TooLong => "TOO_LONG",
            AppError::Dispatch => "DISPATCH",
        }
    }
}

/// IpmiData -- the data bytes of a response, after the completion code: at most one IPMB frame.
#[derive(Clone, Copy, Debug)]
pub struct IpmiData {
    bytes: [u8; IPMB_MAX_LEN],
    len: usize,
}

impl IpmiData {
    /// empty -- a response with no data bytes.
    pub const fn empty() -> IpmiData {
        IpmiData {
            bytes: [0; IPMB_MAX_LEN],
            len: 0,
        }
    }

    /// from_slice -- the data bytes in `data`; None when they do not fit one IPMB frame.
    pub fn from_slice(data: &[u8]) -> Option<IpmiData> {
        let mut out = IpmiData::empty();
        out.bytes.get_mut(..data.len())?.copy_from_slice(data);
        out.len = data.len();
        Some(out)
    }

    /// as_bytes -- the data bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }
}

/// AppHost -- the board, as the application protocol sees it. Every method runs in the main
/// loop, never in an interrupt, and returns without waiting for hardware.
pub trait AppHost {
    /// version -- the firmware version text for `HELLO` (the firmware crate's package version).
    fn version(&self) -> &'static str;

    /// status -- the board values of the `STATUS` answer. The firmware reads the geographic
    /// address and IPMB address from the dispatch context, the power state and health from the
    /// latest sensor snapshot, the live NVMRO pin and the SysTick seconds.
    fn status(&self) -> Status;

    /// fru_state -- the Operational State of FRU 0 (M0, M1, M4, M5 or M6), as the FRU state
    /// machine holds it.
    fn fru_state(&self) -> State;

    /// unit_field -- writes the value of `f` as stored in the FRU image to `out`: the text of a
    /// text field (nothing for an empty one), `YYYY-MM-DD HH:MM` for the date. The firmware reads
    /// the RAM copy of the FRU image, the one a Chassis Manager reads with Read FRU Data.
    fn unit_field(&self, f: FruField, out: &mut dyn fmt::Write) -> Result<(), FieldError>;

    /// set_unit_field -- writes `value` into the field `f` of the FRU image. The firmware
    /// refuses while NVMRO is asserted (`FieldError::Nvmro`), edits the RAM image in place
    /// (`FruStore::set_unit_field`) and reruns the self test. The change reaches the flash log
    /// through the same dirty mark and save delay as a Write FRU Data from the Chassis Manager.
    fn set_unit_field(&mut self, f: FruField, value: &str) -> Result<(), FieldError>;

    /// ipmi -- runs one IPMI request locally, as if the Chassis Manager had sent it, and returns
    /// the completion code with the response data. Nothing goes on the IPMB. The firmware uses
    /// `IpmbLink::local_request`, the function behind the console command `DEBUG IPMI`.
    fn ipmi(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<(u8, IpmiData), AppError>;

    /// save_state -- whether the FRU image in RAM is in the flash log. The firmware reads the
    /// store's dirty mark and the flash log's pending flag.
    fn save_state(&self) -> SaveState;
}
