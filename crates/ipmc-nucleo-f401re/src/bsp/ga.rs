// bsp/ga.rs - strap pin configuration (GA0..GA4, GAP, NVMRO) and the raw level read. Decoding is in ipmc_core::addr.

use cortex_m::interrupt;
use ipmc_sensors::drivers::stm32f401::ports::{self, Pins, PortError, PortId, Pull};

use super::static_slot::{self, StaticSlot};

// Pin assignment. PROVISIONAL: the pin choice is owned by this file and changes here only.
// GA0..GA4 = PC0..PC4, GAP = PC5, NVMRO = PC6. All inputs with internal pull-ups.
// The raw level word has bit n = level of PC(n), the convention of ipmc_core::addr. If the pins
// move, `RAW_MASK` and the bit order in ipmc_core::addr move with them.
const OWNER: &str = "ga";
/// Mask of the seven strap pins in the raw word.
const RAW_MASK: u16 = 0x7F;

static PINS: StaticSlot<Pins> = static_slot::empty();

/// init -- claims PC0..PC6 and configures them as pulled-up inputs.
pub fn init() -> Result<(), PortError> {
    let pins = ports::claim(OWNER, PortId::C, RAW_MASK)?;
    pins.input(Pull::Up);
    static_slot::install(&PINS, pins);
    Ok(())
}

/// read_raw -- reads the raw pin levels (bit n = PC(n), 1 = high). Before `init` every pin reads high.
pub fn read_raw() -> u8 {
    interrupt::free(|cs| {
        PINS.borrow(cs)
            .borrow()
            .as_ref()
            .map_or(RAW_MASK as u8, |p| p.read() as u8)
    })
}
