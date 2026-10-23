//! analog.rs - simulated rails and die temperature (implements `AnalogSource`).
//!
//! A rail value is held after the divider: the simulator ignores `Divider`.

use crate::hal::{AnalogSource, Channel, Divider, SourceError};
use ipmc_core::sensor::RAILS_MAX;

/// SimAnalog -- simulated rails and die temperature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimAnalog {
    rails: [(Channel, u32); RAILS_MAX],
    rail_count: usize,
    temperature_dc: i32,
}

impl SimAnalog {
    /// new -- one entry per element of `initial` (at most RAILS_MAX): the channel and its
    /// start millivolts.
    pub fn new(initial: &[(Channel, u32)], temperature_dc: i32) -> Self {
        let mut sim = SimAnalog {
            rails: [(Channel::Vdda, 0); RAILS_MAX],
            rail_count: 0,
            temperature_dc,
        };
        for (slot, entry) in sim.rails.iter_mut().zip(initial) {
            *slot = *entry;
            sim.rail_count += 1;
        }
        sim
    }

    /// rail_value -- the simulated millivolts of `channel`.
    pub fn rail_value(&self, channel: Channel) -> Result<u32, SourceError> {
        self.rails
            .get(..self.rail_count)
            .unwrap_or(&[])
            .iter()
            .find(|(ch, _)| *ch == channel)
            .map(|&(_, mv)| mv)
            .ok_or(SourceError::UnknownChannel)
    }

    /// set_rail_mv -- sets the simulated millivolts of `channel`; `Err(UnknownChannel)` when
    /// the board has no such rail.
    pub fn set_rail_mv(&mut self, channel: Channel, mv: u32) -> Result<(), SourceError> {
        let entries = self.rails.get_mut(..self.rail_count).unwrap_or(&mut []);
        match entries.iter_mut().find(|(ch, _)| *ch == channel) {
            Some(entry) => {
                entry.1 = mv;
                Ok(())
            }
            None => Err(SourceError::UnknownChannel),
        }
    }

    /// temperature_dc -- the simulated die temperature in tenths of a degree C.
    pub fn temperature_dc(&self) -> i32 {
        self.temperature_dc
    }

    /// set_temperature_dc -- sets the simulated die temperature, tenths of a degree C.
    pub fn set_temperature_dc(&mut self, dc: i32) {
        self.temperature_dc = dc;
    }
}

impl AnalogSource for SimAnalog {
    fn vdda_mv(&mut self) -> Result<u32, SourceError> {
        self.rail_value(Channel::Vdda)
    }

    fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
        Ok(self.temperature_dc)
    }

    fn rail_mv(&mut self, channel: u8, _divider: Divider) -> Result<u32, SourceError> {
        self.rail_value(Channel::Adc(channel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> SimAnalog {
        SimAnalog::new(&[(Channel::Vdda, 3300), (Channel::Adc(1), 5000)], 250)
    }

    #[test]
    fn starts_at_the_given_values() {
        let mut s = sim();
        assert_eq!(s.vdda_mv(), Ok(3300));
        assert_eq!(s.rail_mv(1, Divider { num: 2, den: 1 }), Ok(5000));
        assert_eq!(s.die_temperature_dc(), Ok(250));
    }

    #[test]
    fn rails_are_adjustable_and_unknown_channels_rejected() {
        let mut s = sim();
        assert_eq!(s.set_rail_mv(Channel::Adc(1), 4400), Ok(()));
        assert_eq!(s.rail_mv(1, Divider { num: 1, den: 1 }), Ok(4400));
        assert_eq!(
            s.set_rail_mv(Channel::Adc(99), 1),
            Err(SourceError::UnknownChannel)
        );
        assert_eq!(
            s.rail_mv(99, Divider { num: 1, den: 1 }),
            Err(SourceError::UnknownChannel)
        );
        s.set_temperature_dc(-55);
        assert_eq!(s.die_temperature_dc(), Ok(-55));
    }
}
