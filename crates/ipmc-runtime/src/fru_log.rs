// fru_log.rs - the owner of the FRU flash log state: the boot load before the I2C drivers start.
//
// Ownership: FruLog owns the board's flash (`FlashLog`) and the `Persist` state. The main loop's
// save step and the FRU console commands reach the pair through `parts` and `status`, so the IPMB
// pause around an erase (the link service) wraps every erase the same way. Nothing here runs in
// an interrupt handler.

use ipmc_fru::persist::{FlashLog, Persist, Status};
use ipmc_fru::FruStore;

use crate::text::ConsoleText;

/// ENABLED -- the flash log is built in (defs.rs IPMC_FRU_PERSIST).
pub const ENABLED: bool = ipmc_defs::IPMC_FRU_PERSIST;

/// FruLog -- the log state and the flash it runs on; `flash` is None when the log is off or
/// could not start (a console line says which), and the FRU store is then RAM only.
pub struct FruLog<F: FlashLog> {
    persist: Persist,
    flash: Option<F>,
}

impl<F: FlashLog> FruLog<F> {
    /// boot -- starts the log and loads the stored image; one console line each for the outcome.
    /// `flash` is the board's flash, or the text of why it did not start. Call it after the clock
    /// init and the UART, before the IPMB drivers start: erasing a sector without a valid record
    /// (see `Persist::boot`) stalls the core for up to 500 ms. Returns the log and the stored
    /// image; None means the built image stays in use (nothing stored, the log off, or a flash
    /// fault). While `nvmro_asserted` the boot erases nothing (VITA 46.11 Rec 4.1.10-1).
    // inline(never): the start-up's buffers live in this frame only while the log starts.
    #[inline(never)]
    pub fn boot(
        flash: Result<F, &'static str>,
        nvmro_asserted: bool,
        con: &mut impl ConsoleText,
    ) -> (FruLog<F>, Option<FruStore>) {
        let mut log = FruLog {
            persist: Persist::new(),
            flash: None,
        };
        if !ENABLED {
            con.print(format_args!(
                "FRU log: off (IPMC_FRU_PERSIST false), RAM only\n"
            ));
            return (log, None);
        }
        let mut flash = match flash {
            Ok(flash) => flash,
            Err(text) => {
                con.print(format_args!("FRU log: off, {}; RAM only\n", text));
                return (log, None);
            }
        };
        let (store, report) = match log.persist.boot(&mut flash, nvmro_asserted) {
            Ok(booted) => booted,
            Err(error) => {
                con.print(format_args!(
                    "FRU log: off, boot failed: {}; RAM only\n",
                    error.name()
                ));
                return (log, None);
            }
        };
        if let Some(error) = report.housekeeping {
            // The image found is kept and the log stays on; the next save erases what it needs.
            con.print(format_args!(
                "FRU log: boot housekeeping erase failed: {}; the stored image is kept, log stays on\n",
                error.name()
            ));
        }
        if report.erased {
            con.print(format_args!(
                "FRU log: erased the store sector at boot (no valid record)\n"
            ));
        }
        match report.loaded {
            Some(seq) => con.print(format_args!(
                "FRU log: loaded record {} ({} damaged slots skipped)\n",
                seq, report.skipped
            )),
            None => con.print(format_args!(
                "FRU log: no stored image, built image in use\n"
            )),
        }
        log.flash = Some(flash);
        (log, store)
    }

    /// parts -- the log state and the flash, None while the log is off.
    pub fn parts(&mut self) -> Option<(&mut Persist, &mut F)> {
        let flash = self.flash.as_mut()?;
        Some((&mut self.persist, flash))
    }

    /// flash -- the flash, None while the log is off.
    pub fn flash(&self) -> Option<&F> {
        self.flash.as_ref()
    }

    /// status -- the log state for FRU STORE, None while the log is off.
    pub fn status(&self) -> Option<Status> {
        self.flash.as_ref().map(|_| self.persist.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimSerial;
    use ipmc_fru::RamFlash;

    /// boot_text -- the console text of one boot over `flash`.
    fn boot_text(
        flash: Result<RamFlash, &'static str>,
        nvmro: bool,
    ) -> (FruLog<RamFlash>, Option<FruStore>, std::string::String) {
        let mut con = SimSerial::roomy();
        let (log, store) = FruLog::boot(flash, nvmro, &mut con);
        (log, store, con.text())
    }

    #[test]
    fn a_flash_that_did_not_start_leaves_the_log_off() {
        let (mut log, store, text) = boot_text(Err("no flash"), false);
        if ENABLED {
            assert_eq!(text, "FRU log: off, no flash; RAM only\r\n");
        }
        assert!(store.is_none());
        assert!(log.parts().is_none());
        assert!(log.status().is_none());
    }

    #[test]
    fn a_blank_sector_gives_no_stored_image() {
        if !ENABLED {
            return;
        }
        let (mut log, store, text) = boot_text(Ok(RamFlash::new()), false);
        assert_eq!(text, "FRU log: no stored image, built image in use\r\n");
        assert!(store.is_none());
        assert!(log.parts().is_some());
        assert_eq!(log.status().map(|status| status.records), Some(0));
    }

    #[test]
    fn a_damaged_sector_is_erased_unless_nvmro_holds_it() {
        if !ENABLED {
            return;
        }
        let mut damaged = RamFlash::new();
        damaged.sector.fill(0);
        let (_, _, text) = boot_text(Ok(damaged.clone()), false);
        assert_eq!(
            text,
            "FRU log: erased the store sector at boot (no valid record)\r\n\
             FRU log: no stored image, built image in use\r\n"
        );
        let (log, _, text) = boot_text(Ok(damaged), true);
        assert_eq!(text, "FRU log: no stored image, built image in use\r\n");
        assert!(
            log.status().is_some(),
            "the log stays on while NVMRO holds it"
        );
    }

    #[test]
    fn a_stored_image_is_loaded_and_reported() {
        if !ENABLED {
            return;
        }
        let mut flash = RamFlash::new();
        let mut writer = Persist::new();
        let mut con = SimSerial::roomy();
        let image = ipmc_fru::FruStore::new();
        let _ = writer.boot(&mut flash, false);
        assert!(writer.save(&mut flash, image.image(), 0).is_ok());
        let (log, store) = FruLog::boot(Ok(flash), false, &mut con);
        assert_eq!(
            con.text(),
            "FRU log: loaded record 1 (0 damaged slots skipped)\r\n"
        );
        assert!(store.is_some());
        assert_eq!(log.status().and_then(|status| status.newest_seq), Some(1));
    }

    #[test]
    fn a_sector_too_small_for_a_record_is_a_boot_failure() {
        if !ENABLED {
            return;
        }
        let (log, store, text) = boot_text(Ok(RamFlash::with_len(4)), false);
        assert!(text.starts_with("FRU log: off, boot failed: "));
        assert!(text.ends_with("; RAM only\r\n"));
        assert!(store.is_none());
        assert!(log.status().is_none());
    }
}
