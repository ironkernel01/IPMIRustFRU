//! i2c - example drivers for external I2C parts, generic over the embedded-hal 1.0
//! `embedded_hal::i2c::I2c` trait: `ina226` (bus-voltage monitor, `AnalogSource`) and `pca9555`
//! (16-bit I/O expander, `DigitalInputs` and `PowerSwitch`).
//!
//! The drivers are examples: they build and are unit-tested on the host against the mock bus
//! below, and the firmware does not use them. The STM32's I2C1 and I2C3 are the IPMB buses and
//! no bus is assigned to external parts yet.
//!
//! Adding a new part of this kind (what a board integrator does):
//! 1. Write `drivers/i2c/<part>.rs`: a struct holding an `I2C: embedded_hal::i2c::I2c` and the
//!    7-bit address, a constructor that reads the part's ID register and writes its
//!    configuration, and `impl` of the hal trait(s) the part serves (`AnalogSource` for a
//!    converter or monitor, `DigitalInputs` and `PowerSwitch` for an expander). Map the bus
//!    error to `SourceError::BusError` and an ID mismatch to `SourceError::WrongDevice`.
//! 2. Declare it with `pub mod <part>;` here and test it against `mock::MockI2c`.
//! 3. In board_sensors.rs / routing.rs, hold the driver in the matching router (an
//!    `Option<H>` slot) and answer the `Source::I2c` arm with it instead of `Unavailable`.
//! 4. Select it in ipmi_defs.h with `IPMC_SRC_I2C` on the group's switch (IPMC_ADC_EXTERNAL_SOURCE,
//!    IPMC_FAULT_SOURCE or IPMC_POWER_SOURCE). The firmware build refuses the value until a
//!    bus is assigned: hand the firmware an I2C peripheral that is not an IPMB bus and remove
//!    the guard in ipmc-nucleo-f401re sensing.rs.

pub mod ina226;
pub mod pca9555;

/// A register-file I2C slave for the driver tests: a command byte selects a register, writes
/// store the following bytes from there, reads return the bytes from the selected register on.
#[cfg(test)]
pub(crate) mod mock {
    use embedded_hal::i2c::{ErrorKind, ErrorType, I2c, NoAcknowledgeSource, Operation};

    /// Bytes of register space (256 registers of up to two bytes).
    const SPACE: usize = 512;

    /// MockI2c -- a register-file I2C slave for the driver tests.
    pub struct MockI2c {
        pub address: u8,
        /// Bytes per register: 2 for 16-bit big-endian registers, 1 for 8-bit.
        pub stride: usize,
        pub regs: [u8; SPACE],
        pub fail: bool,
        /// Every write transaction, command byte first.
        pub writes: Vec<Vec<u8>>,
    }

    impl MockI2c {
        /// new -- a mock at `address` whose registers are `stride` bytes wide.
        pub fn new(address: u8, stride: usize) -> Self {
            MockI2c {
                address,
                stride,
                regs: [0; SPACE],
                fail: false,
                writes: Vec::new(),
            }
        }

        /// set_u16 -- stores a 16-bit big-endian register (stride 2).
        pub fn set_u16(&mut self, reg: u8, value: u16) {
            let at = usize::from(reg) * self.stride;
            self.regs[at..at + 2].copy_from_slice(&value.to_be_bytes());
        }

        /// u16_at -- the 16-bit big-endian register `reg` (stride 2).
        pub fn u16_at(&self, reg: u8) -> u16 {
            let at = usize::from(reg) * self.stride;
            u16::from_be_bytes([self.regs[at], self.regs[at + 1]])
        }
    }

    impl ErrorType for MockI2c {
        type Error = ErrorKind;
    }

    impl I2c for MockI2c {
        fn transaction(
            &mut self,
            address: u8,
            operations: &mut [Operation<'_>],
        ) -> Result<(), ErrorKind> {
            if self.fail {
                return Err(ErrorKind::Other);
            }
            if address != self.address {
                return Err(ErrorKind::NoAcknowledge(NoAcknowledgeSource::Address));
            }
            let mut pointer = 0usize;
            for operation in operations.iter_mut() {
                match operation {
                    Operation::Write(data) => {
                        self.writes.push(data.to_vec());
                        if let Some((&command, payload)) = data.split_first() {
                            pointer = usize::from(command) * self.stride;
                            for byte in payload {
                                self.regs[pointer] = *byte;
                                pointer += 1;
                            }
                        }
                    }
                    Operation::Read(buffer) => {
                        for byte in buffer.iter_mut() {
                            *byte = self.regs[pointer];
                            pointer += 1;
                        }
                    }
                }
            }
            Ok(())
        }
    }
}
