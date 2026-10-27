//! ram_flash.rs - `RamFlash`, the in-RAM `FlashLog` of the host: the tests of `persist` and the
//! emulator (ipmc-emu) keep the FRU log in it. Built with the `host` feature (and in tests); the
//! firmware never links it.
//!
//! It models flash: programming only clears bits, an erase sets every byte to FFh. The public
//! fields inject the failures the log has to survive: a word budget (a power cut during a
//! program), a late error (a driver that reports a fault after the word landed), a torn erase, a
//! failing or deferred erase, an asserted NVMRO.

extern crate std;

use crate::persist::{FlashError, FlashLog, ERASED, STORE_LEN, WORD};
use std::vec;
use std::vec::Vec;

/// RamFlash -- a RAM-backed log sector with failure injection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RamFlash {
    /// The sector bytes.
    pub sector: Vec<u8>,
    /// Words that may still be programmed; None = unlimited. At 0 the next word fails.
    pub budget: Option<usize>,
    /// Words programmed so far.
    pub words_programmed: usize,
    /// When `words_programmed` reaches this count the word just written stays and the call
    /// fails with Timeout (the busy flag cleared after the poll bound ran out).
    pub late_error: Option<usize>,
    /// The next erase sets only the first half of the sector to FFh and fails (power cut).
    pub torn_erase: bool,
    /// Erases that started.
    pub erases: usize,
    /// The clock `FlashLog::millis` reports; an erase advances it by `erase_ms` (a slow erase).
    pub now: u32,
    /// Milliseconds one erase takes.
    pub erase_ms: u32,
    /// Every erase fails with Timeout and leaves the sector as it was.
    pub erase_fails: bool,
    /// The next erase is deferred once (`FlashError::Deferred`), touching nothing.
    pub defer_erase: bool,
    /// NVMRO is asserted: every erase and program is refused and touches nothing.
    pub nvmro: bool,
}

impl RamFlash {
    /// with_len -- an erased sector of `len` bytes.
    pub fn with_len(len: usize) -> RamFlash {
        RamFlash {
            sector: vec![ERASED; len],
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

    /// new -- an erased store sector (`STORE_LEN` bytes), the board's flash before any save.
    pub fn new() -> RamFlash {
        RamFlash::with_len(STORE_LEN as usize)
    }
}

impl Default for RamFlash {
    fn default() -> Self {
        RamFlash::new()
    }
}

impl FlashLog for RamFlash {
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
        if std::mem::take(&mut self.torn_erase) {
            let half = self.sector.len() / 2;
            self.sector[..half].fill(ERASED);
            return Err(FlashError::Timeout);
        }
        self.sector.fill(ERASED);
        Ok(())
    }

    fn millis(&self) -> u32 {
        self.now
    }
}
