//! decision - the start decision (design 6.1) and the boot countdown window (design 6.5).

use crate::header::ImageError;
use crate::request::Interface;

/// Reason -- why the bootloader stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Pin,
    Requested,
    NoImage,
    BadMagic,
    BadHeader,
    BadVectorTable,
    DigestMismatch,
    WakeupInCountdown,
}

impl Reason {
    pub const fn text(self) -> &'static str {
        match self {
            Reason::Pin => "BOOTLOADER pin",
            Reason::Requested => "requested",
            Reason::NoImage => "no image",
            Reason::BadMagic => "bad magic",
            Reason::BadHeader => "bad header",
            Reason::BadVectorTable => "bad vector table",
            Reason::DigestMismatch => "digest mismatch",
            Reason::WakeupInCountdown => "WAKEUP in countdown",
        }
    }
}

/// Decision -- what the start-up does after the checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Start,
    Countdown(u32),
    Stay(Reason),
}

/// decide -- the decision table of design 6.1, the pin row before the request row. A stay never
/// counts down.
pub fn decide(
    pin_high: bool,
    request: Option<Interface>,
    image: Result<(), ImageError>,
    countdown_s: u32,
) -> Decision {
    if pin_high {
        return Decision::Stay(Reason::Pin);
    }
    if request.is_some() {
        return Decision::Stay(Reason::Requested);
    }
    match image {
        Err(ImageError::Missing) => Decision::Stay(Reason::NoImage),
        Err(ImageError::BadMagic) => Decision::Stay(Reason::BadMagic),
        Err(ImageError::BadHeader(_)) => Decision::Stay(Reason::BadHeader),
        Err(ImageError::BadVectorTable) => Decision::Stay(Reason::BadVectorTable),
        Err(ImageError::DigestMismatch) => Decision::Stay(Reason::DigestMismatch),
        Ok(()) if countdown_s == 0 => Decision::Start,
        Ok(()) => Decision::Countdown(countdown_s),
    }
}

/// PortId -- which UART a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortId {
    Cli,
    App,
}

/// MS_PER_DIGIT -- one digit of the countdown per second.
const MS_PER_DIGIT: u32 = 1000;

/// Countdown -- the window of design 6.5 as a pure state machine. Time is milliseconds since the
/// first digit was queued (the caller passes SysTick differences).
#[derive(Clone, Copy, Debug)]
pub struct Countdown {
    seconds: u32,
    printed: u32,
}

/// Tick -- what the caller does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Print this digit (a space before it after the first).
    Digit(u32),
    /// Nothing yet.
    Wait,
    /// A WAKEUP completed inside the window on this port: open the session there.
    Session(PortId),
    /// The window is over: start the runtime.
    Start,
}

impl Countdown {
    pub const fn new(seconds: u32) -> Self {
        Countdown {
            seconds,
            printed: 0,
        }
    }

    /// step -- `wakeup` is the port whose WAKEUP line completed since the last call (the caller
    /// passes the first in arrival order). Returns Digit(d) once for each digit when its second
    /// begins (N at 0 ms, N-1 at 1000 ms, ... 1 at (N-1)*1000 ms), Session(port) for a WAKEUP
    /// completed before N*1000 ms, Start at N*1000 ms or later, Wait otherwise. Call again after
    /// a Digit until it returns something else.
    pub fn step(&mut self, elapsed_ms: u32, wakeup: Option<PortId>) -> Tick {
        if elapsed_ms >= self.seconds.saturating_mul(MS_PER_DIGIT) {
            return Tick::Start;
        }
        if let Some(port) = wakeup {
            return Tick::Session(port);
        }
        let due = elapsed_ms / MS_PER_DIGIT + 1;
        if self.printed < due {
            let digit = self.seconds - self.printed;
            self.printed += 1;
            return Tick::Digit(digit);
        }
        Tick::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::HeaderField;

    /// Row -- pin, request, image check, countdown seconds and the expected decision.
    type Row = (
        bool,
        Option<Interface>,
        Result<(), ImageError>,
        u32,
        Decision,
    );

    #[test]
    fn every_row_of_the_decision_table() {
        let bad_header = Err(ImageError::BadHeader(HeaderField::Size));
        // (pin, request, image, countdown, expected)
        let rows: [Row; 14] = [
            (false, None, Ok(()), 0, Decision::Start),
            (false, None, Ok(()), 3, Decision::Countdown(3)),
            (true, None, Ok(()), 3, Decision::Stay(Reason::Pin)),
            (
                true,
                Some(Interface::App),
                Ok(()),
                0,
                Decision::Stay(Reason::Pin),
            ),
            (
                true,
                None,
                Err(ImageError::Missing),
                0,
                Decision::Stay(Reason::Pin),
            ),
            (
                false,
                Some(Interface::Console),
                Ok(()),
                3,
                Decision::Stay(Reason::Requested),
            ),
            (
                false,
                Some(Interface::App),
                Err(ImageError::Missing),
                0,
                Decision::Stay(Reason::Requested),
            ),
            (
                false,
                None,
                Err(ImageError::Missing),
                3,
                Decision::Stay(Reason::NoImage),
            ),
            (
                false,
                None,
                Err(ImageError::BadMagic),
                3,
                Decision::Stay(Reason::BadMagic),
            ),
            (
                false,
                None,
                bad_header,
                3,
                Decision::Stay(Reason::BadHeader),
            ),
            (
                false,
                None,
                Err(ImageError::BadVectorTable),
                0,
                Decision::Stay(Reason::BadVectorTable),
            ),
            (
                false,
                None,
                Err(ImageError::DigestMismatch),
                3,
                Decision::Stay(Reason::DigestMismatch),
            ),
            (false, None, Ok(()), 30, Decision::Countdown(30)),
            (false, None, Ok(()), 1, Decision::Countdown(1)),
        ];
        for (pin, request, image, seconds, want) in rows {
            assert_eq!(
                decide(pin, request, image, seconds),
                want,
                "{pin} {request:?} {image:?}"
            );
        }
    }

    #[test]
    fn reason_texts() {
        let rows = [
            (Reason::Pin, "BOOTLOADER pin"),
            (Reason::Requested, "requested"),
            (Reason::NoImage, "no image"),
            (Reason::BadMagic, "bad magic"),
            (Reason::BadHeader, "bad header"),
            (Reason::BadVectorTable, "bad vector table"),
            (Reason::DigestMismatch, "digest mismatch"),
            (Reason::WakeupInCountdown, "WAKEUP in countdown"),
        ];
        for (reason, text) in rows {
            assert_eq!(reason.text(), text);
        }
    }

    /// drain -- calls `step` at `ms` until it stops printing digits; returns the digits and the
    /// final tick.
    fn drain(c: &mut Countdown, ms: u32, wakeup: Option<PortId>) -> (std::vec::Vec<u32>, Tick) {
        let mut digits = std::vec::Vec::new();
        loop {
            match c.step(ms, wakeup) {
                Tick::Digit(d) => digits.push(d),
                other => return (digits, other),
            }
        }
    }

    #[test]
    fn three_seconds_print_three_two_one_then_start() {
        let mut c = Countdown::new(3);
        // (elapsed ms, digits printed, tick after them)
        let rows: [(u32, &[u32], Tick); 7] = [
            (0, &[3], Tick::Wait),
            (999, &[], Tick::Wait),
            (1000, &[2], Tick::Wait),
            (1999, &[], Tick::Wait),
            (2000, &[1], Tick::Wait),
            (2999, &[], Tick::Wait),
            (3000, &[], Tick::Start),
        ];
        for (ms, digits, tick) in rows {
            let (got, last) = drain(&mut c, ms, None);
            assert_eq!((got.as_slice(), last), (digits, tick), "{ms} ms");
        }
    }

    #[test]
    fn wakeup_inside_the_window_opens_a_session_and_at_the_end_starts() {
        let (_, tick) = drain(&mut Countdown::new(3), 2999, Some(PortId::App));
        assert_eq!(tick, Tick::Session(PortId::App));
        let (_, tick) = drain(&mut Countdown::new(3), 0, Some(PortId::Cli));
        assert_eq!(tick, Tick::Session(PortId::Cli));
        let (_, tick) = drain(&mut Countdown::new(3), 3000, Some(PortId::Cli));
        assert_eq!(tick, Tick::Start);
    }

    #[test]
    fn a_late_poll_prints_the_missed_digits_in_order() {
        let (digits, tick) = drain(&mut Countdown::new(3), 2500, None);
        assert_eq!((digits.as_slice(), tick), (&[3, 2, 1][..], Tick::Wait));
    }

    #[test]
    fn zero_seconds_start_at_once() {
        assert_eq!(Countdown::new(0).step(0, None), Tick::Start);
    }
}
