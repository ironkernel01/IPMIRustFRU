// reset_cause.rs - decodes and clears the reset flags in RCC_CSR (RM0368 6.3.21).
//
// Flags accumulate until RMVF clears them, so they are read once at start-up. Several
// flags can be set together (power-on also sets PINRSTF and BORRSTF), so the most
// specific cause is chosen first and the reset pin is reported last.

use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

use stm32f4::stm32f401::RCC;

// ResetCause lives in ipmc-hal (plain data, shared with every board).
pub use ipmc_hal::system::ResetCause;

/// system_reset -- restarts the processor: AIRCR SYSRESETREQ (`SCB::sys_reset`, which waits with
/// a data barrier until the reset takes effect). RCC_CSR then sets SFTRSTF, which `take` decodes
/// as `ResetCause::Software`, so the banner after the restart reads "software reset". The caller
/// has flushed the console first.
pub fn system_reset() -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

/// KEPT_WORDS -- words of `KEPT`: the next rqSeq, the low and high half of the mask of rqSeq
/// values sent inside T9, and the check word.
const KEPT_WORDS: usize = 4;

/// KEPT -- the request sequence state across a software reset (layout in `KEPT_WORDS`). The
/// `.uninit` section (cortex-m-rt) is not touched by start-up code, so the words keep their value
/// through SYSRESETREQ. Owner: this module; only `KeptSeqWords` (hal_impl.rs) writes the value
/// (`ipmc_runtime::kept` stores and reads the sequence state).
#[link_section = ".uninit.KEPT_SEQUENCE"]
static mut KEPT: MaybeUninit<[u32; KEPT_WORDS]> = MaybeUninit::uninit();

/// kept_base -- the address of the first word of `KEPT` (`KeptSeqWords` reads and writes through
/// it).
pub(crate) fn kept_base() -> *mut u32 {
    addr_of_mut!(KEPT).cast::<u32>()
}

/// read -- decodes the reset flags without clearing them.
pub fn read(rcc: &RCC) -> ResetCause {
    let csr = rcc.csr().read();
    if csr.lpwrrstf().bit_is_set() {
        ResetCause::LowPower
    } else if csr.wwdgrstf().bit_is_set() {
        ResetCause::WindowWatchdog
    } else if csr.wdgrstf().bit_is_set() {
        ResetCause::IndependentWatchdog
    } else if csr.sftrstf().bit_is_set() {
        ResetCause::Software
    } else if csr.porrstf().bit_is_set() {
        ResetCause::PowerOn
    } else if csr.borrstf().bit_is_set() {
        ResetCause::BrownOut
    } else if csr.padrstf().bit_is_set() {
        ResetCause::Pin
    } else {
        ResetCause::Unknown
    }
}

/// take -- reads the reset flags (`read`), then clears them (RMVF).
pub fn take(rcc: &RCC) -> ResetCause {
    let cause = read(rcc);
    rcc.csr().modify(|_, w| w.rmvf().clear());
    cause
}
