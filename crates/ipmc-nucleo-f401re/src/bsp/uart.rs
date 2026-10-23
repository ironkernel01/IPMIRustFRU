// bsp/uart.rs - USART2 console on the ST-LINK virtual COM port (PA2 TX / PA3 RX, AF7).
//
// Ownership: this module owns USART2 and the PA2/PA3 pins (claimed through `ports`, owner "uart"); the USART2 peripheral is
// moved in at init and never handed out.
// Receive: an interrupt moves each byte into a ring buffer; the main loop reads it.
// Transmit: blocking, one byte at a time, each wait bounded by TX_TIMEOUT.
// Failures are counted (RX ring overflow, TX timeout, overrun) and read through `counters()`.

use core::cell::RefCell;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::interrupt::{free, Mutex};
use cortex_m::peripheral::NVIC;
use ipmc_sensors::drivers::stm32f401::ports::{self, Pins, PortError, PortId, Pull, Speed};
use stm32f4::stm32f401::{interrupt, Interrupt, RCC, USART2};

/// RX ring size, from ipmi_defs.h IPMC_UART_RX_BUF. A power of two, so indices wrap with a
/// mask; one slot stays empty. ipmc-defs guards that a whole console line fits in the usable
/// bytes; the rest is margin for the time the main loop spends in blocking transmit (with
/// 128 bytes and an 80-byte line: 127 - 80 = 47 bytes x 86.8 us per byte at 115200 baud,
/// 10 bits per byte = about 4.1 ms).
const RX_BUF_SIZE: usize = ipmc_defs::IPMC_UART_RX_BUF as usize;
const _: () = assert!(
    RX_BUF_SIZE.is_power_of_two() && RX_BUF_SIZE >= 2,
    "RX_BUF_SIZE must be a power of two"
);
/// Poll count per transmitted byte. A byte takes 86.8 us at 115200 baud, and TXE can stay low
/// for up to one frame while the previous byte shifts out. One poll takes at least one CPU
/// cycle, so 200 000 polls last at least 2.38 ms at 84 MHz (12.5 ms at 16 MHz): over 27 frames.
const TX_TIMEOUT: u32 = 200_000;
/// Poll count for transmission complete before a reset: TC follows TXE by at most one frame
/// (86.8 us); 200 000 polls last at least 2.38 ms at 84 MHz, the same margin as TX_TIMEOUT.
const FLUSH_TIMEOUT: u32 = 200_000;
/// NVIC priority byte for USART2 (upper bits are significant on this device).
const USART2_PRIORITY: u8 = 2 << 4;

// Pin and register encodings (DS10086 Table 9 maps PA2 to USART2_TX and PA3 to USART2_RX on AF7).
const OWNER: &str = "uart";
const TX_PIN_MASK: u16 = 1 << 2;
const RX_PIN_MASK: u16 = 1 << 3;
const AF_USART2: u8 = 7;
/// Smallest legal BRR divisor: DIV_Mantissa >= 1 (RM0368 19.6.3), i.e. 16.
const BRR_MIN: u32 = 16;

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
    /// PA2 or PA3 is already claimed by another module.
    Pins(PortError),
}

impl UartError {
    #[allow(dead_code)] // the only init caller has no console to print on; kept for log lines
    /// name -- text for log lines.
    pub fn name(self) -> &'static str {
        match self {
            UartError::ZeroBaud => "baud rate is zero",
            UartError::DivisorOutOfRange => "baud divisor out of range",
            UartError::NotReady => "UART not initialised",
            UartError::TxTimeout => "transmit timeout",
            UartError::Pins(_) => "UART pin already claimed",
        }
    }
}

struct Uart {
    usart: USART2,
    // Held for the life of the console: the claims stay with this module.
    _pins: [Pins; 2],
    rx_buf: [u8; RX_BUF_SIZE],
    head: usize,
    tail: usize,
}

// Shared state. `UART` is written by `init` (thread, once, before the interrupt is unmasked);
// `head` and the buffer slots are written by the USART2 interrupt; `tail` is written by `getc`
// (thread); the transmit path reads SR and writes DR from the thread. Every access runs inside
// a critical section, so the interrupt and the thread never overlap.
static UART: Mutex<RefCell<Option<Uart>>> = Mutex::new(RefCell::new(None));

// Failure counters. Incremented by the USART2 interrupt (RX_OVERFLOWS, OVERRUNS) or by the
// thread (TX_TIMEOUTS); read by `counters()` from the thread. Atomics with Relaxed ordering:
// each is an independent single-word count on a single-core part. They wrap at 2^32.
static RX_OVERFLOWS: AtomicU32 = AtomicU32::new(0);
static TX_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static OVERRUNS: AtomicU32 = AtomicU32::new(0);

/// Counters -- snapshot of the failure counters.
#[derive(Clone, Copy)]
pub struct Counters {
    pub rx_overflow: u32,
    pub tx_timeout: u32,
    pub overrun: u32,
}

/// counters -- returns the RX ring overflow, TX timeout and USART overrun counts since reset.
pub fn counters() -> Counters {
    Counters {
        rx_overflow: RX_OVERFLOWS.load(Ordering::Relaxed),
        tx_timeout: TX_TIMEOUTS.load(Ordering::Relaxed),
        overrun: OVERRUNS.load(Ordering::Relaxed),
    }
}

/// is_up -- true once `init` has installed the UART (false while the state is mutably borrowed).
pub fn is_up() -> bool {
    free(|cs| {
        UART.borrow(cs)
            .try_borrow()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    })
}

/// BRR = fPCLK / baud, rounded (RM0368 19.3.4, oversampling by 16). Rejects zero baud and a
/// divisor that does not fit the 16-bit register or has a zero mantissa.
fn brr_for(pclk1_hz: u32, baud: u32) -> Result<u16, UartError> {
    if baud == 0 {
        return Err(UartError::ZeroBaud);
    }
    let divisor = pclk1_hz
        .checked_add(baud / 2)
        .ok_or(UartError::DivisorOutOfRange)?
        / baud;
    if divisor < BRR_MIN {
        return Err(UartError::DivisorOutOfRange);
    }
    u16::try_from(divisor).map_err(|_| UartError::DivisorOutOfRange)
}

/// init -- configures PA2/PA3 for USART2, starts it at `baud` from `pclk1_hz`, enables the RX
/// interrupt.
///
/// Returns `Err(ZeroBaud)` or `Err(DivisorOutOfRange)` before touching any hardware.
pub fn init(
    rcc: &RCC,
    usart: USART2,
    nvic: &mut NVIC,
    pclk1_hz: u32,
    baud: u32,
) -> Result<(), UartError> {
    let brr = brr_for(pclk1_hz, baud)?;

    // PA2, PA3: alternate function AF7, pull-up on RX so an idle line reads high.
    let tx = ports::claim(OWNER, PortId::A, TX_PIN_MASK).map_err(UartError::Pins)?;
    let rx = ports::claim(OWNER, PortId::A, RX_PIN_MASK).map_err(UartError::Pins)?;
    tx.alternate(AF_USART2, Pull::None, false, Speed::Low);
    rx.alternate(AF_USART2, Pull::Up, false, Speed::Low);

    rcc.apb1enr().modify(|_, w| w.usart2en().enabled());
    let _ = rcc.apb1enr().read();

    usart.cr1().write(|w| w);
    // SAFETY: BRR takes the 16-bit mantissa/fraction word computed and range-checked above.
    usart.brr().write(|w| unsafe { w.bits(brr) });
    usart.cr1().write(|w| {
        w.te()
            .enabled()
            .re()
            .enabled()
            .rxneie()
            .enabled()
            .ue()
            .enabled()
    });

    free(|cs| {
        UART.borrow(cs).replace(Some(Uart {
            usart,
            _pins: [tx, rx],
            rx_buf: [0; RX_BUF_SIZE],
            head: 0,
            tail: 0,
        }));
    });
    // SAFETY: the UART state is installed above, so the handler finds it.
    unsafe {
        nvic.set_priority(Interrupt::USART2, USART2_PRIORITY);
        NVIC::unmask(Interrupt::USART2);
    }
    Ok(())
}

#[interrupt]
fn USART2() {
    free(|cs| {
        if let Some(u) = UART.borrow(cs).borrow_mut().as_mut() {
            // One SR read captures both flags; the DR read below completes the clearing
            // sequence for RXNE and ORE (RM0368 19.6.1).
            let sr = u.usart.sr().read();
            let overrun = sr.ore().bit_is_set();
            let received = sr.rxne().bit_is_set();
            if !(overrun || received) {
                return;
            }
            let byte = u.usart.dr().read().bits() as u8; // low 8 bits; the field is 9 wide
            if overrun {
                OVERRUNS.fetch_add(1, Ordering::Relaxed);
            }
            // ORE can be set with RXNE clear (RM0368 19.3.3); DR then holds no new character,
            // so the byte is queued only when this SR capture had RXNE.
            if received {
                let next = (u.head + 1) & (RX_BUF_SIZE - 1);
                if next == u.tail {
                    RX_OVERFLOWS.fetch_add(1, Ordering::Relaxed); // a full ring drops the byte
                } else {
                    u.rx_buf[u.head] = byte;
                    u.head = next;
                }
            }
        }
    });
}

/// getc -- next received byte, or None when the ring is empty.
pub fn getc() -> Option<u8> {
    free(|cs| {
        let mut slot = UART.borrow(cs).borrow_mut();
        let u = slot.as_mut()?;
        if u.tail == u.head {
            return None;
        }
        let byte = u.rx_buf[u.tail];
        u.tail = (u.tail + 1) & (RX_BUF_SIZE - 1);
        Some(byte)
    })
}

/// Sends one byte. Returns `Err(NotReady)` before `init`, or `Err(TxTimeout)` (counted) when
/// TXE stays low for TX_TIMEOUT polls; the byte is dropped in both cases.
fn putc(byte: u8) -> Result<(), UartError> {
    if !is_up() {
        return Err(UartError::NotReady);
    }
    for _ in 0..TX_TIMEOUT {
        let sent = free(|cs| {
            UART.borrow(cs).borrow().as_ref().is_some_and(|u| {
                if u.usart.sr().read().txe().bit_is_set() {
                    // SAFETY: DR takes the 8-bit data byte.
                    u.usart.dr().write(|w| unsafe { w.bits(u16::from(byte)) });
                    true
                } else {
                    false
                }
            })
        });
        if sent {
            return Ok(());
        }
    }
    TX_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    Err(UartError::TxTimeout)
}

/// flush -- waits (bounded) until the last byte has left the shift register. A timeout is counted.
pub fn flush() {
    for _ in 0..FLUSH_TIMEOUT {
        let done = free(|cs| {
            UART.borrow(cs)
                .borrow()
                .as_ref()
                .is_none_or(|u| u.usart.sr().read().tc().bit_is_set())
        });
        if done {
            return;
        }
    }
    TX_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
}

struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                putc(b'\r').map_err(|_| fmt::Error)?; // terminals expect CR LF
            }
            // A failed byte aborts the rest of the message: one timeout per message, not per byte.
            putc(byte).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

/// print -- backend of the `out!` macro.
pub fn print(args: fmt::Arguments) {
    // A write error means a byte was dropped; TX_TIMEOUTS already counted it and the console
    // has no other channel to report it on.
    let _ = fmt::Write::write_fmt(&mut Writer, args);
}
