// flash_image.rs - the flash as the bootloader uses it: the header check reads it
// (`ImageSource`), and the bootloader session erases and programs it (`FlashOps`).
//
// Ownership: `FlashImage` owns the library's `FlashRegs` (the FLASH peripheral). The register
// layer refuses sectors 0 and 1 and every address below 0x0800_8000 on its own; the region policy
// (`ipmc_boot::geometry`, which allows sectors 3-7 only) sits on top of it.

use ipmc_boot::{FlashOps, ImageSource};
use ipmc_fru::persist::FlashError;
use ipmc_nucleo_f401re::bsp::flash_regs::FlashRegs;
use stm32f4::stm32f401::FLASH;

/// ERASED -- what a byte outside the flash reads as in an `ImageSource` (the trait's rule).
const ERASED: u8 = 0xFF;
/// WORD_BYTES -- bytes of one flash word (PSIZE x32).
const WORD_BYTES: usize = 4;
/// BATCH_WORDS -- words programmed per `program_words` call (one unlock, one lock): 64 words
/// cover the 255-byte data of one Intel HEX record.
const BATCH_WORDS: usize = 64;

/// FlashImage -- the program flash: read, erase a sector, program bytes at any alignment.
pub struct FlashImage {
    regs: FlashRegs,
}

impl FlashImage {
    /// new -- takes the FLASH peripheral after the clock init.
    pub fn new(flash: FLASH) -> Self {
        FlashImage {
            regs: FlashRegs::new(flash),
        }
    }
}

impl ImageSource for FlashImage {
    /// read -- the flash bytes; a range outside the flash reads FFh.
    fn read(&self, addr: u32, out: &mut [u8]) {
        if self.regs.read(addr, out).is_err() {
            out.fill(ERASED);
        }
    }
}

impl FlashOps for FlashImage {
    fn erase_sector(&mut self, number: u8) -> Result<(), FlashError> {
        self.regs.erase_sector(number)
    }

    /// program -- an unaligned head by bytes, the aligned middle by words (batches of
    /// BATCH_WORDS), an unaligned tail by bytes.
    fn program(&mut self, addr: u32, bytes: &[u8]) -> Result<(), FlashError> {
        let head_len = (WORD_BYTES - addr as usize % WORD_BYTES) % WORD_BYTES;
        let (head, rest) = bytes.split_at_checked(head_len).unwrap_or((bytes, &[]));
        let mut at = addr;
        if !head.is_empty() {
            self.regs.program_bytes(at, head)?;
            at = at.wrapping_add(head.len() as u32);
        }
        let middle_len = rest.len() / WORD_BYTES * WORD_BYTES;
        let (middle, tail) = rest.split_at_checked(middle_len).unwrap_or((&[], rest));
        for batch in middle.chunks(BATCH_WORDS * WORD_BYTES) {
            let mut words = [0u32; BATCH_WORDS];
            let count = batch.len() / WORD_BYTES;
            let (chunks, _) = batch.as_chunks::<WORD_BYTES>();
            for (word, chunk) in words.iter_mut().zip(chunks) {
                *word = u32::from_le_bytes(*chunk);
            }
            self.regs
                .program_words(at, words.get(..count).unwrap_or(&[]))?;
            at = at.wrapping_add(batch.len() as u32);
        }
        if tail.is_empty() {
            Ok(())
        } else {
            self.regs.program_bytes(at, tail)
        }
    }
}
