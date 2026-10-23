// bsp/ipmb_common.rs - what both IPMB master methods (bit-bang, hardware I2C) share: the DWT cycle
// counter and the wait times in cycles, the free-bus wait with the I2C BUSY-flag erratum check, and
// the write wrapper with the arbitration-loss retry loop.
//
// Ownership: this module owns the DWT cycle counter use (`start_cycle_counter`); it reads the
// IPMB pins (levels only) lent by the master and reaches the I2C registers only through
// bsp/i2c_hw.rs. One implementation of each piece serves both masters (no copy between them).

use cortex_m::peripheral::{DCB, DWT};
use ipmc_core::queue::Bus;

use super::ipmb_master::MasterError;
use super::ipmb_pins::BusPins;
use super::{i2c_hw, led};

/// T_BUF_NS -- bus free time between a STOP and the next START, standard mode: 4.7 us (I2C-bus
/// specification Table 5, tBUF). A master starts only after both lines were high this long.
pub const T_BUF_NS: u32 = 4_700;
/// T7_BYTE_MAX_NS -- VITA 46.11 Table 4.1.1-1 T7, overall byte duration, at most 3 ms (a spec
/// value, not a tunable). Both lines high continuously for longer than this, with SR2.BUSY set
/// and the slave idle, is the stuck-BUSY signature: no legitimate IPMB byte keeps SCL and SDA
/// both high that long, so a transfer in progress is never mistaken for it.
const T7_BYTE_MAX_NS: u64 = 3_000_000;
/// T1_FRAME_NS -- VITA 46.11 Table 4.1.1-1 T1, IPMI message duration, at most 20 ms: the whole
/// master write of one frame must end within it.
const T1_FRAME_NS: u64 = 20_000_000;
const NS_PER_S: u64 = 1_000_000_000;
const MS_PER_S: u64 = 1_000;
/// WRITE_BUDGET_NS -- the time budget of one master write: T2 (IPMB_BUS_FREE_TIMEOUT_MS, VITA
/// 46.11 Table 4.1.1-1, at least 60 ms) plus T1. With the T2 minimum this is 80 ms, inside T5
/// (227 ms): one response never holds the main loop longer than that, however many arbitration
/// retries it takes.
const WRITE_BUDGET_NS: u64 =
    ipmc_defs::IPMB_BUS_FREE_TIMEOUT_MS as u64 * NS_PER_S / MS_PER_S + T1_FRAME_NS;
const NS_PER_US: u32 = 1_000;
/// Polls of the cycle counter that must show it advancing, at `start_cycle_counter`.
const COUNTER_CHECK_POLLS: u32 = 1_000;

/// B_FITTED -- IPMB-B is built in (ipmi_defs.h IPMB_BUS_B_ENABLE).
const B_FITTED: bool = ipmc_defs::IPMB_BUS_B_ENABLE == 1;
/// ARB_RETRIES -- retries of a write after arbitration loss (ipmi_defs.h IPMB_ARB_RETRIES).
const ARB_RETRIES: u32 = ipmc_defs::IPMB_ARB_RETRIES;

/// TimebaseError -- why a master's `new` refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimebaseError {
    /// The core clock is zero or a derived time does not fit 32 bits of cycles.
    CoreClock,
    /// The DWT cycle counter does not advance after being enabled.
    CycleCounterStopped,
}

impl TimebaseError {
    /// name -- text for log lines.
    pub const fn name(self) -> &'static str {
        match self {
            TimebaseError::CoreClock => "core clock unusable for the master timing",
            TimebaseError::CycleCounterStopped => "DWT cycle counter does not run",
        }
    }
}

/// now -- the DWT cycle counter.
pub fn now() -> u32 {
    DWT::cycle_count()
}

/// cycles_for -- ceil(ns x core_hz / 1e9) as u32 cycles, None when it does not fit.
pub fn cycles_for(ns: u64, core_hz: u32) -> Option<u32> {
    let cycles = (ns * u64::from(core_hz)).div_ceil(NS_PER_S);
    u32::try_from(cycles).ok()
}

/// start_cycle_counter -- DCB trace enable, then CYCCNT (Cortex-M4 TRM, DWT_CTRL.CYCCNTENA);
/// Err when the counter does not advance.
pub fn start_cycle_counter(dcb: &mut DCB, dwt: &mut DWT) -> Result<(), TimebaseError> {
    dcb.enable_trace();
    dwt.enable_cycle_counter();
    let first = now();
    if (0..COUNTER_CHECK_POLLS).any(|_| now() != first) {
        Ok(())
    } else {
        Err(TimebaseError::CycleCounterStopped)
    }
}

/// BusTimes -- the times both masters wait, in core clock cycles.
#[derive(Clone, Copy)]
pub struct BusTimes {
    /// Longest wait for a stretched SCL, or for one hardware event: IPMB_STRETCH_TIMEOUT_US.
    pub stretch: u32,
    /// Bus free time before a START: T_BUF_NS.
    pub tbuf: u32,
    /// The time budget of one `IpmbMaster::write`, first attempt and every arbitration retry
    /// together: IPMB_BUS_FREE_TIMEOUT_MS (VITA T2) plus T1_FRAME_NS (T1).
    pub write_budget: u32,
    /// Lines high with BUSY set and no frame longer than this: the stuck-BUSY signature, T7_BYTE_MAX_NS.
    pub busy_stuck: u32,
    /// Longest master write of one frame: T1_FRAME_NS.
    pub frame: u32,
}

impl BusTimes {
    /// new -- the times at `core_hz`; None when the clock is zero or a time overflows 32 bits.
    pub fn new(core_hz: u32) -> Option<BusTimes> {
        if core_hz == 0 {
            return None;
        }
        Some(BusTimes {
            stretch: cycles_for(
                u64::from(ipmc_defs::IPMB_STRETCH_TIMEOUT_US) * u64::from(NS_PER_US),
                core_hz,
            )?,
            tbuf: cycles_for(u64::from(T_BUF_NS), core_hz)?,
            write_budget: cycles_for(WRITE_BUDGET_NS, core_hz)?,
            busy_stuck: cycles_for(T7_BYTE_MAX_NS, core_hz)?,
            frame: cycles_for(T1_FRAME_NS, core_hz)?,
        })
    }
}

/// start_timebase -- the start of every master's `new`: converts the times of ipmi_defs.h with
/// `make` (the master's own cycle table, None when the clock is unusable -> CoreClock), then
/// starts the DWT cycle counter (`start_cycle_counter`).
pub fn start_timebase<T>(
    dcb: &mut DCB,
    dwt: &mut DWT,
    core_hz: u32,
    make: impl FnOnce(u32) -> Option<T>,
) -> Result<T, TimebaseError> {
    let table = make(core_hz).ok_or(TimebaseError::CoreClock)?;
    start_cycle_counter(dcb, dwt)?;
    Ok(table)
}

/// Deadline -- one time limit: the cycle count at its start and its length in cycles. Compared
/// with wrapping subtraction, so a counter wrap does not matter for limits under 2^31 cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    begin: u32,
    limit: u32,
}

impl Deadline {
    /// starting_at -- a limit of `limit` cycles counted from the cycle count `begin`.
    pub const fn starting_at(begin: u32, limit: u32) -> Deadline {
        Deadline { begin, limit }
    }

    /// from_now -- a limit of `limit` cycles from now.
    pub fn from_now(limit: u32) -> Deadline {
        Deadline::starting_at(now(), limit)
    }

    /// passed -- the cycle count `at` is later than the end of the limit.
    pub const fn passed(self, at: u32) -> bool {
        at.wrapping_sub(self.begin) > self.limit
    }

    /// as_pair -- the (begin, limit) form `wait_for` takes.
    pub const fn as_pair(self) -> (u32, u32) {
        (self.begin, self.limit)
    }
}

/// wait_for -- the one bounded polling loop of the masters: polls `done` until it returns true
/// (-> true) or until one of the `deadlines` (cycle count at the start, limit in cycles) is
/// past (-> false). The deadlines are checked in every round, before success is returned: a
/// condition that is already true when a deadline has passed still ends in false, so a ready
/// condition never lets a transfer run past T1 or the write's shared budget.
pub fn wait_for(mut done: impl FnMut() -> bool, deadlines: &[(u32, u32)]) -> bool {
    loop {
        let ready = done();
        let t = now();
        if deadlines
            .iter()
            .any(|&(begin, limit)| t.wrapping_sub(begin) > limit)
        {
            return false;
        }
        if ready {
            return true;
        }
    }
}

/// lines_high -- both lines of the bus read high.
pub fn lines_high(pins: &BusPins) -> bool {
    pins.scl.read() != 0 && pins.sda.read() != 0
}

/// wait_bus_free -- waits until SCL and SDA have both been high for tBUF and the slave is idle
/// (no frame in progress, SR2.BUSY clear), then runs `claim`, which takes the bus away from the
/// slave (suspend for the bit-bang master, interrupt mask for the hardware master) and returns
/// false when a frame began in between. Bounded by the write's shared `deadline` -> BusBusy. Ok means
/// `claim` succeeded: the caller owes the hand-back.
///
/// Stuck BUSY flag: both lines high for longer than T7 (T7_BYTE_MAX_NS) while SR2.BUSY is set
/// and the slave has no frame is the signature; `i2c_hw::clear_stuck_busy` clears it. The "analog
/// filter locks BUSY" erratum is documented in other STM32 errata sheets (F1 family) and is not
/// confirmed in ES0299; the reset sequence is kept as a defensive recovery for a stuck BUSY flag.
/// A line that cannot be driven in that procedure is SclStuckLow/High or SdaStuckLow/High.
pub fn wait_bus_free(
    bus: Bus,
    pins: &BusPins,
    t: &BusTimes,
    deadline: Deadline,
    claim: impl Fn(Bus) -> bool,
) -> Result<(), MasterError> {
    let mut high_since: Option<u32> = None;
    loop {
        let t_now = now();
        if deadline.passed(t_now) {
            return Err(MasterError::BusBusy);
        }
        if !lines_high(pins) {
            high_since = None;
            continue;
        }
        let since = *high_since.get_or_insert(t_now);
        let high_for = t_now.wrapping_sub(since);
        if i2c_hw::slave_idle(bus) {
            if high_for >= t.tbuf {
                if claim(bus) {
                    return Ok(());
                }
                // A frame began between the idle check and the claim; wait it out.
                high_since = None;
            }
        } else if high_for >= t.busy_stuck && i2c_hw::busy_without_frame(bus) {
            // EVIDENCE: BUSY set, no frame in progress, both lines high for longer than T7.
            i2c_hw::clear_stuck_busy(bus, pins).map_err(|line| line.master_error())?;
            high_since = None;
        }
    }
}

/// write_retrying -- the write wrapper both masters share. Guards (empty frame -> FrameLength,
/// IPMB-B not fitted -> BusBusy with nothing touched), `led::activity()`, then `once` until it
/// ends in anything but ArbitrationLost; arbitration loss is retried on the same bus up to
/// IPMB_ARB_RETRIES times (each retry waits for a free bus again). Every other failure is
/// returned at once for the link to resend on the other bus. Returns the result and the number
/// of arbitration retries made.
///
/// One time budget per call: `budget` cycles (`BusTimes::write_budget`, T2 + T1 = 80 ms at the
/// T2 minimum) from the call, shared by the first attempt and all retries. `once` gets that
/// `Deadline` and every wait inside it (free-bus wait, per-event T8 waits, the whole-frame T1
/// bound) also ends at it, with the error of the step that was waiting (BusBusy for the free-bus
/// wait, StretchTimeout otherwise). One response then never holds the main loop longer than
/// T2 + T1, inside T5 (227 ms, VITA 46.11 Rule 4.1.1-11), whatever IPMB_ARB_RETRIES is.
pub fn write_retrying(
    bus: Bus,
    frame: &[u8],
    budget: u32,
    mut once: impl FnMut(Bus, &[u8], Deadline) -> Result<(), MasterError>,
) -> (Result<(), MasterError>, u32) {
    if frame.is_empty() {
        return (Err(MasterError::FrameLength), 0);
    }
    if bus == Bus::B && !B_FITTED {
        return (Err(MasterError::BusBusy), 0);
    }
    led::activity();
    let deadline = Deadline::from_now(budget);
    let mut retries = 0;
    loop {
        match once(bus, frame, deadline) {
            Err(MasterError::ArbitrationLost) if retries < ARB_RETRIES => retries += 1,
            // The budget ran out after this write lost arbitration: the bus is busy with
            // other masters, not faulty, so the result is ArbitrationLost and does not feed
            // the fault streak.
            Err(MasterError::BusBusy | MasterError::StretchTimeout)
                if retries > 0 && deadline.passed(now()) =>
            {
                return (Err(MasterError::ArbitrationLost), retries);
            }
            other => return (other, retries),
        }
    }
}
