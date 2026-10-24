//! monitor.rs - the sampling loop body: one sample reads the inputs, steps the power state
//! machine, drives PWR_EN, builds the snapshot and prints the trace lines.
//!
//! No hardware: the firmware passes its backends and a millisecond clock. The monitor keeps the
//! latest snapshot and the power controller; the console commands call its power functions.
//!
//! One sample, in order:
//! 1. faults::evaluate reads every input once.
//! 2. The power controller steps with PGOOD; an unreadable PGOOD counts as NOT good, so a
//!    requested power-on times out to the latched Fault. A transition re-drives PWR_EN and prints
//!    an event. A PWR_EN write that fails latches a Fault with the reason; the snapshot keeps
//!    reporting the last confirmed output level, and every sample retries a write that is owed.
//! 3. board_sensors::build_snapshot reads the analog sources and derives the summaries; the
//!    payload counts as shut down when PWR_EN is known released and PGOOD reads deasserted.
//! 4. Trace lines by level (`trace::Level`): EVENTS and ALL print the change lines against the
//!    previous snapshot; ALL adds the sample line. Sampling itself runs at every level.

use crate::board_sensors::{build_snapshot, INPUTS};
use crate::eval::faults;
use crate::eval::power_fsm::{PowerController, PowerError, Transition};
use crate::hal::{AnalogSource, DigitalInputs, Input, PowerSwitch, SourceError};
use crate::trace::{self, Level};
use ipmc_core::dispatch::PowerRequest;
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::sensor::SensorSnapshot;
use ipmc_core::trace::Trace;

/// Monitor -- latest snapshot, power controller and sampling schedule.
pub struct Monitor {
    power: PowerController,
    snapshot: SensorSnapshot,
    /// False until the first sample; the first sample prints no change lines.
    sampled: bool,
    last_sample_ms: u32,
    period_ms: u32,
    trace_level: Level,
}

impl Monitor {
    /// new -- a monitor that samples every `period_ms` (IPMC_SENSOR_PERIOD_MS) and allows the
    /// supplies `pgood_timeout_ms` (IPMC_PGOOD_TIMEOUT_MS) to come up; `trace_level` is the
    /// start-up trace level (IPMC_SENSOR_TRACE).
    pub const fn new(period_ms: u32, pgood_timeout_ms: u32, trace_level: Level) -> Self {
        Monitor {
            power: PowerController::new(pgood_timeout_ms),
            snapshot: SensorSnapshot::EMPTY,
            sampled: false,
            last_sample_ms: 0,
            period_ms,
            trace_level,
        }
    }

    /// snapshot -- the latest sample (EMPTY before the first).
    pub fn snapshot(&self) -> &SensorSnapshot {
        &self.snapshot
    }

    /// power -- the power controller.
    pub fn power(&self) -> &PowerController {
        &self.power
    }

    /// trace_level -- the level the trace lines print at.
    pub fn trace_level(&self) -> Level {
        self.trace_level
    }

    /// set_trace_level -- changes which lines print; sampling is unaffected.
    pub fn set_trace_level(&mut self, level: Level) {
        self.trace_level = level;
    }

    /// is_due -- true when a sample is owed at `now_ms` (the first one at once).
    pub fn is_due(&self, now_ms: u32) -> bool {
        !self.sampled || now_ms.wrapping_sub(self.last_sample_ms) >= self.period_ms
    }

    /// service -- samples when due; returns whether it did.
    pub fn service<A, I, T>(
        &mut self,
        now_ms: u32,
        analog: &mut A,
        io: &mut I,
        trace: &mut T,
    ) -> bool
    where
        A: AnalogSource + ?Sized,
        I: DigitalInputs + PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        if !self.is_due(now_ms) {
            return false;
        }
        self.sample(now_ms, analog, io, trace);
        true
    }

    /// sample -- one full sample now (see the file-top steps).
    pub fn sample<A, I, T>(&mut self, now_ms: u32, analog: &mut A, io: &mut I, trace: &mut T)
    where
        A: AnalogSource + ?Sized,
        I: DigitalInputs + PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let eval = faults::evaluate(&INPUTS, io);
        // Unreadable (or absent) PGOOD is not "good": fail-safe for the power sequence.
        let pgood = eval.input_state(&INPUTS, Input::Pgood) == Some(true);
        if let Some(transition) = self.power.step(now_ms, pgood) {
            let _ = self.apply(&transition, io, trace);
        }
        self.reconcile(io, trace);
        let released = self.power.output_released() && !io.is_enabled();
        let snapshot = build_snapshot(analog, &eval, self.power.state(), io.is_enabled(), released);
        if self.sampled && self.trace_level.prints_events() {
            trace::change_lines(&self.snapshot, &snapshot, trace);
        }
        if self.trace_level.prints_samples() {
            trace::sample_line(&snapshot, trace);
        }
        self.snapshot = snapshot;
        self.sampled = true;
        self.last_sample_ms = now_ms;
    }

    /// power_on -- requests the supplies: PWR_EN asserted now, PGOOD awaited by later samples.
    pub fn power_on<I, T>(
        &mut self,
        now_ms: u32,
        io: &mut I,
        trace: &mut T,
    ) -> Result<(), PowerError>
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let transition = self.power.power_on(now_ms)?;
        self.apply(&transition, io, trace)
    }

    /// power_off -- releases PWR_EN from any state and clears a latched fault.
    /// `Err(OutputWrite)` when the release could not be written (a fault is latched then).
    pub fn power_off<I, T>(&mut self, io: &mut I, trace: &mut T) -> Result<(), PowerError>
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        match self.power.power_off() {
            Some(transition) => self.apply(&transition, io, trace),
            None => Ok(()),
        }
    }

    /// apply_power_request -- performs the `PowerRequest` a handler left in
    /// `Context::pending_power`, for the firmware main loop and the emulator, the one path (owner
    /// decision 9: one power-on check for every power-on path): `power_on_check` is the result
    /// of `ipmc_core::ext::check_power_on` for the moment of the request (the FRU state machine
    /// and the extension: M6, or an activation lock). On asks it first, and a refusal powers
    /// nothing and prints one line; then `power_on` or `power_off`. A failed request prints one
    /// line with the reason. Returns true when the request was carried out.
    pub fn apply_power_request<I, T>(
        &mut self,
        request: PowerRequest,
        power_on_check: Result<(), PowerOnRefused>,
        now_ms: u32,
        io: &mut I,
        trace: &mut T,
    ) -> bool
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let result = match request {
            PowerRequest::On => match power_on_check {
                Ok(()) => self.power_on(now_ms, io, trace),
                Err(refused) => {
                    trace.line(format_args!("power on request refused: {}", refused.name()));
                    return false;
                }
            },
            PowerRequest::Off => self.power_off(io, trace),
        };
        if let Err(error) = result {
            trace.line(format_args!("power request failed: {error}"));
        }
        result.is_ok()
    }

    /// power_clear -- clears a latched fault; `Err(NotFaulted)` when none is latched.
    pub fn power_clear<I, T>(&mut self, io: &mut I, trace: &mut T) -> Result<(), PowerError>
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let transition = self.power.power_clear()?;
        self.apply(&transition, io, trace)
    }

    /// apply -- drives PWR_EN to the controller's level and prints the transition. When the
    /// write fails the controller latches Fault (EnableWrite) with the reason, that transition is
    /// printed too, and the error is returned.
    fn apply<I, T>(
        &mut self,
        transition: &Transition,
        io: &mut I,
        trace: &mut T,
    ) -> Result<(), PowerError>
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let written = io.set_enable(self.power.enable_requested());
        self.print_transition(transition, trace);
        written.map_err(|reason| self.write_failed(reason, trace))
    }

    /// write_failed -- latches the write failure, prints the transition it causes and returns
    /// the error for the command.
    fn write_failed<T: Trace + ?Sized>(
        &mut self,
        reason: SourceError,
        trace: &mut T,
    ) -> PowerError {
        if let Some(transition) = self.power.output_failed(reason) {
            self.print_transition(&transition, trace);
        }
        PowerError::OutputWrite(reason)
    }

    /// reconcile -- when the confirmed output level differs from the level the controller wants
    /// (an earlier write failed), tries the write again; a failure keeps the Fault latched.
    fn reconcile<I, T>(&mut self, io: &mut I, trace: &mut T)
    where
        I: PowerSwitch + ?Sized,
        T: Trace + ?Sized,
    {
        let want = self.power.enable_requested();
        if io.is_enabled() != want {
            if let Err(reason) = io.set_enable(want) {
                self.write_failed(reason, trace);
            }
        }
    }

    /// print_transition -- the event line of one power state change (EVENTS and ALL levels).
    fn print_transition<T: Trace + ?Sized>(&self, transition: &Transition, trace: &mut T) {
        if !self.trace_level.prints_events() {
            return;
        }
        match transition.fault {
            Some(fault) => trace.line(format_args!(
                "event: power {} -> {} ({})",
                transition.from.name(),
                transition.to.name(),
                fault
            )),
            None => trace.line(format_args!(
                "event: power {} -> {}",
                transition.from.name(),
                transition.to.name()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_sensors::{sim_analog, RAILS};
    use crate::drivers::sim::{SimAnalog, SimInputs, SimPower};
    use crate::hal::SourceError;
    use crate::trace::tests::Lines;
    use ipmc_core::sensor::{PowerState, RailStatus, Summary};

    /// One struct that is both the inputs and the switch, as the firmware's router is.
    struct Io {
        inputs: SimInputs,
        power: SimPower,
        /// Inputs whose read fails.
        dead_inputs: Vec<Input>,
        /// Every enable write fails while set.
        fail_writes: bool,
    }

    impl DigitalInputs for Io {
        fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
            if self.dead_inputs.contains(&input) {
                return Err(SourceError::BusError);
            }
            self.inputs.level_high(input)
        }
    }

    impl PowerSwitch for Io {
        fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
            if self.fail_writes {
                return Err(SourceError::BusError);
            }
            self.power.set_enable(on)
        }
        fn is_enabled(&self) -> bool {
            self.power.is_enabled()
        }
    }

    fn rig() -> (Monitor, SimAnalog, Io, Lines) {
        (
            Monitor::new(1000, 1000, Level::All),
            sim_analog(),
            Io {
                inputs: SimInputs::new(),
                power: SimPower::new(),
                dead_inputs: Vec::new(),
                fail_writes: false,
            },
            Lines::default(),
        )
    }

    #[test]
    fn first_sample_is_due_at_once_then_every_period() {
        let (mut m, mut a, mut io, mut t) = rig();
        assert!(m.service(5, &mut a, &mut io, &mut t));
        assert!(!m.service(1004, &mut a, &mut io, &mut t));
        assert!(m.service(1005, &mut a, &mut io, &mut t));
        assert_eq!(t.0.len(), 2, "one sample line each, no change lines");
        assert!(t.0.iter().all(|l| l.starts_with("sample: ")));
    }

    #[test]
    fn power_on_with_pgood_reaches_on_at_the_next_sample() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.power_on(0, &mut io, &mut t).unwrap();
        assert!(io.power.is_enabled());
        assert_eq!(m.power().state(), PowerState::PowerOnRequested);
        m.sample(10, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::On);
        assert_eq!(m.snapshot().power, PowerState::On);
        assert_eq!(m.snapshot().health, Summary::Good);
        assert_eq!(t.0[0], "event: power Off -> PowerOnRequested");
        assert_eq!(t.0[1], "event: power PowerOnRequested -> On");
    }

    #[test]
    fn missing_pgood_times_out_to_a_latched_fault_with_enable_released() {
        let (mut m, mut a, mut io, mut t) = rig();
        io.inputs.set_level(Input::Pgood, false);
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(500, &mut a, &mut io, &mut t);
        assert_eq!(m.snapshot().health, Summary::Bad, "PWR_EN on without PGOOD");
        m.sample(1000, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::Fault);
        assert!(!io.power.is_enabled());
        assert!(t
            .0
            .iter()
            .any(|l| l == "event: power PowerOnRequested -> Fault (PGOOD timeout)"));
        assert_eq!(m.power_on(1001, &mut io, &mut t), Err(PowerError::Faulted));
        m.power_clear(&mut io, &mut t).unwrap();
        assert_eq!(m.power().state(), PowerState::Off);
    }

    #[test]
    fn pgood_lost_while_on_latches_a_fault() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(1, &mut a, &mut io, &mut t);
        io.inputs.set_level(Input::Pgood, false);
        m.sample(2, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::Fault);
        assert!(!io.power.is_enabled());
        assert!(t.0.iter().any(|l| l.contains("On -> Fault (PGOOD lost)")));
    }

    #[test]
    fn rail_crossing_prints_a_change_line_and_updates_the_snapshot() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.sample(0, &mut a, &mut io, &mut t);
        t.0.clear();
        a.set_rail_mv(RAILS[2].channel, 4400).unwrap();
        m.sample(1, &mut a, &mut io, &mut t);
        assert_eq!(m.snapshot().rails[2].status, RailStatus::Critical);
        assert_eq!(t.0[0], "event: rail 5V in range -> critical (4400 mV)");
        assert!(t.0.last().unwrap().starts_with("sample: "));
        a.set_rail_mv(RAILS[2].channel, 5000).unwrap();
        t.0.clear();
        m.sample(2, &mut a, &mut io, &mut t);
        assert_eq!(t.0[0], "event: rail 5V critical -> in range (5000 mV)");
    }

    #[test]
    fn events_level_prints_changes_but_no_sample_lines() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.set_trace_level(Level::Events);
        m.sample(0, &mut a, &mut io, &mut t);
        m.sample(1000, &mut a, &mut io, &mut t);
        assert!(t.0.is_empty(), "no change, no line: {:?}", t.0);
        a.set_rail_mv(RAILS[2].channel, 4400).unwrap();
        m.sample(2000, &mut a, &mut io, &mut t);
        assert_eq!(t.0[0], "event: rail 5V in range -> critical (4400 mV)");
        assert!(t.0.iter().all(|l| l.starts_with("event: ")));
        m.power_on(2001, &mut io, &mut t).unwrap();
        assert_eq!(
            t.0.last().map(String::as_str),
            Some("event: power Off -> PowerOnRequested")
        );
    }

    #[test]
    fn trace_off_prints_nothing_and_power_off_is_idempotent() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.set_trace_level(Level::Off);
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(1, &mut a, &mut io, &mut t);
        assert_eq!(m.power_off(&mut io, &mut t), Ok(()));
        assert_eq!(m.power_off(&mut io, &mut t), Ok(()));
        assert!(t.0.is_empty());
        assert!(!io.power.is_enabled());
        assert_eq!(m.power_clear(&mut io, &mut t), Err(PowerError::NotFaulted));
    }

    #[test]
    fn unreadable_pgood_is_not_good_so_power_on_times_out_to_the_latched_fault() {
        let (mut m, mut a, mut io, mut t) = rig();
        io.dead_inputs.push(Input::Pgood);
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(500, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::PowerOnRequested);
        assert_eq!(
            m.snapshot().health,
            Summary::Bad,
            "PWR_EN on, PGOOD not good"
        );
        m.sample(1000, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::Fault);
        assert!(!io.power.is_enabled());
        assert!(t
            .0
            .iter()
            .any(|l| l == "event: power PowerOnRequested -> Fault (PGOOD timeout)"));
        // The latch holds even when PGOOD becomes readable and high.
        io.dead_inputs.clear();
        m.sample(2000, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::Fault);
    }

    #[test]
    fn unreadable_pgood_while_on_is_pgood_lost() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(1, &mut a, &mut io, &mut t);
        io.dead_inputs.push(Input::Pgood);
        m.sample(2, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::Fault);
    }

    #[test]
    fn unreadable_fault_input_is_traced_as_unreadable_and_health_is_bad() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.set_trace_level(Level::Events);
        m.sample(0, &mut a, &mut io, &mut t);
        assert_eq!(m.snapshot().health, Summary::Good);
        io.dead_inputs.push(Input::FaultN);
        m.sample(1, &mut a, &mut io, &mut t);
        assert_eq!(m.snapshot().health, Summary::Bad);
        assert_eq!(m.snapshot().inputs_unreadable, 0b0010);
        assert!(t.0.iter().any(|l| l == "event: FAULT_N unreadable"));
        assert!(t.0.iter().all(|l| !l.contains("cleared")), "{:?}", t.0);
    }

    #[test]
    fn failed_power_on_write_latches_a_fault_and_reports_the_error() {
        let (mut m, mut a, mut io, mut t) = rig();
        io.fail_writes = true;
        let error = PowerError::OutputWrite(SourceError::BusError);
        assert_eq!(m.power_on(0, &mut io, &mut t), Err(error));
        assert_eq!(m.power().state(), PowerState::Fault);
        assert_eq!(
            t.0,
            [
                "event: power Off -> PowerOnRequested",
                "event: power PowerOnRequested -> Fault (PWR_EN write failed: bus transfer failed)",
            ]
        );
        m.sample(1, &mut a, &mut io, &mut t);
        assert!(!m.snapshot().power_enabled, "the confirmed level is off");
        assert_eq!(m.power_on(2, &mut io, &mut t), Err(PowerError::Faulted));
    }

    #[test]
    fn payload_shut_down_needs_a_released_output_and_a_deasserted_pgood() {
        let (mut m, mut a, mut io, mut t) = rig();
        // Off, PGOOD deasserted (the rig starts with the supplies down): shut down.
        io.inputs.set_level(Input::Pgood, false);
        m.sample(0, &mut a, &mut io, &mut t);
        assert!(m.snapshot().payload_shut_down);
        // PGOOD still asserted while PWR_EN is released: not shut down.
        io.inputs.set_level(Input::Pgood, true);
        m.sample(1, &mut a, &mut io, &mut t);
        assert!(!m.snapshot().payload_shut_down);
        // PGOOD unreadable: not shut down.
        io.inputs.set_level(Input::Pgood, false);
        io.dead_inputs.push(Input::Pgood);
        m.sample(2, &mut a, &mut io, &mut t);
        assert!(!m.snapshot().payload_shut_down);
        io.dead_inputs.clear();
        // PWR_EN requested: not shut down even with PGOOD low.
        m.power_on(3, &mut io, &mut t).unwrap();
        m.sample(4, &mut a, &mut io, &mut t);
        assert!(!m.snapshot().payload_shut_down);
        m.power_off(&mut io, &mut t).unwrap();
        m.sample(5, &mut a, &mut io, &mut t);
        assert!(m.snapshot().payload_shut_down);
    }

    #[test]
    fn an_enable_write_fault_keeps_payload_shut_down_false() {
        let (mut m, mut a, mut io, mut t) = rig();
        io.inputs.set_level(Input::Pgood, false);
        io.fail_writes = true;
        m.power_on(0, &mut io, &mut t).unwrap_err();
        io.fail_writes = false;
        m.sample(1, &mut a, &mut io, &mut t);
        // The output reads off and PGOOD is low, but the EnableWrite fault is latched.
        assert!(!io.power.is_enabled());
        assert!(!m.snapshot().payload_shut_down);
        m.power_clear(&mut io, &mut t).unwrap();
        m.sample(2, &mut a, &mut io, &mut t);
        assert!(m.snapshot().payload_shut_down);
    }

    #[test]
    fn failed_power_off_keeps_reporting_on_and_retries_until_it_succeeds() {
        let (mut m, mut a, mut io, mut t) = rig();
        m.set_trace_level(Level::Events);
        m.power_on(0, &mut io, &mut t).unwrap();
        m.sample(1, &mut a, &mut io, &mut t);
        assert_eq!(m.power().state(), PowerState::On);
        io.fail_writes = true;
        let error = PowerError::OutputWrite(SourceError::BusError);
        assert_eq!(m.power_off(&mut io, &mut t), Err(error));
        // Not Off: a fault is latched, and the output is still confirmed on.
        assert_eq!(m.power().state(), PowerState::Fault);
        assert!(io.power.is_enabled());
        m.sample(2, &mut a, &mut io, &mut t);
        assert!(m.snapshot().power_enabled);
        assert_eq!(m.snapshot().power, PowerState::Fault);
        // Retries that keep failing add no further transition lines.
        let lines = t.0.len();
        m.sample(3, &mut a, &mut io, &mut t);
        assert_eq!(t.0.len(), lines);
        // The write works again: the next sample releases the output; the fault stays latched.
        io.fail_writes = false;
        m.sample(4, &mut a, &mut io, &mut t);
        assert!(!io.power.is_enabled());
        assert!(!m.snapshot().power_enabled);
        assert_eq!(m.power().state(), PowerState::Fault);
        assert_eq!(m.power_clear(&mut io, &mut t), Ok(()));
        assert_eq!(m.power().state(), PowerState::Off);
    }

    #[test]
    fn a_power_request_runs_the_one_power_on_check() {
        use ipmc_core::fru_state::{FruMachine, InitialChecks, Inputs};
        // A machine in M6: M4 needs M1 -> M4 first, so walk it there with the inputs.
        let checks = InitialChecks {
            management_power_valid: true,
            address_parity_valid: true,
            sdr_repository_ready: true,
            self_test_passed: true,
        };
        let inputs = |valid: bool| Inputs {
            checks,
            payload_power_valid: valid,
            payload_shut_down: !valid,
            deactivation_requested: false,
        };
        let mut fru = FruMachine::new();
        for (t, valid) in [(1, false), (2, true), (3, false)] {
            fru.step(t, &inputs(valid));
        }
        assert!(fru.check_power_on().is_err(), "the machine is in M6");
        let (mut m, _a, mut io, mut t) = rig();
        assert!(!m.apply_power_request(PowerRequest::On, fru.check_power_on(), 0, &mut io, &mut t));
        assert!(!io.power.is_enabled());
        assert_eq!(
            t.0,
            ["power on request refused: FRU deactivation in progress (M6), power on after the FRU reaches M1"]
        );
        // Off is never refused; On is carried out outside M6.
        assert!(m.apply_power_request(PowerRequest::Off, fru.check_power_on(), 0, &mut io, &mut t));
        let m1 = FruMachine::new();
        t.0.clear();
        let ok = m1.check_power_on();
        assert!(m.apply_power_request(PowerRequest::On, ok, 1, &mut io, &mut t));
        assert!(io.power.is_enabled());
        assert!(m.apply_power_request(PowerRequest::Off, ok, 2, &mut io, &mut t));
        assert!(!io.power.is_enabled());
        // An activation lock refuses On the same way and powers nothing.
        t.0.clear();
        let locked = Err(PowerOnRefused::ActivationLocked);
        assert!(!m.apply_power_request(PowerRequest::On, locked, 2, &mut io, &mut t));
        assert!(!io.power.is_enabled());
        assert_eq!(
            t.0,
            ["power on request refused: FRU is activation-locked (AL), activate the FRU or clear the lock first"]
        );
        // A failed PWR_EN write is reported once, with the reason.
        io.fail_writes = true;
        t.0.clear();
        assert!(!m.apply_power_request(PowerRequest::On, ok, 3, &mut io, &mut t));
        assert!(t.0.last().unwrap().starts_with("power request failed: "));
    }
}
