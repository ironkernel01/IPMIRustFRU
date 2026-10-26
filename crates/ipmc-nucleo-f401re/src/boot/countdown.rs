// countdown.rs - the boot countdown of Documentation/ipmc_boot_plan.md 6.5: the line and the
// digits on both ports for `IPMC_BOOT_COUNTDOWN` seconds, with both ports listening for WAKEUP.
// The window and the digits are `ipmc_boot::decision::Countdown`; this file drives it from SysTick
// and feeds it the port of the first WAKEUP line (`serve::Editors`). SysTick, the LED and
// APP_UART are started by `serve::start_ports` before.

use ipmc_boot::decision::{Countdown, PortId, Tick};
use ipmc_boot::text::{put_dec, TextOut, CRLF};
use ipmc_boot::trace;
use ipmc_nucleo_f401re::bsp::systick;

use crate::console::Both;
use crate::serve::Editors;

/// run -- prints the countdown line on both ports, then one digit a second (`3 2 1`), then CR LF
/// on both ports. Returns the port of a `WAKEUP` line that completed inside the window, or None
/// when the window of `seconds` is over (the start). A `WAKEUP` whose line end arrives after the
/// window is not answered.
pub fn run(seconds: u32, editors: &mut Editors) -> Option<PortId> {
    let mut both = Both;
    trace::countdown(&mut both, seconds, ipmc_defs::IPMC_APP_UART_ENABLE);
    let mut window = Countdown::new(seconds);
    let started = systick::millis();
    let mut first = true;
    let chosen = loop {
        let wakeup = editors.wakeup();
        match window.step(systick::millis().wrapping_sub(started), wakeup) {
            Tick::Digit(digit) => {
                if !first {
                    both.put(" ");
                }
                first = false;
                put_dec(&mut both, digit);
            }
            Tick::Wait => cortex_m::asm::wfi(),
            Tick::Start => break None,
            Tick::Session(port) => break Some(port),
        }
    };
    both.put(CRLF);
    chosen
}
