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
use ipmc_core::dispatch::Context;
use ipmc_core::link::{BusMode, FrameDrop, Link, LinkConfig, LinkError, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::trace::Trace;
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

/// Emulator -- the board in software.
pub struct Emulator {
    link: Link,
    ctx: Context,
    monitor: Monitor,
    analog: RoutedAnalog<NoHardware>,
    io: RoutedIo<NoHardware>,
    tx_policy: TxPolicy,
}

impl Emulator {
    /// new -- a board in its start-up state for `config`: Link and Context for the resolved
    /// address, a Monitor over the simulators at their ipmi_defs.h start values, payload power
    /// requested when `power_on_at_start`, IPMB-B enabled when `b_enabled_at_start`.
    /// Err(BusNotFitted) when IPMB-B is asked to start enabled but is not fitted;
    /// Err(PowerOnRefused) when the power controller refuses the request.
    pub fn new(config: EmuConfig) -> Result<Emulator, EmuError> {
        let resolved = addr::resolve(config.strap);
        let mut link = Link::new(
            LinkConfig {
                b_fitted: config.b_fitted,
                tx_policy: config.tx_policy,
            },
            &resolved,
        );
        if config.b_enabled_at_start {
            link.set_ipmb_state(None, Some(BusMode::LocalControl))
                .map_err(|_| EmuError::BusNotFitted)?;
        }
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
                .power_on(0, &mut io, &mut ipmc_core::trace::NoTrace)
                .map_err(|_| EmuError::PowerOnRefused)?;
        }
        Ok(Emulator {
            link,
            ctx: Context::new(resolved),
            monitor,
            analog,
            io,
            tx_policy: config.tx_policy,
        })
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
    /// link layer answers the queued requests. Returns the number of requests answered.
    pub fn poll(&mut self, now_ms: u32, trace: &mut dyn Trace) -> usize {
        if self
            .monitor
            .service(now_ms, &mut self.analog, &mut self.io, trace)
        {
            let snapshot = *self.monitor.snapshot();
            self.ctx.update_sensors(now_ms, &snapshot, trace);
        }
        self.link.poll(&mut self.ctx, now_ms, trace)
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

    /// link -- the link layer, for counters and bus state.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// context -- the dispatch context, for the FRU state and the latest sensor snapshot.
    pub fn context(&self) -> &Context {
        &self.ctx
    }
}
