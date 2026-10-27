// reset_cause.rs - decodes and clears the reset flags in RCC_CSR (RM0368 6.3.21).
//
// Flags accumulate until RMVF clears them, so they are read once at start-up. Several
// flags can be set together (power-on also sets PINRSTF and BORRSTF), so the most
// specific cause is chosen first and the reset pin is reported last.

use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

use stm32f4::stm32f401::RCC;

use crate::bsp::kept_words;

/// ResetCause -- why the MCU last reset.
#[derive(Clone, Copy)]
pub enum ResetCause {
    LowPower,
    WindowWatchdog,
    IndependentWatchdog,
    Software,
    PowerOn,
    BrownOut,
    Pin,
    Unknown,
}

impl ResetCause {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            ResetCause::LowPower => "low-power management",
            ResetCause::WindowWatchdog => "window watchdog",
            ResetCause::IndependentWatchdog => "independent watchdog",
            ResetCause::Software => "software reset",
            ResetCause::PowerOn => "power-on",
            ResetCause::BrownOut => "brown-out",
            ResetCause::Pin => "reset pin (button or debugger)",
            ResetCause::Unknown => "unknown",
        }
    }
}

/// system_reset -- restarts the processor: AIRCR SYSRESETREQ (`SCB::sys_reset`, which waits with
/// a data barrier until the reset takes effect). RCC_CSR then sets SFTRSTF, which `take` decodes
/// as `ResetCause::Software`, so the banner after the restart reads "software reset". The caller
/// has flushed the console first.
pub fn system_reset() -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

/// KEPT_MAGIC -- the check value that marks `KEPT` as written by `keep_sequence`: the last word
/// holds the XOR of the other three words and this constant, which random RAM after a power-on
/// matches with probability 2^-32 (and `kept_sequence` reads it only after a software reset).
const KEPT_MAGIC: u32 = 0x5EC0_5EC0;

/// KEPT_WORDS -- words of `KEPT`: the next rqSeq, the low and high half of the mask of rqSeq
/// values sent inside T9, and the check word.
const KEPT_WORDS: usize = 4;

/// KEPT -- the request sequence state across a software reset (layout in `KEPT_WORDS`). The
/// `.uninit` section (cortex-m-rt) is not touched by start-up code, so the words keep their value
/// through SYSRESETREQ. Owner: this module; only `keep_sequence` writes the value and only
/// `kept_sequence` reads and clears it.
#[link_section = ".uninit.KEPT_SEQUENCE"]
static mut KEPT: MaybeUninit<[u32; KEPT_WORDS]> = MaybeUninit::uninit();

/// kept_base -- the address of the first word of `KEPT`.
fn kept_base() -> *mut u32 {
    addr_of_mut!(KEPT).cast::<u32>()
}

/// keep_sequence -- stores the next request sequence number (6 bits) and the mask of the rqSeq
/// values sent inside T9 (bit n for rqSeq n) for the boot after a software reset. Call it just
/// before `system_reset`.
pub fn keep_sequence(next_seq: u8, recent: u64) {
    let seq = u32::from(next_seq);
    // Truncation intended: the two 32-bit halves of the mask.
    let (lo, hi) = (recent as u32, (recent >> 32) as u32);
    // SAFETY: `KEPT` is a static of `KEPT_WORDS` words owned by this module; the main loop is the
    // only caller and no interrupt handler touches it.
    unsafe { kept_words::store_checked(kept_base(), [seq, lo, hi], KEPT_MAGIC) };
}

/// kept_sequence -- the next request sequence number and the mask of recent rqSeq values that
/// `keep_sequence` stored before a software reset; (0, 0) after any other reset, and when the
/// check word does not match. Clears the check word, so a later reset that did not store a value
/// reads (0, 0). Call it once at start-up with the cause `take` returned.
pub fn kept_sequence(cause: ResetCause) -> (u8, u64) {
    // SAFETY: as in `keep_sequence`; `take_checked` clears the check word.
    let kept = unsafe { kept_words::take_checked(kept_base(), KEPT_MAGIC) };
    match kept {
        Some([seq, lo, hi]) if matches!(cause, ResetCause::Software) => match u8::try_from(seq) {
            Ok(next_seq) => (next_seq, u64::from(lo) | u64::from(hi) << 32),
            Err(_) => (0, 0),
        },
        _ => (0, 0),
    }
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
