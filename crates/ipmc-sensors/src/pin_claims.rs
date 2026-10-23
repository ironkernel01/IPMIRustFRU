//! pin_claims - hardware-free bookkeeping of GPIO pin ownership and the register-field mask
//! arithmetic, so it builds and is tested on the host. `drivers::stm32f401::ports` applies it
//! to the real ports.
//!
//! One owner per pin: `ClaimTable::claim` records the owner name of each pin of a mask and
//! refuses a pin that has an owner, naming it. A refused claim changes nothing.

/// PortId -- the three GPIO ports of the STM32F401RE package in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortId {
    A,
    B,
    C,
}

impl PortId {
    /// COUNT -- number of ports.
    pub const COUNT: usize = 3;

    /// letter -- the port letter as printed in a pin name (PA5 is port A).
    pub fn letter(self) -> char {
        match self {
            PortId::A => 'A',
            PortId::B => 'B',
            PortId::C => 'C',
        }
    }

    fn index(self) -> usize {
        match self {
            PortId::A => 0,
            PortId::B => 1,
            PortId::C => 2,
        }
    }
}

/// PINS_PER_PORT -- pins per port.
pub const PINS_PER_PORT: usize = 16;

/// PinId -- one pin by port and number, as the board support names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinId {
    pub port: PortId,
    pub pin: u8,
}

impl PinId {
    /// mask -- the claim mask of the pin (bit n = pin n); `Err(InvalidPin)` above pin 15.
    pub const fn mask(self) -> Result<u16, PortError> {
        if (self.pin as usize) < PINS_PER_PORT {
            Ok(1 << self.pin)
        } else {
            Err(PortError::InvalidPin)
        }
    }
}

/// PortError -- why a claim or `ports::init` failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortError {
    /// A pin of the mask already has an owner. `owner` holds it, `claimant` asked for it.
    AlreadyClaimed {
        port: PortId,
        pin: u8,
        owner: &'static str,
        claimant: &'static str,
    },
    /// The mask names no pin.
    EmptyMask,
    /// The owner name is empty; an empty name means "unclaimed" in the table, so it cannot own.
    EmptyOwner,
    /// A pin number above 15.
    InvalidPin,
    /// `claim` ran before `ports::init` took the ports.
    NotInitialised,
    /// `ports::init` ran twice.
    AlreadyInitialised,
}

/// ClaimTable -- the owner name of every pin of every port (empty = free).
pub struct ClaimTable {
    owners: [[&'static str; PINS_PER_PORT]; PortId::COUNT],
}

impl ClaimTable {
    /// new -- an empty table: every pin free.
    pub const fn new() -> Self {
        ClaimTable {
            owners: [[""; PINS_PER_PORT]; PortId::COUNT],
        }
    }

    /// claim -- records `owner` for every pin in `mask` (bit n = pin n) of `port`. Nothing is
    /// recorded when any pin is taken, the mask is empty or `owner` is empty.
    pub fn claim(&mut self, owner: &'static str, port: PortId, mask: u16) -> Result<(), PortError> {
        if owner.is_empty() {
            return Err(PortError::EmptyOwner);
        }
        if mask == 0 {
            return Err(PortError::EmptyMask);
        }
        let row = &mut self.owners[port.index()];
        for (pin, slot) in row.iter().enumerate() {
            if mask & (1 << pin) != 0 && !slot.is_empty() {
                return Err(PortError::AlreadyClaimed {
                    port,
                    pin: pin as u8,
                    owner: slot,
                    claimant: owner,
                });
            }
        }
        for (pin, slot) in row.iter_mut().enumerate() {
            if mask & (1 << pin) != 0 {
                *slot = owner;
            }
        }
        Ok(())
    }

    /// owner_of -- the owner of one pin, or None while free.
    pub fn owner_of(&self, port: PortId, pin: u8) -> Option<&'static str> {
        let name = *self.owners[port.index()].get(usize::from(pin))?;
        (!name.is_empty()).then_some(name)
    }
}

impl Default for ClaimTable {
    fn default() -> Self {
        Self::new()
    }
}

/// spread2 -- bit n of `mask` becomes bit 2n (the low bit of that pin's 2-bit field).
const fn spread2(mask: u16) -> u32 {
    let mut out = 0;
    let mut pin = 0;
    while pin < 16 {
        if mask & (1 << pin) != 0 {
            out |= 1 << (2 * pin);
        }
        pin += 1;
    }
    out
}

/// field2_mask -- the 2-bit MODER / PUPDR / OSPEEDR fields of the pins in `mask`.
pub const fn field2_mask(mask: u16) -> u32 {
    spread2(mask) * 3
}

/// field2_value -- `value` (0..=3) in each of those fields.
pub const fn field2_value(mask: u16, value: u32) -> u32 {
    spread2(mask) * (value & 3)
}

/// af_mask -- the 4-bit alternate-function fields of the pins of `mask` that sit in AFRL
/// (`high` false: pins 0..=7) or AFRH (`high` true: pins 8..=15).
pub const fn af_mask(mask: u16, high: bool) -> u32 {
    af_value(mask, high, 0xF)
}

/// af_value -- `af` (0..=15) in each of those fields.
pub const fn af_value(mask: u16, high: bool, af: u32) -> u32 {
    let base = if high { 8 } else { 0 };
    let mut out = 0;
    let mut i = 0;
    while i < 8 {
        if mask & (1 << (base + i)) != 0 {
            out |= (af & 0xF) << (4 * i);
        }
        i += 1;
    }
    out
}

/// merge -- `old` with the bits of `field` replaced by those of `value`; all other bits keep.
pub const fn merge(old: u32, field: u32, value: u32) -> u32 {
    (old & !field) | (value & field)
}

/// bsrr_word -- the BSRR word that drives the pins of `mask` to `levels` (bit n = 1 high) and
/// touches no other pin: low half sets, high half resets (RM0368 8.4.7).
pub const fn bsrr_word(mask: u16, levels: u16) -> u32 {
    let set = (levels & mask) as u32;
    let reset = (!levels & mask) as u32;
    set | (reset << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_succeed_on_free_pins_and_record_owner() {
        let mut t = ClaimTable::new();
        assert_eq!(t.claim("led", PortId::A, 1 << 5), Ok(()));
        assert_eq!(t.claim("uart", PortId::A, 0b1100), Ok(()));
        assert_eq!(t.claim("ga", PortId::C, 0x7F), Ok(()));
        assert_eq!(t.owner_of(PortId::A, 5), Some("led"));
        assert_eq!(t.owner_of(PortId::A, 2), Some("uart"));
        assert_eq!(t.owner_of(PortId::A, 4), None);
        assert_eq!(t.owner_of(PortId::B, 5), None);
        assert_eq!(t.owner_of(PortId::A, 16), None);
    }

    #[test]
    fn same_pin_on_another_port_is_free() {
        let mut t = ClaimTable::new();
        t.claim("a", PortId::A, 1 << 8).unwrap();
        assert_eq!(t.claim("b", PortId::B, 1 << 8), Ok(()));
    }

    #[test]
    fn double_claim_names_both_owners_and_changes_nothing() {
        let mut t = ClaimTable::new();
        t.claim("led", PortId::A, 1 << 5).unwrap();
        assert_eq!(
            t.claim("scratch", PortId::A, (1 << 4) | (1 << 5)),
            Err(PortError::AlreadyClaimed {
                port: PortId::A,
                pin: 5,
                owner: "led",
                claimant: "scratch"
            })
        );
        // PA4 stayed free because the refused claim recorded nothing.
        assert_eq!(t.owner_of(PortId::A, 4), None);
        assert_eq!(t.claim("adc", PortId::A, 1 << 4), Ok(()));
    }

    #[test]
    fn empty_mask_is_refused() {
        assert_eq!(
            ClaimTable::new().claim("x", PortId::B, 0),
            Err(PortError::EmptyMask)
        );
    }

    #[test]
    fn empty_owner_is_refused_and_reserves_nothing() {
        let mut t = ClaimTable::new();
        assert_eq!(t.claim("", PortId::A, 1 << 3), Err(PortError::EmptyOwner));
        assert_eq!(t.owner_of(PortId::A, 3), None);
        // The pin is still claimable, by a named owner.
        assert_eq!(t.claim("adc", PortId::A, 1 << 3), Ok(()));
        assert_eq!(
            t.claim("other", PortId::A, 1 << 3),
            Err(PortError::AlreadyClaimed {
                port: PortId::A,
                pin: 3,
                owner: "adc",
                claimant: "other"
            })
        );
    }

    #[test]
    fn pin_id_makes_a_one_bit_mask() {
        let pin = |pin| PinId {
            port: PortId::B,
            pin,
        };
        assert_eq!(pin(0).mask(), Ok(1));
        assert_eq!(pin(15).mask(), Ok(1 << 15));
        assert_eq!(pin(16).mask(), Err(PortError::InvalidPin));
    }

    #[test]
    fn field_masks() {
        assert_eq!(field2_mask(1 << 5), 0b11 << 10);
        assert_eq!(field2_mask(0b101), 0b11_00_11);
        assert_eq!(field2_value(1 << 5, 1), 1 << 10);
        assert_eq!(field2_value(1 << 15, 3), 0b11 << 30);
        assert_eq!(field2_value(0b11, 2), 0b1010);
        assert_eq!(field2_mask(0), 0);
    }

    #[test]
    fn alternate_function_nibbles_split_between_low_and_high() {
        let mask = (1 << 2) | (1 << 3) | (1 << 9);
        assert_eq!(af_mask(mask, false), 0xFF00);
        assert_eq!(af_value(mask, false, 7), 0x7700);
        assert_eq!(af_mask(mask, true), 0xF0);
        assert_eq!(af_value(mask, true, 4), 0x40);
        assert_eq!(af_mask(1 << 8, true), 0xF);
        assert_eq!(af_mask(1 << 7, true), 0);
    }

    #[test]
    fn merge_keeps_other_bits() {
        let field = field2_mask(1 << 5);
        assert_eq!(merge(0xFFFF_FFFF, field, 0), 0xFFFF_F3FF);
        assert_eq!(merge(0, field, field2_value(1 << 5, 1)), 1 << 10);
        assert_eq!(merge(0x1234_5678, 0, 0xFFFF_FFFF), 0x1234_5678);
    }

    #[test]
    fn bsrr_sets_and_resets_only_masked_pins() {
        assert_eq!(bsrr_word(1 << 5, 1 << 5), 1 << 5);
        assert_eq!(bsrr_word(1 << 5, 0), 1 << 21);
        assert_eq!(bsrr_word(0b11, 0b01), 0b01 | (0b10 << 16));
        // Levels outside the mask are ignored.
        assert_eq!(bsrr_word(1 << 1, 0xFFFF), 1 << 1);
    }
}
