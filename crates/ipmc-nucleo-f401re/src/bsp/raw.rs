// raw.rs - the one raw word read of the RAM words that survive a software reset (`KEPT` in
// reset_cause.rs, the boot request in boot_request.rs).

/// read_raw_word -- the word at `p` as an integer, however the memory got its value.
///
/// `p` must be an aligned word inside a static that the calling module owns.
pub(crate) fn read_raw_word(p: *const u32) -> u32 {
    let value: u32;
    // SAFETY: the callers pass an aligned word inside a static they own, so the load is valid. An
    // `asm!` output is an initialised integer whatever the RAM held (a `read_volatile` of
    // never-written memory is not); the value is only an integer, and the check word decides
    // whether it means anything.
    unsafe {
        core::arch::asm!(
            "ldr {v}, [{p}]",
            p = in(reg) p,
            v = out(reg) value,
            options(nostack, readonly, preserves_flags)
        );
    }
    value
}
