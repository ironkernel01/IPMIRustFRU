//! load - erase and the LOAD state machine (design 8.3 and 8.4) over the `FlashOps` trait.

use crate::error::BootError;
use crate::geometry::{load_allowed, Target};
use crate::header::ImageSource;
use crate::hex::{parse, Record, DATA_MAX};
use ipmc_fru::persist::FlashError;

/// COMPARE_CHUNK -- bytes read back at a time.
const COMPARE_CHUNK: usize = 64;
/// ERASED -- what the erased-check compares against.
const ERASED: [u8; DATA_MAX] = [0xFF; DATA_MAX];

/// FlashOps -- the flash as the bootloader uses it. The firmware implements it over the shared
/// register layer; the tests over a RAM model.
pub trait FlashOps: ImageSource {
    /// erase_sector -- erases one sector by number (never 0 or 1: the implementation refuses them too).
    fn erase_sector(&mut self, number: u8) -> Result<(), FlashError>;
    /// program -- writes `bytes` at `addr` (any alignment; the implementation picks word or byte
    /// writes). Does not read back.
    fn program(&mut self, addr: u32, bytes: &[u8]) -> Result<(), FlashError>;
}

/// erase -- erases the target's sectors in `Target::sectors` order; the first error stops it.
pub fn erase(target: Target, flash: &mut dyn FlashOps) -> Result<(), BootError> {
    for sector in target.sectors() {
        flash.erase_sector(sector.number)?;
    }
    Ok(())
}

/// Loader -- the LOAD record state: the upper address bits and the counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct Loader {
    upper: u32,
    records: u32,
    bytes: u32,
}

/// LoadStep -- what an accepted record means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStep {
    Record,
    End { records: u32, bytes: u32 },
}

/// reads_as -- true when flash at `addr` holds exactly `expected`.
fn reads_as(flash: &dyn ImageSource, addr: u32, expected: &[u8]) -> bool {
    let mut at = addr;
    for want in expected.chunks(COMPARE_CHUNK) {
        let mut got = [0u8; COMPARE_CHUNK];
        let Some(window) = got.get_mut(..want.len()) else {
            return false;
        };
        flash.read(at, window);
        if window != want {
            return false;
        }
        at = at.wrapping_add(want.len() as u32);
    }
    true
}

impl Loader {
    pub const fn new() -> Self {
        Loader {
            upper: 0,
            records: 0,
            bytes: 0,
        }
    }

    /// record -- one record line: parse, then for data records the range check
    /// (`load_allowed` on the absolute address `upper << 16 | offset`, else Range), the erased
    /// check (every target byte reads FFh, else NotErased), program, read back and compare (else
    /// Verify). 04 sets `upper`, 05 is accepted and ignored, 01 returns End. A record error
    /// leaves the loader unchanged, so the host resends.
    pub fn record(&mut self, line: &str, flash: &mut dyn FlashOps) -> Result<LoadStep, BootError> {
        let mut buf = [0u8; DATA_MAX + 5];
        match parse(line, &mut buf)? {
            Record::Data { offset, bytes } => {
                let addr = (self.upper << 16) | offset as u32;
                let len = bytes.len() as u32;
                if !load_allowed(addr, len) {
                    return Err(BootError::Range);
                }
                if !reads_as(flash, addr, ERASED.get(..bytes.len()).unwrap_or(&[])) {
                    return Err(BootError::NotErased);
                }
                flash.program(addr, bytes)?;
                if !reads_as(flash, addr, bytes) {
                    return Err(BootError::Verify);
                }
                self.records += 1;
                self.bytes += len;
                Ok(LoadStep::Record)
            }
            Record::ExtendedLinear(upper) => {
                self.upper = upper as u32;
                Ok(LoadStep::Record)
            }
            Record::StartLinear(_) => Ok(LoadStep::Record),
            Record::Eof => Ok(LoadStep::End {
                records: self.records,
                bytes: self.bytes,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{check, ImageError};
    use crate::hex::tests::record_line;
    use crate::testflash::{good_image, hex_image, Ram};
    use std::vec;

    #[test]
    fn a_full_image_loads_and_passes_the_check() {
        let image = good_image();
        for size in [16, 32] {
            let mut flash = Ram::new();
            let mut loader = Loader::new();
            let mut last = LoadStep::Record;
            for line in hex_image(&image, size) {
                last = loader.record(&line, &mut flash).unwrap();
            }
            let records = image.len().div_ceil(size) as u32;
            assert_eq!(
                last,
                LoadStep::End {
                    records,
                    bytes: image.len() as u32
                }
            );
            assert_eq!(check(&flash).result, Ok(()), "record size {size}");
            assert_eq!(&flash.bytes[0xC000..0xC000 + image.len()], &image[..]);
        }
    }

    #[test]
    fn a_repeated_record_is_not_erased() {
        let mut flash = Ram::new();
        let mut loader = Loader::new();
        let lines = hex_image(&[1, 2, 3, 4, 5, 6, 7, 8], 8);
        loader.record(&lines[0], &mut flash).unwrap();
        loader.record(&lines[1], &mut flash).unwrap();
        assert_eq!(
            loader.record(&lines[1], &mut flash),
            Err(BootError::NotErased)
        );
        // an overlapping record at 4 bytes in
        let overlap = record_line(0xC004, 0, &[9, 9, 9, 9]);
        assert_eq!(
            loader.record(&overlap, &mut flash),
            Err(BootError::NotErased)
        );
    }

    #[test]
    fn unaligned_head_and_tail_load() {
        let mut flash = Ram::new();
        let mut loader = Loader::new();
        loader
            .record(&record_line(0, 0x04, &[0x08, 0x00]), &mut flash)
            .unwrap();
        for (offset, data) in [(0xC001u16, vec![1u8, 2, 3]), (0xD003, vec![4, 5, 6, 7, 8])] {
            loader
                .record(&record_line(offset, 0, &data), &mut flash)
                .unwrap();
            let mut got = vec![0u8; data.len()];
            flash.read(0x0800_0000 + offset as u32, &mut got);
            assert_eq!(got, data);
        }
    }

    #[test]
    fn flash_faults_and_wrong_bits_are_reported() {
        let line = record_line(0xC000, 0, &[0x0F, 0xF0]);
        let mut flash = Ram::new();
        flash.wrong_bits = true;
        let mut loader = Loader::new();
        loader
            .record(&record_line(0, 0x04, &[8, 0]), &mut flash)
            .unwrap();
        assert_eq!(loader.record(&line, &mut flash), Err(BootError::Verify));
        for (fault, want) in [
            (
                FlashError::Programming,
                BootError::Flash(FlashError::Programming),
            ),
            (FlashError::NvmroAsserted, BootError::Nvmro),
        ] {
            let mut flash = Ram::new();
            flash.program_fault = Some(fault);
            let mut loader = Loader::new();
            loader
                .record(&record_line(0, 0x04, &[8, 0]), &mut flash)
                .unwrap();
            assert_eq!(loader.record(&line, &mut flash), Err(want));
        }
    }

    #[test]
    fn a_resend_after_a_checksum_error_succeeds() {
        let mut flash = Ram::new();
        let mut loader = Loader::new();
        loader
            .record(&record_line(0, 0x04, &[8, 0]), &mut flash)
            .unwrap();
        let line = record_line(0xC000, 0, &[1, 2, 3, 4]);
        let mut damaged = line.clone();
        damaged.replace_range(9..11, "FF");
        assert_eq!(
            loader.record(&damaged, &mut flash),
            Err(BootError::Checksum)
        );
        assert_eq!(loader.record(&line, &mut flash), Ok(LoadStep::Record));
        assert_eq!(
            loader.record(&record_line(0, 1, &[]), &mut flash),
            Ok(LoadStep::End {
                records: 1,
                bytes: 4
            })
        );
    }

    #[test]
    fn data_outside_app_or_before_an_address_record_is_a_range_error() {
        let mut flash = Ram::new();
        let mut loader = Loader::new();
        // before any 04 record the address is 0..0xFFFF
        assert_eq!(
            loader.record(&record_line(0xC000, 0, &[1]), &mut flash),
            Err(BootError::Range)
        );
        let rows: [(u16, u16, BootError); 4] = [
            (0x0800, 0x0000, BootError::Range),
            (0x0800, 0x8000, BootError::Range),
            (0x0800, 0xBFFF, BootError::Range),
            (0x0900, 0x0000, BootError::Range),
        ];
        for (upper, offset, want) in rows {
            loader
                .record(&record_line(0, 0x04, &upper.to_be_bytes()), &mut flash)
                .unwrap();
            assert_eq!(
                loader.record(&record_line(offset, 0, &[1, 2]), &mut flash),
                Err(want)
            );
        }
        assert!(flash.bytes.iter().all(|&b| b == 0xFF));
        // the last two bytes of APP load; a record running off the end does not
        loader
            .record(&record_line(0, 0x04, &[8, 7]), &mut flash)
            .unwrap();
        assert_eq!(
            loader.record(&record_line(0xFFFE, 0, &[1, 2]), &mut flash),
            Ok(LoadStep::Record)
        );
        assert_eq!(
            loader.record(&record_line(0xFFFF, 0, &[1, 2]), &mut flash),
            Err(BootError::Wrap)
        );
    }

    #[test]
    fn erase_orders_the_sectors() {
        let mut flash = Ram::new();
        erase(Target::App, &mut flash).unwrap();
        assert_eq!(flash.erased, [3, 4, 5, 6, 7]);
        let mut flash = Ram::new();
        erase(Target::Fru, &mut flash).unwrap();
        assert_eq!(flash.erased, [2]);
        let mut flash = Ram::new();
        flash.erase_fault_at = Some(5);
        assert_eq!(
            erase(Target::App, &mut flash),
            Err(BootError::Flash(FlashError::Timeout))
        );
        assert_eq!(flash.erased, [3, 4]);
    }

    #[test]
    fn an_erased_flash_has_no_image() {
        assert_eq!(check(&Ram::new()).result, Err(ImageError::Missing));
    }
}
