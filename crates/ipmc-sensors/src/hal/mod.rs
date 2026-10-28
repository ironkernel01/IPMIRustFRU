//! hal - the interfaces the sensor logic uses, and nothing else: no hardware, no logic.
//!
//! A board gets its values through three traits: `analog::AnalogSource` (voltages and the die
//! temperature), `digital::DigitalInputs` (pin levels by name) and `power::PowerSwitch` (the
//! supply enable). `eval/` and `board_sensors.rs` are written against them once; every driver
//! under `drivers/` implements one or more of them. `SourceError` is the single named error all
//! three share, so a failure keeps its meaning from the driver to the console.

pub mod analog;
pub mod digital;
pub mod power;

pub use analog::{AnalogSource, Channel, Divider, RailWiring};
pub use digital::{DigitalInputs, Input};
pub use power::{PowerSwitch, ResetOutput};

/// SourceError -- why a source could not deliver a value. Variants name the fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// No backend answers for this source (selected but not present, or the part does not
    /// measure this quantity).
    Unavailable,
    /// The ADC end-of-conversion flag did not rise within its bound.
    ConversionTimeout,
    /// The factory calibration values are erased or inconsistent.
    CalibrationInvalid,
    /// The VREFINT conversion read zero, so VDDA cannot be derived.
    ReferenceReadZero,
    /// The channel is not one this source owns.
    UnknownChannel,
    /// A transfer on an external I2C or SPI bus failed.
    BusError,
    /// An external part answered with the wrong identification.
    WrongDevice,
    /// A value does not fit the arithmetic range of the conversion.
    Overflow,
    /// A divisor is zero or negative (a rail divider denominator, a calibration span).
    InvalidDivisor,
    /// The part has not finished its first conversion, so it holds no result yet.
    NotReady,
}

impl SourceError {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            SourceError::Unavailable => "source unavailable",
            SourceError::ConversionTimeout => "ADC conversion timeout",
            SourceError::CalibrationInvalid => "calibration values invalid",
            SourceError::ReferenceReadZero => "VREFINT read zero",
            SourceError::UnknownChannel => "unknown channel",
            SourceError::BusError => "bus transfer failed",
            SourceError::WrongDevice => "wrong device identification",
            SourceError::Overflow => "arithmetic overflow",
            SourceError::InvalidDivisor => "divisor is not positive",
            SourceError::NotReady => "no conversion result yet",
        }
    }
}
