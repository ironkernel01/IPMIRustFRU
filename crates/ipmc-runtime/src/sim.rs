// sim.rs - the simulated board: `Sim` implements `Board` and `Platform` over host handles, so the
// runtime tests and the emulator run the same code the firmware runs. Feature `sim` (std); no
// firmware image enables it.
//
// Every handle is a clone that shares its state with the test or the emulator that built it: the
// serial ports record what is written and take what is fed (`SimSerial`), the clock and the cycle
// counter are set by the caller, the IPMB buses (`SimIpmb`) take frames into per-bus queues and
// keep every frame written with its outcome, and the FRU flash is the RAM flash. A reset is an
// unwinding panic carrying `Reset`, so the `-> !` functions of the runtime end in a host test.

extern crate std;

use core::cell::{OnceCell, RefCell};
use std::boxed::Box;
use std::collections::VecDeque;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

use ipmc_boot::header::{self, HEADER_LEN};
use ipmc_core::dispatch::Context as DispatchContext;
use ipmc_core::ext::Extension;
use ipmc_core::fru_state::Transition;
use ipmc_core::ipmb::Frame;
use ipmc_core::link::BusSpeed;
use ipmc_core::queue::Bus;
use ipmc_core::requester::Requester;
use ipmc_core::trace::Trace;
use ipmc_fru::RamFlash;
use ipmc_hal::board::{Board, Platform};
use ipmc_hal::frame::{FrameSource, RxFrame, RX_FRAME_MAX, RX_QUEUE_LEN};
use ipmc_hal::ipmb::{BringUpFailure, DriverStats, IpmbBus, MasterError, MasterStats, Recovery};
use ipmc_hal::system::{
    ClockDomain, ClockSummary, ResetCause, SystemControl, Watchdog, WatchdogError,
};
pub use ipmc_hal::testing::{
    FakePort as SimSerial, TestClock as SimClock, TestCycles as SimCycles, TestLed as SimLed,
    TestStraps as SimStraps,
};
use ipmc_sensors::board_sensors::sim_analog;
use ipmc_sensors::drivers::sim::{SimAnalog, SimInputs, SimPower, SIM_RAIL_WIRING};
use ipmc_sensors::hal::{DigitalInputs, Input, PowerSwitch, ResetOutput, SourceError};

use crate::run::{BoardParts, StartInfo, SystemParts};

/// SimPins -- the board I/O: simulated inputs and enable, and a log of the reset output.
pub struct SimPins {
    pub inputs: SimInputs,
    pub power: SimPower,
    pub resets: Vec<bool>,
}

impl SimPins {
    pub fn new() -> Self {
        SimPins {
            inputs: SimInputs::new(),
            power: SimPower::new(),
            resets: Vec::new(),
        }
    }
}

impl Default for SimPins {
    fn default() -> Self {
        SimPins::new()
    }
}

impl DigitalInputs for SimPins {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        self.inputs.level_high(input)
    }
}

impl PowerSwitch for SimPins {
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
        self.power.set_enable(on)
    }
    fn is_enabled(&self) -> bool {
        self.power.is_enabled()
    }
}

impl ResetOutput for SimPins {
    fn set_reset(&mut self, asserted: bool) -> Result<(), SourceError> {
        self.resets.push(asserted);
        Ok(())
    }
}

/// BusScript -- what one bus of the simulated board does and has seen.
#[derive(Default)]
pub struct BusScript {
    /// Outcomes of the next `bring_up` calls; an empty list means success without recovery.
    pub bring_up: VecDeque<Result<Recovery, BringUpFailure>>,
    /// Outcomes of the next `write` calls; an empty list means the policy decides.
    pub writes: VecDeque<Result<(), MasterError>>,
    /// Write policy: every `fail_every`-th write fails with `fail_error` (0: none fail).
    pub fail_every: u32,
    /// The error of the writes `fail_every` fails.
    pub fail_error: Option<MasterError>,
    /// Writes made so far (the count `fail_every` divides).
    pub written: u32,
    /// Both lines read high.
    pub lines_high: bool,
    /// Refusals `suspend` gives before it succeeds (a slave still busy).
    pub suspend_busy: u32,
    /// The receive counters `slave_stats` reports.
    pub stats: DriverStats,
    /// Calls of `clear_bus_fault`.
    pub cleared: u32,
    /// The target runs (a `bring_up` succeeded).
    pub running: bool,
    inbox: Option<Rc<RefCell<VecDeque<RxFrame>>>>,
}

/// Sent -- one frame the master wrote: the bus, the bytes and whether the write succeeded.
pub type Sent = (Bus, Vec<u8>, bool);

/// IpmbState -- what the simulated buses did, in order: the frames written and the calls made.
#[derive(Default)]
pub struct IpmbState {
    pub bus: [BusScript; 2],
    /// Write policy over both buses: every `fail_every`-th write of the board fails with
    /// `fail_error` (0: none fail). A bus's own policy and script come first.
    pub fail_every: u32,
    /// The error of the writes `fail_every` fails.
    pub fail_error: Option<MasterError>,
    /// Writes made so far on both buses (the count `fail_every` divides).
    pub written: u32,
    /// Every frame written: the bus, the bytes and the outcome.
    pub outbox: Vec<Sent>,
    /// Calls of `bring_up`, `suspend`, `suspend_faulted`, `resume` and `set_speed`, as text.
    pub events: Vec<String>,
}

impl IpmbState {
    /// push_frame -- a frame arrives on `bus` with STOP time `stop_ms`.
    pub fn push_frame(&mut self, bus: Bus, bytes: &[u8], stop_ms: u32) {
        assert!(bytes.len() <= RX_FRAME_MAX, "a frame fits RX_FRAME_MAX");
        let mut frame = RxFrame::EMPTY;
        frame.bytes[..bytes.len()].copy_from_slice(bytes);
        frame.len = u8::try_from(bytes.len()).expect("a frame fits RX_FRAME_MAX");
        frame.stop_ms = stop_ms;
        let inbox = self.bus[bus.index()]
            .inbox
            .as_ref()
            .expect("the bus was brought up");
        let mut queue = inbox.borrow_mut();
        // The queue of the board holds RX_QUEUE_LEN - 1 frames.
        assert!(queue.len() < Self::INBOX_MAX, "the queue has room");
        queue.push_back(frame);
    }

    /// INBOX_MAX -- frames one bus queue holds (the board's queue holds one less than its length).
    pub const INBOX_MAX: usize = RX_QUEUE_LEN - 1;

    /// queued -- frames waiting in the queue of `bus`; 0 while its target does not run.
    pub fn queued(&self, bus: Bus) -> usize {
        self.bus[bus.index()]
            .inbox
            .as_ref()
            .map_or(0, |queue| queue.borrow().len())
    }

    /// take_outbox -- the frames written since the last call.
    pub fn take_outbox(&mut self) -> Vec<Sent> {
        core::mem::take(&mut self.outbox)
    }
}

/// write_policy -- the outcome of the `count`-th write under a policy that fails every `every`-th
/// write with `error`.
fn write_policy(every: u32, error: Option<MasterError>, count: u32) -> Result<(), MasterError> {
    match error {
        Some(error) if every > 0 && count.is_multiple_of(every) => Err(error),
        _ => Ok(()),
    }
}

/// SimRx -- the main-loop end of one simulated bus queue. `peek` hands out a reference, so the
/// head frame moves from the shared queue into `head` first.
pub struct SimRx {
    shared: Rc<RefCell<VecDeque<RxFrame>>>,
    head: OnceCell<RxFrame>,
}

impl FrameSource for SimRx {
    fn peek(&self) -> Option<&RxFrame> {
        if self.head.get().is_none() {
            if let Some(frame) = self.shared.borrow_mut().pop_front() {
                let _ = self.head.set(frame);
            }
        }
        self.head.get()
    }
    fn dequeue(&mut self) -> Option<RxFrame> {
        self.head
            .take()
            .or_else(|| self.shared.borrow_mut().pop_front())
    }
}

/// SimIpmb -- both buses of the simulated board; the state is shared with the caller.
#[derive(Default)]
pub struct SimIpmb {
    pub state: Rc<RefCell<IpmbState>>,
    stats: MasterStats,
}

impl IpmbBus for SimIpmb {
    type Rx = SimRx;

    fn bring_up(
        &mut self,
        bus: Bus,
        _own_addr8: u8,
    ) -> Result<(Self::Rx, Recovery), BringUpFailure> {
        let mut state = self.state.borrow_mut();
        state.events.push(std::format!("bring_up {}", bus.name()));
        let script = &mut state.bus[bus.index()];
        let recovery = script
            .bring_up
            .pop_front()
            .unwrap_or(Ok(Recovery::NotNeeded))?;
        // A new queue for every start: frames of the run before are gone.
        let shared = Rc::new(RefCell::new(VecDeque::new()));
        script.inbox = Some(Rc::clone(&shared));
        script.running = true;
        Ok((
            SimRx {
                shared,
                head: OnceCell::new(),
            },
            recovery,
        ))
    }

    fn write(&mut self, bus: Bus, frame: &[u8]) -> Result<(), MasterError> {
        let mut state = self.state.borrow_mut();
        state.written += 1;
        let all = write_policy(state.fail_every, state.fail_error, state.written);
        let script = &mut state.bus[bus.index()];
        script.written += 1;
        let own = write_policy(script.fail_every, script.fail_error, script.written);
        let outcome = script.writes.pop_front().unwrap_or(own.and(all));
        state.outbox.push((bus, frame.to_vec(), outcome.is_ok()));
        outcome
    }

    fn set_speed(&mut self, bus: Bus, speed: BusSpeed) -> bool {
        let mut state = self.state.borrow_mut();
        state
            .events
            .push(std::format!("set_speed {} {}", bus.name(), speed.name()));
        true
    }

    fn lines_high(&self, bus: Bus) -> bool {
        self.state.borrow().bus[bus.index()].lines_high
    }

    fn clear_bus_fault(&mut self, bus: Bus) -> Result<(), MasterError> {
        self.state.borrow_mut().bus[bus.index()].cleared += 1;
        Ok(())
    }

    fn suspend(&mut self, bus: Bus) -> bool {
        let mut state = self.state.borrow_mut();
        let script = &mut state.bus[bus.index()];
        if !script.running {
            return false;
        }
        if script.suspend_busy > 0 {
            script.suspend_busy -= 1;
            return false;
        }
        state.events.push(std::format!("suspend {}", bus.name()));
        true
    }

    fn suspend_faulted(&mut self, bus: Bus) -> bool {
        let mut state = self.state.borrow_mut();
        let running = state.bus[bus.index()].running;
        if running {
            state
                .events
                .push(std::format!("suspend_faulted {}", bus.name()));
        }
        running
    }

    fn resume(&mut self, bus: Bus) {
        self.state
            .borrow_mut()
            .events
            .push(std::format!("resume {}", bus.name()));
    }

    fn slave_stats(&self, bus: Bus) -> DriverStats {
        self.state.borrow().bus[bus.index()].stats
    }

    fn master_stats(&self) -> &MasterStats {
        &self.stats
    }
}

/// Reset -- the payload of the panic that `SimSystem::system_reset` raises, so a test sees the
/// reset as the end of a `-> !` function.
pub struct Reset;

/// SimSystem -- processor control that "resets" by unwinding with `Reset`.
#[derive(Default)]
pub struct SimSystem;

impl SystemControl for SimSystem {
    fn system_reset(&mut self) -> ! {
        // No panic hook runs: the reset is no failure.
        std::panic::resume_unwind(Box::new(Reset))
    }
    fn park(&self) -> ! {
        std::panic::resume_unwind(Box::new(Reset))
    }
    fn wait_for_interrupt(&self) {}
}

/// SimWatchdog -- a watchdog that only remembers its period.
#[derive(Default)]
pub struct SimWatchdog(Option<u32>);

impl Watchdog for SimWatchdog {
    fn start(&mut self, period_ms: u32) -> Result<u32, WatchdogError> {
        self.0 = Some(period_ms);
        Ok(period_ms)
    }
    fn feed(&mut self) {}
    fn period_ms(&self) -> Option<u32> {
        self.0
    }
}

/// SimExt -- the extension of the simulated board: a boxed one, the build's own until a host test
/// sets another.
pub struct SimExt {
    ext: Box<dyn Extension>,
}

impl Default for SimExt {
    fn default() -> Self {
        SimExt {
            ext: Box::new(ipmc_ext::Active::default()),
        }
    }
}

impl SimExt {
    /// set -- runs `ext` from now on.
    pub fn set(&mut self, ext: Box<dyn Extension>) {
        self.ext = ext;
    }
}

impl Extension for SimExt {
    fn command(
        &mut self,
        ctx: &mut DispatchContext,
        req: &Frame,
        out: &mut [u8],
    ) -> Option<(&'static str, Result<usize, u8>)> {
        self.ext.command(ctx, req, out)
    }

    fn command_name(&self, netfn: u8, cmd: u8) -> Option<&'static str> {
        self.ext.command_name(netfn, cmd)
    }

    fn poll(
        &mut self,
        ctx: &mut DispatchContext,
        requester: &mut Requester,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) {
        self.ext.poll(ctx, requester, now_ms, trace);
    }

    fn warm_reset(&mut self) {
        self.ext.warm_reset();
    }

    fn power_on_allowed(&self, ctx: &DispatchContext) -> bool {
        self.ext.power_on_allowed(ctx)
    }

    fn on_sample(
        &mut self,
        ctx: &DispatchContext,
        transition: Option<Transition>,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) {
        self.ext.on_sample(ctx, transition, now_ms, trace);
    }
}

/// Sim -- the simulated board.
pub struct Sim;

impl Board for Sim {
    type Clock = SimClock;
    type Cycles = SimCycles;
    type Console = SimSerial;
    type App = SimSerial;
    type Ipmb = SimIpmb;
    type Fru = RamFlash;
    type Analog = SimAnalog;
    type Io = SimPins;
    type Straps = SimStraps;
    type Led = SimLed;
    const APP_UART_PERIPHERAL: &'static str = "SIM";
}

impl Platform for Sim {
    type System = SimSystem;
    type Watchdog = SimWatchdog;
    type KeptSeq = [u32; 4];
    type BootRequest = [u32; 4];
}

/// clock_summary -- the clock tree of the captured board, as the simulated board reports it.
pub fn clock_summary() -> ClockSummary {
    let domain = |label, hz| Some(ClockDomain { label, hz });
    ClockSummary {
        source: "HSE from ST-LINK MCO (bypass), PLL",
        fault: None,
        domains: [
            domain("SYSCLK:       ", 84_000_000),
            domain("HCLK (AHB):   ", 84_000_000),
            domain("PCLK1 (APB1): ", 42_000_000),
            domain("PCLK2 (APB2): ", 84_000_000),
        ],
    }
}

/// image_header -- a runtime header as the patched image carries it: v0.1.0, 143456 bytes and the
/// digest that starts bd7855a1c66ee421.
pub fn image_header() -> [u8; HEADER_LEN] {
    let mut raw = header::placeholder(header::version_bytes("0.1.0"), 0);
    raw[8..12].copy_from_slice(&143_456u32.to_le_bytes());
    raw[0x20..0x28].copy_from_slice(&[0xBD, 0x78, 0x55, 0xA1, 0xC6, 0x6E, 0xE4, 0x21]);
    raw
}

/// start_info -- what the simulated board found at start: a software reset, no fault, the clock
/// tree of `clock_summary`, the header of `image_header` and the start-up choices of the defs.rs
/// defines (the emulator replaces them from its `EmuConfig`).
pub fn start_info() -> StartInfo {
    StartInfo {
        reset_cause: ResetCause::Software,
        last_fault: None,
        clock: clock_summary(),
        app_uart_fault: None,
        image_header: image_header(),
        version: "0.1.0",
        events_at_boot: ipmc_defs::IPMC_EVENTS_AT_BOOT,
        power_at_boot: ipmc_defs::IPMC_POWER_AT_BOOT,
    }
}

/// GOLDEN_RAW -- the strap levels of the captured board: GA4..GA0 = 11111, GAP = 1, NVMRO = 1.
pub const GOLDEN_RAW: u8 = 0x7F;

/// Handles -- the clones of the simulated board's parts that the caller reads and drives.
pub struct Handles {
    pub console: SimSerial,
    pub app: SimSerial,
    pub clock: SimClock,
    pub cycles: SimCycles,
    pub straps: SimStraps,
    pub led: SimLed,
    pub ipmb: Rc<RefCell<IpmbState>>,
}

/// parts -- a complete simulated board over `flash`: both buses start, the sensors run on the
/// simulated analog source and inputs, the straps read `GOLDEN_RAW`, and the console ring is the firmware's.
/// Returns the parts `run` or `start` takes and the handles the caller keeps.
pub fn parts(flash: RamFlash) -> (BoardParts<Sim>, SystemParts<Sim>, Handles) {
    let handles = Handles {
        console: SimSerial::new(ipmc_defs::IPMC_UART_TX_BUF - 1),
        app: SimSerial::roomy(),
        clock: SimClock::default(),
        cycles: SimCycles::default(),
        straps: SimStraps::default(),
        led: SimLed::default(),
        ipmb: Rc::default(),
    };
    handles.straps.set(GOLDEN_RAW);
    let ipmb = SimIpmb {
        state: Rc::clone(&handles.ipmb),
        ..SimIpmb::default()
    };
    let board = BoardParts {
        start: start_info(),
        clock: handles.clock.clone(),
        cycles: handles.cycles.clone(),
        console: handles.console.clone(),
        app: handles.app.clone(),
        ipmb: Ok(ipmb),
        fru: Ok(flash),
        analog: Some(Ok(sim_analog(&SIM_RAIL_WIRING))),
        io: Some(SimPins::new()),
        rail_wiring: &SIM_RAIL_WIRING,
        straps: handles.straps.clone(),
        led: handles.led.clone(),
    };
    let system = SystemParts {
        system: SimSystem,
        watchdog: SimWatchdog::default(),
        kept_seq: [0; 4],
        boot_request: [0; 4],
        #[cfg(ipmc_eval)]
        eval_expired: || false,
    };
    (board, system, handles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_queue_per_bus_and_peek_does_not_consume() {
        let mut ipmb = SimIpmb::default();
        let (mut rx, _) = ipmb.bring_up(Bus::A, 0x82).expect("A starts");
        assert!(rx.peek().is_none());
        ipmb.state.borrow_mut().push_frame(Bus::A, &[1, 2, 3], 7);
        ipmb.state.borrow_mut().push_frame(Bus::A, &[4], 8);
        assert_eq!(rx.peek().map(|f| f.stop_ms), Some(7));
        assert_eq!(rx.peek().map(|f| f.len), Some(3));
        assert_eq!(rx.dequeue().map(|f| f.stop_ms), Some(7));
        assert_eq!(rx.dequeue().map(|f| f.stop_ms), Some(8));
        assert!(rx.dequeue().is_none());
    }

    #[test]
    fn the_write_policy_fails_every_nth_write_and_a_script_wins() {
        let mut ipmb = SimIpmb::default();
        {
            let mut state = ipmb.state.borrow_mut();
            state.bus[0].fail_every = 2;
            state.bus[0].fail_error = Some(MasterError::DataNack);
            state.bus[0].writes.push_back(Err(MasterError::BusBusy));
        }
        let outcomes: Vec<_> = (0..4).map(|_| ipmb.write(Bus::A, &[9])).collect();
        assert_eq!(
            outcomes,
            [
                Err(MasterError::BusBusy),
                Err(MasterError::DataNack),
                Ok(()),
                Err(MasterError::DataNack)
            ]
        );
        let sent = ipmb.state.borrow_mut().take_outbox();
        assert_eq!(
            sent.iter().map(|s| s.2).collect::<Vec<_>>(),
            [false, false, true, false]
        );
        assert!(ipmb.state.borrow().outbox.is_empty());
    }
}
