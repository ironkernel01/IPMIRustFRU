// bsp/ipmb_master.rs - the IPMB master interface: one trait, the error names and the counters
// every master method shares.
//
// Two implementations: the bit-bang master (bsp/i2c_bitbang.rs, `IPMB_MASTER_BITBANG`) and the
// hardware I2C master (bsp/i2c_hw.rs, `IPMB_MASTER_HW`). Both implement the same trait, so the
// main loop (ipmb_link.rs) is written once. `SelectedMaster` is the method ipmc-defs
// IPMB_MASTER_MODE chooses.

use cortex_m::peripheral::{DCB, DWT};
use ipmc_core::link::{BusSpeed, PerBus};
use ipmc_core::queue::Bus;

use super::i2c_bitbang::{self, Cycles};
use super::i2c_hw;
use super::ipmb_common::{self, Deadline, TimebaseError};
use super::ipmb_pins::{BusPins, IpmbPins};

/// Recovery -- outcome of `IpmbMaster::recover_if_stuck`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Both lines were high.
    NotNeeded,
    /// SDA was held low; the recovery clocks freed it.
    Recovered,
}

/// MasterError -- why a master write failed; `name` gives the text for a log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterError {
    /// No free bus before the write's time budget (T2 + T1) ran out.
    BusBusy,
    /// Another master has the bus: a released SDA read low, the hardware reported ARLO, or
    /// another master started in the moment after the free-bus check. The bus is released at
    /// once; the write is retried after a new free-bus wait (IPMB_ARB_RETRIES).
    ArbitrationLost,
    /// Nobody acknowledged the address byte.
    AddressNack,
    /// A data byte was not acknowledged (the transfer ended with a STOP).
    DataNack,
    /// SCL stayed low longer than IPMB_STRETCH_TIMEOUT_US after being released (VITA T8), a
    /// hardware event did not come within it, or the write ran past VITA T1 (20 ms).
    StretchTimeout,
    /// SCL still read low after the transfer ended: the STOP could not complete.
    SclStuckLow,
    /// SDA still read low after the transfer and the recovery clocks.
    SdaStuckLow,
    /// The frame is empty: there is no address byte to send.
    FrameLength,
    /// SCL read high although the recovery procedure drove it low (Table 10.2.2-1 code 3).
    SclStuckHigh,
    /// SDA read high although the recovery procedure drove it low (Table 10.2.2-1 code 4).
    SdaStuckHigh,
}

impl MasterError {
    /// COUNT -- number of variants; sizes the per-kind counters.
    pub const COUNT: usize = 10;

    /// index -- position in the per-kind counters (0..COUNT).
    pub const fn index(self) -> usize {
        match self {
            MasterError::BusBusy => 0,
            MasterError::ArbitrationLost => 1,
            MasterError::AddressNack => 2,
            MasterError::DataNack => 3,
            MasterError::StretchTimeout => 4,
            MasterError::SclStuckLow => 5,
            MasterError::SdaStuckLow => 6,
            MasterError::FrameLength => 7,
            MasterError::SclStuckHigh => 8,
            MasterError::SdaStuckHigh => 9,
        }
    }

    /// name -- text for log lines.
    pub const fn name(self) -> &'static str {
        match self {
            MasterError::BusBusy => "bus busy",
            MasterError::ArbitrationLost => "arbitration lost",
            MasterError::AddressNack => "address not acknowledged",
            MasterError::DataNack => "data not acknowledged",
            MasterError::StretchTimeout => "clock stretch timeout",
            MasterError::SclStuckLow => "SCL stuck low",
            MasterError::SdaStuckLow => "SDA stuck low",
            MasterError::FrameLength => "empty frame",
            MasterError::SclStuckHigh => "SCL stuck high",
            MasterError::SdaStuckHigh => "SDA stuck high",
        }
    }
}

/// BusStats -- master counters of one bus. All wrap-free in practice (u32 saturating adds).
#[derive(Clone, Copy, Debug, Default)]
pub struct BusStats {
    /// Writes that completed with every byte acknowledged.
    pub writes_ok: u32,
    /// Failed writes by kind, indexed by `MasterError::index`.
    pub errors: [u32; MasterError::COUNT],
    /// Arbitration losses retried on the same bus (IPMB_ARB_RETRIES).
    pub arb_retries: u32,
}

/// MasterStats -- the counters of both buses plus the timing of the latest write.
#[derive(Clone, Copy, Debug, Default)]
pub struct MasterStats {
    pub a: BusStats,
    pub b: BusStats,
    /// Core clock cycles from the first SCL fall to the last ACK clock of the latest completed
    /// write, 0 before the first.
    pub last_bit_cycles: u32,
    /// Clock bits in `last_bit_cycles` (9 per byte, address byte included).
    pub last_bits: u32,
    /// Core clock in Hz, to turn cycles into time.
    pub core_hz: u32,
}

impl MasterStats {
    /// new -- zero counters for a master that runs at `core_hz`.
    pub fn new(core_hz: u32) -> Self {
        MasterStats {
            core_hz,
            ..MasterStats::default()
        }
    }

    /// bus -- the counters of `bus`.
    pub fn bus(&self, bus: Bus) -> &BusStats {
        match bus {
            Bus::A => &self.a,
            Bus::B => &self.b,
        }
    }

    /// record -- books the outcome of one `write_retrying` on `bus`.
    pub fn record(&mut self, bus: Bus, result: Result<(), MasterError>, arb_retries: u32) {
        let c = self.bus_mut(bus);
        c.arb_retries = c.arb_retries.saturating_add(arb_retries);
        match result {
            Ok(()) => c.writes_ok = c.writes_ok.saturating_add(1),
            Err(error) => {
                if let Some(count) = c.errors.get_mut(error.index()) {
                    *count = count.saturating_add(1);
                }
            }
        }
    }

    /// bus_mut -- the counters of `bus`, for the master that counts.
    pub fn bus_mut(&mut self, bus: Bus) -> &mut BusStats {
        match bus {
            Bus::A => &mut self.a,
            Bus::B => &mut self.b,
        }
    }

    /// scl_period_ns -- mean SCL period of the latest write in nanoseconds; None before the
    /// first write.
    pub fn scl_period_ns(&self) -> Option<u32> {
        if self.last_bits == 0 || self.core_hz == 0 {
            return None;
        }
        let ns = u64::from(self.last_bit_cycles) * 1_000_000_000
            / u64::from(self.core_hz)
            / u64::from(self.last_bits);
        u32::try_from(ns).ok()
    }
}

/// MasterState -- what every master method holds: the pins of both buses, the wait times in core
/// clock cycles and the counters. The bit-bang master and the hardware master are each this state
/// plus their own `write_once`; `IpmbMaster` reaches it through `state`.
pub struct MasterState {
    pub(super) pins: IpmbPins,
    pub(super) cy: Cycles,
    pub(super) stats: MasterStats,
    /// The speed each bus is applied at (`IpmbMaster::set_speed`); both buses start at 100 kHz
    /// (VITA 46.11 Rule 4.1.1-4.3).
    pub(super) speeds: PerBus<BusSpeed>,
}

impl MasterState {
    /// new -- takes the IPMB pins, enables the DWT cycle counter (`start_timebase`) and converts
    /// the times of ipmc-defs to cycles of `core_hz`. Err when the clock does not give usable
    /// cycle counts or the counter does not run.
    pub fn new(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self, TimebaseError> {
        let cy = ipmb_common::start_timebase(dcb, dwt, core_hz, Cycles::new)?;
        Ok(MasterState {
            pins,
            cy,
            stats: MasterStats::new(core_hz),
            speeds: PerBus {
                a: BusSpeed::Standard,
                b: BusSpeed::Standard,
            },
        })
    }
}

/// IpmbMaster -- one master write on an IPMB.
pub trait IpmbMaster {
    /// state -- the pins, times and counters of this master.
    fn state(&self) -> &MasterState;

    /// state_mut -- the same, for the counting and the transfer.
    fn state_mut(&mut self) -> &mut MasterState;

    /// write_once -- one attempt to send `frame` on `bus`, by this master's method. Every wait
    /// in it also ends at `deadline`, the one shared by all attempts of a `write`.
    fn write_once(&mut self, bus: Bus, frame: &[u8], deadline: Deadline)
        -> Result<(), MasterError>;

    /// stats -- the master counters.
    fn stats(&self) -> &MasterStats {
        &self.state().stats
    }

    /// write -- sends `frame` as one master transmission on `bus`. `frame[0]` is the destination
    /// 8-bit address byte (the address with write bit 0); the master sends it as the address
    /// byte and `frame[1..]` as data, then a STOP. The guards and the arbitration retries are
    /// `write_retrying`; every master method shares this one wrapper around its `write_once`.
    /// The whole call, retries included, takes at most T2 + T1 (80 ms at the T2 minimum).
    /// Ok: every byte was acknowledged. Err: the `MasterError`; the bus is released and the
    /// slave side re-armed in every case.
    fn write(&mut self, bus: Bus, frame: &[u8]) -> Result<(), MasterError> {
        let budget = self.state().cy.times().write_budget;
        let (result, retries) =
            ipmb_common::write_retrying(bus, frame, budget, |b, f, d| self.write_once(b, f, d));
        self.state_mut().stats.record(bus, result, retries);
        result
    }

    /// speed -- the speed `bus` is applied at now.
    fn speed(&self, bus: Bus) -> BusSpeed {
        *self.state().speeds.get(bus)
    }

    /// set_speed -- applies `speed` to `bus` between transfers (the main loop calls it from
    /// `service`, never inside a write): the I2C peripheral takes the timing of the speed
    /// (`i2c_hw::set_speed`, only while its slave is idle) and the master write uses it from the
    /// next frame on. False when the slave was busy or is not running: nothing changed, the
    /// caller asks again on a later pass. True when `speed` is applied (also when it already
    /// was).
    fn set_speed(&mut self, bus: Bus, speed: BusSpeed) -> bool {
        if self.speed(bus) == speed {
            return true;
        }
        if !i2c_hw::set_speed(bus, self.pins(bus), speed) {
            return false;
        }
        *self.state_mut().speeds.get_mut(bus) = speed;
        true
    }

    /// pins -- the pin handles of `bus` (levels are read through them; the alternate function is
    /// selected through them by the I2C driver).
    fn pins(&self, bus: Bus) -> &BusPins {
        self.state().pins.bus(bus)
    }

    /// recover_if_stuck -- before the I2C slave is enabled: bus recovery per i2c-bitbang.md
    /// sections 3 and 5 (up to 9 SCL pulses as open-drain GPIO, then a STOP).
    /// Ok(NotNeeded): both lines high. Ok(Recovered): SDA was low and is free. Err(SclStuckLow),
    /// Err(SdaStuckLow): a line is held low.
    fn recover_if_stuck(&mut self, bus: Bus) -> Result<Recovery, MasterError> {
        let state = self.state();
        i2c_bitbang::recover_if_stuck(state.pins.bus(bus), state.cy)
    }

    /// lines_high -- both lines of `bus` read high now (the evidence the fault clear waits for).
    fn lines_high(&self, bus: Bus) -> bool {
        ipmb_common::lines_high(self.pins(bus))
    }

    /// clear_bus_fault -- the ST BUSY workaround on `bus` and a re-arm of the slave
    /// (`i2c_hw::clear_stuck_busy`). Err: the line that could not be driven.
    fn clear_bus_fault(&mut self, bus: Bus) -> Result<(), MasterError> {
        i2c_hw::clear_stuck_busy(bus, self.pins(bus)).map_err(i2c_hw::StuckLine::master_error)
    }
}

/// SelectedMaster -- the master method of ipmc-defs IPMB_MASTER_MODE.
pub type SelectedMaster = <Select<{ BITBANG }> as MasterFor>::Master;

/// BITBANG -- whether IPMB_MASTER_MODE is the bit-bang method (false = the hardware master).
const BITBANG: bool = matches!(ipmc_defs::IPMB_MASTER_MODE, ipmc_defs::MasterMode::BitBang);

/// Select / MasterFor -- compile-time map from IPMB_MASTER_MODE (`BITBANG`) to the master type.
pub struct Select<const BITBANG_METHOD: bool>;

pub trait MasterFor {
    type Master;

    /// build -- the master of this method: takes the pins and the DWT cycle counter.
    fn build(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self::Master, TimebaseError>;
}

/// new_selected -- builds the master of IPMB_MASTER_MODE.
pub fn new_selected(
    dcb: &mut DCB,
    dwt: &mut DWT,
    core_hz: u32,
    pins: IpmbPins,
) -> Result<SelectedMaster, TimebaseError> {
    <Select<{ BITBANG }> as MasterFor>::build(dcb, dwt, core_hz, pins)
}

impl MasterFor for Select<true> {
    type Master = i2c_bitbang::BitbangMaster;

    fn build(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self::Master, TimebaseError> {
        i2c_bitbang::BitbangMaster::new(dcb, dwt, core_hz, pins)
    }
}

impl MasterFor for Select<false> {
    type Master = i2c_hw::HwMaster;

    fn build(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self::Master, TimebaseError> {
        i2c_hw::HwMaster::new(dcb, dwt, core_hz, pins)
    }
}
