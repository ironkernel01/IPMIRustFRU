// bsp/reset_cause.rs - decodes and clears the reset flags in RCC_CSR (RM0368 6.3.21).
//
// Flags accumulate until RMVF clears them, so they are read once at start-up. Several
// flags can be set together (power-on also sets PINRSTF and BORRSTF), so the most
// specific cause is chosen first and the reset pin is reported last.

use stm32f4::stm32f401::RCC;

/// ResetCause -- why the MCU last reset.
#[derive(Clone, Copy)]
pub enum ResetCause {
    LowPower,
    WindowWatchdog,
    IndependentWatchdog,
    Software,
    PowerOn,
    BrownOut,
    Pin,
    Unknown,
}

impl ResetCause {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            ResetCause::LowPower => "low-power management",
            ResetCause::WindowWatchdog => "window watchdog",
            ResetCause::IndependentWatchdog => "independent watchdog",
            ResetCause::Software => "software reset",
            ResetCause::PowerOn => "power-on",
            ResetCause::BrownOut => "brown-out",
            ResetCause::Pin => "reset pin (button or debugger)",
            ResetCause::Unknown => "unknown",
        }
    }
}

/// take -- reads the reset flags, then clears them.
pub fn take(rcc: &RCC) -> ResetCause {
    let csr = rcc.csr().read();
    let cause = if csr.lpwrrstf().bit_is_set() {
        ResetCause::LowPower
    } else if csr.wwdgrstf().bit_is_set() {
        ResetCause::WindowWatchdog
    } else if csr.wdgrstf().bit_is_set() {
        ResetCause::IndependentWatchdog
    } else if csr.sftrstf().bit_is_set() {
        ResetCause::Software
    } else if csr.porrstf().bit_is_set() {
        ResetCause::PowerOn
    } else if csr.borrstf().bit_is_set() {
        ResetCause::BrownOut
    } else if csr.padrstf().bit_is_set() {
        ResetCause::Pin
    } else {
        ResetCause::Unknown
    };
    rcc.csr().modify(|_, w| w.rmvf().clear());
    cause
}
