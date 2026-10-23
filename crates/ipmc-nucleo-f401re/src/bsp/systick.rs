// bsp/systick.rs - 1 ms tick from the Cortex-M4 SysTick timer; counts time and steps the LED pattern.
//
// Ownership: this module owns the SysTick peripheral (moved in at init) and its exception handler.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m::peripheral::SYST;
use cortex_m_rt::exception;

use super::led;

/// MS_PER_SECOND -- milliseconds per second; the tick period (1 ms) and the seconds counter both derive from it.
pub const MS_PER_SECOND: u32 = 1000;

// Shared state. SECONDS and MILLIS: written only by the SysTick handler (exception context),
// read by `seconds()` and `millis()` from the main loop. MS_IN_SECOND: written and read only by
// the SysTick handler. Atomics with Relaxed ordering: single-word values on a single-core part,
// no ordering between them is relied on.
static SECONDS: AtomicU32 = AtomicU32::new(0);
static MILLIS: AtomicU32 = AtomicU32::new(0);
static MS_IN_SECOND: AtomicU32 = AtomicU32::new(0);

/// init -- starts the 1 ms tick from HCLK.
///
/// Reload = hclk_hz / MS_PER_SECOND - 1 counts (84 000 counts = 1 ms at 84 MHz). The caller
/// passes the decoded nominal HCLK, so the tick is as accurate as the oscillator.
pub fn init(mut syst: SYST, hclk_hz: u32) {
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(hclk_hz / MS_PER_SECOND - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();
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
