//! power.rs - the interface for the supply enable output.

use super::SourceError;

/// ResetOutput -- the payload reset output (PAYLOAD_RST_OUT_N, active low, open drain): the
/// payload is held in reset while it is asserted and released (high impedance) otherwise.
pub trait ResetOutput {
    /// set_reset -- asserts (drives low) or releases the output. Returns the real result of the
    /// write; `Err(Unavailable)` when the output is not claimed (the power group is not hardware).
    fn set_reset(&mut self, asserted: bool) -> Result<(), SourceError>;
}

/// PowerSwitch -- the supply enable output.
pub trait PowerSwitch {
    /// set_enable -- drives the enable output (true = supplies on). Returns the real result of
    /// the write: `Err` when the output could not be driven, in which case `is_enabled` keeps
    /// reporting the last level that was confirmed.
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError>;
    /// is_enabled -- the last confirmed level of the enable output.
    fn is_enabled(&self) -> bool;
}
