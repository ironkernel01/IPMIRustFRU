// build.rs - the one detection point of the Tier-2/3 extension and of the evaluation build. It
// writes no file and reads one line of src/defs.rs, `IPMC_TIER23` (whether the extension is built
// in decides a cfg, which must be known before the crate compiles); the rest of the configuration
// is the Rust module src/defs.rs.
//
// The ipmc-ext bridge crate reads the result as DEP_IPMC_DEFS_TIER23 and the firmware crate as
// DEP_IPMC_DEFS_EVAL (through dep_cfg.rs). The sibling folder IPMIRustFRU_Tier23 holds
// defs_tier23.rs and crates/ipmc-tier23/src/lib.rs. With IPMC_TIER23 true the extension is built:
// `cfg(tier23)` is set and `cargo:tier23=1` is published (src/lib.rs then mounts defs_tier23.rs);
// a missing file fails the build. The rules live in tier23_detect.rs.
// Environment:
//   IPMC_TIER23   unset = the defs.rs value; 0 or 1 overrides it for this build (the tools that
//                 build Tier-1 and Tier-2/3 in one run), with a warning.
//   IPMC_EVAL     unset or 0 = off, 1 = evaluation build (the IPMB stops after
//                 IPMC_EVAL_MINUTES, a define of defs_tier23.rs): needs the extension and
//                 --release. The firmware crate reads the result as DEP_IPMC_DEFS_EVAL; no
//                 other build carries evaluation code.
//   IPMC_FIELD    unset or 0 = bench image, 1 = field image (cfg(ipmc_field): the boot
//                 countdown is IPMC_BOOT_COUNTDOWN_FIELD). Needs no extension, any profile.

use std::env;
use std::path::PathBuf;

#[path = "tier23_detect.rs"]
mod tier23_detect;

/// Tier23 sibling folder, relative to this crate's manifest folder.
const TIER23_ROOT: &str = "../../../IPMIRustFRU_Tier23";
/// The two files that mark a Tier23 checkout, relative to its root.
const TIER23_DEFS: &str = "defs_tier23.rs";
const TIER23_LIB: &str = "crates/ipmc-tier23/src/lib.rs";

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=IPMC_TIER23");
    println!("cargo:rerun-if-env-changed=IPMC_EVAL");
    println!("cargo:rerun-if-env-changed=IPMC_FIELD");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=tier23_detect.rs");
    println!("cargo:rerun-if-changed=src/defs.rs");

    let defs_path = manifest.join("src/defs.rs");
    let defs = std::fs::read_to_string(&defs_path)
        .unwrap_or_else(|e| panic!("{}: {e}", defs_path.display()));
    let in_defs = tier23_detect::parse_defs(&defs).unwrap_or_else(|e| panic!("{e}"));
    let overridden = tier23_detect::parse_override(env::var("IPMC_TIER23").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    if let Some(value) = overridden {
        println!(
            "cargo:warning=IPMC_TIER23={} from the environment overrides defs.rs ({in_defs})",
            u8::from(value)
        );
    }
    let setting = overridden.unwrap_or(in_defs);
    let root = manifest.join(TIER23_ROOT);
    let (tier23_defs, tier23_lib) = (root.join(TIER23_DEFS), root.join(TIER23_LIB));
    let (defs_text, lib_text) = (
        tier23_defs.display().to_string(),
        tier23_lib.display().to_string(),
    );
    // Off skips the file system: a missing watched path would rerun this script, and rebuild
    // the whole workspace, on every cargo call.
    let on = setting
        && tier23_detect::decide(
            setting,
            [
                tier23_detect::Marker {
                    path: &defs_text,
                    present: tier23_defs.is_file(),
                },
                tier23_detect::Marker {
                    path: &lib_text,
                    present: tier23_lib.is_file(),
                },
            ],
        )
        .unwrap_or_else(|e| panic!("{e}"));

    if on {
        println!("cargo:rerun-if-changed={defs_text}");
        println!("cargo:rerun-if-changed={lib_text}");
        println!("cargo:rustc-cfg=tier23");
        println!("cargo:tier23=1");
        println!(
            "cargo:warning=Tier-2/3 extension built from {}",
            root.display()
        );
    } else {
        println!("cargo:tier23=0");
    }
    let eval = tier23_detect::parse_eval(env::var("IPMC_EVAL").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    let profile = env::var("PROFILE").unwrap_or_default();
    tier23_detect::check_eval(eval, on, &profile).unwrap_or_else(|e| panic!("{e}"));
    if eval {
        println!("cargo:rustc-cfg=ipmc_eval");
        println!("cargo:eval=1");
        println!("cargo:warning=EVALUATION build");
    } else {
        println!("cargo:eval=0");
    }
    let field = tier23_detect::parse_field(env::var("IPMC_FIELD").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    if field {
        println!("cargo:rustc-cfg=ipmc_field");
        println!("cargo:field=1");
        println!("cargo:warning=FIELD build");
    } else {
        println!("cargo:field=0");
    }
}
