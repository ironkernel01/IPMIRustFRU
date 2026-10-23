//! ina226.rs - INA226 bus-voltage monitor on I2C, as an `AnalogSource` (example driver).
//!
//! The part measures one bus voltage (0 to 36 V, VBUS pin) and a shunt voltage; this driver
//! serves the bus voltage as channel 0 and leaves VDDA and the die temperature at the trait
//! defaults (`Unavailable`). Registers are 16 bits, most significant byte first, selected by a
//! pointer byte written before the read. Values below are checked against the INA226 datasheet
//! (Table 7-1 register summary, 6.3.1, 7.1.1, 7.1.3, 7.1.8, 7.1.10, Table 6-2).
//!
//! - 00h Configuration. Power-on value 4127h: AVG = 000b (1 average), VBUSCT = VSHCT = 100b
//!   (1.1 ms), MODE = 111b (shunt and bus, continuous). This driver writes AVG and the two
//!   conversion-time fields from ipmi_defs.h (IPMC_INA226_AVG_CODE, IPMC_INA226_CT_CODE; the
//!   defaults give the power-on word) and MODE = 111b to start continuous conversion of both.
//! - 06h Mask/Enable. Bit 3 CVRF (conversion ready) is set after the first conversion, its
//!   averaging and multiplications are complete; a write to the Configuration register clears
//!   it, and so does a read of this register. The driver reports no voltage until it has seen
//!   CVRF once: the bus voltage register holds 0 before the first conversion ends, and 0 mV
//!   would read as a collapsed rail. After the first sight of CVRF, continuous conversion keeps
//!   the register valid.
//! - 02h Bus voltage: 1.25 mV per LSB, bit 15 always 0 (full scale 7FFFh = 40.96 V).
//! - FEh Manufacturer ID: 5449h.
//! - FFh Die ID: bits 15:4 device ID 226h, bits 3:0 die revision; the datasheet lists 2260h
//!   and 2261h by place of manufacture, so only the device ID bits are compared.
//! - 7-bit address 40h to 4Fh, set by the A0 and A1 pins (16 combinations).

use embedded_hal::i2c::I2c;

use crate::eval::rail;
use crate::hal::{AnalogSource, Divider, SourceError};

const REG_CONFIGURATION: u8 = 0x00;
const REG_BUS_VOLTAGE: u8 = 0x02;
const REG_MASK_ENABLE: u8 = 0x06;
const REG_MANUFACTURER_ID: u8 = 0xFE;
const REG_DIE_ID: u8 = 0xFF;
/// Manufacturer ID register value (datasheet Table 7-1).
const MANUFACTURER_ID: u16 = 0x5449;
/// Die ID register: device ID in bits 15:4, die revision in bits 3:0 (datasheet 7.1.10).
const DIE_ID_DEVICE_MASK: u16 = 0xFFF0;
/// Device ID bits of the die ID register, 226h in bits 15:4 (2260h and 2261h both match).
const DIE_ID_DEVICE: u16 = 0x2260;
/// Configuration register fields (datasheet 7.1.1): AVG in bits 11:9, VBUSCT in bits 8:6, VSHCT
/// in bits 5:3, MODE in bits 2:0; bit 14 reads 1 in the power-on word 4127h and is kept 1.
const CONFIG_RESERVED: u16 = 0x4000;
const AVG_SHIFT: u32 = 9;
const VBUSCT_SHIFT: u32 = 6;
const VSHCT_SHIFT: u32 = 3;
/// MODE = 111b: shunt and bus voltage, continuous (datasheet Table 7-5).
const MODE_SHUNT_BUS_CONTINUOUS: u16 = 0b111;
/// Configuration word from the header choices: IPMC_INA226_AVG_CODE for the averaging and
/// IPMC_INA226_CT_CODE for both conversion times, continuous shunt and bus conversion.
/// ipmc-defs guards both codes to 0..=7, three bits each.
const CONFIGURATION: u16 = CONFIG_RESERVED
    | ((ipmc_defs::IPMC_INA226_AVG_CODE as u16) << AVG_SHIFT)
    | ((ipmc_defs::IPMC_INA226_CT_CODE as u16) << VBUSCT_SHIFT)
    | ((ipmc_defs::IPMC_INA226_CT_CODE as u16) << VSHCT_SHIFT)
    | MODE_SHUNT_BUS_CONTINUOUS;
/// Mask/Enable register bit 3: conversion ready flag (datasheet 7.1.8).
const MASK_ENABLE_CVRF: u16 = 1 << 3;
/// Bus voltage register: bit 15 is unused and reads 0 (datasheet Table 7-8).
const BUS_VOLTAGE_MASK: u16 = 0x7FFF;
/// Bus voltage LSB, microvolts: 1.25 mV (datasheet 7.1.3).
const BUS_LSB_UV: u32 = 1250;
const UV_PER_MV: u32 = 1000;
/// CHANNEL_BUS -- the only analog channel of the part: the bus voltage.
pub const CHANNEL_BUS: u8 = 0;

/// Ina226 -- one INA226 on an I2C bus.
pub struct Ina226<I2C> {
    i2c: I2C,
    address: u8,
    /// True once CVRF has been seen after the configuration write: the first conversion is done.
    converted: bool,
}

impl<I2C: I2c> Ina226<I2C> {
    /// new -- checks the manufacturer ID and the device ID bits of the die ID, then writes the
    /// configuration word. `Err(WrongDevice)` on an ID mismatch, `Err(BusError)` on a failed
    /// transfer.
    pub fn new(i2c: I2C, address: u8) -> Result<Self, SourceError> {
        let mut device = Ina226 {
            i2c,
            address,
            converted: false,
        };
        if device.read_register(REG_MANUFACTURER_ID)? != MANUFACTURER_ID
            || device.read_register(REG_DIE_ID)? & DIE_ID_DEVICE_MASK != DIE_ID_DEVICE
        {
            return Err(SourceError::WrongDevice);
        }
        let [hi, lo] = CONFIGURATION.to_be_bytes();
        device
            .i2c
            .write(device.address, &[REG_CONFIGURATION, hi, lo])
            .map_err(|_| SourceError::BusError)?;
        Ok(device)
    }

    /// conversion_done -- true once the first conversion after the configuration write has
    /// finished. Reads Mask/Enable (which clears CVRF) until CVRF is seen, then stays true.
    fn conversion_done(&mut self) -> Result<bool, SourceError> {
        if !self.converted {
            self.converted = self.read_register(REG_MASK_ENABLE)? & MASK_ENABLE_CVRF != 0;
        }
        Ok(self.converted)
    }

    /// read_register -- one 16-bit register: pointer write, then two bytes, one transaction.
    fn read_register(&mut self, register: u8) -> Result<u16, SourceError> {
        let mut bytes = [0u8; 2];
        self.i2c
            .write_read(self.address, &[register], &mut bytes)
            .map_err(|_| SourceError::BusError)?;
        Ok(u16::from_be_bytes(bytes))
    }
}

impl<I2C: I2c> AnalogSource for Ina226<I2C> {
    /// rail_mv -- the bus voltage in millivolts times `divider`; only channel 0 exists.
    /// `Err(NotReady)` until the first conversion has finished, never a 0 mV start-up value.
    fn rail_mv(&mut self, channel: u8, divider: Divider) -> Result<u32, SourceError> {
        if channel != CHANNEL_BUS {
            return Err(SourceError::UnknownChannel);
        }
        if !self.conversion_done()? {
            return Err(SourceError::NotReady);
        }
        let raw = u32::from(self.read_register(REG_BUS_VOLTAGE)? & BUS_VOLTAGE_MASK);
        // raw x 1250 uV / 1000, rounded to nearest.
        let pin_mv = (raw * BUS_LSB_UV + UV_PER_MV / 2) / UV_PER_MV;
        rail::scale_mv(pin_mv, divider)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mock::MockI2c;
    use super::*;
    use crate::hal::analog::contract::{assert_rails_only_part, UNITY};

    const ADDRESS: u8 = 0x40;

    /// present -- the part is on the bus but has not finished a conversion (CVRF clear).
    fn present() -> MockI2c {
        let mut bus = MockI2c::new(ADDRESS, 2);
        bus.set_u16(REG_MANUFACTURER_ID, MANUFACTURER_ID);
        bus.set_u16(REG_DIE_ID, DIE_ID_DEVICE);
        bus
    }

    /// converted -- the part with its first conversion done (CVRF set).
    fn converted() -> MockI2c {
        let mut bus = present();
        bus.set_u16(REG_MASK_ENABLE, MASK_ENABLE_CVRF);
        bus
    }

    #[test]
    fn init_checks_ids_and_writes_the_configuration() {
        let device = Ina226::new(present(), ADDRESS).unwrap();
        assert_eq!(device.i2c.u16_at(REG_CONFIGURATION), CONFIGURATION);
        let [hi, lo] = CONFIGURATION.to_be_bytes();
        assert!(device.i2c.writes.contains(&vec![0x00, hi, lo]));
    }

    #[test]
    fn configuration_fields_come_from_the_header_choices() {
        let word = u32::from(CONFIGURATION);
        assert_eq!((word >> 9) & 7, ipmc_defs::IPMC_INA226_AVG_CODE);
        assert_eq!((word >> 6) & 7, ipmc_defs::IPMC_INA226_CT_CODE);
        assert_eq!((word >> 3) & 7, ipmc_defs::IPMC_INA226_CT_CODE);
        assert_eq!(word & 7, 0b111, "continuous shunt and bus");
        assert_eq!(word >> 12, 0b0100, "reserved bits as at power-on");
    }

    #[test]
    fn both_die_revisions_are_accepted() {
        for die_id in [0x2260, 0x2261] {
            let mut bus = present();
            bus.set_u16(REG_DIE_ID, die_id);
            assert!(Ina226::new(bus, ADDRESS).is_ok(), "{die_id:04X}h");
        }
    }

    #[test]
    fn wrong_ids_and_missing_part_are_named() {
        let mut bus = present();
        bus.set_u16(REG_DIE_ID, 0x1234);
        assert_eq!(
            Ina226::new(bus, ADDRESS).err(),
            Some(SourceError::WrongDevice)
        );
        let mut bus = present();
        bus.set_u16(REG_MANUFACTURER_ID, 0);
        assert_eq!(
            Ina226::new(bus, ADDRESS).err(),
            Some(SourceError::WrongDevice)
        );
        assert_eq!(
            Ina226::new(present(), ADDRESS + 1).err(),
            Some(SourceError::BusError)
        );
    }

    #[test]
    fn bus_voltage_is_1_25_mv_per_lsb() {
        let mut device = Ina226::new(converted(), ADDRESS).unwrap();
        // 9600 x 1.25 mV = 12 000 mV.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 9600);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(12_000));
        // 4000 x 1.25 = 5000 mV at the pin; x 2 / 1 through a divider = 10 000 mV.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 4000);
        assert_eq!(
            device.rail_mv(CHANNEL_BUS, Divider { num: 2, den: 1 }),
            Ok(10_000)
        );
        // Bit 15 is ignored: 8000h reads as 0 mV.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 0x8000);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(0));
        // One LSB: 1.25 mV rounds to 1 mV; three LSB: 3.75 mV rounds to 4 mV.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 1);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(1));
        device.i2c.set_u16(REG_BUS_VOLTAGE, 3);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(4));
    }

    #[test]
    fn no_voltage_before_the_first_conversion_is_complete() {
        let mut device = Ina226::new(present(), ADDRESS).unwrap();
        // The bus voltage register holds 0 now: that must not read as a 0 mV rail.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 0);
        assert_eq!(
            device.rail_mv(CHANNEL_BUS, UNITY),
            Err(SourceError::NotReady)
        );
        // Even a nonzero register is not served while CVRF is clear.
        device.i2c.set_u16(REG_BUS_VOLTAGE, 9600);
        assert_eq!(
            device.rail_mv(CHANNEL_BUS, UNITY),
            Err(SourceError::NotReady)
        );
        // CVRF rises: the first conversion is done and the value is served.
        device.i2c.set_u16(REG_MASK_ENABLE, MASK_ENABLE_CVRF);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(12_000));
        // Reading Mask/Enable clears CVRF in the part; continuous conversion keeps the result valid.
        device.i2c.set_u16(REG_MASK_ENABLE, 0);
        assert_eq!(device.rail_mv(CHANNEL_BUS, UNITY), Ok(12_000));
    }

    #[test]
    fn other_channels_and_bus_faults_are_rejected() {
        let mut device = Ina226::new(converted(), ADDRESS).unwrap();
        assert_rails_only_part(&mut device, 1, CHANNEL_BUS, |d| d.i2c.fail = true);
    }
}
