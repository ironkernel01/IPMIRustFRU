//! reset_pulse.rs - the timing of one payload reset pulse (VITA 46.11 Rule 4.1.2-9: a Cold Reset
//! of the payload is a hardware reset like a power-on reset). Hardware-free: the owner of the
//! `ResetOutput` calls `pulse_payload_reset` when a handler asked for the pulse
//! (`Context::pending_payload_reset`) and `service` on every main-loop pass; nothing here waits,
//! so no handler and no loop pass is held for the pulse time.

use crate::hal::{ResetOutput, SourceError};
use ipmc_core::clock::at_or_after;

/// PayloadResetPulse -- the state of the pulse: the time it ends while the output is asserted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PayloadResetPulse {
    release_at_ms: Option<u32>,
}

impl PayloadResetPulse {
    /// new -- no pulse running; the output is released.
    pub const fn new() -> Self {
        PayloadResetPulse {
            release_at_ms: None,
        }
    }

    /// active -- the output is asserted and waits for `service` to release it.
    pub const fn active(&self) -> bool {
        self.release_at_ms.is_some()
    }

    /// pulse_payload_reset -- asserts the output at `now_ms` for `ms` milliseconds
    /// (IPMC_PAYLOAD_RESET_PULSE_MS); `service` releases it. A pulse requested while one runs
    /// restarts the time from `now_ms`. When the output cannot be driven nothing is recorded.
    pub fn pulse_payload_reset<O: ResetOutput + ?Sized>(
        &mut self,
        out: &mut O,
        now_ms: u32,
        ms: u32,
    ) -> Result<(), SourceError> {
        out.set_reset(true)?;
        self.release_at_ms = Some(now_ms.wrapping_add(ms));
        Ok(())
    }

    /// service -- releases the output when the pulse time is up (wrapping time compare). Returns
    /// Some(result of the release) when this call tried to release it, None otherwise. A release
    /// that fails is tried again on the next call.
    pub fn service<O: ResetOutput + ?Sized>(
        &mut self,
        out: &mut O,
        now_ms: u32,
    ) -> Option<Result<(), SourceError>> {
        let due = self.release_at_ms?;
        if !at_or_after(now_ms, due) {
            return None;
        }
        let result = out.set_reset(false);
        if result.is_ok() {
            self.release_at_ms = None;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reset output that records its level and can refuse writes.
    #[derive(Default)]
    struct Pin {
        asserted: bool,
        fail: bool,
    }

    impl ResetOutput for Pin {
        fn set_reset(&mut self, asserted: bool) -> Result<(), SourceError> {
            if self.fail {
                return Err(SourceError::Unavailable);
            }
            self.asserted = asserted;
            Ok(())
        }
    }

    #[test]
    fn the_output_is_asserted_for_the_pulse_time_and_then_released() {
        let (mut pulse, mut pin) = (PayloadResetPulse::new(), Pin::default());
        assert_eq!(pulse.service(&mut pin, 0), None, "nothing runs");
        assert_eq!(pulse.pulse_payload_reset(&mut pin, 1000, 100), Ok(()));
        assert!(pin.asserted && pulse.active());
        assert_eq!(pulse.service(&mut pin, 1099), None);
        assert!(pin.asserted);
        assert_eq!(pulse.service(&mut pin, 1100), Some(Ok(())));
        assert!(!pin.asserted && !pulse.active());
        assert_eq!(pulse.service(&mut pin, 1200), None, "released once");
    }

    #[test]
    fn a_second_request_restarts_the_time_and_the_clock_may_wrap() {
        let (mut pulse, mut pin) = (PayloadResetPulse::new(), Pin::default());
        pulse
            .pulse_payload_reset(&mut pin, u32::MAX - 10, 100)
            .unwrap();
        // 60 ms later (across the wrap) a second request moves the end to 100 ms after it.
        pulse.pulse_payload_reset(&mut pin, 49, 100).unwrap();
        assert_eq!(pulse.service(&mut pin, 148), None);
        assert_eq!(pulse.service(&mut pin, 149), Some(Ok(())));
        assert!(!pin.asserted);
    }

    #[test]
    fn a_refused_assert_records_nothing_and_a_refused_release_is_retried() {
        let (mut pulse, mut pin) = (PayloadResetPulse::new(), Pin::default());
        pin.fail = true;
        assert_eq!(
            pulse.pulse_payload_reset(&mut pin, 0, 10),
            Err(SourceError::Unavailable)
        );
        assert!(!pulse.active());
        pin.fail = false;
        pulse.pulse_payload_reset(&mut pin, 0, 10).unwrap();
        pin.fail = true;
        assert_eq!(
            pulse.service(&mut pin, 10),
            Some(Err(SourceError::Unavailable))
        );
        assert!(pulse.active(), "still owed");
        pin.fail = false;
        assert_eq!(pulse.service(&mut pin, 11), Some(Ok(())));
        assert!(!pin.asserted && !pulse.active());
    }
}
