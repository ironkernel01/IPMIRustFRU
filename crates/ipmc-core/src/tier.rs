//! tier.rs - the choices that follow from the Tier the IPMC reports (`ipmc_defs::IPMC_TIER`).
//!
//! The one place the Tier rules are derived: Get Device ID (identity.rs), the Device SDR records
//! (sdr.rs) and the event enables (event_enable.rs) all read these. Each rule is a `const fn` of
//! a tier argument, so a test checks all three Tiers with literal expectations while the build
//! runs at one; the consts apply the rule to the build's Tier.

/// event_generator -- Tier-2 and Tier-3 IPMCs generate events (VITA 46.11 Rule 7-3); a Tier-1
/// IPMC does not (Rule 7-4).
pub const fn event_generator(tier: u32) -> bool {
    tier >= 2
}

/// sel_device -- a Tier-3 IPMC keeps a SEL (Rule 7-8).
pub const fn sel_device(tier: u32) -> bool {
    tier == 3
}

/// per_state_event_enables -- a Tier-3 IPMC implements Set/Get Sensor Event Enable
/// (Table 10.1.1-1 M), so each sensor's events can be switched per state; Tier-2 has only the
/// global switch (Set Event Receiver FFh).
pub const fn per_state_event_enables(tier: u32) -> bool {
    tier == 3
}

/// EVENT_GENERATOR -- `event_generator` at the build's Tier.
pub const EVENT_GENERATOR: bool = event_generator(ipmc_defs::IPMC_TIER);
/// SEL_DEVICE -- `sel_device` at the build's Tier.
pub const SEL_DEVICE: bool = sel_device(ipmc_defs::IPMC_TIER);
/// PER_STATE_EVENT_ENABLES -- `per_state_event_enables` at the build's Tier.
pub const PER_STATE_EVENT_ENABLES: bool = per_state_event_enables(ipmc_defs::IPMC_TIER);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_follow_the_tier() {
        // (tier, event generator, SEL device, per-state event enables)
        let table = [
            (1, false, false, false),
            (2, true, false, false),
            (3, true, true, true),
        ];
        for (tier, generator, sel, per_state) in table {
            assert_eq!(event_generator(tier), generator, "tier {tier}");
            assert_eq!(sel_device(tier), sel, "tier {tier}");
            assert_eq!(per_state_event_enables(tier), per_state, "tier {tier}");
        }
        assert_eq!(EVENT_GENERATOR, event_generator(ipmc_defs::IPMC_TIER));
        assert_eq!(SEL_DEVICE, sel_device(ipmc_defs::IPMC_TIER));
        assert_eq!(
            PER_STATE_EVENT_ENABLES,
            per_state_event_enables(ipmc_defs::IPMC_TIER)
        );
    }
}
