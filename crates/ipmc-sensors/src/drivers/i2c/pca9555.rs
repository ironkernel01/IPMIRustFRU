//! pca9555.rs - PCA9555 16-bit I/O expander on I2C, as `DigitalInputs` and `PowerSwitch`
//! (example driver). Pins are numbered 0 to 7 (port 0) and 8 to 15 (port 1).
//!
//! Registers are 8 bits; a command byte selects one, and a two-byte access reads or writes the
//! register pair of both ports (the second byte goes to the other register of the pair).
//! Values below are checked against the PCA9555 datasheet (Table 4, 6.2.2 to 6.2.5, 6.3).
//!
//! - 00h / 01h Input port 0 / 1: pin levels.
//! - 02h / 03h Output port 0 / 1: levels driven on output pins; power-on value FFh.
//! - 04h / 05h Polarity inversion port 0 / 1: a 1 inverts the input port data; power-on value
//!   00h. The part keeps its registers across an MCU reset, so `new` clears them: every level
//!   the driver reports is the electrical pin level.
//! - 06h / 07h Configuration port 0 / 1: a 1 makes the pin an input, a 0 an output; power-on
//!   value FFh, all inputs.
//! - 7-bit address 0100 A2 A1 A0 = 20h to 27h.
//!
//! The supply-enable pin is made an output only after its output level is written low, so the
//! enable never glitches high.

use embedded_hal::i2c::I2c;

use crate::hal::{DigitalInputs, Input, PowerSwitch, SourceError};

const REG_INPUT: u8 = 0x00;
const REG_OUTPUT: u8 = 0x02;
const REG_POLARITY: u8 = 0x04;
const REG_CONFIGURATION: u8 = 0x06;
/// Polarity inversion value that retains every input polarity (datasheet 6.2.4: 0 = retained).
const POLARITY_NONE: u16 = 0x0000;
/// Pins on the part.
const PIN_COUNT: u8 = 16;

/// Pca9555Map -- which expander pin carries each board signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pca9555Map {
    /// Pin of each `Input`, indexed by `Input::index`; None when the signal is not on the part.
    pub inputs: [Option<u8>; Input::COUNT],
    /// Pin of the supply enable, active high.
    pub power_enable: u8,
}

/// Pca9555 -- one PCA9555 on an I2C bus.
pub struct Pca9555<I2C> {
    i2c: I2C,
    address: u8,
    map: Pca9555Map,
    /// Shadow of the output registers, port 0 in the low byte; updated only after a write
    /// succeeded, so it is the last confirmed output state.
    output: u16,
}

impl<I2C: I2c> Pca9555<I2C> {
    /// new -- drives the enable pin low, clears the polarity inversion registers, then
    /// configures the enable as the only output (every other pin stays an input).
    /// `Err(UnknownChannel)` for a pin past 15, `Err(BusError)` on a failed transfer.
    pub fn new(i2c: I2C, address: u8, map: Pca9555Map) -> Result<Self, SourceError> {
        let pins_ok =
            map.power_enable < PIN_COUNT && map.inputs.iter().flatten().all(|&pin| pin < PIN_COUNT);
        if !pins_ok {
            return Err(SourceError::UnknownChannel);
        }
        let enable_bit = 1u16 << map.power_enable;
        let mut device = Pca9555 {
            i2c,
            address,
            map,
            output: !enable_bit,
        };
        device.write_pair(REG_OUTPUT, device.output)?;
        device.write_pair(REG_POLARITY, POLARITY_NONE)?;
        device.write_pair(REG_CONFIGURATION, !enable_bit)?;
        Ok(device)
    }

    /// write_pair -- writes a register pair: command byte, port 0 byte, port 1 byte.
    fn write_pair(&mut self, register: u8, value: u16) -> Result<(), SourceError> {
        let [port0, port1] = value.to_le_bytes();
        self.i2c
            .write(self.address, &[register, port0, port1])
            .map_err(|_| SourceError::BusError)
    }
}

impl<I2C: I2c> DigitalInputs for Pca9555<I2C> {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        let pin = self
            .map
            .inputs
            .get(input.index())
            .copied()
            .flatten()
            .ok_or(SourceError::UnknownChannel)?;
        let mut ports = [0u8; 2];
        self.i2c
            .write_read(self.address, &[REG_INPUT], &mut ports)
            .map_err(|_| SourceError::BusError)?;
        Ok(u16::from_le_bytes(ports) & (1 << pin) != 0)
    }
}

impl<I2C: I2c> PowerSwitch for Pca9555<I2C> {
    /// set_enable -- writes the output register pair with the enable bit set or cleared. The
    /// bus result is returned; the confirmed state changes only when the write succeeded.
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
        let bit = 1u16 << self.map.power_enable;
        let output = if on {
            self.output | bit
        } else {
            self.output & !bit
        };
        self.write_pair(REG_OUTPUT, output)?;
        self.output = output;
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.output & (1u16 << self.map.power_enable) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::super::mock::MockI2c;
    use super::*;

    const ADDRESS: u8 = 0x20;
    /// PGOOD on pin 0, FAULT_N on pin 1, OVERTEMP_N on pin 9, no reset pin; enable on pin 15.
    const MAP: Pca9555Map = Pca9555Map {
        inputs: [Some(0), Some(1), Some(9), None],
        power_enable: 15,
    };

    fn device() -> Pca9555<MockI2c> {
        Pca9555::new(MockI2c::new(ADDRESS, 1), ADDRESS, MAP).unwrap()
    }

    #[test]
    fn init_drives_low_before_making_the_enable_an_output() {
        let d = device();
        // Output pair first: every bit high except pin 15 -> port 0 FFh, port 1 7Fh.
        assert_eq!(d.i2c.writes[0], vec![0x02, 0xFF, 0x7F]);
        // Then the polarity registers are cleared, then the configuration: pin 15 is the only
        // output.
        assert_eq!(d.i2c.writes[1], vec![0x04, 0x00, 0x00]);
        assert_eq!(d.i2c.writes[2], vec![0x06, 0xFF, 0x7F]);
        assert!(!d.is_enabled());
    }

    #[test]
    fn init_clears_polarity_inversion_left_by_an_earlier_run() {
        // The part stayed powered across an MCU reset with every input inverted.
        let mut bus = MockI2c::new(ADDRESS, 1);
        bus.regs[4] = 0xFF;
        bus.regs[5] = 0xFF;
        let mut d = Pca9555::new(bus, ADDRESS, MAP).unwrap();
        assert_eq!((d.i2c.regs[4], d.i2c.regs[5]), (0x00, 0x00));
        // Pin 0 is electrically high and reads high; pin 1 is low and reads low.
        d.i2c.regs[0] = 0x01;
        assert_eq!(d.level_high(Input::Pgood), Ok(true));
        assert_eq!(d.level_high(Input::FaultN), Ok(false));
    }

    #[test]
    fn inputs_read_the_mapped_pins() {
        let mut d = device();
        // Input ports: port 0 = 01h (pin 0 high), port 1 = 02h (pin 9 high).
        d.i2c.regs[0] = 0x01;
        d.i2c.regs[1] = 0x02;
        assert_eq!(d.level_high(Input::Pgood), Ok(true));
        assert_eq!(d.level_high(Input::FaultN), Ok(false));
        assert_eq!(d.level_high(Input::OvertempN), Ok(true));
        assert_eq!(
            d.level_high(Input::PayloadResetN),
            Err(SourceError::UnknownChannel)
        );
        d.i2c.fail = true;
        assert_eq!(d.level_high(Input::Pgood), Err(SourceError::BusError));
    }

    #[test]
    fn enable_sets_and_clears_only_its_pin() {
        let mut d = device();
        assert_eq!(d.set_enable(true), Ok(()));
        assert!(d.is_enabled());
        assert_eq!(d.i2c.writes.last().unwrap(), &vec![0x02, 0xFF, 0xFF]);
        assert_eq!(d.set_enable(false), Ok(()));
        assert!(!d.is_enabled());
        assert_eq!(d.i2c.writes.last().unwrap(), &vec![0x02, 0xFF, 0x7F]);
    }

    #[test]
    fn failed_enable_write_is_returned_and_keeps_the_confirmed_state() {
        let mut d = device();
        d.i2c.fail = true;
        assert_eq!(d.set_enable(true), Err(SourceError::BusError));
        assert!(!d.is_enabled());
        d.i2c.fail = false;
        assert_eq!(d.set_enable(true), Ok(()));
        assert!(d.is_enabled());
        // A failed power-off leaves the last confirmed state: still on.
        d.i2c.fail = true;
        assert_eq!(d.set_enable(false), Err(SourceError::BusError));
        assert!(d.is_enabled());
    }

    #[test]
    fn pins_past_15_are_refused() {
        let bad = Pca9555Map {
            power_enable: 16,
            ..MAP
        };
        assert_eq!(
            Pca9555::new(MockI2c::new(ADDRESS, 1), ADDRESS, bad).err(),
            Some(SourceError::UnknownChannel)
        );
    }
}
