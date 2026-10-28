// main.rs - ipmc-nucleo-f401re entry point for the NUCLEO-F401RE.
// This is the runtime firmware, linked at 0x0800_C000 (link.x, flash_map.x). The bootloader in sectors 0-1 (src/boot/,
// binary ipmc-boot) checks the image header and starts it with the state of a reset; with the
// bootloader gone a debugger can still run it (cortex-m-rt features set-vtor and set-sp).
// Start-up order: reset cause and the kept fault record, clock, pin owner (`ports`), LED, SysTick (after the LED state it
// services exists), console UART, application UART (when enabled), IPMB pin park, sensor hardware (ADC1, board I/O; only the groups whose
// defs.rs source is hardware), strap pins, the DWT cycle counter, the IPMB master and the FRU flash
// driver. Then `ipmc_runtime::run` does the rest, the same on every board: FRU flash log (load; may
// erase a sector, so it runs before the I2C slaves exist), IPMB buses (the own address is known;
// bus recovery, I2C slaves), banner, sensor start, prompt, watchdog start (the IWDG, when
// IPMC_WATCHDOG_ENABLE), then the CLI loop (watchdog feed, console, application port, sensor
// sampling, IPMB link, sleep). The evaluation build (IPMC_EVAL=1) also starts TIM2 after the UART
// and stops the IPMB when the period is over (eval.rs).
// Fail-stop: a UART init error cannot be reported on the console, so the LED flashes at
// IPMC_FAULT_BLINK_CHZ (defs.rs) and the device parks (bsp::fault::park). A pin claim
// conflict prints both owner names on the console and parks (bsp::fault::claim_failed). A panic
// and the fault handlers (bsp::fault) keep a record in NOINIT, print one line and park; once the
// watchdog runs (above) it resets the processor and the next start reports the kept fault.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

pub(crate) use ipmc_nucleo_f401re::bsp;

mod app_header;
mod board;
mod drivers;
#[cfg(ipmc_eval)]
mod eval;
mod sensing;

use crate::board::F401;
use crate::drivers::ipmb_bus::{DriverParts, F401Ipmb};
use crate::drivers::{adc, flash::Flash, gpio};
use cortex_m_rt::entry;
use ipmc_nucleo_f401re::bsp::hal_impl::{
    BootRequestWords, CortexSystem, DwtCycles, GaStraps, KeptSeqWords, Ld2, SysTickClock, Uart,
    APP, CONSOLE,
};
use ipmc_nucleo_f401re::bsp::ports;
use ipmc_runtime::link::{IpmbLink, LinkArgs};
use ipmc_runtime::run::{run, BoardParts, StartInfo, SystemParts};
use stm32f4::stm32f401;

/// home -- the one IpmbLink, in static storage (PLAN 3.3a); Some on the first call, None after.
/// No interrupt handler touches it.
/// `singleton!` builds the value on the stack and then copies it into the static (about
/// 16.9 KB, `size_of::<IpmbLink<F401>>()`: mostly the 8 KB capture log and the duplicate cache).
/// `inline(never)` keeps that temporary in this function's frame, which is gone when `home`
/// returns at start-up; inlined into `main`, the copy would occupy `main`'s frame, and so the
/// stack, for as long as the firmware runs.
#[inline(never)]
fn home(args: LinkArgs<F401>) -> Option<&'static mut IpmbLink<F401>> {
    cortex_m::singleton!(: IpmbLink<F401> = IpmbLink::new(args))
}

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
    let last_fault = bsp::fault_record::take();
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
        ipmc_runtime::out!(
            &mut Uart::<CONSOLE>,
            "EVALUATION BUILD: timer start failed (TIM2 tick rate)\n"
        );
        bsp::led::set_fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
        bsp::fault::park();
    }
    // IPMB pins float until the I2C drivers start, after the strap pins below.
    let ipmb_pins = drivers::ipmb_pins::park().unwrap_or_else(|e| bsp::fault::claim_failed(e));
    // Hardware is started only for the groups the source switches make hardware; a simulated
    // group claims and touches nothing (no analog pins, no input pins, no PWR_EN drive).
    let needs = sensing::hardware_needs();
    let analog = needs.adc.then(|| {
        match adc::init(
            &dp.RCC,
            dp.ADC1,
            dp.ADC_COMMON,
            clock.pclk2_hz,
            &drivers::board_pins::ADC,
            needs.adc_pins,
        ) {
            Err(adc::InitError::Pins(e)) => bsp::fault::claim_failed(e),
            other => other.map_err(adc::InitError::name),
        }
    });
    let io = (needs.input_pins || needs.power_pin).then(|| {
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
    // The DWT cycle counter runs before the FRU flash log starts, so an erase can give SysTick the
    // stalled time (systick::catch_up); a counter that does not run is reported by the IPMB master
    // start.
    let _ = drivers::ipmb_common::start_cycle_counter(&mut cp.DCB, &mut cp.DWT);
    // The IPMB master takes the pins and the cycle counter; the I2C slaves start in `run`, once
    // the FRU log has loaded.
    let ipmb = F401Ipmb::new(
        DriverParts {
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
    )
    .map_err(drivers::ipmb_common::TimebaseError::name);

    run::<F401>(
        BoardParts {
            start: StartInfo {
                reset_cause,
                last_fault,
                clock: clock.summary(clock_fault),
                app_uart_fault: app_uart_fault.map(bsp::uart::UartError::name),
                image_header: app_header::read(),
                version: env!("CARGO_PKG_VERSION"),
                events_at_boot: ipmc_defs::IPMC_EVENTS_AT_BOOT,
                power_at_boot: ipmc_defs::IPMC_POWER_AT_BOOT,
            },
            clock: SysTickClock,
            cycles: DwtCycles { hz: clock.hclk_hz },
            console: Uart::<CONSOLE>,
            app: Uart::<APP>,
            ipmb,
            fru: Flash::new(dp.FLASH).map_err(drivers::flash::StartError::text),
            analog,
            io,
            rail_wiring: &drivers::board_pins::RAIL_WIRING,
            straps: GaStraps,
            led: Ld2,
        },
        SystemParts {
            system: CortexSystem,
            watchdog: bsp::watchdog::Watchdog::new(dp.IWDG, dp.DBGMCU),
            kept_seq: KeptSeqWords,
            boot_request: BootRequestWords,
            #[cfg(ipmc_eval)]
            eval_expired: eval::expired,
        },
        home,
    )
}
