// bsp/mod.rs - the board support both images of the NUCLEO-F401RE use (the runtime firmware and
// the bootloader (sectors 0-1)): one module per hardware access path, each with its ownership in its
// head comment. The runtime-only hardware (IPMB masters and pins, board pins, the FRU store
// driver) is in `drivers`, in the runtime binary's tree.

pub mod boot_request;
pub mod clock;
pub mod cycles;
pub mod fault;
pub mod fault_record;
pub mod flash_regs;
pub mod ga;
pub mod hal_impl;
pub mod kept_words;
pub mod led;
pub mod ports;
mod raw;
pub mod reset_cause;
pub mod static_slot;
pub mod systick;
pub mod uart;
pub mod watchdog;
