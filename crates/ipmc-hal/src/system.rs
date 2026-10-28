// system.rs - processor-level services of the firmware loop and their plain data: the reset cause,
// the watchdog, the straps, the status LED and the clock summary.

pub use ipmc_sensors::eval::led;

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

/// WatchdogError -- why `start` did not leave a running watchdog with the requested period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchdogError {
    /// The period is 0 or above what the /256 prescaler and the 12-bit reload reach.
    PeriodOutOfRange,
    /// PVU or RVU stayed set: the prescaler or reload update did not finish. The IWDG runs
    /// (nothing stops it) with the values it has, and `feed` still reloads it.
    NotReady,
}

impl WatchdogError {
    /// name -- text for the console.
    pub fn name(self) -> &'static str {
        match self {
            WatchdogError::PeriodOutOfRange => "PeriodOutOfRange",
            WatchdogError::NotReady => "NotReady",
        }
    }
}

/// SystemControl -- processor-level services of the firmware loop.
pub trait SystemControl {
    /// system_reset -- restarts the processor; the reset cause then reads `Software`.
    fn system_reset(&mut self) -> !;
    /// park -- the terminal fail-stop loop.
    fn park(&self) -> !;
    /// wait_for_interrupt -- sleeps until the next interrupt.
    fn wait_for_interrupt(&self);
}

/// Watchdog -- an independent watchdog: once started only a reset stops it.
pub trait Watchdog {
    /// start -- starts the watchdog with a period of at most `period_ms`; returns the period set.
    /// A second call returns the first period and changes nothing.
    fn start(&mut self, period_ms: u32) -> Result<u32, WatchdogError>;
    /// feed -- reloads the counter; does nothing before the watchdog runs.
    fn feed(&mut self);
    /// period_ms -- the period `start` set, None before it returned Ok.
    fn period_ms(&self) -> Option<u32>;
}

/// Straps -- the geographic address, GAP, NVMRO and bootloader strap levels.
pub trait Straps {
    /// read_raw -- the raw strap levels, bit n = strap n (the `ipmc_core::addr` convention).
    fn read_raw(&self) -> u8;
    /// bootloader_pin_high -- the live level of the BOOTLOADER strap.
    fn bootloader_pin_high(&self) -> bool;
    /// nvmro_asserted -- the one decode, `ipmc_core::addr::nvmro_asserted(read_raw())`.
    fn nvmro_asserted(&self) -> bool {
        ipmc_core::addr::nvmro_asserted(self.read_raw())
    }
}

/// StatusLed -- the status LED; the patterns are `ipmc_sensors::eval::led`.
pub trait StatusLed {
    /// set_mode -- switches to `mode` and shows it at once.
    fn set_mode(&mut self, mode: led::Mode);
    /// shown -- what the LED is doing now.
    fn shown(&self) -> led::Shown;
    /// activity -- an IPMB frame went in or out (callable from interrupt context).
    fn activity(&self);
}

/// CLOCK_DOMAINS_MAX -- clock domains a `ClockSummary` lists.
pub const CLOCK_DOMAINS_MAX: usize = 4;

/// ClockDomain -- one line of the clock tree: its label and frequency.
#[derive(Clone, Copy)]
pub struct ClockDomain {
    /// The label as printed, padded for alignment (for example "HCLK (AHB):   ").
    pub label: &'static str,
    pub hz: u32,
}

/// ClockSummary -- what the banner and SYS STATUS print about the clock tree.
#[derive(Clone, Copy)]
pub struct ClockSummary {
    /// The clock source text.
    pub source: &'static str,
    /// The start-up clock fault text, if any.
    pub fault: Option<&'static str>,
    pub domains: [Option<ClockDomain>; CLOCK_DOMAINS_MAX],
}
