//! ipmc-sensors - sensors and board I/O of the IPMC: voltage rails, die temperature, fault
//! inputs and the payload power enable.
//!
//! This crate is a hardware abstraction layer (PLAN.md 3.6). `ipmc-core::sensor` holds the
//! snapshot type and the Get Sensor Reading encoders; this crate acquires and evaluates.
//!   hal/              the three traits the sensor logic uses (no hardware, no logic)
//!   eval/             hardware-free evaluation: rails, temperature, faults, power state machine
//!   drivers/          implementations of the traits: sim, i2c, spi (no processor code)
//!   routing.rs        picks the driver per defs.rs source switch
//!   board_sensors.rs  the board table and the snapshot builder
//!   pin_claims.rs     GPIO pin ownership table and register-field masks (hardware-free)
//!   reset_pulse.rs    the timing of one payload reset pulse (hardware-free)
//!   monitor.rs, trace.rs  the sampling step and its console lines
//! Every module is hardware-free and host-tested; the on-chip drivers of a board (ADC, GPIO, pin
//! ports) are in that board's crate.
//! Start reading at `board_sensors.rs`: it lists every value and how each Tier-1 reading is built.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

pub mod analog;
pub mod board_sensors;
pub mod drivers;
pub mod eval;
pub mod hal;
pub mod monitor;
pub mod pin_claims;
pub mod reset_pulse;
pub mod routing;
pub mod trace;
