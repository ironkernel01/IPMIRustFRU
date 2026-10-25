// fru_log.rs - the firmware side of the FRU flash log: the boot load before the I2C drivers
// start, and the owner of the log state (ipmc_fru::persist::Persist) and the flash driver.
//
// Ownership: FruLog owns the `bsp::flash::Flash` and the `Persist` state. The main loop's save
// step and the FRU console commands live in ipmb_link.rs and data_cmds.rs; they reach the pair
// through `parts` and `status`, so the IPMB pause around an erase (ipmb_link.rs) wraps every
// erase the same way. Nothing here runs in an interrupt handler.

use ipmc_fru::persist::{Persist, Status};
use ipmc_fru::FruStore;
use stm32f4::stm32f401::FLASH;

use crate::bsp::flash::Flash;

/// ENABLED -- the flash log is built in (ipmi_defs.h IPMC_FRU_PERSIST).
pub const ENABLED: bool = ipmc_defs::IPMC_FRU_PERSIST == 1;

/// FruLog -- the log state and the flash it runs on; `flash` is None when the log is off or
/// could not start (a console line says which), and the FRU store is then RAM only.
pub struct FruLog {
    persist: Persist,
    flash: Option<Flash>,
}

impl FruLog {
    /// boot -- starts the log and loads the stored image; one console line each for the outcome.
    /// Call it after the clock init and the UART, before the IPMB drivers start: erasing a
    /// sector without a valid record (see `Persist::boot`) stalls the core for up to 500 ms.
    /// Returns the log and the stored image; None means the built image stays in use (nothing
    /// stored, the log off, or a flash fault). While `nvmro_asserted` the boot erases nothing
    /// (VITA 46.11 Rec 4.1.10-1).
    pub fn boot(regs: FLASH, nvmro_asserted: bool) -> (FruLog, Option<FruStore>) {
        let mut log = FruLog {
            persist: Persist::new(),
            flash: None,
        };
        if !ENABLED {
            out!("FRU log: off (IPMC_FRU_PERSIST 0), RAM only\n");
            return (log, None);
        }
        let mut flash = match Flash::new(regs) {
            Ok(flash) => flash,
            Err(error) => {
                out!("FRU log: off, {}; RAM only\n", error.text());
                return (log, None);
            }
        };
        let (store, report) = match log.persist.boot(&mut flash, nvmro_asserted) {
            Ok(booted) => booted,
            Err(error) => {
                out!("FRU log: off, boot failed: {}; RAM only\n", error.name());
                return (log, None);
            }
        };
        if let Some(error) = report.housekeeping {
            // The image found is kept and the log stays on; the next save erases what it needs.
            out!(
                "FRU log: boot housekeeping erase failed: {}; the stored image is kept, log stays on\n",
                error.name()
            );
        }
        if report.erased {
            out!("FRU log: erased the store sector at boot (no valid record)\n");
        }
        match report.loaded {
            Some(seq) => out!(
                "FRU log: loaded record {} ({} damaged slots skipped)\n",
                seq,
                report.skipped
            ),
            None => out!("FRU log: no stored image, built image in use\n"),
        }
        log.flash = Some(flash);
        (log, store)
    }

    /// parts -- the log state and the flash, None while the log is off.
    pub fn parts(&mut self) -> Option<(&mut Persist, &mut Flash)> {
        let flash = self.flash.as_mut()?;
        Some((&mut self.persist, flash))
    }

    /// status -- the log state for FRU STORE, None while the log is off.
    pub fn status(&self) -> Option<Status> {
        self.flash.as_ref().map(|_| self.persist.status())
    }
}
