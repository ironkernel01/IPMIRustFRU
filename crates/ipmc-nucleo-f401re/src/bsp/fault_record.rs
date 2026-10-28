// fault_record.rs - the four NOINIT words of the kept fault record: raw access only.
//
// Ownership: this module owns the four words at `__fault_record` (flash_map.x, the last 16 bytes
// of NOINIT). No start-up code of either image clears NOINIT, so a record written by a fault
// handler or the panic handler survives the watchdog reset and the runtime reports it at its next
// start (`take`). Layout: KIND, VALUE, AUX, CHECK. A debugger reads the same words.

use core::ptr::addr_of_mut;

use crate::bsp::hal_impl::FaultWords;

/// WORDS -- words of the record.
const WORDS: usize = 4;

// The record is the words of `Fault::to_words` and a check word keyed by FAULT_MAGIC (ipmc-hal).
pub use ipmc_hal::fault::Fault;
use ipmc_hal::fault::FAULT_MAGIC;
use ipmc_hal::kept::{store_checked, take_checked};

extern "C" {
    /// The record: `__fault_record` of flash_map.x, the `WORDS` words at ORIGIN(NOINIT) + 0xF0.
    /// Accessed only through its address (`addr_of_mut!`) with volatile writes and the raw word
    /// read.
    static mut __fault_record: [u32; WORDS];
}

/// words -- the address of the first word (`FaultWords` reads and writes through it).
pub(crate) fn words() -> *mut u32 {
    addr_of_mut!(__fault_record).cast::<u32>()
}

/// write -- stores `fault` with volatile writes (a fault handler or the panic handler; the check
/// word goes last).
pub fn write(fault: Fault) {
    // `FaultWords` writes the `WORDS` words at the end of NOINIT (see `words`); only exception
    // context and the panic handler write it, and they park afterwards.
    store_checked(&mut FaultWords, fault.to_words(), FAULT_MAGIC);
}

/// take -- the record `write` stored, or None when the check word does not match or the kind is
/// unknown. Clears the check word, so a later reset without a fault reads None. Call it once at
/// start-up.
pub fn take() -> Option<Fault> {
    // The main thread calls this once at start-up, before any fault handler can run.
    let words = take_checked(&mut FaultWords, FAULT_MAGIC)?;
    Fault::from_words(words)
}
