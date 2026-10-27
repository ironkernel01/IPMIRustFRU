//! clock.rs - comparisons on the wrapping 32-bit millisecond clock (SysTick time, the emulator's
//! clock, the stamps of received frames). One definition of "at or after" and "earlier".

/// HALF_RANGE -- a time difference below this many milliseconds counts as "after"; from it up the
/// first stamp lies in the future. About 24.8 days; no timer or stamp of the IPMC spans more.
const HALF_RANGE: u32 = 1 << 31;

/// at_or_after -- `now_ms` is at or after `at_ms` on the wrapping clock (a difference of less
/// than half the range counts as "after").
pub const fn at_or_after(now_ms: u32, at_ms: u32) -> bool {
    now_ms.wrapping_sub(at_ms) < HALF_RANGE
}

/// earlier -- stamp `a_ms` is strictly before stamp `b_ms` on the wrapping clock.
pub const fn earlier(a_ms: u32, b_ms: u32) -> bool {
    !at_or_after(a_ms, b_ms)
}

#[cfg(test)]
mod tests {
    use super::{at_or_after, earlier};

    #[test]
    fn the_comparisons_hold_across_the_wrap() {
        // (now, at, at_or_after): equal, one before, one after, and the same across u32::MAX.
        for (now, at, after) in [
            (10, 10, true),
            (9, 10, false),
            (11, 10, true),
            (2, u32::MAX - 1, true),
            (u32::MAX - 1, 2, false),
            (0, u32::MAX, true),
            (u32::MAX, 0, false),
            (0x8000_0000, 0, false),
            (0x7FFF_FFFF, 0, true),
        ] {
            assert_eq!(at_or_after(now, at), after, "at_or_after({now}, {at})");
            assert_eq!(earlier(now, at), !after, "earlier({now}, {at})");
        }
    }
}
