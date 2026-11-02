// tier23_detect.rs - the Tier-2/3 detection rules of build.rs as pure functions (std only).
//
// build.rs includes this file with `#[path]`; tests/tier23.rs
// includes it so the rules run under `cargo test`. The file system and the environment stay in
// build.rs: it reads src/defs.rs, the variable and the two marker files, and these functions decide.
//
// The setting is `IPMC_TIER23` in src/defs.rs (true = build the extension). The one line is read
// here because a cfg must be known before the crate compiles; nothing else in defs.rs is read.
// The environment variable IPMC_TIER23 (0 or 1) overrides it for one build: the tools that build
// Tier-1 and Tier-2/3 in one run use it, and build.rs prints a warning whenever it is set.

/// DEFS_LINE -- the start of the one line of defs.rs that holds the Tier-2/3 setting.
pub const DEFS_LINE: &str = "pub const IPMC_TIER23: bool =";

/// parse_defs -- the value of `IPMC_TIER23` in the text of defs.rs. Err when the line is missing,
/// written twice, or its value is not `true;` or `false;`.
pub fn parse_defs(text: &str) -> Result<bool, String> {
    let mut lines = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix(DEFS_LINE));
    let value = match (lines.next(), lines.next()) {
        (Some(value), None) => value.trim(),
        (None, _) => return Err(format!("defs.rs has no line `{DEFS_LINE} true|false;`")),
        (Some(_), Some(_)) => return Err(format!("defs.rs has `{DEFS_LINE}` more than once")),
    };
    match value {
        "true;" => Ok(true),
        "false;" => Ok(false),
        other => Err(format!(
            "defs.rs IPMC_TIER23 is {other:?}: it must be `true;` or `false;`"
        )),
    }
}

/// parse_override -- the IPMC_TIER23 environment override (None = unset, the defs.rs value
/// holds): Some(false) for 0, Some(true) for 1. Err names the variable and the value for any other
/// text, the empty string included.
pub fn parse_override(value: Option<&str>) -> Result<Option<bool>, String> {
    match value {
        None => Ok(None),
        Some("0") => Ok(Some(false)),
        Some("1") => Ok(Some(true)),
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

/// TIER1_HINT -- how to make the public Tier-1 build, for the error of a missing Tier23 folder.
pub const TIER1_HINT: &str =
    "the public Tier-1 build sets IPMC_TIER23, IPMC_TIER2 and IPMC_TIER3 to false in defs.rs";

/// decide -- whether the Tier-2/3 extension is built, for the setting `on` and the two markers.
/// Err: `on` with a marker missing (names its path and how to make the Tier-1 build). `on` false
/// builds no extension whatever exists.
pub fn decide(on: bool, markers: [Marker; 2]) -> Result<bool, String> {
    if !on {
        return Ok(false);
    }
    match markers.iter().find(|m| !m.present) {
        Some(missing) => Err(format!(
            "IPMC_TIER23 is true but {} is missing; {TIER1_HINT}",
            missing.path
        )),
        None => Ok(true),
    }
}

/// parse_switch -- whether the environment switch `name` is on (None = unset): unset or 0 = off,
/// 1 = on. Err names the variable and the value for any other text, the empty string included.
/// The one rule of IPMC_EVAL and IPMC_FIELD.
pub fn parse_switch(name: &str, value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(format!("{name} is {other:?}: it must be unset, 0 or 1")),
    }
}

/// parse_eval -- whether IPMC_EVAL selects the evaluation build (None = unset).
pub fn parse_eval(value: Option<&str>) -> Result<bool, String> {
    parse_switch("IPMC_EVAL", value)
}

/// parse_field -- whether IPMC_FIELD selects the field image (None = unset).
pub fn parse_field(value: Option<&str>) -> Result<bool, String> {
    parse_switch("IPMC_FIELD", value)
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
