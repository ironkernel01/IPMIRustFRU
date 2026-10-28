//! routing.rs - picks, per source switch of ipmc-defs, which driver answers a group.
//!
//! The routers hold the on-chip driver (None when the build has none) and the simulator, and
//! choose per call from the switches given at construction. A switch set to hardware with no
//! driver present reports `SourceError::Unavailable`; the reading is then marked unavailable
//! rather than invented. The switch values `Source::I2c` and `Source::Spi` name external
//! parts (drivers/i2c, drivers/spi); no bus is assigned to them, so the routers answer
//! `Unavailable` and the firmware build refuses the values (ipmc-nucleo-f401re sensing.rs). A board
//! integrator who assigns a bus adds an arm for the new source here.

use crate::drivers::sim::{SimAnalog, SimInputs, SimPower};
use crate::hal::{AnalogSource, DigitalInputs, Divider, Input, PowerSwitch, SourceError};

/// Source -- which driver answers a group (the `IPMC_*_SOURCE` switches of ipmc-defs): the
/// board crate's on-chip driver for `Hw`, the simulator (drivers/sim) for `Sim`, an external
/// I2C part (drivers/i2c) for `I2c` and an external SPI part (drivers/spi) for `Spi`; no bus is
/// assigned to the last two yet.
pub use ipmc_defs::Source;

/// backend -- the backend of a router: `route` over the router's hardware option (coerced to
/// the trait object `$tr`) and its simulator field `$sim`. A macro because the hardware type
/// is generic and each router names its own trait.
macro_rules! backend {
    ($self:ident, $source:expr, $sim:ident, $tr:ident) => {
        route(
            $source,
            $self.hw.as_mut().map(|hw| hw as &mut dyn $tr),
            &mut $self.$sim,
        )
    };
}

/// route -- the one backend selector of the routers: the simulator for `Sim`, the hardware
/// backend for `Hw` (`Unavailable` when the build has none), `Unavailable` for the external
/// sources that have no bus assigned.
fn route<'a, T: ?Sized>(
    source: Source,
    hw: Option<&'a mut T>,
    sim: &'a mut T,
) -> Result<&'a mut T, SourceError> {
    match source {
        Source::Sim => Ok(sim),
        Source::Hw => hw.ok_or(SourceError::Unavailable),
        Source::I2c | Source::Spi => Err(SourceError::Unavailable),
    }
}

/// HardwareNeeds -- which on-chip hardware the source switches ask for. A group whose source is
/// not `Source::Hw` needs none of it: its pins are not claimed or configured and its peripheral
/// is not started (a simulated power group never drives PWR_EN).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareNeeds {
    /// ADC1 is started (internal or external group is hardware).
    pub adc: bool,
    /// The external rail pins are claimed and put in analog mode.
    pub adc_pins: bool,
    /// The fault input pins are claimed and configured.
    pub input_pins: bool,
    /// The PWR_EN pin is claimed and driven.
    pub power_pin: bool,
}

impl HardwareNeeds {
    /// of -- the needs of the four group switches.
    pub const fn of(internal: Source, external: Source, fault: Source, power: Source) -> Self {
        HardwareNeeds {
            adc: matches!(internal, Source::Hw) || matches!(external, Source::Hw),
            adc_pins: matches!(external, Source::Hw),
            input_pins: matches!(fault, Source::Hw),
            power_pin: matches!(power, Source::Hw),
        }
    }
}

/// RoutedAnalog -- `AnalogSource` over a hardware backend and the simulator. The internal
/// group is VDDA and the die temperature; the external group is the pin rails.
pub struct RoutedAnalog<H: AnalogSource> {
    pub hw: Option<H>,
    pub sim: SimAnalog,
    internal: Source,
    external: Source,
}

impl<H: AnalogSource> RoutedAnalog<H> {
    /// new -- a router over `hw` (None when the build has no driver) and `sim`, with the switch of each group.
    pub fn new(hw: Option<H>, sim: SimAnalog, internal: Source, external: Source) -> Self {
        RoutedAnalog {
            hw,
            sim,
            internal,
            external,
        }
    }

    /// internal_source -- the backend of VDDA and the die temperature.
    pub fn internal_source(&self) -> Source {
        self.internal
    }

    /// external_source -- the backend of the pin rails.
    pub fn external_source(&self) -> Source {
        self.external
    }

    /// backend -- the driver that answers a group switched to `source`.
    fn backend(&mut self, source: Source) -> Result<&mut dyn AnalogSource, SourceError> {
        backend!(self, source, sim, AnalogSource)
    }
}

impl<H: AnalogSource> AnalogSource for RoutedAnalog<H> {
    fn vdda_mv(&mut self) -> Result<u32, SourceError> {
        self.backend(self.internal)?.vdda_mv()
    }

    fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
        self.backend(self.internal)?.die_temperature_dc()
    }

    fn rail_mv(&mut self, channel: u8, divider: Divider) -> Result<u32, SourceError> {
        self.backend(self.external)?.rail_mv(channel, divider)
    }
}

/// RoutedIo -- `DigitalInputs` and `PowerSwitch` over one hardware backend (the GPIO owner) and
/// the two simulators. The fault switch routes the inputs, the power switch routes the enable.
pub struct RoutedIo<H: DigitalInputs + PowerSwitch> {
    pub hw: Option<H>,
    pub sim_inputs: SimInputs,
    pub sim_power: SimPower,
    fault: Source,
    power: Source,
}

impl<H: DigitalInputs + PowerSwitch> RoutedIo<H> {
    /// new -- a router over `hw` and the two simulators, with the fault and power switches.
    pub fn new(
        hw: Option<H>,
        sim_inputs: SimInputs,
        sim_power: SimPower,
        fault: Source,
        power: Source,
    ) -> Self {
        RoutedIo {
            hw,
            sim_inputs,
            sim_power,
            fault,
            power,
        }
    }

    /// fault_source -- the backend of the digital inputs.
    pub fn fault_source(&self) -> Source {
        self.fault
    }

    /// power_source -- the backend of the enable output.
    pub fn power_source(&self) -> Source {
        self.power
    }

    /// inputs_backend -- the driver that answers the digital inputs.
    fn inputs_backend(&mut self) -> Result<&mut dyn DigitalInputs, SourceError> {
        backend!(self, self.fault, sim_inputs, DigitalInputs)
    }

    /// power_backend -- the driver that answers the enable output.
    fn power_backend(&mut self) -> Result<&mut dyn PowerSwitch, SourceError> {
        backend!(self, self.power, sim_power, PowerSwitch)
    }
}

impl<H: DigitalInputs + PowerSwitch> DigitalInputs for RoutedIo<H> {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        self.inputs_backend()?.level_high(input)
    }
}

impl<H: DigitalInputs + PowerSwitch> PowerSwitch for RoutedIo<H> {
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
        self.power_backend()?.set_enable(on)
    }

    fn is_enabled(&self) -> bool {
        match self.power {
            Source::Sim => self.sim_power.is_enabled(),
            Source::Hw => self.hw.as_ref().is_some_and(|hw| hw.is_enabled()),
            Source::I2c | Source::Spi => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_sensors::{sim_analog, INPUTS, RAILS};
    use crate::drivers::sim::SIM_RAIL_WIRING;
    use crate::hal::Channel;

    /// A hardware stand-in that answers fixed values, so routing is visible in the result.
    struct Fixed;

    impl AnalogSource for Fixed {
        fn vdda_mv(&mut self) -> Result<u32, SourceError> {
            Ok(1)
        }
        fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
            Ok(2)
        }
        fn rail_mv(&mut self, _channel: u8, _divider: Divider) -> Result<u32, SourceError> {
            Ok(3)
        }
    }

    impl DigitalInputs for Fixed {
        fn level_high(&mut self, _input: Input) -> Result<bool, SourceError> {
            Ok(false)
        }
    }

    impl PowerSwitch for Fixed {
        fn set_enable(&mut self, _on: bool) -> Result<(), SourceError> {
            Ok(())
        }
        fn is_enabled(&self) -> bool {
            true
        }
    }

    #[test]
    fn analog_groups_route_independently() {
        let mut routed = RoutedAnalog::new(
            Some(Fixed),
            sim_analog(&SIM_RAIL_WIRING),
            Source::Hw,
            Source::Sim,
        );
        assert_eq!(routed.vdda_mv(), Ok(1));
        assert_eq!(routed.die_temperature_dc(), Ok(2));
        // The external rails come from the simulator: rail 1 of the board table at nominal.
        let rail = &RAILS[1];
        let wire = &SIM_RAIL_WIRING[1];
        let channel = match wire.channel {
            Channel::Adc(ch) => ch,
            Channel::Vdda => unreachable!("rail 1 is a pin rail"),
        };
        assert_eq!(routed.rail_mv(channel, wire.divider), Ok(rail.nominal_mv));
    }

    #[test]
    fn hardware_selected_without_backend_is_unavailable() {
        let mut routed: RoutedAnalog<Fixed> =
            RoutedAnalog::new(None, sim_analog(&SIM_RAIL_WIRING), Source::Hw, Source::Hw);
        assert_eq!(routed.vdda_mv(), Err(SourceError::Unavailable));
        assert_eq!(routed.die_temperature_dc(), Err(SourceError::Unavailable));
        assert_eq!(
            routed.rail_mv(0, Divider { num: 1, den: 1 }),
            Err(SourceError::Unavailable)
        );
    }

    #[test]
    fn io_routes_inputs_and_power_independently() {
        let mut io = RoutedIo::new(
            Some(Fixed),
            SimInputs::new(),
            SimPower::new(),
            Source::Sim,
            Source::Hw,
        );
        // Inputs from the simulator (all high), enable from the hardware stand-in (true).
        assert_eq!(io.level_high(INPUTS[0].input), Ok(true));
        assert!(io.is_enabled());
        let mut sim_only: RoutedIo<Fixed> = RoutedIo::new(
            None,
            SimInputs::new(),
            SimPower::new(),
            Source::Hw,
            Source::Sim,
        );
        assert_eq!(
            sim_only.level_high(Input::Pgood),
            Err(SourceError::Unavailable)
        );
        assert!(!sim_only.is_enabled());
        assert_eq!(sim_only.set_enable(true), Ok(()));
        assert!(sim_only.is_enabled());
    }

    #[test]
    fn external_sources_have_no_bus_and_answer_unavailable() {
        let mut analog: RoutedAnalog<Fixed> = RoutedAnalog::new(
            Some(Fixed),
            sim_analog(&SIM_RAIL_WIRING),
            Source::I2c,
            Source::Spi,
        );
        assert_eq!(analog.vdda_mv(), Err(SourceError::Unavailable));
        assert_eq!(
            analog.rail_mv(0, Divider { num: 1, den: 1 }),
            Err(SourceError::Unavailable)
        );
        let mut io = RoutedIo::new(
            Some(Fixed),
            SimInputs::new(),
            SimPower::new(),
            Source::I2c,
            Source::Spi,
        );
        assert_eq!(io.level_high(Input::Pgood), Err(SourceError::Unavailable));
        assert_eq!(io.set_enable(true), Err(SourceError::Unavailable));
        assert!(!io.is_enabled());
    }

    #[test]
    fn hardware_selected_without_backend_reports_a_failed_enable() {
        let mut io: RoutedIo<Fixed> = RoutedIo::new(
            None,
            SimInputs::new(),
            SimPower::new(),
            Source::Sim,
            Source::Hw,
        );
        assert_eq!(io.set_enable(true), Err(SourceError::Unavailable));
        assert!(!io.is_enabled());
    }

    #[test]
    fn simulated_groups_need_no_hardware() {
        let sim = Source::Sim;
        let none = HardwareNeeds::of(sim, sim, sim, sim);
        assert_eq!(
            none,
            HardwareNeeds {
                adc: false,
                adc_pins: false,
                input_pins: false,
                power_pin: false
            }
        );
        // The internal group alone needs the ADC but no pin.
        let internal = HardwareNeeds::of(Source::Hw, sim, sim, sim);
        assert!(internal.adc && !internal.adc_pins);
        // The external group needs the ADC and its pins.
        let external = HardwareNeeds::of(sim, Source::Hw, sim, sim);
        assert!(external.adc && external.adc_pins);
        // The input and power groups are independent: simulated power leaves PWR_EN alone.
        let inputs = HardwareNeeds::of(sim, sim, Source::Hw, sim);
        assert!(inputs.input_pins && !inputs.power_pin && !inputs.adc);
        let power = HardwareNeeds::of(sim, sim, sim, Source::Hw);
        assert!(power.power_pin && !power.input_pins);
    }

    #[test]
    fn input_index_matches_all() {
        for (i, input) in Input::ALL.iter().enumerate() {
            assert_eq!(input.index(), i);
        }
    }
}
