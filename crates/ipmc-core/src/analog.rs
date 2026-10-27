//! analog.rs - the hook for analog sensors: Full Sensor Records and readings a build adds to the
//! mandatory sensors.
//!
//! `ipmc-core` decides nothing about a rail or a temperature: the board crate (`ipmc-sensors`
//! `analog.rs`) owns each analog sensor's record, conversion and thresholds beside the board's
//! measurement definitions, and the firmware and the emulator hand it to the `Context`
//! (`Context::with_analog`). The SDR repository (`SdrRepository::with_analog`) inserts the records
//! after the compact ones, Get Sensor Reading (`Context::read_sensor_at`) asks `read` for the
//! numbers from `ANALOG_FIRST`. The default, `NoAnalog`, keeps the mandatory sensors alone (the
//! pattern of `ext::Extension`: a trait the core calls, an implementation in another crate).

use crate::sdr::Record;
use crate::sensor::{EncodeError, SensorSnapshot};

/// ANALOG_FIRST -- the number of the first analog sensor (defs.rs IPMC_SDR_ANALOG_FIRST): the
/// analog sensors follow the VITA 46.11 mandatory sensors 0..=7.
pub const ANALOG_FIRST: u8 = ipmc_defs::IPMC_SDR_ANALOG_FIRST;
/// ANALOG_COUNT_MAX -- the most analog sensors the build can publish (defs.rs
/// IPMC_SDR_ANALOG_SENSORS and the board's rail count); sizes the SDR repository and the event
/// enables.
pub const ANALOG_COUNT_MAX: usize = ipmc_defs::SDR_ANALOG_COUNT;

/// AnalogSensors -- the analog sensors of a build. Sensor `index` (0..`count`) has the number
/// `ANALOG_FIRST + index`.
pub trait AnalogSensors: Sync {
    /// count -- how many analog sensors there are (at most `ANALOG_COUNT_MAX`).
    fn count(&self) -> usize;

    /// record -- the Full Sensor Record of sensor `index`, with record ID `id`, owner
    /// `owner_address` (the 8-bit IPMB address) and the sensor number in it; None for an index past
    /// the last sensor.
    fn record(&self, index: usize, owner_address: u8, id: u16) -> Option<Record>;

    /// read -- the Get Sensor Reading data bytes (reading, status, threshold comparison bits) of
    /// sensor `index` from the latest `snapshot`, written to the front of `out`; the byte count.
    /// Err(UnsupportedSensor) for an index past the last sensor.
    fn read(
        &self,
        index: usize,
        snapshot: &SensorSnapshot,
        out: &mut [u8],
    ) -> Result<usize, EncodeError>;
}

/// NoAnalog -- no analog sensors: the repository holds the mandatory sensors only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoAnalog;

/// NO_ANALOG -- the one `NoAnalog` value `Context::new` points at.
pub static NO_ANALOG: NoAnalog = NoAnalog;

impl AnalogSensors for NoAnalog {
    fn count(&self) -> usize {
        0
    }

    fn record(&self, _index: usize, _owner_address: u8, _id: u16) -> Option<Record> {
        None
    }

    fn read(
        &self,
        _index: usize,
        _snapshot: &SensorSnapshot,
        _out: &mut [u8],
    ) -> Result<usize, EncodeError> {
        Err(EncodeError::UnsupportedSensor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_analog_has_no_sensor() {
        let none = NoAnalog;
        assert_eq!(none.count(), 0);
        assert!(none.record(0, 0x82, 8).is_none());
        let mut out = [0u8; 4];
        assert_eq!(
            none.read(0, &SensorSnapshot::EMPTY, &mut out),
            Err(EncodeError::UnsupportedSensor)
        );
    }

    #[test]
    fn the_first_analog_number_follows_the_mandatory_sensors() {
        assert_eq!(usize::from(ANALOG_FIRST), crate::sdr::MANDATORY_COUNT_MAX);
        assert_eq!(ANALOG_COUNT_MAX, ipmc_defs::IPMC_RAIL_COUNT + 1);
    }
}
