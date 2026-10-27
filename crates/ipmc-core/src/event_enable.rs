//! event_enable.rs - the per-sensor event enables of Context (IPMI v2.0 Set Sensor Event Enable).
//!
//! One `SensorEvents` per mandatory sensor 0..=6 (and sensor 7, Payload Mode, at Tier-3), and one per
//! analog sensor (numbers from `analog::ANALOG_FIRST`; events off, masks 0000h: they send none). They start from the SDR table of event masks
//! (`sdr::event_masks`), the same values the records carry, so the SDR and the enables cannot
//! disagree at power-up. Writers: the Tier-3 Set Sensor Event Enable handler (extension). Readers:
//! Get Sensor Reading (status byte bit 7), and the Tier-2/3 event generator before it sends.

use crate::analog::ANALOG_FIRST;
use crate::sdr::{event_masks, mandatory_count, SENSOR_COUNT_MAX};
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

/// EventEnables -- the enables of the Tier's sensors, 0..`mandatory_count(tier)` (Table 10.2-1),
/// and of the analog sensors, `ANALOG_FIRST` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventEnables {
    sensors: [SensorEvents; SENSOR_COUNT_MAX],
    /// The mandatory sensors this Tier has.
    count: usize,
    /// The analog sensors, numbers `ANALOG_FIRST..ANALOG_FIRST + analog`.
    analog: usize,
}

impl EventEnables {
    /// power_up -- the power-up enables of this build: Tier-1 all off with zero masks; Tier-2/3
    /// every sensor on with the masks of its SDR record.
    pub const fn power_up() -> EventEnables {
        EventEnables::power_up_at(ipmc_defs::IPMC_TIER)
    }

    /// power_up_at -- `power_up` as a Tier-`tier` IPMC (`power_up` passes the build's Tier).
    pub const fn power_up_at(tier: u32) -> EventEnables {
        EventEnables::power_up_with_analog(tier, 0)
    }

    /// power_up_with_analog -- `power_up_at` with `analog` analog sensors (at most
    /// `ANALOG_COUNT_MAX`) that send no events: enabled off, masks 0000h.
    pub const fn power_up_with_analog(tier: u32, analog: usize) -> EventEnables {
        let mut all = [SensorEvents {
            enabled: false,
            assertion: 0,
            deassertion: 0,
        }; SENSOR_COUNT_MAX];
        let count = mandatory_count(tier);
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
            analog,
        }
    }

    /// slot -- the table index of `sensor` when the Tier or the analog sensors have it.
    fn slot(&self, sensor: u8) -> Option<usize> {
        let index = usize::from(sensor);
        let analog = usize::from(ANALOG_FIRST);
        if index < self.count || (index >= analog && index < analog + self.analog) {
            Some(index)
        } else {
            None
        }
    }

    /// get -- the enables of `sensor`; None for a number the Tier does not have.
    pub fn get(&self, sensor: u8) -> Option<&SensorEvents> {
        self.sensors.get(self.slot(sensor)?)
    }

    /// get_mut -- the enables of `sensor`, writable; None for a number the Tier does not have.
    pub fn get_mut(&mut self, sensor: u8) -> Option<&mut SensorEvents> {
        let slot = self.slot(sensor)?;
        self.sensors.get_mut(slot)
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

#[cfg(test)]
mod analog_tests {
    use super::*;
    use crate::analog::ANALOG_FIRST;

    #[test]
    fn analog_sensors_have_events_off_and_masks_zero_from_their_first_number() {
        let off = SensorEvents {
            enabled: false,
            assertion: 0,
            deassertion: 0,
        };
        for tier in [1u32, 2, 3] {
            let mut e = EventEnables::power_up_with_analog(tier, 6);
            for n in 0..6u8 {
                assert_eq!(
                    e.get(ANALOG_FIRST + n),
                    Some(&off),
                    "tier {tier} analog {n}"
                );
                assert!(e.get_mut(ANALOG_FIRST + n).is_some());
            }
            // The number after the last analog sensor does not exist, and below Tier-3 neither
            // does sensor 7, which the analog numbers do not shift.
            assert!(e.get(ANALOG_FIRST + 6).is_none());
            assert_eq!(e.get(7).is_some(), tier == 3, "tier {tier} sensor 7");
        }
        // Without analog sensors the numbers are absent (the default Context).
        assert!(EventEnables::power_up_at(3).get(ANALOG_FIRST).is_none());
    }
}
