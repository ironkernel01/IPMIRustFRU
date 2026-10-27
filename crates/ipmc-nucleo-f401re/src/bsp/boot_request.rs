// boot_request.rs - the four NOINIT words of the boot request (Documentation/ipmc_boot_plan.md
// 7.1): raw access only. The meaning of the words (magic, interface, check) is
// `ipmc_boot::request`; the callers use that.
//
// Ownership: this module owns the four words at `__boot_request` (flash_map.x, the start of the
// NOINIT region). No start-up code of either image clears NOINIT, so the words keep their value
// through SYSRESETREQ and through the bootloader.

use core::ptr::addr_of_mut;

use crate::bsp::kept_words;

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
    // SAFETY: `__boot_request` is the `WORDS` words at the start of NOINIT (flash_map.x), owned by
    // this module; the main loop is the only caller and no interrupt handler writes it.
    unsafe { kept_words::read::<WORDS>(words()) }
}

/// write -- volatile writes of the four words (the runtime, before `system_reset`).
pub fn write(request: [u32; WORDS]) {
    // SAFETY: `__boot_request` is declared as the `WORDS` words at the start of NOINIT
    // (flash_map.x), which both link scripts leave to these words (the runtime's `.uninit` starts
    // 16 bytes later, the bootloader has no section there) and which no start-up code clears; the
    // main loop is the only caller and no interrupt handler touches it.
    unsafe { kept_words::write::<WORDS>(words(), request) };
}

/// clear -- zeroes the check word (word 3), so a later reset reads no request.
pub fn clear() {
    // SAFETY: as in `write`; `CHECK < WORDS`.
    unsafe { kept_words::clear(words(), CHECK) };
}
