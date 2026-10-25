// bsp/systick.rs - 1 ms tick from the Cortex-M4 SysTick timer; counts time and steps the LED pattern.
//
// Ownership: this module owns the SysTick peripheral (moved in at init) and its exception handler.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::interrupt;
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m::peripheral::{SCB, SYST};
use cortex_m_rt::exception;

use super::led;

/// MS_PER_SECOND -- milliseconds per second; the tick period (1 ms) and the seconds counter both derive from it.
pub const MS_PER_SECOND: u32 = 1000;

// Shared state. SECONDS, MILLIS and MS_IN_SECOND: written by the SysTick handler (exception
// context) and by `catch_up` (main loop, inside a critical section, so the handler never runs in
// the middle of it); SECONDS and MILLIS are read by `seconds()` and `millis()` from the main loop.
// CYCLES_PER_MS: written once by `init` before the counter starts, read by `catch_up`. Atomics
// with Relaxed ordering: single-word values on a single-core part, no ordering between them is
// relied on.
static SECONDS: AtomicU32 = AtomicU32::new(0);
static MILLIS: AtomicU32 = AtomicU32::new(0);
static MS_IN_SECOND: AtomicU32 = AtomicU32::new(0);
static CYCLES_PER_MS: AtomicU32 = AtomicU32::new(0);

/// init -- starts the 1 ms tick from HCLK.
///
/// Reload = hclk_hz / MS_PER_SECOND - 1 counts (84 000 counts = 1 ms at 84 MHz). The caller
/// passes the decoded nominal HCLK, so the tick is as accurate as the oscillator.
pub fn init(mut syst: SYST, hclk_hz: u32) {
    CYCLES_PER_MS.store(hclk_hz / MS_PER_SECOND, Ordering::Relaxed);
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(hclk_hz / MS_PER_SECOND - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();
}

/// cycles_per_ms -- core clock cycles in one millisecond, as `init` computed it (0 before
/// `init`); the one source for converting DWT cycle counts to milliseconds.
pub fn cycles_per_ms() -> u32 {
    CYCLES_PER_MS.load(Ordering::Relaxed)
}

/// seconds -- whole seconds since the tick started (wraps at 2^32).
pub fn seconds() -> u32 {
    SECONDS.load(Ordering::Relaxed)
}

/// millis -- milliseconds since the tick started (wraps at 2^32, about 49.7 days). Callers
/// measure intervals with wrapping subtraction.
pub fn millis() -> u32 {
    MILLIS.load(Ordering::Relaxed)
}

/// catch_up -- advances the clock over a stall the SysTick exception could not count. The caller
/// is the flash erase (bsp/flash.rs): while it runs the core waits on flash, the counter keeps
/// wrapping but only one exception stays pending, so up to the 500 ms sector erase time (16 KB sector, DS10086 Table 45) is lost
/// from `millis()` and `seconds()`.
/// `start_ms` is `millis()` and `stall_cycles` the DWT cycle count (core clock, the SysTick
/// source) taken around the stall. The clock gains the stall less the ticks counted meanwhile,
/// the pending one included (a remainder under 1 ms is dropped).
///
/// It keeps every timer that measures with `millis()` true after an erase: the FRU state
/// machine and power-good timeout, the duplicate cache window (IPMB_DUP_WINDOW_MS: a lagging
/// clock keeps an entry past its window and replays a stale response to a reused sequence
/// number), the fault-clear time, the FRU persist delay, the LED pattern and the sensor sample
/// period.
pub fn catch_up(start_ms: u32, stall_cycles: u32) {
    let per_ms = CYCLES_PER_MS.load(Ordering::Relaxed);
    if per_ms == 0 {
        return; // `init` has not run: no clock to advance
    }
    let stall_ms = stall_cycles / per_ms;
    interrupt::free(|_| {
        let now_ms = MILLIS.load(Ordering::Relaxed);
        let counted = now_ms
            .wrapping_sub(start_ms)
            .wrapping_add(u32::from(SCB::is_pendst_pending()));
        let lost = stall_ms.saturating_sub(counted);
        MILLIS.store(now_ms.wrapping_add(lost), Ordering::Relaxed);
        let in_second = MS_IN_SECOND.load(Ordering::Relaxed) + lost;
        SECONDS.store(
            SECONDS
                .load(Ordering::Relaxed)
                .wrapping_add(in_second / MS_PER_SECOND),
            Ordering::Relaxed,
        );
        MS_IN_SECOND.store(in_second % MS_PER_SECOND, Ordering::Relaxed);
    });
}

#[exception]
fn SysTick() {
    let now_ms = MILLIS.load(Ordering::Relaxed).wrapping_add(1);
    MILLIS.store(now_ms, Ordering::Relaxed);
    let ms_in_second = MS_IN_SECOND.load(Ordering::Relaxed) + 1;
    if ms_in_second >= MS_PER_SECOND {
        MS_IN_SECOND.store(0, Ordering::Relaxed);
        SECONDS.store(
            SECONDS.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    } else {
        MS_IN_SECOND.store(ms_in_second, Ordering::Relaxed);
    }
    led::tick_1ms(now_ms);
}
