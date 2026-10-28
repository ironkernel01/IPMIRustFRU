// board.rs - the NUCLEO-F401RE as an `ipmc_hal::board::Board` and `Platform`: the one place that
// names which handle serves which role. Every handle is a zero-sized type (or a word) over a
// static its own module owns (`bsp::hal_impl`), plus the runtime-only drivers (`drivers`).

use crate::drivers::adc::Adc;
use crate::drivers::flash::Flash;
use crate::drivers::gpio::BoardIo;
use crate::drivers::ipmb_bus::F401Ipmb;
use ipmc_hal::board::{Board, Platform};
use ipmc_nucleo_f401re::bsp::hal_impl::{
    BootRequestWords, CortexSystem, DwtCycles, GaStraps, KeptSeqWords, Ld2, SysTickClock, Uart,
    APP, CONSOLE,
};
use ipmc_nucleo_f401re::bsp::watchdog::Watchdog;

/// F401 -- the board.
pub struct F401;

impl Board for F401 {
    type Clock = SysTickClock;
    type Cycles = DwtCycles;
    type Console = Uart<CONSOLE>;
    type App = Uart<APP>;
    type Ipmb = F401Ipmb;
    type Fru = Flash;
    type Analog = Adc;
    type Io = BoardIo;
    type Straps = GaStraps;
    type Led = Ld2;
    const APP_UART_PERIPHERAL: &'static str = "USART1";
}

impl Platform for F401 {
    type System = CortexSystem;
    type Watchdog = Watchdog;
    type KeptSeq = KeptSeqWords;
    type BootRequest = BootRequestWords;
}
