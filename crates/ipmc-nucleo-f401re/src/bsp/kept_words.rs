// kept_words.rs - the one raw access to the NOINIT words that survive a reset: read, volatile
// write, clear and the checked four-word record (three data words and a check word).
//
// Ownership: the callers own the words (`__fault_record` in fault_record.rs, `KEPT` in
// reset_cause.rs, `__boot_request` in boot_request.rs); this module owns only the access pattern.
// Each caller keeps its symbol, layout, magic and decoding. `raw::read_raw_word` is the one raw
// read.

use core::ptr::write_volatile;

use crate::bsp::raw::read_raw_word;

/// CHECKED_DATA -- data words of a checked record; the check word follows them.
const CHECKED_DATA: usize = 3;
/// CHECKED_WORDS -- words of a checked record.
const CHECKED_WORDS: usize = CHECKED_DATA + 1;

/// read -- the `N` words at `base` (the raw word read, whatever the RAM holds).
///
/// # Safety
/// `base` points at `N` aligned words of NOINIT that the caller's module owns, and no writer runs
/// concurrently.
pub unsafe fn read<const N: usize>(base: *mut u32) -> [u32; N] {
    core::array::from_fn(|index| read_raw_word(base.wrapping_add(index)))
}

/// write -- volatile writes of the `N` words in index order (a check word goes last).
///
/// # Safety
/// `base` points at `N` aligned words of NOINIT that the caller's module owns, and no concurrent
/// writer runs. Volatile writes keep the compiler from dropping them before a reset.
pub unsafe fn write<const N: usize>(base: *mut u32, words: [u32; N]) {
    for (index, word) in words.into_iter().enumerate() {
        // SAFETY: `index < N` and the caller's contract covers the `N` words at `base`.
        unsafe { write_volatile(base.add(index), word) };
    }
}

/// clear -- volatile write of 0 to word `index`.
///
/// # Safety
/// `base` points at more than `index` aligned words of NOINIT that the caller's module owns, and
/// no concurrent writer runs.
pub unsafe fn clear(base: *mut u32, index: usize) {
    // SAFETY: the caller's contract covers word `index` at `base`.
    unsafe { write_volatile(base.add(index), 0) };
}

/// store_checked -- the three data words and their check word `ipmc_wire::check_word(data, magic)`.
///
/// # Safety
/// `base` points at four aligned words of NOINIT that the caller's module owns, and no concurrent
/// writer runs.
pub unsafe fn store_checked(base: *mut u32, data: [u32; CHECKED_DATA], magic: u32) {
    let [a, b, c] = data;
    // SAFETY: the caller's contract covers the four words at `base`.
    unsafe { write::<CHECKED_WORDS>(base, [a, b, c, ipmc_wire::check_word(data, magic)]) };
}

/// take_checked -- reads the four words, clears the check word, and returns the data words when
/// the check word matches `magic` (None otherwise).
///
/// # Safety
/// As `store_checked`.
pub unsafe fn take_checked(base: *mut u32, magic: u32) -> Option<[u32; CHECKED_DATA]> {
    // SAFETY: the caller's contract covers the four words at `base`.
    let [a, b, c, check] = unsafe { read::<CHECKED_WORDS>(base) };
    // SAFETY: as above; word `CHECKED_DATA` is the check word.
    unsafe { clear(base, CHECKED_DATA) };
    let data = [a, b, c];
    (check == ipmc_wire::check_word(data, magic)).then_some(data)
}
