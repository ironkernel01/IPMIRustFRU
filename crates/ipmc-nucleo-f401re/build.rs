// build.rs - the linker scripts of the two binaries and the cfg of the evaluation and field builds.
//
// Runtime (`ipmc-nucleo-f401re`): -Tlink.x, which includes memory.x, which includes flash_map.x.
// Bootloader (`ipmc-boot`): -Tboot.x, which includes flash_map.x. flash_map.x is the one place of
// every flash and RAM address; the crate root goes on the linker search path so both scripts find
// it with INCLUDE. Cargo reruns the link when a script changes.
//
// ipmc-defs is the one place that reads IPMC_EVAL and IPMC_FIELD and checks their conditions;
// dep_cfg.rs (shared with ipmc-ext's build script) turns its published decisions into the cfgs.
// This script reads no file of its own.

#[path = "../ipmc-defs/dep_cfg.rs"]
mod dep_cfg;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    println!("cargo:rustc-link-search={manifest_dir}");
    // Each linker script belongs to one binary.
    println!("cargo:rustc-link-arg-bin=ipmc-nucleo-f401re=-Tlink.x");
    println!("cargo:rustc-link-arg-bin=ipmc-boot=-Tboot.x");
    // Sector 0 holds 16 KB: the linker folds identical functions (memclr4/memclr8, memcpy4/memcpy8
    // and equal generic instances) into one. No code compares function addresses.
    println!("cargo:rustc-link-arg-bin=ipmc-boot=--icf=all");
    for script in ["link.x", "memory.x", "boot.x", "flash_map.x"] {
        println!("cargo:rerun-if-changed={script}");
    }
    dep_cfg::cfg_from_defs("EVAL", "ipmc_eval", "../ipmc-defs/dep_cfg.rs");
    dep_cfg::cfg_from_defs("FIELD", "ipmc_field", "../ipmc-defs/dep_cfg.rs");
}
