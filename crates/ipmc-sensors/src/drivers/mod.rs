//! drivers - the processor-independent implementations of the `hal` traits. The on-chip drivers of a
//! board (ADC, GPIO) live in that board's crate.
//!
//! - `sim`: in-memory simulators (always built).
//! - `i2c`, `spi`: example external parts, generic over the embedded-hal 1.0 bus traits. They
//!   are built and unit-tested on the host and are not wired into the firmware: the STM32's
//!   I2C1 and I2C3 are the IPMB buses and no bus is assigned to external parts yet.

pub mod i2c;
pub mod sim;
pub mod spi;
