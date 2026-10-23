//! power.rs - the interface for the supply enable output.

use super::SourceError;

/// PowerSwitch -- the supply enable output.
pub trait PowerSwitch {
    /// set_enable -- drives the enable output (true = supplies on). Returns the real result of
    /// the write: `Err` when the output could not be driven, in which case `is_enabled` keeps
    /// reporting the last level that was confirmed.
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError>;
    /// is_enabled -- the last confirmed level of the enable output.
    fn is_enabled(&self) -> bool;
}
