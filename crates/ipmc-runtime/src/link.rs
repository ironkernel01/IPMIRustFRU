// link.rs - the IPMB link service: composition of the IPMB link layer, the dispatch context, the
// two buses of a board and the FRU flash log.
//
// Owns the ipmc_core::link::Link value (inside `IpmbLink`, which the board keeps in a static), the
// consumer ends of the per-bus frame queues and the board's `IpmbBus`, and runs the main-loop
// step. The behaviour (ingress, duplicate cache, bus choice, resend, bus state) lives in
// ipmc-core; this file moves values between it, the board and the console. Each bus's receive
// interrupt (the board's driver) posts its frames to its queue; `service` drains both queues in
// STOP-time order into `Link::on_frame`, runs `Link::poll`, then sends each queued response through
// the board's `IpmbBus` and reports the outcome with `on_tx_result` (PLAN 3.3a). Nothing here runs
// in an interrupt handler.
//
// Generic over the board (`ipmc_hal::board::Board`): the link holds its own clones of the clock, the
// cycle counter, the strap pins and the console, so it reads no global. A restart of the processor
// is a value: `service` returns `Restart` once the reset gate lets a Cold Reset run, and the
// board's main loop hands it to `restart`, which needs the processor-level handles only a
// `Platform` has.
//
// The capture log (the ipmc-log crate) records every whole IPMB frame taken from a bus queue
// (before the parser) and every frame handed to the board, with its outcome, and the trace and
// status lines. Every trace sink handed to the core or the extension is a `LoggedTrace`: it
// records the line and prints it on the console unless the IPMI console trace is switched off.
// The DEBUG console group reads and switches all of this through the methods at the end of
// `impl IpmbLink`.
//
// The FRU flash log (ipmc_fru::persist) is saved from `service`, once a Write FRU Data has been
// quiet for IPMC_FRU_PERSIST_DELAY_MS. A save that has to erase the store sector first (once per
// 31 records, when the 16 KB sector is full) takes both IPMB slaves off the bus (`PausedErase`):
// the core stalls on flash during an erase, so the board stays silent for that time (at most
// 500 ms for the 16 KB sector, DS10086 Table 45; the console line "IPMB paused <n> ms" reports
// the measured time) instead of stretching SCL in the middle of a frame.

use core::fmt;

use ipmc_boot::request::{self, Interface};
use ipmc_core::addr::{self, AddrInfo};
use ipmc_core::clock;
use ipmc_core::dispatch::{cc, Context as DispatchContext, HandOffs, ResetRequest};
use ipmc_core::ext;
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::ipmb::{self, IPMB_MAX_LEN};
use ipmc_core::link::{BusSpeed, FrameDrop, Link, LinkConfig, LocalError, TxOutcome};
use ipmc_core::names::{CoreNames, TxFault};
use ipmc_core::queue::Bus;
use ipmc_core::reset_gate::ResetGate;
use ipmc_core::sensor::{LocalStatus, SensorSnapshot};
use ipmc_core::trace::Trace;
use ipmc_fru::persist::{FlashError, FlashLog, PersistError, Status};
use ipmc_fru::{FieldError, FruField, FruStore};
use ipmc_hal::board::{Board, Platform};
use ipmc_hal::frame::FrameSource;
use ipmc_hal::ipmb::{BringUpFailure, IpmbBus, MasterError, Recovery};
use ipmc_hal::kept::KeptWords;
use ipmc_hal::serial::SerialPort;
use ipmc_hal::system::{Straps, SystemControl};
use ipmc_hal::time::{Clock, CycleCounter};
use ipmc_log::{CaptureTrace, DefaultLog, Dir, FrameStatus, Kind, LogState, Record, TraceRow};
use ipmc_sensors::analog::BOARD_ANALOG;

use crate::fru_log::FruLog;
use crate::kept::keep_sequence;
use crate::text::{ConsoleText, ConsoleTrace};

#[cfg(test)]
mod tests;

/// RX_DRAIN_MAX -- most frames one `service` pass takes from the queues: both queues full at
/// once (2 x IPMB_RX_QUEUE_DEPTH). Frames that arrive during the pass wait for the next one.
const RX_DRAIN_MAX: usize = 2 * ipmc_defs::IPMB_RX_QUEUE_DEPTH;

/// PAUSE_WAIT_MS -- longest wait for a healthy slave to go idle before an erase (defs.rs
/// IPMC_FRU_PAUSE_WAIT_MS, guarded in ipmc-defs): a frame in progress ends within T1 = 20 ms
/// (VITA 46.11 Table 4.1.1-1). A slave that is still busy makes the erase fail with `Timeout`; the
/// save is retried one delay later.
const PAUSE_WAIT_MS: u32 = ipmc_defs::IPMC_FRU_PAUSE_WAIT_MS;
/// FRU_ERASE_QUIET_MS -- a run-time erase starts only after this long without an IPMB frame
/// (defs.rs IPMC_FRU_ERASE_QUIET_MS).
const FRU_ERASE_QUIET_MS: u32 = ipmc_defs::IPMC_FRU_ERASE_QUIET_MS;
/// FRU_PERSIST_DELAY_MS -- quiet time before a Write FRU Data is saved (defs.rs).
const FRU_PERSIST_DELAY_MS: u32 = ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS;
/// URGENT_LINK_WAIT_MS -- longest an urgent save before a Cold Reset waits for the link to hold
/// no accepted request and no untransmitted response before it erases anyway (defs.rs
/// IPMC_URGENT_LINK_WAIT_MS, guarded in ipmc-defs): longer than the T5 response limit of 227 ms
/// (VITA 46.11 Table 4.1.1-1), so a request accepted just before still gets its answer out.
const URGENT_LINK_WAIT_MS: u32 = ipmc_defs::IPMC_URGENT_LINK_WAIT_MS;

/// B_FITTED -- IPMB-B is built in (defs.rs IPMB_BUS_B_ENABLE).
const B_FITTED: bool = ipmc_defs::IPMB_BUS_B_ENABLE;

/// FAULT_STREAK -- consecutive bus-fault errors that make a fault persistent (IPMB_FAULT_STREAK).
const FAULT_STREAK: u32 = ipmc_defs::IPMB_FAULT_STREAK;
/// FAULT_CLEAR_MS -- both lines idle-high this long before a fault is cleared
/// (IPMB_FAULT_CLEAR_MS).
const FAULT_CLEAR_MS: u32 = ipmc_defs::IPMB_FAULT_CLEAR_MS;

/// Rx -- the frame queue end of one of the board's buses.
type Rx<B> = <<B as Board>::Ipmb as IpmbBus>::Rx;

/// Ext -- the extension the link runs: the build's own (`ipmc_ext::Active`); the simulated board
/// holds a boxed one a host test may replace (`sim::SimExt`).
#[cfg(not(feature = "sim"))]
type Ext = ipmc_ext::Active;
#[cfg(feature = "sim")]
type Ext = crate::sim::SimExt;

/// local_status_of -- the Table 10.2.2-1 code closest to a master error.
fn local_status_of(error: MasterError) -> LocalStatus {
    match error {
        MasterError::SclStuckLow => LocalStatus::ClockStuckLow,
        MasterError::SdaStuckLow => LocalStatus::DataStuckLow,
        MasterError::SclStuckHigh => LocalStatus::ClockStuckHigh,
        MasterError::SdaStuckHigh => LocalStatus::DataStuckHigh,
        MasterError::StretchTimeout => LocalStatus::ClockLowTimeout,
        _ => LocalStatus::Undiagnosed,
    }
}

/// tx_fault_of -- the capture log's cause for a master error: the one mapping from the driver's
/// errors to `TxFault`.
const fn tx_fault_of(error: MasterError) -> TxFault {
    match error {
        MasterError::AddressNack => TxFault::AddressNack,
        MasterError::DataNack => TxFault::DataNack,
        MasterError::ArbitrationLost => TxFault::ArbitrationLost,
        MasterError::BusBusy => TxFault::BusBusy,
        MasterError::StretchTimeout => TxFault::StretchTimeout,
        MasterError::SclStuckLow
        | MasterError::SdaStuckLow
        | MasterError::SclStuckHigh
        | MasterError::SdaStuckHigh => TxFault::LineStuck,
        MasterError::FrameLength => TxFault::Unspecified,
    }
}

/// is_bus_fault -- the master errors that count toward a bus fault (Rule 9.3-10: a persistent
/// local error). An address or data NACK is an absent or busy destination, not a bus fault;
/// arbitration loss is another master using the bus; an empty frame is a caller error.
const fn is_bus_fault(error: MasterError) -> bool {
    matches!(
        error,
        MasterError::StretchTimeout
            | MasterError::SclStuckLow
            | MasterError::SdaStuckLow
            | MasterError::SclStuckHigh
            | MasterError::SdaStuckHigh
            | MasterError::BusBusy
    )
}

/// failure_status -- the Table 10.2.2-1 code the link records for a bus that did not start.
fn failure_status(failure: BringUpFailure) -> LocalStatus {
    match failure {
        BringUpFailure::Bus(error) => local_status_of(error),
        BringUpFailure::Init(_) => LocalStatus::Undiagnosed,
    }
}

/// report_failure -- one line to `trace` naming why `bus` did not start.
fn report_failure(failure: BringUpFailure, bus: Bus, trace: &mut dyn Trace) {
    match failure {
        BringUpFailure::Bus(error) => trace.line(format_args!(
            "IPMB-{}: bus not usable: {}; the bus stays disabled",
            bus.name(),
            error.name()
        )),
        BringUpFailure::Init(name) => trace.line(format_args!(
            "IPMB-{}: I2C init failed: {}; the bus stays disabled",
            bus.name(),
            name
        )),
    }
}

/// announce_recovery -- the console line when the bus recovery freed a held SDA.
fn announce_recovery(recovery: Recovery, bus: Bus, con: &mut impl ConsoleText) {
    if recovery == Recovery::Recovered {
        crate::out!(
            con,
            "IPMB-{}: SDA was held low; released by the recovery clocks\n",
            bus.name()
        );
    }
}

/// LoggedTrace -- the trace sink the link hands to the core and the extension: every line goes
/// into the capture log (stamped with the clock) and, when `console` is set, to the console. The
/// one place a `CaptureTrace` over the console is built.
struct LoggedTrace<'a, C: ConsoleText, K: Clock> {
    log: &'a mut DefaultLog,
    console: bool,
    con: &'a mut C,
    clock: &'a K,
}

impl<'a, C: ConsoleText, K: Clock> LoggedTrace<'a, C, K> {
    fn new(log: &'a mut DefaultLog, console: bool, con: &'a mut C, clock: &'a K) -> Self {
        LoggedTrace {
            log,
            console,
            con,
            clock,
        }
    }

    /// of -- the sink for the IPMI trace switch of `state` (DEBUG TRACE IPMI).
    fn of(state: &'a mut LogState, con: &'a mut C, clock: &'a K) -> Self {
        let (log, console) = state.trace_parts();
        LoggedTrace::new(log, console, con, clock)
    }
}

impl<C: ConsoleText, K: Clock> Trace for LoggedTrace<'_, C, K> {
    fn line(&mut self, args: fmt::Arguments) {
        CaptureTrace {
            log: &mut *self.log,
            inner: &mut ConsoleTrace(&mut *self.con),
            console: self.console,
            now_ms: self.clock.millis(),
        }
        .line(args);
    }
}

/// FaultTracker -- the bus-fault streak and fault state of one bus. `faulted` is set by the streak
/// or by a start-up fault; both clear by the same evidence (`clear_faulted_buses`).
#[derive(Clone, Copy, Default)]
pub struct FaultTracker {
    /// Consecutive bus-fault errors; a good write or frame resets it.
    streak: u32,
    /// The streak reached IPMB_FAULT_STREAK, or the bus failed at start-up; the link records the
    /// local status.
    faulted: bool,
    /// The fault is a start-up failure: the target never ran, so the clear is a new `bring_up`,
    /// not `clear_bus_fault`.
    startup: bool,
    /// The start-up failure last reported on the console, None when none was: a retry that fails
    /// for the same reason stays silent.
    failure: Option<BringUpFailure>,
    /// Clock time since both lines read high without a break, while faulted.
    high_since: Option<u32>,
    /// Slave BERR plus TIMEOUT count at the last pass.
    seen_slave_faults: u32,
    /// Link `rx_ok` count at the last pass.
    seen_rx_ok: u32,
}

impl FaultTracker {
    /// COUNT -- one tracker per bus.
    const COUNT: usize = 2;
}

/// StartedBuses -- what `start_buses` hands the link: the board's buses, the frame queue end of
/// each bus that came up (None for a bus that is not fitted or whose bring-up failed) and the
/// local fault to record for such a bus.
pub struct StartedBuses<I: IpmbBus> {
    pub ipmb: I,
    pub rx_a: Option<I::Rx>,
    pub rx_b: Option<I::Rx>,
    /// Fault of IPMB-A found at start-up, None when it came up.
    pub fault_a: Option<BringUpFailure>,
    /// Fault of IPMB-B found at start-up, None when it came up or is not fitted.
    pub fault_b: Option<BringUpFailure>,
}

/// start_bus -- `IpmbBus::bring_up` of one bus; the line of a recovery or of a failure goes to
/// the console.
fn start_bus<I: IpmbBus>(
    ipmb: &mut I,
    bus: Bus,
    own_addr8: u8,
    con: &mut impl ConsoleText,
) -> (Option<I::Rx>, Option<BringUpFailure>) {
    match ipmb.bring_up(bus, own_addr8) {
        Ok((rx, recovery)) => {
            announce_recovery(recovery, bus, con);
            (Some(rx), None)
        }
        Err(failure) => {
            report_failure(failure, bus, &mut ConsoleTrace(&mut *con));
            (None, Some(failure))
        }
    }
}

/// start_buses -- start-up of the IPMB buses, after the strap pins (the own address is known),
/// before the prompt. Each fitted bus gets `bring_up`. A bus whose address is unusable (parity,
/// GA = 0) is left alone: the link keeps it isolated (Rule 4.1.5.4.1-5) and never starts it.
/// IPMB-B not fitted: it is not touched. A bus that fails prints one line, keeps its fault for the
/// link and stays disabled until the fault clears.
pub fn start_buses<I: IpmbBus>(
    mut ipmb: I,
    address: &AddrInfo,
    con: &mut impl ConsoleText,
) -> StartedBuses<I> {
    let (mut rx_a, mut rx_b) = (None, None);
    let (mut fault_a, mut fault_b) = (None, None);
    if address.usable() {
        (rx_a, fault_a) = start_bus(&mut ipmb, Bus::A, address.ipmb_address, con);
        if B_FITTED {
            (rx_b, fault_b) = start_bus(&mut ipmb, Bus::B, address.ipmb_address, con);
        }
    }
    StartedBuses {
        ipmb,
        rx_a,
        rx_b,
        fault_a,
        fault_b,
    }
}

/// LinkArgs -- what the board hands `IpmbLink::new`: the started buses, the FRU flash log and the
/// stored image, the kept request sequence state and the handles the link keeps.
pub struct LinkArgs<B: Board> {
    pub buses: StartedBuses<B::Ipmb>,
    pub fru_log: FruLog<B::Fru>,
    pub fru_image: Option<FruStore>,
    /// (next request sequence number, mask of rqSeq values recently sent): `kept_sequence`.
    pub kept: (u8, u64),
    pub clock: B::Clock,
    pub cycles: B::Cycles,
    pub straps: B::Straps,
    pub con: B::Console,
}

/// Restart -- a processor restart the link asks for: a Cold Reset (`boot` None) or a restart
/// into the bootloader by the interface in `boot`. The board's main loop hands it to `restart`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restart {
    pub boot: Option<Interface>,
}

/// Reply -- the response frame of a local request.
pub struct Reply {
    len: usize,
    bytes: [u8; IPMB_MAX_LEN],
}

impl Reply {
    /// frame -- the parsed response; None when it does not parse (the core built it, so never).
    fn frame(&self) -> Option<ipmb::Frame<'_>> {
        ipmb::parse(self.bytes.get(..self.len)?).ok()
    }

    /// completion_code -- the completion code of the response.
    pub fn completion_code(&self) -> u8 {
        self.frame()
            .and_then(|f| f.completion_code)
            .unwrap_or(cc::UNSPECIFIED)
    }

    /// data -- the response data after the completion code.
    pub fn data(&self) -> &[u8] {
        self.frame().map_or(&[], |f| f.data)
    }
}

/// IpmbLink -- the link layer, the dispatch context it answers requests with, and the board's
/// buses it moves frames to and from.
pub struct IpmbLink<B: Board> {
    link: Link,
    dispatch: DispatchContext,
    ipmb: B::Ipmb,
    rx_a: Option<Rx<B>>,
    rx_b: Option<Rx<B>>,
    faults: [FaultTracker; FaultTracker::COUNT],
    /// The own address in 8-bit form, for the restart of a bus after a start-up fault.
    own_addr8: u8,
    /// The FRU flash log (state and flash driver); the main loop saves through it.
    fru_log: FruLog<B::Fru>,
    /// Clock time of the latest frame taken from a bus queue (any frame: it is bus traffic);
    /// None before the first. A run-time erase waits for FRU_ERASE_QUIET_MS after it.
    last_rx_ms: Option<u32>,
    /// The extension this build runs: the Tier-2/3 extension when built in, else none.
    ext: Ext,
    /// The one decision when a pending Cold Reset runs (`service_reset` only performs it).
    reset_gate: ResetGate,
    /// The interface that asked for a restart into the bootloader (`request_bootloader`); the
    /// Cold Reset it asked for carries it in its `Restart`.
    boot_request: Option<Interface>,
    /// The application port still has output to send (set by `AppPort::poll` at the end of every
    /// pass): part of the "not idle" input of the reset gate, so a Cold Reset asked for from the
    /// application port finishes its own answer first, inside the gate's `IPMC_RESET_WAIT_MS`.
    app_output_pending: bool,
    /// A save tried while a Cold Reset was pending failed: the normal delay applies again, so a
    /// failing flash does not stall every pass until the reset wait runs out.
    reset_save_failed: bool,
    /// Clock time the urgent save (a Cold Reset is pending) first ran; None otherwise.
    urgent_since: Option<u32>,
    /// The urgent save gave up waiting for the idle link and said so (one console line).
    urgent_forced: bool,
    /// The speed each bus's master and target run at (`apply_bus_speeds`); both start at
    /// 100 kHz (Rule 4.1.1-4.3), as the link does after any restart.
    applied_speed: [BusSpeed; FaultTracker::COUNT],
    /// The capture log and the two trace switches (ipmc-log; DEBUG LOG, DUMP and TRACE).
    log: LogState,
    /// The evaluation period ended and both slaves are off: nothing resumes them.
    #[cfg(ipmc_eval)]
    ipmb_stopped: bool,
    clock: B::Clock,
    cycles: B::Cycles,
    straps: B::Straps,
    con: B::Console,
}

/// BusPause -- what an erase does to one bus's slave.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BusPause {
    /// No slave runs (bus not fitted, address unusable, or its bring-up failed): nothing to pause.
    Absent,
    /// The slave runs and the bus is healthy: wait up to PAUSE_WAIT_MS for it to go idle.
    Healthy,
    /// The slave runs but the bus is marked faulted: switch it off without waiting for idle (a
    /// stuck line keeps BUSY set, so the wait could only time out).
    Faulted,
}

/// pause_plan -- the `BusPause` of each bus from the running slaves and the fault trackers.
fn pause_plan(
    running: [bool; FaultTracker::COUNT],
    faults: &[FaultTracker; FaultTracker::COUNT],
) -> [BusPause; FaultTracker::COUNT] {
    let mut plan = [BusPause::Absent; FaultTracker::COUNT];
    for (slot, (&runs, fault)) in plan.iter_mut().zip(running.iter().zip(faults)) {
        *slot = match (runs, fault.faulted) {
            (false, _) => BusPause::Absent,
            (true, false) => BusPause::Healthy,
            (true, true) => BusPause::Faulted,
        };
    }
    plan
}

/// PausedErase -- the log flash with the IPMB slaves kept off the bus during an erase. Reads and
/// programs pass straight through (a word program stalls the core for at most 100 us).
struct PausedErase<'a, B: Board> {
    flash: &'a mut B::Fru,
    ipmb: &'a mut B::Ipmb,
    /// The cycle counter that measures the pause: the tick does not run while the core waits on
    /// flash.
    cycles: &'a B::Cycles,
    clock: &'a B::Clock,
    /// What to do to each bus's slave.
    plan: [BusPause; FaultTracker::COUNT],
    /// The two bus receive queues: a frame accepted before the slaves came off the bus waits
    /// here, and the erase is deferred rather than racing it.
    rx_a: &'a Option<Rx<B>>,
    rx_b: &'a Option<Rx<B>>,
    /// The link holds an accepted request not yet answered or a response not yet transmitted
    /// (`Link::request_in_flight`, read when this value was made; nothing in the erase call
    /// changes it): the erase is deferred.
    link_busy: bool,
    /// The last erase that ran: milliseconds the IPMB was paused and the result. The owner of
    /// the log prints it through `IpmbLink::status`.
    erase_report: Option<(u32, Result<(), FlashError>)>,
}

impl<B: Board> PausedErase<'_, B> {
    /// suspend_all -- takes every running slave off its bus. Returns the buses suspended, or
    /// Err(Timeout) after resuming those already taken when a healthy one stays busy for
    /// PAUSE_WAIT_MS. A faulted bus is suspended without the wait.
    fn suspend_all(&mut self) -> Result<[bool; FaultTracker::COUNT], FlashError> {
        let mut paused = [false; FaultTracker::COUNT];
        for bus in Bus::BOTH {
            let index = bus.index();
            match self.plan[index] {
                BusPause::Absent => continue,
                BusPause::Faulted => {
                    paused[index] = self.ipmb.suspend_faulted(bus);
                    continue;
                }
                BusPause::Healthy => {}
            }
            let start = self.clock.millis();
            while !self.ipmb.suspend(bus) {
                if self.clock.millis().wrapping_sub(start) >= PAUSE_WAIT_MS {
                    // EVIDENCE: the slave stayed busy for PAUSE_WAIT_MS; give the buses back.
                    self.resume_all(paused);
                    return Err(FlashError::Timeout);
                }
            }
            paused[index] = true;
        }
        Ok(paused)
    }

    /// traffic_waiting -- a received frame sits in a bus queue, not yet taken by `drain_rx`, or
    /// the link holds a request it has not answered or a response it has not transmitted.
    fn traffic_waiting(&self) -> bool {
        self.link_busy
            || [self.rx_a, self.rx_b]
                .into_iter()
                .any(|rx| rx.as_ref().is_some_and(|c| c.peek().is_some()))
    }

    /// resume_all -- gives the suspended buses back to their slaves.
    fn resume_all(&mut self, paused: [bool; FaultTracker::COUNT]) {
        for bus in Bus::BOTH {
            if paused[bus.index()] {
                self.ipmb.resume(bus);
            }
        }
    }
}

impl<B: Board> FlashLog for PausedErase<'_, B> {
    fn sector_len(&self) -> usize {
        self.flash.sector_len()
    }

    fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), FlashError> {
        self.flash.read(offset, out)
    }

    fn program(&mut self, offset: usize, words: &[u32]) -> Result<(), FlashError> {
        self.flash.program(offset, words)
    }

    fn millis(&self) -> u32 {
        self.flash.millis()
    }

    /// erase -- both slaves off, then a check of both receive queues and of the link
    /// (`traffic_waiting`): a frame accepted before the slaves came off the bus, or a request or
    /// response the link still holds (a request the Chassis Manager is waiting on), gives the
    /// buses back and defers the erase (`FlashError::Deferred`, retried on a later pass). Otherwise the
    /// erase, both slaves back on, and `erase_report` holds the time the board was silent
    /// (measured with the cycle counter: the tick does not run while the core waits on flash).
    fn erase(&mut self) -> Result<(), FlashError> {
        let paused = self.suspend_all()?;
        if self.traffic_waiting() {
            self.resume_all(paused);
            return Err(FlashError::Deferred);
        }
        let start = self.cycles.now();
        let result = self.flash.erase();
        let paused_ms = self.cycles.now().wrapping_sub(start) / (self.cycles.hz() / 1000).max(1);
        self.resume_all(paused);
        self.erase_report = Some((paused_ms, result));
        result
    }
}

/// RestoreError -- why FRU RESTORE did not restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The flash log is off (IPMC_FRU_PERSIST false or it did not start).
    LogOff,
    /// NVMRO is asserted: no non-volatile write (VITA 46.11 Rule 8-3, Rec 4.1.10-1).
    Nvmro,
    /// The erase the restore needs was deferred: the link holds a request or a response, or a
    /// frame is waiting (`PausedErase::traffic_waiting`). Nothing was erased; try again.
    Busy,
    /// Erasing the log failed.
    Failed(PersistError),
}

impl<B: Board> IpmbLink<B> {
    /// new -- a link in the power-up state (IPMB-A on, IPMB-B off) for the resolved address; a
    /// bus that failed its start-up gets its local fault, which disables it (Rule 9.3-10).
    pub fn new(args: LinkArgs<B>) -> Self {
        let LinkArgs {
            buses,
            fru_log,
            fru_image,
            kept: (first_request_seq, recent_request_seqs),
            clock,
            cycles,
            straps,
            con,
        } = args;
        let resolved = addr::resolve(straps.read_raw());
        // The kept values count as sent at 0 ms, the start of this boot's clock: each stays out
        // of use for a full T9 from the restart.
        let mut link = Link::with_first_request_seq(
            LinkConfig::from_defs(),
            &resolved,
            first_request_seq,
            recent_request_seqs,
            0,
        );
        let mut faults = [FaultTracker::default(); FaultTracker::COUNT];
        for (bus, fault) in [(Bus::A, buses.fault_a), (Bus::B, buses.fault_b)] {
            if let Some(failure) = fault {
                link.set_local_status(bus, failure_status(failure));
                // A start-up fault is tracked like a run-time one: the same evidence-gated
                // clear, then `bring_up` again.
                if let Some(t) = faults.get_mut(bus.index()) {
                    t.faulted = true;
                    t.startup = true;
                    t.failure = Some(failure);
                }
            }
        }
        let mut dispatch = DispatchContext::with_analog(resolved, &BOARD_ANALOG);
        if let Some(image) = fru_image {
            // The stored image replaces the built one; the self test runs on it (owner decision 8).
            dispatch.set_fru(image);
        }
        IpmbLink {
            link,
            dispatch,
            ipmb: buses.ipmb,
            rx_a: buses.rx_a,
            rx_b: buses.rx_b,
            faults,
            own_addr8: resolved.ipmb_address,
            fru_log,
            last_rx_ms: None,
            ext: Ext::default(),
            reset_gate: ResetGate::new(),
            boot_request: None,
            app_output_pending: false,
            reset_save_failed: false,
            urgent_since: None,
            urgent_forced: false,
            applied_speed: [BusSpeed::Standard; FaultTracker::COUNT],
            log: LogState::new(
                ipmc_defs::IPMC_LOG_AT_BOOT,
                ipmc_defs::IPMC_I2C_TRACE_AT_BOOT,
            ),
            #[cfg(ipmc_eval)]
            ipmb_stopped: false,
            clock,
            cycles,
            straps,
            con,
        }
    }

    /// dispatch -- the context the command handlers answer from: FRU image, SDR repository, FRU
    /// state, latest sensor snapshot. Read by the FRU and SENSORS console commands.
    pub fn dispatch(&self) -> &DispatchContext {
        &self.dispatch
    }

    /// ipmb -- the board's buses, for the counters the console prints.
    pub fn ipmb(&self) -> &B::Ipmb {
        &self.ipmb
    }

    /// fault_state -- (bus-fault streak, faulted) of `bus`, for IPMB STATS.
    pub fn fault_state(&self, bus: Bus) -> (u32, bool) {
        let t = &self.faults[bus.index()];
        (t.streak, t.faulted)
    }

    /// fru_log_status -- the flash log state for FRU STORE; None while the log is off.
    pub fn fru_log_status(&self) -> Option<Status> {
        self.fru_log.status()
    }

    /// fru_flash -- the flash of the log; None while the log is off.
    pub fn fru_flash(&self) -> Option<&B::Fru> {
        self.fru_log.flash()
    }

    /// fru_flash_mut -- the flash of the log, to set its clock or change its state in a host test.
    pub fn fru_flash_mut(&mut self) -> Option<&mut B::Fru> {
        self.fru_log.parts().map(|(_, flash)| flash)
    }

    /// reconfigure -- a link layer with `config` in place of the defs.rs one, for the board
    /// variants of the emulator (IPMB-B fitted or not, response policy, power-up behaviour). Call
    /// it right after `new`: the request sequence state and the start-up faults carry over.
    #[cfg(feature = "sim")]
    pub fn reconfigure(&mut self, config: LinkConfig) {
        let (next_seq, recent) = self.kept_state();
        let resolved = addr::resolve(self.straps.read_raw());
        let mut link = Link::with_first_request_seq(config, &resolved, next_seq, recent, 0);
        for bus in Bus::BOTH {
            let status = self.link.bus_state(bus).status;
            if status != LocalStatus::None {
                link.set_local_status(bus, status);
            }
        }
        self.link = link;
    }

    /// link_mut -- the link layer, to set its bus state in a host test or the emulator.
    #[cfg(feature = "sim")]
    pub fn link_mut(&mut self) -> &mut Link {
        &mut self.link
    }

    /// extension_mut -- the simulated board's extension holder.
    #[cfg(feature = "sim")]
    pub fn extension_mut(&mut self) -> &mut Ext {
        &mut self.ext
    }

    /// stopped -- the evaluation stop switched both slaves off (always false in a normal build).
    const fn stopped(&self) -> bool {
        #[cfg(ipmc_eval)]
        {
            self.ipmb_stopped
        }
        #[cfg(not(ipmc_eval))]
        {
            false
        }
    }

    /// erase_plan -- what an erase does to each slave: nothing after the evaluation stop (a
    /// stopped slave is never resumed), else `pause_plan`.
    fn erase_plan(&self) -> [BusPause; FaultTracker::COUNT] {
        if self.stopped() {
            return [BusPause::Absent; FaultTracker::COUNT];
        }
        pause_plan([self.rx_a.is_some(), self.rx_b.is_some()], &self.faults)
    }

    /// fru_restore -- FRU RESTORE: saves the built image as a new record with the next sequence
    /// number, at once (`Persist::restore`; an erase it needs pauses the IPMB as for any save), and
    /// puts the built image in the store, rerunning the self test. A power cut at any point loads
    /// the previous newest image or the built one. Refused while NVMRO is asserted (the live pin,
    /// and again by the flash driver before each erase and program); Err(LogOff) while the log
    /// is off (the store is then RAM only already).
    pub fn fru_restore(&mut self) -> Result<(), RestoreError> {
        if self.straps.nvmro_asserted() {
            return Err(RestoreError::Nvmro);
        }
        let plan = self.erase_plan();
        let link_busy = self.link.request_in_flight();
        let (persist, flash) = self.fru_log.parts().ok_or(RestoreError::LogOff)?;
        let mut paused = PausedErase::<B> {
            flash,
            ipmb: &mut self.ipmb,
            cycles: &self.cycles,
            clock: &self.clock,
            plan,
            rx_a: &self.rx_a,
            rx_b: &self.rx_b,
            link_busy,
            erase_report: None,
        };
        // The built image replaces the FRU content; the Tier-3 settings byte is not FRU content
        // and stays as it is.
        let mut built = FruStore::new();
        built.load_nv_settings(self.dispatch.nv_settings());
        let restored = persist.restore(&mut paused, built.image(), built.nv_settings());
        let report = paused.erase_report;
        self.erase_line(report);
        match restored {
            Ok(_) => {}
            Err(PersistError::Flash(FlashError::NvmroAsserted)) => return Err(RestoreError::Nvmro),
            Err(PersistError::Flash(FlashError::Deferred)) => return Err(RestoreError::Busy),
            Err(error) => return Err(RestoreError::Failed(error)),
        }
        self.dispatch.set_fru(built);
        Ok(())
    }

    /// bus_quiet -- true when a run-time erase may start: no response waits in the egress queue
    /// and no frame was taken from a bus queue for FRU_ERASE_QUIET_MS.
    fn bus_quiet(&self, now_ms: u32) -> bool {
        self.link.egress_empty()
            && self
                .last_rx_ms
                .is_none_or(|t| now_ms.wrapping_sub(t) >= FRU_ERASE_QUIET_MS)
    }

    /// fru_unsaved -- an accepted Write FRU Data is not in flash yet (the store's dirty mark or
    /// the log's pending save); false while the log is off (the store is RAM only then).
    pub fn fru_unsaved(&self) -> bool {
        self.fru_log
            .status()
            .is_some_and(|s| s.pending || self.dispatch.fru.is_dirty())
    }

    /// set_unit_field -- the application port's write of one per-unit FRU field. Refused while
    /// the live NVMRO pin is asserted (VITA 46.11 Rule 8-3, as Write FRU Data and FRU RESTORE
    /// are); otherwise the dispatch context edits the RAM image and reruns the self test. The
    /// store is marked dirty, so `service_fru_log` saves the image after the usual delay: the
    /// one save path of every FRU change.
    pub fn set_unit_field(&mut self, field: FruField, value: &str) -> Result<(), FieldError> {
        if self.straps.nvmro_asserted() {
            return Err(FieldError::Nvmro);
        }
        self.dispatch.set_unit_field(field, value)
    }

    /// service_fru_log -- the save step of the main loop (`Persist::poll`): one console line per
    /// save or failed save. Nothing runs while the log is off. While a Cold Reset is pending the
    /// save runs at once, with no persist delay and no quiet-bus wait: the IPMB is about to go
    /// away, and the one erase it may need is bounded (a failed attempt returns to the normal
    /// delay).
    pub fn service_fru_log(&mut self) {
        let plan = self.erase_plan();
        let now_ms = self.clock.millis();
        let reset_pending = self.dispatch.pending_reset.is_some();
        let urgent = reset_pending && !self.reset_save_failed;
        // An urgent save waits URGENT_LINK_WAIT_MS for the link to go idle before an erase; the
        // main loop keeps servicing the link meanwhile (the erase defers, `PausedErase`).
        let urgent_since = if urgent {
            *self.urgent_since.get_or_insert(now_ms)
        } else {
            self.urgent_since = None;
            self.urgent_forced = false;
            now_ms
        };
        let in_flight = self.link.request_in_flight();
        let link_wait_over = urgent && now_ms.wrapping_sub(urgent_since) >= URGENT_LINK_WAIT_MS;
        let link_busy = in_flight && !link_wait_over;
        if link_wait_over && in_flight && !self.urgent_forced {
            self.urgent_forced = true;
            self.status(format_args!(
                "FRU log: urgent save erases with the link still busy after {URGENT_LINK_WAIT_MS} ms"
            ));
        }
        let quiet = urgent || self.bus_quiet(now_ms);
        let delay_ms = if urgent { 0 } else { FRU_PERSIST_DELAY_MS };
        let Some((persist, flash)) = self.fru_log.parts() else {
            return;
        };
        let mut paused = PausedErase::<B> {
            flash,
            ipmb: &mut self.ipmb,
            cycles: &self.cycles,
            clock: &self.clock,
            plan,
            rx_a: &self.rx_a,
            rx_b: &self.rx_b,
            link_busy,
            erase_report: None,
        };
        let nvmro = self.dispatch.addr.nvmro_asserted;
        let outcome = persist.poll(
            &mut paused,
            &mut self.dispatch.fru,
            now_ms,
            nvmro,
            delay_ms,
            quiet,
        );
        let report = paused.erase_report;
        self.erase_line(report);
        // An attempt made while a reset is pending that fails falls back to the normal delay and
        // quiet-bus rule; the flag clears when no reset is pending.
        self.reset_save_failed =
            reset_pending && (self.reset_save_failed || matches!(outcome, Some(Err(_))));
        match outcome {
            Some(Ok(saved)) => self.status(format_args!(
                "FRU log: saved record {} at offset {}{}",
                saved.seq,
                saved.offset,
                if saved.erased {
                    " (sector erased first)"
                } else {
                    ""
                }
            )),
            Some(Err(error)) => self.status(format_args!("FRU log: save failed: {}", error.name())),
            None => {}
        }
    }

    /// erase_line -- the status line of a run-time erase (`PausedErase::erase_report`); nothing
    /// for an erase that did not run (a deferred one, or none).
    fn erase_line(&mut self, report: Option<(u32, Result<(), FlashError>)>) {
        match report {
            Some((paused_ms, Ok(()))) => self.status(format_args!(
                "FRU log: erased the store sector at run time (IPMB paused {} ms)",
                paused_ms
            )),
            Some((paused_ms, Err(error))) => self.status(format_args!(
                "FRU log: erase of the store sector failed at run time: {} (IPMB paused {} ms)",
                error.name(),
                paused_ms
            )),
            None => {}
        }
    }

    /// link -- the link layer, for the console views of the bus state.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// feed_sensors -- hands the latest sensor sample to the command handlers and steps the FRU
    /// state machine with it (owner decision 3). The main loop calls it once per sample; a state
    /// change prints one console line (`Context::update_sensors` writes it). The extension's
    /// `on_sample` hook runs after it (`ext::feed_sensors`).
    pub fn feed_sensors(&mut self, snapshot: &SensorSnapshot) {
        let now_ms = self.clock.millis();
        let mut trace = LoggedTrace::of(&mut self.log, &mut self.con, &self.clock);
        ext::feed_sensors(
            &mut self.dispatch,
            &mut self.ext,
            now_ms,
            snapshot,
            &mut trace,
        );
    }

    /// status -- one status line: it prints on the console whatever the IPMI console switch says
    /// and goes into the capture log.
    fn status(&mut self, args: fmt::Arguments) {
        self.logged(true, args);
    }

    /// logged -- one line into the capture log, and to the console when `console` is set.
    fn logged(&mut self, console: bool, args: fmt::Arguments) {
        LoggedTrace::new(self.log.capture_mut(), console, &mut self.con, &self.clock).line(args);
    }

    /// power_on_check -- whether payload power may come on now: the one combined check of every
    /// power-on path (`ext::check_power_on`: the FRU state machine and the extension).
    pub fn power_on_check(&self) -> Result<(), PowerOnRefused> {
        ext::check_power_on(&self.dispatch, &self.ext)
    }

    /// take_hand_offs -- the requests a handler left for the owner of the payload power and of
    /// the payload reset output (`Context::take_hand_offs`), cleared, with the power-on check the
    /// request meets.
    pub fn take_hand_offs(&mut self) -> HandOffs {
        self.dispatch.take_hand_offs(&self.ext)
    }

    /// service -- main-loop step.
    /// 1. Samples NVMRO (one console line when it changes). The GA bits are not re-resolved: the
    ///    address is fixed after start-up.
    /// 2. Drains the receive queues into the link in STOP-time order.
    /// 3. Runs `Link::poll`.
    /// 4. Sends at most one queued response through the board (`transmit`).
    /// 5. Saves the FRU image to the flash log when a Write FRU Data has been quiet long enough
    ///    (a save that must erase a sector also waits for a quiet bus, `bus_quiet`).
    /// 6. Restarts the processor (Cold Reset) or the link state (Warm Reset) when a handler left
    ///    one in `pending_reset` and its response has left (`service_reset`). A Cold Reset ends
    ///    the pass at once: the `Restart` returned is the board's to perform (`restart`).
    /// 7. Applies a speed the link set to a bus (`apply_bus_speeds`), between transfers.
    ///
    /// The bus-fault bookkeeping runs around step 4: `track_slave_faults` before it (frames
    /// received in this pass reset the streak, the writes then add to it) and
    /// `clear_faulted_buses` after it.
    pub fn service(&mut self) -> Option<Restart> {
        self.refresh_nvmro();
        self.drain_rx();
        let now_ms = self.clock.millis();
        let mut trace = LoggedTrace::of(&mut self.log, &mut self.con, &self.clock);
        self.link
            .poll(&mut self.dispatch, &mut self.ext, now_ms, &mut trace);
        self.track_slave_faults();
        self.transmit();
        self.clear_faulted_buses();
        self.service_fru_log();
        if let Some(restart) = self.service_reset() {
            return Some(restart);
        }
        self.apply_bus_speeds();
        None
    }

    /// kept_state -- (next request sequence number, mask of the rqSeq values sent inside T9): what
    /// `keep_sequence` stores before a restart.
    pub fn kept_state(&self) -> (u8, u64) {
        (
            self.link.next_request_seq(),
            self.link.recent_request_seqs(self.clock.millis()),
        )
    }

    /// refresh_nvmro -- samples the NVMRO pin into the dispatch context (the one reader of the
    /// pin for IPMI decisions); one console line when it changes. `service` and
    /// `local_request` call it, so a request from any source sees the live level.
    fn refresh_nvmro(&mut self) {
        let nvmro = self.straps.nvmro_asserted();
        if nvmro != self.dispatch.addr.nvmro_asserted {
            self.dispatch.addr.nvmro_asserted = nvmro;
            LoggedTrace::of(&mut self.log, &mut self.con, &self.clock).line(format_args!(
                "NVMRO: {}",
                if nvmro { "asserted" } else { "deasserted" }
            ));
        }
    }

    /// apply_bus_speeds -- gives each running bus the speed the link holds for it (Set IPMB State
    /// speed byte, Rule 4.1.1-4.4): the master timing and the target change together
    /// (`IpmbBus::set_speed`), once the slave is idle, so a change never lands inside a
    /// transfer. A bus that is busy or has no running slave is asked again on the next pass; a
    /// change that lands prints one console line.
    fn apply_bus_speeds(&mut self) {
        for bus in Bus::BOTH {
            let wanted = self.link.bus_state(bus).speed;
            let index = bus.index();
            let running = match bus {
                Bus::A => self.rx_a.is_some(),
                Bus::B => self.rx_b.is_some(),
            };
            if !running || self.applied_speed[index] == wanted {
                continue;
            }
            if self.ipmb.set_speed(bus, wanted) {
                self.applied_speed[index] = wanted;
                self.status(format_args!(
                    "IPMB-{}: running at {}",
                    bus.name(),
                    wanted.name()
                ));
            }
        }
    }

    /// stop_for_eval -- the end of the evaluation period: switches both IPMB slaves off and drops
    /// what is queued. Each running slave goes through the board's own suspend (waiting up to
    /// PAUSE_WAIT_MS for an idle bus, as an erase pause does); a faulted bus, or one that stays
    /// busy past the wait, goes through `suspend_faulted`. The pins stay as the suspended state
    /// leaves them (released, open drain) and nothing resumes the slaves. The main loop never
    /// calls `service` afterwards.
    #[cfg(ipmc_eval)]
    pub fn stop_for_eval(&mut self) {
        let plan = pause_plan([self.rx_a.is_some(), self.rx_b.is_some()], &self.faults);
        for bus in Bus::BOTH {
            let off = match plan[bus.index()] {
                BusPause::Absent => continue,
                BusPause::Faulted => false,
                BusPause::Healthy => {
                    let start = self.clock.millis();
                    let mut idle = false;
                    while !idle && self.clock.millis().wrapping_sub(start) < PAUSE_WAIT_MS {
                        idle = self.ipmb.suspend(bus);
                    }
                    idle
                }
            };
            if !off {
                let _ = self.ipmb.suspend_faulted(bus);
            }
        }
        for consumer in [self.rx_a.as_mut(), self.rx_b.as_mut()]
            .into_iter()
            .flatten()
        {
            while consumer.dequeue().is_some() {}
        }
        self.ipmb_stopped = true;
    }

    /// set_app_output_pending -- the application port tells whether it still has output to send
    /// (`AppPort::poll`, end of every pass); a pending Cold Reset waits for it.
    pub fn set_app_output_pending(&mut self, pending: bool) {
        self.app_output_pending = pending;
    }

    /// request_bootloader -- a restart into the bootloader, asked for by `interface` (the console
    /// command `DEBUG BOOTLOADER`, the application port command `BOOTLOADER`): a Cold Reset
    /// through the one reset path (`service_reset`: the answer has left, an accepted FRU write is
    /// saved, or IPMC_RESET_WAIT_MS passed), whose `Restart` carries the interface.
    pub fn request_bootloader(&mut self, interface: Interface) {
        self.boot_request = Some(interface);
        self.dispatch.pending_reset = Some(ResetRequest::Cold);
    }

    /// service_reset -- performs `Context::pending_reset` when `ResetGate` says so (the response
    /// left and no accepted FRU write is unsaved, or IPMC_RESET_WAIT_MS passed).
    /// - Cold: one console line, then the `Restart` for the board to perform (`restart`).
    /// - Warm: `warm_reset`, no processor reset.
    fn service_reset(&mut self) -> Option<Restart> {
        let now_ms = self.clock.millis();
        let request = self.dispatch.pending_reset;
        // Nothing waits to leave: no IPMB response in the egress queue and no application port
        // output in its buffer or TX ring.
        let egress_empty = self.link.egress_empty() && !self.app_output_pending;
        let fru_clean = !self.fru_unsaved();
        let mut trace = LoggedTrace::of(&mut self.log, &mut self.con, &self.clock);
        if !self.reset_gate.poll(
            request.is_some(),
            now_ms,
            egress_empty,
            fru_clean,
            &mut trace,
        ) {
            return None;
        }
        match request {
            Some(ResetRequest::Cold) => {
                let target = if self.boot_request.is_some() {
                    " into the bootloader"
                } else {
                    ""
                };
                self.status(format_args!("Cold Reset: restarting{target}"));
                Some(Restart {
                    boot: self.boot_request,
                })
            }
            Some(ResetRequest::Warm) => {
                // A bootloader request never outlives the reset it rode on.
                self.boot_request = None;
                self.warm_reset(now_ms);
                None
            }
            None => None,
        }
    }

    /// warm_reset -- the Warm Reset (`ResetRequest::Warm`): a new Link in the power-up bus state
    /// with the same configuration and address, the request sequence state carried over
    /// (`Link::restarted`), the speeds back at 100 kHz in the drivers (`apply_bus_speeds` does
    /// it on the next pass), `Context::warm_reset` clearing the hand-offs, then the extension.
    /// The start-up faults of a bus stay (`set_local_status` again), so a bus that failed at
    /// start-up stays disabled. Kept: the FRU store, the SDR repository, the self test, the FRU
    /// state, the sensors and payload power, the settings byte, the payload mode and the event
    /// enables.
    fn warm_reset(&mut self, now_ms: u32) {
        self.link.warm_reset(
            &mut self.dispatch,
            &mut self.ext,
            &mut self.reset_gate,
            now_ms,
            |old, new| {
                for bus in Bus::BOTH {
                    let status = old.bus_state(bus).status;
                    if status != LocalStatus::None {
                        new.set_local_status(bus, status);
                    }
                }
            },
        );
        self.status(format_args!("Warm Reset: link state re-initialised"));
    }

    /// tracker -- the fault tracker of `bus`.
    fn tracker(&mut self, bus: Bus) -> &mut FaultTracker {
        &mut self.faults[bus.index()]
    }

    /// note_faults -- adds `count` bus-fault errors of `status` and `name` to the streak of
    /// `bus`. At IPMB_FAULT_STREAK the bus is marked faulted: the local status goes to the link
    /// (which disables the bus, Rule 9.3-10) and one console line names the cause. A bus that
    /// is not fitted, or already carries a local fault, is left alone.
    fn note_faults(&mut self, bus: Bus, count: u32, status: LocalStatus, name: &str) {
        let state = self.link.bus_state(bus);
        let t = self.tracker(bus);
        if !state.fitted || t.faulted || state.status != LocalStatus::None {
            return;
        }
        t.streak = t.streak.saturating_add(count);
        if t.streak >= FAULT_STREAK {
            t.faulted = true;
            t.high_since = None;
            self.link.set_local_status(bus, status);
            self.status(format_args!("IPMB-{} fault: {}", bus.name(), name));
        }
    }

    /// on_write_result -- books a master write on the streak: success resets it, a bus-fault
    /// error adds one; NACKs and the other errors leave it as it is.
    fn on_write_result(&mut self, bus: Bus, result: Result<(), MasterError>) {
        match result {
            Ok(()) => self.tracker(bus).streak = 0,
            Err(error) if is_bus_fault(error) => {
                self.note_faults(bus, 1, local_status_of(error), error.name());
            }
            Err(_) => {}
        }
    }

    /// track_slave_faults -- the slave side of the streak, no waiting: new BERR or TIMEOUT
    /// counts of the driver add to the streak of each fitted bus; a new good frame at the link
    /// resets it (the reset wins when both arrive in one pass).
    fn track_slave_faults(&mut self) {
        for bus in Bus::BOTH {
            if !self.link.bus_state(bus).fitted {
                continue;
            }
            let d = self.ipmb.slave_stats(bus);
            let slave_faults = d.bus_error.wrapping_add(d.timeout);
            let rx_ok = self.link.counters(bus).rx_ok;
            let t = self.tracker(bus);
            let new_faults = slave_faults.wrapping_sub(t.seen_slave_faults);
            let good_frame = rx_ok != t.seen_rx_ok;
            t.seen_slave_faults = slave_faults;
            t.seen_rx_ok = rx_ok;
            if good_frame {
                t.streak = 0;
            } else if new_faults > 0 {
                self.note_faults(bus, new_faults, LocalStatus::Undiagnosed, "slave bus error");
            }
        }
    }

    /// clear_faulted_buses -- the evidence-gated clear, one sample per pass, no waiting. A
    /// faulted bus whose two lines have read high without a break for IPMB_FAULT_CLEAR_MS gets
    /// `clear_bus_fault` (the BUSY workaround and a slave re-arm), or, after a start-up fault,
    /// `restart_bus` (`bring_up` again); on success the local status returns to None and one
    /// console line says so; a failure is reported and the wait starts again.
    fn clear_faulted_buses(&mut self) {
        let now_ms = self.clock.millis();
        for bus in Bus::BOTH {
            if !self.faults[bus.index()].faulted {
                continue;
            }
            if !self.ipmb.lines_high(bus) {
                self.tracker(bus).high_since = None;
                continue;
            }
            let since = *self.tracker(bus).high_since.get_or_insert(now_ms);
            if now_ms.wrapping_sub(since) < FAULT_CLEAR_MS {
                continue;
            }
            let cleared = if self.faults[bus.index()].startup {
                // `restart_bus` reports a new failure reason.
                self.restart_bus(bus)
            } else {
                match self.ipmb.clear_bus_fault(bus) {
                    Ok(()) => true,
                    Err(error) => {
                        self.status(format_args!(
                            "IPMB-{} fault clear failed: {}",
                            bus.name(),
                            error.name()
                        ));
                        false
                    }
                }
            };
            if cleared {
                let d = self.ipmb.slave_stats(bus);
                self.link.set_local_status(bus, LocalStatus::None);
                *self.tracker(bus) = FaultTracker {
                    seen_slave_faults: d.bus_error.wrapping_add(d.timeout),
                    seen_rx_ok: self.link.counters(bus).rx_ok,
                    ..FaultTracker::default()
                };
                self.status(format_args!("IPMB-{} fault cleared", bus.name()));
            } else {
                self.tracker(bus).high_since = None;
            }
        }
    }

    /// restart_bus -- runs `bring_up` again for a bus that failed at start-up; true when the
    /// target now runs (its frame queue joins the drain).
    fn restart_bus(&mut self, bus: Bus) -> bool {
        match self.ipmb.bring_up(bus, self.own_addr8) {
            Ok((rx, recovery)) => {
                announce_recovery(recovery, bus, &mut self.con);
                match bus {
                    Bus::A => self.rx_a = Some(rx),
                    Bus::B => self.rx_b = Some(rx),
                }
                true
            }
            Err(failure) => {
                // Reported when it first fails and again only when the reason changes; the
                // local status follows the latest reason.
                if self.tracker(bus).failure != Some(failure) {
                    self.tracker(bus).failure = Some(failure);
                    report_failure(
                        failure,
                        bus,
                        &mut LoggedTrace::new(
                            self.log.capture_mut(),
                            true,
                            &mut self.con,
                            &self.clock,
                        ),
                    );
                    self.link.set_local_status(bus, failure_status(failure));
                }
                false
            }
        }
    }

    /// drain_rx -- moves frames from the two queues into `Link::on_frame`, the one with the
    /// earlier STOP time first (bus A on a tie), so the link sees arrival order across the buses
    /// (PLAN 3.3). Bounded by RX_DRAIN_MAX. A frame the link drops is counted by the link.
    fn drain_rx(&mut self) {
        for _ in 0..RX_DRAIN_MAX {
            let stop_a = self.rx_a.as_ref().and_then(|c| c.peek()).map(|f| f.stop_ms);
            let stop_b = self.rx_b.as_ref().and_then(|c| c.peek()).map(|f| f.stop_ms);
            let bus = match (stop_a, stop_b) {
                (None, None) => break,
                (Some(_), None) => Bus::A,
                (None, Some(_)) => Bus::B,
                // Wrapping difference: B first only when its STOP is earlier.
                (Some(a), Some(b)) if clock::earlier(b, a) => Bus::B,
                (Some(_), Some(_)) => Bus::A,
            };
            let consumer = match bus {
                Bus::A => self.rx_a.as_mut(),
                Bus::B => self.rx_b.as_mut(),
            };
            if let Some(frame) = consumer.and_then(|c| c.dequeue()) {
                self.last_rx_ms = Some(frame.stop_ms);
                self.record_frame(
                    frame.stop_ms,
                    bus,
                    Dir::In,
                    FrameStatus::Received,
                    frame.data(),
                );
                let _ = self.link.on_frame(bus, frame.data(), frame.stop_ms);
            }
        }
    }

    /// transmit -- sends at most one queued response: copies the frame out of the link, writes
    /// it through the board and reports the outcome with `on_tx_result` (the link resends on
    /// the other bus after a failure). One write per `service` pass, so the receive drain and
    /// the link poll run between writes. A pass blocks for at most one write budget (T2 + T1 =
    /// 80 ms at the T2 minimum), inside T5 (227 ms, VITA 46.11 Rule 4.1.1-11); the main loop
    /// calls `service` on every pass (a 1 ms tick wakes it), so the next queued frame follows
    /// within one tick plus the console and sensor steps.
    fn transmit(&mut self) {
        let mut buf = [0u8; IPMB_MAX_LEN];
        let Some((bus, frame)) = self.link.next_tx() else {
            return;
        };
        let Some(out) = buf.get_mut(..frame.len()) else {
            return; // the egress queue only holds frames up to IPMB_MAX_LEN
        };
        out.copy_from_slice(frame);
        let result = self.ipmb.write(bus, out);
        let now_ms = self.clock.millis();
        let outcome = match result {
            Ok(()) => TxOutcome::Sent,
            Err(error) => TxOutcome::Failed(tx_fault_of(error)),
        };
        let status = match outcome {
            TxOutcome::Sent => FrameStatus::Sent,
            TxOutcome::Failed(fault) => FrameStatus::Failed(fault),
        };
        self.record_frame(now_ms, bus, Dir::Out, status, out);
        self.on_write_result(bus, result);
        let mut trace = LoggedTrace::of(&mut self.log, &mut self.con, &self.clock);
        let _ = self.link.on_tx_result(bus, outcome, now_ms, &mut trace);
    }

    /// record_frame -- one whole frame, in or out, into the capture log and, when the I2C trace
    /// is on, as one console line (the same `TraceRow` the DEBUG DUMP TRACE prints). The line
    /// shows even while the log is off.
    fn record_frame(
        &mut self,
        time_ms: u32,
        bus: Bus,
        dir: Dir,
        status: FrameStatus,
        bytes: &[u8],
    ) {
        let index = self.log.capture().stats().total;
        self.log
            .capture_mut()
            .frame(time_ms, bus, dir, status, bytes);
        if self.log.i2c_trace() {
            let record = Record {
                index,
                time_ms,
                kind: Kind::Frame {
                    bus,
                    dir,
                    status,
                    bytes,
                },
            };
            // Never waits: a row that does not fit the console TX ring whole is dropped and
            // counted (`tx_lines_dropped`); the frame is in the capture log either way.
            self.con
                .row_or_drop(format_args!("{}", TraceRow(&record, &CoreNames(&self.ext))));
        }
    }

    // ---- the DEBUG console group ----

    /// log -- the capture log and the trace switches.
    pub const fn log(&self) -> &LogState {
        &self.log
    }

    /// log_mut -- the capture log and the trace switches, to change them.
    pub fn log_mut(&mut self) -> &mut LogState {
        &mut self.log
    }

    /// names -- the command names of this build, for the capture printers.
    pub fn names(&self) -> CoreNames<'_> {
        CoreNames(&self.ext)
    }

    /// log_text -- a line for the capture log only (console requests and responses).
    pub fn log_text(&mut self, args: fmt::Arguments) {
        self.logged(false, args);
    }

    /// local_request -- runs one request locally, as from the Chassis Manager (`Link::local_request`):
    /// nothing goes on the bus. The response frame comes back as (length, bytes). The dispatcher's
    /// trace line goes into the capture log only: the console command prints the answer itself.
    pub fn local_request(
        &mut self,
        netfn: u8,
        cmd: u8,
        data: &[u8],
    ) -> Result<(usize, [u8; IPMB_MAX_LEN]), LocalError> {
        // The live NVMRO level, as `service` reads it: the handler of a local request (APP_UART
        // IPMI, DEBUG IPMI, the start-up events-off request) decides on it like a bus request.
        self.refresh_nvmro();
        let now_ms = self.clock.millis();
        let mut trace = LoggedTrace::new(self.log.capture_mut(), false, &mut self.con, &self.clock);
        self.link.local_request(
            &mut self.dispatch,
            &mut self.ext,
            netfn,
            cmd,
            data,
            now_ms,
            &mut trace,
        )
    }

    /// local -- `local_request` with the response parsed (`Reply`).
    pub fn local(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<Reply, LocalError> {
        let (len, bytes) = self.local_request(netfn, cmd, data)?;
        Ok(Reply { len, bytes })
    }

    /// inject -- hands a whole frame to the link as if `bus` had received it: logged as an
    /// injected In frame, then `Link::on_frame`. Err(reason) is the drop; a request that is
    /// accepted is answered on the bus by the next `service` passes.
    pub fn inject(&mut self, bus: Bus, bytes: &[u8]) -> Result<(), FrameDrop> {
        let now_ms = self.clock.millis();
        self.last_rx_ms = Some(now_ms);
        self.record_frame(now_ms, bus, Dir::In, FrameStatus::Injected, bytes);
        self.link.on_frame(bus, bytes, now_ms)
    }

    /// clear_duplicate_cache -- empties the duplicate cache (DEBUG CACHE CLEAR).
    pub fn clear_duplicate_cache(&mut self) {
        self.link.clear_duplicate_cache();
    }
}

/// restart -- the one processor restart (Cold Reset, bootloader request and SYS RESET).
/// 1. Flush both ports. The gate waited for the application port output to leave
///    (`app_output_pending`, bounded by IPMC_RESET_WAIT_MS); this flush only covers the last
///    byte in the shift register. It is bounded by the port; with CTS held off the bound
///    runs out (counted as a TX timeout) and the reset goes ahead.
/// 2. Keep the requester's next sequence number and the recent mask (`keep_sequence`).
/// 3. Write the boot request words for `r.boot`, none for `None`.
/// 4. `SystemControl::system_reset`.
pub fn restart<P: Platform>(
    link: &IpmbLink<P>,
    con: &mut P::Console,
    app: &mut P::App,
    kept: &mut P::KeptSeq,
    req: &mut P::BootRequest,
    sys: &mut P::System,
    r: Restart,
) -> ! {
    con.flush();
    app.flush();
    let (next_seq, recent) = link.kept_state();
    keep_sequence(kept, next_seq, recent);
    if let Some(interface) = r.boot {
        req.write(request::encode(interface));
    }
    sys.system_reset()
}
