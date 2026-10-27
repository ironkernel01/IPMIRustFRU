//! sdr.rs - the Device SDR repository of this IPMC and its reservation state.
//!
//! Eight records (nine at Tier-3), built once from the IPMC_SDR_*_NAME defines and the IPMB
//! address, plus the Full Sensor Records of the analog sensors a build hands in
//! (`analog::AnalogSensors`; the firmware and the emulator pass the board's rails and die
//! temperature, ipmc-sensors `analog.rs`): one type 02h compact sensor record for each mandatory sensor 0..6 (VITA 46.11 Table
//! 10.2-1; plus sensor 7, Payload Mode, at Tier-3, `tier::payload_mode_sensor`), then the type 01h
//! analog records, then one type 12h management controller device locator record (VITA 46.11 Rule
//! 6.2-7). Record IDs run 0..=7 in that order (0..=8 at Tier-3) without analog sensors, so the
//! locator record is the last one and its ID is the sensor count; with n analog sensors the IDs
//! run on to the locator at the new sensor count.
//!
//! Layouts are from IPMI v2.0 rev 1.1 (E7 markup): Table 43-2 (type 02h), Table 43-8 (type 12h),
//! section 43.15 (type/length byte); byte numbers below are 0-based from the first byte of the
//! record, so Table 43-2 "byte 6" is index 5. The captured SBC IPMC (vectors/sbc_ab.vec) is the
//! second oracle; every place this file differs from its records is listed here:
//!
//! - Entity instance 60h (device-relative, Rule 6.4.3-5); the capture has 00h.
//! - Sensor initialization and capabilities by Tier (`tier.rs`). Tier-1: 01h (scanning on at
//!   power-up, events off) and 43h (auto re-arm, no hysteresis, no thresholds, no events from the
//!   sensor: a Tier-1 IPMC generates none, Rule 7-4). Tier-2/3: 03h (events on at power-up) and
//!   capabilities 42h (Tier-2, event message control 10b: global disable only) or 40h (Tier-3,
//!   00b: per-state enable, Set Sensor Event Enable). The capture has 00h and 68h, whose
//!   threshold-access field 10b is reserved.
//! - Assertion and deassertion event masks LS byte first (Table 43-2: "LS byte first"): 0000h at
//!   Tier-1 (no events); at Tier-2/3 the offsets the IPMC sends events for (`SENSORS`). The
//!   reading mask is also LS byte first; the capture writes the masks MS byte first and the
//!   reading mask 0000h.
//! - Units 1 is C0h (Table 43-2: bits 7:6 "write as 11b"), share count 1; the capture has 00h and 0.
//! - The record length byte counts exactly the bytes that follow (27 + N for a compact record,
//!   11 + N for the locator); the capture's compact records carry two extra 00h bytes after the
//!   ID string, its locator record one, and the length byte counts them.
//! - The type 12h record: both follow the Table 43-8 layout (three reserved bytes before the
//!   entity ID); ours has global initialization 01b (events off; 00b at Tier-2/3, event message
//!   generation enabled), device capabilities 0Bh (Get Device ID's additional support plus the
//!   Device SDR bit: bit 5 IPMB Event Generator from Tier-2, bit 2 SEL Device at Tier-3) and
//!   entity instance 60h where the capture has 00h, 3Dh and 00h.

use crate::analog::{AnalogSensors, ANALOG_COUNT_MAX, ANALOG_FIRST};
use crate::fru_state::State;
use crate::identity::{additional_support, DEVICE_IDENTITY};
use crate::sensor::{
    EVENT_TYPE_LIMIT, EVENT_TYPE_PREDICTIVE_FAILURE, EVENT_TYPE_SENSOR_SPECIFIC, EVENT_TYPE_STATE,
    SENSOR_FRU_HEALTH, SENSOR_FRU_STATE, SENSOR_FRU_TEMPERATURE, SENSOR_FRU_VOLTAGE,
    SENSOR_PAYLOAD_MODE, SENSOR_PAYLOAD_TEST_RESULTS, SENSOR_PAYLOAD_TEST_STATUS,
    SENSOR_SYSTEM_IPMB_LINK, SENSOR_TYPE_FRU_HEALTH, SENSOR_TYPE_FRU_STATE,
    SENSOR_TYPE_FRU_TEMPERATURE, SENSOR_TYPE_FRU_VOLTAGE, SENSOR_TYPE_PAYLOAD_MODE,
    SENSOR_TYPE_PAYLOAD_TEST_RESULTS, SENSOR_TYPE_PAYLOAD_TEST_STATUS,
    SENSOR_TYPE_SYSTEM_IPMB_LINK, TEMP_FLAG_LC, TEMP_FLAG_LNC, TEMP_FLAG_UC, TEMP_FLAG_UNC,
    TEMP_FLAG_UNR,
};
use crate::tier;
use ipmc_wire::Builder;

/// MANDATORY_COUNT_MAX -- the most mandatory sensors on FRU 0: sensor numbers 0..=7 (Table
/// 10.2-1; sensor 7, Payload Mode, is Tier-3 only, `mandatory_count`).
pub const MANDATORY_COUNT_MAX: usize = 8;
/// SENSOR_COUNT_MAX -- the most sensors on FRU 0: the mandatory ones and the build's analog
/// sensors (`ANALOG_COUNT_MAX`); the analog numbers start at `ANALOG_FIRST`, which equals
/// `MANDATORY_COUNT_MAX`, so a sensor number indexes a table of this size.
pub const SENSOR_COUNT_MAX: usize = MANDATORY_COUNT_MAX + ANALOG_COUNT_MAX;
/// RECORD_COUNT_MAX -- one record per sensor and the locator record, at the most.
pub const RECORD_COUNT_MAX: usize = SENSOR_COUNT_MAX + 1;
const _: () = assert!(
    ANALOG_FIRST as usize == MANDATORY_COUNT_MAX,
    "the analog sensor numbers follow the mandatory ones"
);

/// mandatory_count -- the VITA 46.11 mandatory sensors a Tier-`tier` IPMC has: 7, or 8 with the
/// Payload Mode sensor at Tier-3 (the analog sensors come after them).
pub const fn mandatory_count(tier: u32) -> usize {
    if tier::payload_mode_sensor(tier) {
        MANDATORY_COUNT_MAX
    } else {
        MANDATORY_COUNT_MAX - 1
    }
}
/// NO_NEXT_RECORD -- the "no next record" value of Get Device SDR (IPMI v2.0 section 33.12).
pub const NO_NEXT_RECORD: u16 = 0xFFFF;

/// SDR_VERSION -- 51h: sensor model version 1.5, BCD with the LS digit in bits 7:4 (Table 43-2).
pub const SDR_VERSION: u8 = 0x51;
/// RECORD_TYPE_COMPACT -- compact sensor record (Table 43-2).
const RECORD_TYPE_COMPACT: u8 = 0x02;
/// RECORD_TYPE_FULL -- full sensor record (Table 43-1).
pub const RECORD_TYPE_FULL: u8 = 0x01;
/// RECORD_TYPE_MC_LOCATOR -- management controller device locator record (Table 43-8).
const RECORD_TYPE_MC_LOCATOR: u8 = 0x12;
/// HEADER_LEN -- record ID (2 bytes), version, type, length (Table 43-2 "RECORD HEADER").
pub const HEADER_LEN: usize = 5;
/// ID_LEN_MAX -- the ID string is 16 bytes at most (Tables 43-1, 43-2 and 43-8).
pub const ID_LEN_MAX: usize = 16;
/// COMPACT_FIXED_LEN -- bytes of a compact record before the ID string (Table 43-2: bytes 1..=32).
const COMPACT_FIXED_LEN: usize = 32;
/// FULL_FIXED_LEN -- bytes of a full record before the ID string (Table 43-1: bytes 1..=48).
pub const FULL_FIXED_LEN: usize = 48;
/// RECORD_MAX -- the longest record: a full record with a 16-byte ID string.
pub const RECORD_MAX: usize = FULL_FIXED_LEN + ID_LEN_MAX;

/// ENTITY_ID_VPX_MODULE -- Entity ID A0h, Front Loading VPX Plug-In Module (VITA Rule 6.4.3-5).
pub const ENTITY_ID_VPX_MODULE: u8 = 0xA0;
/// ENTITY_INSTANCE_DEVICE_RELATIVE -- Entity Instance 60h, the first device-relative instance
/// (VITA Rule 6.4.3-5; section 6.4.2: IPMCs refer to themselves with device-relative instances).
pub const ENTITY_INSTANCE_DEVICE_RELATIVE: u8 = 0x60;
/// SENSOR_INIT_SCANNING_ON -- Sensor Initialization byte, bit 0: sensor scanning enabled at
/// power-up (Table 43-2); the other bits are 0 because no init agent configures the sensors.
pub const SENSOR_INIT_SCANNING_ON: u8 = 1 << 0;
/// SENSOR_INIT_EVENTS_ON -- Sensor Initialization byte, bit 1: event generation enabled at
/// power-up (Table 43-2); set when the IPMC generates events (Tier-2/3).
const SENSOR_INIT_EVENTS_ON: u8 = 1 << 1;
/// SENSOR_CAPS_AUTO_REARM -- Sensor Capabilities byte: bit 6 auto re-arm, hysteresis 00b,
/// thresholds 00b; bits 1:0 (event message control) are added per Tier (Table 43-2).
const SENSOR_CAPS_AUTO_REARM: u8 = 1 << 6;
/// EVENT_CONTROL_NONE -- event message control 11b: no events from the sensor (Tier-1).
const EVENT_CONTROL_NONE: u8 = 0b11;
/// EVENT_CONTROL_GLOBAL -- event message control 10b: the global disable only (Tier-2).
const EVENT_CONTROL_GLOBAL: u8 = 0b10;
/// EVENT_CONTROL_PER_STATE -- event message control 00b: each state can be enabled (Tier-3).
const EVENT_CONTROL_PER_STATE: u8 = 0b00;
/// UNITS1_RESERVED -- Sensor Units 1: bits 7:6 "reserved, write as 11b", no rate, modifier or
/// percentage (Table 43-2).
const UNITS1_RESERVED: u8 = 0b1100_0000;
/// SHARE_COUNT_ONE -- Table 43-2 sensor direction/instance modifier byte: share count 1, one
/// sensor per record, no direction, numeric modifier.
const SHARE_COUNT_ONE: u8 = 1;
/// LOCATOR_INIT_EVENTS_OFF -- Table 43-8 byte 8: static controller bit 5 clear (dynamic), global
/// initialization bits 1:0 = 01b: event message generation disabled (a Tier-1 IPMC sends none).
const LOCATOR_INIT_EVENTS_OFF: u8 = 0b01;
/// LOCATOR_INIT_EVENTS_ON -- the same byte with bits 1:0 = 00b: event message generation
/// enabled (Tier-2/3).
const LOCATOR_INIT_EVENTS_ON: u8 = 0b00;
/// DEVICE_SUPPORT_SDR_BIT -- Table 43-8 byte 9 bit 1: "controller accepts Device SDR commands";
/// the same fact as the Device Revision "provides Device SDRs" bit of Get Device ID.
const DEVICE_SUPPORT_SDR_BIT: u8 = 1 << 1;
/// REVISION_SDRS_SHIFT -- Get Device ID byte 3 bit 7 is "provides Device SDRs" (Table 20-2).
const REVISION_SDRS_SHIFT: u32 = 7;

/// SensorDef -- the fixed facts of one mandatory sensor (VITA Table 10.2-1 and the sections
/// 10.2.1..10.2.8 reading layouts).
struct SensorDef {
    number: u8,
    sensor_type: u8,
    event_type: u8,
    /// Reading mask: bit n set when state n can be returned in Get Sensor Reading byte 4.
    reading_mask: u16,
    /// Assertion event mask: bit n set when the IPMC sends an event for entering state n.
    assertion_mask: u16,
    /// Deassertion event mask: bit n set when the IPMC sends an event for leaving state n.
    deassertion_mask: u16,
    /// (reading, assertion, deassertion) bits added to the masks at Tier-3 only: M5 of the FRU
    /// State sensor (`tier::activation_control`), the 400 kHz states of the IPMB Link sensor
    /// (`tier::fast_mode`).
    tier3_extra: (u16, u16, u16),
    name: &'static str,
}

/// Reading masks: bit n set when state n can be returned in Get Sensor Reading byte 4 (Table 43-2
/// reading mask), written to the record LS byte first.
///
/// - FRU State (Table 10.2.1-1): the Current State Mask bits of the reachable states M0, M1, M4
///   and M6 (0053h), plus M5 at Tier-3 (0073h), built from `fru_state::State`; M7 is never
///   reported.
/// - IPMB Link (Table 10.2.2-1 byte 4): the link-state values in bits 3:0 (000Fh), plus the
///   400 kHz states of bits 5:4 at Tier-3 (003Fh).
/// - Temperature (Table 10.2.5-1 byte 4): bits 5:0 except bit 2, lower non-recoverable, which this
///   sensor never sets (`sensor::TEMP_FLAG_LNR`; 003Bh).
/// - Health, Voltage, Payload Test Results and Status (Tables 10.2.3-1, 10.2.4-1, 10.2.6-1,
///   10.2.7-1): bits 1:0, states 01b and 10b (0003h).
const MASK_FRU_STATE: u16 = State::all_mask_at(1) as u16;
const MASK_FRU_STATE_M5: u16 = State::M5.mask() as u16;
const MASK_IPMB_LINK: u16 = 0x000F;
const MASK_IPMB_LINK_FAST: u16 = 0x0030;
/// - Payload Mode (Table 10.2.8-1 byte 4, 5): bit n = operational mode n, modes 0..=14: the modes
///   this payload supports, `IPMC_PAYLOAD_MODES` (defs.rs; the same value Get Payload Mode
///   Capabilities answers).
const MASK_PAYLOAD_MODE: u16 = ipmc_defs::IPMC_PAYLOAD_MODES;
const MASK_TEMPERATURE: u16 =
    (TEMP_FLAG_LNC | TEMP_FLAG_LC | TEMP_FLAG_UNC | TEMP_FLAG_UC | TEMP_FLAG_UNR) as u16;
const MASK_TWO_STATES: u16 = 0x0003;

/// Event masks, the offsets the IPMC sends events for at Tier-2/3 (written 0000h at Tier-1):
/// - FRU State: every reachable state entry (Rule 4.1.5.2-1), the reading mask.
/// - IPMB Link: offsets 0..=3 (Rule 9.3-7); offsets 4 and 5 (400 kHz, Table 10.2.2-2, asserted
///   and deasserted) at Tier-3.
/// - Payload Mode: one event per mode entered, offsets 0..=14 (Table 10.2.8-2), the supported
///   modes (`IPMC_PAYLOAD_MODES`).
/// - Health, Voltage: the new state offset is asserted (Tables 10.2.3-2, 10.2.4-2).
/// - Temperature: offsets 3, 4, 5 asserted on crossing and deasserted when it clears (Rules
///   4.1.7-4, -6, -8).
/// - Payload Test Results and Status: the state never changes on this module.
const EVENTS_NONE: u16 = 0;
const EVENTS_IPMB_LINK: u16 = 0x000F;
const EVENTS_IPMB_LINK_FAST: u16 = 0x0030;
const EVENTS_PAYLOAD_MODE: u16 = ipmc_defs::IPMC_PAYLOAD_MODES;
const EVENTS_TEMPERATURE: u16 = (TEMP_FLAG_UNC | TEMP_FLAG_UC | TEMP_FLAG_UNR) as u16;

/// SENSORS -- sensors 0..=7 in number order; the record ID of a sensor equals its number.
const SENSORS: [SensorDef; MANDATORY_COUNT_MAX] = [
    SensorDef {
        number: SENSOR_FRU_STATE,
        sensor_type: SENSOR_TYPE_FRU_STATE,
        event_type: EVENT_TYPE_SENSOR_SPECIFIC,
        reading_mask: MASK_FRU_STATE,
        assertion_mask: MASK_FRU_STATE,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (MASK_FRU_STATE_M5, MASK_FRU_STATE_M5, 0),
        name: ipmc_defs::IPMC_SDR_FRU_STATE_NAME,
    },
    SensorDef {
        number: SENSOR_SYSTEM_IPMB_LINK,
        sensor_type: SENSOR_TYPE_SYSTEM_IPMB_LINK,
        event_type: EVENT_TYPE_SENSOR_SPECIFIC,
        reading_mask: MASK_IPMB_LINK,
        assertion_mask: EVENTS_IPMB_LINK,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (
            MASK_IPMB_LINK_FAST,
            EVENTS_IPMB_LINK_FAST,
            EVENTS_IPMB_LINK_FAST,
        ),
        name: ipmc_defs::IPMC_SDR_IPMB_LINK_NAME,
    },
    SensorDef {
        number: SENSOR_FRU_HEALTH,
        sensor_type: SENSOR_TYPE_FRU_HEALTH,
        event_type: EVENT_TYPE_PREDICTIVE_FAILURE,
        reading_mask: MASK_TWO_STATES,
        assertion_mask: MASK_TWO_STATES,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_FRU_HEALTH_NAME,
    },
    SensorDef {
        number: SENSOR_FRU_VOLTAGE,
        sensor_type: SENSOR_TYPE_FRU_VOLTAGE,
        event_type: EVENT_TYPE_LIMIT,
        reading_mask: MASK_TWO_STATES,
        assertion_mask: MASK_TWO_STATES,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_FRU_VOLTAGE_NAME,
    },
    SensorDef {
        number: SENSOR_FRU_TEMPERATURE,
        sensor_type: SENSOR_TYPE_FRU_TEMPERATURE,
        event_type: EVENT_TYPE_SENSOR_SPECIFIC,
        reading_mask: MASK_TEMPERATURE,
        assertion_mask: EVENTS_TEMPERATURE,
        deassertion_mask: EVENTS_TEMPERATURE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_FRU_TEMP_NAME,
    },
    SensorDef {
        number: SENSOR_PAYLOAD_TEST_RESULTS,
        sensor_type: SENSOR_TYPE_PAYLOAD_TEST_RESULTS,
        event_type: EVENT_TYPE_PREDICTIVE_FAILURE,
        reading_mask: MASK_TWO_STATES,
        assertion_mask: EVENTS_NONE,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_PAYLOAD_RESULTS_NAME,
    },
    SensorDef {
        number: SENSOR_PAYLOAD_TEST_STATUS,
        sensor_type: SENSOR_TYPE_PAYLOAD_TEST_STATUS,
        event_type: EVENT_TYPE_STATE,
        reading_mask: MASK_TWO_STATES,
        assertion_mask: EVENTS_NONE,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_PAYLOAD_STATUS_NAME,
    },
    SensorDef {
        number: SENSOR_PAYLOAD_MODE,
        sensor_type: SENSOR_TYPE_PAYLOAD_MODE,
        event_type: EVENT_TYPE_SENSOR_SPECIFIC,
        reading_mask: MASK_PAYLOAD_MODE,
        assertion_mask: EVENTS_PAYLOAD_MODE,
        deassertion_mask: EVENTS_NONE,
        tier3_extra: (0, 0, 0),
        name: ipmc_defs::IPMC_SDR_PAYLOAD_MODE_NAME,
    },
];

/// Record -- one SDR record: its bytes and how many of them are used (`ipmc_wire::Builder`).
pub type Record = Builder<RECORD_MAX>;

/// EMPTY_RECORD -- a record with no byte used.
const EMPTY_RECORD: Record = Builder::with_len(0);

/// push_id_string -- the type/length byte and the ID string (8-bit ASCII, section 43.15):
/// 2..=16 bytes, checked when a record is built in a const item.
pub const fn push_id_string(record: &mut Record, text: &str) {
    record.push_type_length_ascii(text, 2, ID_LEN_MAX);
}

/// start_record -- the record header: ID (LS byte first), version, type and a length placeholder.
pub const fn start_record(id: u16, record_type: u8) -> Record {
    let mut record = EMPTY_RECORD;
    record.push_all(&id.to_le_bytes());
    record.push(SDR_VERSION);
    record.push(record_type);
    record.push(0);
    record
}

/// finish_record -- the record length byte: the bytes that follow the header (Table 43-2 byte 5).
pub const fn finish_record(mut record: Record) -> Record {
    record.bytes[HEADER_LEN - 1] = (record.len() - HEADER_LEN) as u8;
    record
}

/// sensor_init -- Sensor Initialization byte (Table 43-2 byte 11) at `tier`.
const fn sensor_init(tier: u32) -> u8 {
    if tier::event_generator(tier) {
        SENSOR_INIT_SCANNING_ON | SENSOR_INIT_EVENTS_ON
    } else {
        SENSOR_INIT_SCANNING_ON
    }
}

/// sensor_capabilities -- Sensor Capabilities byte (Table 43-2 byte 12) at `tier`: the event
/// message control of Tier-1 (none), Tier-2 (global disable only) or Tier-3 (per state).
const fn sensor_capabilities(tier: u32) -> u8 {
    let control = if !tier::event_generator(tier) {
        EVENT_CONTROL_NONE
    } else if tier::per_state_event_enables(tier) {
        EVENT_CONTROL_PER_STATE
    } else {
        EVENT_CONTROL_GLOBAL
    };
    SENSOR_CAPS_AUTO_REARM | control
}

/// tier3_bits -- the (reading, assertion, deassertion) bits `def` adds at `tier`: the M5 bit of
/// the FRU State sensor where `tier::activation_control`, the 400 kHz states of the IPMB Link
/// sensor where `tier::fast_mode`; zero otherwise.
const fn tier3_bits(tier: u32, def: &SensorDef) -> (u16, u16, u16) {
    let on = match def.number {
        SENSOR_FRU_STATE => tier::activation_control(tier),
        SENSOR_SYSTEM_IPMB_LINK => tier::fast_mode(tier),
        _ => false,
    };
    if on {
        def.tier3_extra
    } else {
        (0, 0, 0)
    }
}

/// reading_mask_of -- the reading mask of `def` at `tier` (Table 43-2 bytes 19..20).
const fn reading_mask_of(tier: u32, def: &SensorDef) -> u16 {
    def.reading_mask | tier3_bits(tier, def).0
}

/// event_masks_of -- (assertion, deassertion) masks of `def` at `tier`: 0000h unless the IPMC
/// generates events.
const fn event_masks_of(tier: u32, def: &SensorDef) -> (u16, u16) {
    if tier::event_generator(tier) {
        let extra = tier3_bits(tier, def);
        (def.assertion_mask | extra.1, def.deassertion_mask | extra.2)
    } else {
        (0, 0)
    }
}

/// event_masks -- (assertion, deassertion) event masks of sensor `number` (a mandatory sensor of
/// `tier`, or an analog number) at `tier`, the values the SDR record carries and the power-up
/// event enables start from (`event_enable`), and the most the Tier-3 Set Sensor Event Enable
/// handler (extension) lets a Chassis Manager enable; the analog sensors send no events, so
/// theirs are 0000h; None for a number the Tier does not have.
pub const fn event_masks(tier: u32, number: usize) -> Option<(u16, u16)> {
    if number < mandatory_count(tier) {
        Some(event_masks_of(tier, &SENSORS[number]))
    } else if is_analog_number(number) {
        Some((0, 0))
    } else {
        None
    }
}

/// is_analog_number -- `number` lies in the range the analog sensors take (`ANALOG_FIRST` for
/// `ANALOG_COUNT_MAX` numbers), whether or not a given build publishes all of them.
pub const fn is_analog_number(number: usize) -> bool {
    number >= ANALOG_FIRST as usize && number < ANALOG_FIRST as usize + ANALOG_COUNT_MAX
}

/// compact_record -- the type 02h record of `def` owned by the controller at `owner_address`
/// (the 8-bit IPMB address: bits 7:1 are the 7-bit slave address, bit 0 = 0 "IPMB slave address",
/// Table 43-2 byte 6).
const fn compact_record(id: u16, owner_address: u8, tier: u32, def: &SensorDef) -> Record {
    let (assertion, deassertion) = event_masks_of(tier, def);
    let mut r = start_record(id, RECORD_TYPE_COMPACT);
    r.push(owner_address); // 5: sensor owner ID
    r.push(0); // 6: channel 0, FRU inventory device owner LUN 0, sensor owner LUN 0
    r.push(def.number); // 7: sensor number
    r.push(ENTITY_ID_VPX_MODULE); // 8
    r.push(ENTITY_INSTANCE_DEVICE_RELATIVE); // 9
    r.push(sensor_init(tier)); // 10
    r.push(sensor_capabilities(tier)); // 11
    r.push(def.sensor_type); // 12
    r.push(def.event_type); // 13
    r.push_all(&assertion.to_le_bytes()); // 14..=15: assertion event mask
    r.push_all(&deassertion.to_le_bytes()); // 16..=17: deassertion event mask
    r.push_all(&reading_mask_of(tier, def).to_le_bytes()); // 18..=19: reading mask
    r.push(UNITS1_RESERVED); // 20: sensor units 1
    r.push(0); // 21: base unit, none
    r.push(0); // 22: modifier unit, none
    r.push(SHARE_COUNT_ONE); // 23: direction, ID string modifier type, share count
    r.push(0); // 24: entity instance sharing, ID string modifier offset
    r.push(0); // 25: positive-going hysteresis
    r.push(0); // 26: negative-going hysteresis
    r.push_all(&[0, 0, 0]); // 27..=29: reserved
    r.push(0); // 30: OEM
    push_id_string(&mut r, def.name); // 31: type/length, then the string
    finish_record(r)
}

/// locator_record -- the type 12h management controller device locator record (Table 43-8).
const fn locator_record(id: u16, owner_address: u8, tier: u32) -> Record {
    let capabilities = additional_support(tier)
        | (((DEVICE_IDENTITY.device_revision >> REVISION_SDRS_SHIFT) & 1) * DEVICE_SUPPORT_SDR_BIT);
    let global_init = if tier::event_generator(tier) {
        LOCATOR_INIT_EVENTS_ON
    } else {
        LOCATOR_INIT_EVENTS_OFF
    };
    let mut r = start_record(id, RECORD_TYPE_MC_LOCATOR);
    r.push(owner_address); // 5: device slave address
    r.push(0); // 6: channel 0 (the System IPMB)
    r.push(global_init); // 7: power state notification, global initialization
    r.push(capabilities); // 8: device capabilities, as Get Device ID reports them
    r.push_all(&[0, 0, 0]); // 9..=11: reserved
    r.push(ENTITY_ID_VPX_MODULE); // 12: entity ID of the FRU this device manages
    r.push(ENTITY_INSTANCE_DEVICE_RELATIVE); // 13
    r.push(0); // 14: OEM
    push_id_string(&mut r, ipmc_defs::IPMC_SDR_IPMC_NAME); // 15: type/length, then the string
    finish_record(r)
}

/// build_records -- the records at `tier`: the mandatory sensors as record IDs 0..mandatory_count,
/// the locator after them; the unused slots stay empty. `with_analog` inserts the analog records
/// between the two.
const fn build_records(owner_address: u8, tier: u32) -> [Record; RECORD_COUNT_MAX] {
    let mut records = [EMPTY_RECORD; RECORD_COUNT_MAX];
    let count = mandatory_count(tier);
    let mut i = 0;
    while i < count {
        assert!(
            SENSORS[i].number as usize == i,
            "the sensor table is in number order"
        );
        records[i] = compact_record(i as u16, owner_address, tier, &SENSORS[i]);
        i += 1;
    }
    records[count] = locator_record(count as u16, owner_address, tier);
    records
}

// Builds every record once at compile time: an ID string outside 2..=16 bytes, a record past
// RECORD_MAX or an out-of-order sensor table fails the build here.
const _: [Record; RECORD_COUNT_MAX] = build_records(0x82, ipmc_defs::IPMC_TIER);
const _: [Record; RECORD_COUNT_MAX] = build_records(0x82, 3);
// defs.rs IPMC_SDR_IPMC_NAME: the locator record fits one Get Device SDR response, the
// response data less the 2-byte next record ID (Table 35-3), so a "whole record" read works.
const _: () = assert!(
    locator_record(0, 0x82, ipmc_defs::IPMC_TIER).len() + NO_NEXT_RECORD.to_le_bytes().len()
        <= crate::ipmb::IPMB_RESPONSE_DATA_MAX,
    "IPMC_SDR_IPMC_NAME is too long for the locator record to fit one Get Device SDR response"
);

/// Byte positions (0-based) of the fields `describe` reads back: the sensor number, sensor type
/// and event/reading type of a compact record (Table 43-2 bytes 8, 13, 14), the ID string
/// type/length byte of a compact record (byte 32) and of a locator record (Table 43-8 byte 16).
const IDX_RECORD_TYPE: usize = 3;
const IDX_SENSOR_NUMBER: usize = 7;
const IDX_SENSOR_TYPE: usize = 12;
const IDX_EVENT_TYPE: usize = 13;
const IDX_COMPACT_ID_TL: usize = COMPACT_FIXED_LEN - 1;
const IDX_LOCATOR_ID_TL: usize = 15;
/// LENGTH_MASK -- type/length byte bits 4:0 hold the string length; bit 5 is reserved (section 43.15).
const LENGTH_MASK: u8 = 0x1F;

/// SensorInfo -- the sensor fields of a compact record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorInfo {
    /// Sensor number.
    pub number: u8,
    /// Sensor type code.
    pub sensor_type: u8,
    /// Event/reading type code.
    pub event_type: u8,
}

/// IDX_FULL_ID_TL -- the ID string type/length byte of a full record (Table 43-1 byte 48).
const IDX_FULL_ID_TL: usize = FULL_FIXED_LEN - 1;

/// RecordInfo -- a record decoded for the console listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordInfo<'a> {
    /// Record ID.
    pub id: u16,
    /// SDR record type (01h full sensor, 02h compact sensor, 12h management controller device
    /// locator).
    pub record_type: u8,
    /// Whole record length in bytes.
    pub length: usize,
    /// The sensor fields; None for the locator record.
    pub sensor: Option<SensorInfo>,
    /// The ID string bytes.
    pub name: &'a [u8],
}

/// SdrRepository -- the records and the reservation counter.
#[derive(Clone, Copy, Debug)]
pub struct SdrRepository {
    records: [Record; RECORD_COUNT_MAX],
    /// The sensors this repository describes: the Tier's mandatory sensors (`mandatory_count`)
    /// and the analog ones; the records after the locator are unused.
    sensors: usize,
    /// The mandatory sensors among them; the analog records follow, then the locator.
    mandatory: usize,
    /// The latest reservation ID handed out; 0 before the first Reserve Device SDR Repository.
    reservation: u16,
}

impl SdrRepository {
    /// new -- the repository for the controller at `owner_address` (8-bit IPMB address).
    pub const fn new(owner_address: u8) -> SdrRepository {
        SdrRepository::for_tier(owner_address, ipmc_defs::IPMC_TIER)
    }

    /// for_tier -- the repository as a Tier-`tier` IPMC describes itself (`new` passes the
    /// build's `IPMC_TIER`); the Tier changes the event bytes of the records, not their lengths.
    pub const fn for_tier(owner_address: u8, tier: u32) -> SdrRepository {
        SdrRepository {
            records: build_records(owner_address, tier),
            sensors: mandatory_count(tier),
            mandatory: mandatory_count(tier),
            reservation: 0,
        }
    }

    /// with_analog -- the repository of Tier-`tier` with the Full Sensor Records of `analog`
    /// between the compact records and the locator (record IDs on from `mandatory_count`, the
    /// locator last). Sensors beyond `ANALOG_COUNT_MAX`, or a record `analog` does not deliver, end
    /// the list there.
    pub fn with_analog(owner_address: u8, tier: u32, analog: &dyn AnalogSensors) -> SdrRepository {
        let mut repository = SdrRepository::for_tier(owner_address, tier);
        let mandatory = repository.mandatory;
        let locator = repository.records[mandatory];
        let mut added = 0;
        while added < analog.count().min(ANALOG_COUNT_MAX) {
            let id = (mandatory + added) as u16;
            let Some(record) = analog.record(added, owner_address, id) else {
                break;
            };
            repository.records[mandatory + added] = record;
            added += 1;
        }
        let at = mandatory + added;
        repository.records[at] = locator;
        repository.records[at].bytes[0..2].copy_from_slice(&(at as u16).to_le_bytes());
        repository.sensors = at;
        repository
    }

    /// record_count -- the number of records (Get Device SDR Info "SDR count").
    pub const fn record_count(&self) -> usize {
        self.sensors + 1
    }

    /// mandatory_sensors -- how many sensors are VITA mandatory ones; the analog sensors follow.
    pub const fn mandatory_sensors(&self) -> usize {
        self.mandatory
    }

    /// sensor_count -- the number of sensors (Get Device SDR Info "sensor count").
    pub const fn sensor_count(&self) -> usize {
        self.sensors
    }

    /// set_record_byte -- test hook: changes byte `index` of record `id` (no effect when either is
    /// out of range), to build a damaged repository for the self test.
    #[cfg(test)]
    pub(crate) fn set_record_byte(&mut self, id: usize, index: usize, value: u8) {
        if let Some(byte) = self
            .records
            .get_mut(id)
            .and_then(|record| record.bytes.get_mut(index))
        {
            *byte = value;
        }
    }

    /// locator_record_id -- the record ID of the management controller device locator record.
    pub const fn locator_record_id(&self) -> u16 {
        self.sensors as u16
    }

    /// resolve -- the stored record ID a requested ID names: 0000h is the first record and FFFFh
    /// the last (IPMI v2.0 section 33.12); None when no record has that ID.
    pub fn resolve(&self, requested: u16) -> Option<u16> {
        match requested {
            0 => Some(0),
            NO_NEXT_RECORD => Some(self.locator_record_id()),
            id if usize::from(id) < self.record_count() => Some(id),
            _ => None,
        }
    }

    /// record -- the bytes of record `id`.
    pub fn record(&self, id: u16) -> Option<&[u8]> {
        self.records
            .get(..self.record_count())?
            .get(usize::from(id))
            .map(|record| record.as_bytes())
    }

    /// describe -- record `id` decoded: type, length, sensor fields and ID string; None when there
    /// is no such record or its bytes are shorter than the layout (cannot happen for a built one).
    pub fn describe(&self, id: u16) -> Option<RecordInfo<'_>> {
        let bytes = self.record(id)?;
        let record_type = *bytes.get(IDX_RECORD_TYPE)?;
        let (sensor, tl_index) =
            if record_type == RECORD_TYPE_COMPACT || record_type == RECORD_TYPE_FULL {
                let sensor = SensorInfo {
                    number: *bytes.get(IDX_SENSOR_NUMBER)?,
                    sensor_type: *bytes.get(IDX_SENSOR_TYPE)?,
                    event_type: *bytes.get(IDX_EVENT_TYPE)?,
                };
                let tl_index = if record_type == RECORD_TYPE_FULL {
                    IDX_FULL_ID_TL
                } else {
                    IDX_COMPACT_ID_TL
                };
                (Some(sensor), tl_index)
            } else {
                (None, IDX_LOCATOR_ID_TL)
            };
        let name_len = usize::from(*bytes.get(tl_index)? & LENGTH_MASK);
        Some(RecordInfo {
            id,
            record_type,
            length: bytes.len(),
            sensor,
            name: bytes.get(tl_index + 1..tl_index + 1 + name_len)?,
        })
    }

    /// next_id -- the record ID after `id`, NO_NEXT_RECORD (FFFFh) after the last.
    pub fn next_id(&self, id: u16) -> u16 {
        match id.checked_add(1) {
            Some(next) if usize::from(next) < self.record_count() => next,
            _ => NO_NEXT_RECORD,
        }
    }

    /// reserve -- hands out the next reservation ID: it increments and skips 0, which is reserved
    /// (IPMI v2.0 Table 35-4: "0000h reserved"). Any earlier ID stops being valid.
    pub fn reserve(&mut self) -> u16 {
        self.reservation = self.reservation.checked_add(1).unwrap_or(1);
        self.reservation
    }

    /// reservation_valid -- `id` is the current reservation.
    pub fn reservation_valid(&self, id: u16) -> bool {
        id != 0 && id == self.reservation
    }

    /// reservation -- the latest reservation ID handed out (0 = none yet).
    pub fn reservation(&self) -> u16 {
        self.reservation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u8 = 0x82;

    /// Sensor 0, hand-written from Table 43-2 for "FRU State" (9 characters): ID 0000h, version
    /// 51h, type 02h, length 27 + 9 = 24h; owner 82h; LUN 00h; sensor 00h; entity A0h, 60h; init
    /// 01h; capabilities 43h; type F0h; event/reading type 6Fh; masks 0000h, 0000h, 0053h (LS byte
    /// first); units 1 C0h, 00h, 00h; share count 01h; 00h; hysteresis 00h, 00h; reserved 00h x 3;
    /// OEM 00h; type/length C9h; "FRU State".
    const SENSOR0: [u8; 41] = [
        0x00, 0x00, 0x51, 0x02, 0x24, 0x82, 0x00, 0x00, 0xA0, 0x60, 0x01, 0x43, 0xF0, 0x6F, 0x00,
        0x00, 0x00, 0x00, 0x53, 0x00, 0xC0, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xC9, b'F', b'R', b'U', b' ', b'S', b't', b'a', b't', b'e',
    ];

    /// Locator, from Table 43-8 for "IPMC" (4 characters): ID 0007h (8 at Tier-3), 51h, 12h, length 11 + 4 =
    /// 0Fh; slave address 82h; channel 00h; global init 01h; capabilities 0Bh (sensor, device
    /// SDRs, FRU inventory); reserved 00h x 3; entity A0h, 60h; OEM 00h; C4h; "IPMC".
    const LOCATOR: [u8; 20] = [
        0x07, 0x00, 0x51, 0x12, 0x0F, 0x82, 0x00, 0x01, 0x0B, 0x00, 0x00, 0x00, 0xA0, 0x60, 0x00,
        0xC4, b'I', b'P', b'M', b'C',
    ];

    fn repo() -> SdrRepository {
        SdrRepository::new(OWNER)
    }

    #[test]
    fn sensor_zero_and_the_locator_are_byte_exact() {
        // The constants above are the Tier-1 records; Tier-2/3 differ in the sensor initialization
        // (byte 10), capabilities (byte 11) and assertion mask (bytes 14..16) of sensor 0 and in
        // the locator's global initialization (byte 7) and capabilities (byte 8).
        let tier = ipmc_defs::IPMC_TIER as usize - 1;
        let (init, caps, assertion) = [
            (0x01, 0x43, 0x0000u16),
            (0x03, 0x42, 0x0053),
            (0x03, 0x40, 0x0073),
        ][tier];
        let (global_init, locator_caps) = [(0x01, 0x0B), (0x00, 0x2B), (0x00, 0x2F)][tier];
        let reading = [0x0053u16, 0x0053, 0x0073][tier];
        let mut sensor0 = SENSOR0;
        sensor0[10] = init;
        sensor0[11] = caps;
        sensor0[14..16].copy_from_slice(&assertion.to_le_bytes());
        sensor0[18..20].copy_from_slice(&reading.to_le_bytes());
        let r = repo();
        let mut locator = LOCATOR;
        // The locator is the record after the sensors: ID 7 below Tier-3, 8 with sensor 7.
        locator[..2].copy_from_slice(&r.locator_record_id().to_le_bytes());
        locator[7] = global_init;
        locator[8] = locator_caps;
        assert_eq!(r.record(0).unwrap(), &sensor0);
        assert_eq!(r.record(r.locator_record_id()).unwrap(), &locator);
    }

    #[test]
    fn records_parse_back_field_by_field() {
        // (sensor number, type, event/reading type, reading mask, ID string) from VITA Table
        // 10.2-1 and the reading tables, written out here, not read from SENSORS.
        let expect: [(u8, u8, u8, u16, &str); 7] = [
            (0, 0xF0, 0x6F, 0x0053, "FRU State"),
            (1, 0xF1, 0x6F, 0x000F, "IPMB Link"),
            (2, 0xF2, 0x04, 0x0003, "FRU Health"),
            (3, 0x02, 0x05, 0x0003, "FRU Voltage"),
            (4, 0xF3, 0x6F, 0x003B, "FRU Temperature"),
            (5, 0xF4, 0x04, 0x0003, "Payload Results"),
            (6, 0xF5, 0x03, 0x0003, "Payload Status"),
        ];
        // Sensor initialization and capabilities (bytes 10, 11) per Tier, and the event masks
        // (bytes 14..18: assertion, deassertion) of sensors 0..=6 at Tier-2/3 (Tier-1: all zero).
        let tier = (ipmc_defs::IPMC_TIER as usize - 1).min(1);
        let (init, caps) = [(0x01, 0x43), (0x03, 0x42)][tier];
        let masks: [[u8; 4]; 7] = [
            [0x53, 0x00, 0x00, 0x00],
            [0x0F, 0x00, 0x00, 0x00],
            [0x03, 0x00, 0x00, 0x00],
            [0x03, 0x00, 0x00, 0x00],
            [0x38, 0x00, 0x38, 0x00],
            [0; 4],
            [0; 4],
        ];
        // Tier-3 differs from this table in the masks of sensors 0 and 1 (and has sensor 7):
        // `tier_3_adds_m5_the_400_khz_states_and_the_payload_mode_sensor` pins those.
        let r = SdrRepository::for_tier(OWNER, tier as u32 + 1);
        for (id, (number, ty, ev, mask, name)) in expect.into_iter().enumerate() {
            let b = r.record(id as u16).unwrap();
            assert_eq!(u16::from_le_bytes([b[0], b[1]]), id as u16, "record ID");
            assert_eq!((b[2], b[3]), (0x51, 0x02));
            assert_eq!(
                usize::from(b[4]),
                b.len() - 5,
                "length byte counts the rest"
            );
            assert_eq!(usize::from(b[4]), 27 + name.len());
            assert_eq!(b[5], OWNER, "sensor owner is our IPMB address");
            assert_eq!((b[6], b[7]), (0, number));
            assert_eq!((b[8], b[9]), (0xA0, 0x60));
            assert_eq!((b[10], b[11]), (init, caps));
            assert_eq!((b[12], b[13]), (ty, ev));
            assert_eq!(
                &b[14..18],
                &[[0; 4], masks[id], masks[id]][tier],
                "event masks"
            );
            assert_eq!(u16::from_le_bytes([b[18], b[19]]), mask);
            assert_eq!(&b[20..23], &[0xC0, 0, 0]);
            assert_eq!(b[23], 1);
            assert_eq!(&b[24..31], &[0; 7]);
            assert_eq!(b[31], 0xC0 | name.len() as u8);
            assert_eq!(&b[32..], name.as_bytes());
        }
    }

    #[test]
    fn event_bytes_follow_the_tier() {
        // Per tier: sensor initialization (byte 10), capabilities (byte 11), the locator global
        // initialization (byte 7) and capabilities (byte 8); literals from IPMI v2.0 Tables 43-2
        // and 43-8, with the offsets of the VITA masks table in the module comment.
        // (tier, init, capabilities, locator global init, locator capabilities)
        let tiers = [
            (1u32, 0x01u8, 0x43u8, 0x01u8, 0x0Bu8),
            (2, 0x03, 0x42, 0x00, 0x2B),
            (3, 0x03, 0x40, 0x00, 0x2F),
        ];
        // (assertion, deassertion) of sensors 0..=6 at Tier-2/3.
        let masks: [(u16, u16); 7] = [
            (0x0053, 0),
            (0x000F, 0),
            (0x0003, 0),
            (0x0003, 0),
            (0x0038, 0x0038),
            (0, 0),
            (0, 0),
        ];
        for (tier, init, caps, global_init, locator_caps) in tiers {
            let r = SdrRepository::for_tier(OWNER, tier);
            for (id, (assertion, deassertion)) in masks.into_iter().enumerate() {
                if tier == 3 && id < 2 {
                    continue; // M5 and the 400 kHz states: the Tier-3 test below
                }
                let b = r.record(id as u16).unwrap();
                assert_eq!((b[10], b[11]), (init, caps), "tier {tier} sensor {id}");
                let (want_a, want_d) = if tier == 1 {
                    (0, 0)
                } else {
                    (assertion, deassertion)
                };
                assert_eq!(
                    u16::from_le_bytes([b[14], b[15]]),
                    want_a,
                    "tier {tier} sensor {id} assertion"
                );
                assert_eq!(
                    u16::from_le_bytes([b[16], b[17]]),
                    want_d,
                    "tier {tier} sensor {id} deassertion"
                );
                assert_eq!(usize::from(b[4]), b.len() - 5, "length unchanged by tier");
            }
            let l = r.record(r.locator_record_id()).unwrap();
            assert_eq!(
                (l[7], l[8]),
                (global_init, locator_caps),
                "tier {tier} locator"
            );
            assert_eq!(l.len(), LOCATOR.len(), "tier {tier} locator length");
        }
        assert_eq!(
            SdrRepository::new(OWNER).record(0),
            SdrRepository::for_tier(OWNER, ipmc_defs::IPMC_TIER).record(0)
        );
    }

    #[test]
    fn record_ids_are_consecutive_from_zero_and_chain_ends_at_ffff() {
        let r = repo();
        // (tier, records, sensors): Tier-3 adds sensor 7 and the locator moves behind it.
        for (tier, records, sensors) in [(1u32, 8u16, 7usize), (2, 8, 7), (3, 9, 8)] {
            let r = SdrRepository::for_tier(OWNER, tier);
            assert_eq!(
                (r.record_count(), r.sensor_count()),
                (usize::from(records), sensors)
            );
            let mut id = r.resolve(0).unwrap();
            let mut seen = vec![id];
            loop {
                let next = r.next_id(id);
                if next == NO_NEXT_RECORD {
                    break;
                }
                seen.push(next);
                id = next;
            }
            assert_eq!(seen, (0..records).collect::<Vec<u16>>(), "tier {tier}");
            assert_eq!(r.locator_record_id(), records - 1);
            assert_eq!(r.resolve(0xFFFF), Some(records - 1));
            assert_eq!(r.resolve(records), None);
            assert!(r.record(records).is_none());
            assert!(r.describe(records).is_none());
            assert_eq!(r.describe(records - 1).unwrap().record_type, 0x12);
        }
        assert_eq!(r.record_count(), mandatory_count(ipmc_defs::IPMC_TIER) + 1);
    }

    #[test]
    fn resolve_maps_first_and_last_and_rejects_unknown_ids() {
        let r = repo();
        assert_eq!(r.resolve(0x0000), Some(0));
        assert_eq!(r.resolve(0xFFFF), Some(r.locator_record_id()));
        assert_eq!(r.resolve(5), Some(5));
        assert_eq!(r.resolve(0x0100), None);
    }

    #[test]
    fn reservation_increments_skips_zero_on_wrap_and_invalidates_the_old_id() {
        let mut r = repo();
        assert_eq!(r.reservation(), 0);
        assert!(!r.reservation_valid(0), "0000h is never a reservation");
        assert_eq!(r.reserve(), 1);
        assert!(r.reservation_valid(1));
        assert_eq!(r.reserve(), 2);
        assert!(!r.reservation_valid(1), "the earlier ID is stale");
        assert!(r.reservation_valid(2));
        r.reservation = u16::MAX;
        assert_eq!(r.reserve(), 1, "wraps past 0");
        assert!(!r.reservation_valid(0));
    }

    #[test]
    fn describe_reads_the_fields_back() {
        let r = repo();
        let info = r.describe(4).unwrap();
        assert_eq!((info.id, info.record_type, info.length), (4, 0x02, 47));
        assert_eq!(
            info.sensor,
            Some(SensorInfo {
                number: 4,
                sensor_type: 0xF3,
                event_type: 0x6F
            })
        );
        assert_eq!(info.name, b"FRU Temperature");
        let locator = r.describe(r.locator_record_id()).unwrap();
        assert_eq!((locator.record_type, locator.length), (0x12, 20));
        assert_eq!((locator.sensor, locator.name), (None, &b"IPMC"[..]));
    }

    #[test]
    fn owner_address_follows_the_ipmb_address() {
        let r = SdrRepository::new(0x86);
        for id in 0..r.record_count() as u16 {
            assert_eq!(r.record(id).unwrap()[5], 0x86);
        }
    }

    #[test]
    fn tier_3_adds_m5_the_400_khz_states_and_the_payload_mode_sensor() {
        let r = SdrRepository::for_tier(OWNER, 3);
        let mask = |id: u16, at: usize| {
            let b = r.record(id).unwrap();
            u16::from_le_bytes([b[at], b[at + 1]])
        };
        // FRU State (Table 10.2.1-1): M0, M1, M4, M5, M6 read and asserted: 0073h.
        assert_eq!((mask(0, 14), mask(0, 16), mask(0, 18)), (0x0073, 0, 0x0073));
        // IPMB Link (Table 10.2.2-2): offsets 0..=5 asserted, 4 and 5 also deasserted; the
        // reading mask holds states 0..=5 (003Fh).
        assert_eq!(
            (mask(1, 14), mask(1, 16), mask(1, 18)),
            (0x003F, 0x0030, 0x003F)
        );
        // Payload Mode, hand-written from Table 43-2 for "Payload Mode" (12 characters): ID 0007h,
        // 51h, 02h, length 27 + 12 = 27h; owner 82h; LUN 00h; sensor 07h; entity A0h, 60h; init
        // 03h; capabilities 40h; type F7h; event/reading type 6Fh; assertion IPMC_PAYLOAD_MODES,
        // deassertion 0000h, reading mask IPMC_PAYLOAD_MODES; units 1 C0h, 00h, 00h; share count
        // 01h; 00h; hysteresis 00h x 2; reserved 00h x 3; OEM 00h; type/length CCh; "Payload
        // Mode".
        let [modes_ls, modes_ms] = ipmc_defs::IPMC_PAYLOAD_MODES.to_le_bytes();
        let mut want = vec![
            0x07, 0x00, 0x51, 0x02, 0x27, 0x82, 0x00, 0x07, 0xA0, 0x60, 0x03, 0x40, 0xF7, 0x6F,
            modes_ls, modes_ms, 0x00, 0x00, modes_ls, modes_ms, 0xC0, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0xCC,
        ];
        want.extend_from_slice(b"Payload Mode");
        assert_eq!(r.record(7).unwrap(), want.as_slice());
        // The locator is the last record, ID 8, and its bytes are those of Tier-3 (2Fh).
        let locator = r.record(8).unwrap();
        assert_eq!(&locator[..2], &[0x08, 0x00]);
        assert_eq!((locator[7], locator[8]), (0x00, 0x2F));
        // Below Tier-3 sensor 7 does not exist and the masks lack the Tier-3 bits.
        let two = SdrRepository::for_tier(OWNER, 2);
        assert_eq!(&two.record(0).unwrap()[18..20], &[0x53, 0x00]);
        assert_eq!(&two.record(1).unwrap()[14..20], &[0x0F, 0, 0, 0, 0x0F, 0]);
        assert_eq!(two.record(7).unwrap()[3], 0x12, "record 7 is the locator");
    }
}

#[cfg(test)]
mod analog_tests {
    use super::*;
    use crate::sensor::{EncodeError, SensorSnapshot};

    /// Two analog sensors whose records are one marker byte each after a valid header.
    struct Two;

    impl AnalogSensors for Two {
        fn count(&self) -> usize {
            2
        }

        fn record(&self, index: usize, owner_address: u8, id: u16) -> Option<Record> {
            if index >= 2 {
                return None;
            }
            let mut r = start_record(id, RECORD_TYPE_FULL);
            r.push(owner_address);
            r.push(0);
            r.push(ANALOG_FIRST + index as u8);
            push_id_string(&mut r, "Analog");
            Some(finish_record(r))
        }

        fn read(
            &self,
            _index: usize,
            _snapshot: &SensorSnapshot,
            _out: &mut [u8],
        ) -> Result<usize, EncodeError> {
            Err(EncodeError::UnsupportedSensor)
        }
    }

    #[test]
    fn analog_records_sit_between_the_compact_records_and_the_locator() {
        // (tier, mandatory sensors): the locator moves behind the analog records, its ID follows.
        for (tier, mandatory) in [(1u32, 7usize), (2, 7), (3, 8)] {
            let r = SdrRepository::with_analog(0x82, tier, &Two);
            assert_eq!(r.sensor_count(), mandatory + 2, "tier {tier}");
            assert_eq!(r.record_count(), mandatory + 3, "tier {tier}");
            assert_eq!(r.mandatory_sensors(), mandatory);
            let last = (mandatory + 2) as u16;
            assert_eq!(r.locator_record_id(), last);
            assert_eq!(r.resolve(0xFFFF), Some(last));
            // The walk by next ID visits every ID once and ends at FFFFh.
            let mut seen = vec![0u16];
            while r.next_id(*seen.last().unwrap()) != NO_NEXT_RECORD {
                seen.push(r.next_id(*seen.last().unwrap()));
            }
            assert_eq!(seen, (0..=last).collect::<Vec<u16>>(), "tier {tier}");
            // Compact records unchanged, analog records Full (type 01h) with their IDs, then the
            // locator (type 12h) with the new ID.
            for id in 0..mandatory as u16 {
                assert_eq!(r.record(id).unwrap()[3], RECORD_TYPE_COMPACT);
            }
            for n in 0..2u16 {
                let id = mandatory as u16 + n;
                let b = r.record(id).unwrap();
                assert_eq!(u16::from_le_bytes([b[0], b[1]]), id);
                assert_eq!((b[3], b[5], b[7]), (RECORD_TYPE_FULL, 0x82, 8 + n as u8));
            }
            let locator = r.record(last).unwrap();
            assert_eq!(
                (locator[3], u16::from_le_bytes([locator[0], locator[1]])),
                (0x12, last)
            );
            // Without the hook the repository is the mandatory one.
            let plain = SdrRepository::for_tier(0x82, tier);
            assert_eq!(plain.record_count(), mandatory + 1);
            assert_eq!(plain.mandatory_sensors(), plain.sensor_count());
        }
    }
}
