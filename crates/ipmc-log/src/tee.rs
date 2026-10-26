//! tee.rs - the trace tee: a `Trace` that puts every line into the capture log.

use core::fmt::Arguments;

use ipmc_core::trace::Trace;

use crate::record::CaptureLog;

/// CaptureTrace -- a `Trace` that records every line in the log and forwards it to `inner` when
/// `console` is set. The firmware builds one wherever it passes a trace sink to the core.
pub struct CaptureTrace<'a, const N: usize> {
    /// The log the lines go into.
    pub log: &'a mut CaptureLog<N>,
    /// The sink the lines are forwarded to (the console).
    pub inner: &'a mut dyn Trace,
    /// Forward to `inner`.
    pub console: bool,
    /// The time stamped on the record.
    pub now_ms: u32,
}

impl<const N: usize> Trace for CaptureTrace<'_, N> {
    fn line(&mut self, args: Arguments) {
        self.log.text(self.now_ms, args);
        if self.console {
            self.inner.line(args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DefaultLog, Kind};
    use ipmc_core::trace::RecordingTrace;

    /// Every text record as (index, time, text).
    fn held_texts(log: &DefaultLog) -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        log.for_each(|r| {
            if let Kind::Text(t) = r.kind {
                out.push((r.index, r.time_ms, t.to_string()));
            }
        });
        out
    }

    #[test]
    fn capture_trace_records_and_forwards_only_on_console() {
        let mut log = DefaultLog::new(true);
        let mut inner = RecordingTrace::default();
        for (console, forwarded) in [(false, 0usize), (true, 1)] {
            let mut tee = CaptureTrace {
                log: &mut log,
                inner: &mut inner,
                console,
                now_ms: 7,
            };
            tee.line(format_args!("hello {}", 5));
            assert_eq!(inner.lines.len(), forwarded);
        }
        assert_eq!(inner.lines, ["hello 5"]);
        assert_eq!(
            held_texts(&log),
            [(0, 7, "hello 5".into()), (1, 7, "hello 5".into())]
        );
    }
}
