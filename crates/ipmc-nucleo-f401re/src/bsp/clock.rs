// bsp/clock.rs - PLL bring-up to IPMC_SYSCLK_HZ (HSE bypass, HSI fallback) and RCC clock-tree decode.
//
// The ST-LINK MCU drives its MCO signal into OSC_IN (UM1724 6.7.1), so the HSE oscillator runs
// in bypass. The input frequency (ipmi_defs.h IPMC_HSE_HZ) and the target SYSCLK (IPMC_SYSCLK_HZ)
// are header values; the PLL M, N, P, Q below are datasheet-derived, and a compile-time check
// proves they produce IPMC_SYSCLK_HZ from IPMC_HSE_HZ and from HSI.
//
// PLL derivation for the default header values (RM0368 Rev 6 6.3.2 limits: VCO input 1 to 2 MHz, 192 <= N <= 432,
// VCO output 192 to 432 MHz):
//   HSE: 8 MHz  / M=8  = 1 MHz VCO input; x N=336 = 336 MHz VCO.
//   HSI: 16 MHz / M=16 = 1 MHz VCO input; x N=336 = 336 MHz VCO.
//   SYSCLK = 336 MHz / P=4 = 84 MHz; PLLQ = 336 MHz / Q=7 = 48 MHz.
//   HCLK 84 MHz, PCLK1 42 MHz (APB1 max), PCLK2 84 MHz.
// A 1 MHz VCO input is the lowest the RM allows (more jitter than 2 MHz) and keeps N inside
// the range the RM states for every device in the family.
// When HSE never becomes ready, HSI is the PLL source with the same VCO frequency.

use stm32f4::stm32f401::{FLASH, PWR, RCC};

/// HSE_HZ -- HSE input frequency, Hz (ipmi_defs.h IPMC_HSE_HZ).
pub const HSE_HZ: u32 = ipmc_defs::IPMC_HSE_HZ; // ST-LINK MCO, UM1724 6.7.1
/// HSI_HZ -- internal RC oscillator frequency, Hz (RM0368 6.2.2).
pub const HSI_HZ: u32 = 16_000_000; // internal RC oscillator, RM0368 6.2.2

// Timeout derivations. Each wait polls a register; one poll takes at least one CPU cycle, and
// the core runs from HSI at 16 MHz until the switch, so N polls last at least N / 16 MHz.
/// Poll count before HSE is declared absent: 100 000 polls >= 6.25 ms at 16 MHz. A bypass
/// input needs no oscillator start-up, so the ready flag follows within microseconds.
const HSE_TIMEOUT: u32 = 100_000;
/// Poll count for the flash latency read-back: 10 000 polls >= 625 us at 16 MHz; the register
/// read-back needs a few bus cycles.
const FLASH_LATENCY_TIMEOUT: u32 = 10_000;
/// Poll count for PLLRDY to clear after PLLON is cleared: 100 000 polls >= 6.25 ms at 16 MHz.
const PLL_OFF_TIMEOUT: u32 = 100_000;
/// Poll count for PLL lock: 100 000 polls >= 6.25 ms at 16 MHz against a PLL lock time of at
/// most 300 us (DS10086 Table 41).
const PLL_LOCK_TIMEOUT: u32 = 100_000;
/// Poll count for SWS to follow SW: 100 000 polls >= 1.19 ms even at 84 MHz; a switch to a
/// ready source completes within a few clock cycles.
const SWITCH_TIMEOUT: u32 = 100_000;

const PLL_M_HSE: u32 = 8;
const PLL_M_HSI: u32 = 16;
const PLL_N: u32 = 336;
const PLL_P_DIV: u32 = 4; // PLLP field value div4
const PLL_Q: u32 = 7;
/// RM0368 Rev 6 6.3.2: allowed PLLN range.
const PLL_N_MIN: u32 = 192;
const PLL_N_MAX: u32 = 432;
/// RM0368 6.3.2: VCO output range.
const VCO_MIN_HZ: u32 = 192_000_000;
const VCO_MAX_HZ: u32 = 432_000_000;
/// RM0368 6.3.2: VCO input range.
const VCO_IN_MIN_HZ: u32 = 1_000_000;
const VCO_IN_MAX_HZ: u32 = 2_000_000;
/// Target SYSCLK (ipmi_defs.h IPMC_SYSCLK_HZ) and the 48 MHz PLLQ output (the USB and SDIO
/// clock requirement, RM0368 6.3.2).
const SYSCLK_TARGET_HZ: u32 = ipmc_defs::IPMC_SYSCLK_HZ;
const PLLQ_TARGET_HZ: u32 = 48_000_000;

/// pll_legal -- true when source / m x n is a legal VCO setting for the given input frequency
/// and the P and Q dividers give exactly IPMC_SYSCLK_HZ and 48 MHz from it. The division is
/// exact: a source / m that is not a whole number of hertz fails the check.
const fn pll_legal(source_hz: u32, m: u32) -> bool {
    if !source_hz.is_multiple_of(m) {
        return false;
    }
    let vco_in = source_hz / m;
    let vco = vco_in * PLL_N;
    vco_in >= VCO_IN_MIN_HZ
        && vco_in <= VCO_IN_MAX_HZ
        && vco >= VCO_MIN_HZ
        && vco <= VCO_MAX_HZ
        && vco.is_multiple_of(PLL_P_DIV)
        && vco / PLL_P_DIV == SYSCLK_TARGET_HZ
        && vco.is_multiple_of(PLL_Q)
        && vco / PLL_Q == PLLQ_TARGET_HZ
}
// Field widths (RM0368 6.3.2): PLLM 6 bits (2..=63), PLLN 9 bits, PLLQ 4 bits (2..=15).
const _: () = assert!(PLL_M_HSE >= 2 && PLL_M_HSE <= 63 && PLL_M_HSI >= 2 && PLL_M_HSI <= 63);
const _: () = assert!(PLL_N <= 511 && PLL_Q >= 2 && PLL_Q <= 15);
const _: () = assert!(
    PLL_N >= PLL_N_MIN && PLL_N <= PLL_N_MAX,
    "PLLN outside RM0368 range"
);
const _: () = assert!(
    pll_legal(HSE_HZ, PLL_M_HSE),
    "HSE PLL setting (M, N, P, Q) does not produce IPMC_SYSCLK_HZ from IPMC_HSE_HZ within the RM0368 ranges"
);
const _: () = assert!(
    pll_legal(HSI_HZ, PLL_M_HSI),
    "HSI PLL setting (M, N, P, Q) does not produce IPMC_SYSCLK_HZ from the 16 MHz HSI within the RM0368 ranges"
);

/// ClockFault -- A bring-up step that ran out of its bound. The system stays on HSI when this is returned.
#[derive(Clone, Copy)]
pub enum ClockFault {
    FlashLatency,
    PllOff,
    PllLock,
    SwitchToPll,
    SwitchBackToHsi,
}

impl ClockFault {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            ClockFault::FlashLatency => "flash latency not applied",
            ClockFault::PllOff => "PLL did not stop",
            ClockFault::PllLock => "PLL did not lock",
            ClockFault::SwitchToPll => "SYSCLK switch to PLL timed out (back on HSI)",
            ClockFault::SwitchBackToHsi => "SYSCLK fallback to HSI timed out",
        }
    }
}

/// ClockSource -- what drives SYSCLK.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    HsePll,
    HsiPll,
    Hse,
    Hsi,
}

impl ClockSource {
    /// text -- text for the console.
    pub fn text(self) -> &'static str {
        match self {
            ClockSource::HsePll => "HSE from ST-LINK MCO (bypass), PLL",
            ClockSource::HsiPll => "HSI 16 MHz internal (fallback, no MCO clock), PLL",
            ClockSource::Hse => "HSE direct",
            ClockSource::Hsi => "HSI 16 MHz internal, PLL off",
        }
    }
}

/// ClockInfo -- clock tree decoded from RCC registers.
#[derive(Clone, Copy)]
pub struct ClockInfo {
    pub source: ClockSource,
    pub sysclk_hz: u32,
    pub hclk_hz: u32,
    pub pclk1_hz: u32,
    pub pclk2_hz: u32,
}

/// Polls `done` at most `bound` times. Returns true when it became true.
fn wait_for(bound: u32, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..bound {
        if done() {
            return true;
        }
    }
    false
}

fn start_hse_bypass(rcc: &RCC) -> bool {
    // HSEBYP must be set before HSEON (RM0368 6.3.1).
    rcc.cr().modify(|_, w| w.hsebyp().bypassed());
    rcc.cr().modify(|_, w| w.hseon().on());
    if wait_for(HSE_TIMEOUT, || rcc.cr().read().hserdy().is_ready()) {
        return true;
    }
    // No clock arrived: release the pin.
    rcc.cr().modify(|_, w| w.hseon().off());
    rcc.cr().modify(|_, w| w.hsebyp().not_bypassed());
    false
}

/// init -- brings SYSCLK to IPMC_SYSCLK_HZ. On a fault the clock stays on HSI and the fault is returned.
pub fn init(rcc: &RCC, pwr: &PWR, flash: &FLASH) -> Result<(), ClockFault> {
    // Regulator scale 2 (VOS = 0b10) allows HCLK up to 84 MHz (RM0368 5.4.1).
    rcc.apb1enr().modify(|_, w| w.pwren().enabled());
    // Read back: short delay after enabling a clock.
    let _ = rcc.apb1enr().read();
    // SAFETY: VOS = 0b10 is the documented scale 2 value.
    pwr.cr().modify(|_, w| unsafe { w.vos().bits(0b10) });

    // 2 wait states for 60 < HCLK <= 84 MHz at 2.7-3.6 V (RM0368 3.4), prefetch and caches on.
    // SAFETY: LATENCY = 2 is a valid 4-bit field value.
    flash.acr().write(|w| {
        unsafe { w.latency().bits(2) }
            .prften()
            .enabled()
            .icen()
            .enabled()
            .dcen()
            .enabled()
    });
    if !wait_for(FLASH_LATENCY_TIMEOUT, || {
        flash.acr().read().latency().bits() == 2
    }) {
        return Err(ClockFault::FlashLatency);
    }

    let (pll_m, from_hse) = if start_hse_bypass(rcc) {
        (PLL_M_HSE, true)
    } else {
        (PLL_M_HSI, false)
    };

    // The PLL is off while it is configured.
    rcc.cr().modify(|_, w| w.pllon().off());
    if !wait_for(PLL_OFF_TIMEOUT, || rcc.cr().read().pllrdy().bit_is_clear()) {
        return Err(ClockFault::PllOff);
    }
    // SAFETY: M = 8 or 16, N = 336, Q = 7 are inside their field ranges (RM0368 6.3.2); the
    // const assertions above check the VCO limits. The values fit u8 and u16 (checked there too).
    rcc.pllcfgr().write(|w| unsafe {
        w.pllm()
            .bits(pll_m as u8)
            .plln()
            .bits(PLL_N as u16)
            .pllp()
            .div4()
            .pllq()
            .bits(PLL_Q as u8)
    });
    rcc.pllcfgr().modify(|_, w| {
        if from_hse {
            w.pllsrc().hse()
        } else {
            w.pllsrc().hsi()
        }
    });
    rcc.cr().modify(|_, w| w.pllon().on());
    if !wait_for(PLL_LOCK_TIMEOUT, || rcc.cr().read().pllrdy().is_ready()) {
        // EVIDENCE: PLLRDY never set. Stop the PLL so it cannot lock later and be selected by a
        // pending request (RM0368 6.2.6, 6.3.1: PLLON may be cleared while it is not SYSCLK).
        rcc.cr().modify(|_, w| w.pllon().off());
        return Err(ClockFault::PllLock);
    }

    // AHB /1, APB1 /2 (42 MHz), APB2 /1, then SYSCLK = PLL.
    rcc.cfgr()
        .modify(|_, w| w.hpre().div1().ppre1().div2().ppre2().div1());
    rcc.cfgr().modify(|_, w| w.sw().pll());
    if !wait_for(SWITCH_TIMEOUT, || rcc.cfgr().read().sws().is_pll()) {
        // EVIDENCE: SWS never reported the PLL. A pending selection completes once its source
        // is ready (RM0368 6.2.6), which would move PCLK1 under a UART divisor computed for
        // HSI, so the request is withdrawn and the fallback confirmed (RM0368 6.3.3: SWS).
        return Err(fall_back_to_hsi(rcc));
    }
    Ok(())
}

/// Requests HSI as SYSCLK and waits for SWS to confirm it; then stops the PLL.
/// Returns the fault to report: `SwitchToPll` when the fallback is confirmed,
/// `SwitchBackToHsi` when even HSI did not take over.
fn fall_back_to_hsi(rcc: &RCC) -> ClockFault {
    rcc.cfgr().modify(|_, w| w.sw().hsi());
    if wait_for(SWITCH_TIMEOUT, || rcc.cfgr().read().sws().is_hsi()) {
        rcc.cr().modify(|_, w| w.pllon().off());
        ClockFault::SwitchToPll
    } else {
        ClockFault::SwitchBackToHsi
    }
}

/// AHB prescaler: HPRE 0xxx = /1, 1000..1111 = /2,/4,/8,/16,/64,/128,/256,/512 (RM0368 6.3.3).
const fn ahb_divider(hpre: u8) -> u32 {
    const DIV: [u32; 8] = [2, 4, 8, 16, 64, 128, 256, 512];
    if hpre & 0x8 != 0 {
        DIV[(hpre & 0x7) as usize]
    } else {
        1
    }
}

/// APB prescaler: PPRE 0xx = /1, 100..111 = /2,/4,/8,/16 (RM0368 6.3.3).
const fn apb_divider(ppre: u8) -> u32 {
    if ppre & 0x4 != 0 {
        2 << (ppre & 0x3)
    } else {
        1
    }
}

/// decode_nominal -- decodes the running clock tree from RCC registers.
///
/// Computes nominal frequencies from the RCC settings and the fixed oscillator constants
/// (`HSE_HZ`, `HSI_HZ`). No frequency is measured: the HSI tolerance (DS10086 Table 39) and the
/// accuracy of the 48 MHz PLLQ output are not included.
pub fn decode_nominal(rcc: &RCC) -> ClockInfo {
    let cfgr = rcc.cfgr().read();
    let (source, sysclk_hz) = if cfgr.sws().is_pll() {
        let pll = rcc.pllcfgr().read();
        let m = u32::from(pll.pllm().bits());
        let n = u32::from(pll.plln().bits());
        let p = (u32::from(pll.pllp().bits()) + 1) * 2;
        let (source, in_hz) = if pll.pllsrc().is_hse() {
            (ClockSource::HsePll, HSE_HZ)
        } else {
            (ClockSource::HsiPll, HSI_HZ)
        };
        (source, (in_hz / m) * n / p)
    } else if cfgr.sws().is_hse() {
        (ClockSource::Hse, HSE_HZ)
    } else {
        (ClockSource::Hsi, HSI_HZ)
    };
    let hclk_hz = sysclk_hz / ahb_divider(cfgr.hpre().bits());
    ClockInfo {
        source,
        sysclk_hz,
        hclk_hz,
        pclk1_hz: hclk_hz / apb_divider(cfgr.ppre1().bits()),
        pclk2_hz: hclk_hz / apb_divider(cfgr.ppre2().bits()),
    }
}
