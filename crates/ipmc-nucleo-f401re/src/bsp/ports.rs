//! ports.rs - the ONLY owner of GPIOA, GPIOB and GPIOC and of their RCC clock enables.
//!
//! Every other module asks for the pins it uses with `claim(owner, port, pin_mask)` and gets a
//! `Pins` handle that can configure, read and write those pins and no others. A pin has one
//! owner: a second claim fails with `PortError::AlreadyClaimed`, naming both owners (the
//! firmware prints that at start-up and fail-stops).
//!
//! Why here: the board support library (bsp) is the one place both images link, and the sensor
//! drivers (drivers/adc.rs, drivers/gpio.rs) claim pins through it. The ownership table and the
//! mask arithmetic are in `ipmc_sensors::pin_claims` (hardware-free, host-tested); this file
//! applies them to the registers.
//!
//! Register discipline: every configuration write is a read-modify-write of only the claimed
//! pins' bits, inside one critical section, so handles used from different contexts cannot
//! undo each other. Output writes use BSRR, which has no read-modify-write and so no race on
//! ODR. Claims happen once at start-up, before the interrupts that use the handles are
//! unmasked.

use core::cell::RefCell;
use cortex_m::interrupt::{self, Mutex};
use stm32f4::stm32f401::{GPIOA, GPIOB, GPIOC, RCC};

use ipmc_sensors::pin_claims::{
    af_mask, af_value, bsrr_word, field2_mask, field2_value, merge, ClaimTable,
};
pub use ipmc_sensors::pin_claims::{PinId, PortError, PortId};

/// MODER encodings (RM0368 8.4.1).
const MODER_INPUT: u32 = 0b00;
const MODER_OUTPUT: u32 = 0b01;
const MODER_ALTERNATE: u32 = 0b10;
const MODER_ANALOG: u32 = 0b11;

/// Pull -- the PUPDR setting (RM0368 8.4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    None,
    Up,
    Down,
}

impl Pull {
    const fn bits(self) -> u32 {
        match self {
            Pull::None => 0b00,
            Pull::Up => 0b01,
            Pull::Down => 0b10,
        }
    }
}

/// Speed -- the OSPEEDR setting (RM0368 8.4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Low,
    Medium,
    Fast,
    High,
}

impl Speed {
    const fn bits(self) -> u32 {
        match self {
            Speed::Low => 0b00,
            Speed::Medium => 0b01,
            Speed::Fast => 0b10,
            Speed::High => 0b11,
        }
    }
}

struct State {
    gpioa: GPIOA,
    gpiob: GPIOB,
    gpioc: GPIOC,
    table: ClaimTable,
}

// Shared state. Written by `init` and `claim` (thread, start-up, inside a critical section);
// the Pins methods only read it (a shared borrow inside a critical section) and write the port
// registers. Every access runs inside a critical section, so thread and interrupts never
// overlap.
static PORTS: Mutex<RefCell<Option<State>>> = Mutex::new(RefCell::new(None));

/// on_port -- runs `$body` with `$g` bound to the register block of `$port`. A macro because
/// the three register-block types have no common trait.
macro_rules! on_port {
    ($state:expr, $port:expr, $g:ident => $body:expr) => {
        match $port {
            PortId::A => {
                let $g = &$state.gpioa;
                $body
            }
            PortId::B => {
                let $g = &$state.gpiob;
                $body
            }
            PortId::C => {
                let $g = &$state.gpioc;
                $body
            }
        }
    };
}

/// rmw -- replaces the bits of `$field` in register `$reg` with those of `$value`; every other
/// bit keeps its value.
macro_rules! rmw {
    ($reg:expr, $field:expr, $value:expr) => {
        // SAFETY: only the bits of the claimed pins change (merge keeps the rest).
        $reg.modify(|r, w| unsafe { w.bits(merge(r.bits(), $field, $value)) })
    };
}

/// init -- enables the GPIOA..GPIOC clocks and takes the three ports.
pub fn init(rcc: &RCC, gpioa: GPIOA, gpiob: GPIOB, gpioc: GPIOC) -> Result<(), PortError> {
    rcc.ahb1enr().modify(|_, w| {
        w.gpioaen()
            .enabled()
            .gpioben()
            .enabled()
            .gpiocen()
            .enabled()
    });
    let _ = rcc.ahb1enr().read(); // short delay after enabling a clock
    interrupt::free(|cs| {
        let mut slot = PORTS.borrow(cs).borrow_mut();
        if slot.is_some() {
            return Err(PortError::AlreadyInitialised);
        }
        *slot = Some(State {
            gpioa,
            gpiob,
            gpioc,
            table: ClaimTable::new(),
        });
        Ok(())
    })
}

/// claim -- takes the pins of `mask` (bit n = pin n) of `port` for `owner`.
///
/// Fails with `AlreadyClaimed` (naming the holder and `owner`) when any pin has an owner;
/// nothing is claimed then. Call once per module at start-up.
pub fn claim(owner: &'static str, port: PortId, mask: u16) -> Result<Pins, PortError> {
    interrupt::free(|cs| {
        let mut slot = PORTS.borrow(cs).borrow_mut();
        let state = slot.as_mut().ok_or(PortError::NotInitialised)?;
        state.table.claim(owner, port, mask)?;
        Ok(Pins { port, mask })
    })
}

/// claim_pin -- `claim` for one pin named by `PinId`; `Err(InvalidPin)` above pin 15.
pub fn claim_pin(owner: &'static str, pin: PinId) -> Result<Pins, PortError> {
    claim(owner, pin.port, pin.mask()?)
}

/// Pins -- a claimed set of pins of one port. Not Clone: the holder is the owner.
#[derive(Debug)]
pub struct Pins {
    port: PortId,
    mask: u16,
}

impl Pins {
    /// port -- the port of the pins.
    pub fn port(&self) -> PortId {
        self.port
    }

    /// mask -- the claimed pins, bit n = pin n.
    pub fn mask(&self) -> u16 {
        self.mask
    }

    /// with_state -- runs `f` on the ports inside a critical section; None before `init`.
    fn with_state<R>(&self, f: impl FnOnce(&State) -> R) -> Option<R> {
        interrupt::free(|cs| PORTS.borrow(cs).borrow().as_ref().map(f))
    }

    /// configure -- the shared body of the four modes. Order: output level first, then output
    /// type, speed, pull and alternate function, mode last, so an output never glitches.
    fn configure(
        &self,
        mode: u32,
        pull: Pull,
        speed: Speed,
        open_drain: bool,
        af: Option<u32>,
        initial_high: Option<bool>,
    ) {
        let mask = self.mask;
        let _ = self.with_state(|st| {
            on_port!(st, self.port, g => {
                if let Some(high) = initial_high {
                    let levels = if high { mask } else { 0 };
                    // SAFETY: BSRR is write-only; the word touches only the claimed pins.
                    g.bsrr().write(|w| unsafe { w.bits(bsrr_word(mask, levels)) });
                }
                rmw!(g.otyper(), u32::from(mask), if open_drain { u32::from(mask) } else { 0 });
                rmw!(g.ospeedr(), field2_mask(mask), field2_value(mask, speed.bits()));
                rmw!(g.pupdr(), field2_mask(mask), field2_value(mask, pull.bits()));
                if let Some(af) = af {
                    rmw!(g.afrl(), af_mask(mask, false), af_value(mask, false, af));
                    rmw!(g.afrh(), af_mask(mask, true), af_value(mask, true, af));
                }
                rmw!(g.moder(), field2_mask(mask), field2_value(mask, mode));
            })
        });
    }

    /// input -- digital input with the given pull.
    pub fn input(&self, pull: Pull) {
        self.configure(MODER_INPUT, pull, Speed::Low, false, None, None);
    }

    /// output -- general-purpose output, driven to `initial_high` before the pin becomes an
    /// output.
    pub fn output(&self, initial_high: bool, open_drain: bool, speed: Speed) {
        self.configure(
            MODER_OUTPUT,
            Pull::None,
            speed,
            open_drain,
            None,
            Some(initial_high),
        );
    }

    /// open_drain -- open-drain output with the given pull, released (high) before the pin
    /// becomes an output.
    pub fn open_drain(&self, pull: Pull, speed: Speed) {
        self.configure(MODER_OUTPUT, pull, speed, true, None, Some(true));
    }

    /// alternate -- alternate function number `af` (0..=15).
    pub fn alternate(&self, af: u8, pull: Pull, open_drain: bool, speed: Speed) {
        self.configure(
            MODER_ALTERNATE,
            pull,
            speed,
            open_drain,
            Some(u32::from(af)),
            None,
        );
    }

    /// analog -- analog mode, no pull.
    pub fn analog(&self) {
        self.configure(MODER_ANALOG, Pull::None, Speed::Low, false, None, None);
    }

    /// claimed_levels -- the level register of the claimed pins, bit n = pin n (0 for unclaimed
    /// bits): the output register (ODR) when `output`, else the input register (IDR). Returns 0
    /// before `init`.
    fn claimed_levels(&self, output: bool) -> u16 {
        self.with_state(|st| {
            on_port!(st, self.port, g => if output {
                g.odr().read().bits() as u16
            } else {
                g.idr().read().bits() as u16
            })
        })
        .map_or(0, |v| v & self.mask)
    }

    /// read -- the input levels of the claimed pins, bit n = pin n (0 for unclaimed bits).
    /// Returns 0 before `init`.
    pub fn read(&self) -> u16 {
        self.claimed_levels(false)
    }

    /// output_levels -- the output register levels of the claimed pins, bit n = pin n.
    pub fn output_levels(&self) -> u16 {
        self.claimed_levels(true)
    }

    /// write -- drives the claimed pins to `levels` (bit n = 1 high) with one BSRR write; bits
    /// outside the claim are ignored.
    pub fn write(&self, levels: u16) {
        let word = bsrr_word(self.mask, levels);
        let _ = self.with_state(|st| {
            // SAFETY: BSRR is write-only; the word touches only the claimed pins.
            on_port!(st, self.port, g => g.bsrr().write(|w| unsafe { w.bits(word) }))
        });
    }

    /// set_all -- drives every claimed pin high or low.
    pub fn set_all(&self, high: bool) {
        self.write(if high { self.mask } else { 0 });
    }
}
