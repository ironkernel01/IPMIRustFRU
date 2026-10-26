//! spi - example driver for an external SPI part, generic over the embedded-hal 1.0
//! `embedded_hal::spi::SpiDevice` trait: `mcp3208` (12-bit 8-channel ADC, `AnalogSource`).
//!
//! The driver is an example: it builds and is unit-tested on the host against the mock device
//! below, and the firmware does not use it. No SPI bus or chip select is assigned to external
//! parts yet.
//!
//! Adding a new part of this kind (what a board integrator does):
//! 1. Write `drivers/spi/<part>.rs`: a struct holding an `SPI: embedded_hal::spi::SpiDevice`
//!    (the device owns its chip select), a constructor, and `impl AnalogSource` for a
//!    converter. Map a failed transfer to `SourceError::BusError`.
//! 2. Declare it with `pub mod <part>;` here and test it against `mock::MockSpi`.
//! 3. In board_sensors.rs / routing.rs, hold the driver in the matching router (an
//!    `Option<H>` slot) and answer the `Source::Spi` arm with it instead of `Unavailable`.
//! 4. Select it in defs.rs with `IPMC_SRC_SPI` on IPMC_ADC_EXTERNAL_SOURCE (or the switch
//!    of the group the part serves). The firmware build refuses the value until a bus is
//!    assigned: hand the firmware an SPI device and remove the guard in ipmc-nucleo-f401re sensing.rs.

pub mod mcp3208;

/// A scripted MCP3208 for the driver tests: it decodes the channel from the command bytes and
/// answers with the 12-bit value stored for it.
#[cfg(test)]
pub(crate) mod mock {
    use embedded_hal::spi::{ErrorKind, ErrorType, Operation, SpiDevice};

    /// Channels on the part.
    const CHANNELS: usize = 8;

    /// MockSpi -- a scripted MCP3208 for the driver tests; `default()` has every channel at 0.
    #[derive(Default)]
    pub struct MockSpi {
        pub values: [u16; CHANNELS],
        pub fail: bool,
        /// Every command frame sent.
        pub frames: Vec<Vec<u8>>,
    }

    impl ErrorType for MockSpi {
        type Error = ErrorKind;
    }

    impl SpiDevice<u8> for MockSpi {
        fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), ErrorKind> {
            if self.fail {
                return Err(ErrorKind::Other);
            }
            for operation in operations.iter_mut() {
                if let Operation::Transfer(read, write) = operation {
                    self.frames.push(write.to_vec());
                    // Command: start, SGL/DIFF and D2 in byte 0; D1, D0 in the top of byte 1.
                    let channel = (usize::from(write[0] & 1) << 2) | usize::from(write[1] >> 6);
                    let value = self.values[channel];
                    read[1] = (value >> 8) as u8 & 0x0F;
                    read[2] = value as u8;
                }
            }
            Ok(())
        }
    }
}
