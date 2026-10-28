// time.rs - the time sources of a board: the 1 ms tick and the free-running cycle counter.

/// Clock -- the 1 ms tick: milliseconds (wrapping) and whole seconds since start.
pub trait Clock {
    /// millis -- milliseconds since the tick started (wraps at 2^32); intervals are measured with
    /// wrapping subtraction.
    fn millis(&self) -> u32;
    /// seconds -- whole seconds since the tick started (wraps at 2^32).
    fn seconds(&self) -> u32;
}

/// CycleCounter -- a free-running core-cycle counter that keeps counting through a flash stall.
pub trait CycleCounter {
    /// now -- the count (wrapping).
    fn now(&self) -> u32;
    /// hz -- counts per second; per_ms = hz / 1000 and per_us = hz / 1_000_000.
    fn hz(&self) -> u32;
}

/// HZ_PER_MHZ -- counts per microsecond at 1 MHz.
const HZ_PER_MHZ: u32 = 1_000_000;

/// delay_us -- busy-waits `us` microseconds of core time on the cycle counter. Bounded by the
/// conversion itself: at most `ticks` polls run (one poll takes at least one core cycle), so a
/// counter that does not advance ends the wait after at least the requested time.
pub fn delay_us<C: CycleCounter>(counter: &C, us: u32) {
    let ticks = us.saturating_mul(counter.hz() / HZ_PER_MHZ);
    let start = counter.now();
    for _ in 0..ticks {
        if counter.now().wrapping_sub(start) >= ticks {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// Steps -- a counter that advances by one on every read.
    struct Steps {
        count: Cell<u32>,
        hz: u32,
    }

    impl CycleCounter for Steps {
        fn now(&self) -> u32 {
            let value = self.count.get();
            self.count.set(value.wrapping_add(1));
            value
        }

        fn hz(&self) -> u32 {
            self.hz
        }
    }

    /// Frozen -- a counter that never advances.
    struct Frozen;

    impl CycleCounter for Frozen {
        fn now(&self) -> u32 {
            0
        }

        fn hz(&self) -> u32 {
            84_000_000
        }
    }

    #[test]
    fn delay_waits_the_requested_cycles_and_ends_when_frozen() {
        let steps = Steps {
            count: Cell::new(u32::MAX - 100),
            hz: 84_000_000,
        };
        delay_us(&steps, 2); // 168 ticks, across the wrap
        assert!(steps.count.get().wrapping_sub(u32::MAX - 100) >= 168);
        delay_us(&Frozen, 3); // ends after the bounded polls
        delay_us(&Frozen, 0);
    }
}
