//! adc.rs - ADC1 backend: VDDA and die temperature from the internal channels, and the four
//! external rail inputs. Implements `AnalogSource`.
//!
//! Ownership: this module owns ADC1, ADC_COMMON (ADC_CCR) and the analog function of the external
//! input pins, claimed through `ports` (owner name "adc"). The pins and the ADC channel numbers
//! are the board's choice: the board support hands them in as an `AdcMap` (ipmc-nucleo-f401re
//! drivers/board_pins.rs); this module holds no board pin or channel number.
//!
//! Clock: ADCCLK = PCLK2 / 4 (ADC_CCR.ADCPRE = 01b). At the 84 MHz PCLK2 that is 21 MHz, within
//! the 36 MHz limit for VDDA >= 2.4 V (DS10086 Table 66); `init` refuses a PCLK2 above 144 MHz.
//!
//! Sampling time: every channel uses 480 cycles (SMPx = 111b, RM0368 11.12.5). DS10086 asks at
//! least 10 us for the temperature sensor (Table 72) and for VREFINT (Table 75). At 21 MHz one
//! cycle is 47.6 ns, so 480 cycles = 22.9 us (>= 10 us); the next shorter setting, 144 cycles,
//! gives 6.9 us and fails. One conversion = 480 + 12 cycles = 492 cycles = 23.4 us (RM0368 11.5).
//!
//! Conversion: one software-triggered single conversion per read (SQR1.L = 0, SQR3 = channel,
//! CR2.SWSTART), the end flag SR.EOC polled for at most `EOC_POLLS` iterations. `AVERAGE_COUNT`
//! conversions are averaged to cut noise.

use stm32f4::stm32f401::{ADC1, ADC_COMMON, RCC};

use ipmc_sensors::board_sensors::EXTERNAL_INPUTS;
use ipmc_sensors::eval::rail;
use ipmc_sensors::eval::temperature::{self, Calibration};
use ipmc_sensors::hal::{AnalogSource, Divider, SourceError};

use crate::bsp::ports::{self, PinId, Pins, PortError};

const OWNER: &str = "adc";

/// ExternalInput -- one external rail input: its pin and the ADC1 channel that pin feeds
/// (DS10086 Table 9 gives the pin-to-channel relation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalInput {
    pub pin: PinId,
    pub adc_channel: u8,
}

/// AdcMap -- the board's ADC assignment: the two internal channels (RM0368 11.9 and 11.10) and
/// the external inputs, indexed by the abstract channel id the board table uses
/// (`Channel::Adc(n)` is entry n).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdcMap {
    /// ADC1 channel of VREFINT.
    pub vrefint_channel: u8,
    /// ADC1 channel of the temperature sensor.
    pub temperature_channel: u8,
    pub external: [ExternalInput; EXTERNAL_INPUTS],
}

/// Maximum ADC clock, Hz, at VDDA >= 2.4 V (DS10086 Table 66).
const ADC_CLOCK_MAX_HZ: u32 = 36_000_000;
/// ADC_CCR.ADCPRE = 01b divides PCLK2 by 4 (RM0368 11.13.1).
const ADC_PRESCALER: u32 = 4;

/// SMPR1 holds SMP10..SMP18 in 27 bits, SMPR2 holds SMP0..SMP9 in 30 bits; 111b in every field
/// selects 480 cycles (RM0368 11.12.5, 11.12.6).
const SMPR1_ALL_480: u32 = 0x07FF_FFFF;
const SMPR2_ALL_480: u32 = 0x3FFF_FFFF;
/// CR2 bits: ADON = bit 0, SWSTART = bit 30 (RM0368 11.12.3). SR bit 1 = EOC (11.12.1).
const CR2_ADON: u32 = 1 << 0;
const CR2_SWSTART: u32 = 1 << 30;
const SR_EOC: u32 = 1 << 1;
/// DR holds the 12-bit result right-aligned (RM0368 11.12.14).
const DR_MASK: u32 = 0xFFF;

/// Poll bound for EOC. A conversion takes 492 cycles = 23.4 us at 21 MHz. Each poll is at least
/// one core cycle, so 100 000 polls last at least 1.19 ms at 84 MHz (6.25 ms at 16 MHz): more
/// than 50 times the conversion.
const EOC_POLLS: u32 = 100_000;
/// Conversions averaged per reading (defs.rs IPMC_ADC_AVERAGE): 8 x 23.4 us = 187 us per
/// channel read at the default.
const AVERAGE_COUNT: u32 = ipmc_defs::IPMC_ADC_AVERAGE;
/// Core cycles to wait after ADON and TSVREFE: the temperature sensor starts in 10 us
/// (DS10086 Table 72) and the ADC stabilises in 3 us (RM0368 11.3.1). 2000 cycles are 23.8 us
/// at 84 MHz, over twice the longer time.
const STARTUP_CYCLES: u32 = 2000;

/// System-memory addresses of the factory calibration values (DS10086 Tables 73 and 76).
const TS_CAL1_ADDRESS: usize = 0x1FFF_7A2C;
const TS_CAL2_ADDRESS: usize = 0x1FFF_7A2E;
const VREFIN_CAL_ADDRESS: usize = 0x1FFF_7A2A;

/// InitError -- why `init` refused to start the ADC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    /// PCLK2 / 4 would exceed the 36 MHz ADC clock limit.
    ClockTooFast,
    /// A pin is already claimed by another module.
    Pins(PortError),
}

impl InitError {
    /// name -- text for console lines.
    pub fn name(self) -> &'static str {
        match self {
            InitError::ClockTooFast => "ADC clock above 36 MHz",
            InitError::Pins(_) => "ADC pin already claimed",
        }
    }
}

/// Adc -- ADC1 with its calibration.
pub struct Adc {
    adc: ADC1,
    // Held so no other module can reach ADC_CCR while ADC1 runs; written once in `init`.
    _common: ADC_COMMON,
    map: AdcMap,
    // Held for the life of the ADC: the claims stay with this module. A slot is None when the
    // external group is not hardware (no pin claimed, input unavailable).
    _pins: [Option<Pins>; EXTERNAL_INPUTS],
    external_claimed: bool,
    calibration: Result<Calibration, SourceError>,
}

/// read_calibration_word -- one 16-bit factory value.
fn read_calibration_word(address: usize) -> u16 {
    // SAFETY: the address is a documented, always-mapped, 2-byte-aligned read-only location in
    // the STM32F401 system memory (DS10086 Tables 73 and 76).
    unsafe { core::ptr::read_volatile(address as *const u16) }
}

/// init -- starts ADC1 for the internal and external channels.
///
/// 1. Reject a PCLK2 whose /4 exceeds the ADC clock limit.
/// 2. When `claim_external` (the external rail group is hardware), claim the external input pins
///    of `map` and put them in analog mode with no pull; enable the ADC1 clock.
/// 3. ADC_CCR: ADCPRE = /4, TSVREFE on (VREFINT and the temperature sensor), VBATE off (VBAT
///    shares channel 18 and must stay off while TSVREFE is set, RM0368 11.13.1).
/// 4. 480-cycle sampling on all channels, 12-bit, one conversion in the sequence, ADC on.
/// 5. Wait the start-up time, then read the calibration values; invalid values do not stop
///    `init`: every internal reading then reports `CalibrationInvalid`.
pub fn init(
    rcc: &RCC,
    adc: ADC1,
    common: ADC_COMMON,
    pclk2_hz: u32,
    map: &AdcMap,
    claim_external: bool,
) -> Result<Adc, InitError> {
    if pclk2_hz / ADC_PRESCALER > ADC_CLOCK_MAX_HZ {
        return Err(InitError::ClockTooFast);
    }
    let mut pins: [Option<Pins>; EXTERNAL_INPUTS] = core::array::from_fn(|_| None);
    if claim_external {
        for (slot, input) in pins.iter_mut().zip(map.external.iter()) {
            let claimed = ports::claim_pin(OWNER, input.pin).map_err(InitError::Pins)?;
            claimed.analog();
            *slot = Some(claimed);
        }
    }
    rcc.apb2enr().modify(|_, w| w.adc1en().enabled());
    let _ = rcc.apb2enr().read(); // short delay after enabling a clock

    common
        .ccr()
        .modify(|_, w| w.adcpre().div4().tsvrefe().enabled().vbate().disabled());
    // SAFETY: 111b in every sampling-time field (480 cycles); the fields cover all of the registers.
    adc.smpr1().write(|w| unsafe { w.bits(SMPR1_ALL_480) });
    // SAFETY: as above.
    adc.smpr2().write(|w| unsafe { w.bits(SMPR2_ALL_480) });
    // SAFETY: CR1 = 0: 12-bit resolution, no scan, no interrupts, no watchdog.
    adc.cr1().write(|w| unsafe { w.bits(0) });
    // SAFETY: SQR1.L = 0: a sequence of one conversion.
    adc.sqr1().write(|w| unsafe { w.bits(0) });
    // SAFETY: CR2 = ADON only: software trigger, single conversion mode, right alignment.
    adc.cr2().write(|w| unsafe { w.bits(CR2_ADON) });
    cortex_m::asm::delay(STARTUP_CYCLES);

    let calibration = Calibration {
        ts_cal1: read_calibration_word(TS_CAL1_ADDRESS),
        ts_cal2: read_calibration_word(TS_CAL2_ADDRESS),
        vrefin_cal: read_calibration_word(VREFIN_CAL_ADDRESS),
    }
    .validated();
    Ok(Adc {
        adc,
        _common: common,
        map: *map,
        _pins: pins,
        external_claimed: claim_external,
        calibration,
    })
}

impl Adc {
    /// convert_once -- one software-triggered conversion of the channel in SQR3.
    fn convert_once(&self) -> Result<u32, SourceError> {
        // SAFETY: clears every status flag (SR bits are write-0-to-clear) before the start.
        self.adc.sr().write(|w| unsafe { w.bits(0) });
        // SAFETY: sets SWSTART; ADON stays set, the other CR2 bits keep their value.
        self.adc
            .cr2()
            .modify(|r, w| unsafe { w.bits(r.bits() | CR2_SWSTART) });
        for _ in 0..EOC_POLLS {
            if self.adc.sr().read().bits() & SR_EOC != 0 {
                return Ok(self.adc.dr().read().bits() & DR_MASK);
            }
        }
        Err(SourceError::ConversionTimeout)
    }

    /// read_counts -- the average of `AVERAGE_COUNT` conversions of `channel`, in raw counts.
    fn read_counts(&self, channel: u8) -> Result<u32, SourceError> {
        // SAFETY: SQR3.SQ1 = channel (0..=18, five bits); the other sequence slots are unused.
        self.adc
            .sqr3()
            .write(|w| unsafe { w.bits(u32::from(channel)) });
        let mut sum = 0;
        for _ in 0..AVERAGE_COUNT {
            sum += self.convert_once()?;
        }
        Ok(sum / AVERAGE_COUNT)
    }

    /// vdda_now -- VDDA from a fresh VREFINT reading.
    fn vdda_now(&self) -> Result<u32, SourceError> {
        let cal = self.calibration?;
        temperature::vdda_mv(self.read_counts(self.map.vrefint_channel)?, &cal)
    }
}

impl AnalogSource for Adc {
    fn vdda_mv(&mut self) -> Result<u32, SourceError> {
        self.vdda_now()
    }

    fn die_temperature_dc(&mut self) -> Result<i32, SourceError> {
        let cal = self.calibration?;
        let vdda = self.vdda_now()?;
        let counts = self.read_counts(self.map.temperature_channel)?;
        temperature::die_temperature_dc(counts, vdda, &cal)
    }

    fn rail_mv(&mut self, channel: u8, divider: Divider) -> Result<u32, SourceError> {
        // `channel` is the abstract id: an index into the board map.
        let input = self
            .map
            .external
            .get(usize::from(channel))
            .ok_or(SourceError::UnknownChannel)?;
        if !self.external_claimed {
            return Err(SourceError::Unavailable);
        }
        let vdda = self.vdda_now()?;
        let counts = self.read_counts(input.adc_channel)?;
        rail::scale_mv(temperature::pin_mv(counts, vdda)?, divider)
    }
}
