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

use cortex_m::peripheral::{DCB, DWT, NVIC};
use ipmc_core::addr::{self, AddrInfo};
use ipmc_core::dispatch::Context as DispatchContext;
use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::link::{Link, LinkConfig};
use ipmc_core::queue::Bus;
use ipmc_core::sensor::{self, LocalStatus, SensorSnapshot};
use ipmc_core::trace::Trace;
use ipmc_menu::{Args, CmdResult};
use stm32f4::stm32f401::{I2C1, I2C3, RCC};

use crate::bsp::i2c_hw::{self, I2cError, I2cPeripheral, RxConsumer};
use crate::bsp::ipmb_master::{self, IpmbMaster, MasterError, Recovery, SelectedMaster};
use crate::bsp::ipmb_pins::IpmbPins;
use crate::bsp::{fault, ga, systick, uart};
use crate::console::{ConsoleTrace, Context};

/// RX_DRAIN_MAX -- most frames one `service` pass takes from the queues: both queues full at
/// once (2 x IPMB_RX_QUEUE_DEPTH). Frames that arrive during the pass wait for the next one.
const RX_DRAIN_MAX: usize = 2 * (ipmc_defs::IPMB_RX_QUEUE_DEPTH as usize);

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

    /// report -- one console line naming the failure of `bus`.
    fn report(self, bus: Bus) {
        match self {
            BringUpFailure::Bus(error) => out!(
                "IPMB-{}: bus not usable: {}; the bus stays disabled\n",
                bus.name(),
                error.name()
            ),
            BringUpFailure::Init(error) => out!(
                "IPMB-{}: I2C init failed: {}; the bus stays disabled\n",
                bus.name(),
                error.name()
            ),
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
                uart::flush();
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
                failure.report(Bus::A);
                fault_a = Some(failure);
            }
        }
        periph[tracker_index(Bus::A)] = a;
        if B_FITTED {
            let mut b = Some(I2cPeripheral::I3(i2c3));
            match bring_up(Bus::B, &mut b, &mut ctx, &mut master) {
                Ok(rx) => rx_b = Some(rx),
                Err(failure) => {
                    failure.report(Bus::B);
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
}

impl IpmbLink {
    /// take -- the one IpmbLink, in static storage (PLAN 3.3a); Some on the first call, None after.
    /// No interrupt handler touches it.
    /// `singleton!` builds the value on the stack and then copies it into the static (about
    /// 6.4 KB, mostly the duplicate cache). `inline(never)` keeps that temporary in this
    /// function's frame, which is gone when `take` returns at start-up; inlined into `main`, the
    /// copy would occupy `main`'s frame, and so the stack, for as long as the firmware runs.
    #[inline(never)]
    pub fn take(io: IpmbIo) -> Option<&'static mut IpmbLink> {
        cortex_m::singleton!(: IpmbLink = IpmbLink::new(io))
    }

    /// new -- a link in the power-up state (IPMB-A on, IPMB-B off) for the resolved address; a
    /// bus that failed its start-up gets its local fault, which disables it (Rule 9.3-10).
    fn new(io: IpmbIo) -> Self {
        let resolved = addr::resolve(ga::read_raw());
        let mut link = Link::new(LinkConfig::from_defs(), &resolved);
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
        IpmbLink {
            link,
            dispatch: DispatchContext::new(resolved),
            master: io.master,
            rx_a: io.rx_a,
            rx_b: io.rx_b,
            faults,
            bring_up: io.bring_up,
            periph: io.periph,
        }
    }

    /// dispatch -- the context the command handlers answer from: FRU image, SDR repository, FRU
    /// state, latest sensor snapshot. Read by the FRU and SENSORS console commands.
    pub fn dispatch(&self) -> &DispatchContext {
        &self.dispatch
    }

    /// link -- the link layer, for the console views of the bus state.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// feed_sensors -- hands the latest sensor sample to the command handlers and steps the FRU
    /// state machine with it (owner decision 3). The main loop calls it once per sample; a state
    /// change prints one console line (`Context::update_sensors` writes it).
    pub fn feed_sensors(&mut self, snapshot: &SensorSnapshot) {
        self.dispatch
            .update_sensors(systick::millis(), snapshot, &mut ConsoleTrace);
    }

    /// service -- main-loop step.
    /// 1. Samples NVMRO (one console line when it changes). The GA bits are not re-resolved: the
    ///    address is fixed after start-up.
    /// 2. Drains the receive queues into the link in STOP-time order.
    /// 3. Runs `Link::poll`.
    /// 4. Sends at most one queued response through the master (`transmit`).
    ///
    /// The bus-fault bookkeeping runs around step 4: `track_slave_faults` before it (frames
    /// received in this pass reset the streak, the writes then add to it) and
    /// `clear_faulted_buses` after it.
    pub fn service(&mut self) {
        let nvmro = addr::nvmro_asserted(ga::read_raw());
        if nvmro != self.dispatch.addr.nvmro_asserted {
            self.dispatch.addr.nvmro_asserted = nvmro;
            ConsoleTrace.line(format_args!(
                "NVMRO: {}",
                if nvmro { "asserted" } else { "deasserted" }
            ));
        }
        self.drain_rx();
        self.link
            .poll(&mut self.dispatch, systick::millis(), &mut ConsoleTrace);
        self.track_slave_faults();
        self.transmit();
        self.clear_faulted_buses();
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
            out!("IPMB-{} fault: {}\n", bus.name(), name);
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
                        out!("IPMB-{} fault clear failed: {}\n", bus.name(), error.name());
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
                out!("IPMB-{} fault cleared\n", bus.name());
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
                    failure.report(bus);
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
        if let Err(error) = result {
            trace_master_error(bus, error);
        }
        self.on_write_result(bus, result);
        let _ = self
            .link
            .on_tx_result(bus, result.is_ok(), systick::millis(), &mut ConsoleTrace);
    }
}

/// trace_master_error -- one console line for a failed master write, when IPMI tracing is on.
fn trace_master_error(bus: Bus, error: MasterError) {
    ConsoleTrace.line_when(
        ipmc_defs::IPMC_IPMI_TRACE == 1,
        format_args!("IPMB-{} master: {}", bus.name(), error.name()),
    );
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
        "IPMB-{} rx:    ok={} not-for-us={} response={} unavailable={} ingress-full={}\n",
        name,
        c.rx_ok,
        c.not_for_us,
        c.rx_response,
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
        "IPMB-{} tx:    ok={} fail={} fail-streak={} dropped={} resends={} replays={}\n",
        name,
        c.tx_ok,
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
