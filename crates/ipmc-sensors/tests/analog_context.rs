//! The analog sensors through the real `Context`: the SDR repository the firmware and the emulator
//! build, Get Sensor Reading of each analog number, and the numbers that do not exist.

use ipmc_core::addr;
use ipmc_core::analog::ANALOG_FIRST;
use ipmc_core::dispatch::{Context, SensorReadError};
use ipmc_core::sdr::{SdrRepository, NO_NEXT_RECORD};
use ipmc_core::sensor::{
    RailReading, RailStatus, SensorSnapshot, TemperatureReading, READING_LEN_MAX,
};
use ipmc_sensors::analog::{BOARD_ANALOG, SPECS};
use ipmc_sensors::board_sensors::RAILS;

const OWNER: u8 = 0x82;
/// Rails and the die temperature.
const ANALOG: usize = 6;

fn context() -> Context {
    Context::with_analog(addr::resolve(0x7F), &BOARD_ANALOG)
}

/// A snapshot with every rail at nominal and the die at 25.0 C.
fn nominal() -> SensorSnapshot {
    let mut s = SensorSnapshot::EMPTY;
    for (slot, def) in s.rails.iter_mut().zip(RAILS.iter()) {
        *slot = RailReading {
            mv: def.nominal_mv,
            status: RailStatus::InRange,
        };
        s.rail_count += 1;
    }
    s.temperature = TemperatureReading {
        available: true,
        dc: 250,
        flags: 0,
    };
    s
}

fn read(ctx: &Context, number: u8) -> Result<Vec<u8>, SensorReadError> {
    let mut out = [0u8; READING_LEN_MAX];
    let len = ctx.read_sensor(number, &ctx.link.buses, &mut out)?;
    Ok(out[..len].to_vec())
}

#[test]
fn record_and_sensor_counts_per_tier_and_the_walk_ends_at_ffff() {
    // (tier, mandatory sensors): sensors = mandatory + 6; records = sensors + the locator.
    for (tier, mandatory) in [(1u32, 7usize), (2, 7), (3, 8)] {
        let r = SdrRepository::with_analog(OWNER, tier, &BOARD_ANALOG);
        assert_eq!(r.sensor_count(), mandatory + ANALOG, "tier {tier}");
        assert_eq!(r.record_count(), mandatory + ANALOG + 1, "tier {tier}");
        let mut id = 0u16;
        let mut seen = 1usize;
        loop {
            let next = r.next_id(id);
            if next == NO_NEXT_RECORD {
                break;
            }
            assert_eq!(next, id + 1);
            id = next;
            seen += 1;
        }
        assert_eq!(seen, r.record_count(), "tier {tier}");
        assert_eq!(r.locator_record_id(), id);
        assert_eq!(r.record(id).unwrap()[3], 0x12, "the locator is last");
        // The analog records are Full records (01h) for sensors 08h.. with the board's names.
        for (n, spec) in SPECS.iter().enumerate() {
            let info = r.describe((mandatory + n) as u16).unwrap();
            assert_eq!(info.record_type, 0x01);
            let sensor = info.sensor.unwrap();
            assert_eq!(sensor.number, ANALOG_FIRST + n as u8);
            assert_eq!(sensor.event_type, 0x01);
            assert_eq!(info.name, spec.name.as_bytes());
        }
    }
}

#[test]
fn each_analog_number_reads_in_counts_and_the_next_one_is_absent() {
    let mut ctx = context();
    ctx.sensors = nominal();
    // The nominal counts of the record: 12V payload 96h, and so on; the temperature 25 (19h).
    let want: [u8; ANALOG] = [110, 150, 125, 110, 110, 25];
    for (n, value) in want.into_iter().enumerate() {
        let number = ANALOG_FIRST + n as u8;
        // Scanning enabled (bit 6), events off (bit 7 clear), no comparison bits.
        assert_eq!(
            read(&ctx, number),
            Ok(vec![value, 0x40, 0xC0]),
            "sensor {number}"
        );
    }
    assert_eq!(
        read(&ctx, ANALOG_FIRST + ANALOG as u8),
        Err(SensorReadError::NotPresent)
    );
    // The mandatory sensors still answer where they did.
    assert!(read(&ctx, 3).is_ok());
}

#[test]
fn before_the_first_sample_the_analog_sensors_are_unavailable() {
    let ctx = context();
    for n in 0..ANALOG as u8 {
        assert_eq!(
            read(&ctx, ANALOG_FIRST + n),
            Ok(vec![0, 0x60, 0xC0]),
            "sensor {n}"
        );
    }
}

#[test]
fn without_the_hook_the_analog_numbers_do_not_exist() {
    let mut ctx = Context::new(addr::resolve(0x7F));
    ctx.sensors = nominal();
    assert_eq!(read(&ctx, ANALOG_FIRST), Err(SensorReadError::NotPresent));
    assert_eq!(
        ctx.sdr.record_count(),
        ipmc_core::sdr::mandatory_count(ipmc_defs::IPMC_TIER) + 1
    );
}

#[test]
fn the_enables_of_the_analog_sensors_start_off() {
    let ctx = context();
    for n in 0..ANALOG as u8 {
        let e = ctx.event_enables.get(ANALOG_FIRST + n).unwrap();
        assert!(!e.enabled && e.assertion == 0 && e.deassertion == 0);
    }
}
