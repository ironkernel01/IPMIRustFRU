// dep_cfg.rs - the one build-script step of a crate that takes a `cfg` from ipmc-defs.
//
// ipmc-defs is the one place that decides whether the Tier-2/3 extension and the evaluation
// build are on; it publishes each decision as `cargo:<key>=0|1` (links = "ipmc_defs"), which cargo
// hands to every direct dependent's build script as DEP_IPMC_DEFS_<KEY>. The build scripts of
// ipmc-ext and the firmware crate include this file with `#[path]` and call `cfg_from_defs`; they
// read no file and no other variable, so they cannot disagree with ipmc-defs.

use std::env;

/// cfg_from_defs -- prints `cargo:rustc-cfg=<cfg>` when `DEP_IPMC_DEFS_<key>` is `1`, and the
/// rerun triggers: the variable, the calling crate's build.rs and this file (`self_path`, the
/// `#[path]` the caller used, relative to its manifest folder).
pub fn cfg_from_defs(key: &str, cfg: &str, self_path: &str) {
    let var = format!("DEP_IPMC_DEFS_{key}");
    println!("cargo:rerun-if-env-changed={var}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={self_path}");
    if env::var(&var).as_deref() == Ok("1") {
        println!("cargo:rustc-cfg={cfg}");
    }
}
