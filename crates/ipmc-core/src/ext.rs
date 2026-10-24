//! ext.rs - the extension hook: what a build adds to the Tier-1 IPMC.
//!
//! The Tier-1 build runs `NoExtension`. A build with the Tier-2/3 sources mounted (the
//! `ipmc-ext` bridge crate) runs the extension's own `Extension`, which answers the commands
//! the Tier-1 table does not own and gets one call per link poll. The dispatcher consults the
//! extension only after the Tier-1 table has no row, so a Tier-1 command cannot be replaced.

use crate::dispatch::Context;
use crate::fru_state::Transition;
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
}
