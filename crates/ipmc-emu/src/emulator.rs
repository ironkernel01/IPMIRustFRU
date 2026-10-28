//! emulator.rs - the IPMC in software: the firmware's runtime on the simulated board.
//!
//! An `Emulator` is a board that runs `ipmc-runtime` (`ipmc_runtime::run::start` and `Runtime::pass`,
//! the code the firmware runs) on `ipmc_runtime::sim::Sim`: the console and the application port
//! are text buffers, the IPMB buses are frame queues with a record of every frame written, the
//! FRU flash is the RAM flash and the sensors read the simulators. This file adds only what a
//! host board owns: the time a caller sets, the frames a caller feeds in and takes out, the
//! payload supply that follows PWR_EN, and the rebuild after a Cold Reset from the flash and the
//! words kept across it, as the processor restart does on the board.
//!
//! The methods mirror the calls the drivers and the main loop make (PLAN 3.3a): `rx` is the slave
//! driver's frame at a STOP, `poll` is the main loop run until the link has nothing more to send,
//! `take_sent` hands over the frames the master wrote, each with the outcome the simulated bus
//! gave it. Nothing here knows where frames come from, so the replay and the serve mode share it.

use core::fmt;
use ipmc_core::clock::earlier;
use ipmc_core::dispatch::Context;
use ipmc_core::ext::Extension;
use ipmc_core::link::{BusMode, FrameDrop, IpmbBPowerUp, Link, LinkConfig, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::requester;
use ipmc_core::trace::Trace;
use ipmc_fru::RamFlash;
use ipmc_hal::frame::RX_FRAME_MAX;
use ipmc_hal::ipmb::{IpmbBus, MasterError};
use ipmc_hal::system::Straps;
use ipmc_hal::time::Clock;
use ipmc_runtime::link::{IpmbLink, LinkArgs, Restart};
use ipmc_runtime::run::{start, Runtime};
use ipmc_runtime::sim::{self, Handles, Reset, Sim, SimIpmb};
use ipmc_sensors::hal::{Input, PowerSwitch};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

/// STRAP_PINS_HIGH -- the raw strap word with every pin high: what the board reads with no strap
/// wired (the firmware's `ga::read_raw` before init). With IPMB_GA_SOURCE forced, `addr::resolve`
/// ignores it.
pub const STRAP_PINS_HIGH: u8 = sim::GOLDEN_RAW;

/// PASSES_MAX -- bound on the main-loop passes one `poll` runs: the first attempt and every resend
/// of the one response a request produces, and the transmissions of the IPMC's own requests (one
/// request with every retry, twice: an event raised by the poll and one the answer to it
/// releases), with margin for a replayed duplicate.
const PASSES_MAX: usize =
    2 * (ipmc_core::link::TX_RETRIES as usize + 1) + 2 * requester::MAX_TRANSMISSIONS as usize + 2;

/// FRU_PERSIST -- Write FRU Data is saved to the flash log (defs.rs IPMC_FRU_PERSIST).
const FRU_PERSIST: bool = ipmc_defs::IPMC_FRU_PERSIST;

/// EmuConfig -- the build choices of one emulated board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmuConfig {
    /// Raw strap pin word the board reads (`addr::resolve` decodes it).
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
    /// The event generator of a Tier-2/3 build is on from start-up. The emulator and the
    /// conformance runs start with events on whatever IPMC_EVENTS_AT_BOOT says (PLAN decision 24:
    /// the define acts in the firmware start-up only, where the board fills
    /// `StartInfo::events_at_boot` from it); false gives the bench start-up, in which the runtime
    /// sends Set Event Receiver FFh and prints the `Events: off` line. No effect in a Tier-1 build.
    pub events_at_start: bool,
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
            events_at_start: true,
        }
    }

    /// link -- the link layer configuration of this board.
    const fn link(&self) -> LinkConfig {
        LinkConfig {
            b_fitted: self.b_fitted,
            tx_policy: self.tx_policy,
            b_power_up: self.b_power_up,
            requests_allowed: LinkConfig::from_defs().requests_allowed,
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
}

impl EmuError {
    /// name -- text for messages.
    pub const fn name(self) -> &'static str {
        match self {
            EmuError::BusNotFitted => "IPMB-B enabled at start but not fitted",
        }
    }
}

impl fmt::Display for EmuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for EmuError {}

/// ExtensionFactory -- builds the extension a board starts with; the cold reset calls it again.
pub type ExtensionFactory = fn() -> Box<dyn Extension>;

/// build_active -- the extension this build runs: the Tier-2/3 extension when built in, else none.
fn build_active() -> Box<dyn Extension> {
    Box::new(ipmc_ext::Active::default())
}

/// Tx -- one frame the master wrote: the bus, the bytes and whether the simulated bus took it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tx {
    pub bus: Bus,
    pub bytes: Vec<u8>,
    pub ok: bool,
}

/// Home -- where the emulator keeps the one IPMB link.
type Home = Box<IpmbLink<Sim>>;

/// Emulator -- the board in software.
pub struct Emulator {
    config: EmuConfig,
    /// Builds the extension at start and again after a cold reset.
    make_ext: ExtensionFactory,
    /// The started runtime of the current run (a Cold Reset replaces it).
    runtime: Box<Runtime<Sim, Home>>,
    /// The handles of the simulated board the current run reads and drives.
    handles: Handles,
    /// The caller's time at the start of the current run: the board's clock counts from it, as
    /// the tick of the processor counts from its reset.
    epoch: u32,
    /// The latest time the caller gave; the board's time never goes back.
    latest: u32,
    /// Every frame written and not yet taken (`take_sent`), across Cold Resets.
    sent: Vec<Tx>,
    /// Console text that does not yet end a line (the prompt waits for its trace lines).
    partial: String,
    /// Every Nth write fails (0: none), counted over both buses.
    fail_every: u32,
    /// Writes made over all runs, so the count continues across a Cold Reset.
    written: u32,
    /// A bus whose frames are lost and whose writes fail.
    down: [bool; 2],
}

impl Emulator {
    /// new -- a board in its start-up state for `config`: Link and Context for the resolved
    /// address, a Monitor over the simulators at their defs.rs start values, payload power
    /// requested when `power_on_at_start`, IPMB-B enabled when `b_enabled_at_start`.
    /// Err(BusNotFitted) when IPMB-B is asked to start enabled but is not fitted.
    pub fn new(config: EmuConfig) -> Result<Emulator, EmuError> {
        Emulator::new_with_flash(config, RamFlash::new())
    }

    /// new_with_flash -- `new` for a board whose flash log holds `flash`: the FRU store and the
    /// non-volatile settings are restored from it before the payload power is decided, as at a
    /// boot of the firmware (the power-on hook sees the stored default-activation lock).
    pub fn new_with_flash(config: EmuConfig, flash: RamFlash) -> Result<Emulator, EmuError> {
        Emulator::build(config, build_active, flash)
    }

    /// build -- a power-on start of `config` with the extension `make_ext` builds over `flash`.
    fn build(
        config: EmuConfig,
        make_ext: ExtensionFactory,
        flash: RamFlash,
    ) -> Result<Emulator, EmuError> {
        let (runtime, handles) = Self::start_run(&config, make_ext, flash, [0; 4])?;
        Ok(Emulator {
            config,
            make_ext,
            runtime,
            handles,
            epoch: 0,
            latest: 0,
            sent: Vec::new(),
            partial: String::new(),
            fail_every: 0,
            written: 0,
            down: [false; 2],
        })
    }

    /// start_run -- the start-up of one run: the simulated board over `flash` with the request
    /// sequence words `kept`, started by `ipmc_runtime::run::start`. The home closure builds the
    /// link as the board's `home` does, with the emulator's configuration: IPMB-B brought up
    /// when fitted, the extension from the factory, IPMB-B enabled when configured, and the
    /// start-up power-on request held back when the configuration asks for none.
    fn start_run(
        config: &EmuConfig,
        make_ext: ExtensionFactory,
        flash: RamFlash,
        kept: [u32; 4],
    ) -> Result<(Box<Runtime<Sim, Home>>, Handles), EmuError> {
        if config.b_enabled_at_start && !config.b_fitted {
            return Err(EmuError::BusNotFitted);
        }
        let (mut board, mut system, handles) = sim::parts(flash);
        handles.straps.set(config.strap);
        system.kept_seq = kept;
        board.start.power_at_boot = config.power_on_at_start;
        board.start.events_at_boot = config.events_at_start;
        let runtime = start(board, system, |mut args: LinkArgs<Sim>| {
            let address = ipmc_core::addr::resolve(args.straps.read_raw());
            bring_up_b(&mut args, config.b_fitted, &address);
            let mut link = IpmbLink::new(args);
            link.reconfigure(config.link());
            link.extension_mut().set(make_ext());
            if config.b_enabled_at_start {
                // Cannot fail: the check above ensured IPMB-B is fitted.
                let _ = link
                    .link_mut()
                    .set_ipmb_state(None, Some(BusMode::LocalControl));
            }
            Some(Box::new(link))
        });
        let runtime = Box::new(runtime);
        Ok((runtime, handles))
    }

    /// flash -- the flash log as a power cycle leaves it. A save that is pending (an accepted
    /// Write FRU Data or settings change inside IPMC_FRU_PERSIST_DELAY_MS) is completed first:
    /// the emulator's power cycle is a clean one, taken when the board is quiet.
    pub fn flash(&mut self) -> RamFlash {
        if FRU_PERSIST {
            let now = self.handles.clock.millis();
            let quiet = ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS
                .max(ipmc_defs::IPMC_FRU_ERASE_QUIET_MS)
                .saturating_add(1);
            self.handles.clock.set(now.wrapping_add(quiet));
            self.runtime.console().link_mut().service_fru_log();
            self.handles.clock.set(now);
        }
        self.run_flash()
    }

    /// run_flash -- a copy of the flash of the current run (a fresh one while the log is off).
    fn run_flash(&self) -> RamFlash {
        self.runtime
            .console_view()
            .link()
            .fru_flash()
            .cloned()
            .unwrap_or_else(RamFlash::new)
    }

    /// with_extension -- runs the extension `make` builds instead of the build's own, from a
    /// start-up that already uses it and after every cold reset (tests drive the hooks with
    /// their own).
    pub fn with_extension(mut self, make: ExtensionFactory) -> Emulator {
        let flash = self.flash();
        match Emulator::build(self.config, make, flash) {
            Ok(emulator) => emulator,
            Err(_) => self, // `new` built the same state from the same config
        }
    }

    /// own_address -- the IPMB address (8-bit form) the board answers at.
    pub fn own_address(&self) -> u8 {
        self.link().own_address()
    }

    /// tx_policy -- the response bus policy this board was built with.
    pub fn tx_policy(&self) -> TxPolicy {
        self.config.tx_policy
    }

    /// set_fail_every -- every `every`th write over both buses fails (0: none), so the Link
    /// resends the response on the other bus (Rule 4.1.1-7).
    pub fn set_fail_every(&mut self, every: u32) {
        self.fail_every = every;
        self.apply_policy();
    }

    /// set_bus_down -- breaks (true) or repairs (false) a bus: the writes on it fail with an
    /// address NACK, as if the Chassis Manager's side did not answer.
    pub fn set_bus_down(&mut self, bus: Bus, down: bool) {
        self.down[bus.index()] = down;
        self.apply_policy();
    }

    /// apply_policy -- writes the failure choices into the simulated buses of the current run.
    fn apply_policy(&self) {
        let mut state = self.handles.ipmb.borrow_mut();
        state.fail_every = self.fail_every;
        state.fail_error = Some(MasterError::AddressNack);
        state.written = self.written;
        for bus in Bus::BOTH {
            let script = &mut state.bus[bus.index()];
            let down = self.down[bus.index()];
            script.fail_every = u32::from(down);
            script.fail_error = down.then_some(MasterError::AddressNack);
        }
    }

    /// rx -- a frame received on `bus` at `now_ms` (the slave driver's frame at a STOP): it joins
    /// the bus queue, and the next pass takes it into the link. Err(BusUnavailable) when no
    /// target runs on the bus (IPMB-B not fitted in this build), Err(IngressFull) when the
    /// queue holds as many frames as the board's does, Err(Parse) for a frame longer than the
    /// board's receive buffer; the frame is lost then, as on the board.
    pub fn rx(&mut self, bus: Bus, bytes: &[u8], now_ms: u32) -> Result<(), FrameDrop> {
        let now = self.advance(now_ms);
        let mut state = self.handles.ipmb.borrow_mut();
        let fitted = bus == Bus::A || self.config.b_fitted;
        if !fitted || !state.bus[bus.index()].running {
            return Err(FrameDrop::BusUnavailable);
        }
        if bytes.len() > RX_FRAME_MAX {
            return Err(FrameDrop::Parse(ipmc_core::ipmb::IpmbError::TooLong));
        }
        if state.queued(bus) >= sim::IpmbState::INBOX_MAX {
            return Err(FrameDrop::IngressFull);
        }
        state.push_frame(bus, bytes, now.wrapping_sub(self.epoch));
        Ok(())
    }

    /// advance -- the time of a call: `now_ms`, or the latest time given when that is earlier
    /// (the board's clock never runs back).
    fn advance(&mut self, now_ms: u32) -> u32 {
        if !earlier(now_ms, self.latest) {
            self.latest = now_ms;
        }
        self.latest
    }

    /// follow_payload_supply -- the emulator's payload supply: with `IPMC_SIM_PGOOD_HIGH` true the
    /// PGOOD input reads high while PWR_EN is asserted and low once it is released, so a
    /// deactivation ends in the complete shutdown that takes the FRU from M6 to M1
    /// (`SensorSnapshot::payload_shut_down`, Rule 4.1.5.4.5-1). With false PGOOD stays at its
    /// simulated level (low) whatever PWR_EN does, so the PGOOD timeout and the power failure
    /// stay reachable (`pgood_follows_pwr_en`).
    fn follow_payload_supply(&mut self) {
        let sensing = self.runtime.console().sensing_mut();
        let on = sensing.io.is_enabled();
        if let Some(level) = pgood_follows_pwr_en(ipmc_defs::IPMC_SIM_PGOOD_HIGH, on) {
            if let Some(pins) = sensing.io.hw.as_mut() {
                pins.inputs.set_level(Input::Pgood, level);
            }
        }
    }

    /// poll -- the main loop at `now_ms`: passes of `Runtime::pass` (payload supply, console,
    /// sensor sample and hand-offs, link: receive, requests, one write, FRU save, reset), at
    /// least two and then until a pass writes nothing, so a response and the requests the IPMC
    /// raises with it leave in the same call. A Cold Reset the link asks for restarts the board
    /// (`cold_reset`) and ends the call; the new run starts with the next one. The console text of every pass goes to `trace` line by line. Returns the
    /// number of frames written.
    pub fn poll(&mut self, now_ms: u32, trace: &mut dyn Trace) -> usize {
        let now = self.advance(now_ms);
        let mut total = 0;
        for pass in 0..PASSES_MAX {
            self.set_time(now);
            self.follow_payload_supply();
            let restart = self.runtime.pass();
            let written = self.collect_sent();
            self.forward_console(trace);
            total += written;
            if let Some(restart) = restart {
                // The new run starts with the next call.
                self.cold_reset(restart, now);
                break;
            }
            if written == 0 && pass > 0 {
                break;
            }
        }
        total
    }

    /// set_time -- the board's clock and the flash clock at the caller's time `now`.
    fn set_time(&mut self, now: u32) {
        let ms = now.wrapping_sub(self.epoch);
        self.handles.clock.set(ms);
        if let Some(flash) = self.runtime.console().link_mut().fru_flash_mut() {
            flash.now = ms;
        }
    }

    /// collect_sent -- moves the frames written since the last call into `sent`; returns how
    /// many there were.
    fn collect_sent(&mut self) -> usize {
        let written = self.handles.ipmb.borrow_mut().take_outbox();
        let count = written.len();
        self.sent.extend(
            written
                .into_iter()
                .map(|(bus, bytes, ok)| Tx { bus, bytes, ok }),
        );
        count
    }

    /// take_sent -- the frames written since the last call, in order, each with whether the
    /// simulated bus took it.
    pub fn take_sent(&mut self) -> Vec<Tx> {
        core::mem::take(&mut self.sent)
    }

    /// forward_console -- the console text since the last call, one `trace` line per text line
    /// (the prompt that precedes a line is dropped).
    fn forward_console(&mut self, trace: &mut dyn Trace) {
        let text = self.handles.console.take_text();
        self.partial.push_str(&text.replace("\r\n", "\n"));
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            let mut line = line.trim_end_matches('\n');
            // A trace line follows the prompt (the menu appends `> ` to the root name) on its
            // line: it is forwarded without it.
            while let Some(rest) = line
                .strip_prefix(ipmc_defs::IPMC_CONSOLE_PROMPT)
                .and_then(|rest| rest.strip_prefix("> "))
            {
                line = rest;
            }
            if !line.is_empty() {
                trace.line(format_args!("{line}"));
            }
        }
    }

    /// cold_reset -- the processor restart the link returned at `now`: the restart as the
    /// firmware performs it (`Runtime::restart`: ports flushed, the request sequence state kept
    /// in the words, the reset, which the simulated board raises as an unwinding `Reset`), then a
    /// start-up over the flash of the run that ended and the kept words. The extension, the
    /// link, the context and the sensors are new, payload power is requested again, the FRU
    /// store comes from the flash (an accepted write not saved by then is lost, as on the board),
    /// and the board's clock counts from `now`.
    fn cold_reset(&mut self, restart: Restart, now: u32) {
        // `restart` does not return: the simulated reset unwinds with `Reset`.
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.runtime.restart(restart))) {
            if !payload.is::<Reset>() {
                resume_unwind(payload);
            }
        }
        self.written = self.handles.ipmb.borrow().written;
        let kept = *self.runtime.kept_seq();
        let flash = self.run_flash();
        // Cannot fail: `new` started the same configuration.
        if let Ok((runtime, handles)) = Self::start_run(&self.config, self.make_ext, flash, kept) {
            self.runtime = runtime;
            self.handles = handles;
            self.epoch = now;
            self.apply_policy();
        }
    }

    /// console_line -- types `line` and Enter into the board's console and lets the console run
    /// it (`Console::poll` until the received bytes are gone), as a person at the terminal
    /// would; the output waits for the next `poll`, which forwards it to its trace. The board's
    /// clock stays where the last call left it.
    pub fn console_line(&mut self, line: &str) {
        self.handles.console.feed(line);
        self.handles.console.feed("\u{d}");
        for _ in 0..=line.len() / ipmc_defs::IPMC_CONSOLE_BYTES_PER_POLL + 1 {
            let _ = self.runtime.console().poll();
        }
    }

    /// payload_power -- the state of the payload power controller (a processor reset restarts
    /// it, as it does on the board).
    pub fn payload_power(&self) -> ipmc_core::sensor::PowerState {
        self.runtime
            .console_view()
            .sensing()
            .monitor
            .power()
            .state()
    }

    /// link -- the link layer, for counters and bus state.
    pub fn link(&self) -> &Link {
        self.runtime.console_view().link().link()
    }

    /// context -- the dispatch context, for the FRU state and the latest sensor snapshot.
    pub fn context(&self) -> &Context {
        self.runtime.console_view().link().dispatch()
    }
}

/// bring_up_b -- IPMB-B comes up when the configuration fits it and the build's own start did
/// not (`start_buses` follows IPMB_BUS_B_ENABLE): the emulator varies the fitting per board.
fn bring_up_b(args: &mut LinkArgs<Sim>, b_fitted: bool, address: &ipmc_core::addr::AddrInfo) {
    if b_fitted && address.usable() && args.buses.rx_b.is_none() && args.buses.fault_b.is_none() {
        let ipmb: &mut SimIpmb = &mut args.buses.ipmb;
        match ipmb.bring_up(Bus::B, address.ipmb_address) {
            Ok((rx, _)) => args.buses.rx_b = Some(rx),
            Err(failure) => args.buses.fault_b = Some(failure),
        }
    }
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
