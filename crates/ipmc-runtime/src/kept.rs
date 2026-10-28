// kept.rs - the request sequence state across a software reset.
//
// The next IPMB request sequence number and the mask of the rqSeq values sent inside T9 are stored
// before a restart and read once after it, in four kept words (`KeptWords<4>`: three data words
// and a check word, `ipmc_hal::kept`). The board owns the words (NOINIT RAM on the F401).

use ipmc_hal::kept::{store_checked, take_checked, KeptWords};
use ipmc_hal::system::ResetCause;

/// KEPT_MAGIC -- the check value that marks the words as written by `keep_sequence`: the last
/// word holds the XOR of the other three words and this constant, which random RAM after a
/// power-on matches with probability 2^-32 (and `kept_sequence` reads it only after a software
/// reset).
const KEPT_MAGIC: u32 = 0x5EC0_5EC0;

/// keep_sequence -- stores the next request sequence number (6 bits) and the mask of the rqSeq
/// values sent inside T9 (bit n for rqSeq n) for the boot after a software reset. Call it just
/// before the reset.
pub fn keep_sequence<K: KeptWords<4>>(kept: &mut K, next_seq: u8, recent: u64) {
    let seq = u32::from(next_seq);
    // Truncation intended: the two 32-bit halves of the mask.
    let (lo, hi) = (recent as u32, (recent >> 32) as u32);
    store_checked(kept, [seq, lo, hi], KEPT_MAGIC);
}

/// kept_sequence -- the next request sequence number and the mask of recent rqSeq values that
/// `keep_sequence` stored before a software reset; (0, 0) after any other reset, and when the
/// check word does not match. Clears the check word, so a later reset that did not store a value
/// reads (0, 0). Call it once at start-up with the cause the reset flags gave.
pub fn kept_sequence<K: KeptWords<4>>(kept: &mut K, cause: ResetCause) -> (u8, u64) {
    // `take_checked` clears the check word.
    let stored = take_checked(kept, KEPT_MAGIC);
    match stored {
        Some([seq, lo, hi]) if matches!(cause, ResetCause::Software) => match u8::try_from(seq) {
            Ok(next_seq) => (next_seq, u64::from(lo) | u64::from(hi) << 32),
            Err(_) => (0, 0),
        },
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_reset_returns_what_was_kept_once() {
        let mut ram = [0u32; 4];
        let recent = 0x0123_4567_89AB_CDEF_u64;
        keep_sequence(&mut ram, 0x2A, recent);
        assert_eq!(
            kept_sequence(&mut ram, ResetCause::Software),
            (0x2A, recent)
        );
        assert_eq!(
            kept_sequence(&mut ram, ResetCause::Software),
            (0, 0),
            "the check word is cleared by the first read"
        );
    }

    #[test]
    fn any_other_cause_gives_zero_and_clears() {
        let causes = [
            ResetCause::PowerOn,
            ResetCause::Pin,
            ResetCause::IndependentWatchdog,
            ResetCause::Unknown,
        ];
        for cause in causes {
            let mut ram = [0u32; 4];
            keep_sequence(&mut ram, 5, 0xFF);
            assert_eq!(kept_sequence(&mut ram, cause), (0, 0));
            assert_eq!(kept_sequence(&mut ram, ResetCause::Software), (0, 0));
        }
    }

    #[test]
    fn random_ram_and_a_wide_sequence_give_zero() {
        let mut noise = [0xDEAD_BEEF, 1, 2, 3];
        assert_eq!(kept_sequence(&mut noise, ResetCause::Software), (0, 0));
        let mut wide = [0u32; 4];
        store_checked(&mut wide, [0x100, 1, 2], KEPT_MAGIC);
        assert_eq!(kept_sequence(&mut wide, ResetCause::Software), (0, 0));
    }
}
