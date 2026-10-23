// bsp/i2c_hw.rs - the IPMB I2C slave: I2C1 (IPMB-A) and I2C3 (IPMB-B) in I2C slave mode, receive
// interrupts and the per-bus frame queues.
//
// Ownership: this module owns the I2C1 and I2C3 registers and their RCC clock enables and
// resets; the peripherals are moved in at `init` and never handed out. It does not own the pins:
// the pin claims stay with bsp/ipmb_pins.rs / bsp/i2c_bitbang.rs, which lend a `BusPins` to
// `init` and `resume` so this module can select the alternate function.
// One implementation serves both buses (I2C1 and I2C3 share the i2c1::RegisterBlock layout);
// everything per bus is selected by `Bus`: the state static, the queue, the counters, the
// interrupt numbers.
//
// Receive path (PLAN 3.3a): the EV interrupt appends the bytes of one START..STOP frame to a
// buffer and, at STOP, posts it with the SysTick time to that bus's heapless SPSC queue; the
// main loop (ipmb_link.rs) drains both queues. The interrupt does no formatting and never
// touches the Link.
// Master hand-off: the bit-bang master calls `slave_idle`, `suspend` and `resume` around a
// transmission (PE = 0 while it drives the pins). The hardware master (`HwMaster`, at the end of
// this file) keeps PE = 1 and masks the bus's EV and ER interrupts for the length of a write.
// Stuck BUSY flag: `clear_stuck_busy` is the one implementation of the ST bus-reset sequence (a
// defensive recovery; the "analog filter locks BUSY" erratum is documented for other STM32
// families and is not confirmed in ES0299); both masters use it.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::interrupt::free;
use cortex_m::peripheral::NVIC;
use cortex_m::peripheral::{DCB, DWT};
use heapless::spsc::{Consumer, Producer, Queue};
use ipmc_core::queue::Bus;
use ipmc_sensors::drivers::stm32f401::ports::{Pins, Pull, Speed};
use stm32f4::stm32f401::{i2c1::RegisterBlock, interrupt, Interrupt, I2C1, I2C3, RCC};

use super::ipmb_common::{
    lines_high, now, wait_bus_free, wait_for, BusTimes, Deadline, TimebaseError,
};
use super::ipmb_master::{IpmbMaster, MasterError, MasterState};
use super::ipmb_pins::{BusPins, IpmbPins};
use super::static_slot::{self, StaticSlot};
use super::{led, systick};

// ---- sizes, from ipmi_defs.h ----

/// GENERAL_CALL -- the address byte a broadcast frame starts with (IPMI v2.0 section 20.9). Peer:
/// ipmc-core link.rs GENERAL_CALL strips it; a different value here makes Broadcast Get Device
/// ID answer nothing.
const GENERAL_CALL: u8 = 0x00;

/// RX_FRAME_MAX -- bytes of one received frame as the interrupt stores it: the IPMB frame
/// maximum (IPMB_MSG_MAX, the whole frame from rsSA to the second checksum) plus one for the
/// leading address byte, which the interrupt prepends because the hardware consumes the address
/// phase (own address byte, or 00h for a general call). A broadcast frame is then 00h followed
/// by a normal frame, the same length as a frame that carries its own address (IPMB_MSG_MAX)
/// plus the general-call byte in front. A longer frame is an overrun and is dropped.
pub const RX_FRAME_MAX: usize = ipmc_defs::IPMB_MSG_MAX as usize + 1;
const _: () = assert!(RX_FRAME_MAX <= 255, "RxFrame::len is a u8");

/// RX_QUEUE_LEN -- heapless::spsc::Queue<_, N> holds N - 1 frames (one slot separates a full
/// queue from an empty one), so N = IPMB_RX_QUEUE_DEPTH + 1 holds DEPTH frames.
pub const RX_QUEUE_LEN: usize = ipmc_defs::IPMB_RX_QUEUE_DEPTH as usize + 1;

/// RxFrame -- one received frame as posted by the interrupt: `bytes[..len]`, STOP time in
/// SysTick milliseconds.
#[derive(Clone, Copy)]
pub struct RxFrame {
    pub bytes: [u8; RX_FRAME_MAX],
    pub len: u8,
    pub stop_ms: u32,
}

impl RxFrame {
    const EMPTY: RxFrame = RxFrame {
        bytes: [0; RX_FRAME_MAX],
        len: 0,
        stop_ms: 0,
    };

    /// data -- the received bytes.
    pub fn data(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

type RxQueue = Queue<RxFrame, RX_QUEUE_LEN>;
/// RxProducer -- the interrupt's end of a bus queue.
type RxProducer = Producer<'static, RxFrame, RX_QUEUE_LEN>;
/// RxConsumer -- the main loop's end of a bus queue.
pub type RxConsumer = Consumer<'static, RxFrame, RX_QUEUE_LEN>;

// ---- register encodings (RM0368 chapter 18, I2C) ----

/// SR1 bits (RM0368 18.6.6).
const SR1_SB: u16 = 1 << 0;
const SR1_ADDR: u16 = 1 << 1;
const SR1_BTF: u16 = 1 << 2;
const SR1_STOPF: u16 = 1 << 4;
const SR1_RXNE: u16 = 1 << 6;
const SR1_TXE: u16 = 1 << 7;
const SR1_BERR: u16 = 1 << 8;
const SR1_ARLO: u16 = 1 << 9;
const SR1_AF: u16 = 1 << 10;
const SR1_OVR: u16 = 1 << 11;
const SR1_PECERR: u16 = 1 << 12;
const SR1_TIMEOUT: u16 = 1 << 14;
const SR1_SMBALERT: u16 = 1 << 15;
/// The SR1 error flags, all cleared by writing 0 to them (RM0368 18.6.6, rc_w0).
const SR1_ERROR_FLAGS: u16 =
    SR1_BERR | SR1_ARLO | SR1_AF | SR1_OVR | SR1_PECERR | SR1_TIMEOUT | SR1_SMBALERT;
/// SR2 bits (RM0368 18.6.7).
const SR2_MSL: u16 = 1 << 0;
const SR2_BUSY: u16 = 1 << 1;
const SR2_TRA: u16 = 1 << 2;
const SR2_GENCALL: u16 = 1 << 4;
/// OAR1 bit 14 is kept at 1 by software (RM0368 18.6.3).
const OAR1_KEEP: u16 = 1 << 14;
/// Byte a slave read returns: nothing is served on IPMB reads, so the bus reads 1s.
const READ_FILL: u8 = 0xFF;

/// FREQ field limits (RM0368 18.6.2): standard mode needs at least 2 MHz; the F401 APB1 runs at
/// most 42 MHz.
const FREQ_MIN_MHZ: u32 = 2;
const FREQ_MAX_MHZ: u32 = 42;
/// CCR field: 12 bits, at least 4 in standard mode (RM0368 18.6.8).
const CCR_MIN: u32 = 4;
const CCR_MAX: u32 = 0xFFF;
/// TRISE field: 6 bits (RM0368 18.6.9).
const TRISE_MAX: u32 = 0x3F;
/// Standard mode ceiling on SCL rise time is 1000 ns, so TRISE = FREQ(MHz) + 1 (RM0368 18.6.9).
const TRISE_OVER_FREQ: u32 = 1;
/// Hz in one MHz.
const HZ_PER_MHZ: u32 = 1_000_000;
/// Usable 7-bit slave addresses: 0x00..0x07 and 0x78..0x7F are reserved by the I2C-bus
/// specification.
const ADDR7_MIN: u8 = 0x08;
const ADDR7_MAX: u8 = 0x77;

/// NVIC priority byte of the I2C interrupts (upper bits are significant on this device). One
/// step above USART2 (2 << 4): a slow slave handler stretches SCL, and the VITA 46.11 T8 limit
/// on a stretched clock is 3 ms.
const I2C_PRIORITY: u8 = 1 << 4;

/// Alternate-function numbers (DS10086 Table 9): (SCL, SDA) per bus. Peer: bsp/ipmb_pins.rs
/// chooses the pins; PB8, PB9 and PA8 are AF4, PB4 is AF9. A mismatch leaves the bus silent.
const fn pin_afs(bus: Bus) -> (u8, u8) {
    match bus {
        Bus::A => (4, 4),
        Bus::B => (4, 9),
    }
}

// ---- errors ----

/// I2cError -- why `init` refused; `name` gives the text for a log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum I2cError {
    /// The peripheral handed in is not the one of the bus (A is I2C1, B is I2C3).
    WrongPeripheral,
    /// `init` already ran for this bus.
    AlreadyInitialised,
    /// The 8-bit address is odd, or its 7-bit form is a reserved I2C address.
    OwnAddress,
    /// PCLK1 is not a whole number of MHz (CR2.FREQ counts MHz).
    PclkNotWholeMhz,
    /// PCLK1 is outside the 2..=42 MHz FREQ range.
    PclkRange,
    /// The CCR divisor does not fit the register.
    ClockDivisor,
    /// TRISE does not fit the register.
    TriseRange,
    /// The bus frame queue was already taken.
    QueueTaken,
}

impl I2cError {
    /// name -- text for log lines.
    pub const fn name(self) -> &'static str {
        match self {
            I2cError::WrongPeripheral => "wrong I2C peripheral for the bus",
            I2cError::AlreadyInitialised => "I2C already initialised",
            I2cError::OwnAddress => "own IPMB address not usable",
            I2cError::PclkNotWholeMhz => "PCLK1 is not a whole number of MHz",
            I2cError::PclkRange => "PCLK1 outside 2..=42 MHz",
            I2cError::ClockDivisor => "SCL divisor out of range",
            I2cError::TriseRange => "TRISE out of range",
            I2cError::QueueTaken => "receive queue already taken",
        }
    }
}

/// I2cPeripheral -- the I2C register block handed to `init`.
pub enum I2cPeripheral {
    /// I2C1, IPMB-A.
    I1(I2C1),
    /// I2C3, IPMB-B.
    I3(I2C3),
}

impl I2cPeripheral {
    fn regs(&self) -> &RegisterBlock {
        match self {
            I2cPeripheral::I1(p) => p,
            I2cPeripheral::I3(p) => p,
        }
    }

    const fn matches(&self, bus: Bus) -> bool {
        matches!(
            (self, bus),
            (I2cPeripheral::I1(_), Bus::A) | (I2cPeripheral::I3(_), Bus::B)
        )
    }
}

// ---- timing ----

/// Timing -- the register values computed from PCLK1 and the own address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timing {
    /// CR2.FREQ, PCLK1 in MHz.
    freq_mhz: u32,
    /// CCR.CCR, standard mode.
    ccr: u32,
    /// TRISE.TRISE.
    trise: u32,
    /// OAR1, 7-bit addressing: ADD[7:1] = the 7-bit address, bit 14 kept at 1.
    oar1: u16,
}

/// compute -- the register values for `pclk1_hz` and the slave address `own_addr8` (IPMI 8-bit
/// form: the 7-bit address in bits 7:1, bit 0 clear). Every value is range checked; hardware
/// free.
/// 1. FREQ = PCLK1 in MHz (RM0368 18.6.2).
/// 2. CCR = ceil(PCLK1 / (2 x IPMB_I2C_HZ)) in standard mode (RM0368 18.6.8): SCL low and high
///    last CCR x TPCLK1 each; rounding up keeps SCL at or below IPMB_I2C_HZ.
/// 3. TRISE = FREQ + 1 for the 1000 ns standard-mode rise time (RM0368 18.6.9).
/// 4. OAR1 = (own_addr8 >> 1) << 1 | bit 14 (RM0368 18.6.3); the conversion from the 8-bit form
///    to the 7-bit address happens here and only here.
fn compute(pclk1_hz: u32, own_addr8: u8) -> Result<Timing, I2cError> {
    if own_addr8 & 1 != 0 {
        return Err(I2cError::OwnAddress);
    }
    let addr7 = own_addr8 >> 1;
    if !(ADDR7_MIN..=ADDR7_MAX).contains(&addr7) {
        return Err(I2cError::OwnAddress);
    }
    if !pclk1_hz.is_multiple_of(HZ_PER_MHZ) {
        return Err(I2cError::PclkNotWholeMhz);
    }
    let freq_mhz = pclk1_hz / HZ_PER_MHZ;
    if !(FREQ_MIN_MHZ..=FREQ_MAX_MHZ).contains(&freq_mhz) {
        return Err(I2cError::PclkRange);
    }
    let scl_hz2 = ipmc_defs::IPMB_I2C_HZ
        .checked_mul(2)
        .ok_or(I2cError::ClockDivisor)?;
    let ccr = pclk1_hz.div_ceil(scl_hz2);
    if !(CCR_MIN..=CCR_MAX).contains(&ccr) {
        return Err(I2cError::ClockDivisor);
    }
    let trise = freq_mhz + TRISE_OVER_FREQ;
    if trise > TRISE_MAX {
        return Err(I2cError::TriseRange);
    }
    Ok(Timing {
        freq_mhz,
        ccr,
        trise,
        oar1: (u16::from(addr7) << 1) | OAR1_KEEP,
    })
}

// ---- counters ----

/// DriverCounters -- per-bus driver counters. Incremented by the interrupts, read by `stats`
/// from the main loop: independent single-word counts on a single-core part, so atomics with
/// Relaxed ordering. They wrap at 2^32 (about 4.3 billion events; at the IPMB frame rate the
/// wrap is not reachable in the life of a build).
struct DriverCounters {
    rx_frames: AtomicU32,
    rx_overrun: AtomicU32,
    rx_queue_full: AtomicU32,
    rx_empty: AtomicU32,
    restart_drops: AtomicU32,
    slave_reads: AtomicU32,
    slave_read_end: AtomicU32,
    bus_error: AtomicU32,
    arbitration_lost: AtomicU32,
    ack_failure: AtomicU32,
    overrun: AtomicU32,
    timeout: AtomicU32,
    busy_errata_resets: AtomicU32,
    master_berr_spurious: AtomicU32,
}

impl DriverCounters {
    const fn new() -> Self {
        DriverCounters {
            rx_frames: AtomicU32::new(0),
            rx_overrun: AtomicU32::new(0),
            rx_queue_full: AtomicU32::new(0),
            rx_empty: AtomicU32::new(0),
            restart_drops: AtomicU32::new(0),
            slave_reads: AtomicU32::new(0),
            slave_read_end: AtomicU32::new(0),
            bus_error: AtomicU32::new(0),
            arbitration_lost: AtomicU32::new(0),
            ack_failure: AtomicU32::new(0),
            overrun: AtomicU32::new(0),
            timeout: AtomicU32::new(0),
            busy_errata_resets: AtomicU32::new(0),
            master_berr_spurious: AtomicU32::new(0),
        }
    }
}

fn bump(counter: &AtomicU32) {
    counter.fetch_add(1, Ordering::Relaxed);
}

static COUNTERS_A: DriverCounters = DriverCounters::new();
static COUNTERS_B: DriverCounters = DriverCounters::new();

fn counters(bus: Bus) -> &'static DriverCounters {
    match bus {
        Bus::A => &COUNTERS_A,
        Bus::B => &COUNTERS_B,
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

/// stats -- the driver counters of `bus`.
pub fn stats(bus: Bus) -> DriverStats {
    let c = counters(bus);
    let get = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
    DriverStats {
        rx_frames: get(&c.rx_frames),
        rx_overrun: get(&c.rx_overrun),
        rx_queue_full: get(&c.rx_queue_full),
        rx_empty: get(&c.rx_empty),
        restart_drops: get(&c.restart_drops),
        slave_reads: get(&c.slave_reads),
        slave_read_end: get(&c.slave_read_end),
        bus_error: get(&c.bus_error),
        arbitration_lost: get(&c.arbitration_lost),
        ack_failure: get(&c.ack_failure),
        overrun: get(&c.overrun),
        timeout: get(&c.timeout),
        busy_errata_resets: get(&c.busy_errata_resets),
        master_berr_spurious: get(&c.master_berr_spurious),
    }
}

// ---- slave state ----

/// Phase -- where the slave is in a transfer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Between frames.
    Idle,
    /// Address matched with the write bit: bytes go into the frame.
    Receiving,
    /// Address matched with the read bit: the slave answers FFh until the master NACKs.
    Transmitting,
}

/// RxState -- the frame being received and the producer end of the bus queue.
struct RxState {
    own_addr8: u8,
    producer: RxProducer,
    frame: RxFrame,
    overrun: bool,
    phase: Phase,
}

impl RxState {
    fn clear_frame(&mut self) {
        self.frame.len = 0;
        self.overrun = false;
    }

    /// push -- appends one byte; beyond RX_FRAME_MAX the byte is dropped and the frame marked
    /// overrun (the reads of DR go on so the bus is not held).
    fn push(&mut self, byte: u8) {
        match self.frame.bytes.get_mut(usize::from(self.frame.len)) {
            Some(slot) => {
                *slot = byte;
                self.frame.len += 1;
            }
            None => self.overrun = true,
        }
    }

    /// on_address -- an address matched (SR2 has been read). A write (SR2.TRA = 0) starts a frame
    /// whose first byte is the address that matched: 00h for a general call (SR2.GENCALL), else
    /// the own 8-bit address. A read (TRA = 1) starts a transmission of FFh bytes.
    fn on_address(&mut self, sr2: u16, c: &DriverCounters) {
        if self.phase == Phase::Receiving && self.frame.len > 0 {
            bump(&c.restart_drops); // a repeated START ended a frame without a STOP
        }
        self.clear_frame();
        if sr2 & SR2_TRA != 0 {
            self.phase = Phase::Transmitting;
            bump(&c.slave_reads);
        } else {
            self.phase = Phase::Receiving;
            let first = if sr2 & SR2_GENCALL != 0 {
                GENERAL_CALL
            } else {
                self.own_addr8
            };
            self.push(first);
        }
    }

    /// on_byte -- a received byte (DR read).
    fn on_byte(&mut self, byte: u8) {
        if self.phase == Phase::Receiving {
            self.push(byte);
        }
    }

    /// on_stop -- STOP after a write: posts the frame unless it overran or is address-only.
    fn on_stop(&mut self, c: &DriverCounters) {
        if self.phase == Phase::Receiving {
            if self.overrun {
                bump(&c.rx_overrun);
            } else if self.frame.len <= 1 {
                bump(&c.rx_empty);
            } else {
                self.frame.stop_ms = systick::millis();
                match self.producer.enqueue(self.frame) {
                    Ok(()) => bump(&c.rx_frames),
                    Err(_) => bump(&c.rx_queue_full),
                }
                led::activity();
            }
        }
        self.phase = Phase::Idle;
        self.clear_frame();
    }

    /// on_errors -- SR1 error flags `flags`: counted per kind; a slave-side flag drops the partial
    /// frame and returns the slave to idle. AF at the end of a slave read is the normal end
    /// (RM0368 18.3.3, slave transmitter), counted separately. ARLO alone is only counted: it
    /// reports the hardware master losing arbitration (RM0368 18.6.6, ARLO), after which the
    /// peripheral may already be receiving the winner's frame addressed to this slave, and that
    /// frame is kept.
    fn on_errors(&mut self, flags: u16, c: &DriverCounters) {
        if flags & SR1_BERR != 0 {
            bump(&c.bus_error);
        }
        if flags & SR1_ARLO != 0 {
            bump(&c.arbitration_lost);
        }
        if flags & SR1_AF != 0 {
            if self.phase == Phase::Transmitting {
                bump(&c.slave_read_end);
            } else {
                bump(&c.ack_failure);
            }
        }
        if flags & SR1_OVR != 0 {
            bump(&c.overrun);
        }
        if flags & SR1_TIMEOUT != 0 {
            bump(&c.timeout);
        }
        if flags & !SR1_ARLO != 0 {
            self.phase = Phase::Idle;
            self.clear_frame();
        }
    }
}

/// Slave -- one bus's peripheral, timing and receive state.
struct Slave {
    hw: I2cPeripheral,
    timing: Timing,
    rx: RxState,
}

// Shared state, one per bus. Written by `init` (thread, once, before the interrupts are
// unmasked), by `suspend` and `resume` (thread, the bit-bang master) and by that bus's EV and ER
// interrupts. Every access runs inside a critical section, so the interrupts and the thread
// never overlap; the critical sections are short (no bus transfer, no formatting).
static SLAVE_A: StaticSlot<Slave> = static_slot::empty();
static SLAVE_B: StaticSlot<Slave> = static_slot::empty();

fn slot(bus: Bus) -> &'static StaticSlot<Slave> {
    match bus {
        Bus::A => &SLAVE_A,
        Bus::B => &SLAVE_B,
    }
}

const fn irqs(bus: Bus) -> (Interrupt, Interrupt) {
    match bus {
        Bus::A => (Interrupt::I2C1_EV, Interrupt::I2C1_ER),
        Bus::B => (Interrupt::I2C3_EV, Interrupt::I2C3_ER),
    }
}

/// take_queue -- the bus's frame queue in static storage. One `singleton!` expansion per bus: a
/// second call for the same bus returns None.
fn take_queue(bus: Bus) -> Option<&'static mut RxQueue> {
    match bus {
        Bus::A => cortex_m::singleton!(: RxQueue = Queue::new()),
        Bus::B => cortex_m::singleton!(: RxQueue = Queue::new()),
    }
}

/// pins_to_af -- SCL and SDA to the alternate function of the I2C peripheral, open-drain, no pull
/// (VITA Rule 9.2.3-2).
fn pins_to_af(bus: Bus, pins: &BusPins) {
    let (scl_af, sda_af) = pin_afs(bus);
    pins.scl.alternate(scl_af, Pull::None, true, Speed::Medium);
    pins.sda.alternate(sda_af, Pull::None, true, Speed::Medium);
}

/// arm -- puts the pins on the peripheral and (re)configures it as the IPMB slave, then unmasks
/// its interrupts. The one implementation `init` and `resume` share.
/// 1. SCL and SDA to the alternate function, open-drain, no pull (VITA Rule 9.2.3-2).
/// 2. PE = 0, then CR2.FREQ, CCR (standard mode), TRISE and OAR1: CCR and TRISE are written only
///    while PE = 0 (RM0368 18.6.8, 18.6.9).
/// 3. CR1: ENGC = 1 (general call, Broadcast Get Device ID) and PE = 1; then ACK = 1, after PE
///    (RM0368 18.6.1).
/// 4. CR2: ITEVTEN, ITERREN and ITBUFEN (RM0368 18.6.2).
/// 5. The receive state returns to idle; pending NVIC flags from before are cleared, then the EV
///    and ER interrupts are unmasked. Every caller runs `arm` inside a critical section that
///    installs (or already holds) the bus state, so no handler runs before the state exists.
fn arm(bus: Bus, slave: &mut Slave, pins: &BusPins) {
    pins_to_af(bus, pins);
    let regs = slave.hw.regs();
    let t = slave.timing;
    regs.cr1().write(|w| w.pe().disabled());
    // SAFETY: FREQ holds the PCLK1 MHz value range-checked by `compute`; the other bits are 0.
    regs.cr2().write(|w| unsafe { w.bits(t.freq_mhz as u16) });
    // SAFETY: CCR holds the 12-bit divisor range-checked by `compute`; F/S = 0 (standard mode)
    // and DUTY = 0.
    regs.ccr().write(|w| unsafe { w.bits(t.ccr as u16) });
    // SAFETY: TRISE holds the 6-bit value range-checked by `compute`.
    regs.trise().write(|w| unsafe { w.bits(t.trise as u16) });
    // SAFETY: OAR1 holds ADD[7:1] and bit 14 as built by `compute`; ADDMODE = 0 (7-bit).
    regs.oar1().write(|w| unsafe { w.bits(t.oar1) });
    regs.cr1().write(|w| w.engc().enabled().pe().enabled());
    regs.cr1().modify(|_, w| w.ack().ack());
    regs.cr2().modify(|_, w| {
        w.itevten()
            .enabled()
            .iterren()
            .enabled()
            .itbufen()
            .enabled()
    });

    slave.rx.phase = Phase::Idle;
    slave.rx.clear_frame();
    let (ev, er) = irqs(bus);
    NVIC::unpend(ev);
    NVIC::unpend(er);
    // SAFETY: unmasking is sound because the caller's critical section ends only after the bus
    // state is in its static, so a handler finds it (populate, then unmask).
    unsafe {
        NVIC::unmask(ev);
        NVIC::unmask(er);
    }
}

/// init -- brings up the I2C slave of `bus` at `own_addr8`.
/// 1. Checks the peripheral matches the bus and computes the timing (`compute`).
/// 2. RCC: enables the peripheral clock and pulses its reset (RM0368 6.3.8, 6.3.14), so no state
///    from before survives.
/// 3. Takes the bus frame queue; the producer goes to the interrupt state, the consumer is
///    returned for the main loop.
/// 4. Installs the state, sets the NVIC priorities, then `arm`.
///
/// Bus recovery (SDA held low by a slave) is the caller's step before this one
/// (`i2c_bitbang::BitbangMaster::recover_if_stuck`): the open-drain GPIO primitives live there.
/// Err: `I2cError`; nothing is enabled then and `periph` still holds the peripheral, so the caller
/// can call `init` again (a start-up fault that clears). Ok: `periph` is None, the driver owns it.
pub fn init(
    bus: Bus,
    periph: &mut Option<I2cPeripheral>,
    rcc: &RCC,
    nvic: &mut NVIC,
    pins: &BusPins,
    pclk1_hz: u32,
    own_addr8: u8,
) -> Result<RxConsumer, I2cError> {
    if !periph.as_ref().is_some_and(|p| p.matches(bus)) {
        return Err(I2cError::WrongPeripheral);
    }
    let timing = compute(pclk1_hz, own_addr8)?;
    if free(|cs| slot(bus).borrow(cs).borrow().is_some()) {
        return Err(I2cError::AlreadyInitialised);
    }
    let queue = take_queue(bus).ok_or(I2cError::QueueTaken)?;
    let (producer, consumer) = queue.split();

    match bus {
        Bus::A => {
            rcc.apb1enr().modify(|_, w| w.i2c1en().enabled());
            let _ = rcc.apb1enr().read(); // short delay after enabling a clock
            rcc.apb1rstr().modify(|_, w| w.i2c1rst().reset());
            rcc.apb1rstr().modify(|_, w| w.i2c1rst().clear_bit());
        }
        Bus::B => {
            rcc.apb1enr().modify(|_, w| w.i2c3en().enabled());
            let _ = rcc.apb1enr().read();
            rcc.apb1rstr().modify(|_, w| w.i2c3rst().reset());
            rcc.apb1rstr().modify(|_, w| w.i2c3rst().clear_bit());
        }
    }

    // Checked above and not taken since: the peripheral is there.
    let Some(hw) = periph.take() else {
        return Err(I2cError::WrongPeripheral);
    };
    let (ev, er) = irqs(bus);
    // SAFETY: the priority byte is a valid value for this device's 4 implemented bits; the
    // interrupts are still masked, so no handler runs against half-built state.
    unsafe {
        nvic.set_priority(ev, I2C_PRIORITY);
        nvic.set_priority(er, I2C_PRIORITY);
    }
    free(|cs| {
        let mut state = Slave {
            hw,
            timing,
            rx: RxState {
                own_addr8,
                producer,
                frame: RxFrame::EMPTY,
                overrun: false,
                phase: Phase::Idle,
            },
        };
        arm(bus, &mut state, pins);
        slot(bus).borrow(cs).replace(Some(state));
    });
    Ok(consumer)
}

// ---- interrupt handlers ----

/// on_event -- the EV interrupt of `bus` (RM0368 18.3.3, slave receiver and transmitter).
/// Registers, from one SR1 snapshot, in the order RM0368 clears the flags:
/// 1. ADDR: read SR2 right after SR1, which clears ADDR and releases SCL.
/// 2. RXNE or BTF: read DR (clears RXNE and BTF).
/// 3. TXE: write FFh to DR (clears TXE). TXE is set only while the peripheral transmits
///    (SR2.TRA = 1); the write is unconditional so a TXE left by a slave read that ended in an
///    error cannot re-enter this handler without end.
/// 4. STOPF: SR1 is read again right before the CR1 write that clears it (RM0368 18.6.6, STOPF;
///    Figure 163 gives each flag its own SR1-read sequence, and with ADDR in the same pass the
///    first SR1 read was spent on ADDR's sequence).
///
/// The frame state then follows bus order, which is not the register order: with ADDR set the
/// peripheral holds SCL low until SR2 is read, so a byte and a STOP in the same snapshot belong
/// to the frame before this address. The byte joins that frame, the STOP posts it, and only
/// then does the new address start a frame (a late handler would otherwise append the old
/// frame's last byte to the new one).
fn on_event(bus: Bus) {
    free(|cs| {
        let mut guard = slot(bus).borrow(cs).borrow_mut();
        let Some(slave) = guard.as_mut() else {
            return;
        };
        let c = counters(bus);
        let regs = slave.hw.regs();
        let sr1 = regs.sr1().read().bits();
        let sr2 = (sr1 & SR1_ADDR != 0).then(|| regs.sr2().read().bits());
        // DR: the data field is 8 bits wide.
        let byte =
            (sr1 & (SR1_RXNE | SR1_BTF) != 0).then(|| (regs.dr().read().bits() & 0xFF) as u8);
        if sr1 & SR1_TXE != 0 {
            // SAFETY: DR takes the 8-bit data byte.
            regs.dr().write(|w| unsafe { w.bits(u16::from(READ_FILL)) });
        }
        if sr1 & SR1_STOPF != 0 {
            let _ = regs.sr1().read(); // the SR1 read of STOPF's own clearing sequence
            regs.cr1().modify(|_, w| w); // write CR1 back unchanged: clears STOPF
        }
        if let Some(byte) = byte {
            slave.rx.on_byte(byte);
        }
        if sr1 & SR1_STOPF != 0 {
            slave.rx.on_stop(c);
        }
        if let Some(sr2) = sr2 {
            slave.rx.on_address(sr2, c);
        }
    });
}

/// on_error -- the ER interrupt of `bus`: reads SR1, counts and clears every error flag (write 0
/// to the flag, 1 to the others: rc_w0, RM0368 18.6.6) and drops the partial frame.
fn on_error(bus: Bus) {
    free(|cs| {
        let mut guard = slot(bus).borrow(cs).borrow_mut();
        let Some(slave) = guard.as_mut() else {
            return;
        };
        let regs = slave.hw.regs();
        let flags = regs.sr1().read().bits() & SR1_ERROR_FLAGS;
        if flags == 0 {
            return;
        }
        // SAFETY: SR1 write: 0 in the flags handled here, 1 in the other rc_w0 flags (no
        // effect), 0 in the read-only bits (ignored).
        regs.sr1()
            .write(|w| unsafe { w.bits(SR1_ERROR_FLAGS & !flags) });
        slave.rx.on_errors(flags, counters(bus));
    });
}

#[interrupt]
fn I2C1_EV() {
    on_event(Bus::A);
}

#[interrupt]
fn I2C1_ER() {
    on_error(Bus::A);
}

#[interrupt]
fn I2C3_EV() {
    on_event(Bus::B);
}

#[interrupt]
fn I2C3_ER() {
    on_error(Bus::B);
}

// ---- hand-off to the bit-bang master ----

/// slave_idle -- true when `bus` is up, no frame is in progress and the peripheral sees no
/// traffic (SR2.BUSY clear, RM0368 18.6.7). A lone SR2 read does not clear ADDR (that needs the
/// SR1 read first), so the interrupt still sees a pending address match.
pub fn slave_idle(bus: Bus) -> bool {
    free(|cs| slot(bus).borrow(cs).borrow().as_ref().is_some_and(idle_now))
}

fn idle_now(slave: &Slave) -> bool {
    slave.rx.phase == Phase::Idle && slave.hw.regs().sr2().read().bits() & SR2_BUSY == 0
}

/// take_bus -- takes `bus` away from the slave interrupts for a master transmission.
/// 1. In one critical section, re-checks that the slave is idle (a frame can start after the
///    caller's own check); if not, returns false and changes nothing.
/// 2. Masks the bus's EV and ER interrupts; with `peripheral_off`, also PE = 0 (RM0368 18.6.1):
///    the peripheral releases SDA and SCL.
fn take_bus(bus: Bus, peripheral_off: bool) -> bool {
    free(|cs| {
        let guard = slot(bus).borrow(cs).borrow();
        let Some(slave) = guard.as_ref() else {
            return false;
        };
        if !idle_now(slave) {
            return false;
        }
        let (ev, er) = irqs(bus);
        NVIC::mask(ev);
        NVIC::mask(er);
        if peripheral_off {
            slave.hw.regs().cr1().modify(|_, w| w.pe().disabled());
        }
        true
    })
}

/// suspend -- the bit-bang master's claim: `take_bus` with PE = 0.
///
/// While PE = 0 the slave cannot hear its bus; a frame another master sends now is lost and is
/// recovered by the requester's retry (PLAN 3.4). Call `resume` afterwards on every path.
pub fn suspend(bus: Bus) -> bool {
    take_bus(bus, true)
}

/// mask_slave -- the hardware master's claim: `take_bus` with the peripheral left on (PE = 1).
fn mask_slave(bus: Bus) -> bool {
    take_bus(bus, false)
}

/// unmask_slave -- gives the interrupts of `bus` back to the slave after a hardware master
/// write. Pending NVIC flags are kept: a frame another master began right after the STOP has
/// ADDR set and SCL stretched, and only the handler releases it.
fn unmask_slave(bus: Bus) {
    let (ev, er) = irqs(bus);
    // SAFETY: the bus state exists (take_bus found it); unmasking only lets its handlers run.
    unsafe {
        NVIC::unmask(ev);
        NVIC::unmask(er);
    }
}

/// resume -- gives `bus` back to the slave: `arm`, exactly as `init` configures it.
pub fn resume(bus: Bus, pins: &BusPins) {
    static_slot::with_mut(slot(bus), |slave| arm(bus, slave, pins));
}

// ---- BUSY-flag erratum ----

/// PIN_POLL_MAX -- reads of a line after the workaround drives it, waiting for the level to
/// show: an open-drain line rises through the bus pull-up (at most 1000 ns in standard mode,
/// I2C-bus specification Table 5); a pin read takes tens of core cycles, so 2000 reads cover
/// many times that rise.
const PIN_POLL_MAX: u32 = 2_000;

/// Line -- one of the two lines of a bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Scl,
    Sda,
}

/// Level -- the level a stuck line reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The line reads low although it was released (cannot drive it high: Table 10.2.2-1
    /// code 1 for SCL, 2 for SDA).
    Low,
    /// The line reads high although it was driven low (cannot drive it low: Table 10.2.2-1
    /// code 3 for SCL, 4 for SDA).
    High,
}

/// StuckLine -- the line `clear_stuck_busy` could not drive to the level the procedure needs, and
/// the level it read instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StuckLine {
    pub line: Line,
    pub stuck: Level,
}

impl StuckLine {
    /// master_error -- the master error that names this line and level.
    pub const fn master_error(self) -> MasterError {
        match (self.line, self.stuck) {
            (Line::Scl, Level::Low) => MasterError::SclStuckLow,
            (Line::Sda, Level::Low) => MasterError::SdaStuckLow,
            (Line::Scl, Level::High) => MasterError::SclStuckHigh,
            (Line::Sda, Level::High) => MasterError::SdaStuckHigh,
        }
    }
}

/// level_is -- true when `pin` reads `high` within PIN_POLL_MAX reads.
fn level_is(pin: &Pins, high: bool) -> bool {
    (0..PIN_POLL_MAX).any(|_| (pin.read() != 0) == high)
}

/// drive_check -- one step of the workaround: `pin` is released (`high`) or driven low and must
/// read that level, else `line` is stuck at the other level.
fn drive_check(pin: &Pins, high: bool, line: Line) -> Result<(), StuckLine> {
    pin.set_all(high);
    if level_is(pin, high) {
        Ok(())
    } else {
        Err(StuckLine {
            line,
            stuck: if high { Level::Low } else { Level::High },
        })
    }
}

/// check_lines -- the checks of the ST workaround, pins already open-drain outputs at high:
/// both read high; SDA low (check low); SCL low (check low); SCL high (check high); SDA high
/// (check high).
fn check_lines(pins: &BusPins) -> Result<(), StuckLine> {
    if !level_is(&pins.scl, true) {
        return Err(StuckLine {
            line: Line::Scl,
            stuck: Level::Low,
        });
    }
    if !level_is(&pins.sda, true) {
        return Err(StuckLine {
            line: Line::Sda,
            stuck: Level::Low,
        });
    }
    drive_check(&pins.sda, false, Line::Sda)?;
    drive_check(&pins.scl, false, Line::Scl)?;
    drive_check(&pins.scl, true, Line::Scl)?;
    drive_check(&pins.sda, true, Line::Sda)
}

/// reset_peripheral -- CR1.SWRST = 1, then 0 (RM0368 18.6.1): the peripheral returns to its reset
/// state; `arm` configures it again. The caller has the bus's EV and ER interrupts masked.
fn reset_peripheral(regs: &RegisterBlock) {
    regs.cr1().modify(|_, w| w.swrst().reset());
    regs.cr1().modify(|_, w| w.swrst().clear_bit());
}

/// busy_without_frame -- the slave has no frame in progress yet SR2.BUSY is set (RM0368 18.6.7):
/// with both lines idle-high this is the erratum signature.
pub fn busy_without_frame(bus: Bus) -> bool {
    free(|cs| {
        slot(bus).borrow(cs).borrow().as_ref().is_some_and(|slave| {
            slave.rx.phase == Phase::Idle && slave.hw.regs().sr2().read().bits() & SR2_BUSY != 0
        })
    })
}

/// clear_stuck_busy -- the ST bus-reset sequence for a stuck BUSY flag, in the order ST gives
/// (the "analog filter locks BUSY" erratum is documented for other STM32 families, not in
/// ES0299; the sequence is a defensive recovery here):
/// 1. The bus's EV and ER interrupts are masked in the NVIC (only this bus's; the other bus
///    and SysTick keep running), then PE = 0 (RM0368 18.6.1).
/// 2. SCL and SDA to GPIO open-drain output, latch high; both must read high.
/// 3. SDA low, must read low; SCL low, must read low; SCL high, must read high; SDA high, must
///    read high. Every check is at most PIN_POLL_MAX reads.
/// 4. Pins back to the alternate function; CR1.SWRST = 1, then 0 (RM0368 18.6.1).
/// 5. `arm`: PE, ACK, ENGC, OAR1, CCR, TRISE and the interrupt enables, the same code as `init`;
///    it unmasks the interrupts again.
///
/// Interrupt-off time: the polling in steps 2 and 3 runs with only this bus's interrupts masked.
/// The one global critical section is `arm` inside `static_slot::with_mut`: about 30 peripheral
/// register accesses and the pin configuration, a few microseconds at 84 MHz.
///
/// The lines are released and the slave re-armed on every path, including a failed check. Err:
/// the line that failed its check and the level it read. Ok when the bus has no slave (nothing
/// to clear). Counts `busy_errata_resets`.
pub fn clear_stuck_busy(bus: Bus, pins: &BusPins) -> Result<(), StuckLine> {
    if static_slot::with_mut(slot(bus), |_| ()).is_none() {
        return Ok(());
    }
    bump(&counters(bus).busy_errata_resets);
    let (ev, er) = irqs(bus);
    NVIC::mask(ev);
    NVIC::mask(er);
    let regs = regs_of(bus);
    regs.cr1().modify(|_, w| w.pe().disabled());
    pins.scl.output(true, true, Speed::Medium);
    pins.sda.output(true, true, Speed::Medium);
    let checked = check_lines(pins);
    pins.scl.set_all(true);
    pins.sda.set_all(true);
    pins_to_af(bus, pins);
    reset_peripheral(regs);
    static_slot::with_mut(slot(bus), |slave| arm(bus, slave, pins));
    checked
}

// ---- hardware master ----

/// BITS_PER_BYTE -- clock bits per byte on the wire: eight data bits and the ACK clock.
const BITS_PER_BYTE: u32 = 9;

/// regs_of -- the register block of `bus`, for the master write and the bus reset, which run
/// outside a critical section.
fn regs_of(bus: Bus) -> &'static RegisterBlock {
    // SAFETY: this module owns I2C1 and I2C3 (moved in at `init`); the pointers are the fixed
    // peripheral addresses. The master reads and writes the registers only while that bus's EV
    // and ER interrupts are masked (`mask_slave`, `clear_stuck_busy`), so no handler touches them
    // at the same time.
    unsafe {
        match bus {
            Bus::A => &*I2C1::ptr(),
            Bus::B => &*I2C3::ptr(),
        }
    }
}

/// Xfer -- one master write in progress on one bus: the registers and the deadlines.
struct Xfer<'a> {
    bus: Bus,
    regs: &'a RegisterBlock,
    t: &'a BusTimes,
    /// Cycle count at the claim; the whole write must end within `t.frame` of it (VITA 46.11
    /// Table 4.1.1-1 T1).
    started: u32,
    /// The write's shared time budget (T2 + T1 from the call, all arbitration retries included).
    deadline: Deadline,
}

impl Xfer<'_> {
    /// wait -- polls `done` (`ipmb_common::wait_for`) until it holds, or until the event wait
    /// that began at `begin` is past T8 (IPMB_STRETCH_TIMEOUT_US), the whole write is past T1 or
    /// the write's shared deadline is. The deadlines are checked at every call, whether or not
    /// `done` already holds. False on the timeout.
    fn wait(&self, begin: u32, done: impl FnMut() -> bool) -> bool {
        wait_for(
            done,
            &[
                (begin, self.t.stretch),
                (self.started, self.t.frame),
                self.deadline.as_pair(),
            ],
        )
    }

    /// clear_berr -- clears the BERR flag (write 0 to it, 1 to the other rc_w0 flags, RM0368
    /// 18.6.6) and counts it. In controller mode the F401 can flag BERR spuriously (ES0299
    /// 2.9.1, "Spurious bus error detection in controller mode"): the flag is cleared and the
    /// transfer goes on. It is not an abort and not a bus fault; a real bus problem shows as a
    /// NACK, ARLO or a timeout, which are handled.
    fn clear_berr(&self) {
        // SAFETY: SR1 write: 0 in BERR, 1 in the other rc_w0 flags (no effect), 0 in the
        // read-only bits (ignored).
        self.regs
            .sr1()
            .write(|w| unsafe { w.bits(SR1_ERROR_FLAGS & !SR1_BERR) });
        bump(&counters(self.bus).master_berr_spurious);
    }

    /// wait_sr1 -- polls SR1 until a bit of `want` or AF is set and returns the SR1 value.
    /// ARLO ends the wait (ArbitrationLost). BERR is cleared and ignored (`clear_berr`).
    /// Bounded per event by T8 and for the whole write by T1 and the shared deadline ->
    /// StretchTimeout, checked at every event whether or not the awaited bit is already set.
    fn wait_sr1(&self, want: u16) -> Result<u16, MasterError> {
        let mut sr1 = 0;
        let seen = self.wait(now(), || {
            sr1 = self.regs.sr1().read().bits();
            if sr1 & SR1_BERR != 0 {
                self.clear_berr();
                sr1 &= !SR1_BERR;
            }
            sr1 & (SR1_ARLO | want | SR1_AF) != 0
        });
        if !seen {
            return Err(MasterError::StretchTimeout);
        }
        if sr1 & SR1_ARLO != 0 {
            return Err(MasterError::ArbitrationLost);
        }
        Ok(sr1)
    }

    /// stop -- CR1.STOP, then waits for the bit to clear and for SR2.MSL to drop (RM0368 18.3.3,
    /// master transmitter): the controller's own STOP is complete. It does not wait for SR2.BUSY:
    /// after the STOP BUSY can belong to the next controller's transaction, which this slave
    /// must be free to answer. Bounded like `wait_sr1`.
    fn stop(&self) -> Result<(), MasterError> {
        self.regs.cr1().modify(|_, w| w.stop().set_bit());
        let done = self.wait(now(), || {
            if self.regs.sr1().read().bits() & SR1_BERR != 0 {
                self.clear_berr();
            }
            self.regs.cr1().read().stop().bit_is_clear()
                && self.regs.sr2().read().bits() & SR2_MSL == 0
        });
        if done {
            Ok(())
        } else {
            Err(MasterError::StretchTimeout)
        }
    }

    /// abort -- the exit after an error that left the transfer unfinished (not ArbitrationLost):
    /// if SR2.MSL is set the peripheral is still controller, so STOP is requested and MSL = 0
    /// awaited, bounded by T8 from now (the write's own deadline may be spent). If MSL is still
    /// set then, CR1.SWRST recovers the peripheral (PE is never cleared while MSL is set,
    /// RM0368 18.6.1). Then the error flags are cleared (write 0, RM0368 18.6.6) and the slave
    /// is re-armed (`resume`), which unmasks.
    fn abort(&self, pins: &BusPins) {
        if self.regs.sr2().read().bits() & SR2_MSL != 0 {
            self.regs.cr1().modify(|_, w| w.stop().set_bit());
            let end = Deadline::from_now(self.t.stretch);
            let ended = wait_for(
                || {
                    self.regs.cr1().read().stop().bit_is_clear()
                        && self.regs.sr2().read().bits() & SR2_MSL == 0
                },
                &[end.as_pair()],
            );
            if !ended {
                reset_peripheral(self.regs);
            }
        }
        // SAFETY: SR1 write: 0 clears the rc_w0 error flags, the other bits are read-only.
        self.regs.sr1().write(|w| unsafe { w.bits(0) });
        resume(self.bus, pins);
    }

    /// nack_stop -- the destination did not acknowledge: STOP (RM0368 18.3.3), then `error`.
    fn nack_stop(&self, error: MasterError) -> Result<u32, MasterError> {
        self.stop()?;
        Err(error)
    }

    /// run -- the master transmitter sequence of RM0368 18.3.3 for `frame`; Ok carries the
    /// cycles from the address byte to the last byte's end (BTF).
    /// 1. CR1.START; wait SB. ADDR before SB: another master started first and addressed this
    ///    slave in the moment between the free-bus check and the mask; the slave holds SCL low
    ///    until its handler runs, so START is withdrawn and ArbitrationLost hands the bus to the
    ///    handler at once (a wait for SB would stretch the other master's clock up to T8).
    /// 2. Write the address byte `frame[0]` to DR (this clears SB, SR1 was read); wait ADDR, or
    ///    AF -> STOP, AddressNack, or ARLO.
    /// 3. Read SR2 (SR1 was read): clears ADDR.
    /// 4. Each data byte: wait TXE, write DR; AF -> STOP, DataNack.
    /// 5. After the last byte wait BTF (or AF), then STOP and wait for it to complete.
    fn run(&self, frame: &[u8]) -> Result<u32, MasterError> {
        let (Some(&address), Some(data)) = (frame.first(), frame.get(1..)) else {
            return Err(MasterError::FrameLength);
        };
        self.regs.cr1().modify(|_, w| w.start().set_bit());
        let sr1 = self.wait_sr1(SR1_SB | SR1_ADDR)?;
        if sr1 & SR1_SB == 0 {
            return Err(if sr1 & SR1_ADDR != 0 {
                MasterError::ArbitrationLost
            } else {
                MasterError::BusBusy
            });
        }
        // SAFETY: DR takes the 8-bit address byte.
        self.regs
            .dr()
            .write(|w| unsafe { w.bits(u16::from(address)) });
        let first_bit = now();
        if self.wait_sr1(SR1_ADDR)? & SR1_AF != 0 {
            return self.nack_stop(MasterError::AddressNack);
        }
        let _ = self.regs.sr2().read();
        for &byte in data {
            if self.wait_sr1(SR1_TXE)? & SR1_AF != 0 {
                return self.nack_stop(MasterError::DataNack);
            }
            // SAFETY: DR takes the 8-bit data byte.
            self.regs.dr().write(|w| unsafe { w.bits(u16::from(byte)) });
        }
        if !data.is_empty() && self.wait_sr1(SR1_BTF)? & SR1_AF != 0 {
            return self.nack_stop(MasterError::DataNack);
        }
        let last_bit = now();
        self.stop()?;
        Ok(last_bit.wrapping_sub(first_bit))
    }
}

/// lines_released -- after a write that ended with a STOP, with the slave unmasked again: SCL and
/// SDA read high, or do within the write's shared `deadline` (another controller's frame may
/// hold them low for its length, which the slave serves meanwhile). Else SclStuckLow or
/// SdaStuckLow (the hardware master has no recovery clocks; the fault path of the link handles
/// a persistent line fault), or StretchTimeout when the lines are high again only after the
/// deadline.
fn lines_released(pins: &BusPins, deadline: Deadline) -> Result<(), MasterError> {
    if lines_high(pins) || wait_for(|| lines_high(pins), &[deadline.as_pair()]) {
        return Ok(());
    }
    Err(if pins.scl.read() == 0 {
        MasterError::SclStuckLow
    } else if pins.sda.read() == 0 {
        MasterError::SdaStuckLow
    } else {
        MasterError::StretchTimeout
    })
}

/// HwMaster -- the hardware I2C implementation of `IpmbMaster` (`IPMB_MASTER_HW`). Polled, the
/// peripheral stays on (multi-master: START from slave mode, RM0368 18.3.3 master transmitter and
/// 18.3.8 errors).
pub struct HwMaster {
    state: MasterState,
}

impl HwMaster {
    /// new -- the shared master state (`MasterState::new`) with the hardware transfer.
    pub fn new(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self, TimebaseError> {
        MasterState::new(dcb, dwt, core_hz, pins).map(|state| HwMaster { state })
    }
}

impl IpmbMaster for HwMaster {
    fn state(&self) -> &MasterState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut MasterState {
        &mut self.state
    }

    /// write_once -- one attempt.
    /// 1. `wait_bus_free` (with the stuck-BUSY check), which ends in `mask_slave`: in one
    ///    critical section the slave is re-checked idle and the bus's EV/ER interrupts are
    ///    masked.
    /// 2. `Xfer::run`, which ends with the controller's own STOP complete (CR1.STOP clear,
    ///    SR2.MSL clear).
    /// 3. Exit, every path: Ok or a NACK -> unmask at once, then the lines must read released
    ///    (`lines_released`, bounded by the shared deadline); ArbitrationLost -> CR1.START
    ///    withdrawn (a START still pending would make the peripheral master at the next free
    ///    bus with no frame to send), then unmask, so the slave handler takes a frame the
    ///    peripheral may already be addressed by (the ER handler counts ARLO and keeps that
    ///    frame); any other error -> `Xfer::abort` (STOP if still controller, SWRST if that
    ///    does not end it, flags cleared, slave re-armed, which unmasks).
    fn write_once(
        &mut self,
        bus: Bus,
        frame: &[u8],
        deadline: Deadline,
    ) -> Result<(), MasterError> {
        let state = &mut self.state;
        let t = state.cy.times();
        let pins = state.pins.bus(bus);
        wait_bus_free(bus, pins, &t, deadline, mask_slave)?;
        let xfer = Xfer {
            bus,
            regs: regs_of(bus),
            t: &t,
            started: now(),
            deadline,
        };
        let outcome = xfer.run(frame);
        let result = match outcome {
            Ok(_) | Err(MasterError::AddressNack | MasterError::DataNack) => {
                unmask_slave(bus);
                lines_released(pins, deadline).and(outcome.map(|_| ()))
            }
            Err(MasterError::ArbitrationLost) => {
                // START is set and cleared by software (RM0368 18.6.1); clearing it when the
                // hardware already did is harmless.
                xfer.regs.cr1().modify(|_, w| w.start().clear_bit());
                unmask_slave(bus);
                Err(MasterError::ArbitrationLost)
            }
            Err(error) => {
                xfer.abort(pins);
                Err(error)
            }
        };
        if let Ok(cycles) = outcome {
            state.stats.last_bit_cycles = cycles;
            state.stats.last_bits = u32::try_from(frame.len())
                .unwrap_or(u32::MAX)
                .saturating_mul(BITS_PER_BYTE);
        }
        result
    }
}
