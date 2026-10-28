// board.rs - the NUCLEO-F401RE as a `BootBoard`: the hardware the generic boot sequence of
// `ipmc_boot::run` asks for by name. The sequence itself (trace, pins, request, image check,
// decision, countdown, stay, session) is the `ipmc-boot` crate.
//
// Ownership: `F401Boot` owns the peripherals the sequence starts late or stops: the cycle counter
// registers (DCB, DWT) until `init_straps`, SysTick and USART1 until `start_ports`, and RCC and
// NVIC for the UART start and stop. It reaches the UARTs, the LED and the pins through `bsp` only.

use cortex_m::peripheral::{DCB, DWT, NVIC, SYST};
use ipmc_boot::decision::PortId;
use ipmc_boot::run::BootBoard;
use ipmc_nucleo_f401re::bsp::hal_impl::{BootRequestWords, DwtCycles, GaStraps, Ld2, SysTickClock};
use ipmc_nucleo_f401re::bsp::uart::{self, Port, UartError, UartPeripheral};
use ipmc_nucleo_f401re::bsp::{cycles, fault, ga, reset_cause, systick};
use stm32f4::stm32f401::{RCC, USART1};

use crate::flash_image::FlashImage;
use crate::start;

/// F401Boot -- the bootloader's hardware: what is started late (SysTick, USART1) is held until
/// `start_ports`.
pub struct F401Boot {
    pub rcc: RCC,
    pub nvic: NVIC,
    pub dcb: DCB,
    pub dwt: DWT,
    pub syst: Option<SYST>,
    pub usart1: Option<USART1>,
    pub hclk_hz: u32,
    pub pclk2_hz: u32,
}

/// port_of -- the UART of a port.
fn port_of(id: PortId) -> Port {
    match id {
        PortId::Cli => Port::Console,
        PortId::App => Port::App,
    }
}

impl BootBoard for F401Boot {
    type Console = Port;
    type App = Port;
    type Clock = SysTickClock;
    type Cycles = DwtCycles;
    type Led = Ld2;
    type Straps = GaStraps;
    type Request = BootRequestWords;
    type Flash = FlashImage;

    /// init_straps -- the cycle counter, then the strap and BOOTLOADER pins. A counter that does
    /// not run only shows as 0 ms and a shorter settle wait (both bounded).
    fn init_straps(&mut self) {
        let _ = cycles::start_cycle_counter(&mut self.dcb, &mut self.dwt);
        if let Err(e) = ga::init() {
            fault::claim_failed(e);
        }
    }

    /// start_ports -- SysTick (the 1 ms clock), and APP_UART when `IPMC_APP_UART_ENABLE` is 1. A
    /// port that does not start stays out of the wait (the listener reads a port that is up); a
    /// pin conflict is fatal, as in the runtime.
    fn start_ports(&mut self) {
        if let Some(syst) = self.syst.take() {
            systick::init(syst, self.hclk_hz);
        }
        if let (true, Some(usart1)) = (ipmc_defs::IPMC_APP_UART_ENABLE, self.usart1.take()) {
            if let Err(UartError::Pins(e)) = uart::init(
                Port::App,
                &self.rcc,
                UartPeripheral::U1(usart1),
                &mut self.nvic,
                self.pclk2_hz,
            ) {
                fault::claim_failed(e);
            }
        }
    }

    fn stop_port(&mut self, port: PortId) {
        uart::stop(port_of(port), &self.rcc);
    }

    fn start_runtime(&mut self) -> ! {
        start::start_runtime()
    }

    fn system_reset(&mut self) -> ! {
        reset_cause::system_reset()
    }

    fn wait_for_interrupt(&self) {
        cortex_m::asm::wfi();
    }
}
