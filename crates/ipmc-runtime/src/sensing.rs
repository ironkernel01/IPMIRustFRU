// sensing.rs - the sensor service of the main loop: the Monitor that samples and runs the power
// state machine, the routers over the board's backends, and the owner of the payload power and
// reset output.
//
// Generic over the board's analog source and its digital I/O (`AnalogSource`, `DigitalInputs +
// PowerSwitch + ResetOutput`). The decisions live in ipmc-sensors and ipmc-core; this file moves
// values between them and the console. Methods that print or trace take the console and the time
// of the pass; the service holds no console and reads no clock. Nothing here runs in an
// interrupt handler.

use ipmc_core::dispatch::HandOffs;
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::sensor::SensorSnapshot;
use ipmc_hal::board::Board;
use ipmc_sensors::board_sensors::{sim_analog, RAIL_COUNT};
use ipmc_sensors::drivers::sim::{SimInputs, SimPower};
use ipmc_sensors::hal::{
    AnalogSource, DigitalInputs, PowerSwitch, RailWiring, ResetOutput, SourceError,
};
use ipmc_sensors::monitor::Monitor;
use ipmc_sensors::reset_pulse::PayloadResetPulse;
use ipmc_sensors::routing::{RoutedAnalog, RoutedIo};
use ipmc_sensors::trace::Level;

use crate::text::{ConsoleText, ConsoleTrace};

/// BoardSensing -- the sensor service over the backends of board `B`.
pub type BoardSensing<B> = Sensing<<B as Board>::Analog, <B as Board>::Io>;

/// Sensing -- everything the sensor commands and the main loop share. The fields are public for
/// the SENSORS, POWER and SIM console handlers, which read and set them.
pub struct Sensing<A: AnalogSource, Io: DigitalInputs + PowerSwitch + ResetOutput> {
    pub monitor: Monitor,
    pub analog: RoutedAnalog<A>,
    pub io: RoutedIo<Io>,
    /// Set when the ADC refused to start (the text of the error); shown by SENSORS SHOW.
    pub adc_fault: Option<&'static str>,
    /// A sample was taken that `take_fresh` has not handed on yet.
    fresh: bool,
    /// The timing of the payload reset pulse on PAYLOAD_RST_OUT_N (IPMC_PAYLOAD_RESET_PULSE_MS).
    reset_pulse: PayloadResetPulse,
}

impl<A: AnalogSource, Io: DigitalInputs + PowerSwitch + ResetOutput> Sensing<A, Io> {
    /// payload_power_on -- PWR_EN is asserted: the payload has power (`DEBUG BOOTLOADER` says so
    /// before it restarts the processor, which releases PWR_EN).
    pub fn payload_power_on(&self) -> bool {
        self.io.is_enabled()
    }

    /// new -- builds the routers over the hardware backends and the simulators at their
    /// ipmc-defs start values. `adc` is None when no group uses the ADC, `io` is None when no
    /// group uses the board pins; an ADC that refused to start is kept as a fault text to show.
    /// `wiring` is the board's rail wiring (channel and divider of each rail of `RAILS`).
    pub fn new(
        adc: Option<Result<A, &'static str>>,
        io: Option<Io>,
        wiring: &'static [RailWiring; RAIL_COUNT],
    ) -> Self {
        let (adc, adc_fault) = match adc {
            Some(Ok(adc)) => (Some(adc), None),
            Some(Err(fault)) => (None, Some(fault)),
            None => (None, None),
        };
        Sensing {
            monitor: Monitor::new(
                ipmc_defs::IPMC_SENSOR_PERIOD_MS,
                ipmc_defs::IPMC_PGOOD_TIMEOUT_MS,
                Level::from_define(ipmc_defs::IPMC_SENSOR_TRACE),
                wiring,
            ),
            analog: RoutedAnalog::new(
                adc,
                sim_analog(wiring),
                ipmc_defs::IPMC_ADC_INTERNAL_SOURCE,
                ipmc_defs::IPMC_ADC_EXTERNAL_SOURCE,
            ),
            io: RoutedIo::new(
                io,
                SimInputs::new(),
                SimPower::new(),
                ipmc_defs::IPMC_FAULT_SOURCE,
                ipmc_defs::IPMC_POWER_SOURCE,
            ),
            adc_fault,
            fresh: false,
            reset_pulse: PayloadResetPulse::new(),
        }
    }

    /// serve_hand_offs -- what the owner of the payload power and of the reset output does with
    /// the requests of the command handlers (`IpmbLink::take_hand_offs`), and the release of a
    /// running reset pulse. Called every main-loop pass with the time of the pass.
    /// - A power request goes through `Monitor::apply_power_request` (On asks the FRU state
    ///   machine first, the one power-on check); a resample follows so the snapshot and the FRU
    ///   state see it.
    /// - A reset request asserts PAYLOAD_RST_OUT_N for IPMC_PAYLOAD_RESET_PULSE_MS
    ///   (`PayloadResetPulse`, non-blocking); a build whose power group is simulated has no
    ///   output and prints that.
    /// - A pulse whose time is up is released here.
    pub fn serve_hand_offs<C: ConsoleText>(
        &mut self,
        hand_offs: HandOffs,
        con: &mut C,
        now_ms: u32,
    ) {
        if let Some(request) = hand_offs.power {
            let done = self.monitor.apply_power_request(
                request,
                hand_offs.power_on_check,
                now_ms,
                &mut self.io,
                &mut ConsoleTrace(&mut *con),
            );
            if done {
                self.sample_now(con, now_ms);
            }
        }
        if hand_offs.payload_reset {
            let pulse_ms = ipmc_defs::IPMC_PAYLOAD_RESET_PULSE_MS;
            let started = match self.io.hw.as_mut() {
                Some(hw) => self.reset_pulse.pulse_payload_reset(hw, now_ms, pulse_ms),
                None => Err(SourceError::Unavailable),
            };
            match started {
                Ok(()) => con.print(format_args!(
                    "Payload reset: PAYLOAD_RST_OUT_N low for {} ms\n",
                    pulse_ms
                )),
                Err(error) => con.print(format_args!(
                    "Payload reset: no output pulse: {}\n",
                    error.name()
                )),
            }
        }
        if let Some(hw) = self.io.hw.as_mut() {
            if let Some(Err(error)) = self.reset_pulse.service(hw, now_ms) {
                con.print(format_args!(
                    "Payload reset: release failed: {}\n",
                    error.name()
                ));
            }
        }
    }

    /// start -- requests payload power when `power_at_boot` (`StartInfo::power_at_boot`) is true
    /// and `power_on_check` (`IpmbLink::power_on_check`) allows it; a refusal prints one line.
    pub fn start<C: ConsoleText>(
        &mut self,
        power_at_boot: bool,
        power_on_check: Result<(), PowerOnRefused>,
        con: &mut C,
        now_ms: u32,
    ) {
        if !power_at_boot {
            return;
        }
        match power_on_check {
            // A fresh controller is Off with no fault, so the request cannot be refused.
            Ok(()) => {
                let _ = self
                    .monitor
                    .power_on(now_ms, &mut self.io, &mut ConsoleTrace(con));
            }
            Err(refused) => con.print(format_args!("Power at boot refused: {}\n", refused.name())),
        }
    }

    /// sample_with -- runs one monitor sampling operation and keeps the result for `take_fresh`:
    /// `force` samples at once, else only when the period has elapsed.
    fn sample_with<C: ConsoleText>(&mut self, con: &mut C, now_ms: u32, force: bool) {
        let mut trace = ConsoleTrace(con);
        let (monitor, analog, io) = (&mut self.monitor, &mut self.analog, &mut self.io);
        self.fresh |= if force {
            monitor.sample(now_ms, analog, io, &mut trace);
            true
        } else {
            monitor.service(now_ms, analog, io, &mut trace)
        };
    }

    /// service -- samples when the period has elapsed. Called from the main loop.
    pub fn service<C: ConsoleText>(&mut self, con: &mut C, now_ms: u32) {
        self.sample_with(con, now_ms, false);
    }

    /// sample_now -- samples at once, so a console change shows in the next SENSORS SHOW.
    pub fn sample_now<C: ConsoleText>(&mut self, con: &mut C, now_ms: u32) {
        self.sample_with(con, now_ms, true);
    }

    /// take_fresh -- the latest snapshot when a sample (periodic or taken by a console command)
    /// has not been handed on yet, else None. The main loop passes it to the IPMI handlers and
    /// the FRU state machine, so each sample is fed exactly once.
    pub fn take_fresh(&mut self) -> Option<SensorSnapshot> {
        let fresh = core::mem::take(&mut self.fresh);
        fresh.then(|| *self.monitor.snapshot())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::sim::{SimPins, SimSerial};
    use ipmc_sensors::drivers::sim::{SimAnalog, SIM_RAIL_WIRING};

    type Rig = Sensing<SimAnalog, SimPins>;

    fn rig() -> Rig {
        Sensing::new(None, Some(SimPins::new()), &SIM_RAIL_WIRING)
    }

    fn reset_request() -> HandOffs {
        HandOffs {
            power: None,
            payload_reset: true,
            power_on_check: Ok(()),
        }
    }

    fn nothing() -> HandOffs {
        HandOffs {
            power: None,
            payload_reset: false,
            power_on_check: Ok(()),
        }
    }

    #[test]
    fn no_request_prints_nothing() {
        let mut sensing = rig();
        let mut con = SimSerial::roomy();
        sensing.serve_hand_offs(nothing(), &mut con, 10);
        assert_eq!(con.text(), "");
    }

    #[test]
    fn a_reset_request_pulses_the_output_and_releases_it_when_due() {
        let mut sensing = rig();
        let mut con = SimSerial::roomy();
        let pulse_ms = ipmc_defs::IPMC_PAYLOAD_RESET_PULSE_MS;
        sensing.serve_hand_offs(reset_request(), &mut con, 100);
        assert_eq!(
            con.text(),
            std::format!("Payload reset: PAYLOAD_RST_OUT_N low for {pulse_ms} ms\r\n")
        );
        let pins = sensing.io.hw.as_ref().map(|pins| pins.resets.clone());
        assert_eq!(pins, Some(std::vec![true]));
        sensing.serve_hand_offs(nothing(), &mut con, 100 + pulse_ms - 1);
        assert_eq!(
            sensing.io.hw.as_ref().map(|pins| pins.resets.len()),
            Some(1)
        );
        sensing.serve_hand_offs(nothing(), &mut con, 100 + pulse_ms);
        let pins = sensing.io.hw.as_ref().map(|pins| pins.resets.clone());
        assert_eq!(pins, Some(std::vec![true, false]), "released at the end");
    }

    #[test]
    fn a_reset_request_without_an_output_says_so() {
        let mut sensing = Rig::new(None, None, &SIM_RAIL_WIRING);
        let mut con = SimSerial::roomy();
        sensing.serve_hand_offs(reset_request(), &mut con, 5);
        assert!(con.text().starts_with("Payload reset: no output pulse: "));
        assert!(con.text().ends_with("\r\n"));
    }

    #[test]
    fn a_sample_is_handed_on_once() {
        let mut sensing = rig();
        let mut con = SimSerial::roomy();
        assert!(sensing.take_fresh().is_none());
        sensing.service(&mut con, 1);
        assert!(sensing.take_fresh().is_some());
        assert!(sensing.take_fresh().is_none());
        sensing.sample_now(&mut con, 2);
        assert!(sensing.take_fresh().is_some());
    }

    #[test]
    fn an_adc_that_failed_is_kept_as_text() {
        let sensing = Rig::new(Some(Err("no clock")), None, &SIM_RAIL_WIRING);
        assert_eq!(sensing.adc_fault, Some("no clock"));
        assert!(!sensing.payload_power_on());
    }

    #[test]
    fn start_prints_only_a_refusal_and_only_when_power_at_boot_is_on() {
        for power_at_boot in [true, false] {
            let mut sensing = rig();
            let mut con = SimSerial::roomy();
            sensing.start(
                power_at_boot,
                Err(PowerOnRefused::ActivationLocked),
                &mut con,
                0,
            );
            if power_at_boot {
                assert!(con.text().starts_with("Power at boot refused: "));
            } else {
                assert_eq!(con.text(), "");
            }
        }
    }
}
