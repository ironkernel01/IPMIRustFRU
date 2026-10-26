//! ipmc-sensors - sensors and board I/O of the IPMC: voltage rails, die temperature, fault
//! inputs and the payload power enable.
//!
//! This crate is a hardware abstraction layer (PLAN.md 3.6). `ipmc-core::sensor` holds the
//! snapshot type and the Get Sensor Reading encoders; this crate acquires and evaluates.
//!   hal/              the three traits the sensor logic uses (no hardware, no logic)
//!   eval/             hardware-free evaluation: rails, temperature, faults, power state machine
//!   drivers/          implementations of the traits: sim, stm32f401 (feature), i2c, spi
//!   routing.rs        picks the driver per defs.rs source switch
//!   board_sensors.rs  the board table and the snapshot builder
//!   pin_claims.rs     GPIO pin ownership table and register-field masks (hardware-free)
//!   reset_pulse.rs    the timing of one payload reset pulse (hardware-free)
//!   monitor.rs, trace.rs  the sampling step and its console lines
//! The pure modules have no hardware and are host-tested; `drivers::stm32f401` implements the
//! traits on the part.
//! Start reading at `board_sensors.rs`: it lists every value and how each Tier-1 reading is built.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(feature = "stm32f401"), forbid(unsafe_code))]
#![cfg_attr(feature = "stm32f401", deny(unsafe_code))]

pub mod board_sensors;
pub mod drivers;
pub mod eval;
pub mod hal;
pub mod monitor;
pub mod pin_claims;
pub mod reset_pulse;
pub mod routing;
pub mod trace;
