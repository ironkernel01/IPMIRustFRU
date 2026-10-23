//! eval - hardware-free evaluation. Depends only on the `hal` traits and ipmc-defs.
//!
//! `led` is the status LED level, `rail` judges a rail voltage against its windows,
//! `temperature` converts calibration data and judges thresholds, `faults` turns the digital inputs into a bitmap, `power_fsm` is the
//! PWR_EN / PGOOD state machine. Each decision exists once; `board_sensors.rs` supplies the
//! tables and combines the results.

pub mod faults;
pub mod led;
pub mod power_fsm;
pub mod rail;
pub mod temperature;
