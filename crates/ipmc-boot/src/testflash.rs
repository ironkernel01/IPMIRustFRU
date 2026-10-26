//! testflash - the host test model shared by the `load` and `session` tests: a RAM flash that
//! behaves like the part (program clears bits only, erase sets a sector to FFh) with injectable
//! faults, a settable clock, and the Intel HEX image builders.

use crate::geometry::{APP_START, SECTORS};
use crate::header::{patch, placeholder, ImageSource};
use crate::hex::tests::record_line;
use crate::load::FlashOps;
use crate::session::BootHw;
use ipmc_fru::persist::FlashError;
use std::cell::Cell;
use std::string::String;
use std::vec;
use std::vec::Vec;

pub(crate) const FLASH_BASE: u32 = 0x0800_0000;
pub(crate) const FLASH_SIZE: usize = 512 * 1024;

/// Ram -- the flash model. `now` is the clock `BootHw::now_ms` reads; every read adds `step`.
pub(crate) struct Ram {
    pub bytes: Vec<u8>,
    pub erased: Vec<u8>,
    pub program_fault: Option<FlashError>,
    pub erase_fault_at: Option<u8>,
    pub erase_error: FlashError,
    pub wrong_bits: bool,
    pub now: Cell<u32>,
    pub step: u32,
    pub activity: Cell<u32>,
}

impl Ram {
    pub fn new() -> Self {
        Ram {
            bytes: vec![0xFF; FLASH_SIZE],
            erased: Vec::new(),
            program_fault: None,
            erase_fault_at: None,
            erase_error: FlashError::Timeout,
            wrong_bits: false,
            now: Cell::new(0),
            step: 0,
            activity: Cell::new(0),
        }
    }

    /// with_image -- a flash that holds `image` at APP_START.
    pub fn with_image(image: &[u8]) -> Self {
        let mut ram = Ram::new();
        let from = (APP_START - FLASH_BASE) as usize;
        ram.bytes[from..from + image.len()].copy_from_slice(image);
        ram
    }
}

impl ImageSource for Ram {
    fn read(&self, addr: u32, out: &mut [u8]) {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self
                .bytes
                .get((addr - FLASH_BASE) as usize + i)
                .copied()
                .unwrap_or(0xFF);
        }
    }
}

impl FlashOps for Ram {
    fn erase_sector(&mut self, number: u8) -> Result<(), FlashError> {
        if self.erase_fault_at == Some(number) {
            return Err(self.erase_error);
        }
        let s = SECTORS[number as usize];
        let from = (s.start - FLASH_BASE) as usize;
        self.bytes[from..from + s.len as usize].fill(0xFF);
        self.erased.push(number);
        Ok(())
    }

    fn program(&mut self, addr: u32, bytes: &[u8]) -> Result<(), FlashError> {
        if let Some(fault) = self.program_fault {
            return Err(fault);
        }
        for (i, b) in bytes.iter().enumerate() {
            let at = (addr - FLASH_BASE) as usize + i;
            self.bytes[at] &= if self.wrong_bits { !b } else { *b };
        }
        Ok(())
    }
}

impl BootHw for Ram {
    fn now_ms(&self) -> u32 {
        let now = self.now.get();
        self.now.set(now.wrapping_add(self.step));
        now
    }

    fn activity(&self) {
        self.activity.set(self.activity.get() + 1);
    }
}

/// hex_image -- the Intel HEX lines of `image` at APP_START in records of `size` bytes with
/// 04 records at each 64 KB crossing, ended by 05 and 01.
pub(crate) fn hex_image(image: &[u8], size: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut upper = u32::MAX;
    for (n, chunk) in image.chunks(size).enumerate() {
        let addr = APP_START + (n * size) as u32;
        if addr >> 16 != upper {
            upper = addr >> 16;
            lines.push(record_line(0, 0x04, &(upper as u16).to_be_bytes()));
        }
        lines.push(record_line(addr as u16, 0x00, chunk));
    }
    lines.push(record_line(0, 0x05, &[8, 0, 0x81, 0x41]));
    lines.push(record_line(0, 0x01, &[]));
    lines
}

/// good_image -- a patched runtime image of 70 000 bytes (crosses a 64 KB boundary).
pub(crate) fn good_image() -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..70_000).map(|i| (i * 13 + 5) as u8).collect();
    bytes[0..4].copy_from_slice(&0x2001_8000u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&(APP_START + 0x341).to_le_bytes());
    bytes[0x200..0x240].copy_from_slice(&placeholder([0, 1, 0], 0));
    patch(&mut bytes).unwrap();
    bytes
}
