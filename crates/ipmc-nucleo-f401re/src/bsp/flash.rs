// bsp/flash.rs - the FRU log sectors of the internal flash: word programming, sector erase and
// reads (RM0368 section 3.5), behind the ipmc_fru::persist::FlashLog trait.
//
// Ownership: this module owns the FLASH peripheral once `bsp::clock::init` has set the wait
// states (FLASH_ACR LATENCY, prefetch and cache enables) and main has moved the peripheral in
// with `Flash::new`. The clock module is the only other writer of FLASH_ACR, at init; after that
// only this file touches any FLASH register (the cache reset below is the one ACR access).
//
// Layout (RM0368 Table 5, STM32F401xD/E, 512 KB): sectors 0-3 are 16 KB, sector 4 is 64 KB,
// sectors 5-7 are 128 KB.
//   program   sectors 0-5  0x0800_0000..0x0803_FFFF  256 KB  (memory.x FLASH, LENGTH 256K)
//   log A     sector 6     0x0804_0000..0x0805_FFFF  128 KB
//   log B     sector 7     0x0806_0000..0x0807_FFFF  128 KB
// Peer: memory.x. Its FLASH LENGTH must end at LOG_SECTOR_ADDR[0]; if the two drift apart the
// program grows into the log and the next save or erase destroys code. `Flash::new` checks the
// linked image against the log start at run time and refuses to start when they overlap.
//
// An erase or a program operation stalls every flash access, so the core (and every interrupt
// handler, which also runs from flash) waits until it ends: up to 100 us per word and 2 s per
// 128 KB sector (DS10086 Table 45, PSIZE x32). The caller keeps the IPMB slaves off the bus
// around an erase (ipmb_link.rs); nothing here runs in an interrupt handler.
//
// NVMRO (VITA 46.11 Rule 8-3, Rec 4.1.10-1): `erase` and `program` sample the pin through
// `ga::nvmro_asserted` as their first step after the caller's waits and return
// `FlashError::NvmroAsserted` without touching a flash register while it is asserted. Every
// destructive operation of the log, at boot, in a save and in FRU RESTORE, goes through these two
// functions, so none of them runs on an older sample.

use core::ptr;
use ipmc_fru::persist::{FlashError, FlashLog, LogSector};
use stm32f4::stm32f401::FLASH;

use super::{ga, ipmb_common, systick};

/// KEY1, KEY2 -- the two writes to FLASH_KEYR that unlock FLASH_CR (RM0368 3.7.1).
const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;

/// LOG_SECTOR_ADDR -- first byte of log sector A and B (RM0368 Table 5: sectors 6 and 7).
const LOG_SECTOR_ADDR: [usize; 2] = [0x0804_0000, 0x0806_0000];
/// LOG_SECTOR_NUMBER -- the SNB value of log sector A and B (RM0368 3.7.2: sector number).
const LOG_SECTOR_NUMBER: [u8; 2] = [6, 7];
/// LOG_SECTOR_LEN -- bytes of each log sector (RM0368 Table 5: sectors 5-7 are 128 KB).
pub const LOG_SECTOR_LEN: usize = 128 * 1024;
/// FLASH_END -- one past the last flash byte of the STM32F401RE (512 KB from 0x0800_0000).
const FLASH_END: usize = 0x0808_0000;
const _: () = assert!(LOG_SECTOR_ADDR[0] + LOG_SECTOR_LEN == LOG_SECTOR_ADDR[1]);
const _: () = assert!(LOG_SECTOR_ADDR[1] + LOG_SECTOR_LEN == FLASH_END);

/// WORD_PROGRAM_MAX_US -- longest word program, 100 us (DS10086 Table 45 tprog, PSIZE x8/16/32,
/// measured after 100K erase cycles).
const WORD_PROGRAM_MAX_US: u32 = 100;
/// SECTOR_ERASE_MAX_US -- longest 128 KB sector erase at PSIZE x32, 2 s (DS10086 Table 45
/// tERASE128KB).
const SECTOR_ERASE_MAX_US: u32 = 2_000_000;
/// POLLS_PER_US_MAX -- most BSY polls one microsecond can hold: one poll per core cycle at the
/// highest core clock, 84 MHz (an actual poll takes several cycles, so the bound only grows).
const POLLS_PER_US_MAX: u32 = 84;
/// BOUND_MARGIN -- the poll bound is this many times the datasheet maximum.
const BOUND_MARGIN: u32 = 2;
/// poll_bound -- BSY polls to allow for an operation of at most `max_us` microseconds.
const fn poll_bound(max_us: u32) -> u32 {
    max_us * POLLS_PER_US_MAX * BOUND_MARGIN
}
/// PROGRAM_POLLS / ERASE_POLLS -- the BSY poll bounds: 100 us x 84 x 2 = 16 800 polls per word;
/// 2 000 000 us x 84 x 2 = 336 000 000 polls per sector (fits u32).
const PROGRAM_POLLS: u32 = poll_bound(WORD_PROGRAM_MAX_US);
const ERASE_POLLS: u32 = poll_bound(SECTOR_ERASE_MAX_US);

extern "C" {
    // cortex-m-rt link.x: the load address and the run-time bounds of .data. The program image
    // ends at __sidata + (__edata - __sdata): .data is the last section in flash.
    static __sidata: u32;
    static __sdata: u32;
    static __edata: u32;
}

/// program_end -- one past the last flash byte the linked program uses.
fn program_end() -> usize {
    // Only the addresses of the linker-provided symbols are taken; nothing is read.
    let load = ptr::addr_of!(__sidata) as usize;
    let start = ptr::addr_of!(__sdata) as usize;
    let end = ptr::addr_of!(__edata) as usize;
    load + end.saturating_sub(start)
}

/// StartError -- why the log flash cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// The linked program reaches into log sector A (memory.x and this file disagree).
    ProgramOverlapsLog,
}

impl StartError {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            StartError::ProgramOverlapsLog => "the program image reaches into the log sectors",
        }
    }
}

/// Flash -- the FLASH peripheral, owned.
pub struct Flash {
    regs: FLASH,
}

impl Flash {
    /// new -- takes the FLASH peripheral after the clock init. Err when the linked program
    /// overlaps log sector A: the peripheral is dropped unused and nothing can be programmed.
    pub fn new(regs: FLASH) -> Result<Flash, StartError> {
        if program_end() > LOG_SECTOR_ADDR[0] {
            return Err(StartError::ProgramOverlapsLog);
        }
        Ok(Flash { regs })
    }

    /// wait_idle -- polls BSY at most `bound` times; Err(Timeout) when it stays set.
    /// The bound does not cover a BSY that never clears: the code runs from this same flash bank,
    /// so the core stalls on its own instruction fetch while the controller is busy (RM0368
    /// 3.5.1) and never reaches the timeout, and the timeout cleanup (a FLASH_CR write) stalls
    /// the same way. The protection against that hardware fault is a hardware watchdog (a later
    /// item); `Timeout` covers only a flag that clears late.
    fn wait_idle(&self, bound: u32) -> Result<(), FlashError> {
        for _ in 0..bound {
            if self.regs.sr().read().bsy().is_not_busy() {
                return Ok(());
            }
        }
        Err(FlashError::Timeout)
    }

    /// unlock -- the key sequence on FLASH_KEYR (RM0368 3.7.1); Err(Protection) when the control
    /// register stays locked.
    fn unlock(&self) -> Result<(), FlashError> {
        if self.regs.cr().read().lock().is_locked() {
            self.regs.keyr().write(|w| w.key().set(KEY1));
            self.regs.keyr().write(|w| w.key().set(KEY2));
        }
        if self.regs.cr().read().lock().is_locked() {
            return Err(FlashError::Protection);
        }
        Ok(())
    }

    /// lock -- sets LOCK again so a stray write cannot start an operation.
    fn lock(&self) {
        self.regs.cr().modify(|_, w| w.lock().locked());
    }

    /// take_errors -- reads the status flags, clears them (write 1) and names the first fault:
    /// WRPERR protection, PGAERR alignment, PGSERR sequence, PGPERR or OPERR programming.
    fn take_errors(&self) -> Result<(), FlashError> {
        let sr = self.regs.sr().read();
        let fault = if sr.wrperr().is_active() {
            Some(FlashError::Protection)
        } else if sr.pgaerr().is_active() {
            Some(FlashError::Alignment)
        } else if sr.pgserr().is_active() {
            Some(FlashError::Sequence)
        } else if sr.pgperr().is_active() || sr.operr().is_active() {
            Some(FlashError::Programming)
        } else {
            None
        };
        self.regs.sr().write(|w| {
            w.eop()
                .clear()
                .operr()
                .clear()
                .wrperr()
                .clear()
                .pgaerr()
                .clear()
                .pgperr()
                .clear()
                .pgserr()
                .clear()
        });
        fault.map_or(Ok(()), Err)
    }

    /// clear_stale_errors -- clears flags left by an earlier event before an operation starts, so
    /// they are not read as its fault (a stray write to flash sets PGSERR, RM0368 3.5.4). Every
    /// operation of this file already clears its own flags (`take_errors`), so nothing of ours
    /// is lost here.
    fn clear_stale_errors(&self) {
        let _ = self.take_errors();
    }

    /// reset_caches -- flushes the data and instruction caches (RM0368 3.5.5: a flash write or
    /// erase can leave stale lines): disable both, pulse the reset bits, enable both again. Runs
    /// after every program and erase so the next read of the log sees the flash.
    fn reset_caches(&self) {
        let acr = self.regs.acr();
        acr.modify(|_, w| w.dcen().disabled().icen().disabled());
        acr.modify(|_, w| w.dcrst().reset().icrst().reset());
        acr.modify(|_, w| w.dcrst().no_reset().icrst().no_reset());
        acr.modify(|_, w| w.dcen().enabled().icen().enabled());
    }

    /// map -- the first byte address of `sector` and a range check of `offset..offset + len`.
    fn map(sector: LogSector, offset: usize, len: usize) -> Result<usize, FlashError> {
        let end = offset.checked_add(len).ok_or(FlashError::Range)?;
        if end > LOG_SECTOR_LEN {
            return Err(FlashError::Range);
        }
        Ok(LOG_SECTOR_ADDR[sector.index()] + offset)
    }
}

impl FlashLog for Flash {
    fn sector_len(&self) -> usize {
        LOG_SECTOR_LEN
    }

    fn read(&self, sector: LogSector, offset: usize, out: &mut [u8]) -> Result<(), FlashError> {
        let base = Flash::map(sector, offset, out.len())?;
        for (index, byte) in out.iter_mut().enumerate() {
            // SAFETY: `map` checked that base + index lies inside the log sector, which is
            // mapped, readable flash; a volatile read keeps the compiler from caching a byte
            // that the program and erase paths change.
            *byte = unsafe { ptr::read_volatile((base + index) as *const u8) };
        }
        Ok(())
    }

    /// program -- RM0368 3.5.4 standard programming at PSIZE x32 (2.7-3.6 V supply; the board
    /// runs from 3.3 V): BSY clear, PG set, one word write per word, BSY clear again after each.
    /// The first error stops the run; PG is cleared and the controller locked on every path.
    fn program(
        &mut self,
        sector: LogSector,
        offset: usize,
        words: &[u32],
    ) -> Result<(), FlashError> {
        if !offset.is_multiple_of(core::mem::size_of::<u32>()) {
            return Err(FlashError::Alignment);
        }
        let base = Flash::map(sector, offset, core::mem::size_of_val(words))?;
        self.wait_idle(PROGRAM_POLLS)?;
        if ga::nvmro_asserted() {
            return Err(FlashError::NvmroAsserted);
        }
        self.unlock()?;
        self.clear_stale_errors();
        self.regs
            .cr()
            .modify(|_, w| w.psize().psize32().pg().program());
        let mut result = Ok(());
        for (index, word) in words.iter().enumerate() {
            let address = base + index * core::mem::size_of::<u32>();
            // SAFETY: `map` checked the whole range inside the log sector; the address is
            // word-aligned (offset and base are multiples of 4); PG is set, so the write is a
            // flash program of this word (RM0368 3.5.4 step 3).
            unsafe { ptr::write_volatile(address as *mut u32, *word) };
            result = self
                .wait_idle(PROGRAM_POLLS)
                .and_then(|()| self.take_errors());
            if result.is_err() {
                break;
            }
        }
        self.regs.cr().modify(|_, w| w.pg().clear_bit());
        self.lock();
        self.reset_caches();
        result
    }

    /// erase -- RM0368 3.5.3 sector erase: BSY clear, SER and SNB set, STRT set, wait for BSY.
    /// The core stalls on flash until the erase ends, so the BSY loop usually sees it done on
    /// the first poll; ERASE_POLLS bounds the case where it does not. SysTick cannot run during
    /// the stall: `systick::catch_up` gives the millisecond clock the time measured with the DWT
    /// cycle counter (0 when the counter does not run, and the clock then lags).
    fn erase(&mut self, sector: LogSector) -> Result<(), FlashError> {
        let number = LOG_SECTOR_NUMBER[sector.index()];
        self.wait_idle(ERASE_POLLS)?;
        if ga::nvmro_asserted() {
            return Err(FlashError::NvmroAsserted);
        }
        self.unlock()?;
        self.clear_stale_errors();
        self.regs.cr().modify(|_, w| {
            // SAFETY: SNB is a 4-bit field; 6 and 7 are the sector numbers of RM0368 Table 5.
            unsafe { w.snb().bits(number) }
                .psize()
                .psize32()
                .ser()
                .sector_erase()
        });
        let start_ms = systick::millis();
        let start = ipmb_common::now();
        self.regs.cr().modify(|_, w| w.strt().start());
        let result = self
            .wait_idle(ERASE_POLLS)
            .and_then(|()| self.take_errors());
        systick::catch_up(start_ms, ipmb_common::now().wrapping_sub(start));
        self.regs.cr().modify(|_, w| w.ser().clear_bit());
        self.lock();
        self.reset_caches();
        result
    }

    fn millis(&self) -> u32 {
        systick::millis()
    }
}
