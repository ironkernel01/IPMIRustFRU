//! power_fsm.rs - the payload power-enable state machine. No hardware: time comes in as `now_ms`
//! and the caller drives the PWR_EN output from `enable_requested`.
//!
//! States (`ipmc_core::sensor::PowerState`) and transitions:
//!   Off              --power_on-->                 PowerOnRequested   (PWR_EN asserted)
//!   PowerOnRequested --step, PGOOD seen-->         On
//!   PowerOnRequested --step, timeout elapsed-->    Fault (PgoodTimeout, PWR_EN released)
//!   On               --step, PGOOD lost-->         Fault (PgoodLost, PWR_EN released)
//!   PowerOnRequested / On --power_off-->           Off
//!   Fault            --power_off or power_clear--> Off (latch cleared)
//!   Any state        --output_failed-->            Fault (EnableWrite: the PWR_EN write failed, so the
//!                                                    output is not known to be at the requested level)
//! A fault stays latched until POWER OFF or POWER CLEAR; `power_on` is refused while latched.
//! PGOOD seen in the same step as the timeout wins: the supplies came up in time.

use core::fmt;

use crate::hal::SourceError;
use ipmc_core::sensor::PowerState;

/// PowerFault -- the latched cause of the Fault state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerFault {
    /// PGOOD did not rise within the timeout after PWR_EN was asserted.
    PgoodTimeout,
    /// PGOOD fell while the supplies were on.
    PgoodLost,
    /// The PWR_EN write failed with the given source error.
    EnableWrite(SourceError),
}

impl PowerFault {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            PowerFault::PgoodTimeout => "PGOOD timeout",
            PowerFault::PgoodLost => "PGOOD lost",
            PowerFault::EnableWrite(_) => "PWR_EN write failed",
        }
    }

    /// reason -- the failed write's error, for `EnableWrite`.
    pub fn reason(self) -> Option<SourceError> {
        match self {
            PowerFault::EnableWrite(reason) => Some(reason),
            _ => None,
        }
    }
}

/// write_reason -- the name, and for a failed PWR_EN write the reason after it; the one
/// formatter of `PowerFault` and `PowerError`.
fn write_reason(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    reason: Option<SourceError>,
) -> fmt::Result {
    match reason {
        Some(reason) => write!(f, "{}: {}", name, reason.name()),
        None => f.write_str(name),
    }
}

impl fmt::Display for PowerFault {
    /// fmt -- the name, and for a failed write the reason after it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_reason(f, self.name(), self.reason())
    }
}

/// PowerError -- why a command was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerError {
    /// A fault is latched; POWER OFF or POWER CLEAR first.
    Faulted,
    /// The supplies are already on or requested.
    AlreadyOn,
    /// POWER CLEAR with no fault latched.
    NotFaulted,
    /// The PWR_EN write failed; a fault with that reason is latched.
    OutputWrite(SourceError),
}

impl PowerError {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            PowerError::Faulted => "fault latched, use POWER CLEAR or POWER OFF",
            PowerError::AlreadyOn => "power already on or requested",
            PowerError::NotFaulted => "no fault latched",
            PowerError::OutputWrite(_) => "PWR_EN write failed",
        }
    }

    /// reason -- the failed write's error, for `OutputWrite`.
    pub fn reason(self) -> Option<SourceError> {
        match self {
            PowerError::OutputWrite(reason) => Some(reason),
            _ => None,
        }
    }
}

impl fmt::Display for PowerError {
    /// fmt -- the name, and for a failed write the reason after it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_reason(f, self.name(), self.reason())
    }
}

/// Transition -- one state change, for trace lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: PowerState,
    pub to: PowerState,
    /// The fault that caused it, when `to` is Fault.
    pub fault: Option<PowerFault>,
}

/// PowerController -- the state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerController {
    state: PowerState,
    fault: Option<PowerFault>,
    /// `now_ms` when PWR_EN was asserted; read only in PowerOnRequested.
    requested_ms: u32,
    timeout_ms: u32,
}

impl PowerController {
    /// new -- Off, no fault. `timeout_ms` is the longest wait for PGOOD after the request
    /// (defs.rs IPMC_PGOOD_TIMEOUT_MS).
    pub const fn new(timeout_ms: u32) -> Self {
        PowerController {
            state: PowerState::Off,
            fault: None,
            requested_ms: 0,
            timeout_ms,
        }
    }

    /// state -- the current power state.
    pub fn state(&self) -> PowerState {
        self.state
    }

    /// fault -- the latched cause, if any.
    pub fn fault(&self) -> Option<PowerFault> {
        self.fault
    }

    /// enable_requested -- the level PWR_EN must have: asserted while requested or on.
    pub fn enable_requested(&self) -> bool {
        matches!(self.state, PowerState::PowerOnRequested | PowerState::On)
    }

    /// output_released -- PWR_EN is known to be released: no enable is requested and no
    /// `EnableWrite` fault is latched (after a failed write the output level is unknown). The
    /// caller adds the confirmed output level.
    pub fn output_released(&self) -> bool {
        !self.enable_requested() && !matches!(self.fault, Some(PowerFault::EnableWrite(_)))
    }

    fn move_to(&mut self, to: PowerState, fault: Option<PowerFault>) -> Transition {
        let from = self.state;
        self.state = to;
        self.fault = fault;
        Transition { from, to, fault }
    }

    /// power_on -- Off to PowerOnRequested at `now_ms`. `Err(Faulted)` while a fault is
    /// latched, `Err(AlreadyOn)` when already requested or on.
    pub fn power_on(&mut self, now_ms: u32) -> Result<Transition, PowerError> {
        match self.state {
            PowerState::Off => {
                self.requested_ms = now_ms;
                Ok(self.move_to(PowerState::PowerOnRequested, None))
            }
            PowerState::Fault => Err(PowerError::Faulted),
            PowerState::PowerOnRequested | PowerState::On => Err(PowerError::AlreadyOn),
        }
    }

    /// power_off -- any state to Off, clearing a latched fault. None when already Off.
    pub fn power_off(&mut self) -> Option<Transition> {
        if self.state == PowerState::Off {
            return None;
        }
        Some(self.move_to(PowerState::Off, None))
    }

    /// power_clear -- Fault to Off, clearing the latch. `Err(NotFaulted)` in any other state.
    pub fn power_clear(&mut self) -> Result<Transition, PowerError> {
        if self.state == PowerState::Fault {
            Ok(self.move_to(PowerState::Off, None))
        } else {
            Err(PowerError::NotFaulted)
        }
    }

    /// output_failed -- the PWR_EN write failed with `reason`: latches Fault (EnableWrite) from
    /// any state. None when that same fault is already latched (a retry that failed again).
    pub fn output_failed(&mut self, reason: SourceError) -> Option<Transition> {
        let fault = PowerFault::EnableWrite(reason);
        if self.state == PowerState::Fault && self.fault == Some(fault) {
            return None;
        }
        Some(self.move_to(PowerState::Fault, Some(fault)))
    }

    /// step -- advances the machine with the PGOOD level seen at `now_ms`. Returns the
    /// transition when the state changed. The elapsed time uses wrapping subtraction, so a
    /// u32 millisecond counter that wraps still measures correctly.
    pub fn step(&mut self, now_ms: u32, pgood: bool) -> Option<Transition> {
        match self.state {
            PowerState::PowerOnRequested => {
                if pgood {
                    Some(self.move_to(PowerState::On, None))
                } else if now_ms.wrapping_sub(self.requested_ms) >= self.timeout_ms {
                    Some(self.move_to(PowerState::Fault, Some(PowerFault::PgoodTimeout)))
                } else {
                    None
                }
            }
            PowerState::On if !pgood => {
                Some(self.move_to(PowerState::Fault, Some(PowerFault::PgoodLost)))
            }
            PowerState::On | PowerState::Off | PowerState::Fault => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: u32 = 1000;

    fn requested_at(now: u32) -> PowerController {
        let mut p = PowerController::new(TIMEOUT);
        p.power_on(now).unwrap();
        p
    }

    #[test]
    fn starts_off_with_enable_released() {
        let p = PowerController::new(TIMEOUT);
        assert_eq!(p.state(), PowerState::Off);
        assert!(!p.enable_requested());
        assert_eq!(p.fault(), None);
    }

    #[test]
    fn off_to_requested_asserts_enable() {
        let mut p = PowerController::new(TIMEOUT);
        let t = p.power_on(50).unwrap();
        assert_eq!(
            (t.from, t.to),
            (PowerState::Off, PowerState::PowerOnRequested)
        );
        assert!(p.enable_requested());
    }

    #[test]
    fn a_step_moves_the_controller_and_sets_the_enable_and_the_fault() {
        // (requested at, PGOOD steps before the move (time, level; the state is `from` after them), the step that moves it
        // (time, level), from, to, fault of the move). The enable stays asserted unless a
        // fault moved the controller.
        type Case = (
            u32,
            &'static [(u32, bool)],
            (u32, bool),
            PowerState,
            PowerState,
            Option<PowerFault>,
        );
        let cases: [Case; 3] = [
            (
                0,
                &[(10, false)],
                (20, true),
                PowerState::PowerOnRequested,
                PowerState::On,
                None,
            ),
            (
                100,
                &[(1099, false)],
                (1100, false),
                PowerState::PowerOnRequested,
                PowerState::Fault,
                Some(PowerFault::PgoodTimeout),
            ),
            (
                0,
                &[(1, true), (2, true)],
                (3, false),
                PowerState::On,
                PowerState::Fault,
                Some(PowerFault::PgoodLost),
            ),
        ];
        for (requested_ms, before, (at_ms, pgood), from, to, fault) in cases {
            let mut p = requested_at(requested_ms);
            for &(ms, level) in before {
                p.step(ms, level);
            }
            assert_eq!(p.state(), from, "before the move to {to:?}");
            let t = p.step(at_ms, pgood).unwrap();
            assert_eq!((t.from, t.to), (from, to));
            assert_eq!(t.fault, fault);
            assert_eq!(p.fault(), fault);
            assert_eq!(p.enable_requested(), fault.is_none());
        }
    }

    #[test]
    fn pgood_in_the_timeout_step_wins() {
        let mut p = requested_at(0);
        let t = p.step(TIMEOUT, true).unwrap();
        assert_eq!(t.to, PowerState::On);
    }

    #[test]
    fn timeout_survives_counter_wrap() {
        let mut p = requested_at(u32::MAX - 99);
        assert_eq!(p.step(899, false), None); // 100 + 900 - 1 ms elapsed
        assert!(p.step(900, false).is_some()); // 1000 ms elapsed across the wrap
    }

    #[test]
    fn power_off_from_requested_and_on() {
        let mut p = requested_at(0);
        let t = p.power_off().unwrap();
        assert_eq!(
            (t.from, t.to),
            (PowerState::PowerOnRequested, PowerState::Off)
        );
        assert!(!p.enable_requested());
        p.power_on(1).unwrap();
        p.step(2, true);
        let t = p.power_off().unwrap();
        assert_eq!((t.from, t.to), (PowerState::On, PowerState::Off));
        assert_eq!(p.power_off(), None);
    }

    #[test]
    fn fault_latches_until_off_or_clear() {
        let mut p = requested_at(0);
        p.step(TIMEOUT, false);
        assert_eq!(p.power_on(2000), Err(PowerError::Faulted));
        // PGOOD returning does not clear the latch.
        assert_eq!(p.step(2001, true), None);
        assert_eq!(p.state(), PowerState::Fault);
        let t = p.power_clear().unwrap();
        assert_eq!((t.from, t.to), (PowerState::Fault, PowerState::Off));
        assert_eq!(p.fault(), None);
        assert!(p.power_on(2002).is_ok());
    }

    #[test]
    fn power_off_and_power_clear_both_clear_a_latched_fault() {
        type Clear = fn(&mut PowerController) -> Option<Transition>;
        let clears: [Clear; 2] = [|p| p.power_off(), |p| p.power_clear().ok()];
        for clear in clears {
            let mut p = requested_at(0);
            p.step(TIMEOUT, false);
            let t = clear(&mut p).unwrap();
            assert_eq!((t.from, t.to), (PowerState::Fault, PowerState::Off));
            assert_eq!(p.fault(), None);
        }
    }

    #[test]
    fn refused_commands() {
        let mut p = PowerController::new(TIMEOUT);
        assert_eq!(p.power_clear(), Err(PowerError::NotFaulted));
        p.power_on(0).unwrap();
        assert_eq!(p.power_on(1), Err(PowerError::AlreadyOn));
        assert_eq!(p.power_clear(), Err(PowerError::NotFaulted));
        p.step(1, true);
        assert_eq!(p.power_on(2), Err(PowerError::AlreadyOn));
    }

    #[test]
    fn a_failed_enable_write_latches_a_fault_with_the_reason() {
        let mut p = requested_at(0);
        let t = p.output_failed(SourceError::BusError).unwrap();
        assert_eq!(
            (t.from, t.to),
            (PowerState::PowerOnRequested, PowerState::Fault)
        );
        assert_eq!(
            p.fault(),
            Some(PowerFault::EnableWrite(SourceError::BusError))
        );
        assert!(!p.enable_requested());
        // The same failure again is not a new transition; a different reason is.
        assert_eq!(p.output_failed(SourceError::BusError), None);
        assert!(p.output_failed(SourceError::Unavailable).is_some());
        // Latched until POWER OFF or POWER CLEAR.
        assert_eq!(p.power_on(1), Err(PowerError::Faulted));
        assert!(p.power_clear().is_ok());
        assert_eq!(p.state(), PowerState::Off);
    }

    #[test]
    fn faults_and_errors_print_their_reason() {
        let fault = PowerFault::EnableWrite(SourceError::BusError);
        assert_eq!(
            fault.to_string(),
            "PWR_EN write failed: bus transfer failed"
        );
        assert_eq!(PowerFault::PgoodLost.to_string(), "PGOOD lost");
        let error = PowerError::OutputWrite(SourceError::BusError);
        assert_eq!(
            error.to_string(),
            "PWR_EN write failed: bus transfer failed"
        );
        assert_eq!(PowerError::Faulted.to_string(), PowerError::Faulted.name());
    }

    #[test]
    fn off_and_fault_ignore_pgood() {
        let mut p = PowerController::new(TIMEOUT);
        assert_eq!(p.step(0, true), None);
        assert_eq!(p.step(1, false), None);
        assert_eq!(p.state(), PowerState::Off);
    }
}
