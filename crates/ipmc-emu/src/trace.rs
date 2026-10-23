//! trace.rs - the host trace sink and the hex formatter of the frame lines.

use core::fmt::{self, Arguments};
use ipmc_core::trace::Trace;

/// StdoutTrace -- prints every trace line to standard output, one per line.
pub struct StdoutTrace;

impl Trace for StdoutTrace {
    fn line(&mut self, args: Arguments) {
        println!("{args}");
    }
}

/// Hex -- displays bytes as space-separated upper-case hex pairs (the notation of the vector
/// file and of the PIC32 trace lines).
pub struct Hex<'a>(pub &'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}
