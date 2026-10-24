// build.rs - sets `cfg(tier23)` when the Tier-2/3 extension is built in.
//
// ipmc-defs is the one place that looks for the Tier23 folder; dep_cfg.rs (shared with the
// firmware crate's build script) turns its published decision into the cfg.

#[path = "../ipmc-defs/dep_cfg.rs"]
mod dep_cfg;

fn main() {
    dep_cfg::cfg_from_defs("TIER23", "tier23", "../ipmc-defs/dep_cfg.rs");
}
