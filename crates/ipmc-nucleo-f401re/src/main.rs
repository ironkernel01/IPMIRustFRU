// main.rs - ipmc-nucleo-f401re entry point for the NUCLEO-F401RE.
// Start-up order: reset cause, clock, pin owner (`ports`), LED, SysTick (after the LED state it
// services exists), UART, IPMB pin park, sensor hardware (ADC1, board I/O; only the groups whose
// ipmi_defs.h source is hardware), strap pins, IPMB drivers (the own address is known; bus
// recovery, bit-bang master, I2C slaves), banner, sensor start, prompt, then the CLI loop
// (console, sensor sampling, IPMB link, sleep).
// Fail-stop: a UART init error cannot be reported on the console, so the LED flashes at
// IPMC_FAULT_BLINK_CHZ (ipmi_defs.h) and the device parks (bsp::fault::park). A pin claim
// conflict prints both owner names on the console and parks (bsp::fault::claim_failed). The panic handler is panic-halt.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use panic_halt as _;

/// Formatted write to the console UART (`\n` becomes CR LF).
macro_rules! out {
    ($($arg:tt)*) => { $crate::bsp::uart::print(format_args!($($arg)*)) };
}

mod bsp;
mod console;
mod data_cmds;
mod ipmb_link;
mod sensing;

use cortex_m_rt::entry;
use ipmc_sensors::drivers::stm32f401::{adc, gpio, ports};
use stm32f4::stm32f401;

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

    let reset_cause = bsp::reset_cause::take(&dp.RCC); // first, before anything can reset again
    let clock_fault = bsp::clock::init(&dp.RCC, &dp.PWR, &dp.FLASH).err();
    let clock = bsp::clock::decode_nominal(&dp.RCC);
    // Pin ownership: `ports` takes GPIOA..GPIOC; every other module claims its pins from it,
    // once, here, before any interrupt that uses a handle is unmasked. LED first and UART
    // second so a later claim failure can still be signalled (console line, else LED blink).
    if let Err(e) = ports::init(&dp.RCC, dp.GPIOA, dp.GPIOB, dp.GPIOC) {
        bsp::fault::claim_failed(e);
    }
    if let Err(e) = bsp::led::init() {
        bsp::fault::claim_failed(e);
    }
    // SysTick services the LED state, so it starts only after `led::init` has published it.
    bsp::systick::init(cp.SYST, clock.hclk_hz);
    let uart_result = bsp::uart::init(
        &dp.RCC,
        dp.USART2,
        &mut cp.NVIC,
        clock.pclk1_hz,
        ipmc_defs::IPMC_CONSOLE_BAUD,
    );
    match uart_result {
        Ok(()) => {}
        Err(bsp::uart::UartError::Pins(e)) => bsp::fault::claim_failed(e),
        Err(_) => {
            // No console exists to report on; signal with the LED and stop.
            bsp::led::set_fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
            bsp::fault::park();
        }
    }
    // IPMB pins float until the I2C drivers start, after the strap pins below.
    let ipmb_pins = bsp::ipmb_pins::park().unwrap_or_else(|e| bsp::fault::claim_failed(e));
    // Hardware is started only for the groups the source switches make hardware; a simulated
    // group claims and touches nothing (no analog pins, no input pins, no PWR_EN drive).
    let needs = sensing::hardware_needs();
    let adc = needs.adc.then(|| {
        match adc::init(
            &dp.RCC,
            dp.ADC1,
            dp.ADC_COMMON,
            clock.pclk2_hz,
            &bsp::board_pins::ADC,
            needs.adc_pins,
        ) {
            Err(adc::InitError::Pins(e)) => bsp::fault::claim_failed(e),
            other => other,
        }
    });
    let board_io = (needs.input_pins || needs.power_pin).then(|| {
        gpio::init(&bsp::board_pins::GPIO, needs.input_pins, needs.power_pin)
            .unwrap_or_else(|e| bsp::fault::claim_failed(e))
    });
    if let Err(e) = bsp::ga::init() {
        bsp::fault::claim_failed(e);
    }

    // IPMB drivers: the bus recovery, the bit-bang master and the I2C slaves, now that the own
    // address is known. A bus that cannot start prints a line and stays disabled.
    let address = ipmc_core::addr::resolve(bsp::ga::read_raw());
    let ipmb_io = ipmb_link::start_drivers(
        ipmb_link::DriverParts {
            rcc: dp.RCC,
            nvic: cp.NVIC,
            dcb: &mut cp.DCB,
            dwt: &mut cp.DWT,
            hclk_hz: clock.hclk_hz,
            pclk1_hz: clock.pclk1_hz,
        },
        dp.I2C1,
        dp.I2C3,
        ipmb_pins,
        &address,
    );
    // The one IpmbLink lives in a static; None would mean main re-entered.
    let Some(ipmb) = ipmb_link::IpmbLink::take(ipmb_io) else {
        loop {
            cortex_m::asm::wfi();
        }
    };
    let mut console = console::Console::new(
        reset_cause,
        clock,
        clock_fault,
        sensing::Sensing::new(adc, board_io),
        ipmb,
    );
    // Banner first: start_sensors may print the first power event line.
    console.banner();
    console.start_sensors();
    console.prompt();
    loop {
        console.poll();
        console.service_sensors();
        console.service_link();
        cortex_m::asm::wfi(); // sleeps until the next interrupt (UART byte or 1 ms tick)
    }
}
