//! tests/tier23.rs - runs the fixture tests of tier23_detect.rs, the rules build.rs uses.
//!
//! The file is included, not copied: the build and its tests share one set of rules.

#[path = "../tier23_detect.rs"]
mod tier23_detect;

use tier23_detect::{
    check_eval, decide, parse_defs, parse_eval, parse_field, parse_override, Marker, DEFS_LINE,
};

fn markers(header: bool, lib: bool) -> [Marker<'static>; 2] {
    [
        Marker {
            path: "T/defs_tier23.rs",
            present: header,
        },
        Marker {
            path: "T/lib.rs",
            present: lib,
        },
    ]
}

#[test]
fn defs_line_reads_true_or_false_once() {
    let line = |v: &str| {
        format!(
            "/// doc
{DEFS_LINE} {v}
pub const IPMC_TIER2: bool = true;
"
        )
    };
    assert_eq!(parse_defs(&line("true;")), Ok(true));
    assert_eq!(parse_defs(&line("false;")), Ok(false));
    assert_eq!(parse_defs(&format!("    {DEFS_LINE} false;")), Ok(false));
    for (text, part) in [
        (line("1;"), "\"1;\""),
        (line("true"), "\"true\""),
        ("pub const IPMC_TIER2: bool = true;".to_string(), "no line"),
        (
            format!(
                "{0} true;
{0} false;",
                DEFS_LINE
            ),
            "more than once",
        ),
    ] {
        let err = parse_defs(&text).unwrap_err();
        assert!(err.contains(part), "{err} lacks {part}");
    }
    // The real defs.rs has the line, so the build reads it.
    assert!(parse_defs(include_str!("../src/defs.rs")).is_ok());
}

#[test]
fn override_parses_unset_zero_one_and_rejects_the_rest() {
    assert_eq!(parse_override(None), Ok(None));
    assert_eq!(parse_override(Some("0")), Ok(Some(false)));
    assert_eq!(parse_override(Some("1")), Ok(Some(true)));
    for bad in ["", "2", "yes", "01", " 1"] {
        let err = parse_override(Some(bad)).unwrap_err();
        assert!(err.contains("IPMC_TIER23"), "{err}");
        assert!(err.contains(&format!("{bad:?}")), "{err}");
    }
}

/// Decision -- one (setting, header present, lib present) case and its result.
struct Decision {
    on: bool,
    header: bool,
    lib: bool,
    want: Result<bool, &'static str>,
}

#[test]
fn decide_follows_the_setting_and_the_markers() {
    let case = |on, header, lib, want| Decision {
        on,
        header,
        lib,
        want,
    };
    let table = [
        case(false, true, true, Ok(false)),
        case(false, false, false, Ok(false)),
        case(false, true, false, Ok(false)),
        case(true, true, true, Ok(true)),
        case(true, false, true, Err("T/defs_tier23.rs is missing")),
        case(true, true, false, Err("T/lib.rs is missing")),
        case(
            true,
            false,
            false,
            Err("sets IPMC_TIER23, IPMC_TIER2 and IPMC_TIER3 to false"),
        ),
    ];
    for case in table {
        let got = decide(case.on, markers(case.header, case.lib));
        match case.want {
            Ok(on) => assert_eq!(got, Ok(on), "{} {} {}", case.on, case.header, case.lib),
            Err(part) => {
                let err = got.unwrap_err();
                assert!(err.contains(part), "{err} lacks {part}");
            }
        }
    }
}

#[test]
fn eval_needs_the_extension_and_the_release_profile() {
    // (eval, tier23, profile, expected error text)
    let cases = [
        (false, false, "debug", None),
        (false, true, "debug", None),
        (true, true, "release", None),
        (
            true,
            false,
            "release",
            Some("IPMC_EVAL=1 needs the Tier-2/3 extension"),
        ),
        (
            true,
            true,
            "debug",
            Some("IPMC_EVAL=1 builds only with --release"),
        ),
        (
            true,
            false,
            "debug",
            Some("IPMC_EVAL=1 needs the Tier-2/3 extension"),
        ),
    ];
    for (eval, tier23, profile, expected) in cases {
        let got = check_eval(eval, tier23, profile);
        assert_eq!(got.err().as_deref(), expected, "{eval} {tier23} {profile}");
    }
}

#[test]
fn eval_and_field_switches_share_one_rule() {
    type Parse = fn(Option<&str>) -> Result<bool, String>;
    let cases: [(&str, Parse); 2] = [("IPMC_EVAL", parse_eval), ("IPMC_FIELD", parse_field)];
    for (name, parse) in cases {
        assert_eq!(parse(None), Ok(false));
        assert_eq!(parse(Some("0")), Ok(false));
        assert_eq!(parse(Some("1")), Ok(true));
        for bad in ["", "2", "yes", "01", " 1"] {
            let err = parse(Some(bad)).unwrap_err();
            assert!(err.contains(name), "{err}");
            assert!(err.contains(&format!("{bad:?}")), "{err}");
        }
    }
}
