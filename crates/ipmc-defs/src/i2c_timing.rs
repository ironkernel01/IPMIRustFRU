//! i2c_timing.rs - the arithmetic from the IPMB timing defines to core clock cycles and I2C
//! register values, for STANDARD-MODE (100 kHz) and FAST-MODE (400 kHz, VITA 46.11 Rule 4.1.1-4.4).
//!
//! Hardware-free so the host tests check it: the firmware (ipmc-nucleo-f401re bsp/i2c_bitbang.rs,
//! bsp/i2c_hw.rs, bsp/ipmb_common.rs) only applies the numbers. Every time here comes from
//! ipmi_defs.h (`IPMB_BITBANG_HALF_NS`, `IPMB_BITBANG_FAST_LOW_NS`, `IPMB_BITBANG_FAST_HIGH_NS`,
//! `IPMB_I2C_HZ`, `IPMB_I2C_FAST_HZ`) or from the I2C-bus specification (the minimum high times).

use crate::{
    IPMB_BITBANG_FAST_HIGH_NS, IPMB_BITBANG_FAST_LOW_NS, IPMB_BITBANG_HALF_NS, IPMB_I2C_FAST_HZ,
    IPMB_I2C_HZ,
};

/// NS_PER_S -- nanoseconds in a second.
const NS_PER_S: u64 = 1_000_000_000;
/// T_HIGH_MIN_STANDARD_NS -- SCL high time, standard mode: at least 4.0 us (I2C-bus specification
/// Table 5, tHIGH).
pub const T_HIGH_MIN_STANDARD_NS: u32 = 4_000;
/// T_HIGH_MIN_FAST_NS -- SCL high time, fast mode: at least 0.6 us (I2C-bus specification Table 5,
/// tHIGH; VITA 46.11 Table 9.2.2-1 M0_P1b).
pub const T_HIGH_MIN_FAST_NS: u32 = 600;
/// FAST_DIVIDER -- fast mode with DUTY = 0: tLOW = 2 x tHIGH, so one SCL period is 3 CCR x TPCLK1
/// (RM0368 18.6.8); standard mode is 2 CCR x TPCLK1.
const FAST_DIVIDER: u64 = 3;
const STANDARD_DIVIDER: u64 = 2;
/// TRISE_STANDARD_NS and TRISE_FAST_NS -- the maximum SCL rise time the TRISE register is
/// programmed for: 1000 ns in standard mode, 300 ns in fast mode (RM0368 18.6.9).
const TRISE_STANDARD_NS: u32 = 1_000;
const TRISE_FAST_NS: u32 = 300;
/// NS_PER_US -- nanoseconds in a microsecond (TRISE counts PCLK1 cycles of FREQ MHz).
const NS_PER_US: u32 = 1_000;

/// cycles_for -- ceil(ns x core_hz / 1e9) as u32 cycles, None when it does not fit.
pub fn cycles_for(ns: u64, core_hz: u32) -> Option<u32> {
    let cycles = ns.checked_mul(u64::from(core_hz))?.div_ceil(NS_PER_S);
    u32::try_from(cycles).ok()
}

/// BitPhases -- the SCL phase lengths of the bit-bang master at one speed, in core clock cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitPhases {
    /// SCL low phase of a bit: a half bit in standard mode, IPMB_BITBANG_FAST_LOW_NS in fast mode.
    pub low: u32,
    /// SCL high phase: a half bit in standard mode, IPMB_BITBANG_FAST_HIGH_NS in fast mode.
    pub high: u32,
    /// Slack of the high phase: `high` less the minimum high time of the mode. A rise (plus the
    /// poll that sees it) up to this long is absorbed inside the phase; a longer wait (a
    /// stretched clock) extends it so SCL stays high for the minimum after it was seen high.
    pub rise_allow: u32,
}

impl BitPhases {
    /// standard -- 100 kHz: both phases are IPMB_BITBANG_HALF_NS. None when the clock is zero, a
    /// time overflows 32 bits, or a half bit rounds to zero cycles.
    pub fn standard(core_hz: u32) -> Option<BitPhases> {
        BitPhases::of(
            core_hz,
            IPMB_BITBANG_HALF_NS,
            IPMB_BITBANG_HALF_NS,
            T_HIGH_MIN_STANDARD_NS,
        )
    }

    /// fast -- 400 kHz: low IPMB_BITBANG_FAST_LOW_NS, high IPMB_BITBANG_FAST_HIGH_NS (None as
    /// `standard`).
    pub fn fast(core_hz: u32) -> Option<BitPhases> {
        BitPhases::of(
            core_hz,
            IPMB_BITBANG_FAST_LOW_NS,
            IPMB_BITBANG_FAST_HIGH_NS,
            T_HIGH_MIN_FAST_NS,
        )
    }

    fn of(core_hz: u32, low_ns: u32, high_ns: u32, high_min_ns: u32) -> Option<BitPhases> {
        let (low, high) = (
            cycles_for(u64::from(low_ns), core_hz)?,
            cycles_for(u64::from(high_ns), core_hz)?,
        );
        if low == 0 || high == 0 {
            return None;
        }
        let high_min = cycles_for(u64::from(high_min_ns), core_hz)?;
        Some(BitPhases {
            low,
            high,
            rise_allow: high.saturating_sub(high_min),
        })
    }
}

/// hw_ccr -- the CCR divisor of the I2C peripheral for `pclk1_hz` (RM0368 18.6.8): standard
/// mode ceil(PCLK1 / (2 x IPMB_I2C_HZ)); fast mode (F/S = 1, DUTY = 0) ceil(PCLK1 / (3 x
/// IPMB_I2C_FAST_HZ)), where SCL low lasts 2 CCR and SCL high CCR of PCLK1. Rounding up keeps SCL
/// at or below the nominal rate. None when the product overflows.
pub fn hw_ccr(pclk1_hz: u32, fast: bool) -> Option<u32> {
    let (divider, hz) = if fast {
        (FAST_DIVIDER, IPMB_I2C_FAST_HZ)
    } else {
        (STANDARD_DIVIDER, IPMB_I2C_HZ)
    };
    let per_pclk = divider.checked_mul(u64::from(hz))?;
    u32::try_from(u64::from(pclk1_hz).div_ceil(per_pclk)).ok()
}

/// hw_trise -- the TRISE register for `freq_mhz` (CR2.FREQ): FREQ + 1 for the 1000 ns standard
/// rise time, FREQ x 300 ns + 1 for fast mode (RM0368 18.6.9).
pub fn hw_trise(freq_mhz: u32, fast: bool) -> u32 {
    let rise_ns = if fast {
        TRISE_FAST_NS
    } else {
        TRISE_STANDARD_NS
    };
    freq_mhz * rise_ns / NS_PER_US + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_round_up_and_overflow_is_none() {
        // (ns, core Hz, cycles): 1400 ns at 84 MHz is 117.6 cycles, 1000 ns is 84.
        for (ns, hz, cycles) in [
            (1_400u64, 84_000_000u32, Some(118u32)),
            (1_000, 84_000_000, Some(84)),
            (600, 84_000_000, Some(51)),
            (5_000, 84_000_000, Some(420)),
            (0, 84_000_000, Some(0)),
            (1, 0, Some(0)),
        ] {
            assert_eq!(cycles_for(ns, hz), cycles, "{ns} ns at {hz} Hz");
        }
        assert_eq!(cycles_for(u64::MAX / 2, u32::MAX), None);
    }

    #[test]
    fn the_bit_phases_at_84_mhz_meet_the_i2c_minimums() {
        let core_hz = 84_000_000;
        let (std, fast) = (
            BitPhases::standard(core_hz).unwrap(),
            BitPhases::fast(core_hz).unwrap(),
        );
        // Standard: both phases 5000 ns = 420 cycles, slack 420 - 336 (4000 ns).
        assert_eq!(
            std,
            BitPhases {
                low: 420,
                high: 420,
                rise_allow: 84
            }
        );
        // Fast: low 1400 ns = 118 cycles (1.405 us), high 1000 ns = 84 cycles, slack 84 - 51
        // (600 ns); the half-bit counts the board runs at 400 kHz.
        assert_eq!(
            fast,
            BitPhases {
                low: 118,
                high: 84,
                rise_allow: 33
            }
        );
        let ns = |cycles: u32| u64::from(cycles) * NS_PER_S / u64::from(core_hz);
        assert!(ns(fast.low) >= 1_300, "tLOW of fast mode");
        assert!(
            ns(fast.high) >= u64::from(T_HIGH_MIN_FAST_NS),
            "tHIGH of fast mode"
        );
        // The two phases are 2.4 us; the pin operations and the pull-up rise add the rest of
        // the 2.5 us period, so the nominal clock is at most 400 kHz plus that overhead.
        assert_eq!(ns(fast.low + fast.high), 2_404);
    }

    #[test]
    fn a_clock_that_rounds_a_phase_to_zero_is_refused() {
        assert_eq!(BitPhases::standard(0), None);
        assert_eq!(BitPhases::fast(0), None);
    }

    #[test]
    fn the_peripheral_divisors_follow_rm0368() {
        // (PCLK1, fast, CCR, TRISE at FREQ = PCLK1 in MHz).
        for (pclk1, fast, ccr, trise) in [
            // Standard mode: ceil(42 MHz / 200 kHz) = 210; TRISE = 42 + 1.
            (42_000_000u32, false, 210u32, 43u32),
            // Fast mode: ceil(42 MHz / 1.2 MHz) = 35 (SCL low 70, high 35 PCLK1 cycles =
            // 1.667 us and 0.833 us, a 2.5 us period); TRISE = 42 x 300 / 1000 + 1 = 13.
            (42_000_000, true, 35, 13),
            // 30 MHz: standard 150; fast ceil(30 / 1.2) = 25.
            (30_000_000, false, 150, 31),
            (30_000_000, true, 25, 10),
        ] {
            assert_eq!(hw_ccr(pclk1, fast), Some(ccr), "{pclk1} Hz fast {fast}");
            assert_eq!(
                hw_trise(pclk1 / 1_000_000, fast),
                trise,
                "{pclk1} Hz fast {fast}"
            );
        }
        // Fast mode keeps tLOW = 2 x CCR x TPCLK1 above 1.3 us at PCLK1 = 42 MHz.
        let ccr = u64::from(hw_ccr(42_000_000, true).unwrap());
        assert!(2 * ccr * NS_PER_S / 42_000_000 >= 1_300);
    }
}
