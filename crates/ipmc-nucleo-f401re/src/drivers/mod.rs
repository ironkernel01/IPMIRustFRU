// drivers/mod.rs - the hardware only the runtime firmware uses: IPMB masters (bit-bang and
// hardware I2C), the I2C slaves, the IPMB and board pins, the FRU store driver, and the
// evaluation timer. The shared board support is `crate::bsp`.

pub mod adc;
pub mod board_pins;
#[cfg(ipmc_eval)]
pub mod eval_timer;
pub mod flash;
pub mod gpio;
pub mod i2c_bitbang;
pub mod i2c_hw;
pub mod ipmb_bus;
pub mod ipmb_common;
pub mod ipmb_master;
pub mod ipmb_pins;
