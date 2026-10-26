//! digital.rs - simulated pin levels (implements `DigitalInputs`).

use crate::hal::{DigitalInputs, Input, SourceError};

/// START_HIGH -- the start-up pin level of each input, indexed by `Input::index`
/// (ipmc-defs IPMC_SIM_PGOOD_HIGH, _FAULT_N_HIGH, _OVERTEMP_N_HIGH, _RESET_N_HIGH).
const START_HIGH: [bool; Input::COUNT] = [
    ipmc_defs::IPMC_SIM_PGOOD_HIGH,
    ipmc_defs::IPMC_SIM_FAULT_N_HIGH,
    ipmc_defs::IPMC_SIM_OVERTEMP_N_HIGH,
    ipmc_defs::IPMC_SIM_RESET_N_HIGH,
];

/// SimInputs -- simulated pin levels. They start at the ipmc-defs levels; with the default
/// levels every pin starts high, so an unwired board reads PGOOD good and every fault input
/// clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimInputs {
    high: [bool; Input::COUNT],
}

impl SimInputs {
    /// new -- every pin at its defs.rs start level.
    pub const fn new() -> Self {
        SimInputs { high: START_HIGH }
    }

    /// with_levels -- every pin at the given level (true = high), indexed by `Input::index`.
    pub const fn with_levels(high: [bool; Input::COUNT]) -> Self {
        SimInputs { high }
    }

    /// set_level -- sets the simulated pin level (true = high).
    pub fn set_level(&mut self, input: Input, high: bool) {
        if let Some(level) = self.high.get_mut(input.index()) {
            *level = high;
        }
    }
}

impl Default for SimInputs {
    fn default() -> Self {
        SimInputs::new()
    }
}

impl DigitalInputs for SimInputs {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        self.high
            .get(input.index())
            .copied()
            .ok_or(SourceError::UnknownChannel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_start_at_the_header_levels_and_follow_set_level() {
        let mut inputs = SimInputs::new();
        let expected = [
            ipmc_defs::IPMC_SIM_PGOOD_HIGH,
            ipmc_defs::IPMC_SIM_FAULT_N_HIGH,
            ipmc_defs::IPMC_SIM_OVERTEMP_N_HIGH,
            ipmc_defs::IPMC_SIM_RESET_N_HIGH,
        ];
        for input in Input::ALL {
            assert_eq!(inputs.level_high(input), Ok(expected[input.index()]));
        }
        inputs.set_level(Input::OvertempN, false);
        assert_eq!(inputs.level_high(Input::OvertempN), Ok(false));
    }

    #[test]
    fn with_levels_sets_each_pin() {
        let mut inputs = SimInputs::with_levels([false, true, false, true]);
        assert_eq!(inputs.level_high(Input::Pgood), Ok(false));
        assert_eq!(inputs.level_high(Input::FaultN), Ok(true));
        assert_eq!(inputs.level_high(Input::OvertempN), Ok(false));
        assert_eq!(inputs.level_high(Input::PayloadResetN), Ok(true));
    }
}
