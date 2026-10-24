//! reset_gate.rs - when a pending processor reset may run.
//!
//! A handler asks for a Cold Reset through `Context::pending_reset`; the owner of the processor
//! (the firmware main loop, the emulator) only performs it. This gate is the one decision: the
//! reset runs when nothing would be lost, that is the egress queue is empty (the response left
//! or was dropped) and the FRU store holds no unsaved change (an accepted Write FRU Data is in
//! flash), or when IPMC_RESET_WAIT_MS has passed since the request was first seen (the reset
//! then runs anyway, and one trace line says so).

use crate::trace::Trace;

/// RESET_WAIT_MS -- longest wait for the response and the FRU save (ipmi_defs.h
/// IPMC_RESET_WAIT_MS).
const RESET_WAIT_MS: u32 = ipmc_defs::IPMC_RESET_WAIT_MS;

/// ResetGate -- owns the time a pending reset was first seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResetGate {
    seen_ms: Option<u32>,
}

impl ResetGate {
    /// new -- a gate with no reset pending.
    pub const fn new() -> ResetGate {
        ResetGate { seen_ms: None }
    }

    /// pending -- a reset has been seen and has not run yet.
    pub const fn pending(&self) -> bool {
        self.seen_ms.is_some()
    }

    /// poll -- true when the reset runs now. `pending` is whether a reset is requested
    /// (`Context::pending_reset`); `egress_empty` and `fru_clean` are the two conditions. With
    /// no reset pending the first-seen time is forgotten.
    pub fn poll(
        &mut self,
        pending: bool,
        now_ms: u32,
        egress_empty: bool,
        fru_clean: bool,
        trace: &mut dyn Trace,
    ) -> bool {
        if !pending {
            self.seen_ms = None;
            return false;
        }
        let seen = *self.seen_ms.get_or_insert(now_ms);
        if egress_empty && fru_clean {
            return true;
        }
        if now_ms.wrapping_sub(seen) >= RESET_WAIT_MS {
            trace.line(format_args!(
                "Cold Reset: after {} ms (response queued: {}, FRU unsaved: {})",
                RESET_WAIT_MS, !egress_empty, !fru_clean
            ));
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{NoTrace, RecordingTrace};

    #[test]
    fn the_reset_runs_when_both_conditions_hold() {
        let mut gate = ResetGate::new();
        assert!(!gate.poll(false, 0, true, true, &mut NoTrace));
        assert!(!gate.pending());
        assert!(
            !gate.poll(true, 10, false, true, &mut NoTrace),
            "response queued"
        );
        assert!(gate.pending());
        assert!(
            !gate.poll(true, 20, true, false, &mut NoTrace),
            "FRU unsaved"
        );
        assert!(gate.poll(true, 30, true, true, &mut NoTrace));
    }

    #[test]
    fn the_wait_counts_from_first_seen_and_is_traced() {
        let mut gate = ResetGate::new();
        let mut trace = RecordingTrace::default();
        assert!(!gate.poll(true, 100, false, false, &mut trace));
        assert!(!gate.poll(true, 100 + RESET_WAIT_MS - 1, false, false, &mut trace));
        assert!(trace.lines.is_empty());
        assert!(gate.poll(true, 100 + RESET_WAIT_MS, false, false, &mut trace));
        assert_eq!(trace.lines.len(), 1);
        // No request pending forgets the time: the next request starts a new wait.
        assert!(!gate.poll(false, 100 + RESET_WAIT_MS, true, true, &mut trace));
        assert!(!gate.poll(true, 200 + RESET_WAIT_MS, false, true, &mut trace));
    }
}
