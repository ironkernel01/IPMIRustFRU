//! faults.rs - digital input definitions and the one evaluation of the inputs into an asserted
//! bitmap and the health contribution.
//!
//! An input is asserted when its pin level equals its active level. An input that could not be
//! read is neither asserted nor clear: it is flagged in `unreadable`, and an unreadable input
//! that makes health bad makes health bad. The table of definitions lives in
//! `board_sensors.rs`; bit n of every bitmap is entry n of that table.

use crate::hal::{DigitalInputs, Input};

/// InputDef -- one digital input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputDef {
    pub name: &'static str,
    pub input: Input,
    /// The pin level, high or low, at which the input is asserted.
    pub active_high: bool,
    /// An asserted input makes FRU Health bad.
    pub makes_health_bad: bool,
}

/// FaultEval -- the result of one pass over the inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultEval {
    /// Bit n set: input n is asserted.
    pub asserted: u32,
    /// Bit n set: input n could not be read; its asserted bit stays clear.
    pub unreadable: u32,
    /// An input with `makes_health_bad` is asserted or could not be read (fail-safe).
    pub health_bad: bool,
}

impl FaultEval {
    /// input_state -- Some(asserted) for `input`, or None when it is not in `defs` or could not
    /// be read.
    pub fn input_state(&self, defs: &[InputDef], input: Input) -> Option<bool> {
        let index = defs.iter().position(|def| def.input == input)?;
        let bit = bit_of(index)?;
        if self.unreadable & bit != 0 {
            None
        } else {
            Some(self.asserted & bit != 0)
        }
    }
}

/// bit_of -- the bitmap bit of table index `index`; None past 32 entries.
fn bit_of(index: usize) -> Option<u32> {
    u32::try_from(index)
        .ok()
        .and_then(|shift| 1u32.checked_shl(shift))
}

/// evaluate -- reads every input of `defs` once and builds the bitmap. Entries past 32 are
/// ignored (the board table has four).
pub fn evaluate<I: DigitalInputs + ?Sized>(defs: &[InputDef], inputs: &mut I) -> FaultEval {
    let mut eval = FaultEval {
        asserted: 0,
        unreadable: 0,
        health_bad: false,
    };
    for (index, def) in defs.iter().enumerate() {
        let Some(bit) = bit_of(index) else { break };
        match inputs.level_high(def.input) {
            Ok(high) => {
                if high == def.active_high {
                    eval.asserted |= bit;
                    eval.health_bad |= def.makes_health_bad;
                }
            }
            Err(_) => {
                eval.unreadable |= bit;
                // Fail-safe: a fault input that cannot be read cannot vouch for "no fault".
                eval.health_bad |= def.makes_health_bad;
            }
        }
    }
    eval
}

/// find_by_name -- the table index of the input called `name`, ignoring case and underscores
/// (`faultn` finds FAULT_N). None when no input matches.
pub fn find_by_name(defs: &[InputDef], name: &str) -> Option<usize> {
    defs.iter().position(|def| names_match(def.name, name))
}

fn names_match(table_name: &str, typed: &str) -> bool {
    let mut a = table_name.bytes().filter(|&b| b != b'_');
    let mut b = typed.bytes().filter(|&b| b != b'_');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x.eq_ignore_ascii_case(&y) => {}
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sim::SimInputs;

    const DEFS: [InputDef; 4] = [
        InputDef {
            name: "PGOOD",
            input: Input::Pgood,
            active_high: true,
            makes_health_bad: false,
        },
        InputDef {
            name: "FAULT_N",
            input: Input::FaultN,
            active_high: false,
            makes_health_bad: true,
        },
        InputDef {
            name: "OVERTEMP_N",
            input: Input::OvertempN,
            active_high: false,
            makes_health_bad: true,
        },
        InputDef {
            name: "PAYLOAD_RESET_N",
            input: Input::PayloadResetN,
            active_high: false,
            makes_health_bad: false,
        },
    ];

    #[test]
    fn all_pins_high_asserts_only_pgood() {
        let mut sim = SimInputs::new();
        let eval = evaluate(&DEFS, &mut sim);
        assert_eq!(eval.asserted, 0b0001);
        assert_eq!(eval.unreadable, 0);
        assert!(!eval.health_bad);
        assert_eq!(eval.input_state(&DEFS, Input::Pgood), Some(true));
        assert_eq!(eval.input_state(&DEFS, Input::FaultN), Some(false));
    }

    /// Pulled -- one input pulled low on the simulator and what `evaluate` must report.
    struct Pulled {
        name: &'static str,
        input: Input,
        asserted: u32,
        health_bad: bool,
        pgood_asserted: bool,
    }

    #[test]
    fn a_pulled_low_input_follows_its_polarity_and_health_flag() {
        let cases = [
            Pulled {
                name: "active-low FAULT_N asserts when pulled low and makes health bad",
                input: Input::FaultN,
                asserted: 0b0011,
                health_bad: true,
                pgood_asserted: true,
            },
            Pulled {
                name: "the reset input is reported but does not touch health",
                input: Input::PayloadResetN,
                asserted: 0b1001,
                health_bad: false,
                pgood_asserted: true,
            },
            Pulled {
                name: "lost PGOOD clears its bit",
                input: Input::Pgood,
                asserted: 0,
                health_bad: false,
                pgood_asserted: false,
            },
        ];
        for case in cases {
            let mut sim = SimInputs::new();
            sim.set_level(case.input, false);
            let eval = evaluate(&DEFS, &mut sim);
            assert_eq!(eval.asserted, case.asserted, "{}", case.name);
            assert_eq!(eval.unreadable, 0, "{}", case.name);
            assert_eq!(eval.health_bad, case.health_bad, "{}", case.name);
            assert_eq!(
                eval.input_state(&DEFS, Input::Pgood),
                Some(case.pgood_asserted),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn an_unreadable_input_is_flagged_and_never_asserted() {
        // (name, the unreadable input, its bit, whether health goes bad)
        let cases = [
            // OVERTEMP_N makes health bad: unreadable is not "no fault".
            (
                "OVERTEMP_N makes health bad",
                Input::OvertempN,
                0b0100,
                true,
            ),
            (
                "the reset input leaves health good",
                Input::PayloadResetN,
                0b1000,
                false,
            ),
        ];
        for (name, dead, bit, health_bad) in cases {
            let eval = evaluate(
                &DEFS,
                &mut crate::hal::digital::DeadInput {
                    dead,
                    error: crate::hal::SourceError::Unavailable,
                },
            );
            assert_eq!(eval.unreadable, bit, "{name}");
            assert_eq!(eval.asserted, 0b0001, "{name}");
            assert_eq!(eval.input_state(&DEFS, dead), None, "{name}");
            assert_eq!(eval.health_bad, health_bad, "{name}");
        }
    }

    #[test]
    fn names_match_ignoring_case_and_underscores() {
        assert_eq!(find_by_name(&DEFS, "faultn"), Some(1));
        assert_eq!(find_by_name(&DEFS, "FAULT_N"), Some(1));
        assert_eq!(find_by_name(&DEFS, "Overtemp_N"), Some(2));
        assert_eq!(find_by_name(&DEFS, "payloadresetn"), Some(3));
        assert_eq!(find_by_name(&DEFS, "pgood"), Some(0));
        assert_eq!(find_by_name(&DEFS, "fault"), None);
        assert_eq!(find_by_name(&DEFS, ""), None);
    }
}
