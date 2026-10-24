//! tests/tier23.rs - runs the fixture tests of tier23_detect.rs, the rules build.rs uses.
//!
//! The file is included, not copied: the build and its tests share one set of rules.

#[path = "../defs_parser.rs"]
mod defs_parser;
#[path = "../tier23_detect.rs"]
mod tier23_detect;

use defs_parser::{Define, Value};
use tier23_detect::{
    check_eval, decide, int_of, merge, parse_eval, parse_mode, tier3_ipmb_a_only, tier_of, Marker,
    Mode,
};

fn int(name: &str, value: u32) -> Define {
    Define {
        name: name.to_string(),
        value: Value::Int(value),
    }
}

fn markers(header: bool, lib: bool) -> [Marker<'static>; 2] {
    [
        Marker {
            path: "T/ipmi_defs_tier23.h",
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
            want: Err("half a Tier23 checkout: T/ipmi_defs_tier23.h found, T/lib.rs missing"),
        },
        Decision {
            mode: Auto,
            header: false,
            lib: true,
            want: Err("half a Tier23 checkout: T/lib.rs found, T/ipmi_defs_tier23.h missing"),
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
            want: Err("T/ipmi_defs_tier23.h is missing"),
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
fn merge_off_appends_tier_one() {
    let merged = merge(vec![int("A", 5)], "ipmi_defs.h", None).unwrap();
    assert_eq!(merged.len(), 2);
    assert_eq!(tier_of(&merged), Some(1));
}

#[test]
fn merge_on_holds_both_headers() {
    let extra = vec![int("IPMC_TIER", 2), int("B", 7)];
    let merged = merge(vec![int("A", 5)], "ipmi_defs.h", Some((extra, "t23.h"))).unwrap();
    let names: Vec<&str> = merged.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["A", "IPMC_TIER", "B"]);
    assert_eq!(tier_of(&merged), Some(2));
}

#[test]
fn merge_rejects_a_name_in_both_headers_naming_both_files() {
    let extra = vec![int("IPMC_TIER", 1), int("A", 7)];
    let err = merge(vec![int("A", 5)], "ipmi_defs.h", Some((extra, "t23.h"))).unwrap_err();
    assert!(
        err.contains("A") && err.contains("ipmi_defs.h") && err.contains("t23.h"),
        "{err}"
    );
}

#[test]
fn merge_requires_the_tier_in_the_tier23_header_only() {
    let missing = merge(
        vec![int("A", 5)],
        "ipmi_defs.h",
        Some((vec![int("B", 1)], "t23.h")),
    );
    assert!(missing.unwrap_err().contains("t23.h"));
    let text = Define {
        name: "IPMC_TIER".to_string(),
        value: Value::Text("1".to_string()),
    };
    assert!(merge(vec![], "ipmi_defs.h", Some((vec![text], "t23.h"))).is_err());
    let in_base = merge(vec![int("IPMC_TIER", 1)], "ipmi_defs.h", None);
    assert!(in_base.unwrap_err().contains("ipmi_defs.h"));
    let in_base_on = merge(
        vec![int("IPMC_TIER", 1)],
        "ipmi_defs.h",
        Some((vec![int("IPMC_TIER", 1)], "t23.h")),
    );
    assert!(in_base_on.unwrap_err().contains("ipmi_defs.h"));
}

#[test]
fn the_tier3_ipmb_a_only_deviation_needs_tier_3_bus_b_off_and_the_define() {
    let set = |bus_b: u32, a_only: u32| {
        vec![
            int("IPMB_BUS_B_ENABLE", bus_b),
            int("IPMC_TIER3_IPMB_A_ONLY", a_only),
        ]
    };
    assert_eq!(int_of(&set(0, 1), "IPMC_TIER3_IPMB_A_ONLY"), Some(1));
    assert_eq!(int_of(&set(0, 1), "NOPE"), None);
    assert!(tier3_ipmb_a_only(Some(3), &set(0, 1)));
    for (name, tier, defines) in [
        ("tier 2", Some(2), set(0, 1)),
        ("no extension", None, set(0, 1)),
        ("bus B built", Some(3), set(1, 1)),
        ("define 0", Some(3), set(0, 0)),
        ("no defines", Some(3), vec![]),
    ] {
        assert!(!tier3_ipmb_a_only(tier, &defines), "{name}");
    }
}

#[test]
fn generated_file_holds_the_defines_of_both_headers() {
    let header = |body: &str| format!("#ifndef IPMI_DEFS_H\n#define IPMI_DEFS_H\n{body}\n#endif\n");
    let base = defs_parser::parse(&header("#define A 5")).unwrap();
    let extra = defs_parser::parse(&header("#define IPMC_TIER 3\n#define B \"x\"")).unwrap();
    let on = render_of(base.clone(), Some(extra));
    assert!(on.contains("pub const A: u32 = 5;"), "{on}");
    assert!(on.contains("pub const IPMC_TIER: u32 = 3;"), "{on}");
    assert!(on.contains("pub const B: &str = \"x\";"), "{on}");
    let off = render_of(base, None);
    assert!(off.contains("pub const IPMC_TIER: u32 = 1;"), "{off}");
}

/// render_of -- the generated source for a merge of `base` and `extra`.
fn render_of(base: Vec<Define>, extra: Option<Vec<Define>>) -> String {
    let merged = merge(base, "ipmi_defs.h", extra.map(|d| (d, "t23.h"))).unwrap();
    defs_parser::render(&merged)
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
