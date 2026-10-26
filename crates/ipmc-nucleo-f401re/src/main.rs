// main.rs - ipmc-nucleo-f401re entry point for the NUCLEO-F401RE.
// This is the runtime firmware, linked at 0x0800_C000 (link.x, flash_map.x). The bootloader in sectors 0-1 (src/boot/,
// binary ipmc-boot) checks the image header and starts it with the state of a reset; with the
// bootloader gone a debugger can still run it (cortex-m-rt features set-vtor and set-sp).
// Start-up order: reset cause, clock, pin owner (`ports`), LED, SysTick (after the LED state it
// services exists), console UART, application UART (when enabled), IPMB pin park, sensor hardware (ADC1, board I/O; only the groups whose
// defs.rs source is hardware), strap pins, FRU flash log (load; may erase a sector, so it runs
// before the I2C slaves exist), IPMB drivers (the own address is known; bus
// recovery, bit-bang master, I2C slaves), banner, sensor start, prompt, then the CLI loop
// (console, application port, sensor sampling, IPMB link, sleep). The evaluation build (IPMC_EVAL=1) also starts
// TIM2 after the UART and stops the IPMB when the period is over (eval.rs).
// Fail-stop: a UART init error cannot be reported on the console, so the LED flashes at
// IPMC_FAULT_BLINK_CHZ (defs.rs) and the device parks (bsp::fault::park). A pin claim
// conflict prints both owner names on the console and parks (bsp::fault::claim_failed). The panic handler is panic-halt.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use panic_halt as _;

// `out!` (formatted console output) and the shared board support come from the library.
#[macro_use]
extern crate ipmc_nucleo_f401re;
pub(crate) use ipmc_nucleo_f401re::bsp;

mod app_header;
mod app_port;
mod console;
mod data_cmds;
mod debug_cmds;
mod drivers;
#[cfg(ipmc_eval)]
mod eval;
mod fru_log;
mod ipmb_link;
mod log_cmds;
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
        bsp::uart::Port::Console,
        &dp.RCC,
        bsp::uart::UartPeripheral::U2(dp.USART2),
        &mut cp.NVIC,
        clock.pclk1_hz,
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
    // The application port (APP_UART, USART1 on PCLK2) starts after the console, so a start-up
    // failure of it can be reported on the console. With IPMC_APP_UART_ENABLE false USART1 is never
    // clocked and its pins stay parked.
    let app_uart_fault = if ipmc_defs::IPMC_APP_UART_ENABLE {
        bsp::uart::init(
            bsp::uart::Port::App,
            &dp.RCC,
            bsp::uart::UartPeripheral::U1(dp.USART1),
            &mut cp.NVIC,
            clock.pclk2_hz,
        )
        .err()
    } else {
        None
    };
    if let Some(bsp::uart::UartError::Pins(e)) = app_uart_fault {
        bsp::fault::claim_failed(e);
    }
    // Evaluation build: the run-time clock starts here, before the banner. A clock that cannot
    // start stops the image (it could not end the evaluation).
    #[cfg(ipmc_eval)]
    if drivers::eval_timer::start(&dp.RCC, dp.TIM2, &clock).is_err() {
        out!("EVALUATION BUILD: timer start failed (TIM2 tick rate)\n");
        bsp::led::set_fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
        bsp::fault::park();
    }
    // IPMB pins float until the I2C drivers start, after the strap pins below.
    let ipmb_pins = drivers::ipmb_pins::park().unwrap_or_else(|e| bsp::fault::claim_failed(e));
    // Hardware is started only for the groups the source switches make hardware; a simulated
    // group claims and touches nothing (no analog pins, no input pins, no PWR_EN drive).
    let needs = sensing::hardware_needs();
    let adc = needs.adc.then(|| {
        match adc::init(
            &dp.RCC,
            dp.ADC1,
            dp.ADC_COMMON,
            clock.pclk2_hz,
            &drivers::board_pins::ADC,
            needs.adc_pins,
        ) {
            Err(adc::InitError::Pins(e)) => bsp::fault::claim_failed(e),
            other => other,
        }
    });
    let board_io = (needs.input_pins || needs.power_pin).then(|| {
        gpio::init(
            &drivers::board_pins::GPIO,
            needs.input_pins,
            needs.power_pin,
        )
        .unwrap_or_else(|e| bsp::fault::claim_failed(e))
    });
    if let Err(e) = bsp::ga::init() {
        bsp::fault::claim_failed(e);
    }

    // FRU flash log: loads the stored image (`None`: the built image stays in use) and erases the
    // store sector when it holds only damage and NVMRO is deasserted. After the UART (it prints),
    // before the I2C slaves start: an erase stalls the core for up to 500 ms (16 KB sector) and no
    // slave exists yet to miss a frame. The DWT cycle counter runs first so the erase can give SysTick the stalled time
    // (systick::catch_up); a counter that does not run is reported by the IPMB master start.
    let _ = drivers::ipmb_common::start_cycle_counter(&mut cp.DCB, &mut cp.DWT);
    let nvmro = bsp::ga::nvmro_asserted();
    let (fru_log, fru_image) = fru_log::FruLog::boot(dp.FLASH, nvmro);

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
    let Some(ipmb) = ipmb_link::IpmbLink::take(
        ipmb_io,
        fru_log,
        fru_image,
        bsp::reset_cause::kept_sequence(reset_cause),
    ) else {
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
    if let Some(error) = app_uart_fault {
        out!("APP_UART not started: {}\n", error.name());
    }
    console.start_events();
    console.start_sensors();
    console.prompt();
    #[cfg(ipmc_eval)]
    let mut ipmb_ended = false;
    loop {
        console.poll();
        console.service_app();
        console.service_sensors();
        #[cfg(not(ipmc_eval))]
        console.service_link();
        #[cfg(ipmc_eval)]
        if !ipmb_ended {
            if eval::expired() {
                console.stop_ipmb_for_eval();
                out!("Evaluation period ended: IPMB stopped (power cycle to restart)\n");
                ipmb_ended = true;
            } else {
                console.service_link();
            }
        } else {
            // A Write FRU Data accepted before the stop is still saved; the save never touches
            // the stopped slaves.
            console.service_fru_log();
        }
        cortex_m::asm::wfi(); // sleeps until the next interrupt (UART byte or 1 ms tick)
    }
}
