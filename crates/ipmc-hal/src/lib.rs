// lib.rs - ipmc-hal: the boundary between the IPMC code and one board.
//
// Holds the data types a board and the IPMC code both name (IPMB master errors and counters, the
// received frame, serial counters, reset cause, watchdog error, fault record), the traits a board
// implements (clock, cycle counter, serial port, IPMB buses, system control, watchdog, straps, LED,
// kept words, `Board`, `Platform`) and the interrupt-side frame state of an I2C target
// (`FrameAssembler`). It holds no hardware, no allocator, no atomic
// read-modify-write and no `core::fmt`, so it builds for the processor, the host and a soft core.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod board;
pub mod fault;
pub mod frame;
pub mod ipmb;
pub mod kept;
pub mod serial;
pub mod system;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod time;
