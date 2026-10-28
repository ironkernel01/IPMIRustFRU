// hal_impl.rs - the ipmc-hal traits over the F401 statics: the 1 ms tick, the DWT cycle counter,
// the two UARTs, the strap pins, LD2, the words kept across a reset and the processor control.
//
// Ownership: none. Every type here is a zero-sized handle (or a word) over a function or a static
// that its own module owns (`systick`, `cycles`, `uart`, `ga`, `led`, `boot_request`, `reset_cause`,
// `fault_record`, `fault`); this file adds the trait surface and no state. The watchdog implements
// its trait in `watchdog.rs`, which owns the registers. The three `KeptWords` handles are the only
// readers and writers of their words and are used from the main loop and, for `FaultWords`, from
// exception context after which the processor parks. No `core::fmt` is reachable from this file
// (the library rule).

use ipmc_hal::kept::KeptWords;
use ipmc_hal::serial::{SerialCounters, SerialError, SerialPort};
use ipmc_hal::system::{StatusLed, Straps, SystemControl};
use ipmc_hal::time::{Clock, CycleCounter};
use ipmc_sensors::eval::led::{Mode, Shown};

use crate::bsp::systick;
use crate::bsp::uart::{self, Port, UartError};
use crate::bsp::{boot_request, cycles, fault, fault_record, ga, kept_words, led, reset_cause};

/// SysTickClock -- the 1 ms tick (`systick`).
#[derive(Clone, Copy, Default)]
pub struct SysTickClock;

impl Clock for SysTickClock {
    fn millis(&self) -> u32 {
        systick::millis()
    }

    fn seconds(&self) -> u32 {
        systick::seconds()
    }
}

/// DwtCycles -- the DWT cycle counter; `hz` is the core clock.
#[derive(Clone, Copy)]
pub struct DwtCycles {
    pub hz: u32,
}

impl CycleCounter for DwtCycles {
    fn now(&self) -> u32 {
        cycles::now()
    }

    fn hz(&self) -> u32 {
        self.hz
    }
}

/// CONSOLE -- `Uart` port number of the console (USART2).
pub const CONSOLE: u8 = 0;
/// APP -- `Uart` port number of the application port (USART1).
pub const APP: u8 = 1;

/// Uart -- a handle to one UART ring pair, chosen by the constant `PORT` (`CONSOLE` or `APP`).
#[derive(Clone, Copy, Default)]
pub struct Uart<const PORT: u8>;

impl<const PORT: u8> Uart<PORT> {
    /// VALID -- `PORT` names a port; evaluated by `port`, so a wrong number fails the build.
    const VALID: () = assert!(
        PORT == CONSOLE || PORT == APP,
        "Uart PORT is CONSOLE or APP"
    );

    /// port -- the port the constant names (a total match, no panic path).
    const fn port() -> Port {
        let () = Self::VALID;
        match PORT {
            APP => Port::App,
            _ => Port::Console,
        }
    }
}

/// SerialPort over a runtime `Port` value (the bootloader chooses its port at run time); the
/// `Uart` handles forward to it.
impl SerialPort for Port {
    fn is_up(&self) -> bool {
        uart::is_up(*self)
    }

    fn getc(&mut self) -> Option<u8> {
        uart::getc(*self)
    }

    fn discard_rx(&mut self) {
        uart::discard_rx(*self);
    }

    fn write(&mut self, bytes: &[u8]) -> usize {
        uart::write(*self, bytes)
    }

    fn write_all_wait(&mut self, bytes: &[u8]) -> Result<(), SerialError> {
        uart::write_all_wait(*self, bytes).map_err(|error| match error {
            UartError::NotReady => SerialError::NotReady,
            _ => SerialError::TxTimeout,
        })
    }

    fn tx_free(&self) -> usize {
        uart::tx_free(*self)
    }

    fn tx_pending(&self) -> bool {
        uart::tx_pending(*self)
    }

    fn flush(&mut self) {
        uart::flush(*self);
    }

    fn counters(&self) -> SerialCounters {
        uart::counters(*self)
    }

    fn count_line_dropped(&mut self) {
        uart::count_line_dropped(*self);
    }
}

impl<const PORT: u8> SerialPort for Uart<PORT> {
    fn is_up(&self) -> bool {
        Self::port().is_up()
    }

    fn getc(&mut self) -> Option<u8> {
        Self::port().getc()
    }

    fn discard_rx(&mut self) {
        Self::port().discard_rx();
    }

    fn write(&mut self, bytes: &[u8]) -> usize {
        Self::port().write(bytes)
    }

    fn write_all_wait(&mut self, bytes: &[u8]) -> Result<(), SerialError> {
        Self::port().write_all_wait(bytes)
    }

    fn tx_free(&self) -> usize {
        Self::port().tx_free()
    }

    fn tx_pending(&self) -> bool {
        Self::port().tx_pending()
    }

    fn flush(&mut self) {
        Self::port().flush();
    }

    fn counters(&self) -> SerialCounters {
        Self::port().counters()
    }

    fn count_line_dropped(&mut self) {
        Self::port().count_line_dropped();
    }
}

/// GaStraps -- the strap pins (`ga`).
#[derive(Clone, Copy, Default)]
pub struct GaStraps;

impl Straps for GaStraps {
    fn read_raw(&self) -> u8 {
        ga::read_raw()
    }

    fn bootloader_pin_high(&self) -> bool {
        ga::bootloader_pin_high()
    }
}

/// Ld2 -- the status LED LD2 (`led`).
#[derive(Clone, Copy, Default)]
pub struct Ld2;

impl StatusLed for Ld2 {
    fn set_mode(&mut self, mode: Mode) {
        led::set_mode(mode);
    }

    fn shown(&self) -> Shown {
        led::shown()
    }

    fn activity(&self) {
        led::activity();
    }
}

/// BootRequestWords -- the four boot request words (`boot_request`).
#[derive(Clone, Copy, Default)]
pub struct BootRequestWords;

impl KeptWords<4> for BootRequestWords {
    fn read(&self) -> [u32; 4] {
        boot_request::read()
    }

    fn write(&mut self, words: [u32; 4]) {
        boot_request::write(words);
    }

    fn clear(&mut self, index: usize) {
        boot_request::clear_word(index);
    }
}

/// KeptSeqWords -- the four words of the request sequence state (`reset_cause::KEPT`).
#[derive(Clone, Copy, Default)]
pub struct KeptSeqWords;

impl KeptWords<4> for KeptSeqWords {
    fn read(&self) -> [u32; 4] {
        // SAFETY: `kept_base` is the `KEPT` static of four words that `reset_cause` owns; the
        // main loop is the only caller and no interrupt handler touches it.
        unsafe { kept_words::read::<4>(reset_cause::kept_base()) }
    }

    fn write(&mut self, words: [u32; 4]) {
        // SAFETY: as in `read`.
        unsafe { kept_words::write::<4>(reset_cause::kept_base(), words) };
    }

    fn clear(&mut self, index: usize) {
        // SAFETY: as in `read`; the callers pass an index below 4.
        unsafe { kept_words::clear(reset_cause::kept_base(), index) };
    }
}

/// FaultWords -- the four words of the kept fault record (`fault_record`).
#[derive(Clone, Copy, Default)]
pub struct FaultWords;

impl KeptWords<4> for FaultWords {
    fn read(&self) -> [u32; 4] {
        // SAFETY: `words` is `__fault_record`, four words at the end of NOINIT (flash_map.x)
        // that `fault_record` owns, which the runtime's `.uninit` stays below (a link.x assert),
        // the bootloader has no section in and no start-up code clears. Only the start-up code
        // reads them (before any fault handler can run); exception context and the panic handler
        // write them and park afterwards.
        unsafe { kept_words::read::<4>(fault_record::words()) }
    }

    fn write(&mut self, words: [u32; 4]) {
        // SAFETY: as in `read`.
        unsafe { kept_words::write::<4>(fault_record::words(), words) };
    }

    fn clear(&mut self, index: usize) {
        // SAFETY: as in `read`; the callers pass an index below 4.
        unsafe { kept_words::clear(fault_record::words(), index) };
    }
}

/// CortexSystem -- processor control: SYSRESETREQ, the fail-stop park and WFI.
#[derive(Clone, Copy, Default)]
pub struct CortexSystem;

impl SystemControl for CortexSystem {
    fn system_reset(&mut self) -> ! {
        reset_cause::system_reset()
    }

    fn park(&self) -> ! {
        fault::park()
    }

    fn wait_for_interrupt(&self) {
        cortex_m::asm::wfi();
    }
}
