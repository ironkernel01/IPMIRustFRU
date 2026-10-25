// build.rs - sets `cfg(ipmc_eval)` when the evaluation build is selected (IPMC_EVAL=1).
//
// ipmc-defs is the one place that reads IPMC_EVAL and checks its conditions; dep_cfg.rs (shared
// with ipmc-ext's build script) turns its published decision into the cfg. This script reads no
// file of its own, so it does not change how memory.x is found. It asks cargo to relink when
// the linker script (link.x) or the memory layout (memory.x) changes.

#[path = "../ipmc-defs/dep_cfg.rs"]
mod dep_cfg;

fn main() {
    println!("cargo:rerun-if-changed=link.x");
    println!("cargo:rerun-if-changed=memory.x");
    dep_cfg::cfg_from_defs("EVAL", "ipmc_eval", "../ipmc-defs/dep_cfg.rs");
}
