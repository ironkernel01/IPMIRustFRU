// ipmb_link.rs - firmware composition of the IPMB link layer and the IPMB STATS and IPMB STATE
// console commands.
//
// Owns the ipmc_core::link::Link value (in a static, handed out once by `IpmbLink::take`), the
// consumer ends of the per-bus frame queues and the IPMB master, and runs the main-loop step.
// The behaviour (ingress, duplicate cache, bus choice, resend, bus state) lives in ipmc-core;
// this file moves values between it, the drivers and the console. Each bus's I2C interrupt
// (bsp/i2c_hw.rs) posts its frames to its queue; `service` drains both queues in STOP-time order
// into `Link::on_frame`, runs `Link::poll`, then sends each queued response through the master
// and reports the outcome with `on_tx_result` (PLAN 3.3a). Nothing here runs in an interrupt
// handler.
//
// The capture log (ipmc_core::capture) records every whole IPMB frame taken from a driver queue
// (before the parser) and every frame handed to the master, with its outcome, and the trace and
// status lines. Every trace sink handed to the core or the extension is a `LoggedTrace`: it
// records the line and prints it on the console unless the IPMI console trace is switched off.
// The DEBUG console group (debug_cmds.rs) reads and switches all of this through the methods at
// the end of `impl IpmbLink`.
//
// The FRU flash log (ipmc_fru::persist) is saved from `service`, once a Write FRU Data has been
// quiet for IPMC_FRU_PERSIST_DELAY_MS. A save that has to erase the store sector first (once per
// 60 records, when the 16 KB sector is full) takes both IPMB slaves off the bus (`PausedErase`):
// the core stalls on flash during an erase, so the board stays silent for that time (at most
// 500 ms for the 16 KB sector, DS10086 Table 45; the console line "IPMB paused <n> ms" reports
// the measured time) instead of stretching SCL in the middle of a frame.

use core::fmt;

use cortex_m::peripheral::{DCB, DWT, NVIC};
use ipmc_core::addr::{self, AddrInfo};
use ipmc_core::capture::{
    CaptureTrace, CoreNames, DefaultLog, Dir, FrameStatus, Kind, Record, TraceRow, TxFault,
};
use ipmc_core::dispatch::{Context as DispatchContext, PowerRequest, ResetRequest};
use ipmc_core::ext::{self, Extension};
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::link::{BusSpeed, FrameDrop, Link, LinkConfig, LocalError, TxOutcome};
use ipmc_core::queue::Bus;
use ipmc_core::reset_gate::ResetGate;
use ipmc_core::sensor::{self, LocalStatus, SensorSnapshot};
use ipmc_core::trace::Trace;
use ipmc_fru::persist::{FlashError, FlashLog, PersistError, Status};
use ipmc_fru::{FieldError, FruStore, UnitField};
use ipmc_menu::{Args, CmdResult};
use stm32f4::stm32f401::{I2C1, I2C3, RCC};

use crate::bsp::flash::Flash;
use crate::bsp::i2c_hw::{self, I2cError, I2cPeripheral, RxConsumer};
use crate::bsp::ipmb_common;
use crate::bsp::ipmb_master::{self, IpmbMaster, MasterError, Recovery, SelectedMaster};
use crate::bsp::ipmb_pins::IpmbPins;
use crate::bsp::{fault, ga, reset_cause, systick, uart};
use crate::console::{ConsoleTrace, Context};
use crate::fru_log::FruLog;

/// RX_DRAIN_MAX -- most frames one `service` pass takes from the queues: both queues full at
/// once (2 x IPMB_RX_QUEUE_DEPTH). Frames that arrive during the pass wait for the next one.
const RX_DRAIN_MAX: usize = 2 * (ipmc_defs::IPMB_RX_QUEUE_DEPTH as usize);

/// PAUSE_WAIT_MS -- longest wait for a healthy slave to go idle before an erase (ipmi_defs.h
/// IPMC_FRU_PAUSE_WAIT_MS, guarded in ipmc-defs): a frame in progress ends within T1 = 20 ms
/// (VITA 46.11 Table 4.1.1-1). A slave that is still busy makes the erase fail with `Timeout`; the
/// save is retried one delay later.
const PAUSE_WAIT_MS: u32 = ipmc_defs::IPMC_FRU_PAUSE_WAIT_MS;
/// FRU_ERASE_QUIET_MS -- a run-time erase starts only after this long without an IPMB frame
/// (ipmi_defs.h IPMC_FRU_ERASE_QUIET_MS).
const FRU_ERASE_QUIET_MS: u32 = ipmc_defs::IPMC_FRU_ERASE_QUIET_MS;
/// FRU_PERSIST_DELAY_MS -- quiet time before a Write FRU Data is saved (ipmi_defs.h).
const FRU_PERSIST_DELAY_MS: u32 = ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS;

/// IpmbIo -- what the start-up code hands the link: the master, the consumer end of each bus's
/// frame queue (None for a bus that is not fitted or whose I2C init failed) and the local fault
/// to record for such a bus.
pub struct IpmbIo {
    pub master: SelectedMaster,
    pub rx_a: Option<RxConsumer>,
    pub rx_b: Option<RxConsumer>,
    /// Fault of IPMB-A found at start-up, None when it came up.
    pub fault_a: Option<BringUpFailure>,
    /// Fault of IPMB-B found at start-up, None when it came up or is not fitted.
    pub fault_b: Option<BringUpFailure>,
    /// What `bring_up` needs to start a bus again after its start-up fault clears.
    pub bring_up: BringUp,
    /// The I2C peripherals of the buses whose bring-up failed (None: running, or never tried).
    pub periph: [Option<I2cPeripheral>; FaultTracker::COUNT],
}

/// B_FITTED -- IPMB-B is built in (ipmi_defs.h IPMB_BUS_B_ENABLE).
const B_FITTED: bool = ipmc_defs::IPMB_BUS_B_ENABLE == 1;

/// BringUp -- what `bring_up` takes besides the bus: the clock and interrupt controllers, the PCLK1
/// the I2C timing derives from and the own address. Owned by the link after start-up, so a bus
/// that failed there can be brought up again.
pub struct BringUp {
    rcc: RCC,
    nvic: NVIC,
    pclk1_hz: u32,
    own_addr8: u8,
}

/// DriverParts -- the shared peripherals and clocks `start_drivers` uses.
pub struct DriverParts<'a> {
    pub rcc: RCC,
    pub nvic: NVIC,
    pub dcb: &'a mut DCB,
    pub dwt: &'a mut DWT,
    /// Core clock (HCLK) in Hz: the DWT cycle counter runs at it.
    pub hclk_hz: u32,
    pub pclk1_hz: u32,
}

/// FAULT_STREAK -- consecutive bus-fault errors that make a fault persistent (IPMB_FAULT_STREAK).
const FAULT_STREAK: u32 = ipmc_defs::IPMB_FAULT_STREAK;
/// FAULT_CLEAR_MS -- both lines idle-high this long before a fault is cleared
/// (IPMB_FAULT_CLEAR_MS).
const FAULT_CLEAR_MS: u32 = ipmc_defs::IPMB_FAULT_CLEAR_MS;

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

/// LoggedTrace -- the trace sink the link hands to the core and the extension: every line goes
/// into the capture log (stamped with the SysTick time) and, when `console` is set, to the
/// console. The one place a `CaptureTrace` over the console is built.
struct LoggedTrace<'a> {
    log: &'a mut DefaultLog,
    console: bool,
}

impl<'a> LoggedTrace<'a> {
    fn new(log: &'a mut DefaultLog, console: bool) -> Self {
        LoggedTrace { log, console }
    }
}

impl Trace for LoggedTrace<'_> {
    fn line(&mut self, args: fmt::Arguments) {
        CaptureTrace {
            log: &mut *self.log,
            inner: &mut ConsoleTrace,
            console: self.console,
            now_ms: systick::millis(),
        }
        .line(args);
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

/// FaultTracker -- the bus-fault streak and fault state of one bus. `faulted` is set by the streak
/// or by a start-up fault; both clear by the same evidence (`clear_faulted_buses`).
#[derive(Clone, Copy, Default)]
pub struct FaultTracker {
    /// Consecutive bus-fault errors; a good write or frame resets it.
    streak: u32,
    /// The streak reached IPMB_FAULT_STREAK, or the bus failed at start-up; the link records the
    /// local status.
    faulted: bool,
    /// The fault is a start-up failure: the I2C slave never ran, so the clear is a new
    /// `bring_up`, not `clear_bus_fault`.
    startup: bool,
    /// The start-up failure last reported on the console, None when none was: a retry that fails
    /// for the same reason stays silent.
    failure: Option<BringUpFailure>,
    /// SysTick time since both lines read high without a break, while faulted.
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

const fn tracker_index(bus: Bus) -> usize {
    match bus {
        Bus::A => 0,
        Bus::B => 1,
    }
}

/// BringUpFailure -- why `bring_up` failed. Compared by value: a restart that fails for the same
/// reason is not reported again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BringUpFailure {
    /// The bus recovery found a line it could not free.
    Bus(MasterError),
    /// The I2C slave did not start.
    Init(I2cError),
}

impl BringUpFailure {
    /// status -- the Table 10.2.2-1 code the link records for this failure.
    fn status(self) -> LocalStatus {
        match self {
            BringUpFailure::Bus(error) => local_status_of(error),
            BringUpFailure::Init(_) => LocalStatus::Undiagnosed,
        }
    }

    /// report -- one line to `trace` naming the failure of `bus`.
    fn report(self, bus: Bus, trace: &mut dyn Trace) {
        match self {
            BringUpFailure::Bus(error) => trace.line(format_args!(
                "IPMB-{}: bus not usable: {}; the bus stays disabled",
                bus.name(),
                error.name()
            )),
            BringUpFailure::Init(error) => trace.line(format_args!(
                "IPMB-{}: I2C init failed: {}; the bus stays disabled",
                bus.name(),
                error.name()
            )),
        }
    }
}

/// bring_up -- starts one bus: bus recovery if SDA is held low, then the I2C slave. An error is
/// returned for the caller to report (`BringUpFailure::report`) and to record as the local fault;
/// the bus then stays disabled and the board keeps running. The one function for the first start
/// and for every restart after a start-up fault; on error `periph` still holds the peripheral.
fn bring_up(
    bus: Bus,
    periph: &mut Option<I2cPeripheral>,
    ctx: &mut BringUp,
    master: &mut SelectedMaster,
) -> Result<RxConsumer, BringUpFailure> {
    match master.recover_if_stuck(bus) {
        Ok(Recovery::NotNeeded) => {}
        Ok(Recovery::Recovered) => out!(
            "IPMB-{}: SDA was held low; released by the recovery clocks\n",
            bus.name()
        ),
        Err(error) => return Err(BringUpFailure::Bus(error)),
    }
    i2c_hw::init(
        bus,
        periph,
        &ctx.rcc,
        &mut ctx.nvic,
        master.pins(bus),
        ctx.pclk1_hz,
        ctx.own_addr8,
    )
    .map_err(BringUpFailure::Init)
}

/// start_drivers -- start-up of the IPMB drivers, after the pin park and the strap pins (the own
/// address is known), before the prompt.
/// 1. The master (IPMB_MASTER_MODE) takes the pins and the DWT cycle counter; a failure here is fatal
///    (the board cannot transmit) and parks after a console line.
/// 2. Each fitted bus gets `bring_up`. A bus whose address is unusable (parity, GA = 0) is left
///    alone: the link keeps it isolated (Rule 4.1.5.4.1-5) and never starts it. IPMB-B not
///    fitted: its pins stay parked and I2C3 is not touched (it is dropped here unused). A bus
///    that fails keeps its peripheral in `IpmbIo`; the link tries again when the fault clears.
pub fn start_drivers(
    parts: DriverParts<'_>,
    i2c1: I2C1,
    i2c3: I2C3,
    pins: IpmbPins,
    address: &AddrInfo,
) -> IpmbIo {
    let mut master =
        match ipmb_master::new_selected(&mut *parts.dcb, &mut *parts.dwt, parts.hclk_hz, pins) {
            Ok(master) => master,
            Err(error) => {
                out!("\nFATAL: IPMB master: {}\n", error.name());
                uart::flush(uart::Port::Console);
                fault::park()
            }
        };
    let mut ctx = BringUp {
        rcc: parts.rcc,
        nvic: parts.nvic,
        pclk1_hz: parts.pclk1_hz,
        own_addr8: address.ipmb_address,
    };
    let (mut rx_a, mut rx_b) = (None, None);
    let (mut fault_a, mut fault_b) = (None, None);
    let mut periph = [None, None];
    if address.usable() {
        let mut a = Some(I2cPeripheral::I1(i2c1));
        match bring_up(Bus::A, &mut a, &mut ctx, &mut master) {
            Ok(rx) => rx_a = Some(rx),
            Err(failure) => {
                failure.report(Bus::A, &mut ConsoleTrace);
                fault_a = Some(failure);
            }
        }
        periph[tracker_index(Bus::A)] = a;
        if B_FITTED {
            let mut b = Some(I2cPeripheral::I3(i2c3));
            match bring_up(Bus::B, &mut b, &mut ctx, &mut master) {
                Ok(rx) => rx_b = Some(rx),
                Err(failure) => {
                    failure.report(Bus::B, &mut ConsoleTrace);
                    fault_b = Some(failure);
                }
            }
            periph[tracker_index(Bus::B)] = b;
        }
    }
    IpmbIo {
        master,
        rx_a,
        rx_b,
        fault_a,
        fault_b,
        bring_up: ctx,
        periph,
    }
}

/// HandOffs -- what `IpmbLink::take_hand_offs` returns.
pub struct HandOffs {
    /// A payload power request of a handler.
    pub power: Option<PowerRequest>,
    /// A handler asked for a payload reset pulse.
    pub payload_reset: bool,
    /// The power-on check at that moment (`IpmbLink::power_on_check`).
    pub power_on_check: Result<(), PowerOnRefused>,
}

/// IpmbLink -- the link layer, the dispatch context it answers requests with, and the drivers it
/// moves frames to and from.
pub struct IpmbLink {
    link: Link,
    dispatch: DispatchContext,
    master: SelectedMaster,
    rx_a: Option<RxConsumer>,
    rx_b: Option<RxConsumer>,
    faults: [FaultTracker; FaultTracker::COUNT],
    /// For the restart of a bus after a start-up fault.
    bring_up: BringUp,
    /// The peripheral of a bus whose bring-up failed (taken by `init` when it succeeds).
    periph: [Option<I2cPeripheral>; FaultTracker::COUNT],
    /// The FRU flash log (state and flash driver); the main loop saves through it.
    fru_log: FruLog,
    /// SysTick time of the latest frame taken from a driver queue (any frame: it is bus traffic);
    /// None before the first. A run-time erase waits for FRU_ERASE_QUIET_MS after it.
    last_rx_ms: Option<u32>,
    /// The extension this build runs: the Tier-2/3 extension when built in, else none.
    ext: ipmc_ext::Active,
    /// The one decision when a pending Cold Reset runs (`service_reset` only performs it).
    reset_gate: ResetGate,
    /// A save tried while a Cold Reset was pending failed: the normal delay applies again, so a
    /// failing flash does not stall every pass until the reset wait runs out.
    reset_save_failed: bool,
    /// The speed each bus's master and I2C peripheral run at (`apply_bus_speeds`); both start at
    /// 100 kHz (Rule 4.1.1-4.3), as the link does after any restart.
    applied_speed: [BusSpeed; FaultTracker::COUNT],
    /// The capture log: every whole frame in and out and the trace lines (DEBUG DUMP prints it).
    capture: DefaultLog,
    /// One console line per IPMB frame in and out (DEBUG TRACE I2C).
    i2c_trace: bool,
    /// The IPMI trace lines (requests, link, events) print on the console (DEBUG TRACE IPMI);
    /// they go into the capture log either way.
    ipmi_console: bool,
    /// The evaluation period ended and both slaves are off: nothing resumes them.
    #[cfg(ipmc_eval)]
    ipmb_stopped: bool,
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
struct PausedErase<'a> {
    flash: &'a mut Flash,
    master: &'a SelectedMaster,
    /// What to do to each bus's slave.
    plan: [BusPause; FaultTracker::COUNT],
}

impl PausedErase<'_> {
    /// suspend_all -- takes every running slave off its bus. Returns the buses suspended, or
    /// Err(Timeout) after resuming those already taken when a healthy one stays busy for
    /// PAUSE_WAIT_MS. A faulted bus is suspended without the wait.
    fn suspend_all(&self) -> Result<[bool; FaultTracker::COUNT], FlashError> {
        let mut paused = [false; FaultTracker::COUNT];
        for bus in Bus::BOTH {
            let index = tracker_index(bus);
            match self.plan[index] {
                BusPause::Absent => continue,
                BusPause::Faulted => {
                    paused[index] = i2c_hw::suspend_faulted(bus);
                    continue;
                }
                BusPause::Healthy => {}
            }
            let start = systick::millis();
            while !i2c_hw::suspend(bus) {
                if systick::millis().wrapping_sub(start) >= PAUSE_WAIT_MS {
                    // EVIDENCE: the slave stayed busy for PAUSE_WAIT_MS; give the buses back.
                    self.resume_all(paused);
                    return Err(FlashError::Timeout);
                }
            }
            paused[index] = true;
        }
        Ok(paused)
    }

    /// resume_all -- gives the suspended buses back to their slaves.
    fn resume_all(&self, paused: [bool; FaultTracker::COUNT]) {
        for bus in Bus::BOTH {
            if paused[tracker_index(bus)] {
                i2c_hw::resume(bus, self.master.pins(bus));
            }
        }
    }
}

impl FlashLog for PausedErase<'_> {
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

    /// erase -- both slaves off, the erase, both back on, one console line with the time the
    /// board was silent (measured with the DWT cycle counter: SysTick does not run while the
    /// core waits on flash).
    fn erase(&mut self) -> Result<(), FlashError> {
        let paused = self.suspend_all()?;
        let start = ipmb_common::now();
        let result = self.flash.erase();
        let paused_ms = ipmb_common::now().wrapping_sub(start) / systick::cycles_per_ms().max(1);
        self.resume_all(paused);
        match result {
            Ok(()) => out!(
                "FRU log: erased the store sector at run time (IPMB paused {} ms)\n",
                paused_ms
            ),
            Err(error) => out!(
                "FRU log: erase of the store sector failed at run time: {} (IPMB paused {} ms)\n",
                error.name(),
                paused_ms
            ),
        }
        result
    }
}

/// RestoreError -- why FRU RESTORE did not restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The flash log is off (IPMC_FRU_PERSIST 0 or it did not start).
    LogOff,
    /// NVMRO is asserted: no non-volatile write (VITA 46.11 Rule 8-3, Rec 4.1.10-1).
    Nvmro,
    /// Erasing the log failed.
    Failed(PersistError),
}

impl IpmbLink {
    /// take -- the one IpmbLink, in static storage (PLAN 3.3a); Some on the first call, None after.
    /// No interrupt handler touches it.
    /// `singleton!` builds the value on the stack and then copies it into the static (about
    /// 16.9 KB, `size_of::<IpmbLink>()`: mostly the 8 KB capture log and the duplicate cache).
    /// `inline(never)` keeps that temporary in this function's frame, which is gone when `take`
    /// returns at start-up; inlined into `main`, the copy would occupy `main`'s frame, and so the
    /// stack, for as long as the firmware runs.
    #[inline(never)]
    pub fn take(
        io: IpmbIo,
        fru_log: FruLog,
        fru_image: Option<FruStore>,
        kept_requests: (u8, u64),
    ) -> Option<&'static mut IpmbLink> {
        cortex_m::singleton!(: IpmbLink = IpmbLink::new(io, fru_log, fru_image, kept_requests))
    }

    /// new -- a link in the power-up state (IPMB-A on, IPMB-B off) for the resolved address; a
    /// bus that failed its start-up gets its local fault, which disables it (Rule 9.3-10).
    fn new(
        io: IpmbIo,
        fru_log: FruLog,
        fru_image: Option<FruStore>,
        (first_request_seq, recent_request_seqs): (u8, u64),
    ) -> Self {
        let resolved = addr::resolve(ga::read_raw());
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
        for (bus, fault) in [(Bus::A, io.fault_a), (Bus::B, io.fault_b)] {
            if let Some(failure) = fault {
                link.set_local_status(bus, failure.status());
                // A start-up fault is tracked like a run-time one: the same evidence-gated
                // clear, then `bring_up` again.
                if let Some(t) = faults.get_mut(tracker_index(bus)) {
                    t.faulted = true;
                    t.startup = true;
                    t.failure = Some(failure);
                }
            }
        }
        let mut dispatch = DispatchContext::new(resolved);
        if let Some(image) = fru_image {
            // The stored image replaces the built one; the self test runs on it (owner decision 8).
            dispatch.set_fru(image);
        }
        IpmbLink {
            link,
            dispatch,
            master: io.master,
            rx_a: io.rx_a,
            rx_b: io.rx_b,
            faults,
            bring_up: io.bring_up,
            periph: io.periph,
            fru_log,
            last_rx_ms: None,
            ext: ipmc_ext::Active::default(),
            reset_gate: ResetGate::new(),
            reset_save_failed: false,
            applied_speed: [BusSpeed::Standard; FaultTracker::COUNT],
            capture: DefaultLog::new(ipmc_defs::IPMC_LOG_AT_BOOT == 1),
            i2c_trace: ipmc_defs::IPMC_I2C_TRACE_AT_BOOT == 1,
            ipmi_console: true,
            #[cfg(ipmc_eval)]
            ipmb_stopped: false,
        }
    }

    /// dispatch -- the context the command handlers answer from: FRU image, SDR repository, FRU
    /// state, latest sensor snapshot. Read by the FRU and SENSORS console commands.
    pub fn dispatch(&self) -> &DispatchContext {
        &self.dispatch
    }

    /// fru_log_status -- the flash log state for FRU STORE; None while the log is off.
    pub fn fru_log_status(&self) -> Option<Status> {
        self.fru_log.status()
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
        if ga::nvmro_asserted() {
            return Err(RestoreError::Nvmro);
        }
        let plan = self.erase_plan();
        let (persist, flash) = self.fru_log.parts().ok_or(RestoreError::LogOff)?;
        let mut paused = PausedErase {
            flash,
            master: &self.master,
            plan,
        };
        // The built image replaces the FRU content; the Tier-3 settings byte is not FRU content
        // and stays as it is.
        let mut built = FruStore::new();
        built.load_nv_settings(self.dispatch.nv_settings());
        match persist.restore(&mut paused, built.image(), built.nv_settings()) {
            Ok(_) => {}
            Err(PersistError::Flash(FlashError::NvmroAsserted)) => return Err(RestoreError::Nvmro),
            Err(error) => return Err(RestoreError::Failed(error)),
        }
        self.dispatch.set_fru(built);
        Ok(())
    }

    /// bus_quiet -- true when a run-time erase may start: no response waits in the egress queue
    /// and no frame was taken from a driver queue for FRU_ERASE_QUIET_MS.
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
    pub fn set_unit_field(&mut self, field: UnitField, value: &str) -> Result<(), FieldError> {
        if ga::nvmro_asserted() {
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
        let now_ms = systick::millis();
        let reset_pending = self.dispatch.pending_reset.is_some();
        let urgent = reset_pending && !self.reset_save_failed;
        let quiet = urgent || self.bus_quiet(now_ms);
        let delay_ms = if urgent { 0 } else { FRU_PERSIST_DELAY_MS };
        let Some((persist, flash)) = self.fru_log.parts() else {
            return;
        };
        let mut paused = PausedErase {
            flash,
            master: &self.master,
            plan,
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

    /// link -- the link layer, for the console views of the bus state.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// feed_sensors -- hands the latest sensor sample to the command handlers and steps the FRU
    /// state machine with it (owner decision 3). The main loop calls it once per sample; a state
    /// change prints one console line (`Context::update_sensors` writes it). The extension's
    /// `on_sample` hook runs after it (`ext::feed_sensors`).
    pub fn feed_sensors(&mut self, snapshot: &SensorSnapshot) {
        let mut trace = LoggedTrace::new(&mut self.capture, self.ipmi_console);
        ext::feed_sensors(
            &mut self.dispatch,
            &mut self.ext,
            systick::millis(),
            snapshot,
            &mut trace,
        );
    }

    /// status -- one status line: it prints on the console whatever the IPMI console switch says
    /// and goes into the capture log.
    fn status(&mut self, args: fmt::Arguments) {
        LoggedTrace::new(&mut self.capture, true).line(args);
    }

    /// power_on_check -- whether payload power may come on now: the one combined check of every
    /// power-on path (`ext::check_power_on`: the FRU state machine and the extension).
    pub fn power_on_check(&self) -> Result<(), PowerOnRefused> {
        ext::check_power_on(&self.dispatch, &self.ext)
    }

    /// take_hand_offs -- the requests a handler left for the owner of the payload power and of
    /// the payload reset output (`Context::pending_power`, `Context::pending_payload_reset`),
    /// cleared, with the power-on check the request meets (`Monitor::apply_power_request`).
    pub fn take_hand_offs(&mut self) -> HandOffs {
        HandOffs {
            power: self.dispatch.pending_power.take(),
            payload_reset: core::mem::take(&mut self.dispatch.pending_payload_reset),
            power_on_check: self.power_on_check(),
        }
    }

    /// service -- main-loop step.
    /// 1. Samples NVMRO (one console line when it changes). The GA bits are not re-resolved: the
    ///    address is fixed after start-up.
    /// 2. Drains the receive queues into the link in STOP-time order.
    /// 3. Runs `Link::poll`.
    /// 4. Sends at most one queued response through the master (`transmit`).
    /// 5. Saves the FRU image to the flash log when a Write FRU Data has been quiet long enough
    ///    (a save that must erase a sector also waits for a quiet bus, `bus_quiet`).
    /// 6. Restarts the processor (Cold Reset) or the link state (Warm Reset) when a handler left
    ///    one in `pending_reset` and its response has left (`service_reset`).
    /// 7. Applies a speed the link set to a bus (`apply_bus_speeds`), between transfers.
    ///
    /// The bus-fault bookkeeping runs around step 4: `track_slave_faults` before it (frames
    /// received in this pass reset the streak, the writes then add to it) and
    /// `clear_faulted_buses` after it.
    pub fn service(&mut self) {
        let nvmro = ga::nvmro_asserted();
        if nvmro != self.dispatch.addr.nvmro_asserted {
            self.dispatch.addr.nvmro_asserted = nvmro;
            LoggedTrace::new(&mut self.capture, self.ipmi_console).line(format_args!(
                "NVMRO: {}",
                if nvmro { "asserted" } else { "deasserted" }
            ));
        }
        self.drain_rx();
        let mut trace = LoggedTrace::new(&mut self.capture, self.ipmi_console);
        self.link.poll(
            &mut self.dispatch,
            &mut self.ext,
            systick::millis(),
            &mut trace,
        );
        self.track_slave_faults();
        self.transmit();
        self.clear_faulted_buses();
        self.service_fru_log();
        self.service_reset();
        self.apply_bus_speeds();
    }

    /// apply_bus_speeds -- gives each running bus the speed the link holds for it (Set IPMB State
    /// speed byte, Rule 4.1.1-4.4): the master timing and the I2C peripheral change together
    /// (`IpmbMaster::set_speed`), once the slave is idle, so a change never lands inside a
    /// transfer. A bus that is busy or has no running slave is asked again on the next pass; a
    /// change that lands prints one console line.
    fn apply_bus_speeds(&mut self) {
        for bus in Bus::BOTH {
            let wanted = self.link.bus_state(bus).speed;
            let index = tracker_index(bus);
            let running = match bus {
                Bus::A => self.rx_a.is_some(),
                Bus::B => self.rx_b.is_some(),
            };
            if !running || self.applied_speed[index] == wanted {
                continue;
            }
            if self.master.set_speed(bus, wanted) {
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
    /// what is queued. Each running slave goes through the driver's own suspend (waiting up to
    /// PAUSE_WAIT_MS for an idle bus, as an erase pause does); a faulted bus, or one that stays
    /// busy past the wait, goes through `suspend_faulted`. The pins stay as the suspended state
    /// leaves them (released, open drain) and nothing resumes the slaves. The main loop never
    /// calls `service` afterwards.
    #[cfg(ipmc_eval)]
    pub fn stop_for_eval(&mut self) {
        let plan = pause_plan([self.rx_a.is_some(), self.rx_b.is_some()], &self.faults);
        for bus in Bus::BOTH {
            let off = match plan[tracker_index(bus)] {
                BusPause::Absent => continue,
                BusPause::Faulted => false,
                BusPause::Healthy => {
                    let start = systick::millis();
                    let mut idle = false;
                    while !idle && systick::millis().wrapping_sub(start) < PAUSE_WAIT_MS {
                        idle = i2c_hw::suspend(bus);
                    }
                    idle
                }
            };
            if !off {
                let _ = i2c_hw::suspend_faulted(bus);
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

    /// service_reset -- performs `Context::pending_reset` when `ResetGate` says so (the response
    /// left and no accepted FRU write is unsaved, or IPMC_RESET_WAIT_MS passed).
    /// - Cold: one console line, the console is flushed, the requester's next sequence number is
    ///   kept across the reset (`reset_cause::keep_sequence`) and the processor restarts.
    /// - Warm: `warm_reset`, no processor reset.
    fn service_reset(&mut self) {
        let now_ms = systick::millis();
        let request = self.dispatch.pending_reset;
        let egress_empty = self.link.egress_empty();
        let fru_clean = !self.fru_unsaved();
        let mut trace = LoggedTrace::new(&mut self.capture, self.ipmi_console);
        if !self.reset_gate.poll(
            request.is_some(),
            now_ms,
            egress_empty,
            fru_clean,
            &mut trace,
        ) {
            return;
        }
        match request {
            Some(ResetRequest::Cold) => {
                self.status(format_args!("Cold Reset: restarting"));
                uart::flush(uart::Port::Console);
                reset_cause::keep_sequence(
                    self.link.next_request_seq(),
                    self.link.recent_request_seqs(now_ms),
                );
                reset_cause::system_reset();
            }
            Some(ResetRequest::Warm) => self.warm_reset(now_ms),
            None => {}
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
        let mut link = self.link.restarted(&self.dispatch.addr, now_ms);
        for bus in Bus::BOTH {
            let status = self.link.bus_state(bus).status;
            if status != LocalStatus::None {
                link.set_local_status(bus, status);
            }
        }
        self.dispatch.warm_reset(link.view());
        self.link = link;
        self.ext.warm_reset();
        self.reset_gate = ResetGate::new();
        self.status(format_args!("Warm Reset: link state re-initialised"));
    }

    /// tracker -- the fault tracker of `bus`.
    fn tracker(&mut self, bus: Bus) -> &mut FaultTracker {
        &mut self.faults[tracker_index(bus)]
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
            let d = i2c_hw::stats(bus);
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
        let now_ms = systick::millis();
        for bus in Bus::BOTH {
            if !self.faults[tracker_index(bus)].faulted {
                continue;
            }
            if !self.master.lines_high(bus) {
                self.tracker(bus).high_since = None;
                continue;
            }
            let since = *self.tracker(bus).high_since.get_or_insert(now_ms);
            if now_ms.wrapping_sub(since) < FAULT_CLEAR_MS {
                continue;
            }
            let cleared = if self.faults[tracker_index(bus)].startup {
                // `restart_bus` reports a new failure reason.
                self.restart_bus(bus)
            } else {
                match self.master.clear_bus_fault(bus) {
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
                let d = i2c_hw::stats(bus);
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

    /// restart_bus -- runs `bring_up` again for a bus that failed at start-up; true when the I2C
    /// slave now runs (its frame queue joins the drain).
    fn restart_bus(&mut self, bus: Bus) -> bool {
        let Some(periph) = self.periph.get_mut(tracker_index(bus)) else {
            return false;
        };
        match bring_up(bus, periph, &mut self.bring_up, &mut self.master) {
            Ok(rx) => {
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
                    failure.report(bus, &mut LoggedTrace::new(&mut self.capture, true));
                    self.link.set_local_status(bus, failure.status());
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
                (Some(a), Some(b)) if (b.wrapping_sub(a) as i32) < 0 => Bus::B,
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
    /// it through the master and reports the outcome with `on_tx_result` (the link resends on
    /// the other bus after a failure). One write per `service` pass, so the receive drain and
    /// the link poll run between writes. A pass blocks for at most one write budget (T2 + T1 =
    /// 80 ms at the T2 minimum, `ipmb_common::WRITE_BUDGET_NS`), inside T5 (227 ms, VITA 46.11
    /// Rule 4.1.1-11); the main loop calls `service` on every pass (a 1 ms SysTick wakes it), so
    /// the next queued frame follows within one tick plus the console and sensor steps.
    fn transmit(&mut self) {
        let mut buf = [0u8; IPMB_MAX_LEN];
        let Some((bus, frame)) = self.link.next_tx() else {
            return;
        };
        let Some(out) = buf.get_mut(..frame.len()) else {
            return; // the egress queue only holds frames up to IPMB_MAX_LEN
        };
        out.copy_from_slice(frame);
        let result = self.master.write(bus, out);
        let now_ms = systick::millis();
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
        let mut trace = LoggedTrace::new(&mut self.capture, self.ipmi_console);
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
        let index = self.capture.stats().total;
        self.capture.frame(time_ms, bus, dir, status, bytes);
        if self.i2c_trace {
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
            out!("{}\n", TraceRow(&record, &CoreNames(&self.ext)));
        }
    }

    // ---- the DEBUG console group (debug_cmds.rs) ----

    /// capture -- the capture log.
    pub fn capture(&self) -> &DefaultLog {
        &self.capture
    }

    /// capture_mut -- the capture log, to switch or clear it.
    pub fn capture_mut(&mut self) -> &mut DefaultLog {
        &mut self.capture
    }

    /// names -- the command names of this build, for the capture printers.
    pub fn names(&self) -> CoreNames<'_> {
        CoreNames(&self.ext)
    }

    /// i2c_trace -- one console line per IPMB frame is on.
    pub const fn i2c_trace(&self) -> bool {
        self.i2c_trace
    }

    /// set_i2c_trace -- switches the per-frame console line.
    pub fn set_i2c_trace(&mut self, on: bool) {
        self.i2c_trace = on;
    }

    /// ipmi_console -- the IPMI trace lines print on the console.
    pub const fn ipmi_console(&self) -> bool {
        self.ipmi_console
    }

    /// set_ipmi_console -- switches the IPMI trace lines on the console; the log keeps them.
    pub fn set_ipmi_console(&mut self, on: bool) {
        self.ipmi_console = on;
    }

    /// log_text -- a line for the capture log only (console requests and responses).
    pub fn log_text(&mut self, args: fmt::Arguments) {
        LoggedTrace::new(&mut self.capture, false).line(args);
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
        let mut trace = LoggedTrace::new(&mut self.capture, false);
        self.link.local_request(
            &mut self.dispatch,
            &mut self.ext,
            netfn,
            cmd,
            data,
            systick::millis(),
            &mut trace,
        )
    }

    /// inject -- hands a whole frame to the link as if `bus` had received it: logged as an
    /// injected In frame, then `Link::on_frame`. Err(reason) is the drop; a request that is
    /// accepted is answered on the bus by the next `service` passes.
    pub fn inject(&mut self, bus: Bus, bytes: &[u8]) -> Result<(), FrameDrop> {
        let now_ms = systick::millis();
        self.last_rx_ms = Some(now_ms);
        self.record_frame(now_ms, bus, Dir::In, FrameStatus::Injected, bytes);
        self.link.on_frame(bus, bytes, now_ms)
    }

    /// clear_duplicate_cache -- empties the duplicate cache (DEBUG CACHE CLEAR).
    pub fn clear_duplicate_cache(&mut self) {
        self.link.clear_duplicate_cache();
    }
}

/// print_driver_stats -- the driver, master and fault counter lines of one fitted bus.
fn print_driver_stats(master: &SelectedMaster, faults: &FaultTracker, bus: Bus) {
    let d = i2c_hw::stats(bus);
    let name = bus.name();
    out!(
        "IPMB-{} drv:   rx={} overrun={} q-full={} empty={} restart-drop={} slave-rd={} rd-end={}\n",
        name,
        d.rx_frames,
        d.rx_overrun,
        d.rx_queue_full,
        d.rx_empty,
        d.restart_drops,
        d.slave_reads,
        d.slave_read_end
    );
    out!(
        "IPMB-{} drv err: berr={} arlo={} af={} ovr={} timeout={} master-berr-spurious={}\n",
        name,
        d.bus_error,
        d.arbitration_lost,
        d.ack_failure,
        d.overrun,
        d.timeout,
        d.master_berr_spurious
    );
    out!(
        "IPMB-{} fault: streak={}/{} state={} busy-errata-resets={}\n",
        name,
        faults.streak,
        FAULT_STREAK,
        if faults.faulted { "FAULTED" } else { "ok" },
        d.busy_errata_resets
    );
    let m = master.stats().bus(bus);
    let count = |kind: MasterError| m.errors.get(kind.index()).copied().unwrap_or(0);
    out!(
        "IPMB-{} master: ok={} busy={} arb-lost={} arb-retries={} addr-nack={} data-nack={} stretch={} scl-low={} sda-low={} scl-high={} sda-high={}\n",
        name,
        m.writes_ok,
        count(MasterError::BusBusy),
        count(MasterError::ArbitrationLost),
        m.arb_retries,
        count(MasterError::AddressNack),
        count(MasterError::DataNack),
        count(MasterError::StretchTimeout),
        count(MasterError::SclStuckLow),
        count(MasterError::SdaStuckLow),
        count(MasterError::SclStuckHigh),
        count(MasterError::SdaStuckHigh)
    );
}

/// print_bus_stats -- the three counter lines of one bus.
fn print_bus_stats(link: &Link, bus: Bus) {
    let c = link.counters(bus);
    let e = &c.rx_errors;
    let name = bus.name();
    out!(
        "IPMB-{} rx:    ok={} not-for-us={} response={} answers={} unavailable={} ingress-full={}\n",
        name,
        c.rx_ok,
        c.not_for_us,
        c.rx_response,
        c.rx_answers,
        c.bus_unavailable,
        c.ingress_full
    );
    out!(
        "IPMB-{} rx err: short={} long={} chk1={} chk2={} address={} other={}\n",
        name,
        e.too_short,
        e.too_long,
        e.header_checksum,
        e.data_checksum,
        e.invalid_address,
        e.other
    );
    out!(
        "IPMB-{} tx:    ok={} requests={} fail={} fail-streak={} dropped={} resends={} replays={}\n",
        name,
        c.tx_ok,
        c.tx_requests,
        c.tx_fail,
        c.tx_fail_streak,
        c.tx_dropped,
        c.resends,
        c.replays
    );
}

/// cmd_ipmb_stats -- IPMB STATS: the per-bus counters, the driver and master counters of each
/// fitted bus, and the link-wide counters.
pub fn cmd_ipmb_stats(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let link = &ctx.ipmb.link;
    for bus in Bus::BOTH {
        print_bus_stats(link, bus);
        if link.bus_state(bus).fitted {
            print_driver_stats(&ctx.ipmb.master, &ctx.ipmb.faults[tracker_index(bus)], bus);
        }
    }
    match ctx.ipmb.master.stats().scl_period_ns() {
        Some(ns) => out!("IPMB master:   last write SCL period {} ns\n", ns),
        None => out!("IPMB master:   no write yet\n"),
    }
    out!(
        "IPMB link:     cache-evictions={} late-responses={} dispatch-errors={} egress-full={}\n",
        link.evictions(),
        link.late_responses(),
        link.dispatch_errors(),
        link.egress_dropped()
    );
    Ok(())
}

/// cmd_ipmb_state -- IPMB STATE: per-bus fitted, mode, enable and fault state, the transmit
/// policy and the IPMB Link sensor reading bytes.
pub fn cmd_ipmb_state(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let link = &ctx.ipmb.link;
    for bus in Bus::BOTH {
        let s = link.bus_state(bus);
        out!(
            "IPMB-{}: {}, {}, {}, local status {}\n",
            bus.name(),
            if s.fitted { "fitted" } else { "not fitted" },
            s.mode.name(),
            if s.enabled() { "enabled" } else { "disabled" },
            s.status.name()
        );
    }
    out!(
        "TX policy: {} (ipmi_defs.h IPMB_TX_POLICY)\n",
        link.config().tx_policy.name()
    );
    let mut data = [0u8; sensor::READING_LEN];
    let reading = ctx.ipmb.dispatch.read_sensor(
        sensor::SENSOR_SYSTEM_IPMB_LINK,
        &link.view().buses,
        &mut data,
    );
    if reading.is_ok() {
        out!(
            "Link sensor data (sensor 1): {:02X} {:02X} {:02X}\n",
            data[0],
            data[1],
            data[2]
        );
    }
    Ok(())
}
