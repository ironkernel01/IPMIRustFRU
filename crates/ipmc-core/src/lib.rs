//! ipmc-core - hardware-independent IPMC logic.
//!
//! All IPMI logic lives in this crate: no hardware access, no allocator.
//! It builds for the host and for thumbv7em-none-eabihf.
#![cfg_attr(not(any(test, feature = "recording")), no_std)]
#![forbid(unsafe_code)]

/// addr -- geographic address decode and strap-source resolution.
pub mod addr;
/// analog -- the hook for the analog sensors a build adds (Full Sensor Records and readings).
pub mod analog;
/// clock -- comparisons on the wrapping millisecond clock.
pub mod clock;
/// cmd -- command handlers, one module per NetFn group.
pub mod cmd;
/// commands -- the command table, one row per command.
pub mod commands;
/// dispatch -- request in, response frame queued for transmission.
pub mod dispatch;
/// dupcache -- duplicate-request cache holding answered responses for IPMB_DUP_WINDOW_MS.
pub mod dupcache;
/// event_enable -- the per-sensor event enables.
pub mod event_enable;
/// ext -- the extension hook: the Tier-2/3 commands and their poll hook.
pub mod ext;
/// fru_state -- the FRU 0 Operational State machine.
pub mod fru_state;
/// identity -- the device identity the IPMC reports.
pub mod identity;
/// ipmb -- IPMB frame parser and encoder, checksums and addressing.
pub mod ipmb;
/// link -- dual-bus IPMB link layer: ingress, duplicate cache, bus choice, resend.
pub mod link;
/// names -- the names and bounded text the link layer and the printers share.
pub mod names;
/// queue -- fixed-capacity egress queue of response frames.
pub mod queue;
/// requester -- the IPMB requester: the one request this IPMC sends at a time (Tier-2/3).
pub mod requester;
/// reset_gate -- when a pending processor reset may run.
pub mod reset_gate;
/// ring -- the byte ring shared by the capture log (ipmc-log) and the UART driver.
pub mod ring;
/// sdr -- the Device SDR repository and its reservation.
pub mod sdr;
/// selftest -- the startup self test and the result Get Self Test Results reports.
pub mod selftest;
/// sensor -- Tier-1 sensor data types and Get Sensor Reading encoders.
pub mod sensor;
/// tier -- the choices derived from the Tier the IPMC reports.
pub mod tier;
/// trace -- the trace hook, a sink for single formatted lines.
pub mod trace;

/// CORE_VERSION -- crate version, reported in the firmware banner.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
