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
    sectors: [Vec<u8>; 2],
    /// Words that may still be programmed; None = unlimited. At 0 the next word fails.
    budget: Option<usize>,
    words_programmed: usize,
    /// When `words_programmed` reaches this count the word just written stays and the call
    /// fails with Timeout (the busy flag cleared after the poll bound ran out).
    late_error: Option<usize>,
    /// The next erase sets only the first half of the sector to FFh and fails (power cut).
    torn_erase: bool,
    erases: Vec<LogSector>,
    /// The clock `FlashLog::millis` reports; an erase advances it by `erase_ms` (a slow erase).
    now: u32,
    erase_ms: u32,
    /// Every erase fails with Timeout and leaves the sector as it was.
    erase_fails: bool,
    /// NVMRO is asserted: every erase and program is refused and touches nothing.
    nvmro: bool,
}

impl Mock {
    fn new() -> Mock {
        Mock {
            sectors: [vec![ERASED; SECTOR_LEN], vec![ERASED; SECTOR_LEN]],
            budget: None,
            words_programmed: 0,
            late_error: None,
            torn_erase: false,
            erases: Vec::new(),
            now: 0,
            erase_ms: 0,
            erase_fails: false,
            nvmro: false,
        }
    }
}

impl FlashLog for Mock {
    fn sector_len(&self) -> usize {
        self.sectors[0].len()
    }

    fn read(&self, sector: LogSector, offset: usize, out: &mut [u8]) -> Result<(), FlashError> {
        let end = offset.checked_add(out.len()).ok_or(FlashError::Range)?;
        let bytes = self.sectors[sector.index()]
            .get(offset..end)
            .ok_or(FlashError::Range)?;
        out.copy_from_slice(bytes);
        Ok(())
    }

    fn program(
        &mut self,
        sector: LogSector,
        offset: usize,
        words: &[u32],
    ) -> Result<(), FlashError> {
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
            let target = self.sectors[sector.index()]
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

    fn erase(&mut self, sector: LogSector) -> Result<(), FlashError> {
        if self.nvmro {
            return Err(FlashError::NvmroAsserted);
        }
        self.erases.push(sector);
        self.now = self.now.wrapping_add(self.erase_ms);
        if self.erase_fails {
            return Err(FlashError::Timeout);
        }
        let bytes = &mut self.sectors[sector.index()];
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
    assert_eq!(report.erased, [None, None]);
    assert_eq!(log.status().newest_seq, None);
    assert!(flash.erases.is_empty());
}

#[test]
fn save_then_load_returns_the_image_and_starts_at_sequence_1() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    let saved = log.save(&mut flash, &image(0x11), 0).unwrap();
    assert_eq!(
        saved,
        Saved {
            sector: LogSector::A,
            offset: 0,
            seq: 1,
            erased: None
        }
    );
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image(), &image(0x11));
    assert_eq!(report.loaded, Some((LogSector::A, 1)));
}

#[test]
fn the_newest_sequence_wins_across_both_sectors() {
    // 4 records fill A (1..=4), the next 4 fill B (5..=8), the 9th erases A and lands there.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    for (count, want) in [
        (5u8, (LogSector::B, 5u32)),
        (8, (LogSector::B, 8)),
        (9, (LogSector::A, 9)),
    ] {
        let done = u8::try_from(log.status().newest_seq.unwrap_or(0)).unwrap();
        save_tags(&mut log, &mut flash, done + 1..=count);
        let (_, store, report) = booted(&mut flash);
        assert_eq!(report.loaded, Some(want), "after {count} saves");
        assert_eq!(store.unwrap().image()[TAG_AT], count);
    }
}

#[test]
fn a_full_sector_switches_and_marks_the_old_one_erase_pending() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=4);
    let s = log.status();
    assert_eq!(
        (s.active, s.records, s.erase_pending),
        (LogSector::A, [4, 0], false)
    );
    let saved = log.save(&mut flash, &image(5), 0).unwrap();
    assert_eq!(
        (saved.sector, saved.offset, saved.erased),
        (LogSector::B, 0, None)
    );
    let s = log.status();
    assert_eq!(
        (s.active, s.records, s.erase_pending),
        (LogSector::B, [4, 1], true)
    );
    assert!(flash.erases.is_empty(), "the switch alone erases nothing");
    // B fills, then the next save erases A first and reports it.
    save_tags(&mut log, &mut flash, 6..=8);
    let saved = log.save(&mut flash, &image(9), 0).unwrap();
    assert_eq!(
        (saved.sector, saved.erased),
        (LogSector::A, Some(LogSector::A))
    );
    assert_eq!(flash.erases, [LogSector::A]);
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
            let commit = &flash.sectors[0][RECORD_LEN..RECORD_LEN + WORD];
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
    flash.sectors[0][RECORD_LEN + OFF_IMAGE + 10] ^= 0x20;
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image()[TAG_AT], 1);
    assert_eq!(report.loaded, Some((LogSector::A, 1)));
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
    assert_eq!((s.saves, s.records, s.pending), (1, [1, 0], false));
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
fn boot_erases_the_stale_sector_only_when_the_active_one_is_over_half_full() {
    // After 5 saves B holds one record: under half full, A stays.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=5);
    let (_, _, report) = booted(&mut flash);
    assert_eq!(report.erased, [None, None]);
    assert!(flash.erases.is_empty());
    // After 7 saves B holds three of four: A is erased, the newest record survives.
    save_tags(&mut log, &mut flash, 6..=7);
    let (log, store, report) = booted(&mut flash);
    assert_eq!(report.erased, [Some(LogSector::A), None]);
    assert_eq!(flash.erases, [LogSector::A]);
    assert_eq!(store.unwrap().image()[TAG_AT], 7);
    assert!(!log.status().erase_pending);
}

#[test]
fn boot_clears_sectors_that_hold_no_valid_record() {
    let mut flash = Mock::new();
    flash.sectors[0][5] = 0x00; // garbage in A: an interrupted write or erase
    let (log, store, report) = booted(&mut flash);
    assert!(store.is_none());
    assert_eq!(report.erased, [Some(LogSector::A), None]);
    assert_eq!(report.skipped, 1);
    assert!(flash.sectors[0].iter().all(|&b| b == ERASED));
    assert_eq!(log.status().newest_seq, None);
}

#[test]
fn a_sector_too_small_for_one_record_is_refused() {
    let mut flash = Mock::new();
    flash.sectors = [vec![ERASED; RECORD_LEN - 4], vec![ERASED; RECORD_LEN - 4]];
    assert_eq!(
        Persist::new().boot(&mut flash, false).err(),
        Some(PersistError::SectorGeometry)
    );
}

#[test]
fn garbage_after_the_last_record_leaves_no_room_in_that_sector() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=1);
    flash.sectors[0][SECTOR_LEN - 1] = 0x7F; // an interrupted erase left a cell programmed
    let (mut log, _, _) = booted(&mut flash);
    let saved = log.save(&mut flash, &image(2), 0).unwrap();
    assert_eq!(
        (saved.sector, saved.erased),
        (LogSector::B, None),
        "A is treated as full"
    );
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
        assert_eq!(report.erased, [None, None], "hole {hole}: nothing erased");
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
    assert_eq!(report.loaded, Some((LogSector::A, 2)));
    assert_eq!(store.unwrap().image()[TAG_AT], 2, "the later write wins");
}

#[test]
fn boot_with_nvmro_asserted_erases_nothing() {
    // (saves before the boot, garbage byte in sector A): both would make a boot erase a sector.
    for (saves, garbage) in [(7u8, false), (0, true)] {
        let mut flash = Mock::new();
        let (mut log, _, _) = booted(&mut flash);
        save_tags(&mut log, &mut flash, 1..=saves);
        if garbage {
            flash.sectors[0][5] = 0x00;
        }
        let (_, store, report) = booted_nvmro(&mut flash, true);
        assert_eq!(report.erased, [None, None], "saves {saves}");
        assert!(flash.erases.is_empty(), "saves {saves}");
        assert_eq!(
            store.map(|s| s.image()[TAG_AT]),
            (saves > 0).then_some(saves)
        );
        let (_, _, report) = booted(&mut flash);
        assert_ne!(report.erased, [None, None], "NVMRO clear: the erase is due");
    }
}

#[test]
fn a_power_cut_during_the_switch_erase_keeps_the_newest_record() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=8);
    // B is full and active; the 9th save erases A first, and the power goes in that erase.
    flash.torn_erase = true;
    assert_eq!(
        log.save(&mut flash, &image(9), 0),
        Err(PersistError::Flash(FlashError::Timeout))
    );
    let (mut log, store, report) = booted(&mut flash);
    assert_eq!(report.loaded, Some((LogSector::B, 8)));
    assert_eq!(store.unwrap().image()[TAG_AT], 8);
    assert_eq!(
        report.erased,
        [Some(LogSector::A), None],
        "the torn sector is erased again"
    );
    let saved = log.save(&mut flash, &image(9), 0).unwrap();
    assert_eq!(
        (saved.sector, saved.offset, saved.seq),
        (LogSector::A, 0, 9)
    );
    assert_eq!(loaded_tag(&mut flash), Some(9));
}

#[test]
fn the_highest_sequence_number_loads_and_no_record_follows_it() {
    let mut flash = Mock::new();
    let words = pack_words(&encode(SEQ_MAX, &image(5), 0));
    flash.program(LogSector::A, 0, &words).unwrap();
    let (mut log, store, report) = booted(&mut flash);
    assert_eq!(report.loaded, Some((LogSector::A, SEQ_MAX)));
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
fn a_failed_boot_erase_keeps_the_loaded_image_and_reports_the_error() {
    // Seven saves: B holds three of four records, so the boot erases A, and that erase fails.
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=7);
    flash.erase_fails = true;
    let (mut log, store, report) = booted(&mut flash);
    assert_eq!(
        store.unwrap().image()[TAG_AT],
        7,
        "the image is not discarded"
    );
    assert_eq!(report.loaded, Some((LogSector::B, 7)));
    assert_eq!(report.erased, [None, None]);
    assert_eq!(
        report.housekeeping,
        Some(PersistError::Flash(FlashError::Timeout))
    );
    assert!(log.status().erase_pending, "A still waits for its erase");
    // Persistence stays usable: the next save goes to B.
    let saved = log.save(&mut flash, &image(8), 0).unwrap();
    assert_eq!((saved.sector, saved.seq), (LogSector::B, 8));
    // With no valid record the failed erase is reported the same way.
    let mut flash = Mock::new();
    flash.sectors[0][5] = 0x00;
    flash.erase_fails = true;
    let (_, store, report) = booted(&mut flash);
    assert!(store.is_none());
    assert_eq!(
        report.housekeeping,
        Some(PersistError::Flash(FlashError::Timeout))
    );
}

#[test]
fn restore_is_one_record_write_and_the_built_image_wins() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=6);
    let built = *FruStore::new().image();
    let saved = log.restore(&mut flash, &built, 0).unwrap();
    assert_eq!(saved.seq, 7);
    assert!(flash.erases.is_empty(), "no erase: the record is appended");
    assert_eq!(loaded_tag(&mut flash), Some(built[TAG_AT]));
    assert!(!log.pending());
}

#[test]
fn restore_interrupted_at_every_word_loads_the_previous_newest_or_the_built_image() {
    let built = *FruStore::new().image();
    let previous = 0xA5u8;
    assert_ne!(built[TAG_AT], previous);
    // 3 saves leave room in A; 4 fill A so the restore switches to B; 8 fill both sectors so the
    // restore erases A first.
    for saves in [3u8, 4, 8] {
        for cut_after in 0..=RECORD_WORDS {
            let mut flash = Mock::new();
            let (mut log, _, _) = booted(&mut flash);
            save_tags(&mut log, &mut flash, 1..saves);
            log.save(&mut flash, &image(previous), 0).unwrap();
            flash.budget = Some(cut_after);
            let result = log.restore(&mut flash, &built, 0);
            flash.budget = None;
            let want = if result.is_ok() {
                built[TAG_AT]
            } else {
                previous
            };
            assert_eq!(
                loaded_tag(&mut flash),
                Some(want),
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
        // Both sectors full: A holds 1..=4, B holds 5..=8 and is active; the restore erases A.
        save_tags(&mut log, &mut flash, 1..=8);
        flash.torn_erase = torn;
        flash.erase_fails = !torn;
        assert!(log.restore(&mut flash, &built, 0).is_err(), "torn {torn}");
        flash.erase_fails = false;
        assert_eq!(loaded_tag(&mut flash), Some(8), "torn {torn}");
    }
}

#[test]
fn the_retry_delay_after_a_failed_slow_erase_runs_from_the_failure() {
    let mut flash = Mock::new();
    let (mut log, _, _) = booted(&mut flash);
    save_tags(&mut log, &mut flash, 1..=8); // both sectors full: the next save erases A
    let mut store = FruStore::new();
    store.write(TAG_AT, &[4]).unwrap();
    flash.erase_ms = 2_000; // longer than the delay, as a 128 KB sector erase can be
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
    assert_eq!(flash.erases.len(), 1, "no second erase inside the delay");
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
    save_tags(&mut log, &mut flash, 1..=8);
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
    assert!(log.pending() && flash.erases.is_empty());
    let saved = log.poll(&mut flash, &mut store, DELAY_MS + 1, false, DELAY_MS, true);
    assert!(matches!(
        saved,
        Some(Ok(Saved {
            erased: Some(_),
            ..
        }))
    ));
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
    save_tags(&mut log, &mut flash, 1..=7);
    let before = flash.sectors.clone();
    flash.nvmro = true;
    // Boot: the erase is refused, the image still loads.
    let (_, store, report) = booted(&mut flash);
    assert_eq!(store.unwrap().image()[TAG_AT], 7);
    assert_eq!(
        report.housekeeping,
        Some(PersistError::Flash(FlashError::NvmroAsserted))
    );
    // Restore and save are refused, and nothing is written or erased.
    let built = *FruStore::new().image();
    assert_eq!(
        log.restore(&mut flash, &built, 0),
        Err(PersistError::Flash(FlashError::NvmroAsserted))
    );
    assert_eq!(
        log.save(&mut flash, &image(9), 0),
        Err(PersistError::Flash(FlashError::NvmroAsserted))
    );
    assert_eq!(flash.sectors, before);
    flash.nvmro = false;
    assert_eq!(loaded_tag(&mut flash), Some(7));
}

#[test]
fn the_settings_byte_rides_in_the_version_word_and_reads_zero_in_an_older_record() {
    // An older record: the 16-bit version field is 0001h, so byte 11 is 00h = settings 00h, and a
    // record with settings 00h is byte-identical to it.
    let mut older = encode(3, &image(4), 0);
    assert_eq!(older[OFF_VERSION..OFF_IMAGE], [1, 0]);
    let mut flash = Mock::new();
    flash.sectors[0][..RECORD_LEN].copy_from_slice(&older);
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
