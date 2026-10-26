// console.rs - where the bootloader's text goes: the console, or both serial ports.
//
// The text type for one port is the library's `uart::PortText` (the fault handlers use it too).
// `Both` is the countdown's output: the console always, APP_UART when it is up. The bootloader
// writes no formatted text (design 3.3): every line is fixed text and the number writers of
// `ipmc_boot::text`.

use ipmc_boot::text::TextOut;
use ipmc_nucleo_f401re::bsp::uart::{self, Port, PortText};

/// Both -- the console and, when it is up, APP_UART.
pub struct Both;

impl TextOut for Both {
    fn put(&mut self, text: &str) {
        PortText(Port::Console).put(text);
        if uart::is_up(Port::App) {
            PortText(Port::App).put(text);
        }
    }
}

/// flush -- waits (bounded) until the TX rings of the console and of APP_UART, when it is up,
/// have sent their last byte.
pub fn flush() {
    uart::flush(Port::Console);
    if uart::is_up(Port::App) {
        uart::flush(Port::App);
    }
}
