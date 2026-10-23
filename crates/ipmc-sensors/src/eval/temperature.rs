//! temperature.rs - raw ADC counts to VDDA, die temperature and pin millivolts, and the
//! temperature threshold evaluation. Pure integer arithmetic, no hardware.
//!
//! Method (the proven method of the bench's earlier C project, in integers). The factory stores
//! three raw 12-bit readings taken with VDDA = 3300 mV (DS10086 Tables 73 and 76):
//!   TS_CAL1      die sensor at 30 C          (address 0x1FFF7A2C)
//!   TS_CAL2      die sensor at 110 C         (address 0x1FFF7A2E)
//!   VREFIN_CAL   VREFINT                     (address 0x1FFF7A2A)
//! 1. VDDA = 3300 mV x VREFIN_CAL / vref_counts (VREFINT is fixed, so fewer counts mean a
//!    higher VDDA).
//! 2. The die reading is rescaled to what it would have been at 3300 mV:
//!    ts_at_3v3 = ts_counts x VDDA / 3300.
//! 3. Temperature lies on the line through the two calibration points:
//!    T = 30 C + (ts_at_3v3 - TS_CAL1) x (110 C - 30 C) / (TS_CAL2 - TS_CAL1).
//!
//! The result is in tenths of a degree C; every division rounds to nearest, half away from zero.

use crate::hal::SourceError;
use ipmc_core::sensor::{TEMP_FLAG_LC, TEMP_FLAG_LNC, TEMP_FLAG_UC, TEMP_FLAG_UNC, TEMP_FLAG_UNR};

/// VDDA of the factory calibration, millivolts (DS10086 Tables 73 and 76).
const CAL_VDDA_MV: i64 = 3300;
/// First calibration temperature, tenths of a degree C (TS_CAL1 is taken at 30 C).
const CAL1_DC: i64 = 300;
/// Second calibration temperature, tenths of a degree C (TS_CAL2 is taken at 110 C).
const CAL2_DC: i64 = 1100;
/// ADC_FULL_SCALE -- full scale of the 12-bit ADC (RM0368 11.1: 12-bit resolution).
pub const ADC_FULL_SCALE: u32 = 4095;
/// Erased flash reads 0xFFFF; a zero is never a real calibration value either.
const ERASED: u16 = 0xFFFF;

/// Calibration -- the three factory raw readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub ts_cal1: u16,
    pub ts_cal2: u16,
    pub vrefin_cal: u16,
}

impl Calibration {
    /// validated -- the values when they are usable: none zero or erased, and TS_CAL2 above
    /// TS_CAL1 (the sensor reads higher when hotter). Otherwise `CalibrationInvalid`.
    pub fn validated(self) -> Result<Calibration, SourceError> {
        let present = |v: u16| v != 0 && v != ERASED;
        if present(self.ts_cal1)
            && present(self.ts_cal2)
            && present(self.vrefin_cal)
            && self.ts_cal2 > self.ts_cal1
        {
            Ok(self)
        } else {
            Err(SourceError::CalibrationInvalid)
        }
    }
}

/// div_round -- numerator / denominator rounded to nearest, half away from zero.
/// `Err(InvalidDivisor)` for a denominator that is not positive, `Err(Overflow)` when adding the
/// rounding offset leaves the i64 range.
fn div_round(numerator: i64, denominator: i64) -> Result<i64, SourceError> {
    if denominator <= 0 {
        return Err(SourceError::InvalidDivisor);
    }
    let half = denominator / 2;
    let adjusted = if numerator >= 0 {
        numerator.checked_add(half)
    } else {
        numerator.checked_sub(half)
    };
    adjusted
        .map(|n| n / denominator)
        .ok_or(SourceError::Overflow)
}

/// vdda_mv -- VDDA in millivolts from the raw VREFINT reading.
/// Example: VREFIN_CAL 1500, reading 1550 -> 3300 x 1500 / 1550 = 3193.5 -> 3194 mV.
/// `Err(ReferenceReadZero)` when the reading is zero, `Err(Overflow)` when the result does not
/// fit u32.
pub fn vdda_mv(vref_counts: u32, cal: &Calibration) -> Result<u32, SourceError> {
    if vref_counts == 0 {
        return Err(SourceError::ReferenceReadZero);
    }
    let scaled = div_round(
        CAL_VDDA_MV * i64::from(cal.vrefin_cal),
        i64::from(vref_counts),
    )?;
    u32::try_from(scaled).map_err(|_| SourceError::Overflow)
}

/// die_temperature_dc -- die temperature in tenths of a degree C from the raw sensor reading
/// and the VDDA of the same moment. The two steps 2 and 3 of the file-top method in one
/// division: dc = 300 + (ts x VDDA - TS_CAL1 x 3300) x 800 / ((TS_CAL2 - TS_CAL1) x 3300).
/// Example: cal 950 / 1250, ts 1000 at VDDA 3194 -> 300 + 59 000 x 800 / 990 000 = 300 + 48
/// = 348 (34.8 C). `Err(CalibrationInvalid)` when the span is not positive; `Err(Overflow)`
/// when a product, the rounding offset or the result leaves its range (real readings are at
/// most 4095 counts and a few volts, far below that).
pub fn die_temperature_dc(
    ts_counts: u32,
    vdda_mv: u32,
    cal: &Calibration,
) -> Result<i32, SourceError> {
    let span = i64::from(cal.ts_cal2) - i64::from(cal.ts_cal1);
    if span <= 0 {
        return Err(SourceError::CalibrationInvalid);
    }
    let above_cal1 = i64::from(ts_counts)
        .checked_mul(i64::from(vdda_mv))
        .and_then(|reading| reading.checked_sub(i64::from(cal.ts_cal1) * CAL_VDDA_MV))
        .and_then(|delta| delta.checked_mul(CAL2_DC - CAL1_DC))
        .ok_or(SourceError::Overflow)?;
    let offset = div_round(above_cal1, span * CAL_VDDA_MV)?;
    CAL1_DC
        .checked_add(offset)
        .and_then(|dc| i32::try_from(dc).ok())
        .ok_or(SourceError::Overflow)
}

/// pin_mv -- millivolts at an ADC pin from its raw reading and VDDA, rounded to nearest:
/// counts x VDDA / 4095. Example: 2048 counts at 3300 mV -> 1650.4 -> 1650 mV.
/// `Err(Overflow)` when the arithmetic or the result leaves its range.
pub fn pin_mv(counts: u32, vdda_mv: u32) -> Result<u32, SourceError> {
    let full = u64::from(ADC_FULL_SCALE);
    let twice = u64::from(counts)
        .checked_mul(u64::from(vdda_mv))
        .and_then(|product| product.checked_mul(2))
        .and_then(|doubled| doubled.checked_add(full))
        .ok_or(SourceError::Overflow)?;
    u32::try_from(twice / (full * 2)).map_err(|_| SourceError::Overflow)
}

/// Thresholds -- the temperature thresholds in tenths of a degree C. The board defines no
/// lower non-recoverable threshold, so bit 2 of the state is never set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    pub lower_noncritical_dc: i32,
    pub lower_critical_dc: i32,
    pub upper_noncritical_dc: i32,
    pub upper_critical_dc: i32,
    pub upper_nonrecoverable_dc: i32,
}

/// evaluate_flags -- the `TEMP_FLAG_*` bits for `dc`: lower flags when the reading is at or
/// below the threshold, upper flags when at or above it (Table 10.2.5-1). Flags accumulate: a
/// reading past a critical threshold is also past the non-critical one.
pub fn evaluate_flags(dc: i32, thresholds: &Thresholds) -> u8 {
    let mut flags = 0;
    if dc <= thresholds.lower_noncritical_dc {
        flags |= TEMP_FLAG_LNC;
    }
    if dc <= thresholds.lower_critical_dc {
        flags |= TEMP_FLAG_LC;
    }
    if dc >= thresholds.upper_noncritical_dc {
        flags |= TEMP_FLAG_UNC;
    }
    if dc >= thresholds.upper_critical_dc {
        flags |= TEMP_FLAG_UC;
    }
    if dc >= thresholds.upper_nonrecoverable_dc {
        flags |= TEMP_FLAG_UNR;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sensor with 3.75 counts per degree: 950 at 30 C, 1250 at 110 C, VREFINT 1500.
    const CAL: Calibration = Calibration {
        ts_cal1: 950,
        ts_cal2: 1250,
        vrefin_cal: 1500,
    };

    const LIMITS: Thresholds = Thresholds {
        lower_noncritical_dc: 0,
        lower_critical_dc: -100,
        upper_noncritical_dc: 700,
        upper_critical_dc: 850,
        upper_nonrecoverable_dc: 1000,
    };

    #[test]
    fn vdda_follows_vrefint() {
        // Same count as the calibration: exactly 3300 mV.
        assert_eq!(vdda_mv(1500, &CAL), Ok(3300));
        // 3300 x 1500 = 4 950 000; / 1550 = 3193.55 -> 3194.
        assert_eq!(vdda_mv(1550, &CAL), Ok(3194));
        // 4 950 000 / 1000 = 4950 exactly.
        assert_eq!(vdda_mv(1000, &CAL), Ok(4950));
        assert_eq!(vdda_mv(0, &CAL), Err(SourceError::ReferenceReadZero));
    }

    #[test]
    fn temperature_hits_the_calibration_points() {
        // ts = TS_CAL1 at 3300 mV: offset 0 -> 30.0 C.
        assert_eq!(die_temperature_dc(950, 3300, &CAL), Ok(300));
        // ts = TS_CAL2: (1250 - 950) x 800 / 300 = 800 -> 110.0 C.
        assert_eq!(die_temperature_dc(1250, 3300, &CAL), Ok(1100));
        // ts 1100: 150 x 800 / 300 = 400 -> 70.0 C.
        assert_eq!(die_temperature_dc(1100, 3300, &CAL), Ok(700));
        // ts 800: -150 x 800 / 300 = -400 -> -10.0 C.
        assert_eq!(die_temperature_dc(800, 3300, &CAL), Ok(-100));
    }

    #[test]
    fn temperature_corrects_for_vdda() {
        // ts 1000 at VDDA 3194: (1000 x 3194 - 950 x 3300) = 59 000;
        // 59 000 x 800 / (300 x 3300) = 47.68 -> 48; 300 + 48 = 348 (34.8 C).
        assert_eq!(die_temperature_dc(1000, 3194, &CAL), Ok(348));
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(div_round(3, 2), Ok(2));
        assert_eq!(div_round(-3, 2), Ok(-2));
        assert_eq!(div_round(1, 3), Ok(0));
        assert_eq!(div_round(-1, 3), Ok(0));
        assert_eq!(div_round(5, 3), Ok(2));
        assert_eq!(div_round(-5, 3), Ok(-2));
    }

    #[test]
    fn extreme_inputs_are_named_errors_not_overflows() {
        assert_eq!(
            die_temperature_dc(u32::MAX, u32::MAX, &CAL),
            Err(SourceError::Overflow)
        );
        assert_eq!(pin_mv(u32::MAX, u32::MAX), Err(SourceError::Overflow));
        // The result does not fit u32 although the products fit u64.
        assert_eq!(pin_mv(u32::MAX, 5000), Err(SourceError::Overflow));
        // Review input: every checked product succeeds, only the rounding offset leaves i64.
        let cal = Calibration {
            ts_cal1: 950,
            ts_cal2: 1250,
            vrefin_cal: 1500,
        };
        assert_eq!(
            die_temperature_dc(3_002_212, 3_840_240_146, &cal),
            Err(SourceError::Overflow)
        );
        assert_eq!(div_round(i64::MAX, 2), Err(SourceError::Overflow));
        assert_eq!(div_round(i64::MIN, 2), Err(SourceError::Overflow));
        assert_eq!(div_round(1, 0), Err(SourceError::InvalidDivisor));
        assert_eq!(div_round(1, -1), Err(SourceError::InvalidDivisor));
    }

    #[test]
    fn pin_millivolts_from_counts() {
        assert_eq!(pin_mv(0, 3300), Ok(0));
        // 2048 x 3300 / 4095 = 1650.4 -> 1650.
        assert_eq!(pin_mv(2048, 3300), Ok(1650));
        assert_eq!(pin_mv(ADC_FULL_SCALE, 3300), Ok(3300));
        // 4095 x 3194 / 4095 = 3194.
        assert_eq!(pin_mv(ADC_FULL_SCALE, 3194), Ok(3194));
    }

    #[test]
    fn calibration_must_be_present_and_ordered() {
        assert_eq!(CAL.validated(), Ok(CAL));
        for bad in [
            Calibration { ts_cal1: 0, ..CAL },
            Calibration {
                ts_cal2: 0xFFFF,
                ..CAL
            },
            Calibration {
                vrefin_cal: 0,
                ..CAL
            },
            Calibration {
                ts_cal2: 950,
                ..CAL
            },
            Calibration {
                ts_cal2: 900,
                ..CAL
            },
        ] {
            assert_eq!(bad.validated(), Err(SourceError::CalibrationInvalid));
        }
        let flat = Calibration {
            ts_cal2: 950,
            ..CAL
        };
        assert_eq!(
            die_temperature_dc(1000, 3300, &flat),
            Err(SourceError::CalibrationInvalid)
        );
    }

    #[test]
    fn thresholds_use_at_or_below_and_at_or_above() {
        assert_eq!(evaluate_flags(1, &LIMITS), 0);
        assert_eq!(evaluate_flags(0, &LIMITS), TEMP_FLAG_LNC);
        assert_eq!(evaluate_flags(-99, &LIMITS), TEMP_FLAG_LNC);
        assert_eq!(evaluate_flags(-100, &LIMITS), TEMP_FLAG_LNC | TEMP_FLAG_LC);
        assert_eq!(evaluate_flags(699, &LIMITS), 0);
        assert_eq!(evaluate_flags(700, &LIMITS), TEMP_FLAG_UNC);
        assert_eq!(evaluate_flags(850, &LIMITS), TEMP_FLAG_UNC | TEMP_FLAG_UC);
        assert_eq!(
            evaluate_flags(1000, &LIMITS),
            TEMP_FLAG_UNC | TEMP_FLAG_UC | TEMP_FLAG_UNR
        );
    }

    #[test]
    fn lower_critical_at_minus_ten_and_below() {
        // LIMITS: LNC 0.0 C, LC -10.0 C. -9.9 C is past LNC only; -10.0 C and colder set LC too.
        assert_eq!(evaluate_flags(-99, &LIMITS), TEMP_FLAG_LNC);
        assert_eq!(evaluate_flags(-100, &LIMITS), TEMP_FLAG_LNC | TEMP_FLAG_LC);
        assert_eq!(evaluate_flags(-101, &LIMITS), TEMP_FLAG_LNC | TEMP_FLAG_LC);
        assert_eq!(evaluate_flags(-400, &LIMITS), TEMP_FLAG_LNC | TEMP_FLAG_LC);
        assert_eq!(
            evaluate_flags(i32::MIN, &LIMITS),
            TEMP_FLAG_LNC | TEMP_FLAG_LC
        );
    }

    #[test]
    fn board_limits_from_the_defines_are_signed_degrees() {
        let limits = crate::board_sensors::TEMPERATURE_LIMITS;
        assert_eq!(limits.lower_critical_dc, -100);
        assert_eq!(evaluate_flags(-100, &limits), TEMP_FLAG_LNC | TEMP_FLAG_LC);
        assert_eq!(evaluate_flags(-99, &limits), TEMP_FLAG_LNC);
    }
}
