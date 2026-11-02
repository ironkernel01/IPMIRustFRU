// drivers/ipmb_pins.rs - claims the four IPMB pins and parks them as floating inputs: PB8, PB9 (IPMB-A
// I2C1 SCL/SDA), PA8 (IPMB-B I2C3 SCL) and PB4 (IPMB-B I2C3 SDA).
//
// Ownership: this module holds the claims (owner "ipmb"); the handles it returns go to the
// bit-bang master (drivers/i2c_bitbang.rs), which owns the pins while it is the master, and are
// lent to the I2C driver (drivers/i2c_hw.rs) to select the alternate function. The pin numbers live
// only here; the alternate-function numbers (AF4 for PB8, PB9, PA8; AF9 for PB4) live in
// i2c_hw.rs, which names this file as their peer: a pin change here needs the matching AF there,
// or the bus stays silent.
// VITA 46.11 Rule 9.2.3-2 forbids IPMC pull-ups on the IPMB lines; the backplane supplies them.
// defs.rs IPMB_PULL_UPS selects the pull of every IPMB pin mode (`PULL`): none, as the rule asks,
// or the internal pull-up (owner bench deviation), so an unwired bus reads idle-high. `BusPins`
// owns the three modes (`idle`, `drive`, `alternate`), so no mode change drops the pull. PB4
// resets as AF0 NJTRST with an internal pull-up (RM0368 8.4.1, 8.4.4), so the reset state is not
// used; all four pins are written here so the state does not depend on reset defaults.
// When IPMB_BUS_B_ENABLE is false the IPMB-B pins stay parked and nothing else touches them.

use crate::bsp::ports::{self, PinId, Pins, PortError, PortId, Pull, Speed};
use ipmc_core::queue::Bus;

const OWNER: &str = "ipmb";

/// PULL -- the pull of the IPMB pins in every mode (defs.rs IPMB_PULL_UPS; Rule 9.2.3-2).
const PULL: Pull = if ipmc_defs::IPMB_PULL_UPS {
    Pull::Up
} else {
    Pull::None
};

/// SPEED -- the output speed of the IPMB pins when driven or on the I2C peripheral.
const SPEED: Speed = Speed::Medium;

const fn pin(port: PortId, pin: u8) -> PinId {
    PinId { port, pin }
}

const A_SCL: PinId = pin(PortId::B, 8);
const A_SDA: PinId = pin(PortId::B, 9);
const B_SCL: PinId = pin(PortId::A, 8);
const B_SDA: PinId = pin(PortId::B, 4);

/// BusPins -- the claimed SCL and SDA pins of one IPMB.
#[derive(Debug)]
pub struct BusPins {
    pub scl: Pins,
    pub sda: Pins,
}

/// IpmbPins -- the pins of both buses.
#[derive(Debug)]
pub struct IpmbPins {
    pub a: BusPins,
    pub b: BusPins,
}

impl BusPins {
    /// idle -- both pins inputs (released), with the IPMB pull.
    pub fn idle(&self) {
        self.scl.input(PULL);
        self.sda.input(PULL);
    }

    /// drive -- both pins open-drain outputs, released high, with the IPMB pull: the bit-bang
    /// master and the line checks drive the bus this way.
    pub fn drive(&self) {
        self.scl.open_drain(PULL, SPEED);
        self.sda.open_drain(PULL, SPEED);
    }

    /// alternate -- both pins on the I2C peripheral (`scl_af`, `sda_af`), open-drain, with the
    /// IPMB pull.
    pub fn alternate(&self, scl_af: u8, sda_af: u8) {
        self.scl.alternate(scl_af, PULL, true, SPEED);
        self.sda.alternate(sda_af, PULL, true, SPEED);
    }
}

impl IpmbPins {
    /// bus -- the pins of `bus`; the one bus-to-pins lookup of both masters.
    pub fn bus(&self, bus: Bus) -> &BusPins {
        match bus {
            Bus::A => &self.a,
            Bus::B => &self.b,
        }
    }
}

/// claim_bus -- claims the two pins of one bus.
fn claim_bus(scl: PinId, sda: PinId) -> Result<BusPins, PortError> {
    Ok(BusPins {
        scl: ports::claim_pin(OWNER, scl)?,
        sda: ports::claim_pin(OWNER, sda)?,
    })
}

/// park -- claims PB8, PB9, PA8 and PB4 (each its own claim) and makes them inputs with the IPMB
/// pull (`idle`). Returns the handles; the caller hands them to the I2C driver.
pub fn park() -> Result<IpmbPins, PortError> {
    let pins = IpmbPins {
        a: claim_bus(A_SCL, A_SDA)?,
        b: claim_bus(B_SCL, B_SDA)?,
    };
    pins.a.idle();
    pins.b.idle();
    Ok(pins)
}
