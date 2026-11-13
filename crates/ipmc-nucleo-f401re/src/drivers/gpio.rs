//! gpio.rs - the board's digital pins: the power-enable output, the payload reset output and the
//! four inputs. Implements `DigitalInputs`, `PowerSwitch` and `ResetOutput`.
//!
//! The pins are the board's choice: the board support hands them in as a `GpioMap` (ipmc-nucleo-f401re
//! drivers/board_pins.rs, listed in local_only/Documentation/nucleo_f401re_pinout.md); this module holds no
//! board pin.
//!
//! Ownership: the pins are claimed through `ports` (owner name "board_io"), one handle per pin;
//! this module never touches a port register directly. Only the groups the source switches make
//! hardware are claimed: `init` takes a flag for the inputs and one for the power output, and a
//! group that is not claimed is never configured or driven (a simulated power group leaves
//! PWR_EN alone).

use ipmc_sensors::hal::{DigitalInputs, Input, PowerSwitch, ResetOutput, SourceError};

use crate::bsp::ports::{self, PinId, Pins, PortError, Pull, Speed};

const OWNER: &str = "board_io";

/// GpioMap -- the board's pin assignment for the digital signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpioMap {
    /// PWR_EN, push-pull output, active high.
    pub power_enable: PinId,
    /// Pin of each input, indexed by `Input::index`; pulled-up inputs.
    pub inputs: [PinId; Input::COUNT],
    /// PAYLOAD_RST_OUT_N, open-drain output, active low, released (high impedance) at start-up.
    pub payload_reset_out: PinId,
}

/// BoardIo -- the claimed pins; a slot is None when its group is not hardware.
pub struct BoardIo {
    pwr_en: Option<Pins>,
    payload_reset_out: Option<Pins>,
    inputs: [Option<Pins>; Input::COUNT],
}

/// init -- claims and configures the pins of the hardware groups.
///
/// 1. `power`: the enable pin: claim, drive low and select push-pull before the pin
///    becomes an output, so the supplies never see a glitch high (`Pins::output` orders the
///    writes).
///    The payload reset output belongs to the same group (the controls of the payload): claimed
///    as an open-drain output released (high) before the pin becomes an output, so the payload
///    sees no reset pulse at start-up.
/// 2. `inputs`: claim the four input pins as pulled-up inputs.
///
/// A group with its flag false is not touched. Fails with the first `PortError` (a pin already
/// claimed, or a pin number above 15).
pub fn init(map: &GpioMap, inputs: bool, power: bool) -> Result<BoardIo, PortError> {
    let mut io = BoardIo {
        pwr_en: None,
        payload_reset_out: None,
        inputs: core::array::from_fn(|_| None),
    };
    if power {
        let pwr_en = ports::claim_pin(OWNER, map.power_enable)?;
        pwr_en.output(false, false, Speed::Low);
        io.pwr_en = Some(pwr_en);
        let reset_out = ports::claim_pin(OWNER, map.payload_reset_out)?;
        reset_out.output(true, true, Speed::Low);
        io.payload_reset_out = Some(reset_out);
    }
    if inputs {
        for (slot, pin) in io.inputs.iter_mut().zip(map.inputs.iter()) {
            let pins = ports::claim_pin(OWNER, *pin)?;
            pins.input(Pull::Up);
            *slot = Some(pins);
        }
    }
    Ok(io)
}

impl DigitalInputs for BoardIo {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        let pins = self
            .inputs
            .get(input.index())
            .and_then(Option::as_ref)
            .ok_or(SourceError::Unavailable)?;
        Ok(pins.read() != 0)
    }
}

/// drive -- writes `high` to the output `pin`; `Err(Unavailable)` when the power group is not
/// hardware and the pin was never claimed. The one output write of PWR_EN and the reset output.
fn drive(pin: Option<&Pins>, high: bool) -> Result<(), SourceError> {
    pin.ok_or(SourceError::Unavailable)?.set_all(high);
    Ok(())
}

impl ResetOutput for BoardIo {
    /// set_reset -- drives PAYLOAD_RST_OUT_N low (asserted) or releases it (open drain, high
    /// impedance); `Err(Unavailable)` when the power group is not hardware.
    fn set_reset(&mut self, asserted: bool) -> Result<(), SourceError> {
        drive(self.payload_reset_out.as_ref(), !asserted)
    }
}

impl PowerSwitch for BoardIo {
    /// set_enable -- drives the output; `Err(Unavailable)` when the power group is not hardware.
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
        drive(self.pwr_en.as_ref(), on)
    }

    fn is_enabled(&self) -> bool {
        self.pwr_en
            .as_ref()
            .is_some_and(|pin| pin.output_levels() != 0)
    }
}
