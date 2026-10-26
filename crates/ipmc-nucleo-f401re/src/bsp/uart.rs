// bsp/uart.rs - the one UART driver of both serial ports, interrupt driven in both directions.
//
// Ports (`Port`): the console on USART2 (PA2 TX / PA3 RX, AF7, the ST-LINK virtual COM port) and
// the application port APP_UART on USART1 (PB6 TX / PB7 RX, optional PA11 CTS / PA12 RTS, AF7;
// DS10086 Table 9). Both run the same code: `Port` selects the port data (peripheral, pins, clock
// enable, ring sizes, baud rate) and `UartPeripheral` gives one register-block reference for
// either USART, the way `I2cPeripheral` does for the I2C buses.
//
// Ownership: this module owns USART1, USART2 and their pins (claimed through `ports`, owners
// "uart" and "app_uart"); the peripherals move in at `init` and are never handed out.
// Receive: the interrupt moves each byte into the port's RX ring; the main loop reads it with
// `getc`. Transmit: `write` and `write_all_wait` copy bytes into the port's TX ring and set TXEIE;
// the interrupt sends them. Nothing in the thread waits for the wire except `flush` (before a
// reset or the fault park) and `write_all_wait` while the ring is full. Each of those waits is
// bounded and counted, and polls the same `service` body the interrupt runs, so they also work
// where the interrupt cannot (HardFault, an unhandled interrupt).
// Failures are counted (RX ring overflow, TX timeout, overrun, console lines dropped) and read
// through `counters()`.

use core::cell::Cell;
use core::fmt;
use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use heapless::Vec;
use ipmc_core::capture::format_cut;
use ipmc_core::ring::ByteRing;
use ipmc_sensors::drivers::stm32f401::ports::{self, PinId, Pins, PortError, PortId, Pull, Speed};
use stm32f4::stm32f401::{interrupt, usart1::RegisterBlock, Interrupt, RCC, USART1, USART2};

use super::static_slot::{self, StaticSlot};

/// Port -- which serial port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    /// The console on USART2 (ST-LINK virtual COM port).
    Console,
    /// The application port APP_UART on USART1.
    App,
}

/// Role -- what a pin does for its port.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Tx,
    Rx,
    /// Clear to send (input, active low); claimed only with hardware flow control.
    Cts,
    /// Request to send (output, active low); claimed only with hardware flow control.
    Rts,
}

/// PinSpec -- one pin of a port.
struct PinSpec {
    pin: PinId,
    role: Role,
}

const fn spec(port: PortId, pin: u8, role: Role) -> PinSpec {
    PinSpec {
        pin: PinId { port, pin },
        role,
    }
}

/// Pins of the console (DS10086 Table 9: PA2 USART2_TX, PA3 USART2_RX, AF7).
const CONSOLE_PINS: [PinSpec; 2] = [spec(PortId::A, 2, Role::Tx), spec(PortId::A, 3, Role::Rx)];
/// Pins of the application port (DS10086 Table 9: PB6 USART1_TX, PB7 USART1_RX, PA11 USART1_CTS,
/// PA12 USART1_RTS, all AF7).
const APP_PINS: [PinSpec; 4] = [
    spec(PortId::B, 6, Role::Tx),
    spec(PortId::B, 7, Role::Rx),
    spec(PortId::A, 11, Role::Cts),
    spec(PortId::A, 12, Role::Rts),
];
/// Alternate function of every UART pin of both ports.
const AF_USART: u8 = 7;

const CONSOLE_RX: usize = ipmc_defs::IPMC_UART_RX_BUF;
const CONSOLE_TX: usize = ipmc_defs::IPMC_UART_TX_BUF;
const APP_RX: usize = ipmc_defs::IPMC_APP_UART_RX_BUF;
const APP_TX: usize = ipmc_defs::IPMC_APP_UART_TX_BUF;

impl Port {
    /// COUNT -- number of ports.
    const COUNT: usize = 2;

    const fn index(self) -> usize {
        match self {
            Port::Console => 0,
            Port::App => 1,
        }
    }

    /// owner -- the pin-claim owner name.
    const fn owner(self) -> &'static str {
        match self {
            Port::Console => "uart",
            Port::App => "app_uart",
        }
    }

    fn pins(self) -> &'static [PinSpec] {
        match self {
            Port::Console => &CONSOLE_PINS,
            Port::App => &APP_PINS,
        }
    }

    const fn interrupt(self) -> Interrupt {
        match self {
            Port::Console => Interrupt::USART2,
            Port::App => Interrupt::USART1,
        }
    }

    /// baud -- the configured baud rate (defs.rs).
    const fn baud(self) -> u32 {
        match self {
            Port::Console => ipmc_defs::IPMC_CONSOLE_BAUD,
            Port::App => ipmc_defs::IPMC_APP_UART_BAUD,
        }
    }

    /// tx_size -- the TX ring size in bytes.
    const fn tx_size(self) -> usize {
        match self {
            Port::Console => CONSOLE_TX,
            Port::App => APP_TX,
        }
    }

    /// flow -- hardware flow control on (the application port only).
    const fn flow(self) -> bool {
        matches!(self, Port::App) && ipmc_defs::IPMC_APP_UART_FLOW
    }
}

/// UartPeripheral -- the USART register block handed to `init`.
pub enum UartPeripheral {
    /// USART1, the application port.
    U1(USART1),
    /// USART2, the console.
    U2(USART2),
}

impl UartPeripheral {
    fn regs(&self) -> &RegisterBlock {
        match self {
            UartPeripheral::U1(p) => p,
            UartPeripheral::U2(p) => p,
        }
    }

    /// port -- the port this USART serves.
    const fn port(&self) -> Port {
        match self {
            UartPeripheral::U1(_) => Port::App,
            UartPeripheral::U2(_) => Port::Console,
        }
    }
}

/// Poll count per transmitted byte. A byte takes 86.8 us at 115200 baud, and TXE can stay low
/// for up to one frame while the previous byte shifts out. One poll takes at least one CPU
/// cycle, so 200 000 polls last at least 2.38 ms at 84 MHz (12.5 ms at 16 MHz): over 27 frames.
const TX_TIMEOUT: u32 = 200_000;
/// Margin added to the flush bound: TC follows TXE by at most one frame (86.8 us at 115200 baud);
/// 200 000 polls last at least 2.38 ms at 84 MHz, the same margin as TX_TIMEOUT.
const FLUSH_MARGIN: u32 = 200_000;
/// Bits on the wire per byte: start, eight data, stop (8N1).
const BITS_PER_BYTE: u64 = 10;
/// NVIC priority byte for USART1 and USART2 (upper bits are significant on this device): below
/// the I2C slaves (priority 1 << 4), so a burst of console bytes never delays an IPMB byte; a
/// UART byte lasts 86.8 us at 115200 baud, so the interrupt body is short and rare. The body runs
/// inside the critical section of `with_port`, which masks every interrupt: an I2C interrupt that
/// arrives meanwhile waits for it (a few microseconds), so the priority orders the interrupts only
/// outside that section.
const UART_PRIORITY: u8 = 2 << 4;
/// Smallest legal BRR divisor: DIV_Mantissa >= 1 (RM0368 19.6.3), i.e. 16.
const BRR_MIN: u32 = 16;
/// PUT_CHUNK -- most bytes `put` copies in one critical section. The critical section masks the
/// I2C interrupts, so the copy stays short (64 bytes take a few microseconds); `write` loops over
/// chunks, each in its own critical section.
const PUT_CHUNK: usize = 64;

/// UartError -- rejects of `init` and the failures inside the transmit path; `name` gives the
/// text for a log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartError {
    /// The requested baud rate is zero.
    ZeroBaud,
    /// The BRR divisor does not fit the register or has a zero mantissa.
    DivisorOutOfRange,
    /// The UART is not initialised yet.
    NotReady,
    /// TXE stayed low for the whole poll bound.
    TxTimeout,
    /// The USART handed to `init` is not the one of the port.
    WrongPeripheral,
    /// A pin of the port is already claimed by another module.
    Pins(PortError),
}

impl UartError {
    /// name -- text for log lines.
    pub fn name(self) -> &'static str {
        match self {
            UartError::ZeroBaud => "baud rate is zero",
            UartError::DivisorOutOfRange => "baud divisor out of range",
            UartError::NotReady => "UART not initialised",
            UartError::TxTimeout => "transmit timeout",
            UartError::WrongPeripheral => "wrong USART for the port",
            UartError::Pins(_) => "UART pin already claimed",
        }
    }
}

/// Ops -- what the thread API needs of a port, whatever its ring sizes. `Uart<RX, TX>` is the one
/// implementation; the object type lets one `with_port` reach either port's static.
trait Ops {
    /// service -- the interrupt body (also polled by the waits).
    fn service(&mut self, port: Port);
    /// getc -- next received byte.
    fn getc(&mut self) -> Option<u8>;
    /// discard_rx -- empties the RX ring and starts a stalled reader again.
    fn discard_rx(&mut self);
    /// put -- copies bytes into the TX ring and starts the transmitter: what fits, at most
    /// PUT_CHUNK bytes per call (one critical section). Returns the count copied.
    fn put(&mut self, bytes: &[u8]) -> usize;
    /// tx_free -- bytes the TX ring still takes.
    fn tx_free(&self) -> usize;
    /// tx_idle -- the TX ring is empty and the last byte has left the shift register.
    fn tx_idle(&self) -> bool;
}

/// Uart -- one port's state. Its empty value (`None` in the slot) is all zero bytes, so the
/// rings land in .bss and not in the flash image: the only field with a niche is the register
/// base address (0 is the empty value), and the flag is a `Cell` (no niche) for that reason.
struct Uart<const RX: usize, const TX: usize> {
    /// The address of the USART register block. The peripheral handle moved into `init` is a
    /// zero-sized token of this fixed MMIO block, so the address stays valid for the whole
    /// program.
    base: NonZeroUsize,
    // Held for the life of the port: the claims stay with this module.
    _pins: Vec<Pins, 4>,
    rx: ByteRing<RX>,
    tx: ByteRing<TX>,
    /// The reader is stopped (RXNEIE off) because the RX ring is full; the byte stays in DR.
    rx_stalled: Cell<bool>,
}

impl<const RX: usize, const TX: usize> Uart<RX, TX> {
    fn new(base: NonZeroUsize, pins: Vec<Pins, 4>) -> Self {
        Uart {
            base,
            _pins: pins,
            rx: ByteRing::new(),
            tx: ByteRing::new(),
            rx_stalled: Cell::new(false),
        }
    }

    /// regs -- the USART registers (`'static`: the block is fixed MMIO, not part of `self`).
    fn regs(&self) -> &'static RegisterBlock {
        // SAFETY: `base` is the address of the register block of the USART handed to `init`
        // (checked against the port there); the block is fixed MMIO that exists for the whole
        // program, and this module is its only user.
        unsafe { &*(self.base.get() as *const RegisterBlock) }
    }
}

impl<const RX: usize, const TX: usize> Ops for Uart<RX, TX> {
    fn service(&mut self, port: Port) {
        let regs = self.regs();
        let counters = &COUNTERS[port.index()];
        // One SR read captures every flag; the DR read below completes the clearing sequence
        // for RXNE and ORE (RM0368 19.6.1).
        let sr = regs.sr().read();
        let overrun = sr.ore().bit_is_set();
        let received = sr.rxne().bit_is_set();
        if overrun || received {
            if port.flow() && self.rx.free() == 0 {
                // Flow control: leave the byte in DR. RTS stays deasserted while DR holds an
                // unread byte (RM0368 19.3.14), so the sender stops; `getc` starts the reader
                // again once the ring has room. RTS deasserts only when DR holds an unread
                // byte, so a sender with a byte already in flight can still overrun by one
                // byte: the overrun counter counts it and the application port answers the
                // line `ERROR LINE_DAMAGED` (ipmc-app).
                regs.cr1().modify(|_, w| w.rxneie().disabled());
                self.rx_stalled.set(true);
            } else {
                let byte = regs.dr().read().bits() as u8; // low 8 bits; the field is 9 wide
                if overrun {
                    counters.overrun.fetch_add(1, Ordering::Relaxed);
                }
                // ORE can be set with RXNE clear (RM0368 19.3.3); DR then holds no new
                // character, so the byte is queued only when this SR capture had RXNE.
                if received && !self.rx.push(byte) {
                    counters.rx_overflow.fetch_add(1, Ordering::Relaxed); // a full ring drops it
                }
            }
        }
        if sr.txe().bit_is_set() && regs.cr1().read().txeie().bit_is_set() {
            if let Some(byte) = self.tx.pop() {
                // SAFETY: DR takes the 8-bit data byte.
                regs.dr().write(|w| unsafe { w.bits(u16::from(byte)) });
            } else {
                regs.cr1().modify(|_, w| w.txeie().disabled());
            }
        }
    }

    fn getc(&mut self) -> Option<u8> {
        let byte = self.rx.pop();
        if self.rx_stalled.get() && self.rx.free() > 0 {
            self.rx_stalled.set(false);
            self.regs().cr1().modify(|_, w| w.rxneie().enabled());
        }
        byte
    }

    fn discard_rx(&mut self) {
        while self.rx.pop().is_some() {}
        if self.rx_stalled.get() {
            self.rx_stalled.set(false);
            self.regs().cr1().modify(|_, w| w.rxneie().enabled());
        }
    }

    fn put(&mut self, bytes: &[u8]) -> usize {
        let take = bytes.len().min(self.tx.free()).min(PUT_CHUNK);
        let Some(part) = bytes.get(..take) else {
            return 0;
        };
        if take > 0 && self.tx.push_slice(part) {
            self.regs().cr1().modify(|_, w| w.txeie().enabled());
            take
        } else {
            0
        }
    }

    fn tx_free(&self) -> usize {
        self.tx.free()
    }

    fn tx_idle(&self) -> bool {
        self.tx.is_empty() && self.regs().sr().read().tc().bit_is_set()
    }
}

// Shared state, one slot per port. Written by `init` (thread, once, before the interrupt is
// unmasked); the ring indices and the stall flag are written by the port's interrupt and by the
// thread API. Every access runs inside a critical section, so the interrupt and the thread never
// overlap.
static CONSOLE: StaticSlot<Uart<CONSOLE_RX, CONSOLE_TX>> = static_slot::empty();
static APP: StaticSlot<Uart<APP_RX, APP_TX>> = static_slot::empty();

/// with_port -- runs `f` on the port's state inside a critical section; None before `init` (or
/// while the state is already borrowed, as in a fault handler that fired inside this call).
fn with_port<R>(port: Port, f: impl FnOnce(&mut dyn Ops) -> R) -> Option<R> {
    match port {
        Port::Console => static_slot::try_with_mut(&CONSOLE, |u| f(u)),
        Port::App => static_slot::try_with_mut(&APP, |u| f(u)),
    }
}

/// PortCounters -- the failure counters of one port. Incremented by the interrupt (rx_overflow,
/// overrun) or by the thread (tx_timeout, tx_lines_dropped); read by `counters()`. Atomics with
/// Relaxed ordering: each is an independent single-word count on a single-core part. They wrap
/// at 2^32.
struct PortCounters {
    rx_overflow: AtomicU32,
    tx_timeout: AtomicU32,
    overrun: AtomicU32,
    tx_lines_dropped: AtomicU32,
}

impl PortCounters {
    const fn new() -> Self {
        PortCounters {
            rx_overflow: AtomicU32::new(0),
            tx_timeout: AtomicU32::new(0),
            overrun: AtomicU32::new(0),
            tx_lines_dropped: AtomicU32::new(0),
        }
    }
}

static COUNTERS: [PortCounters; Port::COUNT] = [PortCounters::new(), PortCounters::new()];

/// Counters -- snapshot of the failure counters of one port.
#[derive(Clone, Copy)]
pub struct Counters {
    pub rx_overflow: u32,
    pub tx_timeout: u32,
    pub overrun: u32,
    /// Trace lines dropped because the TX ring had no room for the whole line.
    pub tx_lines_dropped: u32,
}

/// counters -- returns the failure counts of `port` since reset.
pub fn counters(port: Port) -> Counters {
    let c = &COUNTERS[port.index()];
    Counters {
        rx_overflow: c.rx_overflow.load(Ordering::Relaxed),
        tx_timeout: c.tx_timeout.load(Ordering::Relaxed),
        overrun: c.overrun.load(Ordering::Relaxed),
        tx_lines_dropped: c.tx_lines_dropped.load(Ordering::Relaxed),
    }
}

/// is_up -- true once `init` has installed the port (false while its state is borrowed).
pub fn is_up(port: Port) -> bool {
    with_port(port, |_| ()).is_some()
}

/// BRR = fPCLK / baud, rounded (RM0368 19.3.4, oversampling by 16). Rejects zero baud and a
/// divisor that does not fit the 16-bit register or has a zero mantissa.
fn brr_for(pclk_hz: u32, baud: u32) -> Result<u16, UartError> {
    if baud == 0 {
        return Err(UartError::ZeroBaud);
    }
    let divisor = pclk_hz
        .checked_add(baud / 2)
        .ok_or(UartError::DivisorOutOfRange)?
        / baud;
    if divisor < BRR_MIN {
        return Err(UartError::DivisorOutOfRange);
    }
    u16::try_from(divisor).map_err(|_| UartError::DivisorOutOfRange)
}

/// claim_pins -- claims and configures the pins of `port`: TX alternate function, RX with a
/// pull-up (an idle line reads high), and with flow control CTS (pull-up: an unconnected CTS
/// reads "not clear to send") and RTS. Without flow control PA11 and PA12 are not claimed.
fn claim_pins(port: Port) -> Result<Vec<Pins, 4>, UartError> {
    let mut held = Vec::new();
    for spec in port.pins() {
        let wanted = match spec.role {
            Role::Tx | Role::Rx => true,
            Role::Cts | Role::Rts => port.flow(),
        };
        if !wanted {
            continue;
        }
        let pins = ports::claim_pin(port.owner(), spec.pin).map_err(UartError::Pins)?;
        let pull = match spec.role {
            Role::Rx | Role::Cts => Pull::Up,
            Role::Tx | Role::Rts => Pull::None,
        };
        pins.alternate(AF_USART, pull, false, Speed::Low);
        held.push(pins).map_err(|_| UartError::NotReady)?; // cannot overflow: at most four pins
    }
    Ok(held)
}

/// enable_clock -- switches on the USART clock of `port` (APB1 for USART2, APB2 for USART1).
fn enable_clock(rcc: &RCC, port: Port) {
    match port {
        Port::Console => {
            rcc.apb1enr().modify(|_, w| w.usart2en().enabled());
        }
        Port::App => {
            rcc.apb2enr().modify(|_, w| w.usart1en().enabled());
        }
    }
    // Read back the register that was written: the delay RM0368 requires after enabling a clock.
    let _ = match port {
        Port::Console => rcc.apb1enr().read().bits(),
        Port::App => rcc.apb2enr().read().bits(),
    };
}

/// init -- configures the pins of `port`, starts its USART at the port's baud rate from `pclk_hz`
/// (PCLK1 for the console, PCLK2 for the application port) and enables the RX interrupt.
///
/// Returns `Err(ZeroBaud)`, `Err(DivisorOutOfRange)` or `Err(WrongPeripheral)` before touching
/// any hardware.
pub fn init(
    port: Port,
    rcc: &RCC,
    periph: UartPeripheral,
    nvic: &mut NVIC,
    pclk_hz: u32,
) -> Result<(), UartError> {
    if periph.port() != port {
        return Err(UartError::WrongPeripheral);
    }
    let brr = brr_for(pclk_hz, port.baud())?;
    let pins = claim_pins(port)?;
    enable_clock(rcc, port);

    let regs = periph.regs();
    let base =
        NonZeroUsize::new(core::ptr::from_ref(regs) as usize).ok_or(UartError::WrongPeripheral)?;
    regs.cr1().write(|w| w);
    // SAFETY: BRR takes the 16-bit mantissa/fraction word computed and range-checked above.
    regs.brr().write(|w| unsafe { w.bits(brr) });
    if port.flow() {
        // RTS/CTS handshake (RM0368 19.3.14): CTSE stops the transmitter while CTS is high,
        // RTSE deasserts RTS while DR holds an unread byte.
        regs.cr3().write(|w| w.ctse().enabled().rtse().enabled());
    }
    regs.cr1().write(|w| {
        w.te()
            .enabled()
            .re()
            .enabled()
            .rxneie()
            .enabled()
            .ue()
            .enabled()
    });

    match port {
        Port::Console => static_slot::install(&CONSOLE, Uart::new(base, pins)),
        Port::App => static_slot::install(&APP, Uart::new(base, pins)),
    }
    // SAFETY: the port state is installed above, so the handler finds it.
    unsafe {
        nvic.set_priority(port.interrupt(), UART_PRIORITY);
        NVIC::unmask(port.interrupt());
    }
    Ok(())
}

/// service -- the one interrupt body of both ports.
fn service(port: Port) {
    let _ = with_port(port, |u| u.service(port));
}

#[interrupt]
fn USART1() {
    service(Port::App);
}

#[interrupt]
fn USART2() {
    service(Port::Console);
}

/// getc -- next received byte of `port`, or None when its ring is empty.
pub fn getc(port: Port) -> Option<u8> {
    with_port(port, |u| u.getc()).flatten()
}

/// discard_rx -- throws away every byte waiting in the RX ring of `port` (one critical section)
/// and starts a reader stalled by flow control again. The application port uses it after a loss,
/// when the bytes in the ring cannot be trusted to form whole lines.
pub fn discard_rx(port: Port) {
    let _ = with_port(port, |u| u.discard_rx());
}

/// write -- copies as much of `bytes` as the TX ring takes, starts the transmitter and returns
/// the count. Never waits.
pub fn write(port: Port, bytes: &[u8]) -> usize {
    let mut copied = 0;
    while let Some(rest) = bytes.get(copied..).filter(|rest| !rest.is_empty()) {
        // One critical section per chunk: the interrupts wait only for PUT_CHUNK bytes.
        let n = with_port(port, |u| u.put(rest)).unwrap_or(0);
        if n == 0 {
            break;
        }
        copied += n;
    }
    copied
}

/// tx_pending -- bytes of `port` still wait in the TX ring or in the shift register (false before
/// `init`). Never waits.
pub fn tx_pending(port: Port) -> bool {
    with_port(port, |u| !u.tx_idle()).unwrap_or(false)
}

/// tx_free -- bytes the TX ring of `port` takes now (0 before `init`).
pub fn tx_free(port: Port) -> usize {
    with_port(port, |u| u.tx_free()).unwrap_or(0)
}

/// wait_for_room -- waits until the TX ring takes a byte. Each poll also runs the interrupt body,
/// so the wait ends even where the interrupt cannot run. Bounded by TX_TIMEOUT polls; a timeout
/// is counted.
fn wait_for_room(port: Port) -> Result<(), UartError> {
    for _ in 0..TX_TIMEOUT {
        match with_port(port, |u| {
            u.service(port);
            u.tx_free()
        }) {
            None => return Err(UartError::NotReady),
            Some(0) => {}
            Some(_) => return Ok(()),
        }
    }
    COUNTERS[port.index()]
        .tx_timeout
        .fetch_add(1, Ordering::Relaxed);
    Err(UartError::TxTimeout)
}

/// write_all_wait -- copies all of `bytes` into the TX ring, waiting (bounded, see
/// `wait_for_room`) while the ring is full. Fails with `NotReady` before `init` and with
/// `TxTimeout` when the ring stays full; the bytes not copied are dropped.
pub fn write_all_wait(port: Port, bytes: &[u8]) -> Result<(), UartError> {
    if !is_up(port) {
        return Err(UartError::NotReady);
    }
    let mut rest = bytes;
    while !rest.is_empty() {
        let copied = write(port, rest);
        rest = rest.get(copied..).unwrap_or(&[]);
        if !rest.is_empty() {
            wait_for_room(port)?;
        }
    }
    Ok(())
}

/// flush_polls -- the flush bound for `port`: the whole TX ring at one frame per byte, in polls
/// (a poll lasts at least one cycle of the fastest clock), plus FLUSH_MARGIN.
fn flush_polls(port: Port) -> u32 {
    let ring_bits = (port.tx_size() as u64) * BITS_PER_BYTE;
    let cycles = ring_bits * u64::from(ipmc_defs::IPMC_SYSCLK_HZ) / u64::from(port.baud().max(1));
    u32::try_from(cycles)
        .unwrap_or(u32::MAX)
        .saturating_add(FLUSH_MARGIN)
}

/// flush -- waits (bounded by `flush_polls`) until the TX ring of `port` is empty and the last
/// byte has left the shift register. Each poll runs the interrupt body, so it also drains the
/// ring where the interrupt cannot run. A timeout is counted.
pub fn flush(port: Port) {
    for _ in 0..flush_polls(port) {
        let idle = with_port(port, |u| {
            u.service(port);
            u.tx_idle()
        });
        if idle.unwrap_or(true) {
            return;
        }
    }
    COUNTERS[port.index()]
        .tx_timeout
        .fetch_add(1, Ordering::Relaxed);
}

/// Writer -- `fmt::Write` into the TX ring of a port; `\n` becomes CR LF.
struct Writer {
    port: Port,
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for (i, part) in s.split('\n').enumerate() {
            // A failed write aborts the rest of the message: one timeout per message, not per
            // byte.
            if i > 0 {
                write_all_wait(self.port, b"\r\n").map_err(|_| fmt::Error)?;
            }
            write_all_wait(self.port, part.as_bytes()).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

/// print -- backend of the `out!` macro: formatted console output, complete (waits only while the
/// TX ring is full).
pub fn print(args: fmt::Arguments) {
    // A write error means bytes were dropped; the timeout is already counted and the console has
    // no other channel to report it on.
    let _ = fmt::Write::write_fmt(
        &mut Writer {
            port: Port::Console,
        },
        args,
    );
}

/// ROW_TEXT_MAX -- longest I2C trace row, bytes: the columns of a `TraceRow` (up to 104 characters
/// with the longest names) plus IPMB_MSG_MAX three-character hex bytes (96), rounded up.
const ROW_TEXT_MAX: usize = 224;
const _: () = assert!(
    ROW_TEXT_MAX + 2 <= ipmc_defs::IPMC_UART_TX_BUF,
    "the console TX ring must hold one whole I2C trace row"
);

/// CRLF -- the end of every console line.
const CRLF: &[u8] = b"\r\n";

/// print_cut_or_drop -- one line of at most N bytes plus CR LF, written whole when the TX ring
/// of `port` has room for all of it, else dropped and counted (`tx_lines_dropped`); never a
/// partial line and never a wait. A longer line is cut and ends in `...`.
fn print_cut_or_drop<const N: usize>(port: Port, args: fmt::Arguments) {
    let line = format_cut::<N>(args);
    let text = line.bytes();
    // Only the main loop writes a port and the interrupt only drains the ring, so the room found
    // here is still there for the two writes.
    if tx_free(port) >= text.len() + CRLF.len() {
        write(port, text);
        write(port, CRLF);
    } else {
        COUNTERS[port.index()]
            .tx_lines_dropped
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// print_line_or_drop -- a trace line of at most IPMC_LOG_TEXT_MAX bytes (the length the capture
/// log keeps) through `print_cut_or_drop`.
pub fn print_line_or_drop(port: Port, args: fmt::Arguments) {
    print_cut_or_drop::<{ ipmc_defs::IPMC_LOG_TEXT_MAX }>(port, args);
}

/// print_row_or_drop -- an I2C trace row (longer than a log text line, ROW_TEXT_MAX) through
/// `print_cut_or_drop`.
pub fn print_row_or_drop(port: Port, args: fmt::Arguments) {
    print_cut_or_drop::<ROW_TEXT_MAX>(port, args);
}
