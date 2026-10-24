//! event_enable.rs - the per-sensor event enables of Context (IPMI v2.0 Set Sensor Event Enable).
//!
//! One `SensorEvents` per mandatory sensor 0..=6. They start from the SDR table of event masks
//! (`sdr::event_masks`), the same values the records carry, so the SDR and the enables cannot
//! disagree at power-up. Writers: the Tier-3 Set Sensor Event Enable handler (extension). Readers:
//! Get Sensor Reading (status byte bit 7), and the Tier-2/3 event generator before it sends.

use crate::sdr::{event_masks, SENSOR_COUNT};
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

/// EventEnables -- the enables of sensors 0..SENSOR_COUNT (Table 10.2-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventEnables([SensorEvents; SENSOR_COUNT]);

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
        }; SENSOR_COUNT];
        let mut i = 0;
        while i < SENSOR_COUNT {
            if let Some((assertion, deassertion)) = event_masks(tier, i) {
                all[i] = SensorEvents {
                    enabled: tier::event_generator(tier),
                    assertion,
                    deassertion,
                };
            }
            i += 1;
        }
        EventEnables(all)
    }

    /// get -- the enables of `sensor`; None for a number outside 0..SENSOR_COUNT.
    pub fn get(&self, sensor: u8) -> Option<&SensorEvents> {
        self.0.get(usize::from(sensor))
    }

    /// get_mut -- the enables of `sensor`, writable; None for a number outside 0..SENSOR_COUNT.
    pub fn get_mut(&mut self, sensor: u8) -> Option<&mut SensorEvents> {
        self.0.get_mut(usize::from(sensor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_up_follows_the_tier_and_the_sdr_masks() {
        // (sensor, assertion, deassertion) at Tier-2/3; Tier-1 is all zero and off.
        let masks: [(u8, u16, u16); SENSOR_COUNT] = [
            (0, 0x0053, 0),
            (1, 0x000F, 0),
            (2, 0x0003, 0),
            (3, 0x0003, 0),
            (4, 0x0038, 0x0038),
            (5, 0, 0),
            (6, 0, 0),
        ];
        for tier in [1u32, 2, 3] {
            let e = EventEnables::power_up_at(tier);
            for (sensor, assertion, deassertion) in masks {
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
        assert!(e.get(SENSOR_COUNT as u8).is_none());
        assert!(e.get_mut(255).is_none());
        e.get_mut(2).unwrap().enabled = true;
        assert!(e.get(2).unwrap().enabled);
        assert!(!e.get(3).unwrap().enabled);
    }
}
