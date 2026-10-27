//! rail.rs - the rail definition and the one evaluation of a rail voltage against its windows.
//!
//! Every threshold decision for a rail goes through `evaluate`; the board table in
//! `board_sensors.rs` only supplies `RailDef` values.

use crate::hal::{Channel, Divider, SourceError};
use ipmc_core::sensor::RailStatus;

/// RailDef -- one monitored rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailDef {
    pub name: &'static str,
    pub channel: Channel,
    pub nominal_mv: u32,
    /// Rail mV = pin mV x num / den.
    pub divider: Divider,
    /// Non-critical window, +/- percent of nominal.
    pub noncrit_pct: u32,
    /// Critical window, +/- percent of nominal; larger than `noncrit_pct`.
    pub crit_pct: u32,
}

/// Percent per unit, for the window arithmetic.
const PERCENT: u64 = 100;

/// deviates_beyond -- true when |mv - nominal| is more than `pct` percent of nominal. The
/// comparison is deviation x 100 > pct x nominal in u64, so a window edge itself is in range.
fn deviates_beyond(def: &RailDef, mv: u32, pct: u32) -> bool {
    let deviation = u64::from(mv.abs_diff(def.nominal_mv));
    deviation * PERCENT > u64::from(pct) * u64::from(def.nominal_mv)
}

/// evaluate -- status of `mv` for `def`: `Critical` beyond the critical window, `NonCritical`
/// beyond the non-critical window, else `InRange`. Window edges are in range.
pub fn evaluate(def: &RailDef, mv: u32) -> RailStatus {
    if deviates_beyond(def, mv, def.crit_pct) {
        RailStatus::Critical
    } else if deviates_beyond(def, mv, def.noncrit_pct) {
        RailStatus::NonCritical
    } else {
        RailStatus::InRange
    }
}

/// edge_x100 -- the window edge of `pct` percent of `nominal_mv`, in hundredths of a millivolt:
/// nominal x (100 + pct) above, nominal x (100 - pct) below. A reading is past the edge exactly
/// when `mv x 100` is beyond it (`deviates_beyond`), so a threshold byte of an SDR record and the
/// status of a reading come from the same edge. `pct` is below 100 (ipmc-defs guard).
pub const fn edge_x100(nominal_mv: u32, pct: u32, upper: bool) -> u64 {
    let nominal = nominal_mv as u64;
    if upper {
        nominal * (PERCENT + pct as u64)
    } else {
        nominal * (PERCENT - pct as u64)
    }
}

/// scale_mv -- pin millivolts to rail millivolts through `divider`, rounded to nearest.
/// `Err(InvalidDivisor)` for a zero denominator, `Err(Overflow)` when the result does not fit
/// u32; the ipmc-defs guards keep every board divider nonzero and in range.
pub fn scale_mv(pin_mv: u32, divider: Divider) -> Result<u32, SourceError> {
    if divider.den == 0 {
        return Err(SourceError::InvalidDivisor);
    }
    // u32 x u32 plus den / 2 (below 2^31) fits u64.
    let scaled = u64::from(pin_mv) * u64::from(divider.num);
    let den = u64::from(divider.den);
    u32::try_from((scaled + den / 2) / den).map_err(|_| SourceError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE_VOLT: RailDef = RailDef {
        name: "5V",
        channel: Channel::Adc(1),
        nominal_mv: 5000,
        divider: Divider { num: 2, den: 1 },
        noncrit_pct: 5,
        crit_pct: 10,
    };

    #[test]
    fn edges_are_in_range_and_one_millivolt_past_is_not() {
        // 5 % of 5000 = 250: 4750..=5250 in range.
        assert_eq!(evaluate(&FIVE_VOLT, 5000), RailStatus::InRange);
        assert_eq!(evaluate(&FIVE_VOLT, 5250), RailStatus::InRange);
        assert_eq!(evaluate(&FIVE_VOLT, 4750), RailStatus::InRange);
        assert_eq!(evaluate(&FIVE_VOLT, 5251), RailStatus::NonCritical);
        assert_eq!(evaluate(&FIVE_VOLT, 4749), RailStatus::NonCritical);
        // 10 % of 5000 = 500: 4500..=5500 non-critical at most.
        assert_eq!(evaluate(&FIVE_VOLT, 5500), RailStatus::NonCritical);
        assert_eq!(evaluate(&FIVE_VOLT, 4500), RailStatus::NonCritical);
        assert_eq!(evaluate(&FIVE_VOLT, 5501), RailStatus::Critical);
        assert_eq!(evaluate(&FIVE_VOLT, 4499), RailStatus::Critical);
    }

    #[test]
    fn rail_two_at_4400_is_critical_and_at_5000_in_range() {
        // |4400 - 5000| = 600 mV = 12 % of 5000, above the 10 % critical window.
        assert_eq!(evaluate(&FIVE_VOLT, 4400), RailStatus::Critical);
        assert_eq!(evaluate(&FIVE_VOLT, 5000), RailStatus::InRange);
        assert_eq!(evaluate(&FIVE_VOLT, 0), RailStatus::Critical);
        assert_eq!(evaluate(&FIVE_VOLT, u32::MAX), RailStatus::Critical);
    }

    #[test]
    fn the_edges_are_the_ones_evaluate_uses() {
        // edge_x100 / 100 mV is the last in-range voltage: 5 % of 5000 mV is 5250 and 4750 mV.
        assert_eq!(edge_x100(5000, 5, true), 525_000);
        assert_eq!(edge_x100(5000, 5, false), 475_000);
        for (pct, upper) in [(5, true), (5, false), (10, true), (10, false)] {
            let edge_mv = (edge_x100(5000, pct, upper) / 100) as u32;
            let beyond = if upper { edge_mv + 1 } else { edge_mv - 1 };
            let status = |mv| evaluate(&FIVE_VOLT, mv);
            let (inside, outside) = (status(edge_mv), status(beyond));
            assert_eq!(inside == RailStatus::InRange, pct == 5, "{pct} {upper}");
            assert_ne!(inside, outside, "{pct} {upper}");
        }
    }

    #[test]
    fn scale_rounds_to_nearest() {
        // 2500 mV x 2 / 1 = 5000; 3 mV x 4 / 3 = 4; 1 x 1 / 2 = 0.5 rounds to 1.
        assert_eq!(scale_mv(2500, Divider { num: 2, den: 1 }), Ok(5000));
        assert_eq!(scale_mv(3, Divider { num: 4, den: 3 }), Ok(4));
        assert_eq!(scale_mv(1, Divider { num: 1, den: 2 }), Ok(1));
    }

    #[test]
    fn scale_names_a_zero_denominator_and_an_overflow() {
        assert_eq!(
            scale_mv(1, Divider { num: 1, den: 0 }),
            Err(SourceError::InvalidDivisor)
        );
        // u32::MAX x 2 / 1 does not fit u32: an error, not a clamp.
        assert_eq!(
            scale_mv(u32::MAX, Divider { num: 2, den: 1 }),
            Err(SourceError::Overflow)
        );
        // The largest result that fits is still returned.
        assert_eq!(scale_mv(u32::MAX, Divider { num: 1, den: 1 }), Ok(u32::MAX));
    }
}
