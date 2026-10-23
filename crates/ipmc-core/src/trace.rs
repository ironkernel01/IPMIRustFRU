//! trace.rs - the one trace hook of the project: a sink for single formatted lines.
//!
//! The dispatcher writes one line per request through this trait. The sensors crate reuses
//! the same trait for its sample and threshold lines, so the firmware implements one sink
//! (the console) and every layer shares it. Never call it from an interrupt handler.

use core::fmt::Arguments;

/// Trace -- accepts one formatted line per call; the implementer adds the line ending.
pub trait Trace {
    /// line -- emits one trace line.
    fn line(&mut self, args: Arguments);

    /// line_when -- emits the line when `on` holds: the one "trace this if" helper.
    fn line_when(&mut self, on: bool, args: Arguments) {
        if on {
            self.line(args);
        }
    }
}

/// NoTrace -- discards every line.
pub struct NoTrace;

impl Trace for NoTrace {
    fn line(&mut self, _args: Arguments) {}
}

/// RecordingTrace -- collects the lines for tests (std only).
#[cfg(test)]
#[derive(Default)]
pub struct RecordingTrace {
    pub lines: Vec<String>,
}

#[cfg(test)]
impl Trace for RecordingTrace {
    fn line(&mut self, args: Arguments) {
        self.lines.push(std::fmt::format(args));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_trace_keeps_formatted_lines() {
        let mut trace = RecordingTrace::default();
        trace.line(format_args!("FN({:02X}h)", 6));
        NoTrace.line(format_args!("dropped"));
        assert_eq!(trace.lines, ["FN(06h)"]);
    }
}
