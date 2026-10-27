// flash_regs.rs - the FLASH peripheral's erase and program sequences (RM0368 sections 3.5 to
// 3.7) and flash reads. The FRU store driver (firmware drivers/flash.rs) and the bootloader's
// loader (boot/flash_image.rs) are built over it.
//
// Ownership: this module owns the FLASH peripheral once `clock::init` has set the wait states
// (FLASH_ACR LATENCY, prefetch and cache enables) and the binary has moved the peripheral in with
// `FlashRegs::new`. The clock module is the only other writer of FLASH_ACR, at init; after that
// only this file touches any FLASH register (the cache reset below is the one ACR access).
//
// Layout (RM0368 Table 5, STM32F401xD/E, 512 KB): sectors 0-3 are 16 KB, sector 4 is 64 KB,
// sectors 5-7 are 128 KB. Sectors 0 and 1 (0x0800_0000..0x0800_7FFF) belong to the bootloader: no erase
// or program reaches an address below `WRITE_START` (bootloader.md requirement 9), whatever the
// caller's policy does.
//
// An erase or a program operation stalls every flash access, so the core (and every interrupt
// handler, which also runs from flash) waits until it ends: up to 100 us per word and 500 ms per
// 16 KB sector (DS10086 Table 45, PSIZE x32). The caller keeps the IPMB slaves off the bus
// around an erase; nothing here runs in an interrupt handler.
//
// NVMRO (VITA 46.11 Rule 8-3, Rec 4.1.10-1): `erase_sector` and the two program functions sample
// the pin through `ga::nvmro_asserted` as their first step after the idle wait and return
// `FlashError::NvmroAsserted` without touching a flash register while it is asserted.

use core::ptr;
use ipmc_fru::persist::FlashError;
use stm32f4::stm32f401::FLASH;

use crate::bsp::{cycles, ga, systick};

/// KEY1, KEY2 -- the two writes to FLASH_KEYR that unlock FLASH_CR (RM0368 3.7.1).
const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;

/// FLASH_START -- the first flash byte of the STM32F401RE.
const FLASH_START: u32 = 0x0800_0000;
/// WRITE_START -- the lowest address an erase or program may touch: the end of sector 1, the
/// bootloader's two sectors (32 KB).
pub const WRITE_START: u32 = 0x0800_8000;
/// FLASH_END -- one past the last flash byte (512 KB from 0x0800_0000).
const FLASH_END: u32 = 0x0808_0000;
/// FIRST_WRITABLE_SECTOR -- the first sector after the bootloader's two (WRITE_START).
const FIRST_WRITABLE_SECTOR: u8 = 2;
/// LAST_SECTOR -- the highest sector number (RM0368 Table 5).
const LAST_SECTOR: u8 = 7;
/// LAST_16K_SECTOR -- sectors 0..=3 are 16 KB (2 and 3 are the ones an erase may reach).
const LAST_16K_SECTOR: u8 = 3;

/// WORD_PROGRAM_MAX_US -- longest word program, 100 us (DS10086 Table 45 tprog, PSIZE x8/16/32,
/// measured after 100K erase cycles); the byte program is bounded by the same value.
const WORD_PROGRAM_MAX_US: u32 = 100;
/// SECTOR_ERASE_16K_MAX_US -- longest 16 KB sector erase at PSIZE x32, 500 ms (DS10086 Table 45
/// tERASE16KB, the datasheet maximum).
const SECTOR_ERASE_16K_MAX_US: u32 = 500_000;
/// SECTOR_ERASE_LARGE_MAX_US -- longest 128 KB sector erase at PSIZE x32, 2 s (DS10086 Table 45
/// tERASE128KB). The 64 KB sector (sector 4) uses the same bound: DS10086 is not in References/,
/// and the 128 KB maximum bounds the smaller sector.
const SECTOR_ERASE_LARGE_MAX_US: u32 = 2_000_000;
/// POLLS_PER_US_MAX -- most BSY polls one microsecond can hold: one poll per core cycle at the
/// highest core clock, 84 MHz (an actual poll takes several cycles, so the bound only grows).
const POLLS_PER_US_MAX: u32 = 84;
/// BOUND_MARGIN -- the poll bound is this many times the datasheet maximum.
const BOUND_MARGIN: u32 = 2;
/// poll_bound -- BSY polls to allow for an operation of at most `max_us` microseconds.
const fn poll_bound(max_us: u32) -> u32 {
    max_us * POLLS_PER_US_MAX * BOUND_MARGIN
}
/// PROGRAM_POLLS -- the BSY poll bound of one program: 100 us x 84 x 2 = 16 800 polls.
const PROGRAM_POLLS: u32 = poll_bound(WORD_PROGRAM_MAX_US);
/// ERASE_POLLS_16K / ERASE_POLLS_LARGE -- the BSY poll bounds of a sector erase: 500 000 us x 84
/// x 2 = 84 000 000 polls for sectors 2 and 3; 2 000 000 us x 84 x 2 = 336 000 000 for sectors
/// 4..=7 (both fit u32).
const ERASE_POLLS_16K: u32 = poll_bound(SECTOR_ERASE_16K_MAX_US);
const ERASE_POLLS_LARGE: u32 = poll_bound(SECTOR_ERASE_LARGE_MAX_US);

/// WordSize -- the program parallelism (FLASH_CR PSIZE): x32 words or x8 bytes.
#[derive(Clone, Copy)]
enum WordSize {
    Word,
    Byte,
}

impl WordSize {
    /// bytes -- bytes of one unit.
    const fn bytes(self) -> usize {
        match self {
            WordSize::Word => core::mem::size_of::<u32>(),
            WordSize::Byte => core::mem::size_of::<u8>(),
        }
    }
}

/// FlashRegs -- the FLASH peripheral's erase and program sequences (RM0368 3.5 to 3.7). Owns the
/// peripheral. Refuses sectors 0 and 1 and every address below 0x0800_8000 (the bootloader's sectors;
/// bootloader.md requirement 9), so no caller can reach it through a policy bug.
pub struct FlashRegs {
    regs: FLASH,
}

impl FlashRegs {
    /// new -- takes the FLASH peripheral after the clock init.
    pub fn new(regs: FLASH) -> FlashRegs {
        FlashRegs { regs }
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
    /// after every program and erase so the next read sees the flash.
    fn reset_caches(&self) {
        let acr = self.regs.acr();
        acr.modify(|_, w| w.dcen().disabled().icen().disabled());
        acr.modify(|_, w| w.dcrst().reset().icrst().reset());
        acr.modify(|_, w| w.dcrst().no_reset().icrst().no_reset());
        acr.modify(|_, w| w.dcen().enabled().icen().enabled());
    }

    /// erase_sector -- RM0368 3.5.3 sector erase of SNB `number` (2..=7, Table 5; sectors 0 and 1 and a
    /// number above 7 are Err(Range)): BSY clear, NVMRO sample, unlock, stale flags cleared, SER
    /// and SNB and PSIZE x32 set, STRT set, wait for BSY. The core stalls on flash until the erase
    /// ends, so the BSY loop usually sees it done on the first poll; the sector's poll bound
    /// covers the case where it does not. SysTick cannot run during the stall: `systick::catch_up`
    /// gives the millisecond clock the time measured with the DWT cycle counter (0 when the
    /// counter does not run, and the clock then lags).
    pub fn erase_sector(&mut self, number: u8) -> Result<(), FlashError> {
        if !(FIRST_WRITABLE_SECTOR..=LAST_SECTOR).contains(&number) {
            return Err(FlashError::Range);
        }
        let polls = if number <= LAST_16K_SECTOR {
            ERASE_POLLS_16K
        } else {
            ERASE_POLLS_LARGE
        };
        self.wait_idle(polls)?;
        if ga::nvmro_asserted() {
            return Err(FlashError::NvmroAsserted);
        }
        self.unlock()?;
        self.clear_stale_errors();
        self.regs.cr().modify(|_, w| {
            // SAFETY: SNB is a 4-bit field; `number` is 2..=7, a sector number of RM0368 Table 5.
            unsafe { w.snb().bits(number) }
                .psize()
                .psize32()
                .ser()
                .sector_erase()
        });
        let start_ms = systick::millis();
        let start = cycles::now();
        self.regs.cr().modify(|_, w| w.strt().start());
        let result = self.wait_idle(polls).and_then(|()| self.take_errors());
        systick::catch_up(start_ms, cycles::now().wrapping_sub(start));
        self.regs.cr().modify(|_, w| w.ser().clear_bit());
        self.lock();
        self.reset_caches();
        result
    }

    /// program_words -- RM0368 3.5.4 standard programming at PSIZE x32 (2.7-3.6 V supply; the
    /// board runs from 3.3 V) at the word-aligned `addr`: BSY clear, PG set, one word write per
    /// word, BSY clear again after each. The first error stops the run; PG is cleared and the
    /// controller locked on every path.
    pub fn program_words(&mut self, addr: u32, words: &[u32]) -> Result<(), FlashError> {
        if !addr.is_multiple_of(WordSize::Word.bytes() as u32) {
            return Err(FlashError::Alignment);
        }
        self.program(addr, words.len(), WordSize::Word, |address, index| {
            // SAFETY: `program` checked the whole range inside the writable flash; the address
            // is word-aligned (`addr` is a multiple of 4); PG is set with PSIZE x32, so the write
            // is a flash program of this word (RM0368 3.5.4 step 3).
            unsafe { ptr::write_volatile(address as *mut u32, words[index]) };
        })
    }

    /// program_bytes -- PSIZE x8 byte programming (RM0368 3.5.4; allowed at 2.7-3.6 V), for an
    /// unaligned head or tail: the same sequence as `program_words`, one byte write per byte.
    pub fn program_bytes(&mut self, addr: u32, bytes: &[u8]) -> Result<(), FlashError> {
        self.program(addr, bytes.len(), WordSize::Byte, |address, index| {
            // SAFETY: `program` checked the whole range inside the writable flash; PG is set with
            // PSIZE x8, so the write is a flash program of this byte (RM0368 3.5.4 step 3).
            unsafe { ptr::write_volatile(address as *mut u8, bytes[index]) };
        })
    }

    /// program -- the sequence of both program functions: range check of `count` units of `size`
    /// from `addr`, idle wait, NVMRO sample, unlock, PSIZE and PG, then `put(address, index)` for
    /// each unit followed by the idle wait and the error check.
    fn program(
        &mut self,
        addr: u32,
        count: usize,
        size: WordSize,
        put: impl Fn(u32, usize),
    ) -> Result<(), FlashError> {
        let len = count.checked_mul(size.bytes()).ok_or(FlashError::Range)?;
        check_range(addr, len, WRITE_START)?;
        self.wait_idle(PROGRAM_POLLS)?;
        if ga::nvmro_asserted() {
            return Err(FlashError::NvmroAsserted);
        }
        self.unlock()?;
        self.clear_stale_errors();
        self.regs.cr().modify(|_, w| {
            match size {
                WordSize::Word => w.psize().psize32(),
                WordSize::Byte => w.psize().psize8(),
            }
            .pg()
            .program()
        });
        let mut result = Ok(());
        for index in 0..count {
            put(addr + (index * size.bytes()) as u32, index);
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

    /// read -- `read_flash` (a read needs no peripheral handle; this method keeps the call form
    /// of the other flash operations).
    pub fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), FlashError> {
        read_flash(addr, out)
    }
}

/// read_flash -- volatile byte reads of `addr..addr + out.len()` inside the flash (any sector: a
/// read changes nothing). Err(Range) for a range outside the flash. The one flash read of the
/// firmware: `FlashRegs::read` and the runtime header read (app_header.rs) call it.
pub fn read_flash(addr: u32, out: &mut [u8]) -> Result<(), FlashError> {
    check_range(addr, out.len(), FLASH_START)?;
    for (index, byte) in out.iter_mut().enumerate() {
        // SAFETY: `check_range` checked that addr + index lies inside the flash, which is
        // mapped, readable memory; a volatile read keeps the compiler from caching a byte
        // that the program and erase paths change.
        *byte = unsafe { ptr::read_volatile((addr as usize + index) as *const u8) };
    }
    Ok(())
}

/// check_range -- `addr..addr + len` lies in `low..FLASH_END` without overflow; Err(Range) when it
/// starts below `low`, ends past the flash or wraps.
fn check_range(addr: u32, len: usize, low: u32) -> Result<(), FlashError> {
    let end = u32::try_from(len)
        .ok()
        .and_then(|len| addr.checked_add(len))
        .ok_or(FlashError::Range)?;
    if addr < low || end > FLASH_END {
        return Err(FlashError::Range);
    }
    Ok(())
}
