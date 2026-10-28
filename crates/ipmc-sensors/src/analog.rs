//! analog.rs - the analog IPMI sensors of the board: each rail of `board_sensors::RAILS` and the
//! die temperature as a threshold-based sensor with a Full Sensor Record (IPMI v2.0 Table 43-1), so
//! an SDR listing shows volts and degrees C. The VITA 46.11 mandatory sensors 00h..07h stay as they
//! are (`ipmc-core` sdr.rs); these come after them (owner decision 10-27-2026).
//!
//! The record of a sensor sits with its measurement definition: the conversion (M, R exponent,
//! unit, data format), the thresholds and the readings are computed here from the rail table
//! and the defs.rs rail and temperature defines, in millivolts and tenths of a degree C as the
//! sensor snapshot holds them. `ipmc-core` sees only the hook `analog::AnalogSensors`, which
//! `BoardAnalog` implements; the firmware and the emulator hand `BOARD_ANALOG` to the `Context`.
//!
//! Numbers: rail n is sensor `IPMC_SDR_ANALOG_FIRST + n`, the die temperature follows the last rail
//! (0Dh with five rails).
//!
//! Conversion, `y = M x 10^R` with B = 0 (Table 43-1 bytes 25..30):
//! - rail: R = -2, so one count is M x 10 mV; M = ceil(nominal x 1.7 / 255 counts / 10 mV), which puts
//!   the nominal at about 60 % of the 8-bit range (12 V payload: M 8, 80 mV per count, nominal 96h);
//!   unsigned.
//! - die temperature: M = 1, R = 0, one degree C per count, two's complement.
//!
//! Thresholds: a rail's from the same percent windows its status uses (`rail::edge_x100`,
//! `IPMC_RAIL_NONCRIT_PCT`, `IPMC_RAIL_CRIT_PCT`), rounded to a count; the die temperature's from
//! `board_sensors::TEMPERATURE_LIMITS` (`IPMC_TEMP_*_C`). A non-recoverable threshold the board does not
//! define (both for a rail, the lower one for the temperature) holds the end of the sensor range in
//! the record's own data format: the sensor maximum for an upper one, the sensor minimum for a lower
//! one, so a reader that publishes every threshold byte sees no limit inside the range.
//!
//! Reading: byte 2 the converted value, byte 4 the comparison bits of Table 35-15 (bit 0 at or
//! below lower non-critical, 1 lower critical, 2 lower non-recoverable, 3 at or above upper
//! non-critical, 4 upper critical, 5 upper non-recoverable), bits 7:6 reserved and returned as 1.
//! `comparison_flags` computes them from the returned reading byte against the record's own
//! threshold bytes, inclusively (at or above, at or below; unsigned for a rail, two's complement for
//! the temperature), as IPMI v2.0 section 36.5 and Table 35-15 do, so the bits always agree with
//! the thresholds a listing shows. The VITA discrete sensors 3 (FRU Voltage) and 4 (FRU Temperature)
//! keep their millivolt and tenth-of-a-degree evaluation (a window edge is in range), so they and
//! these bits can differ within half a count of an edge (12V payload: 12600 mV reads 9Eh, which is
//! UNC, while sensor 3 still reads in range).
//!
//! Capabilities: threshold access 11b (fixed, unreadable: there is no Get Sensor Thresholds), event
//! message control 11b (no events), no hysteresis; the sensors scan from power-up and send no
//! events (sensor initialization 01h).

use crate::board_sensors::{RAILS, RAIL_COUNT, TEMPERATURE_LIMITS, TENTHS};
use crate::eval::rail::{self, RailDef};
use ipmc_core::analog::AnalogSensors;
use ipmc_core::sdr::{
    finish_record, push_id_string, start_record, Record, ENTITY_ID_VPX_MODULE,
    ENTITY_INSTANCE_DEVICE_RELATIVE, RECORD_TYPE_FULL, SENSOR_INIT_SCANNING_ON,
};
use ipmc_core::sensor::{
    encode_threshold_reading, EncodeError, SensorSnapshot, TEMP_FLAG_LC, TEMP_FLAG_LNC,
    TEMP_FLAG_UC, TEMP_FLAG_UNC, TEMP_FLAG_UNR,
};

/// ANALOG_COUNT -- the analog sensors of the board: every rail and the die temperature
/// (`IPMC_SDR_ANALOG_SENSORS` false publishes none).
pub const ANALOG_COUNT: usize = RAIL_COUNT + 1;

/// First analog sensor number (defs.rs `IPMC_SDR_ANALOG_FIRST`).
const FIRST: u8 = ipmc_defs::IPMC_SDR_ANALOG_FIRST;

/// SENSOR_TYPE_VOLTAGE / SENSOR_TYPE_TEMPERATURE -- IPMI v2.0 Table 42-3 sensor type codes.
const SENSOR_TYPE_TEMPERATURE: u8 = 0x01;
const SENSOR_TYPE_VOLTAGE: u8 = 0x02;
/// EVENT_TYPE_THRESHOLD -- event/reading type 01h, threshold-based (Table 42-1).
const EVENT_TYPE_THRESHOLD: u8 = 0x01;
/// UNIT_VOLTS / UNIT_DEGREES_C -- IPMI v2.0 Table 43-15 sensor unit type codes.
const UNIT_DEGREES_C: u8 = 1;
const UNIT_VOLTS: u8 = 4;
/// UNITS1_UNSIGNED / UNITS1_TWOS_COMPLEMENT -- Table 43-1 byte 21: analog data format in bits 7:6
/// (00b unsigned, 10b 2's complement), no rate, modifier or percentage.
const UNITS1_UNSIGNED: u8 = 0b0000_0000;
const UNITS1_TWOS_COMPLEMENT: u8 = 0b1000_0000;
/// CAPABILITIES -- Table 43-1 byte 12: bit 6 auto re-arm, hysteresis 00b (none), threshold access
/// 11b (fixed, unreadable) in bits 3:2, event message control 11b (none) in bits 1:0.
const CAPABILITIES: u8 = (1 << 6) | (0b11 << 2) | 0b11;
/// CHARACTERISTICS_ALL -- Table 43-1 byte 31: nominal reading, normal maximum and normal minimum
/// are specified (bits 0..2).
const CHARACTERISTICS_ALL: u8 = 0b0000_0111;
/// R_EXP_SHIFT -- Table 43-1 byte 30: the R (result) exponent is bits 7:4, two's complement; the
/// B exponent in bits 3:0 is 0.
const R_EXP_SHIFT: u32 = 4;
/// R_EXP_MASK -- the 4 bits of an exponent.
const R_EXP_MASK: u8 = 0x0F;
/// COMPARISON_SHIFT -- Table 43-1 bytes 16:17 and 18:19: bits 14:12 of the assertion event mask
/// word hold the lower threshold reading mask (LNC, LC, LNR) and the same bits of the deassertion
/// word the upper one (UNC, UC, UNR): the comparisons Get Sensor Reading can return.
const COMPARISON_SHIFT: u32 = 12;
/// LOWER_FLAGS -- the three comparison bits of one side (LNC, LC, LNR or UNC, UC, UNR); the upper
/// ones start at bit `UPPER_FLAGS_SHIFT` of the `TEMP_FLAG_*` layout.
const LOWER_FLAGS: u8 = 0b111;
const UPPER_FLAGS_SHIFT: u32 = 3;
/// RAIL_SPAN_DIV -- 100 (percent) x 255 counts x 10 mV (one unit of M): `IPMC_SDR_RAIL_SPAN_PCT` over
/// this is M.
const RAIL_SPAN_DIV: u32 = 100 * 255 * RAIL_STEP_UNIT_MV;
/// RAIL_R_EXP -- one count is M x 10^R volts, R = -2 (M x 10 mV).
const RAIL_R_EXP: i8 = -2;
/// RAIL_STEP_UNIT_MV -- millivolts per unit of M for R = -2.
const RAIL_STEP_UNIT_MV: u32 = 10;
/// RAIL_RANGE_MAX / RAIL_RANGE_MIN -- the ends of an unsigned 8-bit rail count: the sensor maximum
/// and minimum of the record, and the values of the thresholds the board does not define.
const RAIL_RANGE_MAX: u8 = u8::MAX;
const RAIL_RANGE_MIN: u8 = u8::MIN;
/// TEMPERATURE_RANGE_MAX / TEMPERATURE_RANGE_MIN -- the ends of a two's complement 8-bit count
/// (+127 and -128 degrees C), likewise.
const TEMPERATURE_RANGE_MAX: u8 = i8::MAX as u8;
const TEMPERATURE_RANGE_MIN: u8 = i8::MIN as u8;
/// PERCENT -- percent per whole, for the window arithmetic.
const PERCENT: u64 = 100;

/// Spec -- one analog sensor's record: everything Table 43-1 carries that differs between the
/// sensors, all in raw counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// ID string.
    pub name: &'static str,
    /// Sensor type code.
    pub sensor_type: u8,
    /// Sensor units 1 (analog data format).
    pub units1: u8,
    /// Base unit code.
    pub base_unit: u8,
    /// M of `y = M x 10^R`.
    pub m: u8,
    /// R exponent.
    pub r_exp: i8,
    /// Nominal, normal maximum, normal minimum, sensor maximum, sensor minimum (raw counts).
    pub nominal: u8,
    pub normal_max: u8,
    pub normal_min: u8,
    pub sensor_max: u8,
    pub sensor_min: u8,
    /// Thresholds, raw counts: upper non-recoverable, upper critical, upper non-critical, lower
    /// non-recoverable, lower critical, lower non-critical (a threshold the board does not define holds the sensor maximum or minimum).
    pub unr: u8,
    pub uc: u8,
    pub unc: u8,
    pub lnr: u8,
    pub lc: u8,
    pub lnc: u8,
    /// Comparison bits Get Sensor Reading can return (`TEMP_FLAG_*` layout).
    pub comparisons: u8,
}

/// step_mv -- millivolts per count of a rail sensor with `m`.
const fn step_mv(m: u8) -> u32 {
    m as u32 * RAIL_STEP_UNIT_MV
}

/// rail_m -- M of a rail: ceil(nominal x `IPMC_SDR_RAIL_SPAN_PCT` / 100 / (255 counts x 10 mV)),
/// at least 1.
const fn rail_m(nominal_mv: u32) -> u8 {
    let m = (nominal_mv * ipmc_defs::IPMC_SDR_RAIL_SPAN_PCT).div_ceil(RAIL_SPAN_DIV);
    assert!(m >= 1 && m <= 255, "rail nominal outside the 8-bit range");
    m as u8
}

/// rail_count_at -- `mv` as a rounded count of a rail with `m`, clamped to the 8-bit range.
const fn rail_count_at(mv: u32, m: u8) -> u8 {
    let step = step_mv(m) as u64;
    let count = (mv as u64 + step / 2) / step;
    if count > 255 {
        255
    } else {
        count as u8
    }
}

/// edge_count -- the rounded count of a window edge (`rail::edge_x100`, hundredths of a millivolt).
const fn edge_count(edge_x100: u64, m: u8) -> u8 {
    let step_x100 = step_mv(m) as u64 * PERCENT;
    let count = (edge_x100 * 2 + step_x100) / (step_x100 * 2);
    assert!(count <= 255, "a rail threshold outside the 8-bit range");
    count as u8
}

/// rail_spec -- the record of rail `def`.
const fn rail_spec(def: &RailDef) -> Spec {
    let m = rail_m(def.nominal_mv);
    let unc = edge_count(rail::edge_x100(def.nominal_mv, def.noncrit_pct, true), m);
    let lnc = edge_count(rail::edge_x100(def.nominal_mv, def.noncrit_pct, false), m);
    Spec {
        name: def.name,
        sensor_type: SENSOR_TYPE_VOLTAGE,
        units1: UNITS1_UNSIGNED,
        base_unit: UNIT_VOLTS,
        m,
        r_exp: RAIL_R_EXP,
        nominal: rail_count_at(def.nominal_mv, m),
        normal_max: unc,
        normal_min: lnc,
        sensor_max: RAIL_RANGE_MAX,
        sensor_min: RAIL_RANGE_MIN,
        unr: RAIL_RANGE_MAX,
        uc: edge_count(rail::edge_x100(def.nominal_mv, def.crit_pct, true), m),
        unc,
        lnr: RAIL_RANGE_MIN,
        lc: edge_count(rail::edge_x100(def.nominal_mv, def.crit_pct, false), m),
        lnc,
        comparisons: TEMP_FLAG_LNC | TEMP_FLAG_LC | TEMP_FLAG_UNC | TEMP_FLAG_UC,
    }
}

/// degrees_byte -- a whole degree C as the 8-bit two's complement count of the temperature sensor.
const fn degrees_byte(degrees: i32) -> u8 {
    assert!(
        degrees >= -128 && degrees <= 127,
        "temperature outside the 8-bit range"
    );
    degrees as i8 as u8
}

/// temperature_spec -- the record of the die temperature sensor, from `TEMPERATURE_LIMITS`.
const fn temperature_spec() -> Spec {
    let limits = TEMPERATURE_LIMITS;
    let unc = degrees_byte(limits.upper_noncritical_dc / TENTHS);
    let lnc = degrees_byte(limits.lower_noncritical_dc / TENTHS);
    Spec {
        name: ipmc_defs::IPMC_SDR_DIE_TEMP_NAME,
        sensor_type: SENSOR_TYPE_TEMPERATURE,
        units1: UNITS1_TWOS_COMPLEMENT,
        base_unit: UNIT_DEGREES_C,
        m: 1,
        r_exp: 0,
        nominal: degrees_byte(ipmc_defs::IPMC_TEMP_NOMINAL_C),
        normal_max: unc,
        normal_min: lnc,
        sensor_max: TEMPERATURE_RANGE_MAX,
        sensor_min: TEMPERATURE_RANGE_MIN,
        unr: degrees_byte(limits.upper_nonrecoverable_dc / TENTHS),
        uc: degrees_byte(limits.upper_critical_dc / TENTHS),
        unc,
        lnr: TEMPERATURE_RANGE_MIN,
        lc: degrees_byte(limits.lower_critical_dc / TENTHS),
        lnc,
        comparisons: TEMP_FLAG_LNC | TEMP_FLAG_LC | TEMP_FLAG_UNC | TEMP_FLAG_UC | TEMP_FLAG_UNR,
    }
}

/// SPECS -- the analog sensors in number order: the rails in table order, then the die temperature.
pub const SPECS: [Spec; ANALOG_COUNT] = {
    let mut specs = [temperature_spec(); ANALOG_COUNT];
    let mut i = 0;
    while i < RAIL_COUNT {
        specs[i] = rail_spec(&RAILS[i]);
        i += 1;
    }
    specs
};

// The sensor numbers must stay below FFh and agree with the defs.rs count the core sizes its
// tables with.
const _: () = assert!(
    ANALOG_COUNT == ipmc_defs::IPMC_RAIL_COUNT + 1
        && FIRST as usize + ANALOG_COUNT <= 0xFF
        && ANALOG_COUNT >= ipmc_core::analog::ANALOG_COUNT_MAX,
    "the analog sensor count and numbers"
);

/// full_record -- the type 01h Full Sensor Record (Table 43-1) of `spec` as sensor `number`, record
/// ID `id`, owned by the controller at `owner_address` (8-bit IPMB address). Byte numbers are
/// 0-based from the first byte of the record, so Table 43-1 "byte 7" is index 6.
pub const fn full_record(id: u16, owner_address: u8, number: u8, spec: &Spec) -> Record {
    // Flag bits 0..=2 (LNC, LC, LNR) are the lower reading mask, bits 3..=5 (UNC, UC, UNR) the
    // upper one; each lands in bits 14:12 of its word.
    let lower = ((spec.comparisons & LOWER_FLAGS) as u16) << COMPARISON_SHIFT;
    let upper =
        (((spec.comparisons >> UPPER_FLAGS_SHIFT) & LOWER_FLAGS) as u16) << COMPARISON_SHIFT;
    let mut r = start_record(id, RECORD_TYPE_FULL);
    r.push(owner_address); // 5: sensor owner ID
    r.push(0); // 6: channel 0, sensor owner LUN 0
    r.push(number); // 7: sensor number
    r.push(ENTITY_ID_VPX_MODULE); // 8
    r.push(ENTITY_INSTANCE_DEVICE_RELATIVE); // 9
    r.push(SENSOR_INIT_SCANNING_ON); // 10: scans from power-up, no events
    r.push(CAPABILITIES); // 11
    r.push(spec.sensor_type); // 12
    r.push(EVENT_TYPE_THRESHOLD); // 13
    r.push_all(&lower.to_le_bytes()); // 14..=15: assertion events none, lower comparisons
    r.push_all(&upper.to_le_bytes()); // 16..=17: deassertion events none, upper comparisons
    r.push_all(&[0, 0]); // 18..=19: readable and settable threshold masks, none
    r.push(spec.units1); // 20: sensor units 1
    r.push(spec.base_unit); // 21
    r.push(0); // 22: modifier unit, none
    r.push(0); // 23: linearization, linear
    r.push_all(&[spec.m, 0]); // 24..=25: M (LS 8 bits), M MS bits and tolerance 0
    r.push_all(&[0, 0, 0]); // 26..=28: B 0, accuracy 0, direction unspecified
    r.push((spec.r_exp as u8 & R_EXP_MASK) << R_EXP_SHIFT); // 29: R exponent, B exponent 0
    r.push(CHARACTERISTICS_ALL); // 30
    r.push_all(&[
        spec.nominal,
        spec.normal_max,
        spec.normal_min,
        spec.sensor_max,
        spec.sensor_min,
    ]); // 31..=35
    r.push_all(&[spec.unr, spec.uc, spec.unc, spec.lnr, spec.lc, spec.lnc]); // 36..=41
    r.push_all(&[0, 0]); // 42..=43: positive and negative-going hysteresis
    r.push_all(&[0, 0]); // 44..=45: reserved
    r.push(0); // 46: OEM
    push_id_string(&mut r, spec.name); // 47: type/length, then the string
    finish_record(r)
}

// Every record builds at compile time: an ID string outside 2..=16 bytes or a threshold outside
// the 8-bit range stops the build here.
const _: () = {
    let mut i = 0;
    while i < ANALOG_COUNT {
        let _ = full_record(0, 0x82, FIRST + i as u8, &SPECS[i]);
        i += 1;
    }
};

/// signed_value -- `raw` as a number in the data format of `spec`: two's complement for the
/// temperature, unsigned for a rail.
const fn signed_value(spec: &Spec, raw: u8) -> i16 {
    if spec.units1 == UNITS1_TWOS_COMPLEMENT {
        raw as i8 as i16
    } else {
        raw as i16
    }
}

/// comparison_flags -- the comparison bits of Get Sensor Reading for the reading byte `raw`: each
/// bit set when `raw` is at or below a lower threshold byte or at or above an upper one of `spec`,
/// the thresholds the record holds, limited to the comparisons the record's reading mask offers
/// (`Spec::comparisons`).
pub const fn comparison_flags(spec: &Spec, raw: u8) -> u8 {
    let x = signed_value(spec, raw);
    let mut flags = 0;
    if x <= signed_value(spec, spec.lnc) {
        flags |= TEMP_FLAG_LNC;
    }
    if x <= signed_value(spec, spec.lc) {
        flags |= TEMP_FLAG_LC;
    }
    if x >= signed_value(spec, spec.unc) {
        flags |= TEMP_FLAG_UNC;
    }
    if x >= signed_value(spec, spec.uc) {
        flags |= TEMP_FLAG_UC;
    }
    if x >= signed_value(spec, spec.unr) {
        flags |= TEMP_FLAG_UNR;
    }
    flags & spec.comparisons
}

/// rail_value -- the reading byte of a rail in `snapshot`: its millivolts as a count of `spec`;
/// None when the rail is unavailable or the snapshot has no such rail.
fn rail_value(index: usize, snapshot: &SensorSnapshot) -> Option<u8> {
    let reading = snapshot.valid_rails().get(index)?;
    if reading.status == ipmc_core::sensor::RailStatus::Unavailable {
        return None;
    }
    Some(rail_count_at(reading.mv, SPECS[index].m))
}

/// temperature_value -- the reading byte of the die temperature: whole degrees C, rounded half away
/// from zero (in 64 bits, so an extreme simulated value cannot overflow) and clamped to the 8-bit
/// signed range; None when unavailable.
fn temperature_value(snapshot: &SensorSnapshot) -> Option<u8> {
    let t = &snapshot.temperature;
    if !t.available {
        return None;
    }
    let (dc, tenths) = (i64::from(t.dc), i64::from(TENTHS));
    let half = tenths / 2;
    let rounded = if dc >= 0 { dc + half } else { dc - half } / tenths;
    Some(rounded.clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8 as u8)
}

/// reading -- the (value, comparison bits) of analog sensor `index` in `snapshot`.
pub fn reading(index: usize, snapshot: &SensorSnapshot) -> Option<(u8, u8)> {
    let value = if index < RAIL_COUNT {
        rail_value(index, snapshot)
    } else {
        temperature_value(snapshot)
    }?;
    Some((value, comparison_flags(&SPECS[index], value)))
}

/// BoardAnalog -- the board's analog sensors behind the `ipmc-core` hook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardAnalog;

/// BOARD_ANALOG -- the value the firmware and the emulator hand to `Context::with_analog`.
pub static BOARD_ANALOG: BoardAnalog = BoardAnalog;

impl AnalogSensors for BoardAnalog {
    fn count(&self) -> usize {
        if ipmc_defs::IPMC_SDR_ANALOG_SENSORS {
            ANALOG_COUNT
        } else {
            0
        }
    }

    fn record(&self, index: usize, owner_address: u8, id: u16) -> Option<Record> {
        if index >= self.count() {
            return None;
        }
        let spec = SPECS.get(index)?;
        Some(full_record(id, owner_address, FIRST + index as u8, spec))
    }

    fn read(
        &self,
        index: usize,
        snapshot: &SensorSnapshot,
        out: &mut [u8],
    ) -> Result<usize, EncodeError> {
        if index >= self.count() {
            return Err(EncodeError::UnsupportedSensor);
        }
        let (value, flags) = match reading(index, snapshot) {
            Some((value, flags)) => (Some(value), flags),
            None => (None, 0),
        };
        encode_threshold_reading(value, flags, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_sensors::sim_analog;
    use ipmc_core::sensor::{RailReading, RailStatus, TemperatureReading};

    const OWNER: u8 = 0x82;

    /// The 12V payload rail (sensor 09h, record ID 9 in a Tier-3 build), hand-written from
    /// Table 43-1: ID 0009h, 51h, type 01h, length 43 + 11 = 36h; owner 82h; LUN 00h; sensor
    /// 09h; entity A0h, 60h; init 01h; capabilities 4Fh; type 02h; event type 01h; lower
    /// comparisons LNC, LC (bits 12, 13: 3000h), upper UNC, UC (3000h); threshold masks 00h 00h;
    /// units 1 00h (unsigned); base unit 04h (Volts); modifier 00h; linear 00h; M = 8 (08h 00h);
    /// B = 0; accuracy 0; direction 0; R exponent -2 (E0h); characteristics 07h; nominal 12000 mV /
    /// 80 mV = 150 (96h); normal max 12600 / 80 = 157.5 -> 158 (9Eh); normal min 11400 / 80 = 142.5
    /// -> 143 (8Fh); sensor max FFh, min 00h; UNR FFh (not defined: the sensor maximum); UC 13200 / 80 = 165 (A5h); UNC 9Eh; LNR 00h (the sensor minimum);
    /// LC 10800 / 80 = 135 (87h); LNC 8Fh; hysteresis 00h 00h; reserved 00h 00h; OEM 00h; type/length
    /// CBh; "12V payload".
    const RAIL_12V: [u8; 59] = [
        0x09, 0x00, 0x51, 0x01, 0x36, 0x82, 0x00, 0x09, 0xA0, 0x60, 0x01, 0x4F, 0x02, 0x01, 0x00,
        0x30, 0x00, 0x30, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0xE0,
        0x07, 0x96, 0x9E, 0x8F, 0xFF, 0x00, 0xFF, 0xA5, 0x9E, 0x00, 0x87, 0x8F, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xCB, b'1', b'2', b'V', b' ', b'p', b'a', b'y', b'l', b'o', b'a', b'd',
    ];

    /// The die temperature (sensor 0Dh, record ID 13 in a Tier-3 build): ID 000Dh, 51h, 01h, length
    /// 43 + 13 = 38h ("IPMC Die Temp"); sensor 0Dh; type 01h; event type 01h; lower comparisons
    /// 3000h, upper UNC, UC, UNR (7000h); units 1 80h (2's complement); base unit 01h (degrees C);
    /// M = 1; R exponent 0; characteristics 07h; nominal 25 (19h); normal max 70 (46h); normal min 0;
    /// sensor max 7Fh, min 80h; UNR 100 (64h); UC 85 (55h); UNC 46h; LNR 80h (not defined: the sensor minimum); LC -10 (F6h); LNC 00h;
    /// type/length CDh.
    const TEMPERATURE: [u8; 61] = [
        0x0D, 0x00, 0x51, 0x01, 0x38, 0x82, 0x00, 0x0D, 0xA0, 0x60, 0x01, 0x4F, 0x01, 0x01, 0x00,
        0x30, 0x00, 0x70, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x07, 0x19, 0x46, 0x00, 0x7F, 0x80, 0x64, 0x55, 0x46, 0x80, 0xF6, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xCD, b'I', b'P', b'M', b'C', b' ', b'D', b'i', b'e', b' ', b'T', b'e', b'm',
        b'p',
    ];

    fn record(index: usize, id: u16) -> Vec<u8> {
        BOARD_ANALOG
            .record(index, OWNER, id)
            .unwrap()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn the_rail_and_temperature_records_are_byte_exact() {
        assert_eq!(record(1, 9), RAIL_12V);
        assert_eq!(record(RAIL_COUNT, 13), TEMPERATURE);
    }

    #[test]
    fn the_conversions_follow_the_nominals() {
        // (rail, name, M, nominal count, UNC, UC, LNC, LC): 80, 40 and 30 mV per count.
        let expect: [(usize, u8, u8, u8, u8, u8, u8); 5] = [
            (0, 3, 110, 116, 121, 105, 99),
            (1, 8, 150, 158, 165, 143, 135),
            (2, 4, 125, 131, 138, 119, 113),
            (3, 3, 110, 116, 121, 105, 99),
            (4, 3, 110, 116, 121, 105, 99),
        ];
        for (rail, m, nominal, unc, uc, lnc, lc) in expect {
            let s = SPECS[rail];
            assert_eq!(
                (s.m, s.nominal, s.unc, s.uc, s.lnc, s.lc),
                (m, nominal, unc, uc, lnc, lc),
                "rail {rail}"
            );
            assert_eq!((s.normal_max, s.normal_min), (unc, lnc));
            assert_eq!((s.unr, s.lnr, s.sensor_max, s.sensor_min), (255, 0, 255, 0));
        }
    }

    /// A snapshot with rail `index` at `mv` (status by the rail evaluation) and the others
    /// nominal, the die temperature at `dc` with `flags`.
    fn snapshot_with(index: usize, mv: u32, dc: i32, flags: u8) -> SensorSnapshot {
        let mut s = SensorSnapshot::EMPTY;
        for (i, def) in RAILS.iter().enumerate() {
            let value = if i == index { mv } else { def.nominal_mv };
            s.rails[i] = RailReading {
                mv: value,
                status: rail::evaluate(def, value),
            };
            s.rail_count += 1;
        }
        s.temperature = TemperatureReading {
            available: true,
            dc,
            flags,
        };
        s
    }

    fn read(index: usize, s: &SensorSnapshot) -> [u8; 3] {
        let mut out = [0xEE; 3];
        assert_eq!(BOARD_ANALOG.read(index, s, &mut out), Ok(3));
        out
    }

    /// Byte 4 reserved bits 7:6, returned as 1 (Table 35-15).
    const RESERVED: u8 = 0xC0;
    /// Status byte: scanning enabled.
    const SCANNING: u8 = 0x40;

    #[test]
    fn rails_read_in_counts_with_inclusive_comparison_bits() {
        // 12V payload (index 1), 80 mV per count, bytes UNC 9Eh, UC A5h, LNC 8Fh, LC 87h. The bits
        // compare the returned byte with those bytes, at or above and at or below:
        // (mV, byte 2, comparison bits)
        let rows: [(u32, u8, u8); 14] = [
            (12_000, 0x96, 0),
            (12_580, 0x9D, 0),                            // one count below UNC
            (12_600, 0x9E, TEMP_FLAG_UNC),                // exactly UNC (157.5 rounds up)
            (12_700, 0x9F, TEMP_FLAG_UNC),                // one count above
            (13_159, 0xA4, TEMP_FLAG_UNC),                // one count below UC
            (13_160, 0xA5, TEMP_FLAG_UNC | TEMP_FLAG_UC), // exactly UC
            (13_200, 0xA5, TEMP_FLAG_UNC | TEMP_FLAG_UC), // the old edge: UC set
            (11_480, 0x90, 0),                            // one count above LNC
            (11_440, 0x8F, TEMP_FLAG_LNC),                // exactly LNC
            (11_400, 0x8F, TEMP_FLAG_LNC),
            (10_880, 0x88, TEMP_FLAG_LNC), // one count above LC
            (10_800, 0x87, TEMP_FLAG_LNC | TEMP_FLAG_LC), // exactly LC
            (10_799, 0x87, TEMP_FLAG_LNC | TEMP_FLAG_LC),
            (0, 0x00, TEMP_FLAG_LNC | TEMP_FLAG_LC),
        ];
        for (mv, value, flags) in rows {
            let s = snapshot_with(1, mv, 250, 0);
            assert_eq!(read(1, &s), [value, SCANNING, flags | RESERVED], "{mv} mV");
        }
        // The count clamps at the top: no UNR bit, the rail has none.
        assert_eq!(
            read(1, &snapshot_with(1, 40_000, 250, 0)),
            [0xFF, SCANNING, TEMP_FLAG_UNC | TEMP_FLAG_UC | RESERVED]
        );
        // FRU Voltage keeps its millivolt window: at 12600 mV it is in range although the byte is
        // UNC, and one millivolt past it is not.
        assert_eq!(
            snapshot_with(1, 12_600, 0, 0).rails[1].status,
            RailStatus::InRange
        );
        assert_eq!(
            snapshot_with(1, 12_601, 0, 0).rails[1].status,
            RailStatus::NonCritical
        );
    }

    #[test]
    fn the_temperature_reads_in_degrees_with_inclusive_comparison_bits() {
        // Bytes: UNR 100 (64h), UC 85, UNC 70, LC -10 (F6h), LNC 0, two's complement.
        // (tenths of a degree, byte 2, comparison bits)
        let upper = TEMP_FLAG_UNC | TEMP_FLAG_UC;
        let rows: [(i32, u8, u8); 17] = [
            (250, 25, 0),
            (694, 69, 0),             // rounds down, below UNC
            (699, 70, TEMP_FLAG_UNC), // 69.9 C is the byte 70: UNC
            (700, 70, TEMP_FLAG_UNC),
            (844, 84, TEMP_FLAG_UNC),
            (845, 85, upper), // 84.5 C rounds up to UC
            (999, 100, upper | TEMP_FLAG_UNR),
            (1300, 127, upper | TEMP_FLAG_UNR),
            (5, 1, 0),             // 0.5 C is the byte 1: above LNC
            (4, 0, TEMP_FLAG_LNC), // 0.4 C is the byte 0: LNC
            (0, 0, TEMP_FLAG_LNC),
            (-5, 0xFF, TEMP_FLAG_LNC),  // -0.5 C is the byte -1
            (-94, 0xF7, TEMP_FLAG_LNC), // -9.4 C is -9: above LC
            (-95, 0xF6, TEMP_FLAG_LNC | TEMP_FLAG_LC), // -9.5 C is -10: exactly LC
            (-100, 0xF6, TEMP_FLAG_LNC | TEMP_FLAG_LC),
            (-105, 0xF5, TEMP_FLAG_LNC | TEMP_FLAG_LC),
            (-2000, 0x80, TEMP_FLAG_LNC | TEMP_FLAG_LC), // clamps at -128 C
        ];
        for (dc, value, flags) in rows {
            let s = snapshot_with(0, 3300, dc, 0);
            assert_eq!(
                read(RAIL_COUNT, &s),
                [value, SCANNING, flags | RESERVED],
                "{dc} dC"
            );
        }
    }

    #[test]
    fn extreme_simulator_values_clamp_without_overflow() {
        // u32::MAX mV and the i32 extremes: no panic in a debug build, the byte clamps.
        assert_eq!(read(1, &snapshot_with(1, u32::MAX, 0, 0))[0], 0xFF);
        assert_eq!(read(0, &snapshot_with(0, u32::MAX, 0, 0))[0], 0xFF);
        assert_eq!(
            read(RAIL_COUNT, &snapshot_with(0, 3300, i32::MAX, 0))[0],
            0x7F
        );
        assert_eq!(
            read(RAIL_COUNT, &snapshot_with(0, 3300, i32::MIN, 0))[0],
            0x80
        );
        assert_eq!(rail_count_at(u32::MAX, 3), 255);
    }

    #[test]
    fn an_unavailable_source_reads_as_unavailable() {
        // Unavailable bit (bit 5) and scanning (bit 6) in the status byte, value 0, no
        // comparison bits, the reserved bits 7:6 still 1.
        let mut s = snapshot_with(1, 12_000, 250, 0);
        s.rails[1].status = RailStatus::Unavailable;
        s.temperature.available = false;
        assert_eq!(read(1, &s), [0, 0x60, RESERVED]);
        assert_eq!(read(RAIL_COUNT, &s), [0, 0x60, RESERVED]);
        // A snapshot before the first sample has no rails.
        assert_eq!(read(0, &SensorSnapshot::EMPTY), [0, 0x60, RESERVED]);
    }

    #[test]
    fn the_hook_stops_at_the_last_sensor() {
        assert_eq!(BOARD_ANALOG.count(), RAIL_COUNT + 1);
        assert!(BOARD_ANALOG.record(RAIL_COUNT + 1, OWNER, 14).is_none());
        let mut out = [0u8; 3];
        assert_eq!(
            BOARD_ANALOG.read(RAIL_COUNT + 1, &SensorSnapshot::EMPTY, &mut out),
            Err(EncodeError::UnsupportedSensor)
        );
        let mut short = [0u8; 2];
        assert_eq!(
            BOARD_ANALOG.read(0, &SensorSnapshot::EMPTY, &mut short),
            Err(EncodeError::BufferTooShort)
        );
    }

    #[test]
    fn the_simulated_nominals_decode_as_volts_and_degrees() {
        // The bench build starts the external rails at nominal and the die at IPMC_SIM_TEMP_C. The
        // value a Chassis Manager shows is M x count x 10^R: (rail, volts x 100).
        let mut analog = sim_analog(&crate::drivers::sim::SIM_RAIL_WIRING);
        let eval = crate::eval::faults::evaluate(
            &crate::board_sensors::INPUTS,
            &mut crate::drivers::sim::SimInputs::new(),
        );
        let s = crate::board_sensors::build_snapshot(
            &mut analog,
            &crate::drivers::sim::SIM_RAIL_WIRING,
            &eval,
            ipmc_core::sensor::PowerState::Off,
            false,
            true,
        );
        for (index, volts_x100) in [(0, 330), (1, 1200), (2, 500), (3, 330), (4, 330)] {
            let (count, flags) = reading(index, &s).unwrap();
            assert_eq!(flags, 0, "rail {index}");
            let shown = u32::from(count) * u32::from(SPECS[index].m);
            // Rounded to a count: within half a count of the nominal volts.
            assert!(
                shown.abs_diff(volts_x100) <= u32::from(SPECS[index].m) / 2 + 1,
                "rail {index}: {shown}"
            );
        }
        let (degrees, flags) = reading(RAIL_COUNT, &s).unwrap();
        assert_eq!((degrees, flags), (ipmc_defs::IPMC_SIM_TEMP_C as u8, 0));
    }
}
