//! trace.rs - the host trace sink and the hex formatter of the frame lines.

use core::fmt::Arguments;
use ipmc_core::trace::Trace;

/// StdoutTrace -- prints every trace line to standard output, one per line.
pub struct StdoutTrace;

impl Trace for StdoutTrace {
    fn line(&mut self, args: Arguments) {
        println!("{args}");
    }
}

/// Hex -- displays bytes as space-separated upper-case hex pairs (the notation of the vector
/// file and of the PIC32 trace lines): the capture log's printer, the one implementation.
pub use ipmc_core::capture::HexBytes as Hex;
