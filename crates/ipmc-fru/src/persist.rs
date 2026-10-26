//! persist.rs - keeps the FRU scratchpad on flash: the stored record, the load at boot, the save
//! after a Write FRU Data, the one-sector log and the dirty/delay rule.
//!
//! The firmware implements `FlashLog` over one erase sector (the 16 KB sector 1 of the
//! STM32F401RE, RM0368 Table 5, `STORE_START`..`STORE_LEN`); everything else is here and runs on
//! the host in the tests. The log is that one sector. A save appends one record to it; when the
//! next record does not fit, the save erases the sector and writes the newest image as the first
//! record. Boot takes the valid record with the highest sequence number.
//!
//! Single-sector trade-off (owner decision 10-25-2026, no second copy): a power cut between that
//! erase and that write loses the stored image, and the next start-up loads the built image. The
//! erase comes after `Persist::save_needs_erase` and happens once per `RECORDS_PER_SECTOR`
//! saves: a record is `RECORD_LEN` bytes, so the 16 KB sector holds 16384 / RECORD_LEN = 31
//! records (RECORD_LEN is 528 bytes at IPMC_FRU_SIZE 512) before the erase.
//!
//! Record (little-endian, every field word-aligned, `RECORD_LEN` bytes, SIZE = IPMC_FRU_SIZE):
//!
//! ```text
//! offset  size  field
//!   0      4    commit word 55464931h ("1IFU" in memory order), programmed LAST
//!   4      4    sequence number: starts at 1, +1 per record, never 0 or FFFFFFFFh
//!   8      2    image length (= SIZE)
//!  10      1    record format version (1)
//!  11      1    Tier-3 non-volatile settings (bit 0 Default-Activation-Locked, bit 1
//!               Commanded-Deactivation-Ignored, other bits 0)
//!  12     SIZE  the FRU image
//!  12+SIZE 4    CRC-32 (IEEE 802.3, ipmc_wire::crc32) over bytes 4 .. 12+SIZE
//! ```
//!
//! The settings byte takes the high byte of what an earlier build wrote as a 16-bit version field
//! of value 1: a record written before the byte existed reads as settings 00h, and a record with
//! settings 00h is byte-identical to one of that build (Tier-1 and Tier-2 images never write a
//! non-zero value). A build from before the byte rejects a record whose settings are not 00h.
//!
//! A record from an older image size (another length field and record length) is not valid for
//! this build: every slot is invalid and the built image loads. `Persist::boot` then finds no valid
//! record in a sector that is not blank and, with NVMRO deasserted, erases it at once (the old
//! content is dropped at that boot, `BootReport::erased`); with NVMRO asserted it erases nothing
//! (VITA 46.11 Rec 4.1.10-1), the old records stay in the sector and the first save after NVMRO
//! clears erases them. A drive-copy flash erases the sector as well; an SWD flash keeps it, so
//! the boot after it is where the old content goes.
//!
//! A slot is valid when the commit word, the length, the version, the sequence and the CRC all
//! check. Erased flash reads FFh. The scan reads every slot of the sector: the next record goes
//! after the last slot that is not all FFh, so an all-FFh slot left behind by a save that failed
//! before it programmed a bit is never taken for the end of the log (a record after it still
//! counts). A slot written only in part (power cut) fails a check, is skipped, and the next
//! record goes after it. The words after the commit word are programmed first and the commit
//! word last, so a power cut never leaves a slot that looks valid and is not.
//!
//! Every sequence number goes to at most one slot: a save that fails after programming started
//! uses its number up, so a record the driver reported as failed but that did land cannot share
//! its number with the next one.
//!
//! NVMRO asserting between the erase and the program of a save leaves the sector erased with the
//! image in RAM: the same window as the accepted power cut above, and the write is saved again
//! when NVMRO clears.
//!
//! Power cut at any word of an append: the previous record is still valid and wins (tested for
//! every word of a record). The erase of the full sector is the one operation that can lose the
//! image (see above).

use crate::records::SIZE;
use crate::store::FruStore;
use ipmc_wire::crc32;

/// WORD -- bytes in one flash word: the unit the flash is programmed in.
pub const WORD: usize = 4;
/// COMMIT_WORD -- offset 0 of a valid record (the ASCII "1IFU" read as one little-endian word).
pub const COMMIT_WORD: u32 = 0x5546_4931;
/// RECORD_VERSION -- the record format described in the file header (byte 10).
pub const RECORD_VERSION: u8 = 1;
/// NV_SETTINGS_MASK -- the settings bits defined (the file header); a stored byte is masked with
/// it when loaded and a saved byte is masked with it before it is written.
pub const NV_SETTINGS_MASK: u8 = 0b11;
/// SEQ_MAX -- the highest sequence number a record carries; FFFFFFFFh reads as erased flash.
const SEQ_MAX: u32 = u32::MAX - 1;
/// ERASED -- the value of an erased flash byte.
const ERASED: u8 = 0xFF;

// Record field offsets (the table in the file header).
const OFF_COMMIT: usize = 0;
const OFF_SEQ: usize = 4;
const OFF_LEN: usize = 8;
const OFF_VERSION: usize = 10;
const OFF_SETTINGS: usize = 11;
const OFF_IMAGE: usize = 12;
const OFF_CRC: usize = OFF_IMAGE + SIZE;
/// RECORD_LEN -- bytes of one record: the fields above, which are word-aligned because SIZE is a
/// multiple of 8 (ipmc-defs guard).
pub const RECORD_LEN: usize = OFF_CRC + WORD;
/// RECORD_WORDS -- flash words of one record.
pub const RECORD_WORDS: usize = RECORD_LEN / WORD;
const _: () = assert!(
    RECORD_LEN.is_multiple_of(WORD),
    "a record is a whole number of words"
);
const _: () = assert!(
    SIZE <= u16::MAX as usize,
    "the image length field is 16 bits"
);
const _: () = assert!(OFF_COMMIT == 0 && OFF_SEQ == WORD && OFF_LEN == 2 * WORD);
const _: () = assert!(OFF_VERSION == OFF_LEN + 2 && OFF_SETTINGS == OFF_VERSION + 1);
const _: () = assert!(OFF_IMAGE == OFF_SETTINGS + 1);

/// BLANK_CHUNK -- bytes read at a time when a range is checked for erased flash (defs.rs
/// IPMC_FRU_SCAN_CHUNK, guarded in ipmc-defs: a multiple of 4, 16..=272).
const BLANK_CHUNK: usize = ipmc_defs::IPMC_FRU_SCAN_CHUNK;

/// STORE_START -- first byte of the store sector: sector 1 of the STM32F401RE (RM0368 Table 5).
pub const STORE_START: u32 = 0x0800_4000;
/// STORE_LEN -- bytes of the store sector: sector 1 is 16 KB (RM0368 Table 5).
pub const STORE_LEN: u32 = 16 * 1024;
/// RECORDS_PER_SECTOR -- whole records the sector holds before a save erases it.
pub const RECORDS_PER_SECTOR: usize = STORE_LEN as usize / RECORD_LEN;

/// RegionError -- why the linked store region is not the sector the log is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionError {
    /// The region start is not on a 16 KB boundary.
    NotSectorAligned,
    /// The region does not start at the store sector (RM0368 Table 5, sector 1).
    Shifted,
    /// The region is shorter than the sector.
    TooShort,
    /// The region is longer than the sector.
    TooLong,
    /// The vector table reaches into the region.
    VectorsReachIn,
    /// The program starts inside the region.
    ProgramInside,
}

impl RegionError {
    /// text -- text for the console.
    pub const fn text(self) -> &'static str {
        match self {
            RegionError::NotSectorAligned => "the store region is not sector-aligned",
            RegionError::Shifted => "the store region is not flash sector 1",
            RegionError::TooShort => "the store region is shorter than sector 1",
            RegionError::TooLong => "the store region is longer than sector 1",
            RegionError::VectorsReachIn => "the vector table reaches into the store region",
            RegionError::ProgramInside => "the program starts inside the store region",
        }
    }
}

/// store_region_ok -- checks the linker's store region `start..end` against the sector the log
/// erases: it must be exactly sector 1 (`STORE_START`, `STORE_LEN`, aligned to its size), the
/// vector table must end at or before `start` (`vectors_end`) and the program must start at or
/// after `end` (`text_start`), so an erase of the store never touches code. Returns the region
/// length. The firmware passes the values of the linker symbols (memory.x, link.x).
pub const fn store_region_ok(
    start: u32,
    end: u32,
    vectors_end: u32,
    text_start: u32,
) -> Result<u32, RegionError> {
    if !start.is_multiple_of(STORE_LEN) {
        return Err(RegionError::NotSectorAligned);
    }
    if start != STORE_START {
        return Err(RegionError::Shifted);
    }
    let len = end.wrapping_sub(start);
    if end < start || len < STORE_LEN {
        return Err(RegionError::TooShort);
    }
    if len > STORE_LEN {
        return Err(RegionError::TooLong);
    }
    if vectors_end > start {
        return Err(RegionError::VectorsReachIn);
    }
    if text_start < end {
        return Err(RegionError::ProgramInside);
    }
    Ok(len)
}

/// FlashError -- why a flash operation failed (the firmware driver maps its status flags here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashError {
    /// The controller reported a programming parallelism or operation error.
    Programming,
    /// A programming address or length that is not word-aligned.
    Alignment,
    /// The sector is write-protected, or the controller is locked.
    Protection,
    /// The controller reported a programming sequence error.
    Sequence,
    /// A busy flag did not clear within its bound.
    Timeout,
    /// What was read back is not what was written or erased.
    Verify,
    /// An offset or length lies outside the sector.
    Range,
    /// NVMRO is asserted: the driver refuses to start an erase or a program (VITA 46.11 Rule 8-3,
    /// Rec 4.1.10-1); the flash is untouched.
    NvmroAsserted,
    /// The driver did not start an erase because a frame had been accepted on an IPMB bus (the
    /// firmware checks after it takes the slaves off the bus); nothing was touched. Not a
    /// failure: the write stays pending and the next pass retries.
    Deferred,
}

impl FlashError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            FlashError::Programming => "Programming",
            FlashError::Alignment => "Alignment",
            FlashError::Protection => "Protection",
            FlashError::Sequence => "Sequence",
            FlashError::Timeout => "Timeout",
            FlashError::Verify => "Verify",
            FlashError::Range => "Range",
            FlashError::NvmroAsserted => "NvmroAsserted",
            FlashError::Deferred => "Deferred",
        }
    }
}

/// PersistError -- why a log operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistError {
    /// The flash driver failed.
    Flash(FlashError),
    /// The sector is shorter than one record, or not a whole number of words.
    SectorGeometry,
    /// The sequence number reached its maximum; no further record can be ordered.
    SequenceExhausted,
}

impl PersistError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            PersistError::Flash(e) => e.name(),
            PersistError::SectorGeometry => "SectorGeometry",
            PersistError::SequenceExhausted => "SequenceExhausted",
        }
    }
}

impl From<FlashError> for PersistError {
    fn from(error: FlashError) -> Self {
        PersistError::Flash(error)
    }
}

/// FlashLog -- the log sector, as the firmware's flash driver offers it.
pub trait FlashLog {
    /// Bytes of the log sector.
    fn sector_len(&self) -> usize;
    /// Reads `out.len()` bytes at `offset` of the sector; Err(Range) beyond the sector.
    fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), FlashError>;
    /// Programs whole words; `offset` and `words.len() * 4` within the sector, word-aligned.
    /// The words land little-endian in memory; the location must be erased.
    fn program(&mut self, offset: usize, words: &[u32]) -> Result<(), FlashError>;
    /// Erases the sector: every byte reads FFh afterwards.
    fn erase(&mut self) -> Result<(), FlashError>;
    /// The clock in milliseconds (wrapping), read when `Persist` needs the time after a flash
    /// operation: an erase can take seconds, so a time read before it is stale.
    fn millis(&self) -> u32;
}

/// Slot -- what one record-sized region of a sector holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// Every byte is FFh.
    Blank,
    /// A record that passes every check, with its sequence number.
    Valid(u32),
    /// Anything else: a partly written record or garbage.
    Invalid,
}

/// field -- the `N` bytes at `at` of a record buffer (zeros if the range lies outside, which a
/// constant offset of this file never does); callers decode with `from_le_bytes`.
fn field<const N: usize>(record: &[u8; RECORD_LEN], at: usize) -> [u8; N] {
    let mut bytes = [0u8; N];
    if let Some(source) = record.get(at..at + N) {
        bytes.copy_from_slice(source);
    }
    bytes
}

/// classify -- checks one record-sized region (the file header lists the checks).
fn classify(record: &[u8; RECORD_LEN]) -> Slot {
    if record.iter().all(|&b| b == ERASED) {
        return Slot::Blank;
    }
    let seq = u32::from_le_bytes(field(record, OFF_SEQ));
    let crc_covered = record.get(OFF_SEQ..OFF_CRC).unwrap_or(&[]);
    let valid = u32::from_le_bytes(field(record, OFF_COMMIT)) == COMMIT_WORD
        && usize::from(u16::from_le_bytes(field(record, OFF_LEN))) == SIZE
        && u8::from_le_bytes(field(record, OFF_VERSION)) == RECORD_VERSION
        && seq != 0
        && seq != u32::MAX
        && u32::from_le_bytes(field(record, OFF_CRC)) == crc32(crc_covered);
    if valid {
        Slot::Valid(seq)
    } else {
        Slot::Invalid
    }
}

/// encode -- the record of `image` and the Tier-3 `settings` byte (masked with
/// `NV_SETTINGS_MASK`) with sequence number `seq`, in memory order.
fn encode(seq: u32, image: &[u8; SIZE], settings: u8) -> [u8; RECORD_LEN] {
    let mut record = [0u8; RECORD_LEN];
    record[OFF_COMMIT..OFF_COMMIT + WORD].copy_from_slice(&COMMIT_WORD.to_le_bytes());
    record[OFF_SEQ..OFF_SEQ + WORD].copy_from_slice(&seq.to_le_bytes());
    // SIZE fits 16 bits (assertion above).
    record[OFF_LEN..OFF_LEN + 2].copy_from_slice(&(SIZE as u16).to_le_bytes());
    record[OFF_VERSION] = RECORD_VERSION;
    record[OFF_SETTINGS] = settings & NV_SETTINGS_MASK;
    record[OFF_IMAGE..OFF_CRC].copy_from_slice(image);
    let crc = crc32(&record[OFF_SEQ..OFF_CRC]);
    record[OFF_CRC..OFF_CRC + WORD].copy_from_slice(&crc.to_le_bytes());
    record
}

/// pack_words -- the record as the words the flash is programmed with (little-endian).
fn pack_words(record: &[u8; RECORD_LEN]) -> [u32; RECORD_WORDS] {
    let mut words = [0u32; RECORD_WORDS];
    let (chunks, _) = record.as_chunks::<WORD>();
    for (word, chunk) in words.iter_mut().zip(chunks) {
        *word = u32::from_le_bytes(*chunk);
    }
    words
}

/// SectorInfo -- what the last scan or operation found out about one sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SectorInfo {
    /// Offset of the first blank slot with only erased flash after it; None when the sector has
    /// no room for another record (full, or garbage after the last record).
    next_free: Option<usize>,
    /// Highest sequence number among the valid records.
    newest: Option<u32>,
    /// Valid records.
    valid: u32,
    /// Slots that are neither blank nor valid.
    skipped: u32,
}

impl SectorInfo {
    /// BLANK -- a freshly erased sector.
    const BLANK: SectorInfo = SectorInfo {
        next_free: Some(0),
        newest: None,
        valid: 0,
        skipped: 0,
    };

    /// is_blank -- the sector holds nothing: no record, no partial record, all erased.
    fn is_blank(&self) -> bool {
        *self == SectorInfo::BLANK
    }
}

/// is_erased_from -- true when every byte of the sector from `start` to its end reads FFh.
fn is_erased_from<F: FlashLog>(flash: &F, start: usize) -> Result<bool, FlashError> {
    let mut chunk = [0u8; BLANK_CHUNK];
    let mut offset = start;
    while offset < flash.sector_len() {
        let len = BLANK_CHUNK.min(flash.sector_len() - offset);
        let part = chunk.get_mut(..len).ok_or(FlashError::Range)?;
        flash.read(offset, part)?;
        if part.iter().any(|&b| b != ERASED) {
            return Ok(false);
        }
        offset += len;
    }
    Ok(true)
}

/// read_slot -- the record-sized region at `offset` of the sector.
fn read_slot<F: FlashLog>(flash: &F, offset: usize) -> Result<[u8; RECORD_LEN], FlashError> {
    let mut record = [0u8; RECORD_LEN];
    flash.read(offset, &mut record)?;
    Ok(record)
}

/// scan_sector -- reads every slot of the sector, bounded by the sector length over RECORD_LEN.
/// The next free slot is the one after the last slot that is not blank; a blank slot before it
/// (a save that failed before programming a bit, then a save after it) is a hole, never reused.
/// The bytes past the last whole slot must read erased too, else the sector has no usable room
/// (an interrupted erase leaves such garbage).
fn scan_sector<F: FlashLog>(flash: &F) -> Result<SectorInfo, FlashError> {
    let slots = flash.sector_len() / RECORD_LEN;
    let mut info = SectorInfo {
        next_free: None,
        ..SectorInfo::BLANK
    };
    let mut used_end = 0;
    for slot in 0..slots {
        let offset = slot * RECORD_LEN;
        match classify(&read_slot(flash, offset)?) {
            Slot::Blank => continue,
            Slot::Valid(seq) => {
                info.valid += 1;
                info.newest = Some(info.newest.map_or(seq, |newest| newest.max(seq)));
            }
            Slot::Invalid => info.skipped += 1,
        }
        used_end = offset + RECORD_LEN;
    }
    if used_end + RECORD_LEN <= flash.sector_len() && is_erased_from(flash, slots * RECORD_LEN)? {
        info.next_free = Some(used_end);
    }
    Ok(info)
}

/// Saved -- what one save did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Its offset in the sector.
    pub offset: usize,
    /// Its sequence number.
    pub seq: u32,
    /// The sector was full and was erased first; the record is the first one in it.
    pub erased: bool,
}

/// BootReport -- what `boot` found and did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootReport {
    /// The sequence number of the record loaded; None when no valid record exists.
    pub loaded: Option<u32>,
    /// The sector was erased at boot (it held no valid record, only damage).
    pub erased: bool,
    /// Slots skipped as partly written or damaged.
    pub skipped: u32,
    /// The housekeeping erase that failed, if one did. A failed erase of a sector without a
    /// valid record loses nothing, and the next save erases again as needed.
    pub housekeeping: Option<PersistError>,
}

/// Status -- the log state the console shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Valid records in the sector.
    pub records: u32,
    /// Highest sequence number in the log; None while the log holds no record.
    pub newest_seq: Option<u32>,
    /// Bytes still free in the sector (a whole number of records is smaller).
    pub bytes_free: usize,
    /// A write is waiting for its delay or for NVMRO to clear.
    pub pending: bool,
    /// SysTick time of the last successful save; None before the first.
    pub last_save_ms: Option<u32>,
    /// The sector has no room for another record: the next save erases it first.
    pub erase_pending: bool,
    /// Successful saves since boot.
    pub saves: u32,
    /// Failed saves since boot.
    pub failures: u32,
    /// The last failure.
    pub last_error: Option<PersistError>,
}

/// Persist -- the log state and the save policy. It holds no flash: every operation takes the
/// `FlashLog`, so the firmware can hand a different one (with the IPMB paused around an erase)
/// to the save that may erase.
pub struct Persist {
    sector_len: usize,
    info: SectorInfo,
    next_seq: u32,
    pending: bool,
    last_write_ms: u32,
    last_save_ms: Option<u32>,
    saves: u32,
    failures: u32,
    last_error: Option<PersistError>,
}

impl Persist {
    /// new -- a log that has not been scanned; call `boot` before anything else.
    pub const fn new() -> Persist {
        Persist {
            sector_len: 0,
            info: SectorInfo::BLANK,
            next_seq: 1,
            pending: false,
            last_write_ms: 0,
            last_save_ms: None,
            saves: 0,
            failures: 0,
            last_error: None,
        }
    }

    /// boot -- scans the sector and loads the newest valid record.
    /// 1. The sector must hold at least one record, in whole words.
    /// 2. The sector is scanned (`scan_sector`); the valid record with the highest sequence
    ///    number is the stored image and the next sequence number follows it. Nothing is
    ///    erased: the sector is the only copy, and a full sector is erased by the next save,
    ///    which writes the newest image straight after.
    /// 3. No valid record: a sector that is not blank (damage only, nothing is lost) is erased
    ///    and the built image stays in use (`Ok(None)`). An erase that fails does not stop the
    ///    boot: the error is in `BootReport::housekeeping`.
    ///
    /// While `nvmro_asserted` step 3 erases nothing (VITA 46.11 Rec 4.1.10-1: no write to IPMC
    /// non-volatile memory); the sector keeps its state and the first save after NVMRO clears
    /// erases what it needs. Call it before the I2C drivers start: an erase takes the flash away
    /// from the CPU for up to the sector erase time.
    pub fn boot<F: FlashLog>(
        &mut self,
        flash: &mut F,
        nvmro_asserted: bool,
    ) -> Result<(Option<FruStore>, BootReport), PersistError> {
        let sector_len = flash.sector_len();
        if sector_len < RECORD_LEN || !sector_len.is_multiple_of(WORD) {
            return Err(PersistError::SectorGeometry);
        }
        self.sector_len = sector_len;
        self.info = scan_sector(flash)?;
        let mut report = BootReport {
            loaded: None,
            erased: false,
            skipped: self.info.skipped,
            housekeeping: None,
        };
        let Some(seq) = self.info.newest else {
            if !nvmro_asserted && !self.info.is_blank() {
                match self.erase_sector(flash) {
                    Ok(()) => report.erased = true,
                    Err(error) => report.housekeeping = Some(error),
                }
            }
            self.next_seq = 1;
            return Ok((None, report));
        };
        self.next_seq = seq.checked_add(1).ok_or(PersistError::SequenceExhausted)?;
        report.loaded = Some(seq);
        let (image, settings) = self.read_newest(flash, seq)?;
        let mut store = FruStore::from_image(image);
        store.load_nv_settings(settings);
        Ok((Some(store), report))
    }

    /// read_newest -- the image and the settings byte (masked) of the valid record `seq`: found
    /// again by a scan of the sector (the scan keeps no offsets, to stay small).
    fn read_newest<F: FlashLog>(
        &self,
        flash: &F,
        seq: u32,
    ) -> Result<([u8; SIZE], u8), PersistError> {
        for slot in 0..self.sector_len / RECORD_LEN {
            let record = read_slot(flash, slot * RECORD_LEN)?;
            if classify(&record) == Slot::Valid(seq) {
                let mut image = [0u8; SIZE];
                if let Some(bytes) = record.get(OFF_IMAGE..OFF_CRC) {
                    image.copy_from_slice(bytes);
                }
                let settings = u8::from_le_bytes(field(&record, OFF_SETTINGS)) & NV_SETTINGS_MASK;
                return Ok((image, settings));
            }
        }
        // The record was valid in the scan a moment ago: flash that now disagrees is a read fault.
        Err(PersistError::Flash(FlashError::Verify))
    }

    /// erase_sector -- erases the sector, checks that it reads blank and records it as blank.
    fn erase_sector<F: FlashLog>(&mut self, flash: &mut F) -> Result<(), PersistError> {
        flash.erase()?;
        if !is_erased_from(flash, 0)? {
            return Err(PersistError::Flash(FlashError::Verify));
        }
        self.info = SectorInfo::BLANK;
        Ok(())
    }

    /// save -- appends the record of `image` and the settings byte to the log.
    /// 1. The sector has a free slot: the record goes there.
    /// 2. Else the sector is erased (`flash.erase`) and the record becomes its first one. The
    ///    sector held the only copy, so a power cut between the erase and the end of the
    ///    programming loses the stored image (module comment).
    /// 3. The words after the commit word are programmed, then the commit word; the slot is read
    ///    back and must classify as valid with the same sequence number (Err(Verify) otherwise).
    ///
    /// A failure after programming started skips the slot (it may hold partial data, or even the
    /// whole record when the driver's error came after the last word landed) and uses the
    /// sequence number up, so no two slots ever carry the same number. Err(SequenceExhausted)
    /// when no higher sequence number exists.
    pub fn save<F: FlashLog>(
        &mut self,
        flash: &mut F,
        image: &[u8; SIZE],
        settings: u8,
    ) -> Result<Saved, PersistError> {
        let seq = self.next_seq;
        if seq > SEQ_MAX {
            return Err(PersistError::SequenceExhausted);
        }
        let mut erased = false;
        let offset = match self.info.next_free {
            Some(offset) => offset,
            None => {
                self.erase_sector(flash)?;
                erased = true;
                0
            }
        };
        let record = encode(seq, image, settings);
        let words = pack_words(&record);
        if let Err(error) = Self::program_record(flash, offset, &words, &record) {
            self.skip_slot(offset);
            // seq <= SEQ_MAX (checked above), so this does not overflow.
            self.next_seq = seq + 1;
            return Err(error);
        }
        self.next_seq = seq + 1;
        let next = offset + RECORD_LEN;
        self.info.next_free = (next + RECORD_LEN <= flash.sector_len()).then_some(next);
        self.info.newest = Some(seq);
        self.info.valid += 1;
        Ok(Saved {
            offset,
            seq,
            erased,
        })
    }

    /// program_record -- the programming order and the read-back of one record.
    fn program_record<F: FlashLog>(
        flash: &mut F,
        offset: usize,
        words: &[u32; RECORD_WORDS],
        record: &[u8; RECORD_LEN],
    ) -> Result<(), PersistError> {
        let (commit, rest) = words.split_at(1);
        flash.program(offset + WORD, rest)?;
        flash.program(offset, commit)?;
        let stored = read_slot(flash, offset)?;
        if stored != *record
            || classify(&stored) != Slot::Valid(u32::from_le_bytes(field(record, OFF_SEQ)))
        {
            return Err(PersistError::Flash(FlashError::Verify));
        }
        Ok(())
    }

    /// skip_slot -- marks the slot at `offset` unusable after a failed program: the next record
    /// goes to the slot after it, or the sector counts as full.
    fn skip_slot(&mut self, offset: usize) {
        let next = offset + RECORD_LEN;
        self.info.skipped += 1;
        self.info.next_free = (next + RECORD_LEN <= self.sector_len).then_some(next);
    }

    /// restore -- FRU RESTORE: saves `image` (the built image) as a new record with the next
    /// sequence number, immediately and not after the delay. When the sector is full this erases
    /// it first, like any save. A pending write is dropped on success (the caller replaces the
    /// store with `image`); on failure nothing changes.
    pub fn restore<F: FlashLog>(
        &mut self,
        flash: &mut F,
        image: &[u8; SIZE],
        settings: u8,
    ) -> Result<Saved, PersistError> {
        let saved = self.save(flash, image, settings)?;
        self.pending = false;
        Ok(saved)
    }

    /// save_needs_erase -- the next save erases the sector first: it has no free slot.
    pub fn save_needs_erase(&self) -> bool {
        self.info.next_free.is_none()
    }

    /// poll -- the dirty/delay rule, called every main-loop pass with the SysTick time.
    /// 1. A dirty store (`FruStore::take_dirty`) starts the quiet time: `delay_ms` after the
    ///    last write the image is saved, so a Chassis Manager writing an area in chunks causes
    ///    one record.
    /// 2. While `nvmro_asserted` nothing is saved (VITA 46.11 Rule 8-3, Rec 4.1.10-1); the write
    ///    stays pending and goes out once NVMRO clears.
    /// 3. A failed save keeps the write pending and restarts the quiet time from the moment the
    ///    failure is reported (`FlashLog::millis`, read after the operation: a slow erase that
    ///    fails must not use up the delay), so a failing flash is retried once per delay, not
    ///    once per pass.
    /// 4. A save that has to erase the sector (`save_needs_erase`) waits until `erase_ok`: the
    ///    firmware passes true when the bus has been quiet; the image stays safe in RAM. An
    ///    erase the driver defers (`FlashError::Deferred`) returns None with the failure counter,
    ///    `last_error` and `last_write_ms` unchanged, and the next pass retries.
    ///
    /// Returns the outcome of a save attempted in this call, None when none was due.
    pub fn poll<F: FlashLog>(
        &mut self,
        flash: &mut F,
        store: &mut FruStore,
        now_ms: u32,
        nvmro_asserted: bool,
        delay_ms: u32,
        erase_ok: bool,
    ) -> Option<Result<Saved, PersistError>> {
        if store.take_dirty() {
            self.pending = true;
            self.last_write_ms = now_ms;
        }
        if !self.pending || nvmro_asserted || now_ms.wrapping_sub(self.last_write_ms) < delay_ms {
            return None;
        }
        if self.save_needs_erase() && !erase_ok {
            return None;
        }
        let result = self.save(flash, store.image(), store.nv_settings());
        // A deferred erase touched nothing and used no sequence number: not now, not a failure.
        if result == Err(PersistError::Flash(FlashError::Deferred)) {
            return None;
        }
        match result {
            Ok(_) => {
                self.pending = false;
                self.last_save_ms = Some(now_ms);
                self.saves += 1;
            }
            Err(error) => {
                self.last_write_ms = flash.millis();
                self.failures += 1;
                self.last_error = Some(error);
            }
        }
        Some(result)
    }

    /// pending -- a write is waiting to be saved (the store's own dirty mark not included).
    pub const fn pending(&self) -> bool {
        self.pending
    }

    /// status -- the log state for the console.
    pub fn status(&self) -> Status {
        Status {
            records: self.info.valid,
            newest_seq: self.info.newest,
            bytes_free: self
                .info
                .next_free
                .map_or(0, |free| self.sector_len.saturating_sub(free)),
            pending: self.pending,
            last_save_ms: self.last_save_ms,
            erase_pending: self.save_needs_erase(),
            saves: self.saves,
            failures: self.failures,
            last_error: self.last_error,
        }
    }
}

impl Default for Persist {
    fn default() -> Self {
        Persist::new()
    }
}

#[cfg(test)]
mod tests;
