// main.rs - ipmc-boot, the bootloader (sectors 0-1) of the NUCLEO-F401RE (Documentation/
// ipmc_boot_plan.md). Every decision (image check, boot request, start decision, countdown,
// the command session, trace text) is the `ipmc-boot` crate; this binary is the hardware glue.
//
// Start-up order (design 6.1): reset cause (no clear: the runtime clears and reports it), clock,
// pin owner, LED, console UART, trace (banner, reset cause, clock fault), cycle counter, strap
// and BOOTLOADER pins, pin settle wait and the one pin read, boot request, image check (timed),
// decision, then the start, the countdown (which ends in the start or in a session), or the
// stay (both ports wait for WAKEUP, then the session). The session is serve.rs.
// Fail-stop: a pin claim conflict or a UART start-up failure parks (bsp::fault), as in the
// runtime. HardFault and DefaultHandler are the library ones (bsp/fault.rs).
//
// No `core::fmt` is reachable (design 3.3): every number goes through `ipmc_boot::text`.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use panic_halt as _;

mod console;
mod countdown;
mod flash_image;
mod serve;
mod start;

use cortex_m_rt::entry;
use ipmc_boot::decision::{decide, Decision, PortId, Reason};
use ipmc_boot::session::StartRecord;
use ipmc_boot::{header, request, trace};
use ipmc_nucleo_f401re::bsp::reset_cause::{self, ResetCause};
use ipmc_nucleo_f401re::bsp::uart::{self, Port, PortText, UartError, UartPeripheral};
use ipmc_nucleo_f401re::bsp::{boot_request, clock, cycles, fault, ga, led, systick};
use ipmc_sensors::drivers::stm32f401::ports;
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
    let clock_fault = clock::init(&dp.RCC, &dp.PWR, &dp.FLASH).err();
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
    let mut console = PortText(Port::Console);

    // Step 4: the first trace lines.
    trace::banner(
        &mut console,
        env!("CARGO_PKG_VERSION"),
        ipmc_defs::FIELD_BUILD,
    );
    trace::reset_cause(&mut console, cause.text());
    if let Some(fault) = clock_fault {
        trace::clock_fault(&mut console, fault.text());
    }

    // Step 5: the cycle counter, the strap and BOOTLOADER pins, the settle wait, the one read.
    // A counter that does not run only shows as 0 ms and a shorter settle wait (both bounded).
    let _ = cycles::start_cycle_counter(&mut cp.DCB, &mut cp.DWT);
    if let Err(e) = ga::init() {
        fault::claim_failed(e);
    }
    cycles::delay_us(ipmc_defs::IPMC_FWU_PIN_SETTLE_US, clock.hclk_hz);
    let pin_high = ga::bootloader_pin_high();
    trace::pin(&mut console, pin_high);

    // Step 6: the boot request words, then the check word is cleared so a later reset reads
    // none; the request counts only after a software reset.
    let words = boot_request::read();
    boot_request::clear();
    let request = request::decode(words, matches!(cause, ResetCause::Software));
    trace::request(&mut console, request);

    // Step 7: the image check, timed with the cycle counter (32-bit arithmetic).
    let mut flash = FlashImage::new(dp.FLASH);
    let started = cycles::now();
    let report = header::check(&flash);
    let elapsed = cycles::now().wrapping_sub(started);
    let ms = elapsed
        .checked_div(clock.hclk_hz / systick::MS_PER_SECOND)
        .unwrap_or(0);
    trace::runtime_image(&mut console, &report);
    trace::image_check(&mut console, &report, ms);

    // Step 8: the decision.
    let decision = decide(
        pin_high,
        request,
        report.result,
        ipmc_defs::IPMC_BOOT_COUNTDOWN,
    );
    trace::decision(&mut console, &decision, ipmc_defs::IPMC_APP_UART_ENABLE);

    // Step 9: start, countdown then start or session, or stay and wait for a session.
    let hardware = serve::Hardware {
        syst: cp.SYST,
        hclk_hz: clock.hclk_hz,
        usart1: dp.USART1,
        rcc: &dp.RCC,
        nvic: &mut cp.NVIC,
        pclk2_hz: clock.pclk2_hz,
    };
    let mut editors = serve::Editors::new();
    let (chosen, decision) = match decision {
        Decision::Start => (None, decision),
        Decision::Countdown(seconds) => {
            serve::start_ports(hardware);
            match countdown::run(seconds, &mut editors) {
                Some(port) => {
                    let decision = Decision::Stay(Reason::WakeupInCountdown);
                    trace::decision(&mut console, &decision, ipmc_defs::IPMC_APP_UART_ENABLE);
                    (Some(port), decision)
                }
                None => (None, decision),
            }
        }
        Decision::Stay(_) => {
            serve::start_ports(hardware);
            (Some(serve::wait_for_wakeup(&mut editors)), decision)
        }
    };
    let Some(chosen) = chosen else {
        trace::starting(&mut console);
        start::start_runtime()
    };
    let record = StartRecord {
        version: env!("CARGO_PKG_VERSION"),
        reset_cause: cause.text(),
        pin_high,
        request,
        decision,
        port_name: match chosen {
            PortId::Cli => ipmc_defs::IPMC_CONSOLE_UART_NAME,
            PortId::App => ipmc_defs::IPMC_APP_UART_NAME,
        },
        baud: match chosen {
            PortId::Cli => ipmc_defs::IPMC_CONSOLE_BAUD,
            PortId::App => ipmc_defs::IPMC_APP_UART_BAUD,
        },
        flow: chosen == PortId::App && ipmc_defs::IPMC_APP_UART_FLOW,
        app_enabled: ipmc_defs::IPMC_APP_UART_ENABLE,
    };
    serve::session(chosen, &mut editors, &mut flash, &record, &dp.RCC)
}
