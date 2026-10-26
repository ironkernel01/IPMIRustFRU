//! switches.rs - the log state the DEBUG menu switches: the capture log, the IPMI console trace
//! and the per-frame I2C trace, with the status lines DEBUG STATUS, LOG and TRACE print.

use core::fmt::{self, Display};

use crate::record::DefaultLog;

/// TraceWhat -- the two trace switches of DEBUG TRACE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceWhat {
    /// The request, link and event lines on the console (the log keeps them either way).
    Ipmi,
    /// One console line per IPMB frame.
    I2c,
}

impl TraceWhat {
    /// parse -- the typed word, any case: IPMI or I2C; None for another word.
    pub fn parse(word: &str) -> Option<TraceWhat> {
        crate::word_in(word, &[("IPMI", TraceWhat::Ipmi), ("I2C", TraceWhat::I2c)])
    }
}

/// LogState -- the capture log and the two trace switches.
pub struct LogState {
    /// The capture log: every whole frame in and out and the trace lines.
    capture: DefaultLog,
    /// One console line per IPMB frame in and out (DEBUG TRACE I2C).
    i2c_trace: bool,
    /// The IPMI trace lines (requests, link, events) print on the console (DEBUG TRACE IPMI);
    /// they go into the capture log either way.
    ipmi_console: bool,
}

impl LogState {
    /// new -- the log recording when `log_on`, the per-frame console line when `i2c_trace`, the
    /// IPMI lines on the console.
    pub const fn new(log_on: bool, i2c_trace: bool) -> Self {
        LogState {
            capture: DefaultLog::new(log_on),
            i2c_trace,
            ipmi_console: true,
        }
    }

    /// capture -- the capture log.
    pub const fn capture(&self) -> &DefaultLog {
        &self.capture
    }

    /// capture_mut -- the capture log, to switch or clear it.
    pub fn capture_mut(&mut self) -> &mut DefaultLog {
        &mut self.capture
    }

    /// trace_parts -- the log and the IPMI console switch together, for a trace sink that
    /// borrows both.
    pub fn trace_parts(&mut self) -> (&mut DefaultLog, bool) {
        (&mut self.capture, self.ipmi_console)
    }

    /// i2c_trace -- one console line per IPMB frame is on.
    pub const fn i2c_trace(&self) -> bool {
        self.i2c_trace
    }

    /// set_i2c_trace -- switches the per-frame console line.
    pub fn set_i2c_trace(&mut self, on: bool) {
        self.i2c_trace = on;
    }

    /// ipmi_console -- the IPMI trace lines print on the console.
    pub const fn ipmi_console(&self) -> bool {
        self.ipmi_console
    }

    /// set_ipmi_console -- switches the IPMI trace lines on the console; the log keeps them.
    pub fn set_ipmi_console(&mut self, on: bool) {
        self.ipmi_console = on;
    }

    /// set_trace -- switches one of the two trace outputs.
    pub fn set_trace(&mut self, what: TraceWhat, on: bool) {
        match what {
            TraceWhat::Ipmi => self.ipmi_console = on,
            TraceWhat::I2c => self.i2c_trace = on,
        }
    }
}

/// on_off -- the switch word.
const fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// LogLine -- the capture log line of DEBUG STATUS and DEBUG LOG, with its line ending.
pub struct LogLine<'a>(pub &'a LogState);

impl Display for LogLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0.capture.stats();
        writeln!(
            f,
            "Capture log:     {}, {} bytes, used {}, {} records, {} overwritten",
            on_off(s.enabled),
            s.size,
            s.used,
            s.records,
            s.overwritten
        )
    }
}

/// SwitchLines -- the two trace switch lines (DEBUG STATUS and DEBUG TRACE without argument),
/// each with its line ending.
pub struct SwitchLines<'a>(pub &'a LogState);

impl Display for SwitchLines<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "IPMI console:    trace {}", on_off(self.0.ipmi_console))?;
        writeln!(f, "I2C trace:       {}", on_off(self.0.i2c_trace))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::LOG_SIZE;

    #[test]
    fn the_status_lines_print_as_the_console_did() {
        let mut state = LogState::new(true, false);
        assert_eq!(
            LogLine(&state).to_string(),
            format!(
                "Capture log:     on, {} bytes, used 0, 0 records, 0 overwritten\n",
                LOG_SIZE - 1
            )
        );
        assert_eq!(
            SwitchLines(&state).to_string(),
            "IPMI console:    trace on\nI2C trace:       off\n"
        );
        state.set_trace(TraceWhat::Ipmi, false);
        state.set_trace(TraceWhat::I2c, true);
        assert_eq!(
            SwitchLines(&state).to_string(),
            "IPMI console:    trace off\nI2C trace:       on\n"
        );
        state.capture_mut().set_enabled(false);
        assert!(LogLine(&state)
            .to_string()
            .starts_with("Capture log:     off, "));
    }

    #[test]
    fn the_trace_word_parses_in_any_case() {
        for (word, want) in [
            ("ipmi", Some(TraceWhat::Ipmi)),
            ("I2c", Some(TraceWhat::I2c)),
            ("spi", None),
        ] {
            assert_eq!(TraceWhat::parse(word), want);
        }
    }
}
