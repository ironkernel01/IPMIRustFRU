// persist/tests.rs - the log against a RAM-backed `FlashLog` (flash semantics: programming only
// clears bits, erase sets every byte to FFh, a word budget simulates a power cut, a late error
// simulates a driver that reports a fault after the word landed, a torn erase simulates a power
// cut during an erase).

use super::*;

/// SLOTS -- records per mock sector; the sector also has a short blank tail.
const SLOTS: usize = 4;
const TAIL: usize = 8;
const SECTOR_LEN: usize = SLOTS * RECORD_LEN + TAIL;
const DELAY_MS: u32 = 1000;
/// A byte of the image the tests change to tell images apart (inside the zero padding).
const TAG_AT: usize = SIZE - 1;

struct Mock {
    sector: Vec<u8>,
    /// Words that may still be programmed; None = unlimited. At 0 the next word fails.
    budget: Option<usize>,
    words_programmed: usize,
    /// When `words_programmed` reaches this count the word just written stays and the call
    /// fails with Timeout (the busy flag cleared after the poll bound ran out).
    late_error: Option<usize>,
    /// The next erase sets only the first half of the sector to FFh and fails (power cut).
    torn_erase: bool,
    erases: usize,
    /// The clock `FlashLog::millis` reports; an erase advances it by `erase_ms` (a slow erase).
    now: u32,
    erase_ms: u32,
    /// Every erase fails with Timeout and leaves the sector as it was.
    erase_fails: bool,
    /// The next erase is deferred once (`FlashError::Deferred`), touching nothing.
    defer_erase: bool,
    /// NVMRO is asserted: every erase and program is refused and touches nothing.
    nvmro: bool,
}

impl Mock {
    fn new() -> Mock {
        Mock {
            sector: vec![ERASED; SECTOR_LEN],
            budget: None,
            words_programmed: 0,
            late_error: None,
            torn_erase: false,
            erases: 0,
            now: 0,
            erase_ms: 0,
            erase_fails: false,
            defer_erase: false,
            nvmro: false,
        }
    }
}

impl FlashLog for Mock {
    fn sector_len(&self) -> usize {
        self.sector.len()
    }

    fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), FlashError> {
        for (i, slot) in out.iter_mut().enumerate() {
            let at = offset.checked_add(i).ok_or(FlashError::Range)?;
            *slot = *self.sector.get(at).ok_or(FlashError::Range)?;
        }
        Ok(())
    }

    fn program(&mut self, offset: usize, words: &[u32]) -> Result<(), FlashError> {
        if self.nvmro {
            return Err(FlashError::NvmroAsserted);
        }
        if !offset.is_multiple_of(WORD) {
            return Err(FlashError::Alignment);
        }
        for (i, word) in words.iter().enumerate() {
            if self.budget == Some(0) {
                return Err(FlashError::Programming);
            }
            let at = offset + i * WORD;
            let target = self
                .sector
                .get_mut(at..at + WORD)
                .ok_or(FlashError::Range)?;
            for (byte, new) in target.iter_mut().zip(word.to_le_bytes()) {
                *byte &= new;
            }
            self.words_programmed += 1;
            self.budget = self.budget.map(|b| b - 1);
            if self.late_error == Some(self.words_programmed) {
                return Err(FlashError::Timeout);
            }
        }
        Ok(())
    }

    fn erase(&mut self) -> Result<(), FlashError> {
        if self.nvmro {
            return Err(FlashError::NvmroAsserted);
        }
        if std::mem::take(&mut self.defer_erase) {
            return Err(FlashError::Deferred);
        }
        self.erases += 1;
        self.now = self.now.wrapping_add(self.erase_ms);
        if self.erase_fails {
            return Err(FlashError::Timeout);
        }
        let bytes = &mut self.sector;
        if std::mem::take(&mut self.torn_erase) {
            bytes[..SECTOR_LEN / 2].fill(ERASED);
            return Err(FlashError::Timeout);
        }
        bytes.fill(ERASED);
        Ok(())
    }

    fn millis(&self) -> u32 {
        self.now
    }
}

/// image -- the built image with the tag byte set to `tag`.
fn image(tag: u8) -> [u8; SIZE] {
    let mut bytes = *FruStore::new().image();
    bytes[TAG_AT] = tag;
    bytes
}

/// booted -- a log over `flash` after a boot.
fn booted(flash: &mut Mock) -> (Persist, Option<FruStore>, BootReport) {
    booted_nvmro(flash, false)
}

/// booted_nvmro -- `booted` with the NVMRO level given.
fn booted_nvmro(flash: &mut Mock, nvmro: bool) -> (Persist, Option<FruStore>, BootReport) {
    let mut log = Persist::new();
    let (store, report) = log.boot(flash, nvmro).expect("boot");
    (log, store, report)
}

/// save_tags -- saves one image per tag, in order.
fn save_tags(log: &mut Persist, flash: &mut Mock, tags: impl Iterator<Item = u8>) {
    for tag in tags {
        log.save(flash, &image(tag), 0).expect("save");
    }
}

/// loaded_tag -- the tag of the image a fresh boot loads, None when nothing is stored.
fn loaded_tag(flash: &mut Mock) -> Option<u8> {
    booted(flash).1.map(|store| store.image()[TAG_AT])
}

#[test]
fn the_record_layout_is_as_documented() {
    let record = encode(0x0102_0304, &image(0x5A), 0);
    assert_eq!(RECORD_LEN, SIZE + 16);
    assert_eq!(&record[..4], b"1IFU");
    assert_eq!(record[4..8], [0x04, 0x03, 0x02, 0x01]);
    assert_eq!(u16::from_le_bytes([record[8], record[9]]) as usize, SIZE);
    assert_eq!(u16::from_le_bytes([record[10], record[11]]), 1);
    assert_eq!(record[12 + TAG_AT], 0x5A);
    let crc = u32::from_le_bytes(record[12 + SIZE..].try_into().unwrap());
    assert_eq!(crc, crc32(&record[4..12 + SIZE]));
    assert_eq!(classify(&record), Slot::Valid(0x0102_0304));
    assert_eq!(pack_words(&record)[0], COMMIT_WORD);
}

#[test]
fn each_check_of_a_slot_rejects_a_record_that_breaks_it() {
    // (field offset, bytes written there); the CRC is recomputed so only that field is wrong.
    let cases: [(usize, &[u8]); 5] = [
        (OFF_LEN, &[0x01, 0x00]),
        (OFF_VERSION, &[2, 0]),
        (OFF_SEQ, &[0, 0, 0, 0]),
        (OFF_SEQ, &[0xFF, 0xFF, 0xFF, 0xFF]),
        (OFF_COMMIT, &[0, 0, 0, 0]),
    ];
    for (offset, bytes) in cases {
        let mut record = encode(7, &image(1), 0);
        record[offset..offset + bytes.len()].copy_from_slice(bytes);
        let crc = crc32(&record[OFF_SEQ..OFF_CRC]);
        record[OFF_CRC..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(classify(&record), Slot::Invalid, "field at {offset}");
    }
    let mut flipped = encode(7, &image(1), 0);
    flipped[OFF_IMAGE + 3] ^= 0x01;
    assert_eq!(classify(&flipped), Slot::Invalid, "image bit flip");
    assert_eq!(classify(&[ERASED; RECORD_LEN]), Slot::Blank);
}

#[test]
fn empty_flash_loads_the_built_image() {
    let mut flash = Mock::new();
    let (log, store, report) = booted(&mut flash);
    assert!(store.is_none(), "None means: use the built image");
    assert_eq!(report.loaded, None);
    assert!(!report.erased);
    assert_eq!(log.status().newest_seq, None);
    assert_eq!(flash.erases, 0);
}

#[test]
fn save_then_load_returns_the_image_and_starts_at_sequence_1() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let saved = log.save(&mut flash, &image(0x11), 0).unwrap();
    assert_eq!(
        saved,
        Saved {
            offset: 0,
            seq: 1,
            erased: false
        }
    );
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image(), &image(0x11));
    assert_eq!(report.loaded, Some(1));
}

#[test]
fn the_newest_sequence_wins_and_survives_the_erase_of_a_full_sector() {
    // 4 records fill the sector (1..=4); the 5th erases it and becomes the first record.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    let (_, store, report) = booted(&mut flash);
    assert_eq!(report.loaded, Some(4));
    assert_eq!(store.unwrap().image()[TAG_AT], 4);
    let saved = log.save(&mut flash, &image(5), 0).unwrap();
    assert_eq!(
        saved,
        Saved {
            offset: 0,
            seq: 5,
            erased: true
        }
    );
    let (_, store, report) = booted(&mut flash);
    assert_eq!(
        report.loaded,
        Some(5),
        "the sequence goes on after the erase"
    );
    assert_eq!(store.unwrap().image()[TAG_AT], 5);
    assert_eq!(report.skipped, 0);
}

#[test]
fn a_full_sector_is_erased_by_the_next_save_alone() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=3);
    let s = log.status();
    assert_eq!((s.records, s.erase_pending), (3, false));
    assert!(!log.save_needs_erase());
    log.save(&mut flash, &image(4), 0).unwrap();
    let s = log.status();
    assert_eq!((s.records, s.erase_pending, s.bytes_free), (4, true, 0));
    assert!(log.save_needs_erase());
    assert_eq!(flash.erases, 0, "filling the sector erases nothing");
    let saved = log.save(&mut flash, &image(5), 0).unwrap();
    assert_eq!((saved.offset, saved.erased), (0, true));
    assert_eq!(flash.erases, 1);
    let s = log.status();
    assert_eq!((s.records, s.erase_pending), (1, false));
}

#[test]
fn a_power_cut_after_any_single_word_loads_the_previous_good_image() {
    for cut_after in 0..=RECORD_WORDS {
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        log.save(&mut flash, &image(1), 0).unwrap();
        flash.budget = Some(cut_after);
        let result = log.save(&mut flash, &image(2), 0);
        let complete = cut_after == RECORD_WORDS;
        assert_eq!(result.is_ok(), complete, "cut after word {cut_after}");
        if !complete {
            // The commit word goes last: a cut before the final word leaves it erased.
            let commit = &flash.sector[RECORD_LEN..RECORD_LEN + WORD];
            assert_eq!(commit, [ERASED; WORD], "cut after word {cut_after}");
        }
        flash.budget = None;
        let want = if complete { 2 } else { 1 };
        assert_eq!(
            loaded_tag(&mut flash),
            Some(want),
            "cut after word {cut_after}"
        );
        // The next boot saves after the damaged slot and still orders correctly.
        let (mut log, _, _) = booted(&mut flash);
        log.save(&mut flash, &image(3), 0).unwrap();
        assert_eq!(loaded_tag(&mut flash), Some(3), "after cut at {cut_after}");
    }
}

#[test]
fn a_failed_save_skips_its_slot_in_the_running_log_too() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    flash.budget = Some(2);
    assert_eq!(
        log.save(&mut flash, &image(1), 0),
        Err(PersistError::Flash(FlashError::Programming))
    );
    flash.budget = None;
    let saved = log.save(&mut flash, &image(2), 0).unwrap();
    assert_eq!(saved.offset, RECORD_LEN, "the damaged slot is not reused");
    assert_eq!(loaded_tag(&mut flash), Some(2));
}

#[test]
fn a_flipped_bit_in_a_stored_image_fails_the_crc_and_the_previous_record_loads() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=2);
    flash.sector[RECORD_LEN + OFF_IMAGE + 10] ^= 0x20;
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image()[TAG_AT], 1);
    assert_eq!(report.loaded, Some(1));
    assert_eq!(report.skipped, 1);
}

#[test]
fn nvmro_asserted_never_saves_and_the_write_waits() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let mut store = FruStore::new();
    store.write(TAG_AT, &[9]).unwrap();
    for now in (0..5 * DELAY_MS).step_by(100) {
        assert!(log
            .poll(&mut flash, &mut store, now, true, DELAY_MS, true)
            .is_none());
    }
    assert_eq!(flash.words_programmed, 0, "nothing reaches the flash");
    assert!(log.pending());
    let saved = log.poll(&mut flash, &mut store, 5 * DELAY_MS, false, DELAY_MS, true);
    assert!(matches!(saved, Some(Ok(_))));
    assert_eq!(loaded_tag(&mut flash), Some(9));
}

#[test]
fn the_delay_coalesces_quick_writes_into_one_save() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let mut store = FruStore::new();
    let mut now = 0;
    for tag in 1..=16u8 {
        store.write(TAG_AT, &[tag]).unwrap();
        assert!(log
            .poll(&mut flash, &mut store, now, false, DELAY_MS, true)
            .is_none());
        now += 50;
    }
    let last_write = now - 50;
    // Not yet: the quiet time restarts at every write.
    assert!(log
        .poll(
            &mut flash,
            &mut store,
            last_write + DELAY_MS - 1,
            false,
            DELAY_MS,
            true
        )
        .is_none());
    let saved = log.poll(
        &mut flash,
        &mut store,
        last_write + DELAY_MS,
        false,
        DELAY_MS,
        true,
    );
    assert!(matches!(saved, Some(Ok(Saved { seq: 1, .. }))));
    assert!(log
        .poll(
            &mut flash,
            &mut store,
            last_write + 10 * DELAY_MS,
            false,
            DELAY_MS,
            true
        )
        .is_none());
    let s = log.status();
    assert_eq!((s.saves, s.records, s.pending), (1, 1, false));
    assert_eq!(s.last_save_ms, Some(last_write + DELAY_MS));
    assert_eq!(
        loaded_tag(&mut flash),
        Some(16),
        "the last write is the one stored"
    );
}

#[test]
fn a_failing_flash_is_retried_once_per_delay() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let mut store = FruStore::new();
    store.write(TAG_AT, &[4]).unwrap();
    flash.budget = Some(0);
    assert!(log
        .poll(&mut flash, &mut store, 0, false, DELAY_MS, true)
        .is_none());
    flash.now = DELAY_MS;
    let first = log.poll(&mut flash, &mut store, DELAY_MS, false, DELAY_MS, true);
    assert!(matches!(first, Some(Err(_))));
    assert!(log
        .poll(&mut flash, &mut store, DELAY_MS + 1, false, DELAY_MS, true)
        .is_none());
    flash.budget = None;
    let retry = log.poll(&mut flash, &mut store, 2 * DELAY_MS, false, DELAY_MS, true);
    assert!(matches!(retry, Some(Ok(_))));
    let s = log.status();
    assert_eq!(
        (s.failures, s.saves, s.last_error.map(PersistError::name)),
        (1, 1, Some("Programming"))
    );
}

#[test]
fn a_deferred_erase_is_not_a_failure_and_the_next_poll_saves() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=SLOTS as u8);
    assert!(log.save_needs_erase());
    let mut store = FruStore::new();
    store.write(TAG_AT, &[9]).unwrap();
    flash.defer_erase = true;
    assert!(log
        .poll(&mut flash, &mut store, 0, false, DELAY_MS, true)
        .is_none());
    let deferred = log.poll(&mut flash, &mut store, DELAY_MS, false, DELAY_MS, true);
    assert!(deferred.is_none(), "deferred: no outcome to report");
    let s = log.status();
    assert_eq!((s.failures, s.last_error, s.pending), (0, None, true));
    assert_eq!(flash.erases, 0);
    // No wait for a new delay: the very next pass saves.
    let saved = log.poll(&mut flash, &mut store, DELAY_MS + 1, false, DELAY_MS, true);
    assert!(matches!(saved, Some(Ok(s)) if s.erased));
    assert_eq!(loaded_tag(&mut flash), Some(9));
    assert_eq!(log.status().failures, 0);
}

#[test]
fn boot_never_erases_a_sector_that_holds_a_valid_record() {
    // Single sector: the erase of a full sector belongs to the next save, which writes the
    // newest image straight after; a boot erase would destroy the only copy.
    for saves in [1u8, 3, 4] {
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        save_tags(&mut log, &mut flash, 1..=saves);
        let (log, store, report) = booted(&mut flash);
        assert!(!report.erased, "saves {saves}");
        assert_eq!(flash.erases, 0, "saves {saves}");
        assert_eq!(store.unwrap().image()[TAG_AT], saves);
        assert_eq!(log.save_needs_erase(), saves == 4, "saves {saves}");
    }
}

#[test]
fn boot_clears_a_sector_that_holds_no_valid_record() {
    let mut flash = Mock::new();
    flash.sector[5] = 0x00; // garbage: an interrupted write or erase
    let (log, store, report) = booted(&mut flash);
    assert!(store.is_none());
    assert!(report.erased);
    assert_eq!(report.skipped, 1);
    assert!(flash.sector.iter().all(|&b| b == ERASED));
    assert_eq!(log.status().newest_seq, None);
}

#[test]
fn a_sector_too_small_for_one_record_is_refused() {
    let mut flash = Mock::new();
    flash.sector = vec![ERASED; RECORD_LEN - 4];
    assert_eq!(
        Persist::new().boot(&mut flash, false).err(),
        Some(PersistError::SectorGeometry)
    );
}

#[test]
fn garbage_after_the_last_record_leaves_no_room_so_the_save_erases_first() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=1);
    flash.sector[SECTOR_LEN - 1] = 0x7F; // an interrupted erase left a cell programmed
    let (mut log, _, _) = booted(&mut flash);
    assert!(log.save_needs_erase(), "the sector is treated as full");
    let saved = log.save(&mut flash, &image(2), 0).unwrap();
    assert_eq!((saved.offset, saved.erased), (0, true));
    assert_eq!(loaded_tag(&mut flash), Some(2));
}

#[test]
fn a_save_that_programs_nothing_leaves_a_hole_and_the_record_after_it_still_loads() {
    // A failure before the first bit (busy or locked controller) leaves an all-FFh slot; the
    // next save goes after it. The scan must not end the log at that hole.
    for hole in 0..2usize {
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        save_tags(&mut log, &mut flash, 1..=u8::try_from(hole).unwrap());
        flash.budget = Some(0);
        assert!(
            log.save(&mut flash, &image(0xEE), 0).is_err(),
            "hole {hole}"
        );
        flash.budget = None;
        assert_eq!(
            log.save(&mut flash, &image(7), 0).unwrap().offset,
            (hole + 1) * RECORD_LEN
        );
        let (mut log, store, report) = booted(&mut flash);
        assert_eq!(store.map(|s| s.image()[TAG_AT]), Some(7), "hole {hole}");
        assert!(!report.erased, "hole {hole}: nothing erased");
        let next = log.save(&mut flash, &image(8), 0).unwrap();
        assert_eq!(
            next.offset,
            (hole + 2) * RECORD_LEN,
            "the hole is not reused"
        );
        assert_eq!(loaded_tag(&mut flash), Some(8), "hole {hole}");
    }
}

#[test]
fn a_driver_error_after_the_record_landed_uses_its_sequence_number_up() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    // The commit word is the last word of the first record; the driver reports it failed.
    flash.late_error = Some(RECORD_WORDS);
    assert_eq!(
        log.save(&mut flash, &image(1), 0),
        Err(PersistError::Flash(FlashError::Timeout))
    );
    flash.late_error = None;
    let saved = log.save(&mut flash, &image(2), 0).unwrap();
    assert_eq!((saved.offset, saved.seq), (RECORD_LEN, 2));
    let (_, store, report) = booted(&mut flash);
    assert_eq!(report.loaded, Some(2));
    assert_eq!(store.unwrap().image()[TAG_AT], 2, "the later write wins");
}

#[test]
fn boot_with_nvmro_asserted_erases_nothing() {
    // A sector with garbage and no valid record is erased by a boot, unless NVMRO is asserted.
    let mut flash = Mock::new();
    flash.sector[5] = 0x00;
    let (_, store, report) = booted_nvmro(&mut flash, true);
    assert!(store.is_none());
    assert!(!report.erased);
    assert_eq!(flash.erases, 0);
    let (_, _, report) = booted(&mut flash);
    assert!(report.erased, "NVMRO clear: the erase is due");
    // A sector with a valid record is never erased by a boot, NVMRO or not.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    let (_, store, report) = booted_nvmro(&mut flash, true);
    assert_eq!(store.map(|s| s.image()[TAG_AT]), Some(4));
    assert!(!report.erased);
    assert_eq!(flash.erases, 0);
}

#[test]
fn a_power_cut_between_the_erase_and_the_write_loses_the_image() {
    // Owner decision 10-25-2026: one sector, no second copy. The save of the 5th record erases
    // the full sector and the power goes before the first word lands: the next start-up finds
    // no record and loads the built image.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    flash.budget = Some(0);
    assert_eq!(
        log.save(&mut flash, &image(5), 0),
        Err(PersistError::Flash(FlashError::Programming))
    );
    assert_eq!(flash.erases, 1);
    assert_eq!(loaded_tag(&mut flash), None, "the stored image is gone");
}

#[test]
fn a_power_cut_during_the_erase_never_loads_a_damaged_image() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    // The erase is torn: the first half of the sector reads FFh, the rest is as it was.
    flash.torn_erase = true;
    assert_eq!(
        log.save(&mut flash, &image(5), 0),
        Err(PersistError::Flash(FlashError::Timeout))
    );
    let (mut log, store, report) = booted(&mut flash);
    let tag = store.map(|s| s.image()[TAG_AT]);
    assert!(
        matches!(tag, Some(4) | None),
        "only a whole record of the old log may load, got {tag:?}"
    );
    assert_eq!(report.loaded.is_some(), tag.is_some());
    // The log goes on: the next save erases (garbage or a full sector) and writes record one.
    let saved = log.save(&mut flash, &image(6), 0).unwrap();
    assert_eq!(saved.offset, if saved.erased { 0 } else { saved.offset });
    assert_eq!(loaded_tag(&mut flash), Some(6));
}

#[test]
fn the_highest_sequence_number_loads_and_no_record_follows_it() {
    let mut flash = Mock::new();
    let words = pack_words(&encode(SEQ_MAX, &image(5), 0));
    flash.program(0, &words).unwrap();
    let (mut log, store, report) = booted(&mut flash);
    assert_eq!(report.loaded, Some(SEQ_MAX));
    assert_eq!(store.unwrap().image()[TAG_AT], 5);
    assert_eq!(
        log.save(&mut flash, &image(6), 0),
        Err(PersistError::SequenceExhausted)
    );
    assert_eq!(
        loaded_tag(&mut flash),
        Some(5),
        "the stored record is untouched"
    );
}

#[test]
fn a_failed_boot_erase_is_reported_and_the_log_stays_usable() {
    // Garbage after the last slot and no valid record: the boot erases, the erase fails.
    let mut flash = Mock::new();
    flash.sector[SECTOR_LEN - 1] = 0x7F;
    flash.erase_fails = true;
    let (mut log, store, report) = booted(&mut flash);
    assert!(store.is_none());
    assert!(!report.erased);
    assert_eq!(
        report.housekeeping,
        Some(PersistError::Flash(FlashError::Timeout))
    );
    // The next save tries the erase again, and succeeds once the flash does.
    assert!(log.save(&mut flash, &image(1), 0).is_err());
    flash.erase_fails = false;
    let saved = log.save(&mut flash, &image(2), 0).unwrap();
    assert_eq!((saved.offset, saved.erased), (0, true));
    assert_eq!(loaded_tag(&mut flash), Some(2));
}

#[test]
fn restore_is_one_record_write_and_the_built_image_wins() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=3);
    let built = *FruStore::new().image();
    let saved = log.restore(&mut flash, &built, 0).unwrap();
    assert_eq!(saved.seq, 4);
    assert_eq!(flash.erases, 0, "no erase: the record is appended");
    assert_eq!(loaded_tag(&mut flash), Some(built[TAG_AT]));
    assert!(!log.pending());
}

#[test]
fn restore_interrupted_at_every_word_loads_the_previous_newest_or_the_built_image() {
    let built = *FruStore::new().image();
    let previous = 0xA5u8;
    assert_ne!(built[TAG_AT], previous);
    // 2 earlier saves and the previous image leave room: the restore appends, so a cut leaves
    // the previous image. With 3 earlier saves the sector is full and the restore erases it
    // first: a cut after the erase leaves no record at all (the single-sector trade-off), so
    // the built image is in use.
    for (saves, full) in [(2u8, false), (3, true)] {
        for cut_after in 0..=RECORD_WORDS {
            let mut flash = Mock::new();
            let (mut log, _, _) = booted(&mut flash);
            save_tags(&mut log, &mut flash, 1..=saves);
            log.save(&mut flash, &image(previous), 0).unwrap();
            assert_eq!(log.save_needs_erase(), full);
            flash.budget = Some(cut_after);
            let result = log.restore(&mut flash, &built, 0);
            flash.budget = None;
            let want = match (result.is_ok(), full) {
                (true, _) => Some(built[TAG_AT]),
                (false, false) => Some(previous),
                (false, true) => None,
            };
            assert_eq!(
                loaded_tag(&mut flash),
                want,
                "saves {saves}, cut after word {cut_after}"
            );
        }
    }
}

#[test]
fn restore_with_a_failed_or_torn_erase_loads_the_previous_newest_image() {
    let built = *FruStore::new().image();
    for torn in [false, true] {
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        // The sector is full (1..=4); the restore erases it.
        save_tags(&mut log, &mut flash, 1..=4);
        flash.torn_erase = torn;
        flash.erase_fails = !torn;
        assert!(log.restore(&mut flash, &built, 0).is_err(), "torn {torn}");
        flash.erase_fails = false;
        // The torn erase spared the last record (it lies in the second half of the sector).
        assert_eq!(loaded_tag(&mut flash), Some(4), "torn {torn}");
    }
}

#[test]
fn the_retry_delay_after_a_failed_slow_erase_runs_from_the_failure() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4); // the sector is full: the next save erases it
    let mut store = FruStore::new();
    store.write(TAG_AT, &[4]).unwrap();
    flash.erase_ms = 2_000; // longer than the delay, as a slow sector erase can be
    flash.erase_fails = true;
    assert!(log
        .poll(&mut flash, &mut store, 0, false, DELAY_MS, true)
        .is_none());
    flash.now = DELAY_MS;
    let first = log.poll(&mut flash, &mut store, DELAY_MS, false, DELAY_MS, true);
    assert!(matches!(first, Some(Err(_))));
    let failed_at = flash.now; // DELAY_MS + 2000: the clock after the erase
    flash.erase_fails = false;
    assert!(log
        .poll(
            &mut flash,
            &mut store,
            failed_at + DELAY_MS - 1,
            false,
            DELAY_MS,
            true
        )
        .is_none());
    assert_eq!(flash.erases, 1, "no second erase inside the delay");
    let retry = log.poll(
        &mut flash,
        &mut store,
        failed_at + DELAY_MS,
        false,
        DELAY_MS,
        true,
    );
    assert!(matches!(retry, Some(Ok(_))));
}

#[test]
fn an_erase_waits_for_a_quiet_bus_and_a_plain_save_does_not() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    assert!(log.save_needs_erase());
    let mut store = FruStore::new();
    store.write(TAG_AT, &[4]).unwrap();
    assert!(log
        .poll(&mut flash, &mut store, 0, false, DELAY_MS, false)
        .is_none());
    assert!(
        log.poll(&mut flash, &mut store, DELAY_MS, false, DELAY_MS, false)
            .is_none(),
        "busy bus: the erase waits, the write stays pending"
    );
    assert!(log.pending() && flash.erases == 0);
    let saved = log.poll(&mut flash, &mut store, DELAY_MS + 1, false, DELAY_MS, true);
    assert!(matches!(saved, Some(Ok(Saved { erased: true, .. }))));
    // A save that needs no erase goes out on a busy bus.
    assert!(!log.save_needs_erase());
    store.write(TAG_AT, &[5]).unwrap();
    assert!(log
        .poll(&mut flash, &mut store, 5 * DELAY_MS, false, DELAY_MS, false)
        .is_none());
    let saved = log.poll(&mut flash, &mut store, 6 * DELAY_MS, false, DELAY_MS, false);
    assert!(matches!(saved, Some(Ok(_))));
}

#[test]
fn a_driver_that_refuses_for_nvmro_leaves_flash_and_the_image_untouched() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    let before = flash.sector.clone();
    flash.nvmro = true;
    // Boot: nothing to erase, the image loads.
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image()[TAG_AT], 4);
    assert_eq!(report.housekeeping, None);
    // Restore and save need the erase of the full sector: refused, and nothing is written or
    // erased.
    let built = *FruStore::new().image();
    assert_eq!(
        log.restore(&mut flash, &built, 0),
        Err(PersistError::Flash(FlashError::NvmroAsserted))
    );
    assert_eq!(
        log.save(&mut flash, &image(9), 0),
        Err(PersistError::Flash(FlashError::NvmroAsserted))
    );
    assert_eq!(flash.sector, before);
    flash.nvmro = false;
    assert_eq!(loaded_tag(&mut flash), Some(4));
    // A damaged sector at boot: the housekeeping erase is refused the same way.
    let mut flash = Mock::new();
    flash.sector[5] = 0x00;
    flash.nvmro = true;
    let (mut log, store, report) = booted_nvmro(&mut flash, false);
    assert!(store.is_none());
    assert_eq!(
        report.housekeeping,
        Some(PersistError::Flash(FlashError::NvmroAsserted))
    );
    assert!(log.save(&mut flash, &image(1), 0).is_err());
}

#[test]
fn the_settings_byte_rides_in_the_version_word_and_reads_zero_in_an_older_record() {
    // An older record: the 16-bit version field is 0001h, so byte 11 is 00h = settings 00h, and a
    // record with settings 00h is byte-identical to it.
    let mut older = encode(3, &image(4), 0);
    assert_eq!(older[OFF_VERSION..OFF_IMAGE], [1, 0]);
    let mut flash = Mock::new();
    flash.sector[..RECORD_LEN].copy_from_slice(&older);
    let (_, store, _) = booted(&mut flash);
    assert_eq!(store.unwrap().nv_settings(), 0, "older record");
    // (settings written, settings read back): bits 0 and 1 are kept, the others masked off.
    for (written, read) in [(0, 0), (1, 1), (2, 2), (3, 3), (0xFC, 0), (0xFF, 3)] {
        let record = encode(5, &image(4), written);
        assert_eq!(record[OFF_VERSION], 1, "version byte");
        assert_eq!(record[OFF_SETTINGS], read, "settings {written:02X}");
        assert_eq!(classify(&record), Slot::Valid(5));
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        log.save(&mut flash, &image(4), written).unwrap();
        let store = booted(&mut flash).1.unwrap();
        assert_eq!(store.nv_settings(), read);
        assert!(!store.is_dirty());
    }
    older[OFF_SETTINGS] = 1; // the CRC no longer holds
    assert_eq!(classify(&older), Slot::Invalid);
}

#[test]
fn a_settings_change_marks_the_store_dirty_and_is_saved_with_the_image() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let mut store = FruStore::new();
    store.set_nv_settings(0);
    assert!(!store.is_dirty(), "no change, no mark");
    store.set_nv_settings(0b10);
    assert!(store.is_dirty());
    assert!(log
        .poll(&mut flash, &mut store, 0, false, DELAY_MS, true)
        .is_none());
    let saved = log.poll(&mut flash, &mut store, DELAY_MS, false, DELAY_MS, true);
    assert!(matches!(saved, Some(Ok(_))));
    assert_eq!(booted(&mut flash).1.unwrap().nv_settings(), 0b10);
}

#[test]
fn the_store_region_check_accepts_sector_2_and_names_every_other_layout() {
    const BOOT_END: u32 = 0x0800_8000;
    const TEXT: u32 = 0x0800_C000;
    // (start, end, boot_end, app_start, expected)
    type Case = (u32, u32, u32, u32, Result<u32, RegionError>);
    let cases: [Case; 11] = [
        (0x0800_8000, 0x0800_C000, BOOT_END, TEXT, Ok(STORE_LEN)),
        (0x0800_8000, 0x0800_C000, 0x0800_7C00, TEXT, Ok(STORE_LEN)),
        (
            0x0800_C000,
            0x0801_0000,
            BOOT_END,
            0x0801_0000,
            Err(RegionError::Shifted),
        ),
        (
            0x0800_4000,
            0x0800_8000,
            BOOT_END,
            0x0800_8000,
            Err(RegionError::Shifted),
        ),
        (
            0x0800_8000,
            0x0800_B000,
            BOOT_END,
            TEXT,
            Err(RegionError::TooShort),
        ),
        (
            0x0800_8000,
            0x0800_8000,
            BOOT_END,
            TEXT,
            Err(RegionError::TooShort),
        ),
        (
            0x0800_8000,
            0x0801_0000,
            BOOT_END,
            0x0801_0000,
            Err(RegionError::TooLong),
        ),
        (
            0x0800_8200,
            0x0800_C200,
            BOOT_END,
            0x0800_C200,
            Err(RegionError::NotSectorAligned),
        ),
        (
            0x0800_8000,
            0x0800_C000,
            0x0800_8400,
            TEXT,
            Err(RegionError::VectorsReachIn),
        ),
        (
            0x0800_8000,
            0x0800_C000,
            BOOT_END,
            0x0800_A000,
            Err(RegionError::ProgramInside),
        ),
        (
            0x0800_8000,
            0x0800_C000,
            BOOT_END,
            0x0800_BFFF,
            Err(RegionError::ProgramInside),
        ),
    ];
    for (start, end, boot_end, app_start, want) in cases {
        assert_eq!(
            store_region_ok(start, end, boot_end, app_start),
            want,
            "region {start:08X}..{end:08X}, bootloader end {boot_end:08X}, runtime {app_start:08X}"
        );
    }
}

#[test]
fn a_16_kb_sector_holds_thirty_one_records_of_the_default_image() {
    assert_eq!(STORE_LEN, 16 * 1024);
    assert_eq!(
        RECORDS_PER_SECTOR,
        STORE_LEN as usize / RECORD_LEN,
        "the count the module comment states"
    );
    if SIZE == 512 {
        assert_eq!((RECORD_LEN, RECORDS_PER_SECTOR), (528, 31));
    }
}
