//! ext.rs - the extension hook: what a build adds to the Tier-1 IPMC.
//!
//! The Tier-1 build runs `NoExtension`. A build with the Tier-2/3 sources mounted (the
//! `ipmc-ext` bridge crate) runs the extension's own `Extension`, which answers the commands
//! the Tier-1 table does not own and gets one call per link poll. The dispatcher consults the
//! extension only after the Tier-1 table has no row, so a Tier-1 command cannot be replaced.

use crate::dispatch::Context;
use crate::fru_state::{PowerOnRefused, Transition};
use crate::ipmb::Frame;
use crate::requester::Requester;
use crate::sensor::SensorSnapshot;
use crate::trace::Trace;

/// Extension -- what a build adds to the Tier-1 IPMC: the Tier-2/3 commands and their hooks.
/// The Tier-1 build uses `NoExtension`.
pub trait Extension {
    /// command -- runs the extension's handler for `req` when the extension owns its
    /// (netFn, cmd): Some((name, result)) with `result` as a `Handler` returns it. None: the
    /// extension does not answer it, and the request gets C1h. Called only after the Tier-1
    /// table has no row for (netFn, cmd), with the LUN and VSO checks already passed (for a
    /// Group Extension request `req.data` and `out` exclude the VSO byte, as for a Handler).
    fn command(
        &mut self,
        ctx: &mut Context,
        req: &Frame,
        out: &mut [u8],
    ) -> Option<(&'static str, Result<usize, u8>)>;

    /// command_name -- the name of a command the extension answers (netFn even, the request
    /// form), for the trace and capture lines. None: the extension has no such command.
    fn command_name(&self, netfn: u8, cmd: u8) -> Option<&'static str> {
        let _ = (netfn, cmd);
        None
    }

    /// poll -- one call per `Link::poll`, after the queued requests are answered, with the link
    /// view in `ctx.link` refreshed and the requester the extension sends its requests through.
    fn poll(
        &mut self,
        ctx: &mut Context,
        requester: &mut Requester,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) {
        let _ = (ctx, requester, now_ms, trace);
    }

    /// warm_reset -- the owner applied a Warm Reset (`ResetRequest::Warm`): the extension
    /// re-initialises its volatile state (its requests in flight, its event timers) and keeps what
    /// the reset keeps. Called once, after `Context::warm_reset`.
    fn warm_reset(&mut self) {}

    /// power_on_allowed -- the extension lets payload power come on now. Asked after
    /// `FruMachine::check_power_on` by every payload power-on path (`check_power_on` below): the
    /// boot power-on, the `Context::pending_power` On hand-off, the console POWER ON command.
    /// The default allows it, so the Tier-1 build keeps its behaviour; a Tier-3 extension answers
    /// false while the FRU is Activation-Locked (VITA 46.11 Rule 4.1.5.4.2-1).
    fn power_on_allowed(&self, ctx: &Context) -> bool {
        let _ = ctx;
        true
    }

    /// on_sample -- one call per sensor sample, after `Context::update_sensors` stored the
    /// snapshot and stepped the FRU state machine; `transition` is the FRU state change it made.
    fn on_sample(
        &mut self,
        ctx: &Context,
        transition: Option<Transition>,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) {
        let _ = (ctx, transition, now_ms, trace);
    }
}

/// check_power_on -- the one combined power-on check of every payload power-on path (owner
/// decision 9): the FRU state machine's check (`FruMachine::check_power_on`: refused in M6) and
/// the extension's `power_on_allowed` (refused while the FRU is activation-locked). The first
/// refusal is returned, so the caller prints its `name()`.
pub fn check_power_on(ctx: &Context, ext: &dyn Extension) -> Result<(), PowerOnRefused> {
    ctx.fru_state.check_power_on()?;
    if ext.power_on_allowed(ctx) {
        Ok(())
    } else {
        Err(PowerOnRefused::ActivationLocked)
    }
}

/// feed_sensors -- the one sensor-sample entry of the firmware and the emulator:
/// `ctx.update_sensors`, then `ext.on_sample` with its transition; returns the transition.
pub fn feed_sensors(
    ctx: &mut Context,
    ext: &mut dyn Extension,
    now_ms: u32,
    snapshot: &SensorSnapshot,
    trace: &mut dyn Trace,
) -> Option<Transition> {
    let transition = ctx.update_sensors(now_ms, snapshot, trace);
    ext.on_sample(ctx, transition, now_ms, trace);
    transition
}

/// NoExtension -- the Tier-1 build: no extra command, no hook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoExtension;

impl Extension for NoExtension {
    fn command(
        &mut self,
        _ctx: &mut Context,
        _req: &Frame,
        _out: &mut [u8],
    ) -> Option<(&'static str, Result<usize, u8>)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr;
    use crate::trace::NoTrace;

    /// Records each `on_sample` call.
    #[derive(Default)]
    struct Sampler {
        calls: Vec<(Option<Transition>, u32)>,
    }

    impl Extension for Sampler {
        fn command(
            &mut self,
            _ctx: &mut Context,
            _req: &Frame,
            _out: &mut [u8],
        ) -> Option<(&'static str, Result<usize, u8>)> {
            None
        }

        fn on_sample(
            &mut self,
            _ctx: &Context,
            transition: Option<Transition>,
            now_ms: u32,
            _trace: &mut dyn Trace,
        ) {
            self.calls.push((transition, now_ms));
        }
    }

    #[test]
    fn feed_sensors_calls_on_sample_once_with_the_update_transition() {
        let snapshot = SensorSnapshot::EMPTY;
        // The same samples through a bare Context give the transitions to expect.
        let mut reference = Context::new(addr::resolve(0x7F));
        let mut ctx = Context::new(addr::resolve(0x7F));
        let mut ext = Sampler::default();
        for now_ms in [0, 1000, 2000] {
            let expected = reference.update_sensors(now_ms, &snapshot, &mut NoTrace);
            let got = feed_sensors(&mut ctx, &mut ext, now_ms, &snapshot, &mut NoTrace);
            assert_eq!(got, expected);
            assert_eq!(ext.calls.last(), Some(&(expected, now_ms)));
        }
        assert_eq!(ext.calls.len(), 3, "one call per sample");
        assert!(
            ext.calls.iter().any(|(t, _)| t.is_some()),
            "the first sample moves the FRU state"
        );
    }

    /// Refuses payload power, as a Tier-3 extension does while the FRU is activation-locked.
    struct Locked;

    impl Extension for Locked {
        fn command(
            &mut self,
            _ctx: &mut Context,
            _req: &Frame,
            _out: &mut [u8],
        ) -> Option<(&'static str, Result<usize, u8>)> {
            None
        }

        fn power_on_allowed(&self, _ctx: &Context) -> bool {
            false
        }
    }

    #[test]
    fn the_power_on_check_asks_the_state_machine_and_then_the_extension() {
        let mut ctx = Context::new(addr::resolve(0x7F));
        // (extension, expected result): the default extension allows power, the refusing one
        // does not.
        assert_eq!(check_power_on(&ctx, &NoExtension), Ok(()));
        assert_eq!(
            check_power_on(&ctx, &Locked),
            Err(PowerOnRefused::ActivationLocked)
        );
        // In M6 the state machine refuses first, whatever the extension says.
        ctx.fru_state = crate::fru_state::FruMachine::new();
        let mut shut = crate::fru_state::Inputs {
            checks: crate::fru_state::InitialChecks {
                management_power_valid: true,
                address_parity_valid: true,
                sdr_repository_ready: true,
                self_test_passed: true,
            },
            payload_power_valid: true,
            payload_shut_down: false,
            deactivation_requested: false,
        };
        for _ in 0..3 {
            ctx.fru_state.step(0, &shut);
        }
        shut.payload_power_valid = false;
        ctx.fru_state.step(0, &shut);
        assert_eq!(
            check_power_on(&ctx, &NoExtension),
            Err(PowerOnRefused::DeactivationInProgress)
        );
        assert_eq!(
            check_power_on(&ctx, &Locked),
            Err(PowerOnRefused::DeactivationInProgress)
        );
    }

    #[test]
    fn warm_reset_has_an_empty_default() {
        // NoExtension and the Sampler above take the default: calling it changes nothing.
        let mut none = NoExtension;
        none.warm_reset();
        let mut sampler = Sampler::default();
        sampler.warm_reset();
        assert!(sampler.calls.is_empty());
    }
}
