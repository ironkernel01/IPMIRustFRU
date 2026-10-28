// ipmb.rs - the IPMB master error names and counters, the I2C target's driver counters and the
// `IpmbBus` trait of a board's two buses.

use crate::frame::FrameSource;
use ipmc_core::link::BusSpeed;
use ipmc_core::queue::Bus;

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

/// DriverStats -- snapshot of one bus's driver counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriverStats {
    /// Frames posted to the queue.
    pub rx_frames: u32,
    /// Frames dropped for exceeding RX_FRAME_MAX.
    pub rx_overrun: u32,
    /// Frames dropped because the queue was full.
    pub rx_queue_full: u32,
    /// Address-only writes (no data byte): dropped.
    pub rx_empty: u32,
    /// Partial frames dropped by a repeated START.
    pub restart_drops: u32,
    /// Reads of this slave answered with FFh.
    pub slave_reads: u32,
    /// Slave reads that ended with the master's NACK (normal end).
    pub slave_read_end: u32,
    /// BERR: misplaced START or STOP.
    pub bus_error: u32,
    /// ARLO.
    pub arbitration_lost: u32,
    /// AF outside a slave read: a NACK that was not the end of one.
    pub ack_failure: u32,
    /// OVR.
    pub overrun: u32,
    /// TIMEOUT.
    pub timeout: u32,
    /// Runs of `clear_stuck_busy` (the BUSY-flag erratum workaround).
    pub busy_errata_resets: u32,
    /// BERR flags the hardware master cleared and ignored (ES0299 2.9.1).
    pub master_berr_spurious: u32,
}

/// BringUpFailure -- why a bus did not start; compared by value, so a repeat is not reported again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BringUpFailure {
    /// The bus recovery at start failed.
    Bus(MasterError),
    /// The target (slave) receiver did not start; carries the board's error name.
    Init(&'static str),
}

/// IpmbBus -- both IPMB buses and the board's master method, keyed by `Bus`.
pub trait IpmbBus {
    /// Rx -- the main-loop end of one bus's frame queue.
    type Rx: FrameSource;
    /// bring_up -- bus recovery if SDA is held low, then the target (slave) receiver at
    /// `own_addr8`. The one call for the first start and for every restart after a start-up
    /// fault.
    fn bring_up(&mut self, bus: Bus, own_addr8: u8)
        -> Result<(Self::Rx, Recovery), BringUpFailure>;
    /// write -- one master write (guards, arbitration retries and the T2 + T1 budget inside).
    fn write(&mut self, bus: Bus, frame: &[u8]) -> Result<(), MasterError>;
    /// set_speed -- applies `speed` between transfers; false = busy or not running, ask again.
    fn set_speed(&mut self, bus: Bus, speed: BusSpeed) -> bool;
    /// lines_high -- SCL and SDA both read high now.
    fn lines_high(&self, bus: Bus) -> bool;
    /// clear_bus_fault -- the stuck-BUSY recovery and a re-arm of the target.
    fn clear_bus_fault(&mut self, bus: Bus) -> Result<(), MasterError>;
    /// suspend -- takes a healthy bus off its target only when idle (false: busy or not running).
    fn suspend(&mut self, bus: Bus) -> bool;
    /// suspend_faulted -- the same without the idle check (false only when no target runs).
    fn suspend_faulted(&mut self, bus: Bus) -> bool;
    /// resume -- gives a suspended bus back to its target, armed as at start.
    fn resume(&mut self, bus: Bus);
    /// slave_stats -- the receive-side counters of `bus`.
    fn slave_stats(&self, bus: Bus) -> DriverStats;
    /// master_stats -- the master counters.
    fn master_stats(&self) -> &MasterStats;
}
