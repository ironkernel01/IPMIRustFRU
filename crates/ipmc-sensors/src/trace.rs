//! trace.rs - the console lines of the sensors: one per sample and one per state change.
//!
//! Lines go through the project's one trace hook, `ipmc_core::trace::Trace`. Change detection
//! compares two snapshots: rail status, temperature flags, input assertion and the Tier-1
//! summaries. Power transitions are traced by `monitor.rs` where their cause is known.
//! `Level` selects which lines print: none, the change lines, or the change and sample lines.

use core::fmt;

use crate::board_sensors::{INPUTS, RAILS, TENTHS};
use ipmc_core::sensor::{RailStatus, SensorSnapshot, Summary};
use ipmc_core::trace::Trace;

/// Level -- which sensor lines reach the console (ipmi_defs.h IPMC_SENSOR_TRACE and the
/// IPMC_TRACE_* values). Sampling runs at every level; only the printing changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// No lines.
    Off,
    /// One line per change: rail status, temperature flags, input assert or clear, power
    /// transition, Tier-1 summary.
    Events,
    /// The change lines plus one sample line per sample.
    All,
}

impl Level {
    /// ALL_LEVELS -- every level, in define order (IPMC_TRACE_OFF, _EVENTS, _ALL).
    const ALL_LEVELS: [Level; 3] = [Level::Off, Level::Events, Level::All];

    /// from_define -- the level of an ipmi_defs.h IPMC_TRACE_* value. ipmc-defs guards the
    /// range 0..=2, so a value that is neither OFF nor EVENTS is ALL.
    pub const fn from_define(value: u32) -> Level {
        if value == ipmc_defs::IPMC_TRACE_OFF {
            Level::Off
        } else if value == ipmc_defs::IPMC_TRACE_EVENTS {
            Level::Events
        } else {
            Level::All
        }
    }

    /// name -- the console word of the level: "off", "events" or "all".
    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Events => "events",
            Level::All => "all",
        }
    }

    /// from_name -- the level whose `name` equals `text` ignoring case; None for any other word.
    pub fn from_name(text: &str) -> Option<Level> {
        Level::ALL_LEVELS
            .into_iter()
            .find(|level| level.name().eq_ignore_ascii_case(text))
    }

    /// prints_events -- true when change lines print (EVENTS and ALL).
    pub fn prints_events(self) -> bool {
        self != Level::Off
    }

    /// prints_samples -- true when the per-sample line prints (ALL only).
    pub fn prints_samples(self) -> bool {
        self == Level::All
    }
}

/// Tenths -- a value in tenths printed with one decimal ("34.8", "-0.5").
pub struct Tenths(pub i32);

impl fmt::Display for Tenths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        let tenths = TENTHS.unsigned_abs();
        write!(f, "{}{}.{}", sign, magnitude / tenths, magnitude % tenths)
    }
}

/// Status -- a rail status printed as its name.
struct Status(RailStatus);

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.name())
    }
}

/// Asserted -- the names of the asserted inputs, space separated, or "none".
pub struct Asserted(pub u32);

impl fmt::Display for Asserted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut any = false;
        for (index, def) in INPUTS.iter().enumerate() {
            if self.0 & (1u32 << index) != 0 {
                if any {
                    f.write_str(" ")?;
                }
                f.write_str(def.name)?;
                any = true;
            }
        }
        if any {
            Ok(())
        } else {
            f.write_str("none")
        }
    }
}

/// UnreadableSuffix -- "; unreadable NAME ..." for the inputs that could not be read, nothing when
/// all were read.
struct UnreadableSuffix(u32);

impl fmt::Display for UnreadableSuffix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            Ok(())
        } else {
            write!(f, "; unreadable {}", Asserted(self.0))
        }
    }
}

/// Rails -- every valid rail as "name=NNNNmV status", comma separated.
struct Rails<'a>(&'a SensorSnapshot);

impl fmt::Display for Rails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (reading, def)) in self.0.valid_rails().iter().zip(RAILS.iter()).enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(
                f,
                "{}={}mV {}",
                def.name,
                reading.mv,
                Status(reading.status)
            )?;
        }
        Ok(())
    }
}

/// Temperature -- "NN.N C flags NNh" or "unavailable".
struct Temperature<'a>(&'a SensorSnapshot);

impl fmt::Display for Temperature<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = &self.0.temperature;
        if t.available {
            write!(f, "{} C flags {:02X}h", Tenths(t.dc), t.flags)
        } else {
            f.write_str("unavailable")
        }
    }
}

/// sample_line -- the per-sample line: rails, temperature, asserted inputs (and the unreadable
/// ones when there are any), power, summaries.
pub fn sample_line<T: Trace + ?Sized>(snapshot: &SensorSnapshot, trace: &mut T) {
    trace.line(format_args!(
        "sample: {}; temp {}; asserted {}{}; power {}; health {} voltage {}",
        Rails(snapshot),
        Temperature(snapshot),
        Asserted(snapshot.inputs_asserted),
        UnreadableSuffix(snapshot.inputs_unreadable),
        snapshot.power.name(),
        snapshot.health.name(),
        snapshot.voltage.name()
    ));
}

/// summary_changed -- emits one line when a Tier-1 summary changed.
fn summary_changed<T: Trace + ?Sized>(label: &str, old: Summary, new: Summary, trace: &mut T) {
    trace.line_when(
        old != new,
        format_args!("event: {} {} -> {}", label, old.name(), new.name()),
    );
}

/// InputState -- what one sample knows about an input. An input that could not be read is
/// `Unreadable`, never `Clear`: no measurement says nothing about the pin.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InputState {
    Asserted,
    Clear,
    Unreadable,
}

impl InputState {
    /// word -- the trace word of the state: "asserted", "cleared" or "unreadable".
    fn word(self) -> &'static str {
        match self {
            InputState::Asserted => "asserted",
            InputState::Clear => "cleared",
            InputState::Unreadable => "unreadable",
        }
    }
}

/// input_state -- the state of table input `index` in `snapshot`.
fn input_state(snapshot: &SensorSnapshot, index: usize) -> InputState {
    let bit = 1u32 << index;
    if snapshot.inputs_unreadable & bit != 0 {
        InputState::Unreadable
    } else if snapshot.inputs_asserted & bit != 0 {
        InputState::Asserted
    } else {
        InputState::Clear
    }
}

/// change_lines -- one line per difference between `old` and `new`: a rail status, the
/// temperature flags or availability, an input asserting or clearing, a summary.
pub fn change_lines<T: Trace + ?Sized>(old: &SensorSnapshot, new: &SensorSnapshot, trace: &mut T) {
    for ((o, n), def) in old
        .valid_rails()
        .iter()
        .zip(new.valid_rails())
        .zip(RAILS.iter())
    {
        if o.status != n.status {
            trace.line(format_args!(
                "event: rail {} {} -> {} ({} mV)",
                def.name,
                Status(o.status),
                Status(n.status),
                n.mv
            ));
        }
    }
    let (ot, nt) = (&old.temperature, &new.temperature);
    if ot.available != nt.available || ot.flags != nt.flags {
        trace.line(format_args!(
            "event: temperature {} -> {}",
            Temperature(old),
            Temperature(new)
        ));
    }
    for (index, def) in INPUTS.iter().enumerate() {
        let (was, is) = (input_state(old, index), input_state(new, index));
        if was != is {
            trace.line(format_args!("event: {} {}", def.name, is.word()));
        }
    }
    summary_changed("FRU Health", old.health, new.health, trace);
    summary_changed("FRU Voltage", old.voltage, new.voltage, trace);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ipmc_core::sensor::{PowerState, TemperatureReading};
    use std::fmt::Arguments;

    /// Collects trace lines for assertions.
    #[derive(Default)]
    pub struct Lines(pub Vec<String>);

    impl Trace for Lines {
        fn line(&mut self, args: Arguments) {
            self.0.push(std::fmt::format(args));
        }
    }

    fn nominal() -> SensorSnapshot {
        let mut s = SensorSnapshot::EMPTY;
        for (slot, def) in s.rails.iter_mut().zip(RAILS.iter()) {
            slot.mv = def.nominal_mv;
            slot.status = RailStatus::InRange;
            s.rail_count += 1;
        }
        s.temperature = TemperatureReading {
            available: true,
            dc: 348,
            flags: 0,
        };
        s.inputs_asserted = 1;
        s.power = PowerState::On;
        s.health = Summary::Good;
        s.voltage = Summary::Good;
        s
    }

    #[test]
    fn tenths_prints_sign_and_one_decimal() {
        assert_eq!(Tenths(348).to_string(), "34.8");
        assert_eq!(Tenths(-5).to_string(), "-0.5");
        assert_eq!(Tenths(0).to_string(), "0.0");
        assert_eq!(Tenths(-100).to_string(), "-10.0");
        assert_eq!(Tenths(i32::MIN).to_string(), "-214748364.8");
    }

    #[test]
    fn sample_line_lists_rails_temperature_inputs_and_summaries() {
        let mut lines = Lines::default();
        sample_line(&nominal(), &mut lines);
        assert_eq!(
            lines.0,
            [
                "sample: VDDA=3300mV in range, 12V payload=12000mV in range, 5V=5000mV in range, \
              3V3 payload=3300mV in range, 1V0 core=1000mV in range; temp 34.8 C flags 00h; \
              asserted PGOOD; power On; health good voltage good"
            ]
        );
    }

    #[test]
    fn rail_crossing_prints_one_event_with_the_millivolts() {
        let old = nominal();
        let mut new = nominal();
        new.rails[2].mv = 4400;
        new.rails[2].status = RailStatus::Critical;
        new.voltage = Summary::Bad;
        new.health = Summary::Bad;
        let mut lines = Lines::default();
        change_lines(&old, &new, &mut lines);
        assert_eq!(
            lines.0,
            [
                "event: rail 5V in range -> critical (4400 mV)",
                "event: FRU Health good -> bad",
                "event: FRU Voltage good -> bad",
            ]
        );
    }

    #[test]
    fn input_and_temperature_changes_are_reported() {
        let old = nominal();
        let mut new = nominal();
        new.inputs_asserted = 0b0011; // FAULT_N asserted too
        new.temperature.flags = 0x18;
        new.temperature.dc = 900;
        let mut lines = Lines::default();
        change_lines(&old, &new, &mut lines);
        assert_eq!(
            lines.0,
            [
                "event: temperature 34.8 C flags 00h -> 90.0 C flags 18h",
                "event: FAULT_N asserted",
            ]
        );
        let mut lines = Lines::default();
        change_lines(&new, &old, &mut lines);
        assert!(lines.0.iter().any(|l| l == "event: FAULT_N cleared"));
    }

    #[test]
    fn an_unreadable_input_is_reported_as_unreadable_never_cleared() {
        let old = nominal();
        let mut new = nominal();
        new.inputs_unreadable = 0b0010; // FAULT_N could not be read
        let mut lines = Lines::default();
        change_lines(&old, &new, &mut lines);
        assert_eq!(lines.0, ["event: FAULT_N unreadable"]);
        // Back to a measured clear input.
        let mut lines = Lines::default();
        change_lines(&new, &old, &mut lines);
        assert_eq!(lines.0, ["event: FAULT_N cleared"]);
        // An asserted input that becomes unreadable is not reported as cleared either.
        let mut asserted = nominal();
        asserted.inputs_asserted = 0b0011;
        let mut unreadable = asserted;
        unreadable.inputs_asserted = 0b0001;
        unreadable.inputs_unreadable = 0b0010;
        let mut lines = Lines::default();
        change_lines(&asserted, &unreadable, &mut lines);
        assert_eq!(lines.0, ["event: FAULT_N unreadable"]);
        // The sample line names the unreadable inputs.
        let mut lines = Lines::default();
        sample_line(&unreadable, &mut lines);
        assert!(
            lines.0[0].contains("asserted PGOOD; unreadable FAULT_N; power"),
            "{}",
            lines.0[0]
        );
    }

    #[test]
    fn levels_follow_the_defines_and_parse_their_names() {
        assert_eq!(Level::from_define(ipmc_defs::IPMC_TRACE_OFF), Level::Off);
        assert_eq!(
            Level::from_define(ipmc_defs::IPMC_TRACE_EVENTS),
            Level::Events
        );
        assert_eq!(Level::from_define(ipmc_defs::IPMC_TRACE_ALL), Level::All);
        assert_eq!(Level::from_name("EVENTS"), Some(Level::Events));
        assert_eq!(Level::from_name("all"), Some(Level::All));
        assert_eq!(Level::from_name("Off"), Some(Level::Off));
        assert_eq!(Level::from_name("on"), None);
        assert!(!Level::Off.prints_events() && !Level::Off.prints_samples());
        assert!(Level::Events.prints_events() && !Level::Events.prints_samples());
        assert!(Level::All.prints_events() && Level::All.prints_samples());
    }

    #[test]
    fn identical_snapshots_print_nothing() {
        let s = nominal();
        let mut lines = Lines::default();
        change_lines(&s, &s, &mut lines);
        assert!(lines.0.is_empty());
    }
}
