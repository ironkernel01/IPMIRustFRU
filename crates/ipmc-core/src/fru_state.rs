//! fru_state.rs - the Operational State machine of FRU 0 (VITA 46.11 section 4.1.5, Table
//! 4.1.5.1-1, Figure 4.1.5.4-1). Pure: no hardware, no clock; the caller passes the time and the
//! inputs, and the machine answers with the transition it made, if any.
//!
//! Owner decision 3 (PLAN.md 3.7): the FRU state follows payload power.
//!
//! ```text
//!        initial checks pass            payload power valid
//!   M0 ------------------------> M1 ---------------------------> M4 <---+
//!    ^   (Rule 4.1.5.4.1-2)      ^ ^                              |  |   |
//!    |                           | |   payload power lost or off  |  |   | no deactivation
//!    |                           | +------------ M6 <-------------+  |   | request, power
//!    |                           |   payload shut down             | M5 -+ valid (Tier-3)
//!    |                           |   (Rule 4.1.5.4.5-1)            |  |
//!    |                           |                    deactivation |  | payload power lost
//!    |                           |                    request      |  | or off (M5 -> M6)
//!    +---- management power lost, from any state     (M4 -> M5)    v  v
//! ```
//!
//! One transition per `step`. A Tier-1 IPMC reports the state only when it is polled with Get
//! Sensor Reading on sensor 0 (Rule 4.1.5.2: Tier-1 generates no events). M5 (FRU Deactivation
//! Request) is reachable only at Tier-3, where the IPMC controls payload power (`tier::
//! activation_control`): `Inputs::deactivation_requested` is false below Tier-3, so M4 never
//! leaves for M5 there. M7 (communication lost) is a state the Chassis Manager declares for an
//! IPMC, never one an IPMC puts its own FRU in (Observation 4.1.5.4.6-1), so it is not
//! reachable here.

use crate::tier;

/// State -- an Operational State of FRU 0 (VITA 46.11 Table 4.1.5.1-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// M0: IPMC inactive, the initial criteria are not met.
    M0,
    /// M1: FRU inactive, payload power is off or not yet valid.
    M1,
    /// M4: FRU active, payload power is valid.
    M4,
    /// M5: FRU deactivation request (Tier-3): the deactivation criteria are met or requested and
    /// the payload still runs (Rule 4.1.5.4.3-2).
    M5,
    /// M6: FRU deactivation in progress, payload power has been lost or switched off.
    M6,
}

impl State {
    /// ALL -- every state an IPMC of any Tier can be in (M7 is never reachable, see the file-top
    /// comment); `reachable_at` says which ones a Tier uses.
    pub const ALL: [State; 5] = [State::M0, State::M1, State::M4, State::M5, State::M6];

    /// reachable_at -- the state can be entered by an IPMC of Tier `tier`: M5 only when
    /// `tier::activation_control(tier)`.
    pub const fn reachable_at(self, tier: u32) -> bool {
        match self {
            State::M5 => tier::activation_control(tier),
            _ => true,
        }
    }

    /// all_mask_at -- the OR of `mask()` over the states reachable at `tier`: every Current State
    /// Mask bit Get Sensor Reading on sensor 0 can return at that Tier. The SDR reading mask and
    /// the event masks of the FRU State sensor are this value.
    pub const fn all_mask_at(tier: u32) -> u8 {
        let mut mask = 0;
        let mut i = 0;
        while i < State::ALL.len() {
            if State::ALL[i].reachable_at(tier) {
                mask |= State::ALL[i].mask();
            }
            i += 1;
        }
        mask
    }

    /// all_mask -- `all_mask_at` the build's Tier.
    pub const fn all_mask() -> u8 {
        State::all_mask_at(ipmc_defs::IPMC_TIER)
    }

    /// number -- the n of Mn.
    pub const fn number(self) -> u8 {
        match self {
            State::M0 => 0,
            State::M1 => 1,
            State::M4 => 4,
            State::M5 => 5,
            State::M6 => 6,
        }
    }

    /// mask -- the Current State Mask of Get Sensor Reading on sensor 0 (Table 10.2.1-1 byte 4):
    /// bit n is set for state Mn.
    pub const fn mask(self) -> u8 {
        1 << self.number()
    }

    /// name -- the state's name in Table 4.1.5.1-1, for console lines.
    pub const fn name(self) -> &'static str {
        match self {
            State::M0 => "M0 IPMC Inactive",
            State::M1 => "M1 FRU Inactive",
            State::M4 => "M4 FRU Active",
            State::M5 => "M5 FRU Deactivation Request",
            State::M6 => "M6 FRU Deactivation In Progress",
        }
    }
}

/// InitialChecks -- the minimum FRU initial criteria of Rule 4.1.5.4.1-2, one flag each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialChecks {
    /// Management Power Availability: management power is enabled and valid (Rule 4.1.5.4.1-3).
    pub management_power_valid: bool,
    /// Hardware Address Integrity: the GA[4:0] and GAP strap has odd parity (Rule 4.1.5.4.1-4).
    pub address_parity_valid: bool,
    /// Local SDR Repository Check: the Device SDR repository is built (Rule 4.1.5.4.1-8).
    pub sdr_repository_ready: bool,
    /// IPMC Diagnostics Status: the self test passed (Rule 4.1.5.4.1-9).
    pub self_test_passed: bool,
}

impl InitialChecks {
    /// passed -- every check holds.
    pub const fn passed(&self) -> bool {
        self.management_power_valid
            && self.address_parity_valid
            && self.sdr_repository_ready
            && self.self_test_passed
    }
}

/// Inputs -- what one `step` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The initial criteria.
    pub checks: InitialChecks,
    /// Payload power is valid: the power state machine is On, which means PGOOD was seen.
    pub payload_power_valid: bool,
    /// Payload power is completely shut down: PWR_EN is released and PGOOD was read deasserted
    /// (`SensorSnapshot::payload_shut_down`). It ends M6.
    pub payload_shut_down: bool,
    /// A deactivation is requested: the Tier-3 deactivation criteria are met, or Set FRU
    /// Activation or Set FRU State Policy Bits asked for it (Rules 4.1.5.4.3-2, -6). It moves M4
    /// to M5 and holds M5; clearing it moves M5 back to M4. False below Tier-3.
    pub deactivation_requested: bool,
}

/// Transition -- one state change `step` made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    /// State before.
    pub from: State,
    /// State after.
    pub to: State,
}

/// PowerOnRefused -- why a payload power-on request is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerOnRefused {
    /// The FRU is in M6: deactivation is in progress (Rule 4.1.5.4.5-2).
    DeactivationInProgress,
    /// The extension refuses payload power: the FRU is Activation-Locked, so the payload is not
    /// powered by any path until it is activated (Rule 4.1.5.4.2-1; Tier-3).
    ActivationLocked,
}

impl PowerOnRefused {
    /// name -- text for console lines.
    pub const fn name(self) -> &'static str {
        match self {
            PowerOnRefused::DeactivationInProgress => {
                "FRU deactivation in progress (M6), power on after the FRU reaches M1"
            }
            PowerOnRefused::ActivationLocked => {
                "FRU is activation-locked (AL), activate the FRU or clear the lock first"
            }
        }
    }
}

/// FruMachine -- the state of FRU 0 and the time it entered it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FruMachine {
    state: State,
    since_ms: u32,
}

impl FruMachine {
    /// new -- a machine in M0, as after power-up (Rule 4.1.5.4.1-1: M1 only when the initial
    /// criteria are met).
    pub const fn new() -> FruMachine {
        FruMachine {
            state: State::M0,
            since_ms: 0,
        }
    }

    /// state -- the current state.
    pub const fn state(&self) -> State {
        self.state
    }

    /// since_ms -- the `now_ms` of the step that entered the current state (0 for the power-up M0).
    pub const fn since_ms(&self) -> u32 {
        self.since_ms
    }

    /// check_power_on -- whether a payload power-on request may proceed now. Refused in M6: the
    /// deactivation runs to a complete shutdown and M1 first (Rule 4.1.5.4.5-1), as an Activate
    /// request in M6 is refused with D5h (Rule 4.1.5.4.5-2; owner decision 10-23-2026). Every
    /// power-on request (console POWER ON, a later Set FRU Activation) checks here.
    pub const fn check_power_on(&self) -> Result<(), PowerOnRefused> {
        match self.state {
            State::M6 => Err(PowerOnRefused::DeactivationInProgress),
            _ => Ok(()),
        }
    }

    /// step -- applies `inputs` at `now_ms` and returns the transition made, if any.
    /// 1. Management power lost: M0 from M1, M4 and M6 (Figure 4.1.5.4-1).
    /// 2. M0 -> M1 when every initial check passes (Rule 4.1.5.4.1-2).
    /// 3. M1 -> M4 when payload power is valid (Rule 4.1.5.4.2-3).
    /// 4. M4 -> M6 when payload power is lost or switched off; else M4 -> M5 when
    ///    `deactivation_requested` (Rule 4.1.5.4.3-2, -6).
    /// 5. M5 -> M6 when payload power is lost or switched off (Rule 4.1.5.4.4-5, as M4 -> M6);
    ///    else M5 -> M4 when `deactivation_requested` is cleared (Rule 4.1.5.4.4-2, -4).
    /// 6. M6 -> M1 once the payload power is completely shut down (`payload_shut_down`: PWR_EN
    ///    released and PGOOD read deasserted, Rule 4.1.5.4.5-1). M6 stays while the shutdown is
    ///    not complete, whatever `payload_power_valid` says: power that returns during M6 does
    ///    not cancel the deactivation.
    pub fn step(&mut self, now_ms: u32, inputs: &Inputs) -> Option<Transition> {
        let next = if !inputs.checks.management_power_valid {
            State::M0
        } else {
            match self.state {
                State::M0 if inputs.checks.passed() => State::M1,
                State::M1 if inputs.payload_power_valid => State::M4,
                State::M4 | State::M5 if !inputs.payload_power_valid => State::M6,
                State::M4 if inputs.deactivation_requested => State::M5,
                State::M5 if !inputs.deactivation_requested => State::M4,
                State::M6 if inputs.payload_shut_down => State::M1,
                other => other,
            }
        };
        if next == self.state {
            return None;
        }
        let transition = Transition {
            from: self.state,
            to: next,
        };
        self.state = next;
        self.since_ms = now_ms;
        Some(transition)
    }
}

impl Default for FruMachine {
    fn default() -> Self {
        FruMachine::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: InitialChecks = InitialChecks {
        management_power_valid: true,
        address_parity_valid: true,
        sdr_repository_ready: true,
        self_test_passed: true,
    };

    /// Inputs with the payload shut down exactly when its power is not valid (the usual pair).
    fn inputs(checks: InitialChecks, payload_power_valid: bool) -> Inputs {
        Inputs {
            checks,
            payload_power_valid,
            payload_shut_down: !payload_power_valid,
            deactivation_requested: false,
        }
    }

    fn machine_in(state: State) -> FruMachine {
        FruMachine { state, since_ms: 0 }
    }

    #[test]
    fn power_on_is_refused_only_in_m6() {
        for state in State::ALL {
            let expected = if state == State::M6 {
                Err(PowerOnRefused::DeactivationInProgress)
            } else {
                Ok(())
            };
            assert_eq!(machine_in(state).check_power_on(), expected, "{state:?}");
        }
    }

    #[test]
    fn power_on_is_allowed_again_once_m6_reaches_m1() {
        let mut m = machine_in(State::M6);
        assert!(m.check_power_on().is_err());
        let t = m.step(10, &inputs(GOOD, false)).unwrap();
        assert_eq!((t.from, t.to), (State::M6, State::M1));
        assert_eq!(m.check_power_on(), Ok(()));
    }

    #[test]
    fn power_up_state_is_m0() {
        let m = FruMachine::new();
        assert_eq!((m.state(), m.since_ms()), (State::M0, 0));
        assert_eq!(FruMachine::default(), m);
    }

    #[test]
    fn masks_follow_table_10_2_1_1() {
        assert_eq!(State::M0.mask(), 0x01);
        assert_eq!(State::M1.mask(), 0x02);
        assert_eq!(State::M4.mask(), 0x10);
        assert_eq!(State::M5.mask(), 0x20);
        assert_eq!(State::M6.mask(), 0x40);
        assert_eq!(State::M4.number(), 4);
        assert_eq!(State::M5.name(), "M5 FRU Deactivation Request");
        assert_eq!(State::M6.name(), "M6 FRU Deactivation In Progress");
    }

    #[test]
    fn m0_waits_for_every_initial_check() {
        let each_missing = [
            InitialChecks {
                management_power_valid: false,
                ..GOOD
            },
            InitialChecks {
                address_parity_valid: false,
                ..GOOD
            },
            InitialChecks {
                sdr_repository_ready: false,
                ..GOOD
            },
            InitialChecks {
                self_test_passed: false,
                ..GOOD
            },
        ];
        for checks in each_missing {
            let mut m = FruMachine::new();
            // Payload power valid changes nothing in M0.
            assert_eq!(m.step(5, &inputs(checks, true)), None);
            assert_eq!(m.state(), State::M0);
        }
    }

    #[test]
    fn m0_to_m1_is_one_transition_per_step() {
        let mut m = FruMachine::new();
        // Payload power is already valid, yet the machine takes M0 -> M1 first.
        assert_eq!(
            m.step(1000, &inputs(GOOD, true)),
            Some(Transition {
                from: State::M0,
                to: State::M1
            })
        );
        assert_eq!((m.state(), m.since_ms()), (State::M1, 1000));
        assert_eq!(
            m.step(2000, &inputs(GOOD, true)),
            Some(Transition {
                from: State::M1,
                to: State::M4
            })
        );
        assert_eq!((m.state(), m.since_ms()), (State::M4, 2000));
    }

    #[test]
    fn m1_and_m4_follow_payload_power() {
        // (name, state, power valid, the state it moves to, or None when it holds)
        let cases = [
            ("M1 waits for payload power", State::M1, false, None),
            (
                "M1 to M4 when payload power is valid",
                State::M1,
                true,
                Some(State::M4),
            ),
            ("M4 holds while power is valid", State::M4, true, None),
            (
                "M4 to M6 when payload power is lost",
                State::M4,
                false,
                Some(State::M6),
            ),
        ];
        for (name, state, power_valid, moves_to) in cases {
            let mut m = machine_in(state);
            let t = m.step(10, &inputs(GOOD, power_valid));
            match moves_to {
                Some(to) => {
                    assert_eq!(t, Some(Transition { from: state, to }), "{name}");
                    assert_eq!((m.state(), m.since_ms()), (to, 10), "{name}");
                }
                None => {
                    assert_eq!(t, None, "{name}");
                    assert_eq!(m.state(), state, "{name}");
                    assert_eq!(
                        m.since_ms(),
                        0,
                        "{name}: no transition leaves the entry time alone"
                    );
                }
            }
        }
    }

    #[test]
    fn m6_stays_until_the_payload_is_completely_shut_down() {
        let mut m = machine_in(State::M6);
        let not_yet = Inputs {
            checks: GOOD,
            payload_power_valid: false,
            payload_shut_down: false,
            deactivation_requested: false,
        };
        // Power is already invalid, but PWR_EN or PGOOD says the shutdown is not complete.
        assert_eq!(m.step(200, &not_yet), None);
        assert_eq!(m.step(300, &not_yet), None);
        assert_eq!((m.state(), m.since_ms()), (State::M6, 0));
        // The shutdown completes: M6 -> M1.
        let t = m.step(400, &inputs(GOOD, false)).unwrap();
        assert_eq!((t.from, t.to), (State::M6, State::M1));
        assert_eq!((m.state(), m.since_ms()), (State::M1, 400));
        // Power returns: back to M4.
        let t = m.step(500, &inputs(GOOD, true)).unwrap();
        assert_eq!((t.from, t.to), (State::M1, State::M4));
    }

    #[test]
    fn power_that_returns_in_m6_does_not_cancel_the_deactivation() {
        let mut m = machine_in(State::M6);
        let returned = Inputs {
            checks: GOOD,
            payload_power_valid: true,
            payload_shut_down: false,
            deactivation_requested: false,
        };
        assert_eq!(m.step(10, &returned), None);
        assert_eq!(m.state(), State::M6);
        // Once shut down is reported (power cycled off), M6 completes.
        let shut = Inputs {
            payload_shut_down: true,
            ..returned
        };
        let t = m.step(20, &shut).unwrap();
        assert_eq!((t.from, t.to), (State::M6, State::M1));
    }

    #[test]
    fn all_lists_the_reachable_states_and_their_masks() {
        assert_eq!(
            State::ALL,
            [State::M0, State::M1, State::M4, State::M5, State::M6]
        );
        // Table 10.2.1-1 byte 4: bit 0 M0, bit 1 M1, bit 4 M4, bit 5 M5 (Tier-3), bit 6 M6.
        // (tier, mask of the reachable states)
        for (tier, mask) in [(1, 0x53), (2, 0x53), (3, 0x73)] {
            assert_eq!(State::all_mask_at(tier), mask, "tier {tier}");
            assert_eq!(State::M5.reachable_at(tier), tier == 3, "tier {tier}");
            assert!(State::M4.reachable_at(tier));
        }
        assert_eq!(State::all_mask(), State::all_mask_at(ipmc_defs::IPMC_TIER));
    }

    #[test]
    fn lost_management_power_returns_every_state_to_m0() {
        let lost = InitialChecks {
            management_power_valid: false,
            ..GOOD
        };
        for state in [State::M1, State::M4, State::M5, State::M6] {
            let mut m = machine_in(state);
            let t = m.step(77, &inputs(lost, true)).unwrap();
            assert_eq!((t.from, t.to), (state, State::M0));
            assert_eq!((m.state(), m.since_ms()), (State::M0, 77));
        }
    }

    /// Inputs with the deactivation request and payload power given (payload shut down exactly
    /// when its power is not valid).
    fn with_request(power_valid: bool, requested: bool) -> Inputs {
        Inputs {
            deactivation_requested: requested,
            ..inputs(GOOD, power_valid)
        }
    }

    #[test]
    fn m4_and_m5_follow_the_deactivation_request_and_payload_power() {
        // (name, state, power valid, request, the state it moves to or None when it holds)
        let cases = [
            (
                "M4 to M5 on a request",
                State::M4,
                true,
                true,
                Some(State::M5),
            ),
            ("M4 holds without a request", State::M4, true, false, None),
            ("M5 holds while requested", State::M5, true, true, None),
            (
                "M5 to M4 when the request clears",
                State::M5,
                true,
                false,
                Some(State::M4),
            ),
            (
                "M5 to M6 on power loss, request held",
                State::M5,
                false,
                true,
                Some(State::M6),
            ),
            (
                "M5 to M6 on power loss, request cleared",
                State::M5,
                false,
                false,
                Some(State::M6),
            ),
            (
                "M4 to M6 on power loss wins over a request",
                State::M4,
                false,
                true,
                Some(State::M6),
            ),
            ("M1 ignores a request", State::M1, false, true, None),
            ("M6 ignores a request", State::M6, true, true, None),
        ];
        for (name, state, power_valid, requested, moves_to) in cases {
            let mut m = machine_in(state);
            let t = m.step(10, &with_request(power_valid, requested));
            match moves_to {
                Some(to) => assert_eq!(t, Some(Transition { from: state, to }), "{name}"),
                None => assert_eq!(t, None, "{name}"),
            }
        }
    }

    #[test]
    fn a_machine_fed_tier_1_and_2_inputs_never_reaches_m5() {
        // The request input is false below Tier-3: every power sequence leaves M5 unvisited.
        let mut m = FruMachine::new();
        for (now, valid) in [
            (1, true),
            (2, true),
            (3, true),
            (4, false),
            (5, false),
            (6, true),
        ] {
            m.step(now, &inputs(GOOD, valid));
            assert_ne!(m.state(), State::M5);
        }
    }
}
