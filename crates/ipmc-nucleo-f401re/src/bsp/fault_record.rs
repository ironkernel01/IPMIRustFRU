// fault_record.rs - the four NOINIT words of the kept fault record: raw access only.
//
// Ownership: this module owns the four words at `__fault_record` (flash_map.x, the last 16 bytes
// of NOINIT). No start-up code of either image clears NOINIT, so a record written by a fault
// handler or the panic handler survives the watchdog reset and the runtime reports it at its next
// start (`take`). Layout: KIND, VALUE, AUX, CHECK. A debugger reads the same words.

use core::ptr::addr_of_mut;

use crate::bsp::kept_words;

/// WORDS -- words of the record.
const WORDS: usize = 4;

/// FAULT_MAGIC -- the check value that marks the words as written by `write`: the check word is
/// the XOR of the other three words and this constant, which random RAM after a power-on matches
/// with probability 2^-32 (as `KEPT_MAGIC` in reset_cause.rs).
const FAULT_MAGIC: u32 = 0xFA17_0C0D;

/// Kind codes in the KIND word.
const KIND_HARDFAULT: u32 = 1;
const KIND_UNHANDLED_IRQ: u32 = 2;
const KIND_PANIC: u32 = 3;

/// Fault -- what the record holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// HardFault; `pc` is the stacked program counter.
    HardFault { pc: u32 },
    /// Unhandled interrupt; `irq` is the number (negative: a system exception).
    UnhandledIrq { irq: i32 },
    /// Panic; the source line and column (0 when the location is unknown).
    Panic { line: u32, column: u32 },
}

extern "C" {
    /// The record: `__fault_record` of flash_map.x, the `WORDS` words at ORIGIN(NOINIT) + 0xF0.
    /// Accessed only through its address (`addr_of_mut!`) with volatile writes and the raw word
    /// read.
    static mut __fault_record: [u32; WORDS];
}

/// words -- the address of the first word.
fn words() -> *mut u32 {
    addr_of_mut!(__fault_record).cast::<u32>()
}

/// write -- stores `fault` with volatile writes (a fault handler or the panic handler; the check
/// word goes last).
pub fn write(fault: Fault) {
    let (kind, value, aux) = match fault {
        Fault::HardFault { pc } => (KIND_HARDFAULT, pc, 0),
        // The cast reinterprets the bits, so a negative system-exception number stays recognisable.
        Fault::UnhandledIrq { irq } => (KIND_UNHANDLED_IRQ, irq as u32, 0),
        Fault::Panic { line, column } => (KIND_PANIC, line, column),
    };
    // SAFETY: `__fault_record` is declared as the `WORDS` words at the end of NOINIT
    // (flash_map.x), which the runtime's `.uninit` stays below (a link.x assert), the bootloader
    // has no section in and no start-up code clears. Only exception context and the panic handler
    // write it, and they park afterwards.
    unsafe { kept_words::store_checked(words(), [kind, value, aux], FAULT_MAGIC) };
}

/// take -- the record `write` stored, or None when the check word does not match or the kind is
/// unknown. Clears the check word, so a later reset without a fault reads None. Call it once at
/// start-up.
pub fn take() -> Option<Fault> {
    // SAFETY: as in `write`; the main thread calls this once at start-up, before any fault
    // handler can run.
    let [kind, value, aux] = unsafe { kept_words::take_checked(words(), FAULT_MAGIC) }?;
    match kind {
        KIND_HARDFAULT => Some(Fault::HardFault { pc: value }),
        // The number was stored as its 32 bits.
        KIND_UNHANDLED_IRQ => Some(Fault::UnhandledIrq { irq: value as i32 }),
        KIND_PANIC => Some(Fault::Panic {
            line: value,
            column: aux,
        }),
        _ => None,
    }
}
