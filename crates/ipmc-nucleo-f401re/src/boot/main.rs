// main.rs - ipmc-boot, the bootloader (sectors 0-1) of the NUCLEO-F401RE (Documentation/
// ipmc_boot_plan.md). Every decision and the whole boot sequence (trace, pins, boot request,
// image check, start decision, countdown, stay, command session) are the `ipmc-boot` crate
// (`ipmc_boot::run::boot`); this binary is the hardware glue: the reset cause, the clock, the pin
// owner, the LED and the console UART, then the board handles handed to `boot`.
//
// Start-up order (design 6.1): reset cause (no clear: the runtime clears and reports it), clock,
// pin owner, LED, console UART, then `boot`: trace (banner, reset cause, clock fault), cycle
// counter, strap and BOOTLOADER pins, pin settle wait and the one pin read, boot request, image
// check (timed), decision, then the start, the countdown (which ends in the start or in a
// session), or the stay (both ports wait for WAKEUP, then the session).
// Fail-stop: a pin claim conflict or a UART start-up failure parks (bsp::fault), as in the
// runtime. HardFault and DefaultHandler are the library ones (bsp/fault.rs).
//
// No `core::fmt` is reachable (design 3.3): every number goes through `ipmc_boot::text`.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod board;
mod flash_image;
mod start;

use board::F401Boot;
use cortex_m_rt::entry;
use ipmc_boot::run::{boot, BootParts};
use ipmc_nucleo_f401re::bsp::hal_impl::{BootRequestWords, DwtCycles, GaStraps, Ld2, SysTickClock};
use ipmc_nucleo_f401re::bsp::ports;
use ipmc_nucleo_f401re::bsp::reset_cause;
use ipmc_nucleo_f401re::bsp::uart::{self, Port, UartError, UartPeripheral};
use ipmc_nucleo_f401re::bsp::{clock, fault, led};
use stm32f4::stm32f401;

use flash_image::FlashImage;

#[entry]
fn main() -> ! {
    // Both take() calls run once, at reset; None would mean main re-entered.
    let (Some(dp), Some(mut cp)) = (
        stm32f401::Peripherals::take(),
        cortex_m::Peripherals::take(),
    ) else {
        loop {
            cortex_m::asm::wfi();
        }
    };

    // Steps 1 to 3: the reset cause (read, not cleared), the clock, the pins, LED, console.
    let cause = reset_cause::read(&dp.RCC);
    let clock_fault = match clock::init(&dp.RCC, &dp.PWR, &dp.FLASH) {
        Ok(()) => None,
        Err(fault) => Some(fault.text()),
    };
    let clock = clock::decode_nominal(&dp.RCC);
    if let Err(e) = ports::init(&dp.RCC, dp.GPIOA, dp.GPIOB, dp.GPIOC) {
        fault::claim_failed(e);
    }
    if let Err(e) = led::init() {
        fault::claim_failed(e);
    }
    match uart::init(
        Port::Console,
        &dp.RCC,
        UartPeripheral::U2(dp.USART2),
        &mut cp.NVIC,
        clock.pclk1_hz,
    ) {
        Ok(()) => {}
        Err(UartError::Pins(e)) => fault::claim_failed(e),
        Err(_) => {
            // No console exists to report on; signal with the LED and stop.
            led::set_fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
            fault::park();
        }
    }

    // The rest is the generic sequence over this board's handles.
    let parts = BootParts::<F401Boot> {
        console: Port::Console,
        app: Port::App,
        clock: SysTickClock,
        cycles: DwtCycles { hz: clock.hclk_hz },
        led: Ld2,
        straps: GaStraps,
        request: BootRequestWords,
        flash: FlashImage::new(dp.FLASH),
        cause,
        clock_fault,
        version: env!("CARGO_PKG_VERSION"),
    };
    let board = F401Boot {
        rcc: dp.RCC,
        nvic: cp.NVIC,
        dcb: cp.DCB,
        dwt: cp.DWT,
        syst: Some(cp.SYST),
        usart1: Some(dp.USART1),
        hclk_hz: clock.hclk_hz,
        pclk2_hz: clock.pclk2_hz,
    };
    boot(board, parts)
}
