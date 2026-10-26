//! tests/tier23.rs - runs the fixture tests of tier23_detect.rs, the rules build.rs uses.
//!
//! The file is included, not copied: the build and its tests share one set of rules.

#[path = "../tier23_detect.rs"]
mod tier23_detect;

use tier23_detect::{check_eval, decide, parse_eval, parse_mode, Marker, Mode};

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
fn mode_parses_unset_zero_one_and_rejects_the_rest() {
    assert_eq!(parse_mode(None), Ok(Mode::Auto));
    assert_eq!(parse_mode(Some("0")), Ok(Mode::Off));
    assert_eq!(parse_mode(Some("1")), Ok(Mode::On));
    for bad in ["", "2", "yes", "01", " 1"] {
        let err = parse_mode(Some(bad)).unwrap_err();
        assert!(err.contains("IPMC_TIER23"), "{err}");
        assert!(err.contains(&format!("{bad:?}")), "{err}");
    }
}

/// Decision -- one (mode, header present, lib present) case and its result.
struct Decision {
    mode: Mode,
    header: bool,
    lib: bool,
    want: Result<bool, &'static str>,
}

#[test]
fn decide_follows_the_mode_and_the_markers() {
    use Mode::{Auto, Off, On};
    let table = [
        Decision {
            mode: Auto,
            header: true,
            lib: true,
            want: Ok(true),
        },
        Decision {
            mode: Auto,
            header: false,
            lib: false,
            want: Ok(false),
        },
        Decision {
            mode: Auto,
            header: true,
            lib: false,
            want: Err("half a Tier23 checkout: T/defs_tier23.rs found, T/lib.rs missing"),
        },
        Decision {
            mode: Auto,
            header: false,
            lib: true,
            want: Err("half a Tier23 checkout: T/lib.rs found, T/defs_tier23.rs missing"),
        },
        Decision {
            mode: Off,
            header: true,
            lib: true,
            want: Ok(false),
        },
        Decision {
            mode: Off,
            header: true,
            lib: false,
            want: Ok(false),
        },
        Decision {
            mode: On,
            header: true,
            lib: true,
            want: Ok(true),
        },
        Decision {
            mode: On,
            header: false,
            lib: true,
            want: Err("T/defs_tier23.rs is missing"),
        },
        Decision {
            mode: On,
            header: true,
            lib: false,
            want: Err("T/lib.rs is missing"),
        },
    ];
    for case in table {
        let got = decide(case.mode, markers(case.header, case.lib));
        match case.want {
            Ok(on) => assert_eq!(got, Ok(on), "{:?} {} {}", case.mode, case.header, case.lib),
            Err(part) => {
                let err = got.unwrap_err();
                assert!(err.contains(part), "{err} lacks {part}");
            }
        }
    }
}

#[test]
fn eval_parses_unset_zero_one_and_rejects_the_rest() {
    assert_eq!(parse_eval(None), Ok(false));
    assert_eq!(parse_eval(Some("0")), Ok(false));
    assert_eq!(parse_eval(Some("1")), Ok(true));
    for bad in ["", "2", "yes", "01", " 1"] {
        let err = parse_eval(Some(bad)).unwrap_err();
        assert!(
            err.contains("IPMC_EVAL") && err.contains(&format!("{bad:?}")),
            "{err}"
        );
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
