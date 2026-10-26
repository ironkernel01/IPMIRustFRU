//! event_enable.rs - the per-sensor event enables of Context (IPMI v2.0 Set Sensor Event Enable).
//!
//! One `SensorEvents` per mandatory sensor 0..=6 (and sensor 7, Payload Mode, at Tier-3). They start from the SDR table of event masks
//! (`sdr::event_masks`), the same values the records carry, so the SDR and the enables cannot
//! disagree at power-up. Writers: the Tier-3 Set Sensor Event Enable handler (extension). Readers:
//! Get Sensor Reading (status byte bit 7), and the Tier-2/3 event generator before it sends.

use crate::sdr::{event_masks, sensor_count, SENSOR_COUNT_MAX};
use crate::tier;

/// SensorEvents -- the event enables of one sensor (IPMI v2.0 Set Sensor Event Enable): events
/// on/off for the whole sensor, and the assertion and deassertion masks (bit n = state offset n).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorEvents {
    /// Event messages enabled for the whole sensor.
    pub enabled: bool,
    /// Assertion events enabled, bit n = state offset n.
    pub assertion: u16,
    /// Deassertion events enabled, bit n = state offset n.
    pub deassertion: u16,
}

/// EventEnables -- the enables of the Tier's sensors, 0..`sensor_count(tier)` (Table 10.2-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventEnables {
    sensors: [SensorEvents; SENSOR_COUNT_MAX],
    /// The sensors this Tier has.
    count: usize,
}

impl EventEnables {
    /// power_up -- the power-up enables of this build: Tier-1 all off with zero masks; Tier-2/3
    /// every sensor on with the masks of its SDR record.
    pub const fn power_up() -> EventEnables {
        EventEnables::power_up_at(ipmc_defs::IPMC_TIER)
    }

    /// power_up_at -- `power_up` as a Tier-`tier` IPMC (`power_up` passes the build's Tier).
    pub const fn power_up_at(tier: u32) -> EventEnables {
        let mut all = [SensorEvents {
            enabled: false,
            assertion: 0,
            deassertion: 0,
        }; SENSOR_COUNT_MAX];
        let count = sensor_count(tier);
        let mut i = 0;
        while i < count {
            if let Some((assertion, deassertion)) = event_masks(tier, i) {
                all[i] = SensorEvents {
                    enabled: tier::event_generator(tier),
                    assertion,
                    deassertion,
                };
            }
            i += 1;
        }
        EventEnables {
            sensors: all,
            count,
        }
    }

    /// get -- the enables of `sensor`; None for a number the Tier does not have.
    pub fn get(&self, sensor: u8) -> Option<&SensorEvents> {
        self.sensors.get(..self.count)?.get(usize::from(sensor))
    }

    /// get_mut -- the enables of `sensor`, writable; None for a number the Tier does not have.
    pub fn get_mut(&mut self, sensor: u8) -> Option<&mut SensorEvents> {
        self.sensors
            .get_mut(..self.count)?
            .get_mut(usize::from(sensor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_up_follows_the_tier_and_the_sdr_masks() {
        // (sensor, assertion, deassertion) at Tier-2; Tier-1 is all zero and off. Tier-3 adds M5
        // to sensor 0, the 400 kHz states to sensor 1 (asserted and deasserted) and sensor 7.
        let masks: [(u8, u16, u16); 7] = [
            (0, 0x0053, 0),
            (1, 0x000F, 0),
            (2, 0x0003, 0),
            (3, 0x0003, 0),
            (4, 0x0038, 0x0038),
            (5, 0, 0),
            (6, 0, 0),
        ];
        let tier3: [(u8, u16, u16); 8] = [
            (0, 0x0073, 0),
            (1, 0x003F, 0x0030),
            (2, 0x0003, 0),
            (3, 0x0003, 0),
            (4, 0x0038, 0x0038),
            (5, 0, 0),
            (6, 0, 0),
            (7, ipmc_defs::IPMC_PAYLOAD_MODES, 0),
        ];
        for tier in [1u32, 2, 3] {
            let e = EventEnables::power_up_at(tier);
            let table: &[(u8, u16, u16)] = if tier == 3 { &tier3 } else { &masks };
            for &(sensor, assertion, deassertion) in table {
                let want = if tier == 1 {
                    SensorEvents {
                        enabled: false,
                        assertion: 0,
                        deassertion: 0,
                    }
                } else {
                    SensorEvents {
                        enabled: true,
                        assertion,
                        deassertion,
                    }
                };
                assert_eq!(e.get(sensor), Some(&want), "tier {tier} sensor {sensor}");
            }
        }
        assert_eq!(
            EventEnables::power_up(),
            EventEnables::power_up_at(ipmc_defs::IPMC_TIER)
        );
    }

    #[test]
    fn get_and_get_mut_reject_a_sensor_past_the_table() {
        let mut e = EventEnables::power_up_at(1);
        assert!(e.get(7).is_none(), "sensor 7 exists only at Tier-3");
        assert!(e.get_mut(7).is_none());
        assert!(EventEnables::power_up_at(3).get(7).is_some());
        assert!(EventEnables::power_up_at(3).get(8).is_none());
        assert!(e.get_mut(255).is_none());
        e.get_mut(2).unwrap().enabled = true;
        assert!(e.get(2).unwrap().enabled);
        assert!(!e.get(3).unwrap().enabled);
    }
}
