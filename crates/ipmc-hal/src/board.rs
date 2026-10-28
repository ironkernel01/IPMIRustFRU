// board.rs - the services one IPMC instance runs on.

use crate::ipmb::IpmbBus;
use crate::kept::KeptWords;
use crate::serial::SerialPort;
use crate::system::{StatusLed, Straps, SystemControl, Watchdog};
use crate::time::{Clock, CycleCounter};
use ipmc_sensors::hal::{AnalogSource, DigitalInputs, PowerSwitch, ResetOutput};

/// Board -- the services one IPMC instance runs on; the processor board and the emulator
/// implement it. Every handle is cheap to clone: on the F401 each is a zero-sized type or a word.
pub trait Board: Sized + 'static {
    /// The 1 ms tick.
    type Clock: Clock + Clone;
    /// The free-running cycle counter.
    type Cycles: CycleCounter + Clone;
    /// The console port.
    type Console: SerialPort + Clone;
    /// The application port.
    type App: SerialPort + Clone;
    /// Both IPMB buses.
    type Ipmb: IpmbBus;
    /// The FRU store sector.
    type Fru: ipmc_fru::persist::FlashLog;
    /// The analog inputs.
    type Analog: AnalogSource;
    /// The digital inputs, the supply enable and the payload reset output.
    type Io: DigitalInputs + PowerSwitch + ResetOutput;
    /// The strap pins.
    type Straps: Straps + Clone;
    /// The status LED.
    type Led: StatusLed + Clone;
    /// APP_UART_PERIPHERAL -- the banner's peripheral name of the application port ("USART1").
    const APP_UART_PERIPHERAL: &'static str;
}

/// Platform -- what only a processor build has: reset, watchdog and the words kept across a reset.
pub trait Platform: Board {
    /// Reset, park and sleep.
    type System: SystemControl;
    /// The independent watchdog.
    type Watchdog: Watchdog;
    /// The words that carry the request sequence state across a software reset.
    type KeptSeq: KeptWords<4>;
    /// The boot request words.
    type BootRequest: KeptWords<4>;
}
