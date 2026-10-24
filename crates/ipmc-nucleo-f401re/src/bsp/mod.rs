// bsp/mod.rs - board support: one module per hardware access path.

pub mod board_pins;
pub mod clock;
#[cfg(ipmc_eval)]
pub mod eval_timer;
pub mod fault;
pub mod flash;
pub mod ga;
pub mod i2c_bitbang;
pub mod i2c_hw;
pub mod ipmb_common;
pub mod ipmb_master;
pub mod ipmb_pins;
pub mod led;
pub mod reset_cause;
pub mod static_slot;
pub mod systick;
pub mod uart;
