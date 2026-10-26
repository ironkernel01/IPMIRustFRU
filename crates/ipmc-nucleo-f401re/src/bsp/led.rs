// bsp/led.rs - user LED LD2 on PA5 (UM1724 6.4: pin HIGH = LED on) and its status patterns.
//
// Ownership: this module owns PA5, claimed through `ports` (owner "led"); the handle lives in
// the `LED` mutex and the SysTick tick drives it through that handle.
// The level is a pure function of (mode, time, time of the last activity):
// ipmc_sensors::eval::led::led_level, host-tested. This file only stores the mode, runs the
// function once per millisecond and writes the pin when the level changes.
// Modes (PLAN.md 3.6b): Auto = heartbeat, or the activity rate for IPMC_LED_ACTIVITY_HOLD_MS
// after each `activity()` call; Manual On, Off, Blink; the start-up Fault blink.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::interrupt;
use ipmc_sensors::drivers::stm32f401::ports::{self, Pins, PortError, PortId, Speed};
use ipmc_sensors::eval::led::{self as pattern, Mode, NO_ACTIVITY_MS};

pub use ipmc_sensors::eval::led::{rate_chz, Shown};

use super::static_slot::{self, StaticSlot};
use super::systick;

const OWNER: &str = "led";
const LED_PIN_MASK: u16 = 1 << 5;
/// RATE_MIN_CHZ -- lowest accepted flash rate in centihertz (defs.rs IPMC_LED_RATE_MIN_CHZ).
pub const RATE_MIN_CHZ: u32 = ipmc_defs::IPMC_LED_RATE_MIN_CHZ;
/// RATE_MAX_CHZ -- highest accepted flash rate in centihertz (defs.rs IPMC_LED_RATE_MAX_CHZ).
pub const RATE_MAX_CHZ: u32 = ipmc_defs::IPMC_LED_RATE_MAX_CHZ;

struct Led {
    pins: Pins,
    mode: Mode,
    lit: bool,
}

// Shared state. Written by `init` (thread, once) and by the `set_*` functions (thread, main
// loop); also written by `tick_1ms` (SysTick exception: the PA5 output and `lit`). Every
// access runs inside a critical section, so the tick and the thread never overlap.
static LED: StaticSlot<Led> = static_slot::empty();

// Millisecond stamp of the last `activity()` call (pattern::stamp), NO_ACTIVITY_MS before the
// first. Written by `activity()` from any context, read by the SysTick tick and `shown`: a
// single word, so an atomic with Relaxed ordering is enough on this single-core part.
static LAST_ACTIVITY: AtomicU32 = AtomicU32::new(NO_ACTIVITY_MS);

/// init -- claims PA5, makes it an output driven low, and starts the heartbeat (mode Auto).
pub fn init() -> Result<(), PortError> {
    let pins = ports::claim(OWNER, PortId::A, LED_PIN_MASK)?;
    pins.output(false, false, Speed::Low);
    static_slot::install(
        &LED,
        Led {
            pins,
            mode: Mode::Auto,
            lit: false,
        },
    );
    Ok(())
}

/// activity -- tells the LED that an IPMB frame was received or sent: the LED blinks at the
/// activity rate until IPMC_LED_ACTIVITY_HOLD_MS passes without another call. One atomic store
/// of the current millisecond time, so it is callable from interrupt context.
///
/// Nothing calls it yet: the IPMB driver (Phase 6) is its caller, on every received or sent
/// frame. Only the host tests of `eval::led` exercise the behaviour today.
#[allow(dead_code)] // the IPMB driver (Phase 6) is the first caller
pub fn activity() {
    LAST_ACTIVITY.store(pattern::stamp(systick::millis()), Ordering::Relaxed);
}

/// update -- computes the level for `now_ms` and writes the pin when it changed.
fn update(led: &mut Led, now_ms: u32) {
    let want = pattern::led_level(led.mode, now_ms, LAST_ACTIVITY.load(Ordering::Relaxed));
    if want != led.lit {
        led.pins.set_all(want);
        led.lit = want;
    }
}

/// set_mode -- switches to `mode` and shows it at once. Does nothing before `init`.
fn set_mode(mode: Mode) {
    static_slot::with_mut(&LED, |led| {
        led.mode = mode;
        update(led, systick::millis());
    });
}

/// set_on -- manual: steady on. Does nothing before `init`.
pub fn set_on() {
    set_mode(Mode::On);
}

/// set_off -- manual: steady off. Does nothing before `init`.
pub fn set_off() {
    set_mode(Mode::Off);
}

/// set_blink_chz -- manual blink at `rate_chz` centihertz. The caller has checked RATE_MIN_CHZ..=RATE_MAX_CHZ.
pub fn set_blink_chz(rate_chz: u32) {
    set_mode(Mode::Blink(rate_chz));
}

/// set_auto -- back to the heartbeat / activity pattern.
pub fn set_auto() {
    set_mode(Mode::Auto);
}

/// set_fault -- start-up fault blink at `rate_chz` centihertz.
pub fn set_fault(rate_chz: u32) {
    set_mode(Mode::Fault(rate_chz));
}

/// shown -- what the LED is doing now (Off before `init`).
pub fn shown() -> Shown {
    let mode = interrupt::free(|cs| LED.borrow(cs).borrow().as_ref().map(|led| led.mode));
    mode.map_or(Shown::Off, |m| {
        pattern::shown(m, systick::millis(), LAST_ACTIVITY.load(Ordering::Relaxed))
    })
}

/// tick_1ms -- called from the SysTick handler once per millisecond with the new time. Does
/// nothing before `init`.
pub fn tick_1ms(now_ms: u32) {
    static_slot::with_mut(&LED, |led| update(led, now_ms));
}
