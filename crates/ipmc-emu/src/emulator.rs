//! emulator.rs - the IPMC in software: the Link, the dispatch Context and the simulated sensors.
//!
//! Built exactly as the firmware builds them (ipmc-nucleo-f401re ipmb_link.rs and sensing.rs):
//! `addr::resolve`, `Link::new`, `Context::new`, a `Monitor` over the `drivers::sim` backends.
//! The methods mirror the calls the I2C drivers and the main loop make (PLAN 3.3a): `rx` is the
//! slave driver's `Link::on_frame` at a STOP, `poll` is one main-loop step, `next_tx` and
//! `tx_done` are the master driver's calls. Nothing here knows where frames come from, so the
//! replay and a later software transport share it.

use core::fmt;
use ipmc_core::addr;
use ipmc_core::dispatch::{Context, ResetRequest};
use ipmc_core::ext::{self, Extension};
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::link::{BusMode, FrameDrop, IpmbBPowerUp, Link, LinkConfig, LinkError, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::reset_gate::ResetGate;
use ipmc_core::trace::Trace;
use ipmc_fru::persist::Persist;
use ipmc_fru::{FruStore, RamFlash};
use ipmc_sensors::analog::BOARD_ANALOG;
use ipmc_sensors::board_sensors::sim_analog;
use ipmc_sensors::drivers::sim::{SimInputs, SimPower};
use ipmc_sensors::hal::{AnalogSource, DigitalInputs, Divider, Input, PowerSwitch, SourceError};
use ipmc_sensors::monitor::Monitor;
use ipmc_sensors::routing::{RoutedAnalog, RoutedIo, Source};
use ipmc_sensors::trace::Level;

/// STRAP_PINS_HIGH -- the raw strap word with every pin high: what the board reads with no strap
/// wired (the firmware's `ga::read_raw` before init). With IPMB_GA_SOURCE forced, `addr::resolve`
/// ignores it.
pub const STRAP_PINS_HIGH: u8 = 0x7F;

/// EmuConfig -- the build choices of one emulated board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmuConfig {
    /// Raw strap pin word handed to `addr::resolve`.
    pub strap: u8,
    /// IPMB-B is built in.
    pub b_fitted: bool,
    /// IPMB-B starts in Local Control and enabled (what the Chassis Manager's Set IPMB State
    /// does); false is the power-up state of Rule 4.1.1-4. Needs `b_fitted`.
    pub b_enabled_at_start: bool,
    /// Response bus policy.
    pub tx_policy: TxPolicy,
    /// IPMB-B behaviour before the first Set IPMB State that sets it. `b_enabled_at_start` ends
    /// the window at once (it goes through `Link::set_ipmb_state`).
    pub b_power_up: IpmbBPowerUp,
    /// Payload power is requested at start-up (IPMC_POWER_AT_BOOT).
    pub power_on_at_start: bool,
}

impl EmuConfig {
    /// from_defs -- the defs.rs values, with the board's start-up behaviour: IPMB-B disabled
    /// until Set IPMB State, payload power requested at boot as the firmware does.
    pub const fn from_defs() -> EmuConfig {
        let link = LinkConfig::from_defs();
        EmuConfig {
            strap: STRAP_PINS_HIGH,
            b_fitted: link.b_fitted,
            b_enabled_at_start: false,
            tx_policy: link.tx_policy,
            b_power_up: link.b_power_up,
            power_on_at_start: ipmc_defs::IPMC_POWER_AT_BOOT,
        }
    }
}

impl Default for EmuConfig {
    fn default() -> Self {
        Self::from_defs()
    }
}

/// EmuError -- why an emulator could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmuError {
    /// `b_enabled_at_start` asks for a bus that is not fitted.
    BusNotFitted,
    /// The power controller refused the power-on request of start-up.
    PowerOnRefused,
}

impl EmuError {
    /// name -- text for messages.
    pub const fn name(self) -> &'static str {
        match self {
            EmuError::BusNotFitted => "IPMB-B enabled at start but not fitted",
            EmuError::PowerOnRefused => "power-on request refused at start",
        }
    }
}

impl fmt::Display for EmuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for EmuError {}

/// NoHardware -- the "no on-chip driver" type of the routers: the host has no ADC or GPIO, every
/// group is simulated, so a hardware read is never routed here and answers Unavailable.
struct NoHardware;

impl AnalogSource for NoHardware {
    fn vdda_mv(&mut self) -> Result<u32, SourceError> {
        Err(SourceError::Unavailable)
    }
    fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
        Err(SourceError::Unavailable)
    }
    fn rail_mv(&mut self, _channel: u8, _divider: Divider) -> Result<u32, SourceError> {
        Err(SourceError::Unavailable)
    }
}

impl DigitalInputs for NoHardware {
    fn level_high(&mut self, _input: Input) -> Result<bool, SourceError> {
        Err(SourceError::Unavailable)
    }
}

impl PowerSwitch for NoHardware {
    fn set_enable(&mut self, _on: bool) -> Result<(), SourceError> {
        Err(SourceError::Unavailable)
    }
    fn is_enabled(&self) -> bool {
        false
    }
}

/// FRU_PERSIST -- Write FRU Data is saved to the flash log (defs.rs IPMC_FRU_PERSIST).
const FRU_PERSIST: bool = ipmc_defs::IPMC_FRU_PERSIST;

/// Board -- the sensor and power side of the board, rebuilt by a processor reset as at a boot.
struct Board {
    monitor: Monitor,
    analog: RoutedAnalog<NoHardware>,
    io: RoutedIo<NoHardware>,
}

impl Board {
    /// boot -- the Monitor over the simulators at their defs.rs start values, payload power
    /// requested at `now_ms` when `power_on_at_start` (the firmware start-up) and
    /// `power_on_check` (`ipmc_core::ext::check_power_on` for the context and extension of this
    /// boot: an activation lock keeps the payload off) allows it.
    /// Err(PowerOnRefused) when the power controller refuses the request.
    fn boot(
        config: &EmuConfig,
        now_ms: u32,
        power_on_check: Result<(), PowerOnRefused>,
    ) -> Result<Board, EmuError> {
        let mut monitor = Monitor::new(
            ipmc_defs::IPMC_SENSOR_PERIOD_MS,
            ipmc_defs::IPMC_PGOOD_TIMEOUT_MS,
            Level::from_define(ipmc_defs::IPMC_SENSOR_TRACE),
        );
        let analog = RoutedAnalog::new(None, sim_analog(), Source::Sim, Source::Sim);
        let mut io = RoutedIo::new(
            None,
            SimInputs::new(),
            SimPower::new(),
            Source::Sim,
            Source::Sim,
        );
        if config.power_on_at_start && power_on_check.is_ok() {
            monitor
                .power_on(now_ms, &mut io, &mut ipmc_core::trace::NoTrace)
                .map_err(|_| EmuError::PowerOnRefused)?;
        }
        Ok(Board {
            monitor,
            analog,
            io,
        })
    }
}

/// ExtensionFactory -- builds the extension a board starts with; the cold reset calls it again.
pub type ExtensionFactory = fn() -> Box<dyn Extension>;

/// build_active -- the extension this build runs: the Tier-2/3 extension when built in, else none.
fn build_active() -> Box<dyn Extension> {
    Box::new(ipmc_ext::Active::default())
}

/// Emulator -- the board in software.
pub struct Emulator {
    config: EmuConfig,
    link: Link,
    ctx: Context,
    board: Board,
    tx_policy: TxPolicy,
    /// The extension this board runs.
    ext: Box<dyn Extension>,
    /// Builds `ext` at start and again after a cold reset.
    make_ext: ExtensionFactory,
    /// The one decision when a pending cold reset runs (`ResetGate`, as in the firmware).
    reset_gate: ResetGate,
    /// The flash log sector: what a reset restores, in RAM (`ipmc_fru::RamFlash`). It keeps its
    /// content across a power cycle of the emulator (`Emulator::flash`, `new_with_flash`).
    flash: RamFlash,
    /// The log state and the save rule, the one the firmware runs (`Persist`, saved by
    /// `service_fru_save` with the arguments `IpmbLink::service_fru_log` passes).
    persist: Persist,
}

impl Emulator {
    /// new -- a board in its start-up state for `config`: Link and Context for the resolved
    /// address, a Monitor over the simulators at their defs.rs start values, payload power
    /// requested when `power_on_at_start`, IPMB-B enabled when `b_enabled_at_start`.
    /// Err(BusNotFitted) when IPMB-B is asked to start enabled but is not fitted;
    /// Err(PowerOnRefused) when the power controller refuses the request.
    pub fn new(config: EmuConfig) -> Result<Emulator, EmuError> {
        Emulator::new_with_flash(config, RamFlash::new())
    }

    /// new_with_flash -- `new` for a board whose flash log holds `flash`: the FRU store and the
    /// non-volatile settings are restored from it (`Persist::boot`) before the payload power is
    /// decided, as at a boot of the firmware (the power-on hook sees the stored
    /// default-activation lock).
    pub fn new_with_flash(config: EmuConfig, mut flash: RamFlash) -> Result<Emulator, EmuError> {
        let resolved = addr::resolve(config.strap);
        let mut link = Link::new(
            LinkConfig {
                b_fitted: config.b_fitted,
                tx_policy: config.tx_policy,
                b_power_up: config.b_power_up,
                requests_allowed: LinkConfig::from_defs().requests_allowed,
            },
            &resolved,
        );
        Self::enable_b_if_configured(&config, &mut link)?;
        let mut ctx = Context::with_analog(resolved, &BOARD_ANALOG);
        let (persist, store) = boot_fru(&mut flash, ctx.addr.nvmro_asserted);
        ctx.set_fru(store);
        let ext = build_active();
        let board = Board::boot(&config, 0, ext::check_power_on(&ctx, ext.as_ref()))?;
        Ok(Emulator {
            config,
            link,
            ctx,
            board,
            tx_policy: config.tx_policy,
            ext,
            make_ext: build_active,
            reset_gate: ResetGate::new(),
            flash,
            persist,
        })
    }

    /// flash -- the flash log as a power cycle leaves it. A save that is pending (an accepted
    /// Write FRU Data or settings change inside IPMC_FRU_PERSIST_DELAY_MS) is completed first:
    /// the emulator's power cycle is a clean one, taken when the board is quiet.
    pub fn flash(&mut self) -> RamFlash {
        if FRU_PERSIST {
            self.save_fru(self.flash.now, 0);
        }
        self.flash.clone()
    }

    /// save_fru -- the firmware's call of `Persist::poll` (`IpmbLink::service_fru_log`) on the
    /// RAM flash at `now_ms` with `delay_ms`: the live NVMRO level, the bus always quiet for an
    /// erase (`erase_ok`). The outcome is dropped: the RAM flash does not fail.
    fn save_fru(&mut self, now_ms: u32, delay_ms: u32) {
        self.flash.now = now_ms;
        let _ = self.persist.poll(
            &mut self.flash,
            &mut self.ctx.fru,
            now_ms,
            self.ctx.addr.nvmro_asserted,
            delay_ms,
            true,
        );
    }

    /// enable_b_if_configured -- IPMB-B enabled when `b_enabled_at_start` (what the Chassis
    /// Manager's Set IPMB State does): the bus state a new or restarted link starts in.
    fn enable_b_if_configured(config: &EmuConfig, link: &mut Link) -> Result<(), EmuError> {
        if config.b_enabled_at_start {
            link.set_ipmb_state(None, Some(BusMode::LocalControl))
                .map_err(|_| EmuError::BusNotFitted)?;
        }
        Ok(())
    }

    /// restart_link -- the Link after a Cold or Warm Reset at `now_ms`: `Link::restarted` (the
    /// same configuration and address, power-up bus state, the request sequence state of the
    /// old link carried over so no rqSeq is reused inside T9), IPMB-B enabled when configured.
    /// The one rebuild both resets use.
    fn restart_link(&self, now_ms: u32) -> Result<Link, EmuError> {
        let mut link = self.link.restarted(&self.ctx.addr, now_ms);
        Self::enable_b_if_configured(&self.config, &mut link)?;
        Ok(link)
    }

    /// with_extension -- runs the extension `make` builds instead of the build's own, now and
    /// after every cold reset (tests drive the hooks with their own).
    pub fn with_extension(mut self, make: ExtensionFactory) -> Emulator {
        self.ext = make();
        self.make_ext = make;
        self
    }

    /// own_address -- the IPMB address (8-bit form) the board answers at.
    pub fn own_address(&self) -> u8 {
        self.link.own_address()
    }

    /// tx_policy -- the response bus policy this board was built with.
    pub fn tx_policy(&self) -> TxPolicy {
        self.tx_policy
    }

    /// rx -- a frame received on `bus` at `now_ms` (the slave driver's call at a STOP).
    /// Err(reason) is the link layer's drop of the frame.
    pub fn rx(&mut self, bus: Bus, bytes: &[u8], now_ms: u32) -> Result<(), FrameDrop> {
        self.link.on_frame(bus, bytes, now_ms)
    }

    /// follow_payload_supply -- the emulator's payload supply: with `IPMC_SIM_PGOOD_HIGH` true the
    /// PGOOD input reads high while PWR_EN is asserted and low once it is released, so a
    /// deactivation ends in the complete shutdown that takes the FRU from M6 to M1
    /// (`SensorSnapshot::payload_shut_down`, Rule 4.1.5.4.5-1). With false PGOOD stays at its
    /// simulated level (low) whatever PWR_EN does, so the PGOOD timeout and the power failure
    /// stay reachable (`pgood_follows_pwr_en`).
    fn follow_payload_supply(&mut self) {
        let on = self.board.io.is_enabled();
        if let Some(level) = pgood_follows_pwr_en(ipmc_defs::IPMC_SIM_PGOOD_HIGH, on) {
            self.board.io.sim_inputs.set_level(Input::Pgood, level);
        }
    }

    /// poll -- one main-loop step at `now_ms`: the payload supply follows PWR_EN
    /// (`follow_payload_supply`), a sensor sample when one is due (fed to the command handlers
    /// and the FRU state machine, a state change traces one line), then the link layer answers
    /// the queued requests, and a pending cold reset is applied (`service_reset`). Returns the
    /// number of requests answered.
    pub fn poll(&mut self, now_ms: u32, trace: &mut dyn Trace) -> usize {
        self.follow_payload_supply();
        if self
            .board
            .monitor
            .service(now_ms, &mut self.board.analog, &mut self.board.io, trace)
        {
            self.feed_snapshot(now_ms, trace);
        }
        let taken = self
            .link
            .poll(&mut self.ctx, self.ext.as_mut(), now_ms, trace);
        self.service_hand_offs(now_ms, trace);
        self.service_fru_save(now_ms);
        self.service_reset(now_ms, trace);
        taken
    }

    /// feed_snapshot -- hands the monitor's latest snapshot to the command handlers and the FRU
    /// state machine (`ext::feed_sensors`); one call per sample.
    fn feed_snapshot(&mut self, now_ms: u32, trace: &mut dyn Trace) {
        let snapshot = *self.board.monitor.snapshot();
        ext::feed_sensors(&mut self.ctx, self.ext.as_mut(), now_ms, &snapshot, trace);
    }

    /// service_hand_offs -- what the firmware main loop does with the requests a handler left in
    /// the context: a payload power request goes through `Monitor::apply_power_request` (the one
    /// power-on check, a refusal is traced; an applied request is followed by a sample fed at once), and a payload reset request is the pulse on
    /// PAYLOAD_RST_OUT_N, modelled by one trace line `payload reset pulse <ms> ms`.
    fn service_hand_offs(&mut self, now_ms: u32, trace: &mut dyn Trace) {
        let hand_offs = self.ctx.take_hand_offs(self.ext.as_ref());
        if let Some(request) = hand_offs.power {
            let done = self.board.monitor.apply_power_request(
                request,
                hand_offs.power_on_check,
                now_ms,
                &mut self.board.io,
                trace,
            );
            if done {
                // As the firmware (`Sensing::serve_hand_offs`): sample at once, so the snapshot
                // and the FRU state machine see the new power state before the next command.
                self.board.monitor.sample(
                    now_ms,
                    &mut self.board.analog,
                    &mut self.board.io,
                    trace,
                );
                self.feed_snapshot(now_ms, trace);
            }
        }
        if hand_offs.payload_reset {
            trace.line(format_args!(
                "payload reset pulse {} ms",
                ipmc_defs::IPMC_PAYLOAD_RESET_PULSE_MS
            ));
        }
    }

    /// service_fru_save -- the firmware save step (`IpmbLink::service_fru_log`) on the RAM flash:
    /// `Persist::poll` with IPMC_FRU_PERSIST_DELAY_MS, or delay 0 while a cold reset is pending
    /// (urgent); nothing is saved while NVMRO is asserted or the log is off (IPMC_FRU_PERSIST
    /// false).
    fn service_fru_save(&mut self, now_ms: u32) {
        if !FRU_PERSIST {
            return;
        }
        let urgent = self.ctx.pending_reset.is_some();
        let delay_ms = if urgent {
            0
        } else {
            ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS
        };
        self.save_fru(now_ms, delay_ms);
    }

    /// fru_unsaved -- an accepted Write FRU Data is not in the flash yet (the log's pending save
    /// or the store's dirty mark, as `IpmbLink::fru_unsaved`); false while the log is off.
    fn fru_unsaved(&self) -> bool {
        FRU_PERSIST && (self.persist.pending() || self.ctx.fru.is_dirty())
    }

    /// service_reset -- the emulator's restart: when `ResetGate` (the one decision the firmware
    /// uses too) says so, a pending Cold or Warm Reset is applied.
    /// - Cold: the board restarts as after a processor reset. The link (`restart_link`), the
    ///   context and the extension are rebuilt as at start-up; the FRU store is restored from the
    ///   flash copy (an accepted write not saved by then is lost, as on the board); the Monitor
    ///   and the power switch are rebuilt as at a boot (the reset releases PWR_EN,
    ///   `IPMC_POWER_AT_BOOT` requests power again). Traces `IPMC cold reset`.
    /// - Warm: the link is rebuilt the same way (`restart_link`), `Context::warm_reset` clears
    ///   the hand-offs and the extension's `warm_reset` runs; everything else, payload power
    ///   included, stays. Traces `IPMC warm reset`.
    ///
    /// Both run on afterwards.
    fn service_reset(&mut self, now_ms: u32, trace: &mut dyn Trace) {
        let request = self.ctx.pending_reset;
        let (egress_empty, fru_clean) = (self.link.egress_empty(), !self.fru_unsaved());
        if !self
            .reset_gate
            .poll(request.is_some(), now_ms, egress_empty, fru_clean, trace)
        {
            return;
        }
        match request {
            Some(ResetRequest::Warm) => {
                let config = self.config;
                self.link.warm_reset(
                    &mut self.ctx,
                    self.ext.as_mut(),
                    &mut self.reset_gate,
                    now_ms,
                    // Cannot fail: `new` built the same state from the same config.
                    |_old, new| {
                        let _ = Self::enable_b_if_configured(&config, new);
                    },
                );
                trace.line(format_args!("IPMC warm reset"));
            }
            Some(ResetRequest::Cold) => {
                let Ok(link) = self.restart_link(now_ms) else {
                    return; // `new` built the same state from the same config
                };
                self.reset_gate = ResetGate::new();
                let mut ctx = Context::with_analog(addr::resolve(self.config.strap), &BOARD_ANALOG);
                let (persist, store) = boot_fru(&mut self.flash, ctx.addr.nvmro_asserted);
                ctx.set_fru(store);
                let ext = (self.make_ext)();
                let power_on_check = ext::check_power_on(&ctx, ext.as_ref());
                let Ok(board) = Board::boot(&self.config, now_ms, power_on_check) else {
                    return;
                };
                self.link = link;
                self.ctx = ctx;
                self.board = board;
                self.ext = ext;
                self.persist = persist;
                trace.line(format_args!("IPMC cold reset"));
            }
            None => {}
        }
    }

    /// next_tx -- the next frame for the master driver to transmit and the bus to use, or None.
    /// The frame stays queued until `tx_done` reports its outcome.
    pub fn next_tx(&mut self) -> Option<(Bus, &[u8])> {
        self.link.next_tx()
    }

    /// tx_done -- the outcome of the transmission `next_tx` handed out on `bus`, at `now_ms`.
    /// A failure queues a resend on the other bus (Rule 4.1.1-7); Err when nothing was handed out
    /// or the bus differs.
    pub fn tx_done(
        &mut self,
        bus: Bus,
        ok: bool,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> Result<(), LinkError> {
        self.link.on_tx_result(bus, ok, now_ms, trace)
    }

    /// payload_power -- the state of the payload power controller (a processor reset restarts
    /// it, as it does on the board).
    pub fn payload_power(&self) -> ipmc_core::sensor::PowerState {
        self.board.monitor.power().state()
    }

    /// link -- the link layer, for counters and bus state.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// context -- the dispatch context, for the FRU state and the latest sensor snapshot.
    pub fn context(&self) -> &Context {
        &self.ctx
    }
}

/// boot_fru -- the FRU store at a boot: `Persist::boot` over `flash` (the newest valid record,
/// image and settings), the built store when the log is off, holds no record or fails.
fn boot_fru(flash: &mut RamFlash, nvmro_asserted: bool) -> (Persist, FruStore) {
    let mut persist = Persist::new();
    if FRU_PERSIST {
        if let Ok((Some(store), _)) = persist.boot(flash, nvmro_asserted) {
            return (persist, store);
        }
    }
    (persist, FruStore::new())
}

/// pgood_follows_pwr_en -- the level the emulated PGOOD input takes while PWR_EN is `pwr_en`:
/// `Some(pwr_en)` when `IPMC_SIM_PGOOD_HIGH` (`level_high`) is true, None (the input keeps its
/// simulated level, low for 0) otherwise.
const fn pgood_follows_pwr_en(level_high: bool, pwr_en: bool) -> Option<bool> {
    if level_high {
        Some(pwr_en)
    } else {
        None
    }
}

#[cfg(test)]
mod pgood_tests {
    use super::pgood_follows_pwr_en;

    #[test]
    fn pgood_follows_pwr_en_only_when_the_simulated_level_is_high() {
        for (define, pwr_en, want) in [
            (true, true, Some(true)),
            (true, false, Some(false)),
            (false, true, None),
            (false, false, None),
        ] {
            assert_eq!(
                pgood_follows_pwr_en(define, pwr_en),
                want,
                "{define} {pwr_en}"
            );
        }
    }
}
