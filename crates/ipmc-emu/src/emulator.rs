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
use ipmc_core::link::{BusMode, FrameDrop, Link, LinkConfig, LinkError, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::reset_gate::ResetGate;
use ipmc_core::trace::Trace;
use ipmc_fru::records::SIZE as FRU_SIZE;
use ipmc_fru::FruStore;
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
    /// Payload power is requested at start-up (IPMC_POWER_AT_BOOT).
    pub power_on_at_start: bool,
}

impl EmuConfig {
    /// from_defs -- the ipmi_defs.h values, with the board's start-up behaviour: IPMB-B disabled
    /// until Set IPMB State, payload power requested at boot as the firmware does.
    pub const fn from_defs() -> EmuConfig {
        let link = LinkConfig::from_defs();
        EmuConfig {
            strap: STRAP_PINS_HIGH,
            b_fitted: link.b_fitted,
            b_enabled_at_start: false,
            tx_policy: link.tx_policy,
            power_on_at_start: ipmc_defs::IPMC_POWER_AT_BOOT == 1,
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

/// FRU_PERSIST -- Write FRU Data is saved to the flash log (ipmi_defs.h IPMC_FRU_PERSIST).
const FRU_PERSIST: bool = ipmc_defs::IPMC_FRU_PERSIST == 1;

/// Board -- the sensor and power side of the board, rebuilt by a processor reset as at a boot.
struct Board {
    monitor: Monitor,
    analog: RoutedAnalog<NoHardware>,
    io: RoutedIo<NoHardware>,
}

impl Board {
    /// boot -- the Monitor over the simulators at their ipmi_defs.h start values, payload power
    /// requested at `now_ms` when `power_on_at_start` (the firmware start-up).
    /// Err(PowerOnRefused) when the power controller refuses the request.
    fn boot(config: &EmuConfig, now_ms: u32) -> Result<Board, EmuError> {
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
        if config.power_on_at_start {
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
    /// The FRU image in the flash log: what a reset restores. Updated where the firmware saves
    /// (`service_fru_save`).
    flash_fru: [u8; FRU_SIZE],
    /// Time an accepted Write FRU Data was last seen unsaved, None while the store is saved.
    fru_unsaved_since: Option<u32>,
}

impl Emulator {
    /// new -- a board in its start-up state for `config`: Link and Context for the resolved
    /// address, a Monitor over the simulators at their ipmi_defs.h start values, payload power
    /// requested when `power_on_at_start`, IPMB-B enabled when `b_enabled_at_start`.
    /// Err(BusNotFitted) when IPMB-B is asked to start enabled but is not fitted;
    /// Err(PowerOnRefused) when the power controller refuses the request.
    pub fn new(config: EmuConfig) -> Result<Emulator, EmuError> {
        let (link, ctx) = Self::start_state(&config, 0, 0, 0)?;
        let board = Board::boot(&config, 0)?;
        Ok(Emulator {
            config,
            link,
            ctx,
            board,
            tx_policy: config.tx_policy,
            ext: build_active(),
            make_ext: build_active,
            reset_gate: ResetGate::new(),
            flash_fru: *FruStore::new().image(),
            fru_unsaved_since: None,
        })
    }

    /// start_state -- the Link and Context of a board at start-up for `config` (also what a cold
    /// reset rebuilds): IPMB-B enabled when `b_enabled_at_start`; the first request sequence
    /// number is `first_seq` and the values in `recent` count as sent at `start_ms` (power-up: 0,
    /// 0, 0; after a reset: the old link's counter, its sequence numbers sent inside T9 and the
    /// reset time).
    fn start_state(
        config: &EmuConfig,
        first_seq: u8,
        recent: u64,
        start_ms: u32,
    ) -> Result<(Link, Context), EmuError> {
        let resolved = addr::resolve(config.strap);
        let mut link = Link::with_first_request_seq(
            LinkConfig {
                b_fitted: config.b_fitted,
                tx_policy: config.tx_policy,
                requests_allowed: LinkConfig::from_defs().requests_allowed,
            },
            &resolved,
            first_seq,
            recent,
            start_ms,
        );
        if config.b_enabled_at_start {
            link.set_ipmb_state(None, Some(BusMode::LocalControl))
                .map_err(|_| EmuError::BusNotFitted)?;
        }
        Ok((link, Context::new(resolved)))
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

    /// poll -- one main-loop step at `now_ms`: a sensor sample when one is due (fed to the
    /// command handlers and the FRU state machine, a state change traces one line), then the
    /// link layer answers the queued requests, and a pending cold reset is applied
    /// (`service_reset`). Returns the number of requests answered.
    pub fn poll(&mut self, now_ms: u32, trace: &mut dyn Trace) -> usize {
        if self
            .board
            .monitor
            .service(now_ms, &mut self.board.analog, &mut self.board.io, trace)
        {
            let snapshot = *self.board.monitor.snapshot();
            ext::feed_sensors(&mut self.ctx, self.ext.as_mut(), now_ms, &snapshot, trace);
        }
        let taken = self
            .link
            .poll(&mut self.ctx, self.ext.as_mut(), now_ms, trace);
        self.service_fru_save(now_ms);
        self.service_reset(now_ms, trace);
        taken
    }

    /// service_fru_save -- the firmware save step (`IpmbLink::service_fru_log`) on the flash
    /// copy: an accepted Write FRU Data is saved IPMC_FRU_PERSIST_DELAY_MS after it was seen, and
    /// at once while a cold reset is pending; nothing is saved while NVMRO is asserted or the
    /// log is off (IPMC_FRU_PERSIST 0).
    fn service_fru_save(&mut self, now_ms: u32) {
        if !FRU_PERSIST {
            return;
        }
        if self.ctx.fru.take_dirty() {
            self.fru_unsaved_since = Some(now_ms);
        }
        let Some(since) = self.fru_unsaved_since else {
            return;
        };
        let urgent = self.ctx.pending_reset.is_some();
        let due = now_ms.wrapping_sub(since) >= ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS;
        if (urgent || due) && !self.ctx.addr.nvmro_asserted {
            self.flash_fru = *self.ctx.fru.image();
            self.fru_unsaved_since = None;
        }
    }

    /// fru_unsaved -- an accepted Write FRU Data is not in the flash copy yet; false while the log
    /// is off.
    fn fru_unsaved(&self) -> bool {
        FRU_PERSIST && (self.fru_unsaved_since.is_some() || self.ctx.fru.is_dirty())
    }

    /// service_reset -- the emulator processor reset: when `ResetGate` (the one decision the
    /// firmware uses too) says so, the board restarts as after a processor reset. The link, the
    /// context and the extension are rebuilt as at start-up; the request sequence counter
    /// continues from the old link; the FRU store is restored from the flash copy (an accepted
    /// write not saved by then is lost, as on the board); the Monitor and the power switch are
    /// rebuilt as at a boot (the reset releases PWR_EN, `IPMC_POWER_AT_BOOT` requests power
    /// again). Traces `IPMC cold reset` and runs on.
    fn service_reset(&mut self, now_ms: u32, trace: &mut dyn Trace) {
        let pending = matches!(self.ctx.pending_reset, Some(ResetRequest::Cold));
        let (egress_empty, fru_clean) = (self.link.egress_empty(), !self.fru_unsaved());
        if !self
            .reset_gate
            .poll(pending, now_ms, egress_empty, fru_clean, trace)
        {
            return;
        }
        let first_seq = self.link.next_request_seq();
        let recent = self.link.recent_request_seqs(now_ms);
        let Ok((link, mut ctx)) = Self::start_state(&self.config, first_seq, recent, now_ms) else {
            return; // `new` built the same state from the same config
        };
        let Ok(board) = Board::boot(&self.config, now_ms) else {
            return;
        };
        ctx.set_fru(FruStore::from_image(self.flash_fru));
        self.link = link;
        self.ctx = ctx;
        self.board = board;
        self.ext = (self.make_ext)();
        self.reset_gate = ResetGate::new();
        self.fru_unsaved_since = None;
        trace.line(format_args!("IPMC cold reset"));
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
