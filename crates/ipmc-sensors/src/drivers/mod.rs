//! drivers - everything that implements the `hal` traits.
//!
//! - `sim`: in-memory simulators (always built).
//! - `stm32f401`: on-chip ADC and GPIO (feature `stm32f401`; the only code with `unsafe`).
//! - `i2c`, `spi`: example external parts, generic over the embedded-hal 1.0 bus traits. They
//!   are built and unit-tested on the host and are not wired into the firmware: the STM32's
//!   I2C1 and I2C3 are the IPMB buses and no bus is assigned to external parts yet.

pub mod i2c;
pub mod sim;
pub mod spi;
#[cfg(feature = "stm32f401")]
#[allow(unsafe_code)] // register and system-memory access
pub mod stm32f401;
