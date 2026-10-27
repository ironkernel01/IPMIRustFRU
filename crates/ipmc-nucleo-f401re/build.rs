// build.rs - the linker scripts of the two binaries and the cfg of the evaluation and field builds.
//
// Runtime (`ipmc-nucleo-f401re`): -Tlink.x, which includes memory.x, which includes flash_map.x.
// Bootloader (`ipmc-boot`): -Tboot.x, which includes flash_map.x. flash_map.x is the one place of
// every flash and RAM address; the crate root goes on the linker search path so both scripts find
// it with INCLUDE. Cargo reruns the link when a script changes.
//
// ipmc-defs is the one place that reads IPMC_EVAL and IPMC_FIELD and checks their conditions;
// dep_cfg.rs (shared with ipmc-ext's build script) turns its published decisions into the cfgs.
// Its only file is defs.rs, for the two stack reservations (above).

#[path = "../ipmc-defs/dep_cfg.rs"]
mod dep_cfg;

// The stack reservations are defines of defs.rs (plain Rust, no `use`): the script reads them by
// mounting the file itself, so the linker fragments cannot drift from the configuration.
#[allow(dead_code)]
#[path = "../ipmc-defs/src/defs.rs"]
mod defs;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    println!("cargo:rustc-link-search={manifest_dir}");
    // stack_runtime.x and stack_boot.x: `_stack_reserve = <bytes>;` from defs.rs, included by
    // memory.x and boot.x.
    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    println!("cargo:rustc-link-search={out_dir}");
    println!("cargo:rerun-if-changed=../ipmc-defs/src/defs.rs");
    for (file, bytes) in [
        ("stack_runtime.x", defs::IPMC_STACK_RESERVE_BYTES),
        ("stack_boot.x", defs::IPMC_BOOT_STACK_RESERVE_BYTES),
    ] {
        let path = std::path::Path::new(&out_dir).join(file);
        if let Err(e) = std::fs::write(&path, format!("_stack_reserve = {bytes};\n")) {
            panic!("cannot write {}: {e}", path.display());
        }
    }
    // Each linker script belongs to one binary.
    println!("cargo:rustc-link-arg-bin=ipmc-nucleo-f401re=-Tlink.x");
    println!("cargo:rustc-link-arg-bin=ipmc-boot=-Tboot.x");
    // Sectors 0-1 hold 32 KB: the linker folds identical functions (memclr4/memclr8, memcpy4/memcpy8
    // and equal generic instances) into one. No code compares function addresses. `all` also folds
    // identical read-only data, so no code may compare the addresses of two statics either; nothing
    // does today.
    println!("cargo:rustc-link-arg-bin=ipmc-boot=--icf=all");
    for script in ["link.x", "memory.x", "boot.x", "flash_map.x"] {
        println!("cargo:rerun-if-changed={script}");
    }
    dep_cfg::cfg_from_defs("EVAL", "ipmc_eval", "../ipmc-defs/dep_cfg.rs");
    dep_cfg::cfg_from_defs("FIELD", "ipmc_field", "../ipmc-defs/dep_cfg.rs");
}
