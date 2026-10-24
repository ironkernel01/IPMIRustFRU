// bsp/reset_cause.rs - decodes and clears the reset flags in RCC_CSR (RM0368 6.3.21).
//
// Flags accumulate until RMVF clears them, so they are read once at start-up. Several
// flags can be set together (power-on also sets PINRSTF and BORRSTF), so the most
// specific cause is chosen first and the reset pin is reported last.

use core::mem::MaybeUninit;
use core::ptr::{addr_of_mut, write_volatile};

use stm32f4::stm32f401::RCC;

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
/// Word positions in `KEPT`.
const KEPT_SEQ: usize = 0;
const KEPT_RECENT_LO: usize = 1;
const KEPT_RECENT_HI: usize = 2;
const KEPT_CHECK: usize = 3;

/// KEPT -- the request sequence state across a software reset (layout in `KEPT_WORDS`). The
/// `.uninit` section (cortex-m-rt) is not touched by start-up code, so the words keep their value
/// through SYSRESETREQ. Owner: this module; only `keep_sequence` writes the value and only
/// `kept_sequence` reads and clears it.
#[link_section = ".uninit.KEPT_SEQUENCE"]
static mut KEPT: MaybeUninit<[u32; KEPT_WORDS]> = MaybeUninit::uninit();

/// check_word -- the check word of the three data words.
const fn check_word(seq: u32, lo: u32, hi: u32) -> u32 {
    seq ^ lo ^ hi ^ KEPT_MAGIC
}

/// read_raw_word -- the word at `p` as an integer, however the memory got its value.
fn read_raw_word(p: *const u32) -> u32 {
    let value: u32;
    // SAFETY: `p` is an aligned word inside `KEPT`, a static this module owns, so the load is
    // valid. An `asm!` output is an initialised integer whatever the RAM held (a
    // `read_volatile` of never-written memory is not); the value is only an integer, and the
    // check word decides whether it means anything.
    unsafe {
        core::arch::asm!(
            "ldr {v}, [{p}]",
            p = in(reg) p,
            v = out(reg) value,
            options(nostack, readonly, preserves_flags)
        );
    }
    value
}

/// keep_sequence -- stores the next request sequence number (6 bits) and the mask of the rqSeq
/// values sent inside T9 (bit n for rqSeq n) for the boot after a software reset. Call it just
/// before `system_reset`.
pub fn keep_sequence(next_seq: u8, recent: u64) {
    let seq = u32::from(next_seq);
    // Truncation intended: the two 32-bit halves of the mask.
    let (lo, hi) = (recent as u32, (recent >> 32) as u32);
    let words = addr_of_mut!(KEPT).cast::<u32>();
    // SAFETY: `KEPT` is a static of `KEPT_WORDS` words owned by this module; the main loop is the
    // only caller and no interrupt handler touches it. Volatile writes keep the compiler from
    // dropping them before the reset.
    unsafe {
        write_volatile(words.add(KEPT_SEQ), seq);
        write_volatile(words.add(KEPT_RECENT_LO), lo);
        write_volatile(words.add(KEPT_RECENT_HI), hi);
        write_volatile(words.add(KEPT_CHECK), check_word(seq, lo, hi));
    }
}

/// kept_sequence -- the next request sequence number and the mask of recent rqSeq values that
/// `keep_sequence` stored before a software reset; (0, 0) after any other reset, and when the
/// check word does not match. Clears the check word, so a later reset that did not store a value
/// reads (0, 0). Call it once at start-up with the cause `take` returned.
pub fn kept_sequence(cause: ResetCause) -> (u8, u64) {
    let words = addr_of_mut!(KEPT).cast::<u32>();
    let word = |index: usize| read_raw_word(words.wrapping_add(index));
    let (seq, lo, hi, check) = (
        word(KEPT_SEQ),
        word(KEPT_RECENT_LO),
        word(KEPT_RECENT_HI),
        word(KEPT_CHECK),
    );
    // SAFETY: as in `keep_sequence`; the write clears the check word.
    unsafe { write_volatile(words.add(KEPT_CHECK), 0) };
    match u8::try_from(seq) {
        Ok(next_seq)
            if matches!(cause, ResetCause::Software) && check == check_word(seq, lo, hi) =>
        {
            (next_seq, u64::from(lo) | u64::from(hi) << 32)
        }
        _ => (0, 0),
    }
}

/// take -- reads the reset flags, then clears them.
pub fn take(rcc: &RCC) -> ResetCause {
    let csr = rcc.csr().read();
    let cause = if csr.lpwrrstf().bit_is_set() {
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
    };
    rcc.csr().modify(|_, w| w.rmvf().clear());
    cause
}
