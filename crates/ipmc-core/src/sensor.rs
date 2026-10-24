//! sensor.rs - Tier-1 sensor data types and the Get Sensor Reading data encoders.
//!
//! Holds the VITA 46.11 mandatory sensor numbers (Table 10.2-1), the plain-data snapshot the
//! sensors crate fills, and one encoder per sensor that the board feeds: FRU Health
//! (Table 10.2.3-1), FRU Voltage (Table 10.2.4-1) and FRU Temperature (Table 10.2.5-1).
//! No hardware and no decisions about thresholds live here; `ipmc-sensors` builds the snapshot.
//!
//! Response data layout, the bytes after the completion code (IPMI v2.0 Get Sensor Reading):
//!   byte 2  sensor reading; 00h for these discrete sensors
//!   byte 3  status: bit 7 event messages enabled (the sensor's `event_enable` flag, set by
//!           `apply_event_enable`; 0 at Tier-1, which sends no events),
//!           bit 6 sensor scanning enabled (1), bit 5 reading unavailable
//!   byte 4  state bits as each table specifies

/// SENSOR_FRU_STATE -- Sensor number of FRU State (type F0h, reading type 6Fh).
pub const SENSOR_FRU_STATE: u8 = 0;
/// SENSOR_SYSTEM_IPMB_LINK -- Sensor number of System IPMB Link (type F1h, reading type 6Fh).
pub const SENSOR_SYSTEM_IPMB_LINK: u8 = 1;
/// SENSOR_FRU_HEALTH -- Sensor number of FRU Health (type F2h, reading type 04h).
pub const SENSOR_FRU_HEALTH: u8 = 2;
/// SENSOR_FRU_VOLTAGE -- Sensor number of FRU Voltage (type 02h, reading type 05h).
pub const SENSOR_FRU_VOLTAGE: u8 = 3;
/// SENSOR_FRU_TEMPERATURE -- Sensor number of FRU Temperature (type F3h, reading type 6Fh).
pub const SENSOR_FRU_TEMPERATURE: u8 = 4;
/// SENSOR_PAYLOAD_TEST_RESULTS -- Sensor number of Payload Test Results (type F4h, reading type 04h).
pub const SENSOR_PAYLOAD_TEST_RESULTS: u8 = 5;
/// SENSOR_PAYLOAD_TEST_STATUS -- Sensor number of Payload Test Status (type F5h, reading type 03h).
pub const SENSOR_PAYLOAD_TEST_STATUS: u8 = 6;

/// SENSOR_TYPE_FRU_STATE -- Sensor type of FRU State.
pub const SENSOR_TYPE_FRU_STATE: u8 = 0xF0;
/// SENSOR_TYPE_SYSTEM_IPMB_LINK -- Sensor type of System IPMB Link.
pub const SENSOR_TYPE_SYSTEM_IPMB_LINK: u8 = 0xF1;
/// SENSOR_TYPE_FRU_HEALTH -- Sensor type of FRU Health.
pub const SENSOR_TYPE_FRU_HEALTH: u8 = 0xF2;
/// SENSOR_TYPE_FRU_VOLTAGE -- Sensor type of FRU Voltage (IPMI sensor type 02h, Voltage).
pub const SENSOR_TYPE_FRU_VOLTAGE: u8 = 0x02;
/// SENSOR_TYPE_FRU_TEMPERATURE -- Sensor type of FRU Temperature.
pub const SENSOR_TYPE_FRU_TEMPERATURE: u8 = 0xF3;
/// SENSOR_TYPE_PAYLOAD_TEST_RESULTS -- Sensor type of Payload Test Results.
pub const SENSOR_TYPE_PAYLOAD_TEST_RESULTS: u8 = 0xF4;
/// SENSOR_TYPE_PAYLOAD_TEST_STATUS -- Sensor type of Payload Test Status.
pub const SENSOR_TYPE_PAYLOAD_TEST_STATUS: u8 = 0xF5;

/// EVENT_TYPE_SENSOR_SPECIFIC -- event/reading type 6Fh, sensor-specific (Table 10.2-1: sensors 0, 1, 4).
pub const EVENT_TYPE_SENSOR_SPECIFIC: u8 = 0x6F;
/// EVENT_TYPE_PREDICTIVE_FAILURE -- event/reading type 04h, "digital" discrete (Table 10.2-1:
/// sensors 2 and 5).
pub const EVENT_TYPE_PREDICTIVE_FAILURE: u8 = 0x04;
/// EVENT_TYPE_LIMIT -- event/reading type 05h, "digital" discrete limit exceeded (sensor 3).
pub const EVENT_TYPE_LIMIT: u8 = 0x05;
/// EVENT_TYPE_STATE -- event/reading type 03h, state deasserted/asserted (sensor 6).
pub const EVENT_TYPE_STATE: u8 = 0x03;

/// TEMP_FLAG_LNC -- Temperature state bit 0 (Table 10.2.5-1): at or below lower non-critical.
pub const TEMP_FLAG_LNC: u8 = 1 << 0;
/// TEMP_FLAG_LC -- Temperature state bit 1: at or below lower critical.
pub const TEMP_FLAG_LC: u8 = 1 << 1;
/// TEMP_FLAG_LNR -- Temperature state bit 2: at or below lower non-recoverable. The board defines no such
/// threshold, so the sensors crate never sets it.
pub const TEMP_FLAG_LNR: u8 = 1 << 2;
/// TEMP_FLAG_UNC -- Temperature state bit 3: at or above upper non-critical.
pub const TEMP_FLAG_UNC: u8 = 1 << 3;
/// TEMP_FLAG_UC -- Temperature state bit 4: at or above upper critical.
pub const TEMP_FLAG_UC: u8 = 1 << 4;
/// TEMP_FLAG_UNR -- Temperature state bit 5: at or above upper non-recoverable.
pub const TEMP_FLAG_UNR: u8 = 1 << 5;

/// Status byte bit 6: sensor scanning enabled (IPMI v2.0 Get Sensor Reading byte 3).
const STATUS_SCANNING_ENABLED: u8 = 1 << 6;
/// Status byte bit 7: event messages enabled for the sensor.
const STATUS_EVENTS_ENABLED: u8 = 1 << 7;
/// Status byte bit 5: reading/state unavailable.
const STATUS_UNAVAILABLE: u8 = 1 << 5;
/// Byte 2 of a discrete sensor.
const READING_DISCRETE: u8 = 0x00;
/// READING_LEN -- bytes after the completion code that the encoders write: reading, status, state.
pub const READING_LEN: usize = 3;
/// STATUS_INDEX -- position of the status byte within the bytes the encoders write.
const STATUS_INDEX: usize = 1;

/// Table 10.2.3-1 and 10.2.4-1 byte 4 bits 1:0: 01b is the good state, 10b the bad state.
const STATE_GOOD: u8 = 0b01;
const STATE_BAD: u8 = 0b10;

/// RailStatus -- where one rail voltage sits against its windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailStatus {
    InRange,
    NonCritical,
    Critical,
    /// The source could not deliver a reading.
    Unavailable,
}

impl RailStatus {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            RailStatus::InRange => "in range",
            RailStatus::NonCritical => "non-critical",
            RailStatus::Critical => "critical",
            RailStatus::Unavailable => "unavailable",
        }
    }
}

/// PowerState -- the payload power-enable state machine (`ipmc-sensors` eval/power_fsm.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    Off,
    PowerOnRequested,
    On,
    Fault,
}

impl PowerState {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            PowerState::Off => "Off",
            PowerState::PowerOnRequested => "PowerOnRequested",
            PowerState::On => "On",
            PowerState::Fault => "Fault",
        }
    }
}

/// Summary -- one of the three Tier-1 discrete readings before encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summary {
    /// Table state 01b: functioning properly, or all voltages within range.
    Good,
    /// Table state 10b: not functioning properly, or a voltage out of range.
    Bad,
    /// No reading: the status byte reports "reading unavailable".
    Unavailable,
}

impl Summary {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            Summary::Good => "good",
            Summary::Bad => "bad",
            Summary::Unavailable => "unavailable",
        }
    }
}

/// RailReading -- one rail: millivolts at the rail (after the divider) and its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailReading {
    pub mv: u32,
    pub status: RailStatus,
}

/// TemperatureReading -- die temperature in tenths of a degree C and its threshold flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TemperatureReading {
    pub available: bool,
    /// Tenths of a degree C; meaningful only when `available`.
    pub dc: i32,
    /// OR of the `TEMP_FLAG_*` bits.
    pub flags: u8,
}

/// RAILS_MAX -- capacity of the rail array (ipmi_defs.h IPMC_RAILS_MAX).
pub const RAILS_MAX: usize = ipmc_defs::IPMC_RAILS_MAX as usize;

/// SensorSnapshot -- everything one sample produced, as plain data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorSnapshot {
    /// The first `rail_count` entries are valid, in board-table order.
    pub rails: [RailReading; RAILS_MAX],
    pub rail_count: usize,
    pub temperature: TemperatureReading,
    /// Bit n = input n of the board input table is asserted (at its active level).
    pub inputs_asserted: u32,
    /// Bit n = input n could not be read; its asserted bit is clear and means nothing.
    pub inputs_unreadable: u32,
    pub power: PowerState,
    /// PWR_EN is driven or requested.
    pub power_enabled: bool,
    /// Payload power is completely shut down: PWR_EN is released (no enable request, no
    /// `EnableWrite` fault, whose output level is unknown) and the PGOOD input was read and is
    /// deasserted. An unreadable PGOOD makes it false. It ends FRU state M6 (VITA Rule
    /// 4.1.5.4.5-1); `Context::update_sensors` hands it to the state machine.
    pub payload_shut_down: bool,
    pub health: Summary,
    pub voltage: Summary,
}

impl SensorSnapshot {
    /// EMPTY -- the state before the first sample: nothing available, power off.
    pub const EMPTY: SensorSnapshot = SensorSnapshot {
        rails: [RailReading {
            mv: 0,
            status: RailStatus::Unavailable,
        }; RAILS_MAX],
        rail_count: 0,
        temperature: TemperatureReading {
            available: false,
            dc: 0,
            flags: 0,
        },
        inputs_asserted: 0,
        inputs_unreadable: 0,
        power: PowerState::Off,
        power_enabled: false,
        // Before the first sample the supplies are off by construction (power-up state Off).
        payload_shut_down: true,
        health: Summary::Unavailable,
        voltage: Summary::Unavailable,
    };

    /// valid_rails -- the rail entries filled by the last sample.
    pub fn valid_rails(&self) -> &[RailReading] {
        self.rails.get(..self.rail_count).unwrap_or(&[])
    }
}

/// EncodeError -- why a reading was not encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer is shorter than `READING_LEN`.
    BufferTooShort,
    /// The sensor number has no encoder here.
    UnsupportedSensor,
}

impl EncodeError {
    /// name -- text for log lines.
    pub fn name(self) -> &'static str {
        match self {
            EncodeError::BufferTooShort => "buffer too short",
            EncodeError::UnsupportedSensor => "unsupported sensor",
        }
    }
}

/// write_reading -- the one writer of the three data bytes: byte 2 (`reading`), the status byte,
/// byte 4 (`state`). An unavailable reading carries zero in bytes 2 and 4 and the unavailable bit.
fn write_reading(
    out: &mut [u8],
    available: bool,
    reading: u8,
    state: u8,
) -> Result<usize, EncodeError> {
    let Some([reading_byte, status, states]) = out.get_mut(..READING_LEN) else {
        return Err(EncodeError::BufferTooShort);
    };
    if available {
        *reading_byte = reading;
        *status = STATUS_SCANNING_ENABLED;
        *states = state;
    } else {
        *reading_byte = 0;
        *status = STATUS_SCANNING_ENABLED | STATUS_UNAVAILABLE;
        *states = 0;
    }
    Ok(READING_LEN)
}

/// apply_event_enable -- sets or clears status byte bit 7 (event messages enabled) of the
/// `READING_LEN` bytes an encoder wrote to the front of `out`; the one place the bit is written.
pub fn apply_event_enable(out: &mut [u8], enabled: bool) {
    if let Some(status) = out.get_mut(STATUS_INDEX) {
        if enabled {
            *status |= STATUS_EVENTS_ENABLED;
        } else {
            *status &= !STATUS_EVENTS_ENABLED;
        }
    }
}

/// summary_state -- (available, byte 4 bits 1:0) of a summary, Tables 10.2.3-1 and 10.2.4-1.
fn summary_state(summary: Summary) -> (bool, u8) {
    match summary {
        Summary::Good => (true, STATE_GOOD),
        Summary::Bad => (true, STATE_BAD),
        Summary::Unavailable => (false, 0),
    }
}

/// encode_summary -- the reading of a summary sensor, one encoder for both:
/// - FRU Health (Table 10.2.3-1): byte 4 bits 1:0 = 01b functioning properly, 10b not
///   functioning properly.
/// - FRU Voltage (Table 10.2.4-1): byte 4 bits 1:0 = 01b all monitored voltages within range,
///   10b at least one out of range.
///
/// An unavailable summary marks the reading unavailable. Writes `READING_LEN` bytes; returns the count.
fn encode_summary(summary: Summary, out: &mut [u8]) -> Result<usize, EncodeError> {
    let (available, state) = summary_state(summary);
    write_reading(out, available, READING_DISCRETE, state)
}

/// encode_temperature -- FRU Temperature reading (Table 10.2.5-1): byte 4 holds the
/// `TEMP_FLAG_*` bits. Unavailable when the die sensor gave no reading.
pub fn encode_temperature(snapshot: &SensorSnapshot, out: &mut [u8]) -> Result<usize, EncodeError> {
    let t = &snapshot.temperature;
    write_reading(out, t.available, READING_DISCRETE, t.flags)
}

/// LocalStatus -- IPMB local status code of one bus (Table 10.2.2-1 byte 2, 3 bits per bus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalStatus {
    /// No fault.
    None,
    /// The IPMC cannot drive the clock high.
    ClockStuckLow,
    /// The IPMC cannot drive the data line high.
    DataStuckLow,
    /// The IPMC cannot drive the clock low.
    ClockStuckHigh,
    /// The IPMC cannot drive the data line low.
    DataStuckHigh,
    /// The clock stayed low past the timeout.
    ClockLowTimeout,
    /// The bus is under test.
    UnderTest,
    /// A fault with no diagnosis.
    Undiagnosed,
}

impl LocalStatus {
    /// code -- the 3-bit value of Table 10.2.2-1.
    pub const fn code(self) -> u8 {
        match self {
            LocalStatus::None => 0,
            LocalStatus::ClockStuckLow => 1,
            LocalStatus::DataStuckLow => 2,
            LocalStatus::ClockStuckHigh => 3,
            LocalStatus::DataStuckHigh => 4,
            LocalStatus::ClockLowTimeout => 5,
            LocalStatus::UnderTest => 6,
            LocalStatus::Undiagnosed => 7,
        }
    }

    /// name -- text for console lines.
    pub const fn name(self) -> &'static str {
        match self {
            LocalStatus::None => "none",
            LocalStatus::ClockStuckLow => "cannot drive clock high",
            LocalStatus::DataStuckLow => "cannot drive data high",
            LocalStatus::ClockStuckHigh => "cannot drive clock low",
            LocalStatus::DataStuckHigh => "cannot drive data low",
            LocalStatus::ClockLowTimeout => "clock low timeout",
            LocalStatus::UnderTest => "under test",
            LocalStatus::Undiagnosed => "undiagnosed",
        }
    }
}

/// LinkBus -- one bus as the IPMB Link sensor reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkBus {
    /// true = Local Control (the IPMC decides), false = Override (isolated).
    pub local_control: bool,
    /// Local fault status.
    pub status: LocalStatus,
    /// The bus is connected and in use.
    pub enabled: bool,
}

/// LinkSensor -- both buses as plain data; `link::Link::sensor_data` fills it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSensor {
    /// IPMB-A.
    pub a: LinkBus,
    /// IPMB-B.
    pub b: LinkBus,
}

/// Table 10.2.2-1 byte 2: IPMB-B override bit 7 and status bits 6:4; IPMB-A override bit 3 and
/// status bits 2:0.
const LINK_B_OVERRIDE_BIT: u8 = 1 << 7;
const LINK_B_STATUS_SHIFT: u8 = 4;
const LINK_A_OVERRIDE_BIT: u8 = 1 << 3;
/// Byte 4 bits 3:0: A en/B en, A dis/B en, A en/B dis, A dis/B dis. The 400 kHz bits 5:4 stay 0:
/// the IPMB runs at IPMB_I2C_HZ = 100 kHz.
const LINK_STATE_BOTH_ENABLED: u8 = 1 << 3;
const LINK_STATE_ONLY_B: u8 = 1 << 2;
const LINK_STATE_ONLY_A: u8 = 1 << 1;
const LINK_STATE_NONE: u8 = 1 << 0;

/// encode_link -- IPMB Link sensor reading (sensor 1, type F1h, Table 10.2.2-1): byte 2 holds
/// the override and local status bits of both buses, byte 4 the link state. Always available.
/// Writes `READING_LEN` bytes; the table's optional byte 5 is not written.
pub fn encode_link(link: &LinkSensor, out: &mut [u8]) -> Result<usize, EncodeError> {
    let a = (u8::from(link.a.local_control) * LINK_A_OVERRIDE_BIT) | link.a.status.code();
    let b = (u8::from(link.b.local_control) * LINK_B_OVERRIDE_BIT)
        | (link.b.status.code() << LINK_B_STATUS_SHIFT);
    let state = match (link.a.enabled, link.b.enabled) {
        (true, true) => LINK_STATE_BOTH_ENABLED,
        (false, true) => LINK_STATE_ONLY_B,
        (true, false) => LINK_STATE_ONLY_A,
        (false, false) => LINK_STATE_NONE,
    };
    write_reading(out, true, a | b, state)
}

/// encode_fru_state -- FRU State reading (sensor 0, Table 10.2.1-1): byte 2 is 00h, byte 4 the
/// Current State Mask, bit n set for state Mn (bits 3:2 unused, written 0). `mask` comes from
/// `fru_state::State::mask`. Writes `READING_LEN` bytes.
pub fn encode_fru_state(mask: u8, out: &mut [u8]) -> Result<usize, EncodeError> {
    write_reading(out, true, READING_DISCRETE, mask)
}

/// encode_payload_test_results -- Payload Test Results reading (sensor 5, Table 10.2.6-1): no
/// payload test has run on this IPMC, so the reading/state unavailable bit is set and the test
/// result byte is ignored on read. Writes `READING_LEN` bytes.
pub fn encode_payload_test_results(out: &mut [u8]) -> Result<usize, EncodeError> {
    write_reading(out, false, READING_DISCRETE, 0)
}

/// encode_payload_test_status -- Payload Test Status reading (sensor 6, Table 10.2.7-1): byte 4
/// bits 1:0 = 10b test in progress, 01b not in progress. Writes `READING_LEN` bytes.
pub fn encode_payload_test_status(in_progress: bool, out: &mut [u8]) -> Result<usize, EncodeError> {
    let state = if in_progress { STATE_BAD } else { STATE_GOOD };
    write_reading(out, true, READING_DISCRETE, state)
}

/// encode_sensor_reading -- Get Sensor Reading response data for `sensor_number` (the bytes
/// after the completion code). `Err(UnsupportedSensor)` for a sensor this board does not feed.
pub fn encode_sensor_reading(
    sensor_number: u8,
    snapshot: &SensorSnapshot,
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    match sensor_number {
        SENSOR_FRU_HEALTH => encode_summary(snapshot.health, out),
        SENSOR_FRU_VOLTAGE => encode_summary(snapshot.voltage, out),
        SENSOR_FRU_TEMPERATURE => encode_temperature(snapshot, out),
        _ => Err(EncodeError::UnsupportedSensor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(health: Summary, voltage: Summary, available: bool, flags: u8) -> SensorSnapshot {
        let mut s = SensorSnapshot::EMPTY;
        s.health = health;
        s.voltage = voltage;
        s.temperature = TemperatureReading {
            available,
            dc: 250,
            flags,
        };
        s
    }

    fn encoded(sensor: u8, s: &SensorSnapshot) -> [u8; READING_LEN] {
        let mut out = [0xEE; READING_LEN];
        assert_eq!(encode_sensor_reading(sensor, s, &mut out), Ok(READING_LEN));
        out
    }

    #[test]
    fn sensor_numbers_match_table_10_2_1() {
        assert_eq!(SENSOR_FRU_STATE, 0);
        assert_eq!(SENSOR_SYSTEM_IPMB_LINK, 1);
        assert_eq!(SENSOR_FRU_HEALTH, 2);
        assert_eq!(SENSOR_FRU_VOLTAGE, 3);
        assert_eq!(SENSOR_FRU_TEMPERATURE, 4);
        assert_eq!(SENSOR_PAYLOAD_TEST_RESULTS, 5);
        assert_eq!(SENSOR_PAYLOAD_TEST_STATUS, 6);
        assert_eq!(SENSOR_TYPE_FRU_STATE, 0xF0);
        assert_eq!(SENSOR_TYPE_FRU_HEALTH, 0xF2);
        assert_eq!(SENSOR_TYPE_FRU_VOLTAGE, 0x02);
        assert_eq!(SENSOR_TYPE_FRU_TEMPERATURE, 0xF3);
    }

    #[test]
    fn health_readings_are_00_40_01_good_and_00_40_02_bad() {
        let cases = [
            ("health good", Summary::Good, [0x00, 0x40, 0x01]),
            ("health bad", Summary::Bad, [0x00, 0x40, 0x02]),
        ];
        for (name, health, want) in cases {
            let s = snapshot(health, Summary::Good, true, 0);
            assert_eq!(encoded(SENSOR_FRU_HEALTH, &s), want, "{name}");
        }
    }

    #[test]
    fn voltage_in_range_is_00_40_01_and_out_of_range_00_40_02() {
        let good = snapshot(Summary::Good, Summary::Good, true, 0);
        let bad = snapshot(Summary::Good, Summary::Bad, true, 0);
        assert_eq!(encoded(SENSOR_FRU_VOLTAGE, &good), [0x00, 0x40, 0x01]);
        assert_eq!(encoded(SENSOR_FRU_VOLTAGE, &bad), [0x00, 0x40, 0x02]);
    }

    #[test]
    fn unavailable_summary_sets_status_bit_5_and_clears_state() {
        let s = snapshot(Summary::Unavailable, Summary::Unavailable, true, 0);
        assert_eq!(encoded(SENSOR_FRU_HEALTH, &s), [0x00, 0x60, 0x00]);
        assert_eq!(encoded(SENSOR_FRU_VOLTAGE, &s), [0x00, 0x60, 0x00]);
    }

    #[test]
    fn temperature_bits_follow_table_10_2_5_1() {
        // bit0 LNC, bit1 LC, bit2 LNR, bit3 UNC, bit4 UC, bit5 UNR.
        for (flag, byte4) in [
            (TEMP_FLAG_LNC, 0x01),
            (TEMP_FLAG_LC, 0x02),
            (TEMP_FLAG_LNR, 0x04),
            (TEMP_FLAG_UNC, 0x08),
            (TEMP_FLAG_UC, 0x10),
            (TEMP_FLAG_UNR, 0x20),
        ] {
            let s = snapshot(Summary::Good, Summary::Good, true, flag);
            assert_eq!(encoded(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x40, byte4]);
        }
        // Over 85 C: UNC and UC together, 18h.
        let s = snapshot(
            Summary::Good,
            Summary::Good,
            true,
            TEMP_FLAG_UNC | TEMP_FLAG_UC,
        );
        assert_eq!(encoded(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x40, 0x18]);
        let none = snapshot(Summary::Good, Summary::Good, true, 0);
        assert_eq!(encoded(SENSOR_FRU_TEMPERATURE, &none), [0x00, 0x40, 0x00]);
        // No die reading: status bit 5 set and the state byte cleared, whatever the flags say.
        let missing = snapshot(Summary::Good, Summary::Good, false, TEMP_FLAG_UC);
        assert_eq!(
            encoded(SENSOR_FRU_TEMPERATURE, &missing),
            [0x00, 0x60, 0x00],
            "unavailable temperature"
        );
    }

    #[test]
    fn apply_event_enable_sets_and_clears_status_bit_7() {
        let mut out = [0x00, 0x40, 0x01];
        apply_event_enable(&mut out, true);
        assert_eq!(out, [0x00, 0xC0, 0x01]);
        apply_event_enable(&mut out, false);
        assert_eq!(out, [0x00, 0x40, 0x01]);
        let mut empty: [u8; 0] = [];
        apply_event_enable(&mut empty, true);
    }

    #[test]
    fn short_buffer_and_other_sensors_are_rejected() {
        let s = SensorSnapshot::EMPTY;
        let mut short = [0u8; READING_LEN - 1];
        assert_eq!(
            encode_sensor_reading(SENSOR_FRU_HEALTH, &s, &mut short),
            Err(EncodeError::BufferTooShort)
        );
        let mut out = [0u8; 8];
        for other in [0u8, 1, 5, 6, 7, 0xFF] {
            assert_eq!(
                encode_sensor_reading(other, &s, &mut out),
                Err(EncodeError::UnsupportedSensor)
            );
        }
    }

    #[test]
    fn fru_state_payload_test_readings_follow_tables_10_2_1_1_to_10_2_7_1() {
        let mut out = [0xEE; READING_LEN];
        assert_eq!(encode_fru_state(0x10, &mut out), Ok(READING_LEN));
        assert_eq!(out, [0x00, 0x40, 0x10]);
        assert_eq!(encode_payload_test_results(&mut out), Ok(READING_LEN));
        assert_eq!(out, [0x00, 0x60, 0x00]);
        assert_eq!(encode_payload_test_status(false, &mut out), Ok(READING_LEN));
        assert_eq!(out, [0x00, 0x40, 0x01]);
        assert_eq!(encode_payload_test_status(true, &mut out), Ok(READING_LEN));
        assert_eq!(out, [0x00, 0x40, 0x02]);
        let mut short = [0u8; READING_LEN - 1];
        assert_eq!(
            encode_fru_state(1, &mut short),
            Err(EncodeError::BufferTooShort)
        );
    }

    fn bus(local_control: bool, status: LocalStatus, enabled: bool) -> LinkBus {
        LinkBus {
            local_control,
            status,
            enabled,
        }
    }

    fn link_bytes(a: LinkBus, b: LinkBus) -> [u8; READING_LEN] {
        let mut out = [0xEE; READING_LEN];
        assert_eq!(encode_link(&LinkSensor { a, b }, &mut out), Ok(READING_LEN));
        out
    }

    #[test]
    fn link_sensor_bytes_follow_table_10_2_2_1() {
        let ok = LocalStatus::None;
        // Power-up: A local control and enabled, B isolated: bit 3, A en / B dis.
        assert_eq!(
            link_bytes(bus(true, ok, true), bus(false, ok, false)),
            [0x08, 0x40, 0x02]
        );
        // Both local control and enabled: bits 7 and 3, A en / B en.
        assert_eq!(
            link_bytes(bus(true, ok, true), bus(true, ok, true)),
            [0x88, 0x40, 0x08]
        );
        // B clock low timeout (5) and disabled by the fault: 0x80 | 5 << 4 | 0x08.
        assert_eq!(
            link_bytes(
                bus(true, ok, true),
                bus(true, LocalStatus::ClockLowTimeout, false)
            ),
            [0xD8, 0x40, 0x02]
        );
        // A undiagnosed (7) and isolated, B local control: A dis / B en.
        assert_eq!(
            link_bytes(
                bus(false, LocalStatus::Undiagnosed, false),
                bus(true, ok, true)
            ),
            [0x87, 0x40, 0x04]
        );
        // Both isolated: A dis / B dis. The status codes 1..=4 and 6 sit in their fields.
        assert_eq!(
            link_bytes(
                bus(false, LocalStatus::UnderTest, false),
                bus(false, LocalStatus::DataStuckHigh, false)
            ),
            [0x46, 0x40, 0x01]
        );
        assert_eq!(LocalStatus::ClockStuckLow.code(), 1);
        assert_eq!(LocalStatus::DataStuckLow.code(), 2);
        assert_eq!(LocalStatus::ClockStuckHigh.code(), 3);
    }

    #[test]
    fn link_sensor_rejects_a_short_buffer() {
        let ok = bus(true, LocalStatus::None, true);
        let mut short = [0u8; READING_LEN - 1];
        assert_eq!(
            encode_link(&LinkSensor { a: ok, b: ok }, &mut short),
            Err(EncodeError::BufferTooShort)
        );
    }

    #[test]
    fn valid_rails_follows_rail_count() {
        let mut s = SensorSnapshot::EMPTY;
        assert!(s.valid_rails().is_empty());
        s.rail_count = 2;
        assert_eq!(s.valid_rails().len(), 2);
        s.rail_count = RAILS_MAX + 1;
        assert!(s.valid_rails().is_empty());
    }
}
