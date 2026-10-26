// eval.rs - the evaluation period of the evaluation build (IPMC_EVAL=1; `cfg(ipmc_eval)`).
//
// The period is IPMC_EVAL_MINUTES (defs_tier23.rs) of run time, measured by the TIM2
// counter of bsp/eval_timer.rs; when it is over the main loop stops the IPMB once and no longer
// services the link. Nothing here, or in the places that call it, exists in any other build.

use crate::bsp::eval_timer::TICK_HZ;

/// MINUTE_TICKS -- timer ticks in one minute.
const MINUTE_TICKS: u32 = TICK_HZ * 60;

/// LIMIT_TICKS -- the evaluation period in timer ticks; the build fails when it overflows 32 bits.
const LIMIT_TICKS: u32 = match ipmc_defs::IPMC_EVAL_MINUTES.checked_mul(MINUTE_TICKS) {
    Some(ticks) if ipmc_defs::IPMC_EVAL_MINUTES >= 1 => ticks,
    _ => panic!("IPMC_EVAL_MINUTES must be 1..=7158 (tick count must fit 32 bits)"),
};

/// expired -- true once the evaluation period has run.
pub fn expired() -> bool {
    crate::bsp::eval_timer::elapsed_ticks() >= LIMIT_TICKS
}
