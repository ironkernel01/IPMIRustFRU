//! mcp3208.rs - MCP3208 12-bit, 8-channel ADC on SPI, as an `AnalogSource` (example driver).
//! Values below are checked against the MCP3208 datasheet (Figure 6-1, the transfer function
//! and the electrical characteristics).
//!
//! One conversion is a 3-byte full-duplex frame (Figure 6-1, SPI with 8-bit segments):
//!   write byte 0  00000 1 SGL D2   start bit (bit 2) = 1, single-ended (bit 1) = 1, D2 (bit 0)
//!   write byte 1  D1 D0 000000     channel bits in the top two bits
//!   write byte 2  don't care
//!   read byte 1   bit 4 = null bit (0), bits 3:0 = result bits 11:8
//!   read byte 2   result bits 7:0
//! The result is code = 4096 x VIN / VREF, so millivolts = code x Vref / 4096. SPI mode 0,0 or
//! 1,1, clock up to 1 MHz at VDD = 2.7 V and 2 MHz at 5 V; the integrator sets the mode and
//! clock on the `SpiDevice`.

use embedded_hal::spi::SpiDevice;

use crate::eval::rail;
use crate::hal::{AnalogSource, Divider, SourceError};

/// CHANNEL_COUNT -- input channels on the part (CH0 to CH7).
pub const CHANNEL_COUNT: u8 = 8;
/// Command byte 0 base: start bit (bit 2) and single-ended (bit 1); D2 is OR-ed into bit 0.
const START_SINGLE_ENDED: u8 = 0b0000_0110;
/// Channel bits D1 and D0 sit in the top two bits of command byte 1.
const D1_D0_SHIFT: u32 = 6;
const D1_D0_MASK: u8 = 0b11;
/// D2 is the channel's bit 2.
const D2_SHIFT: u32 = 2;
/// Result bits 11:8 are the low nibble of read byte 1.
const RESULT_HIGH_MASK: u8 = 0x0F;
/// LSBs in a full scale: 12 bits.
const COUNTS_PER_VREF: u64 = 4096;

/// Mcp3208 -- one MCP3208 on an SPI device.
pub struct Mcp3208<SPI> {
    spi: SPI,
    vref_mv: u32,
}

impl<SPI: SpiDevice<u8>> Mcp3208<SPI> {
    /// new -- `vref_mv` is the reference voltage on the part's VREF pin, millivolts.
    pub fn new(spi: SPI, vref_mv: u32) -> Self {
        Mcp3208 { spi, vref_mv }
    }

    /// read_counts -- one single-ended conversion of `channel`, 0..=4095.
    fn read_counts(&mut self, channel: u8) -> Result<u16, SourceError> {
        if channel >= CHANNEL_COUNT {
            return Err(SourceError::UnknownChannel);
        }
        let command = [
            START_SINGLE_ENDED | (channel >> D2_SHIFT),
            (channel & D1_D0_MASK) << D1_D0_SHIFT,
            0,
        ];
        let mut reply = [0u8; 3];
        self.spi
            .transfer(&mut reply, &command)
            .map_err(|_| SourceError::BusError)?;
        let [_, high, low] = reply;
        Ok((u16::from(high & RESULT_HIGH_MASK) << 8) | u16::from(low))
    }
}

impl<SPI: SpiDevice<u8>> AnalogSource for Mcp3208<SPI> {
    /// rail_mv -- the input voltage in millivolts times `divider`; channels 0 to 7.
    fn rail_mv(&mut self, channel: u8, divider: Divider) -> Result<u32, SourceError> {
        let counts = u64::from(self.read_counts(channel)?);
        // counts x Vref / 4096, rounded to nearest.
        let pin_mv = (counts * u64::from(self.vref_mv) + COUNTS_PER_VREF / 2) / COUNTS_PER_VREF;
        let pin_mv = u32::try_from(pin_mv).map_err(|_| SourceError::Overflow)?;
        rail::scale_mv(pin_mv, divider)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mock::MockSpi;
    use super::*;
    use crate::hal::analog::contract::{assert_rails_only_part, UNITY};

    #[test]
    fn command_frame_carries_start_single_ended_and_channel() {
        let mut adc = Mcp3208::new(MockSpi::default(), 3300);
        adc.rail_mv(3, UNITY).unwrap(); // D2 D1 D0 = 011
        adc.rail_mv(5, UNITY).unwrap(); // 101
        adc.rail_mv(0, UNITY).unwrap();
        assert_eq!(adc.spi.frames[0], vec![0x06, 0xC0, 0x00]);
        assert_eq!(adc.spi.frames[1], vec![0x07, 0x40, 0x00]);
        assert_eq!(adc.spi.frames[2], vec![0x06, 0x00, 0x00]);
    }

    #[test]
    fn counts_become_millivolts_of_vref() {
        let mut adc = Mcp3208::new(MockSpi::default(), 3300);
        adc.spi.values[1] = 2048; // half scale: 2048 x 3300 / 4096 = 1650 mV
        adc.spi.values[2] = 4095; // 4095 x 3300 / 4096 = 3298.8 -> 3299 mV
        adc.spi.values[7] = 0x0ABC; // result bits cross the nibble boundary: 2748
        assert_eq!(adc.rail_mv(1, UNITY), Ok(1650));
        assert_eq!(adc.rail_mv(2, UNITY), Ok(3299));
        // 2748 x 3300 = 9 068 400; / 4096 = 2213.9 -> 2214 mV.
        assert_eq!(adc.rail_mv(7, UNITY), Ok(2214));
        // Through a 2 / 1 divider the half-scale reading is 3300 mV.
        assert_eq!(adc.rail_mv(1, Divider { num: 2, den: 1 }), Ok(3300));
    }

    #[test]
    fn bad_channel_bus_fault_and_unmeasured_quantities() {
        let mut adc = Mcp3208::new(MockSpi::default(), 3300);
        assert_rails_only_part(&mut adc, 8, 0, |a| a.spi.fail = true);
    }
}
