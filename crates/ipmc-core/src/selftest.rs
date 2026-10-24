//! selftest.rs - the controller self test and the result Get Self Test Results reports.
//!
//! IPMI v2.0 section 20.4 and Table 20-5: byte 2 of the response is 55h when every test passed or
//! 57h "corrupted or inaccessible data or devices", and byte 3 is then a bit field naming what
//! failed (a set bit means failed, a clear bit means unknown). This controller tests the two
//! stores it owns:
//!
//! - the FRU image: `ipmc_fru::verify` over the RAM scratchpad (headers, versions, lengths,
//!   zero checksums, end-of-fields markers);
//! - the Device SDR repository: a walk of the record chain from the first record to
//!   FFFFh, checking each record's own ID and length against its header.
//!
//! `Context::new` runs the test once at startup, and every accepted Write FRU Data runs it again
//! (VITA 46.11 Rec 8-2: corrupt FRU data is recorded in the self test results). A write of more
//! than one chunk therefore shows 57h between the chunks, until the last one restores a valid
//! image. The result gates only the FRU initial checks (Rule 4.1.5.4.1-9): it decides M0 to M1.
//! A failure after the FRU left M0 does not take it out of M1 or M4.

use crate::sdr::{SdrRepository, HEADER_LEN, NO_NEXT_RECORD, RECORD_COUNT_MAX};

/// RESULT_PASSED -- byte 2 = 55h: no error, all self tests passed (IPMI v2.0 Table 20-5).
const RESULT_PASSED: u8 = 0x55;
/// RESULT_CORRUPTED -- byte 2 = 57h: corrupted or inaccessible data or devices (Table 20-5); byte 3
/// is then the error bit field below.
const RESULT_CORRUPTED: u8 = 0x57;
/// DETAIL_NONE -- byte 3 = 00h for byte 2 = 55h (Table 20-5).
const DETAIL_NONE: u8 = 0x00;
/// DETAIL_SDR_UNREADABLE -- byte 3 bit 6: cannot access SDR repository (Table 20-5).
const DETAIL_SDR_UNREADABLE: u8 = 1 << 6;
/// DETAIL_FRU_UNREADABLE -- byte 3 bit 5: cannot access BMC FRU device (Table 20-5). A FRU
/// image that fails `ipmc_fru::verify` is reported here. Bit 2, "Internal Use Area of BMC FRU
/// corrupted", does not fit: this image has no Internal Use Area.
const DETAIL_FRU_UNREADABLE: u8 = 1 << 5;
/// DETAIL_SDR_EMPTY -- byte 3 bit 3: SDR repository empty (Table 20-5).
const DETAIL_SDR_EMPTY: u8 = 1 << 3;

/// LENGTH_INDEX -- the record length byte, the last byte of the SDR header (Table 43-2 byte 5).
const LENGTH_INDEX: usize = HEADER_LEN - 1;
/// WALK_STEPS_MAX -- most records one chain walk visits: one more than the repository holds, so a
/// chain that does not end by itself is seen as broken.
const WALK_STEPS_MAX: usize = RECORD_COUNT_MAX + 1;

/// SelfTest -- bytes 2 and 3 of the Get Self Test Results response (IPMI v2.0 Table 20-5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfTest {
    result: u8,
    detail: u8,
}

impl SelfTest {
    /// passed -- every test passed (result 55h). The FRU initial checks read this.
    pub const fn passed(&self) -> bool {
        self.result == RESULT_PASSED
    }

    /// bytes -- the two response data bytes: the result, then the detail.
    pub const fn bytes(&self) -> [u8; 2] {
        [self.result, self.detail]
    }
}

/// chain_intact -- walks the SDR chain from the first record, following next IDs to FFFFh, at
/// most WALK_STEPS_MAX records. True when each record exists, carries the ID used to reach it,
/// has a length byte that matches its size, and the number of records visited equals the
/// repository's record count.
fn chain_intact(sdr: &SdrRepository) -> bool {
    let Some(mut id) = sdr.resolve(0) else {
        return false;
    };
    let mut visited = 0;
    for _ in 0..WALK_STEPS_MAX {
        let Some(record) = sdr.record(id) else {
            return false;
        };
        let [id_lo, id_hi, ..] = record else {
            return false;
        };
        let Some(length) = record.get(LENGTH_INDEX) else {
            return false;
        };
        if u16::from_le_bytes([*id_lo, *id_hi]) != id
            || usize::from(*length) + HEADER_LEN != record.len()
        {
            return false;
        }
        visited += 1;
        let next = sdr.next_id(id);
        if next == NO_NEXT_RECORD {
            return visited == sdr.record_count();
        }
        id = next;
    }
    false
}

/// run -- checks the FRU image (`ipmc_fru::verify`) and walks the SDR repository.
/// 1. A FRU image that fails `verify`: 57h with the "cannot access BMC FRU device" bit.
/// 2. A repository with no records: 57h with the "SDR repository empty" bit.
/// 3. A repository whose chain is broken (see `chain_intact`): 57h with the "cannot access SDR
///    repository" bit.
///
/// Several failures OR their bits; no failure is 55h 00h.
pub fn run(fru: &[u8], sdr: &SdrRepository) -> SelfTest {
    let mut detail = DETAIL_NONE;
    if ipmc_fru::verify(fru).is_err() {
        detail |= DETAIL_FRU_UNREADABLE;
    }
    if sdr.record_count() == 0 {
        detail |= DETAIL_SDR_EMPTY;
    } else if !chain_intact(sdr) {
        detail |= DETAIL_SDR_UNREADABLE;
    }
    let result = if detail == DETAIL_NONE {
        RESULT_PASSED
    } else {
        RESULT_CORRUPTED
    };
    SelfTest { result, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipmc_fru::FruStore;

    fn repo() -> SdrRepository {
        SdrRepository::new(0x82)
    }

    #[test]
    fn a_fresh_image_and_repository_pass() {
        let t = run(FruStore::new().bytes(), &repo());
        assert!(t.passed());
        assert_eq!(t.bytes(), [0x55, 0x00]);
    }

    #[test]
    fn a_damaged_fru_image_sets_the_fru_bit() {
        let mut fru = FruStore::new();
        fru.write(20, &[0xFF]).unwrap();
        let t = run(fru.bytes(), &repo());
        assert!(!t.passed());
        assert_eq!(t.bytes(), [0x57, 0x20]);
    }

    #[test]
    fn a_broken_sdr_chain_sets_the_sdr_bit() {
        let fru = FruStore::new();
        // A record whose header ID is not the ID it is stored under.
        let mut wrong_id = repo();
        wrong_id.set_record_byte(3, 0, 9);
        assert_eq!(run(fru.bytes(), &wrong_id).bytes(), [0x57, 0x40]);
        // A record whose length byte does not match its size.
        let mut wrong_length = repo();
        wrong_length.set_record_byte(5, 4, 1);
        assert_eq!(run(fru.bytes(), &wrong_length).bytes(), [0x57, 0x40]);
    }

    #[test]
    fn several_failures_or_their_bits() {
        let mut fru = FruStore::new();
        fru.write(0, &[0x07]).unwrap();
        let mut sdr = repo();
        sdr.set_record_byte(0, 4, 0);
        assert_eq!(run(fru.bytes(), &sdr).bytes(), [0x57, 0x60]);
    }
}
