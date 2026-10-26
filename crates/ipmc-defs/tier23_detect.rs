// tier23_detect.rs - the Tier-2/3 detection rules of build.rs as pure functions (std only).
//
// build.rs includes this file with `#[path]`; tests/tier23.rs
// includes it so the rules run under `cargo test`. The file system and the environment stay in
// build.rs: it reads the variable and checks the two marker files, and these functions decide.
//
// IPMC_TIER23 selects the extension: unset = on when both marker files exist, 0 = off whatever
// exists, 1 = on and a missing marker fails the build; any other value fails the build.

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
