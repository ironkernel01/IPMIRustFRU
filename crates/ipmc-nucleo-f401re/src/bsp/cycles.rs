// cycles.rs - the DWT cycle counter: the one free-running time base of the firmware (the IPMB
// masters' waits, the flash stall catch-up, the bootloader's hash timer).
//
// Ownership: this module owns the DWT cycle counter use; `start_cycle_counter` is the only
// writer of DCB trace enable and DWT_CTRL.CYCCNTENA. The bootloader's hand-over (boot/start.rs)
// switches the counter off with `stop_cycle_counter`.

use cortex_m::peripheral::{DCB, DWT};

/// COUNTER_CHECK_POLLS -- polls of the cycle counter that must show it advancing, at
/// `start_cycle_counter`.
const COUNTER_CHECK_POLLS: u32 = 1_000;

/// CycleCounterError -- the cycle counter does not advance after being enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CycleCounterError;

/// now -- the DWT cycle counter.
pub fn now() -> u32 {
    DWT::cycle_count()
}

/// HZ_PER_MHZ -- core cycles per microsecond at 1 MHz.
const HZ_PER_MHZ: u32 = 1_000_000;

/// delay_us -- busy-waits `us` microseconds of core time (`hclk_hz` is the core clock) on the
/// cycle counter. Bounded by the conversion itself: at most `ticks` polls run (one poll takes at
/// least one core cycle), so a counter that does not advance ends the wait after at least the
/// requested time.
pub fn delay_us(us: u32, hclk_hz: u32) {
    let ticks = us.saturating_mul(hclk_hz / HZ_PER_MHZ);
    let start = now();
    for _ in 0..ticks {
        if now().wrapping_sub(start) >= ticks {
            break;
        }
    }
}

/// stop_cycle_counter -- the reset state of the counter: CYCCNTENA and trace enable cleared (the
/// runtime starts the counter itself).
pub fn stop_cycle_counter(dcb: &mut DCB, dwt: &mut DWT) {
    dwt.disable_cycle_counter();
    dcb.disable_trace();
}

/// start_cycle_counter -- DCB trace enable, then CYCCNT (Cortex-M4 TRM, DWT_CTRL.CYCCNTENA);
/// Err when the counter does not advance.
pub fn start_cycle_counter(dcb: &mut DCB, dwt: &mut DWT) -> Result<(), CycleCounterError> {
    dcb.enable_trace();
    dwt.enable_cycle_counter();
    let first = now();
    if (0..COUNTER_CHECK_POLLS).any(|_| now() != first) {
        Ok(())
    } else {
        Err(CycleCounterError)
    }
}
