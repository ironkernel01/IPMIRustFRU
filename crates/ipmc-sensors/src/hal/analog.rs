//! analog.rs - the interface for voltages and the die temperature.

use super::SourceError;

/// Channel -- where a rail voltage comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The ADC supply, derived from the internal reference.
    Vdda,
    /// An input by channel number on the driver that serves it (ADC channel, converter input).
    Adc(u8),
}

/// Divider -- rail millivolts = pin millivolts x `num` / `den`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divider {
    pub num: u32,
    pub den: u32,
}

/// RailWiring -- the board facts of one rail: which channel carries it and the divider in front of
/// that channel. The neutral sensor facts (name, nominal, windows) are `RailDef` in
/// `eval/rail.rs`; the board crate supplies one `RailWiring` per `RailDef`, in the same order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailWiring {
    pub channel: Channel,
    pub divider: Divider,
}

/// AnalogSource -- voltages and temperature in engineering units.
pub trait AnalogSource {
    /// vdda_mv -- the ADC supply in millivolts. Parts that do not measure it keep the default.
    fn vdda_mv(&mut self) -> Result<u32, SourceError> {
        Err(SourceError::Unavailable)
    }

    /// die_temperature_dc -- die temperature in tenths of a degree C. Parts that do not measure
    /// it keep the default.
    fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
        Err(SourceError::Unavailable)
    }

    /// rail_mv -- the rail voltage in millivolts for input `channel` behind `divider`.
    /// A simulator holds the value after the divider and ignores `divider`.
    fn rail_mv(&mut self, channel: u8, divider: Divider) -> Result<u32, SourceError>;
}

/// contract -- what every external rail part must do, checked once for all drivers.
#[cfg(test)]
pub(crate) mod contract {
    use super::{AnalogSource, Divider};
    use crate::hal::SourceError;

    /// UNITY -- a 1 / 1 divider: the pin millivolts are the rail millivolts.
    pub(crate) const UNITY: Divider = Divider { num: 1, den: 1 };

    /// assert_rails_only_part -- a part that measures rails only: `bad_channel` is an unknown
    /// channel, VDDA and the die temperature are unavailable, and after `inject_bus_fault` a
    /// read of `good_channel` fails with `BusError`.
    pub(crate) fn assert_rails_only_part<S: AnalogSource>(
        part: &mut S,
        bad_channel: u8,
        good_channel: u8,
        inject_bus_fault: impl FnOnce(&mut S),
    ) {
        assert_eq!(
            part.rail_mv(bad_channel, UNITY),
            Err(SourceError::UnknownChannel)
        );
        assert_eq!(part.vdda_mv(), Err(SourceError::Unavailable));
        assert_eq!(part.die_temperature_dc(), Err(SourceError::Unavailable));
        inject_bus_fault(part);
        assert_eq!(
            part.rail_mv(good_channel, UNITY),
            Err(SourceError::BusError)
        );
    }
}
