// build.rs - sets `cfg(ipmc_eval)` when the evaluation build is on.
//
// ipmc-defs is the one place that reads IPMC_EVAL; dep_cfg.rs (shared with the other build
// scripts that take a cfg from ipmc-defs) turns its published decision into the cfg.

#[path = "../ipmc-defs/dep_cfg.rs"]
mod dep_cfg;

fn main() {
    dep_cfg::cfg_from_defs("EVAL", "ipmc_eval", "../ipmc-defs/dep_cfg.rs");
}
