//! addr.rs - geographic address (VITA 46.11) decode and strap-source resolution.
//!
//! Raw pin word convention: bit n = electrical level of PC(n), 1 = high.
//! GA0..GA4 are bits 0..4, GAP is bit 5, NVMRO is bit 6.

/// Number of GA bits (GA0..GA4).
const GA_BITS: u8 = 5;
/// Mask for GA[4:0].
const GA_MASK: u8 = (1 << GA_BITS) - 1;
/// Bit of GAP in the raw word.
const GAP_BIT: u8 = GA_BITS;
/// Mask for GA0..GA4 and GAP in the raw word.
const GA_PINS_MASK: u8 = GA_MASK | (1 << GAP_BIT);
/// Bit of NVMRO in the raw word.
const NVMRO_BIT: u8 = GAP_BIT + 1;
/// Mask for every strap pin in the raw word.
const ALL_PINS_MASK: u8 = GA_PINS_MASK | (1 << NVMRO_BIT);
/// Hardware Address = HW_ADDRESS_BASE + GA[4:0] (VITA 46.11 geographic addressing).
const HW_ADDRESS_BASE: u8 = 0x40;

/// Source -- where a strap value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Read from the pins.
    Pins,
    /// Taken from ipmi_defs.h.
    Forced,
}

impl Source {
    /// text -- log text for the source; returns a static string.
    pub const fn text(self) -> &'static str {
        match self {
            Source::Pins => "from pins",
            Source::Forced => "forced by ipmi_defs.h",
        }
    }
}

/// AddrInfo -- decoded geographic address and NVMRO strap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddrInfo {
    /// Electrical pin levels as read, 1 = high.
    pub raw: u8,
    /// GA[4:0] as logic values (grounded pin = 1).
    pub ga: u8,
    /// 7-bit hardware address.
    pub hw_address: u8,
    /// 8-bit IPMB slave address.
    pub ipmb_address: u8,
    /// True when the count of logic-1 bits among GA0..GA4 and GAP is odd.
    pub parity_ok: bool,
    /// False when GA[4:0] = 0.
    pub valid: bool,
    /// True when NVMRO is asserted (FRU writes refused).
    pub nvmro_asserted: bool,
    /// Source of GA and GAP.
    pub ga_source: Source,
    /// Source of NVMRO.
    pub nvmro_source: Source,
}

impl AddrInfo {
    /// usable -- the hardware address is valid: odd parity (VITA Rule 4.1.5.4.1-4) and GA[4:0]
    /// nonzero. The FRU initial checks and the link layer's power-up state both read this one
    /// decision (Rule 4.1.5.4.1-5: an invalid address leaves the IPMC isolated from both buses).
    pub const fn usable(&self) -> bool {
        self.parity_ok && self.valid
    }

    /// ga_pin_levels -- the electrical levels of GA4..GA0 as read (bit n = GAn, 1 = high).
    pub const fn ga_pin_levels(&self) -> u8 {
        self.raw & GA_MASK
    }

    /// gap_pin_high -- the electrical level of GAP as read.
    pub const fn gap_pin_high(&self) -> bool {
        self.raw & (1 << GAP_BIT) != 0
    }

    /// nvmro_pin_high -- the electrical level of NVMRO as read.
    pub const fn nvmro_pin_high(&self) -> bool {
        self.raw & (1 << NVMRO_BIT) != 0
    }
}

/// nvmro_from_pins -- NVMRO state from a raw word; returns true when asserted.
const fn nvmro_from_pins(raw: u8) -> bool {
    // VERIFY: VITA 46.0 NVMRO polarity
    raw & (1 << NVMRO_BIT) != 0
}

/// nvmro_asserted -- NVMRO state for a raw pin word under IPMB_NVMRO_SOURCE: the forced value
/// of ipmi_defs.h when the source is forced, else the NVMRO pin level. The one implementation
/// `resolve` and the run-time NVMRO sampling share.
pub fn nvmro_asserted(raw: u8) -> bool {
    nvmro_for(
        ipmc_defs::IPMB_NVMRO_SOURCE == ipmc_defs::IPMB_STRAP_FORCED,
        ipmc_defs::IPMB_NVMRO_FORCED == 1,
        raw,
    )
}

/// nvmro_for -- the source rule of `nvmro_asserted` with the configuration passed in.
const fn nvmro_for(forced_source: bool, forced_value: bool, raw: u8) -> bool {
    if forced_source {
        forced_value
    } else {
        nvmro_from_pins(raw & ALL_PINS_MASK)
    }
}

/// Inputs to `build`: the GA logic word and where each value came from.
struct Strap {
    raw: u8,
    /// GA[4:0] in bits 0..4, GAP in bit 5, logic values (grounded pin = 1).
    logic: u8,
    nvmro_asserted: bool,
    ga_source: Source,
    nvmro_source: Source,
}

/// build -- fills an AddrInfo from the strap values; returns the AddrInfo.
const fn build(s: Strap) -> AddrInfo {
    let ga = s.logic & GA_MASK;
    let hw_address = HW_ADDRESS_BASE + ga;
    AddrInfo {
        raw: s.raw,
        ga,
        hw_address,
        ipmb_address: hw_address * 2,
        parity_ok: (s.logic & GA_PINS_MASK).count_ones() % 2 == 1,
        valid: ga != 0,
        nvmro_asserted: s.nvmro_asserted,
        ga_source: s.ga_source,
        nvmro_source: s.nvmro_source,
    }
}

/// pins_logic -- GA/GAP logic word from a raw word (a pin at electrical 0 is logic 1).
const fn pins_logic(raw: u8) -> u8 {
    !raw & GA_PINS_MASK
}

/// decode -- pure decode of a raw pin word, every value from the pins.
/// Hardware Address = 0x40 + GA[4:0]; IPMB address = 2 x Hardware Address.
/// Returns the decoded AddrInfo.
pub const fn decode(raw: u8) -> AddrInfo {
    let raw = raw & ALL_PINS_MASK;
    build(Strap {
        raw,
        logic: pins_logic(raw),
        nvmro_asserted: nvmro_from_pins(raw),
        ga_source: Source::Pins,
        nvmro_source: Source::Pins,
    })
}

/// forced_logic -- GA logic word for a forced GA with GAP generated for odd parity.
const fn forced_logic(ga: u8) -> u8 {
    let ga = ga & GA_MASK;
    let gap = if ga.count_ones().is_multiple_of(2) {
        1
    } else {
        0
    };
    ga | (gap << GAP_BIT)
}

/// resolve -- applies IPMB_GA_SOURCE and IPMB_NVMRO_SOURCE to a raw pin word.
/// A forced GA gets a generated GAP, so its parity is valid.
/// Returns the resolved AddrInfo; `raw` keeps the pins as read.
pub fn resolve(raw_pins: u8) -> AddrInfo {
    let raw = raw_pins & ALL_PINS_MASK;
    let (logic, ga_source) = if ipmc_defs::IPMB_GA_SOURCE == ipmc_defs::IPMB_STRAP_FORCED {
        (
            forced_logic(ipmc_defs::IPMB_GA_FORCED as u8),
            Source::Forced,
        )
    } else {
        (pins_logic(raw), Source::Pins)
    };
    let nvmro_source = if ipmc_defs::IPMB_NVMRO_SOURCE == ipmc_defs::IPMB_STRAP_FORCED {
        Source::Forced
    } else {
        Source::Pins
    };
    let nvmro_asserted = nvmro_asserted(raw);
    build(Strap {
        raw,
        logic,
        nvmro_asserted,
        ga_source,
        nvmro_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_pin_levels_split_into_ga_gap_and_nvmro() {
        let a = decode(0b101_1010);
        assert_eq!(a.ga_pin_levels(), 0b1_1010);
        assert!(!a.gap_pin_high());
        assert!(a.nvmro_pin_high());
        let b = decode(0b010_0101);
        assert_eq!(b.ga_pin_levels(), 0b0_0101);
        assert!(b.gap_pin_high());
        assert!(!b.nvmro_pin_high());
    }

    #[test]
    fn slot1_decodes_to_82() {
        // GA0 grounded, GA1..GA4 and GAP high, NVMRO low.
        let a = decode(0x3E);
        assert_eq!(a.ga, 1);
        assert_eq!(a.hw_address, 0x41);
        assert_eq!(a.ipmb_address, 0x82);
        assert!(a.parity_ok && a.valid && !a.nvmro_asserted);
    }

    #[test]
    fn slot31_decodes_to_be() {
        // GA = 0x1F (five ones) with GAP high (logic 0): odd parity.
        let a = decode(0x20);
        assert_eq!(a.ga, 0x1F);
        assert_eq!(a.hw_address, 0x5F);
        assert_eq!(a.ipmb_address, 0xBE);
        assert!(a.parity_ok);
    }

    #[test]
    fn bad_parity_detected() {
        // GA = 1 with GAP also logic 1: two ones, even.
        assert!(!decode(0x1E).parity_ok);
    }

    #[test]
    fn usable_needs_odd_parity_and_a_nonzero_ga() {
        assert!(decode(0x1A).usable(), "GA 5, odd parity");
        assert!(!decode(0x1E).usable(), "GA 1 with GAP set: even parity");
        assert!(!decode(0x7F).usable(), "GA 0");
    }

    #[test]
    fn ga_zero_is_invalid() {
        let a = decode(0x3F);
        assert_eq!(a.ga, 0);
        assert!(!a.valid);
    }

    #[test]
    fn nvmro_high_is_asserted_from_pins() {
        assert!(decode(0x7E).nvmro_asserted);
        assert!(!decode(0x3E).nvmro_asserted);
    }

    #[test]
    fn forced_logic_always_has_valid_parity() {
        for ga in 1..=GA_MASK {
            let logic = forced_logic(ga);
            assert_eq!((logic & GA_PINS_MASK).count_ones() % 2, 1, "ga {ga}");
            assert_eq!(logic & GA_MASK, ga);
        }
    }

    #[test]
    fn resolve_uses_forced_defaults() {
        // Pins read as all high (GA 0, invalid) do not matter when forced.
        let a = resolve(0x7F);
        assert_eq!(a.ga, ipmc_defs::IPMB_GA_FORCED as u8);
        assert_eq!(a.ipmb_address, 0x82);
        assert!(a.parity_ok && a.valid);
        assert!(!a.nvmro_asserted);
        assert_eq!(a.raw, 0x7F);
        assert_eq!(a.ga_source, Source::Forced);
        assert_eq!(a.nvmro_source, Source::Forced);
    }

    #[test]
    fn nvmro_rule_covers_both_sources() {
        assert!(
            nvmro_for(true, true, 0x3F),
            "forced asserted ignores a low pin"
        );
        assert!(
            !nvmro_for(true, false, 0x7F),
            "forced clear ignores a high pin"
        );
        assert!(nvmro_for(false, false, 0x7F), "pins: high is asserted");
        assert!(!nvmro_for(false, true, 0x3F), "pins: low is clear");
    }

    #[test]
    fn nvmro_asserted_honors_the_source() {
        // Pin high (bit 6) and pin low; the result follows the configured source.
        let (high, low) = (nvmro_asserted(0x7F), nvmro_asserted(0x3F));
        if ipmc_defs::IPMB_NVMRO_SOURCE == ipmc_defs::IPMB_STRAP_FORCED {
            let forced = ipmc_defs::IPMB_NVMRO_FORCED == 1;
            assert_eq!((high, low), (forced, forced), "forced ignores the pin");
        } else {
            assert_eq!((high, low), (true, false), "pins: high = asserted");
        }
        assert_eq!(resolve(0x7F).nvmro_asserted, high);
        assert_eq!(resolve(0x3F).nvmro_asserted, low);
    }

    #[test]
    fn source_text() {
        assert_eq!(Source::Pins.text(), "from pins");
        assert_eq!(Source::Forced.text(), "forced by ipmi_defs.h");
    }
}
