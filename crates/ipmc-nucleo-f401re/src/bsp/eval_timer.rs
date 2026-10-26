// bsp/eval_timer.rs - the evaluation run-time clock: TIM2 as a free-running 32-bit up-counter.
// Compiled only for the evaluation build (`cfg(ipmc_eval)`, IPMC_EVAL=1).
//
// Ownership: this module owns the TIM2 registers (the peripheral is moved into `start`). It
// does not use SysTick: the evaluation limit is measured by a CPU timer, read through the BSP.
//
// TIM2 sits on APB1. Its timer clock is PCLK1 when the APB1 prescaler is 1 and 2 x PCLK1 for
// any other prescaler (RM0368 6.2, "Timer clock"); `timer_clock_hz` computes that from the
// decoded clock tree. The prescaler divides it to TICK_HZ, ARR = FFFF_FFFFh, so the counter
// runs to 2^32 ticks (119 hours at 10 kHz) before it wraps.

use stm32f4::stm32f401::{RCC, TIM2};

use super::clock::ClockInfo;

/// TICK_HZ -- counter rate, Hz: 10 kHz gives 0.1 ms resolution and 7158 minutes of range in
/// 32 bits, far past any evaluation length. The defs_tier23.rs guard on
/// IPMC_EVAL_MINUTES (defs_guards.rs) assumes this rate.
pub const TICK_HZ: u32 = 10_000;

/// Largest PSC register value (16-bit field, RM0368 13.4.11).
const PSC_MAX: u32 = 0xFFFF;

/// StartError -- why the counter does not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// The timer clock is not a whole multiple of TICK_HZ within the 16-bit prescaler.
    TickRate,
}

/// timer_clock_hz -- the TIM2 input clock for a decoded clock tree: PCLK1 when the APB1
/// prescaler is 1 (PCLK1 equals HCLK), else 2 x PCLK1 (RM0368 6.2).
pub const fn timer_clock_hz(hclk_hz: u32, pclk1_hz: u32) -> u32 {
    if pclk1_hz == hclk_hz {
        pclk1_hz
    } else {
        2 * pclk1_hz
    }
}

/// prescaler -- the PSC value that divides `timer_clock_hz` to TICK_HZ; None when the division
/// is not exact or does not fit the 16-bit register (reject, not round).
pub const fn prescaler(timer_clock_hz: u32) -> Option<u32> {
    if timer_clock_hz < TICK_HZ || !timer_clock_hz.is_multiple_of(TICK_HZ) {
        return None;
    }
    let psc = timer_clock_hz / TICK_HZ - 1;
    if psc > PSC_MAX {
        None
    } else {
        Some(psc)
    }
}

/// start -- enables the TIM2 clock and starts the counter at 0, once, at boot.
pub fn start(rcc: &RCC, tim2: TIM2, clock: &ClockInfo) -> Result<(), StartError> {
    let psc =
        prescaler(timer_clock_hz(clock.hclk_hz, clock.pclk1_hz)).ok_or(StartError::TickRate)?;
    rcc.apb1enr().modify(|_, w| w.tim2en().enabled());
    // Read back: short delay after enabling a clock.
    let _ = rcc.apb1enr().read();
    // SAFETY: PSC is a 16-bit field and `prescaler` returned at most PSC_MAX; ARR is 32 bits.
    tim2.psc().write(|w| unsafe { w.psc().bits(psc as u16) });
    tim2.arr().write(|w| unsafe { w.bits(u32::MAX) });
    // Update event: loads the prescaler and zeroes the counter (RM0368 13.4.9).
    tim2.egr().write(|w| w.ug().update());
    tim2.cr1().modify(|_, w| w.cen().enabled());
    Ok(())
}

/// elapsed_ticks -- TICK_HZ ticks since `start` (wraps at 2^32; 0 before `start`).
pub fn elapsed_ticks() -> u32 {
    // SAFETY: a read of CNT has no side effect; TIM2 is only written by `start`, at boot.
    unsafe { (*TIM2::ptr()).cnt().read().bits() }
}
