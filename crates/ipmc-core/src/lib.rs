//! ipmc-core - hardware-independent IPMC logic.
//!
//! All IPMI logic lives in this crate: no hardware access, no allocator.
//! It builds for the host and for thumbv7em-none-eabihf.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// addr -- geographic address decode and strap-source resolution.
pub mod addr;
/// cmd -- command handlers, one module per NetFn group.
pub mod cmd;
/// commands -- the command table, one row per command.
pub mod commands;
/// dispatch -- request in, response frame queued for transmission.
pub mod dispatch;
/// dupcache -- duplicate-request cache holding answered responses for IPMB_DUP_WINDOW_MS.
pub mod dupcache;
/// fru_state -- the FRU 0 Operational State machine.
pub mod fru_state;
/// identity -- the device identity the IPMC reports.
pub mod identity;
/// ipmb -- IPMB frame parser and encoder, checksums and addressing.
pub mod ipmb;
/// link -- dual-bus IPMB link layer: ingress, duplicate cache, bus choice, resend.
pub mod link;
/// queue -- fixed-capacity egress queue of response frames.
pub mod queue;
/// sdr -- the Device SDR repository and its reservation.
pub mod sdr;
/// selftest -- the startup self test and the result Get Self Test Results reports.
pub mod selftest;
/// sensor -- Tier-1 sensor data types and Get Sensor Reading encoders.
pub mod sensor;
/// trace -- the trace hook, a sink for single formatted lines.
pub mod trace;

/// CORE_VERSION -- crate version, reported in the firmware banner.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
