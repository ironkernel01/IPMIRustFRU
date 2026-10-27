// boot_request.rs - the four NOINIT words of the boot request (Documentation/ipmc_boot_plan.md
// 7.1): raw access only. The meaning of the words (magic, interface, check) is
// `ipmc_boot::request`; the callers use that.
//
// Ownership: this module owns the four words at `__boot_request` (flash_map.x, the start of the
// NOINIT region). No start-up code of either image clears NOINIT, so the words keep their value
// through SYSRESETREQ and through the bootloader.

use core::ptr::{addr_of_mut, write_volatile};

use crate::bsp::raw::read_raw_word;

/// WORDS -- words of the boot request.
pub const WORDS: usize = 4;
/// Word position of the check word.
const CHECK: usize = WORDS - 1;

extern "C" {
    /// The boot request: `__boot_request` of flash_map.x (ORIGIN(NOINIT)), the `WORDS` words at
    /// the start of NOINIT. Accessed only through its address (`addr_of_mut!`) with volatile
    /// writes and the raw word read.
    static mut __boot_request: [u32; WORDS];
}

/// words -- the address of the first request word.
fn words() -> *mut u32 {
    addr_of_mut!(__boot_request).cast::<u32>()
}

/// read -- the four words as integers, however the RAM got its value (the raw word read).
pub fn read() -> [u32; WORDS] {
    core::array::from_fn(|index| read_raw_word(words().wrapping_add(index)))
}

/// write -- volatile writes of the four words (the runtime, before `system_reset`).
pub fn write(request: [u32; WORDS]) {
    let base = words();
    for (index, word) in request.into_iter().enumerate() {
        // SAFETY: `__boot_request` is declared as the `WORDS` words at the start of NOINIT
        // (flash_map.x), which both link scripts leave to these words (the runtime's `.uninit`
        // starts 16 bytes later, the bootloader has no section there) and which no start-up code
        // clears; `index < WORDS`; the main loop is the only caller and no interrupt handler
        // touches it. Volatile writes keep the compiler from dropping them before the reset.
        unsafe { write_volatile(base.add(index), word) };
    }
}

/// clear -- zeroes the check word (word 3), so a later reset reads no request.
pub fn clear() {
    // SAFETY: as in `write`; `CHECK < WORDS`.
    unsafe { write_volatile(words().add(CHECK), 0) };
}
