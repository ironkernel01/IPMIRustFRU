//! sim - simulation drivers: implement the hal traits with values held in memory, for the
//! bench and the host tests. The SIM console group adjusts them at run time.
//!
//! One simulator per hal trait covers every board, so an integrator adds nothing here. Start
//! values: `analog` takes them from the board table (board_sensors::sim_analog, over the wiring table the caller passes), `digital`
//! starts every pin at its defs.rs IPMC_SIM_*_HIGH level, `power` starts with the enable off.

pub mod analog;
pub mod digital;
pub mod power;

pub use analog::{SimAnalog, SIM_RAIL_WIRING};
pub use digital::SimInputs;
pub use power::SimPower;
