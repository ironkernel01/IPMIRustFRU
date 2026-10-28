// watchdog.rs - the independent watchdog (IWDG, RM0368 chapter 18), clocked by the LSI.
//
// Ownership: this module owns the IWDG registers and the DBGMCU_APB1_FZ bit DBG_IWDG_STOP (no
// other module touches DBGMCU); `start` sets the bit. The runtime starts the watchdog just before the main loop and
// feeds it once per loop pass, nowhere else (PLAN decision 42); the bootloader does not start it.
// Once started only a reset stops the IWDG.
//
// The IWDG counts down from the 12-bit reload (IWDG_RLR) at LSI / (4 * 2^PR); the reset fires when
// the counter reaches 0, so the timeout is 4 * 2^PR * (RLR + 1) / LSI. The LSI is an RC oscillator
// (DS10086, LSI characteristics: 17 kHz minimum, 32 kHz typical, 47 kHz maximum), so a period set
// at the nominal 32 kHz is shorter on a fast part: `LSI_MAX_HZ` bounds the shortest real period.
// No `core::fmt` is reachable from this file (the library rule).

use ipmc_hal::system::Watchdog as WatchdogService;
use stm32f4::stm32f401::{DBGMCU, IWDG};

pub use ipmc_hal::system::WatchdogError;

/// LSI_NOMINAL_HZ -- LSI typical frequency (DS10086, LSI characteristics, 32 kHz). The periods of
/// `setting` and `IPMC_WATCHDOG_MS` are at this clock.
pub const LSI_NOMINAL_HZ: u32 = 32_000;
/// LSI_MAX_HZ -- LSI maximum (DS10086, 47 kHz): the watchdog runs fastest, so its period is
/// shortest.
pub const LSI_MAX_HZ: u32 = 47_000;
/// LSI_MIN_HZ -- LSI minimum (DS10086, 17 kHz): the register update of `start` takes longest.
const LSI_MIN_HZ: u32 = 17_000;
/// ERASE_MAX_MS -- longest stall of the main loop: one 16 KB FRU sector erase (DS10086 flash
/// characteristics, 500 ms).
const ERASE_MAX_MS: u32 = 500;
/// PR_MAX -- highest IWDG_PR code (divider /256).
const PR_MAX: u8 = 6;
/// RLR_MAX -- highest IWDG_RLR value (12 bits).
const RLR_MAX: u16 = 0x0FFF;
/// DIVIDER_BASE -- the divider at PR code 0 (RM0368 IWDG_PR: /4); each code doubles it.
const DIVIDER_BASE: u64 = 4;
/// MS_PER_S -- milliseconds in a second.
const MS_PER_S: u64 = 1000;
/// SR_POLLS -- polls of IWDG_SR (PVU, RVU) that `start` waits. The update of PR and RLR crosses
/// into the LSI domain and takes up to 5 LSI periods at the slowest LSI (17 kHz): 5 / 17 kHz =
/// about 300 us. A poll of SR takes at least 2 core cycles (24 ns at 84 MHz), so 50 000 polls last
/// at least 1.2 ms, four times the longest update.
const SR_POLLS: u32 = 50_000;

// RM0368 IWDG_KR keys: the PAC names them `KEY::Start` (0xCCCC, starts the watchdog and forces
// the LSI on), `KEY::Unlock` (0x5555, enables write access to PR and RLR) and `KEY::Feed`
// (0xAAAA, reloads the counter from RLR).

// WatchdogError lives in ipmc-hal (plain data, shared with every board), re-exported above.

/// Setting -- prescaler code (IWDG_PR, 0..=6 for /4../256), reload (IWDG_RLR, 0..=4095) and the
/// period in ms they give at `LSI_NOMINAL_HZ`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub pr: u8,
    pub rlr: u16,
    pub period_ms: u32,
}

/// setting -- the smallest prescaler whose 12-bit reload counts `period_ms`; timeout =
/// 4 * 2^pr * (rlr + 1) / LSI (RM0368 IWDG). The period set is at most `period_ms` (rounded
/// down). None if `period_ms` is 0 or above the /256 maximum.
pub const fn setting(period_ms: u32) -> Option<Setting> {
    let mut pr: u8 = 0;
    while pr <= PR_MAX {
        let divider = DIVIDER_BASE << pr;
        // counts = period * LSI / (divider * 1000), rounded down.
        let counts = (period_ms as u64) * (LSI_NOMINAL_HZ as u64) / (divider * MS_PER_S);
        if counts >= 1 && counts <= (RLR_MAX as u64) + 1 {
            // counts <= 4096 and the period fits 32 bits, so the casts keep the value.
            let actual = counts * divider * MS_PER_S / (LSI_NOMINAL_HZ as u64);
            return Some(Setting {
                pr,
                rlr: (counts - 1) as u16,
                period_ms: actual as u32,
            });
        }
        pr += 1;
    }
    None
}

// The shortest real period (fastest LSI) of the configured period exceeds twice the longest stall
// of the main loop, so a FRU sector erase never trips the watchdog.
const _: () = {
    if ipmc_defs::IPMC_WATCHDOG_ENABLE {
        match setting(ipmc_defs::IPMC_WATCHDOG_MS) {
            Some(s) => assert!(
                (s.period_ms as u64) * (LSI_NOMINAL_HZ as u64) / (LSI_MAX_HZ as u64)
                    > 2 * (ERASE_MAX_MS as u64),
                "IPMC_WATCHDOG_MS: the period at the fastest LSI must exceed twice the 500 ms FRU sector erase"
            ),
            None => panic!("IPMC_WATCHDOG_MS is outside what the independent watchdog counts"),
        }
    }
};

// Known values at 32 kHz, smallest prescaler first.
const _: () = {
    assert!(matches!(
        setting(4000),
        Some(Setting {
            pr: 3,
            rlr: 3999,
            period_ms: 4000
        })
    ));
    assert!(matches!(
        setting(1000),
        Some(Setting {
            pr: 1,
            rlr: 3999,
            period_ms: 1000
        })
    ));
    assert!(matches!(
        setting(32_768),
        Some(Setting {
            pr: 6,
            rlr: 4095,
            period_ms: 32_768
        })
    ));
    assert!(setting(40_000).is_none());
    assert!(setting(0).is_none());
    assert!(LSI_MIN_HZ < LSI_NOMINAL_HZ && LSI_NOMINAL_HZ < LSI_MAX_HZ);
};

/// Watchdog -- the IWDG and what `start` set.
pub struct Watchdog {
    iwdg: IWDG,
    dbgmcu: DBGMCU,
    /// The IWDG is counting (KR start written); `feed` reloads it.
    running: bool,
    /// The period `start` set, once it returned Ok.
    started: Option<u32>,
}

impl Watchdog {
    /// new -- takes the IWDG and the DBGMCU (`start` sets DBG_IWDG_STOP in it). Writes nothing.
    pub fn new(iwdg: IWDG, dbgmcu: DBGMCU) -> Self {
        Watchdog {
            iwdg,
            dbgmcu,
            running: false,
            started: None,
        }
    }
}

impl WatchdogService for Watchdog {
    /// start -- DBG_IWDG_STOP (the counter stops while a debugger halts the core), then ST's
    /// order: KR start (forces the LSI on), KR unlock (write access), PR, RLR, wait until SR
    /// (PVU, RVU) reads 0 within `SR_POLLS` polls (the counter runs the reset value, about
    /// 512 ms, meanwhile), KR feed (reload). Returns the period set.
    /// Once started only a reset stops it. A second call returns Ok(previous) and changes nothing.
    /// Err(PeriodOutOfRange): nothing is written and the IWDG is not started. Err(NotReady): the
    /// IWDG runs with the values it has.
    fn start(&mut self, period_ms: u32) -> Result<u32, WatchdogError> {
        if let Some(previous) = self.started {
            return Ok(previous);
        }
        let setting = setting(period_ms).ok_or(WatchdogError::PeriodOutOfRange)?;
        self.dbgmcu
            .apb1_fz()
            .modify(|_, w| w.dbg_iwdg_stop().set_bit());
        self.iwdg.kr().write(|w| w.key().start());
        self.running = true;
        self.iwdg.kr().write(|w| w.key().unlock());
        self.iwdg.pr().write(|w| w.pr().set(setting.pr));
        self.iwdg.rlr().write(|w| w.rl().set(setting.rlr));
        let mut polls = 0;
        loop {
            let sr = self.iwdg.sr().read();
            if sr.pvu().bit_is_clear() && sr.rvu().bit_is_clear() {
                break;
            }
            polls += 1;
            if polls >= SR_POLLS {
                return Err(WatchdogError::NotReady);
            }
        }
        self.iwdg.kr().write(|w| w.key().feed());
        self.started = Some(setting.period_ms);
        Ok(setting.period_ms)
    }

    /// feed -- KR feed; does nothing before the IWDG runs.
    fn feed(&mut self) {
        if self.running {
            self.iwdg.kr().write(|w| w.key().feed());
        }
    }

    /// period_ms -- Some(period set) once started.
    fn period_ms(&self) -> Option<u32> {
        self.started
    }
}
