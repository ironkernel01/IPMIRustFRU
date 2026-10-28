// lib.rs - ipmc-runtime: the IPMC firmware that runs on any board, generic over the ipmc-hal
// traits.
//
// Holds the console text layer (`text`), the console with its command tables and handlers
// (`console`, `data_cmds`, `debug_cmds`, `log_cmds`, `sensing_cmds`, `ipmb_cmds`), the application
// port (`app_port`), the FRU flash log owner (`fru_log`), the sensor service (`sensing`), the
// request sequence kept across a software reset (`kept`), the IPMB link service (`link`) and the
// start-up order with the main loop (`run`). It holds no hardware, no allocator and no `unsafe`,
// so the firmware, the host tests and the emulator link the same code. The simulated board
// (`sim`, feature `sim`) is the host board of the tests and the emulator. A board hands in its
// ipmc-hal handles through `BoardParts` and `SystemParts`; nothing here reads a global.

#![cfg_attr(not(any(test, feature = "sim")), no_std)]
#![forbid(unsafe_code)]

pub mod app_port;
pub mod console;
mod data_cmds;
mod debug_cmds;
pub mod fru_log;
mod ipmb_cmds;
pub mod kept;
pub mod link;
mod log_cmds;
pub mod run;
pub mod sensing;
mod sensing_cmds;
pub mod text;

#[cfg(feature = "sim")]
pub mod sim;
