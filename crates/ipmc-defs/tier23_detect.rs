// tier23_detect.rs - the Tier-2/3 detection rules of build.rs as pure functions (std only).
//
// build.rs includes this file with `#[path]`, as it does defs_parser.rs; tests/tier23.rs
// includes it so the rules run under `cargo test`. The file system and the environment stay in
// build.rs: it reads the variable and checks the two marker files, and these functions decide.
//
// IPMC_TIER23 selects the extension: unset = on when both marker files exist, 0 = off whatever
// exists, 1 = on and a missing marker fails the build; any other value fails the build.

use crate::defs_parser::{Define, Value};

/// Name of the define that selects the Tier the IPMC reports (VITA 46.11 section 4).
pub const TIER_DEFINE: &str = "IPMC_TIER";
/// Name of the define that builds IPMB-B in (ipmi_defs.h).
pub const BUS_B_DEFINE: &str = "IPMB_BUS_B_ENABLE";
/// Name of the Tier23 define that allows a Tier-3 build on IPMB-A only.
pub const TIER3_A_ONLY_DEFINE: &str = "IPMC_TIER3_IPMB_A_ONLY";

/// Mode -- the setting of the IPMC_TIER23 environment variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Variable unset: on when both marker files exist, off when neither does.
    Auto,
    /// Variable 0: off whatever exists.
    Off,
    /// Variable 1: on; a missing marker fails the build.
    On,
}

/// parse_mode -- the Mode for the value of IPMC_TIER23 (None = unset).
/// Err names the variable and the value for any other text, the empty string included.
pub fn parse_mode(value: Option<&str>) -> Result<Mode, String> {
    match value {
        None => Ok(Mode::Auto),
        Some("0") => Ok(Mode::Off),
        Some("1") => Ok(Mode::On),
        Some(other) => Err(format!(
            "IPMC_TIER23 is {other:?}: it must be unset, 0 or 1"
        )),
    }
}

/// Marker -- one file whose presence marks a Tier23 checkout, with whether it exists.
#[derive(Clone, Copy, Debug)]
pub struct Marker<'a> {
    pub path: &'a str,
    pub present: bool,
}

/// decide -- whether the Tier-2/3 extension is built, for `mode` and the two markers.
/// Err: mode On with a marker missing (names its path); mode Auto with exactly one marker
/// present ("half a Tier23 checkout").
pub fn decide(mode: Mode, markers: [Marker; 2]) -> Result<bool, String> {
    let [first, second] = markers;
    match mode {
        Mode::Off => Ok(false),
        Mode::On => match markers.iter().find(|m| !m.present) {
            Some(missing) => Err(format!("IPMC_TIER23=1 but {} is missing", missing.path)),
            None => Ok(true),
        },
        Mode::Auto => match (first.present, second.present) {
            (true, true) => Ok(true),
            (false, false) => Ok(false),
            (true, false) => Err(half_checkout(first.path, second.path)),
            (false, true) => Err(half_checkout(second.path, first.path)),
        },
    }
}

fn half_checkout(present: &str, missing: &str) -> String {
    format!("half a Tier23 checkout: {present} found, {missing} missing")
}

/// merge -- the defines of the generated file.
/// Off: `base` followed by IPMC_TIER = 1 (the Tier-1 build reports Tier-1).
/// On: `base` followed by `extra`; Err when a name is in both headers (names both files),
/// when IPMC_TIER is in `base_file` (it belongs in the Tier23 header) or when `extra` lacks
/// it, or when IPMC_TIER is not an unsigned integer.
pub fn merge(
    base: Vec<Define>,
    base_file: &str,
    extra: Option<(Vec<Define>, &str)>,
) -> Result<Vec<Define>, String> {
    if base.iter().any(|d| d.name == TIER_DEFINE) {
        return Err(format!(
            "{base_file}: {TIER_DEFINE} belongs in the Tier23 header, not here"
        ));
    }
    let mut all = base;
    match extra {
        None => all.push(tier_define(1)),
        Some((defines, extra_file)) => {
            for d in &defines {
                if all.iter().any(|b| b.name == d.name) {
                    return Err(format!(
                        "{} is defined in both {base_file} and {extra_file}",
                        d.name
                    ));
                }
            }
            if !defines
                .iter()
                .any(|d| d.name == TIER_DEFINE && matches!(d.value, Value::Int(_)))
            {
                return Err(format!(
                    "{extra_file}: {TIER_DEFINE} must be defined as an unsigned integer"
                ));
            }
            all.extend(defines);
        }
    }
    Ok(all)
}

fn tier_define(tier: u32) -> Define {
    Define {
        name: TIER_DEFINE.to_string(),
        value: Value::Int(tier),
    }
}

/// int_of -- the value of the integer define `name` in `defines`, None when absent or not an
/// integer.
pub fn int_of(defines: &[Define], name: &str) -> Option<u32> {
    defines
        .iter()
        .find_map(|d| match (&d.value, d.name == name) {
            (Value::Int(v), true) => Some(*v),
            _ => None,
        })
}

/// tier_of -- the value of IPMC_TIER in `defines`, None when absent or not an integer.
pub fn tier_of(defines: &[Define]) -> Option<u32> {
    int_of(defines, TIER_DEFINE)
}

/// tier3_ipmb_a_only -- whether the build is the Tier-3 deviation of the owner (Tier23 decision
/// 4a): Tier 3 on IPMB-A only (IPMB_BUS_B_ENABLE 0), allowed by IPMC_TIER3_IPMB_A_ONLY 1 against
/// VITA 46.11 Rules 4.1.1-1.1 and 4.1.1-4.4. `defines` is the merged set of both headers.
pub fn tier3_ipmb_a_only(tier: Option<u32>, defines: &[Define]) -> bool {
    tier == Some(3)
        && int_of(defines, BUS_B_DEFINE) == Some(0)
        && int_of(defines, TIER3_A_ONLY_DEFINE) == Some(1)
}

/// parse_eval -- whether IPMC_EVAL selects the evaluation build (None = unset): unset or 0 =
/// off, 1 = on. Err names the variable and the value for any other text, the empty string
/// included.
pub fn parse_eval(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(format!("IPMC_EVAL is {other:?}: it must be unset, 0 or 1")),
    }
}

/// check_eval -- the build conditions of the evaluation build. `tier23` is whether the Tier-2/3
/// extension is built in, `profile` the PROFILE of the build script. Err for IPMC_EVAL=1 without
/// the extension, and for IPMC_EVAL=1 outside the release profile; always Ok when `eval` is false.
pub fn check_eval(eval: bool, tier23: bool, profile: &str) -> Result<(), String> {
    if !eval {
        Ok(())
    } else if !tier23 {
        Err("IPMC_EVAL=1 needs the Tier-2/3 extension".to_string())
    } else if profile != "release" {
        Err("IPMC_EVAL=1 builds only with --release".to_string())
    } else {
        Ok(())
    }
}
