//! geometry - RM0368 Table 5 sectors of the STM32F401xE and the region policy of ERASE and LOAD.

/// Sector -- one flash sector: number, first address, length in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sector {
    pub number: u8,
    pub start: u32,
    pub len: u32,
}

const fn sector(number: u8, start: u32, len: u32) -> Sector {
    Sector { number, start, len }
}

const KB: u32 = 1024;

/// SECTORS -- the eight sectors of the STM32F401xE (RM0368 Table 5): 4 x 16 KB, 64 KB, 3 x 128 KB.
pub const SECTORS: [Sector; 8] = [
    sector(0, 0x0800_0000, 16 * KB),
    sector(1, 0x0800_4000, 16 * KB),
    sector(2, 0x0800_8000, 16 * KB),
    sector(3, 0x0800_C000, 16 * KB),
    sector(4, 0x0801_0000, 64 * KB),
    sector(5, 0x0802_0000, 128 * KB),
    sector(6, 0x0804_0000, 128 * KB),
    sector(7, 0x0806_0000, 128 * KB),
];
/// The table as a static, so `Target::sectors` can hand out slices of it.
static TABLE: [Sector; 8] = SECTORS;

pub const FLASH_START: u32 = 0x0800_0000;
pub const FLASH_END: u32 = 0x0808_0000;
/// BOOT -- sector 0, the bootloader.
pub const BOOT_START: u32 = 0x0800_0000;
pub const BOOT_END: u32 = 0x0800_4000;
/// STORE -- sector 1, the FRU flash store.
pub const STORE_START: u32 = 0x0800_4000;
pub const STORE_END: u32 = 0x0800_8000;
/// APP -- sectors 2 to 7, the runtime firmware.
pub const APP_START: u32 = 0x0800_8000;
pub const APP_END: u32 = 0x0808_0000;
/// APP_LEN_MAX -- bytes of the APP region (491 520).
pub const APP_LEN_MAX: u32 = APP_END - APP_START;
/// RAM: NOINIT 0x2000_0000..0x2000_0100 (boot request at offset 0), the rest above.
pub const RAM_START: u32 = 0x2000_0000;
pub const NOINIT_END: u32 = 0x2000_0100;
pub const RAM_END: u32 = 0x2001_8000;

/// Target -- what an erase command names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Fru,
    App,
}

impl Target {
    /// name -- the word of the command and the answer.
    pub const fn name(self) -> &'static str {
        match self {
            Target::Fru => "FRU",
            Target::App => "APP",
        }
    }

    /// sectors -- the sectors the target erases, in erase order: FRU is sector 1; APP is sectors
    /// 2..=7 with sector 2 first (it holds the vector table and the header).
    pub fn sectors(self) -> &'static [Sector] {
        let range = match self {
            Target::Fru => 1..2,
            Target::App => 2..8,
        };
        TABLE.get(range).unwrap_or(&[])
    }
}

/// load_allowed -- true when every byte of addr..addr+len lies inside APP (the only LOAD target:
/// owner 10-26-2026, nothing in the bootloader programs sector 0 or sector 1). Overflow-safe;
/// len 0 is false.
pub fn load_allowed(addr: u32, len: u32) -> bool {
    len > 0 && addr >= APP_START && addr.checked_add(len).is_some_and(|end| end <= APP_END)
}

/// sectors_contiguous -- the table runs from FLASH_START to FLASH_END without a gap.
const fn sectors_contiguous() -> bool {
    let mut next = FLASH_START;
    let mut i = 0;
    while i < SECTORS.len() {
        if SECTORS[i].number as usize != i || SECTORS[i].start != next {
            return false;
        }
        next += SECTORS[i].len;
        i += 1;
    }
    next == FLASH_END
}

const _: () = assert!(
    sectors_contiguous(),
    "SECTORS must run contiguously from FLASH_START to FLASH_END"
);
const _: () = assert!(
    STORE_START == ipmc_fru::persist::STORE_START
        && STORE_END == ipmc_fru::persist::STORE_START + ipmc_fru::persist::STORE_LEN,
    "STORE_START and STORE_END must equal the ipmc-fru store sector"
);
const _: () = assert!(
    BOOT_START == SECTORS[0].start
        && BOOT_END == SECTORS[1].start
        && STORE_START == SECTORS[1].start
        && STORE_END == SECTORS[2].start
        && APP_START == SECTORS[2].start
        && APP_END == SECTORS[7].start + SECTORS[7].len,
    "BOOT, STORE and APP must be sector 0, sector 1 and sectors 2-7 of SECTORS"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_allowed_at_every_boundary() {
        // (addr, len, allowed)
        let rows: [(u32, u32, bool); 14] = [
            (APP_START, 1, true),
            (APP_START, APP_LEN_MAX, true),
            (APP_END - 1, 1, true),
            (APP_START - 1, 1, false),
            (APP_END, 1, false),
            (APP_END - 1, 2, false),
            (APP_START - 2, 4, false),
            (BOOT_START, 16, false),
            (STORE_START, 16, false),
            (STORE_END - 1, 1, false),
            (APP_START, 0, false),
            (APP_START, u32::MAX, false),
            (u32::MAX, 2, false),
            (u32::MAX - 1, 4, false),
        ];
        for (addr, len, want) in rows {
            assert_eq!(load_allowed(addr, len), want, "{addr:#x} + {len}");
        }
    }

    #[test]
    fn target_sectors_and_names() {
        let fru: [u8; 1] = [1];
        let app: [u8; 6] = [2, 3, 4, 5, 6, 7];
        for (target, name, numbers) in [
            (Target::Fru, "FRU", &fru[..]),
            (Target::App, "APP", &app[..]),
        ] {
            let got: std::vec::Vec<u8> = target.sectors().iter().map(|s| s.number).collect();
            assert_eq!(got, numbers);
            assert_eq!(target.name(), name);
        }
        assert_eq!(APP_LEN_MAX, 491_520);
    }
}
