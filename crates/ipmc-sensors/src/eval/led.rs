//! led.rs - the status LED level as a pure function of mode and time (PLAN.md 3.6b).
//!
//! No hardware: `ipmc-nucleo-f401re/src/bsp/led.rs` owns PA5 and calls `led_level` once per millisecond.
//! Time is the wrapping millisecond counter; blink phases come from the absolute time, so no
//! state is kept between calls.
//!
//! Rates are centihertz: one period lasts 100 000 / chz milliseconds. The phase of a rate is
//! `now_ms x chz mod 100 000`: each millisecond adds `chz` units of a 100 000-unit period.
//! It is exact for rates that do not divide 100 000 and needs no floating point.

/// One period in phase units (a millisecond adds `chz` units).
const PHASE_PERIOD: u32 = 100_000;
/// Half a period: the blink on-time (50 % duty).
const PHASE_HALF: u32 = PHASE_PERIOD / 2;
/// PHASE_SPLIT -- the rate is split as `high x PHASE_SPLIT + low` so every product of `phase`
/// fits 32 bits (no 64-bit division reaches the bootloader).
const PHASE_SPLIT: u32 = 1000;

/// NO_ACTIVITY_MS -- `last_activity_ms` value that means no activity yet.
pub const NO_ACTIVITY_MS: u32 = u32::MAX;

/// Mode -- what the LED is commanded to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Heartbeat, or the activity rate while IPMB frames moved within the hold window.
    Auto,
    /// Manual: steady on.
    On,
    /// Manual: steady off.
    Off,
    /// Manual: blink at this rate, centihertz, 50 % duty.
    Blink(u32),
    /// Start-up fault: blink at this rate, centihertz, forever.
    Fault(u32),
}

/// Shown -- what the LED is doing now (`Auto` resolved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Heartbeat,
    Activity,
    On,
    Off,
    Blink(u32),
    Fault(u32),
}

/// stamp -- the value to store as `last_activity_ms` for an activity at `now_ms`. The stored
/// word never equals `NO_ACTIVITY_MS`: an activity at that very millisecond is stored one
/// millisecond older.
pub const fn stamp(now_ms: u32) -> u32 {
    if now_ms == NO_ACTIVITY_MS {
        now_ms - 1
    } else {
        now_ms
    }
}

/// shown -- resolves `mode` at `now_ms`. Auto is Activity while the last activity is younger
/// than IPMC_LED_ACTIVITY_HOLD_MS and Heartbeat otherwise (also with no activity yet). The
/// activity stamp is never newer than `now_ms` (the caller takes both from the same tick), so
/// the age is a plain wrapping difference.
pub fn shown(mode: Mode, now_ms: u32, last_activity_ms: u32) -> Shown {
    match mode {
        Mode::Auto => {
            if last_activity_ms != NO_ACTIVITY_MS
                && now_ms.wrapping_sub(last_activity_ms) < ipmc_defs::IPMC_LED_ACTIVITY_HOLD_MS
            {
                Shown::Activity
            } else {
                Shown::Heartbeat
            }
        }
        Mode::On => Shown::On,
        Mode::Off => Shown::Off,
        Mode::Blink(chz) => Shown::Blink(chz),
        Mode::Fault(chz) => Shown::Fault(chz),
    }
}

/// rate_chz -- the flash rate of a shown state, centihertz; 0 for steady on and off.
pub const fn rate_chz(shown: Shown) -> u32 {
    match shown {
        Shown::Heartbeat => ipmc_defs::IPMC_LED_HEARTBEAT_CHZ,
        Shown::Activity => ipmc_defs::IPMC_LED_ACTIVITY_CHZ,
        Shown::Blink(chz) | Shown::Fault(chz) => chz,
        Shown::On | Shown::Off => 0,
    }
}

/// phase -- position within the period of `chz`, in 0..PHASE_PERIOD: `now_ms x chz mod
/// PHASE_PERIOD` for every input, in 32-bit arithmetic. With `a = now_ms mod P` and `c = chz mod
/// P` (both below P = 100 000) and `c = high x 1000 + low` (high below 100, low below 1000):
/// `a x c = (a x high) x 1000 + a x low`, so the result is `(((a x high) mod P) x 1000 + a x
/// low) mod P`. Largest intermediates: `a x high` < 10 000 000, `(..mod P) x 1000` < 100 000 000,
/// `a x low` < 100 000 000, their sum < 200 000 000, all under 2^32.
fn phase(now_ms: u32, chz: u32) -> u32 {
    let a = now_ms % PHASE_PERIOD;
    let c = chz % PHASE_PERIOD;
    let (high, low) = (c / PHASE_SPLIT, c % PHASE_SPLIT);
    ((a * high) % PHASE_PERIOD * PHASE_SPLIT + a * low) % PHASE_PERIOD
}

/// led_level -- true when the LED is lit.
///
/// Heartbeat: lit for IPMC_LED_HEARTBEAT_ON_MS at the start of each period. Activity and
/// every blink: lit for the first half of each period (50 % duty). A rate of 0 in `Blink` or
/// `Fault` is unlit.
pub fn led_level(mode: Mode, now_ms: u32, last_activity_ms: u32) -> bool {
    match shown(mode, now_ms, last_activity_ms) {
        Shown::On => true,
        Shown::Off => false,
        Shown::Heartbeat => {
            u64::from(phase(now_ms, ipmc_defs::IPMC_LED_HEARTBEAT_CHZ))
                < u64::from(ipmc_defs::IPMC_LED_HEARTBEAT_ON_MS)
                    * u64::from(ipmc_defs::IPMC_LED_HEARTBEAT_CHZ)
        }
        Shown::Activity => phase(now_ms, ipmc_defs::IPMC_LED_ACTIVITY_CHZ) < PHASE_HALF,
        Shown::Blink(chz) | Shown::Fault(chz) => chz != 0 && phase(now_ms, chz) < PHASE_HALF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipmc_defs::{IPMC_LED_ACTIVITY_HOLD_MS as HOLD, IPMC_LED_HEARTBEAT_ON_MS as ON_MS};

    const NONE: u32 = NO_ACTIVITY_MS;

    #[test]
    fn the_32_bit_phase_equals_the_64_bit_formula() {
        // The old formula: (now_ms x chz) mod 100 000 in u64. Every rate the LED accepts, every
        // millisecond of three periods and of the counter wrap, plus the largest u32 rates.
        let old = |now: u32, chz: u32| ((u64::from(now) * u64::from(chz)) % 100_000) as u32;
        let times = (0..100_100u32)
            .chain(u32::MAX - 1_000..=u32::MAX)
            .chain((0..2_000u32).map(|n| n.wrapping_mul(2_654_435_761)));
        let times: std::vec::Vec<u32> = times.collect();
        let rates = (0..=ipmc_defs::IPMC_LED_RATE_MAX_CHZ).step_by(13).chain([
            1,
            100,
            200,
            1000,
            5000,
            99_999,
            100_000,
            100_001,
            u32::MAX,
        ]);
        for chz in rates {
            for &now in &times {
                assert_eq!(phase(now, chz), old(now, chz), "{now} ms {chz} chz");
            }
        }
    }

    fn lit_ms(mode: Mode, from: u32, to: u32, last: u32) -> u32 {
        (from..to).filter(|&t| led_level(mode, t, last)).count() as u32
    }

    #[test]
    fn heartbeat_is_a_pulse_at_the_start_of_each_period() {
        // Defaults: 1 Hz period of 1000 ms, 100 ms on.
        assert_eq!(ipmc_defs::IPMC_LED_HEARTBEAT_CHZ, 100);
        assert!(led_level(Mode::Auto, 0, NONE));
        assert!(led_level(Mode::Auto, ON_MS - 1, NONE));
        assert!(!led_level(Mode::Auto, ON_MS, NONE));
        assert!(!led_level(Mode::Auto, 999, NONE));
        assert!(led_level(Mode::Auto, 1000, NONE));
        assert_eq!(lit_ms(Mode::Auto, 0, 10_000, NONE), 10 * ON_MS);
        assert_eq!(shown(Mode::Auto, 5, NONE), Shown::Heartbeat);
    }

    #[test]
    fn activity_blinks_at_ten_hertz_half_duty() {
        let last = 1_000;
        // Defaults: 10 Hz, period 100 ms, 50 ms on.
        assert_eq!(ipmc_defs::IPMC_LED_ACTIVITY_CHZ, 1000);
        assert_eq!(shown(Mode::Auto, 1_000, last), Shown::Activity);
        assert!(led_level(Mode::Auto, 1_000, last));
        assert!(led_level(Mode::Auto, 1_049, last));
        assert!(!led_level(Mode::Auto, 1_050, last));
        assert!(led_level(Mode::Auto, 1_100, last));
        assert_eq!(lit_ms(Mode::Auto, 1_000, 1_000 + HOLD, last), HOLD / 2);
    }

    #[test]
    fn hold_expires_back_to_heartbeat() {
        let last = 5_000;
        assert_eq!(shown(Mode::Auto, last + HOLD - 1, last), Shown::Activity);
        assert_eq!(shown(Mode::Auto, last + HOLD, last), Shown::Heartbeat);
        assert_eq!(
            shown(Mode::Auto, last + HOLD + 5_000, last),
            Shown::Heartbeat
        );
        // Retrigger: a newer stamp restarts the window.
        let again = last + HOLD - 1;
        assert_eq!(shown(Mode::Auto, again + HOLD - 1, again), Shown::Activity);
        assert_eq!(shown(Mode::Auto, again + HOLD, again), Shown::Heartbeat);
    }

    #[test]
    fn activity_hold_survives_counter_wrap() {
        let last = u32::MAX - 10;
        assert_eq!(shown(Mode::Auto, 5, last), Shown::Activity);
        assert_eq!(shown(Mode::Auto, HOLD, last), Shown::Heartbeat);
    }

    #[test]
    fn no_activity_means_heartbeat_even_early_after_boot() {
        assert_eq!(shown(Mode::Auto, 0, NONE), Shown::Heartbeat);
        assert_eq!(shown(Mode::Auto, 10, NONE), Shown::Heartbeat);
    }

    #[test]
    fn stamp_never_collides_with_the_sentinel() {
        assert_eq!(stamp(7), 7);
        assert_eq!(stamp(u32::MAX), u32::MAX - 1);
        assert_ne!(stamp(u32::MAX), NO_ACTIVITY_MS);
    }

    #[test]
    fn manual_modes_ignore_activity() {
        for t in [0, 50, 999] {
            assert!(led_level(Mode::On, t, t));
            assert!(!led_level(Mode::Off, t, t));
        }
        assert_eq!(shown(Mode::On, 10, 10), Shown::On);
        assert_eq!(shown(Mode::Off, 10, 10), Shown::Off);
        assert_eq!(shown(Mode::Blink(250), 10, 10), Shown::Blink(250));
    }

    #[test]
    fn manual_blink_is_half_duty_at_the_commanded_rate() {
        // 2 Hz: period 500 ms, 250 ms on.
        let m = Mode::Blink(200);
        assert!(led_level(m, 0, NONE));
        assert!(led_level(m, 249, NONE));
        assert!(!led_level(m, 250, NONE));
        assert!(led_level(m, 500, NONE));
        assert_eq!(lit_ms(m, 0, 5_000, NONE), 2_500);
        // 0.5 Hz: 2 s period.
        assert_eq!(lit_ms(Mode::Blink(50), 0, 20_000, NONE), 10_000);
        // A rate that does not divide 100 000 stays exact on average: 3 Hz over 10 s.
        assert_eq!(lit_ms(Mode::Blink(300), 0, 10_000, NONE), 5_000);
        // Limits.
        assert_eq!(
            lit_ms(
                Mode::Blink(ipmc_defs::IPMC_LED_RATE_MAX_CHZ),
                0,
                1_000,
                NONE
            ),
            500
        );
        assert!(!led_level(Mode::Blink(0), 0, NONE));
    }

    #[test]
    fn fault_blinks_forever_and_ignores_activity() {
        let m = Mode::Fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
        assert_eq!(shown(m, 100, 100), Shown::Fault(500));
        // 5 Hz: 200 ms period, 100 ms on.
        assert!(led_level(m, 0, 0));
        assert!(!led_level(m, 100, 0));
        assert_eq!(lit_ms(m, 0, 2_000, 0), 1_000);
    }

    #[test]
    fn rates_of_each_shown_state() {
        assert_eq!(rate_chz(Shown::Heartbeat), 100);
        assert_eq!(rate_chz(Shown::Activity), 1000);
        assert_eq!(rate_chz(Shown::Blink(250)), 250);
        assert_eq!(rate_chz(Shown::Fault(500)), 500);
        assert_eq!(rate_chz(Shown::On), 0);
        assert_eq!(rate_chz(Shown::Off), 0);
    }
}
