// bsp/ga.rs - strap pin configuration (GA0..GA4, GAP, NVMRO, BOOTLOADER) and the raw level read.
// Decoding is in ipmc_core::addr.

use cortex_m::interrupt;
use ipmc_core::addr;
use ipmc_sensors::drivers::stm32f401::ports::{self, PinId, Pins, PortError, PortId, Pull};

use super::static_slot::{self, StaticSlot};

// Pin assignment. PROVISIONAL: the pin choice is owned by this file and changes here only.
// GA0..GA4 = PC0..PC4, GAP = PC5, NVMRO = PC6. All inputs with internal pull-ups.
// The raw level word has bit n = level of PC(n), the convention of ipmc_core::addr. If the pins
// move, `RAW_MASK` and the bit order in ipmc_core::addr move with them.
const OWNER: &str = "ga";
/// Mask of the seven strap pins in the raw word.
const RAW_MASK: u16 = 0x7F;

/// The BOOTLOADER strap: PC10 (morpho CN7-1), owner "boot_pin". The bootloader (Phase 12) treats
/// logic 1 at reset as "stay in the bootloader"; the runtime firmware only reads and reports it.
/// An input with the internal pull-down, so an open pin reads 0.
const BOOT_OWNER: &str = "boot_pin";
const BOOT_PIN: PinId = PinId {
    port: PortId::C,
    pin: 10,
};

static PINS: StaticSlot<Pins> = static_slot::empty();
static BOOT: StaticSlot<Pins> = static_slot::empty();

/// init -- claims PC0..PC6 and configures them as pulled-up inputs; claims PC10 (BOOTLOADER) as
/// a pulled-down input.
pub fn init() -> Result<(), PortError> {
    let pins = ports::claim(OWNER, PortId::C, RAW_MASK)?;
    pins.input(Pull::Up);
    static_slot::install(&PINS, pins);
    let boot = ports::claim_pin(BOOT_OWNER, BOOT_PIN)?;
    boot.input(Pull::Down);
    static_slot::install(&BOOT, boot);
    Ok(())
}

/// bootloader_pin_high -- the live level of the BOOTLOADER pin (PC10): true = logic 1. Reads
/// false before `init`.
pub fn bootloader_pin_high() -> bool {
    static_slot::with_mut(&BOOT, |p| p.read() != 0).unwrap_or(false)
}

/// nvmro_asserted -- the live NVMRO state: a fresh pin sample through the one decoding rule
/// (`addr::nvmro_asserted`, which honours the IPMB_NVMRO_SOURCE forced value). The flash driver
/// calls it immediately before every erase and every program; the main loop calls it for the
/// cached state that drives the Write FRU Data answer.
pub fn nvmro_asserted() -> bool {
    addr::nvmro_asserted(read_raw())
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
