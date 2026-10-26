// drivers/flash.rs - the FRU store sector of the internal flash: word programming, sector erase and
// reads (RM0368 section 3.5), behind the ipmc_fru::persist::FlashLog trait.
//
// Ownership: this module owns the FRU store policy: which sector, the layout check and the
// range map. The FLASH registers belong to `crate::bsp::flash_regs::FlashRegs` (erase and
// program sequences, the sector-0 refusal), which `Flash::new` takes over.
//
// Layout (RM0368 Table 5, STM32F401xD/E, 512 KB): sectors 0-3 are 16 KB, sector 4 is 64 KB,
// sectors 5-7 are 128 KB. flash_map.x (included by memory.x) places:
//   BOOT      sector 0     0x0800_0000..0x0800_3FFF   16 KB  the bootloader
//   STORE     sector 1     0x0800_4000..0x0800_7FFF   16 KB  the FRU flash store, nothing else
//   APP       sectors 2-7  0x0800_8000..0x0807_FFFF  480 KB  the runtime (link.x)
// The store is the one sector this module erases and programs, so an erase never touches the
// bootloader or the program. Peer: flash_map.x, which exports the linker symbols
// `__fru_store_start`, `__fru_store_end`, `__boot_end` and `__app_vectors` (link.x asserts that
// the program starts after the header). `Flash::new` checks them
// (`ipmc_fru::persist::store_region_ok`) and refuses to start the store unless the region is exactly sector 1, the bootloader ends before
// it and the runtime starts after it: a layout that drifts apart can never erase code.
//
// An erase or a program operation stalls every flash access (timing in flash_regs.rs). The
// caller keeps the IPMB slaves off the bus around an erase (ipmb_link.rs); nothing here runs in
// an interrupt handler.
//
// NVMRO (VITA 46.11 Rule 8-3, Rec 4.1.10-1): `FlashRegs` samples the pin as the first step of
// every erase and program and returns `FlashError::NvmroAsserted` without touching a flash
// register while it is asserted. Every destructive operation of the log, at boot, in a save and
// in FRU RESTORE, goes through `erase` and `program` here, so none of them runs on an older
// sample.

use crate::bsp::flash_regs::FlashRegs;
use core::ptr;
use ipmc_fru::persist::{
    store_region_ok, FlashError, FlashLog, RegionError, STORE_LEN, STORE_START,
};
use stm32f4::stm32f401::FLASH;

use crate::bsp::systick;

/// STORE_SECTOR_NUMBER -- the SNB value of the store sector (RM0368 3.7.2: sector number; Table 5:
/// sector 1 is the second 16 KB sector).
const STORE_SECTOR_NUMBER: u8 = 1;
/// STORE_SECTOR_LEN -- bytes of the store sector (RM0368 Table 5: sector 1 is 16 KB). The number
/// itself is `ipmc_fru::persist::STORE_LEN`, so the log and the driver cannot disagree.
pub const STORE_SECTOR_LEN: usize = STORE_LEN as usize;
/// FLASH_END -- one past the last flash byte of the STM32F401RE (512 KB from 0x0800_0000).
const FLASH_END: usize = 0x0808_0000;

extern "C" {
    // link.x / cortex-m-rt: the load address and the run-time bounds of .data. The program image
    // ends at __sidata + (__edata - __sdata): .data is the last section in flash.
    static __sidata: u32;
    static __sdata: u32;
    static __edata: u32;
    // flash_map.x: the store region, the end of the bootloader and the start of the runtime
    // (its vector table). link.x: the start of .text.
    static __fru_store_start: u8;
    static __fru_store_end: u8;
    static __boot_end: u8;
    static __app_vectors: u8;
}

/// program_end -- one past the last flash byte the linked program uses.
fn program_end() -> usize {
    // Only the addresses of the linker-provided symbols are taken; nothing is read.
    let load = ptr::addr_of!(__sidata) as usize;
    let start = ptr::addr_of!(__sdata) as usize;
    let end = ptr::addr_of!(__edata) as usize;
    load + end.saturating_sub(start)
}

/// linker_address -- the value of a linker symbol that holds an address (not data).
macro_rules! linker_address {
    ($symbol:ident) => {
        // `$symbol` is an `extern` static from the linker script. Only its address is taken (no
        // `unsafe` is needed for that): a linker address, never read or written through.
        ptr::addr_of!($symbol) as usize
    };
}

/// StartError -- why the store flash cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// The linked store region is not flash sector 1, or the vector table or the program
    /// reaches into it (flash_map.x, link.x and this file disagree).
    Region(RegionError),
    /// The linked program reaches past the end of the flash.
    ProgramPastFlash,
}

impl StartError {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            StartError::Region(error) => error.text(),
            StartError::ProgramPastFlash => "the program image reaches past the end of the flash",
        }
    }
}

/// check_layout -- the linked flash layout against the sector the log erases.
fn check_layout() -> Result<(), StartError> {
    let word = |address: usize| u32::try_from(address).unwrap_or(u32::MAX);
    let start = word(linker_address!(__fru_store_start));
    let end = word(linker_address!(__fru_store_end));
    let boot_end = word(linker_address!(__boot_end));
    let app_vectors = word(linker_address!(__app_vectors));
    // The store lies between the bootloader and the runtime.
    store_region_ok(start, end, boot_end, app_vectors).map_err(StartError::Region)?;
    if program_end() > FLASH_END {
        return Err(StartError::ProgramPastFlash);
    }
    Ok(())
}

/// Flash -- the store sector over the FLASH register layer, owned.
pub struct Flash {
    regs: FlashRegs,
}

impl Flash {
    /// new -- takes the FLASH peripheral after the clock init. Err when the linked layout is not
    /// the one the log is built for (`check_layout`): the peripheral is dropped unused and
    /// nothing can be programmed.
    pub fn new(regs: FLASH) -> Result<Flash, StartError> {
        check_layout()?;
        Ok(Flash {
            regs: FlashRegs::new(regs),
        })
    }

    /// map -- the byte address of `offset` in the store sector and a range check of
    /// `offset..offset + len`. Every read, program and erase goes through the sector this
    /// returns an address in.
    fn map(offset: usize, len: usize) -> Result<usize, FlashError> {
        let end = offset.checked_add(len).ok_or(FlashError::Range)?;
        if end > STORE_SECTOR_LEN {
            return Err(FlashError::Range);
        }
        Ok(STORE_START as usize + offset)
    }
}

impl FlashLog for Flash {
    fn sector_len(&self) -> usize {
        STORE_SECTOR_LEN
    }

    fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), FlashError> {
        let base = Flash::map(offset, out.len())?;
        self.regs.read(base_u32(base)?, out)
    }

    /// program -- word programming (`FlashRegs::program_words`, RM0368 3.5.4 at PSIZE x32) of
    /// the words at `offset` in the store sector.
    fn program(&mut self, offset: usize, words: &[u32]) -> Result<(), FlashError> {
        if !offset.is_multiple_of(core::mem::size_of::<u32>()) {
            return Err(FlashError::Alignment);
        }
        let base = Flash::map(offset, core::mem::size_of_val(words))?;
        self.regs.program_words(base_u32(base)?, words)
    }

    /// erase -- the store sector's erase (`FlashRegs::erase_sector`, RM0368 3.5.3).
    fn erase(&mut self) -> Result<(), FlashError> {
        self.regs.erase_sector(STORE_SECTOR_NUMBER)
    }

    fn millis(&self) -> u32 {
        systick::millis()
    }
}

/// base_u32 -- a flash byte address as the u32 the register layer takes.
fn base_u32(address: usize) -> Result<u32, FlashError> {
    u32::try_from(address).map_err(|_| FlashError::Range)
}
