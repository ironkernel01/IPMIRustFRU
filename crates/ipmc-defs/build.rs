// build.rs - parses ipmi_defs.h into $OUT_DIR/ipmi_defs.rs (`pub const NAME: u32` per integer
// define, `pub const NAME: &str` per string define) and detects the Tier-2/3 extension.
//
// The parser and its rules live in defs_parser.rs (shared with the unit tests); a rejected
// header fails the build with `path:line: reason`.
//
// Tier-2/3 detection (the one detection point; the ipmc-ext bridge crate reads the result as
// DEP_IPMC_DEFS_TIER23): the sibling folder IPMIRustFRU_Tier23 holds ipmi_defs_tier23.h and
// crates/ipmc-tier23/src/lib.rs. When both exist the extension is built: its header is parsed
// into the same generated file, `cfg(tier23)` is set and `cargo:tier23=1` is published. The
// rules (environment, half a checkout, duplicate names) live in tier23_detect.rs.
// Environment:
//   IPMI_DEFS_H   overrides the header path (used to prove that a bad line fails the build).
//   IPMC_TIER23   unset = auto, 0 = off, 1 = on and a missing file fails the build.
//   IPMC_EVAL     unset or 0 = off, 1 = evaluation build (the IPMB stops after
//                 IPMC_EVAL_MINUTES): needs the extension and --release. The firmware crate reads
//                 the result as DEP_IPMC_DEFS_EVAL; no other build carries evaluation code.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "defs_parser.rs"]
mod defs_parser;
#[path = "tier23_detect.rs"]
mod tier23_detect;

/// Tier23 sibling folder, relative to this crate's manifest folder.
const TIER23_ROOT: &str = "../../../IPMIRustFRU_Tier23";
/// The two files that mark a Tier23 checkout, relative to its root.
const TIER23_HEADER: &str = "ipmi_defs_tier23.h";
const TIER23_LIB: &str = "crates/ipmc-tier23/src/lib.rs";

/// The Tier23 define that sets the evaluation length, for the build warning.
const EVAL_MINUTES_DEFINE: &str = "IPMC_EVAL_MINUTES";

/// parse_header -- reads and parses one header; a rejected header fails the build with
/// `path:line: reason`.
fn parse_header(header: &Path) -> Vec<defs_parser::Define> {
    let path = header.display().to_string();
    // Lossy decoding turns an invalid UTF-8 byte into U+FFFD, which the parser rejects with its
    // line number if it sits in a string literal.
    let bytes = fs::read(header).unwrap_or_else(|e| panic!("{path}: {e}"));
    let raw = String::from_utf8_lossy(&bytes);
    defs_parser::parse(&raw).unwrap_or_else(|e| panic!("{path}:{}: {}", e.line, e.why))
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=IPMI_DEFS_H");
    println!("cargo:rerun-if-env-changed=IPMC_TIER23");
    println!("cargo:rerun-if-env-changed=IPMC_EVAL");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=defs_parser.rs");
    println!("cargo:rerun-if-changed=tier23_detect.rs");
    let header = match env::var("IPMI_DEFS_H") {
        Ok(p) => PathBuf::from(p),
        Err(_) => manifest.join("../../ipmi_defs.h"),
    };
    println!("cargo:rerun-if-changed={}", header.display());
    let base = parse_header(&header);

    let mode = tier23_detect::parse_mode(env::var("IPMC_TIER23").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    let root = manifest.join(TIER23_ROOT);
    let (tier23_header, tier23_lib) = (root.join(TIER23_HEADER), root.join(TIER23_LIB));
    let (header_text, lib_text) = (
        tier23_header.display().to_string(),
        tier23_lib.display().to_string(),
    );
    // Off skips the file system: a missing watched path would rerun this script, and rebuild
    // the whole workspace, on every cargo call.
    let on = mode != tier23_detect::Mode::Off
        && tier23_detect::decide(
            mode,
            [
                tier23_detect::Marker {
                    path: &header_text,
                    present: tier23_header.is_file(),
                },
                tier23_detect::Marker {
                    path: &lib_text,
                    present: tier23_lib.is_file(),
                },
            ],
        )
        .unwrap_or_else(|e| panic!("{e}"));

    let extra = on.then(|| {
        println!("cargo:rerun-if-changed={header_text}");
        println!("cargo:rerun-if-changed={lib_text}");
        parse_header(&tier23_header)
    });
    let tier = extra.as_deref().and_then(tier23_detect::tier_of);
    let header_name = header.display().to_string();
    let defines = tier23_detect::merge(
        base,
        &header_name,
        extra.map(|defines| (defines, header_text.as_str())),
    )
    .unwrap_or_else(|e| panic!("{e}"));

    if let Some(tier) = tier {
        println!("cargo:rustc-cfg=tier23");
        println!("cargo:tier23=1");
        println!(
            "cargo:warning=Tier-2/3 extension built from {} (IPMC_TIER {tier})",
            root.display()
        );
        if tier23_detect::tier3_ipmb_a_only(Some(tier), &defines) {
            println!(
                "cargo:warning=Tier-3 DEVIATION (owner 10-24-2026): IPMB-A only, against VITA 46.11 Rule 4.1.1-1.1 and Rule 4.1.1-4.4 for IPMB-B ({} 1)",
                tier23_detect::TIER3_A_ONLY_DEFINE
            );
        }
    } else {
        println!("cargo:tier23=0");
    }
    let eval = tier23_detect::parse_eval(env::var("IPMC_EVAL").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    let profile = env::var("PROFILE").unwrap_or_default();
    tier23_detect::check_eval(eval, tier.is_some(), &profile).unwrap_or_else(|e| panic!("{e}"));
    if eval {
        let minutes = tier23_detect::int_of(&defines, EVAL_MINUTES_DEFINE).unwrap_or(0);
        println!("cargo:rustc-cfg=ipmc_eval");
        println!("cargo:eval=1");
        println!("cargo:warning=EVALUATION build: the IPMB stops after {minutes} minutes");
    } else {
        println!("cargo:eval=0");
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("ipmi_defs.rs"), defs_parser::render(&defines))
        .expect("write ipmi_defs.rs");
}
