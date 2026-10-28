// frame.rs - the received IPMB frame, the consumer end of a bus queue and `FrameAssembler`, the
// interrupt-side frame state of one I2C target.
//
// `FrameAssembler` holds no counters, no queue and no clock: the board feeds it the events its
// interrupt decodes and books the returned events in its own counters (a board without atomic
// read-modify-write keeps them under its critical section).

use heapless::spsc::Consumer;

/// GENERAL_CALL -- the address byte a broadcast frame starts with (IPMI v2.0 section 20.9). Peer:
/// ipmc-core link.rs GENERAL_CALL strips it; a different value here makes Broadcast Get Device
/// ID answer nothing.
const GENERAL_CALL: u8 = 0x00;

/// RX_FRAME_MAX -- bytes of one received frame as the interrupt stores it: the IPMB frame
/// maximum (IPMB_MSG_MAX, the whole frame from rsSA to the second checksum) plus one for the
/// leading address byte, which the interrupt prepends because the hardware consumes the address
/// phase (own address byte, or 00h for a general call). A broadcast frame is then 00h followed
/// by a normal frame, the same length as a frame that carries its own address (IPMB_MSG_MAX)
/// plus the general-call byte in front. A longer frame is an overrun and is dropped.
pub const RX_FRAME_MAX: usize = ipmc_defs::IPMB_MSG_MAX + 1;
const _: () = assert!(RX_FRAME_MAX <= 255, "RxFrame::len is a u8");

/// RX_QUEUE_LEN -- heapless::spsc::Queue<_, N> holds N - 1 frames (one slot separates a full
/// queue from an empty one), so N = IPMB_RX_QUEUE_DEPTH + 1 holds DEPTH frames.
pub const RX_QUEUE_LEN: usize = ipmc_defs::IPMB_RX_QUEUE_DEPTH + 1;

/// RxFrame -- one received frame as posted by the interrupt: `bytes[..len]`, STOP time in
/// SysTick milliseconds.
#[derive(Clone, Copy)]
pub struct RxFrame {
    pub bytes: [u8; RX_FRAME_MAX],
    pub len: u8,
    pub stop_ms: u32,
}

impl RxFrame {
    pub const EMPTY: RxFrame = RxFrame {
        bytes: [0; RX_FRAME_MAX],
        len: 0,
        stop_ms: 0,
    };

    /// data -- the received bytes.
    pub fn data(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// FrameSource -- the consumer end of one bus's frame queue.
pub trait FrameSource {
    /// peek -- the oldest frame, left in the queue.
    fn peek(&self) -> Option<&RxFrame>;
    /// dequeue -- removes and returns the oldest frame.
    fn dequeue(&mut self) -> Option<RxFrame>;
}

impl FrameSource for Consumer<'static, RxFrame, RX_QUEUE_LEN> {
    fn peek(&self) -> Option<&RxFrame> {
        Consumer::peek(self)
    }

    fn dequeue(&mut self) -> Option<RxFrame> {
        Consumer::dequeue(self)
    }
}

/// Phase -- where the target is in a transfer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Between frames.
    Idle,
    /// Address matched with the write bit: bytes go into the frame.
    Receiving,
    /// Address matched with the read bit: the target answers FFh until the master NACKs.
    Transmitting,
}

/// AddressEvent -- what `on_address` did, for the board's counters.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AddressEvent {
    /// A repeated START ended a partial frame that had no STOP; the partial frame is dropped.
    pub restart_drop: bool,
    /// The address matched with the read bit: the target now answers FFh.
    pub slave_read: bool,
}

/// StopEvent -- what `on_stop` found, for the board to post and count.
#[derive(Clone, Copy)]
pub enum StopEvent {
    /// No write was in progress.
    Ignored,
    /// The frame exceeded RX_FRAME_MAX and is dropped.
    Overrun,
    /// An address-only write (no data byte) is dropped.
    Empty,
    /// A complete frame, with `stop_ms` set, for the board to enqueue.
    Frame(RxFrame),
}

/// SlaveErrors -- the target-side error flags of one error interrupt.
#[derive(Clone, Copy, Default)]
pub struct SlaveErrors {
    /// BERR: misplaced START or STOP.
    pub bus_error: bool,
    /// ARLO: the hardware master lost arbitration.
    pub arbitration_lost: bool,
    /// AF: a NACK.
    pub ack_failure: bool,
    pub overrun: bool,
    pub timeout: bool,
    /// Any other error flag (PEC error, SMBus alert).
    pub other: bool,
}

/// ErrorEvent -- what `on_errors` decided.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ErrorEvent {
    /// The ack failure ended a slave read (AF while transmitting is its normal end).
    pub slave_read_end: bool,
}

/// FrameAssembler -- the frame being received by one I2C target.
#[derive(Clone, Copy)]
pub struct FrameAssembler {
    own_addr8: u8,
    frame: RxFrame,
    overrun: bool,
    phase: Phase,
}

impl FrameAssembler {
    /// new -- idle, with the 8-bit own address that starts a write frame.
    pub const fn new(own_addr8: u8) -> Self {
        FrameAssembler {
            own_addr8,
            frame: RxFrame::EMPTY,
            overrun: false,
            phase: Phase::Idle,
        }
    }

    /// is_idle -- true between frames.
    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    /// reset -- back to idle with the frame cleared.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.clear_frame();
    }

    fn clear_frame(&mut self) {
        self.frame.len = 0;
        self.overrun = false;
    }

    /// push -- appends one byte; beyond RX_FRAME_MAX the byte is dropped and the frame marked
    /// overrun (the reads of DR go on so the bus is not held).
    fn push(&mut self, byte: u8) {
        match self.frame.bytes.get_mut(usize::from(self.frame.len)) {
            Some(slot) => {
                *slot = byte;
                self.frame.len += 1;
            }
            None => self.overrun = true,
        }
    }

    /// on_address -- an address matched. A write (`read` false) starts a frame whose first byte is
    /// the address that matched: 00h for a general call, else the own 8-bit address. A read starts
    /// a transmission of FFh bytes.
    pub fn on_address(&mut self, read: bool, general_call: bool) -> AddressEvent {
        let restart_drop = self.phase == Phase::Receiving && self.frame.len > 0;
        self.clear_frame();
        if read {
            self.phase = Phase::Transmitting;
        } else {
            self.phase = Phase::Receiving;
            let first = if general_call {
                GENERAL_CALL
            } else {
                self.own_addr8
            };
            self.push(first);
        }
        AddressEvent {
            restart_drop,
            slave_read: read,
        }
    }

    /// on_byte -- a received byte.
    pub fn on_byte(&mut self, byte: u8) {
        if self.phase == Phase::Receiving {
            self.push(byte);
        }
    }

    /// on_stop -- STOP: returns the frame of a write unless it overran or is address-only, then
    /// returns to idle. `now_ms` is called only for a frame that is returned.
    pub fn on_stop(&mut self, now_ms: impl FnOnce() -> u32) -> StopEvent {
        let event = if self.phase != Phase::Receiving {
            StopEvent::Ignored
        } else if self.overrun {
            StopEvent::Overrun
        } else if self.frame.len <= 1 {
            StopEvent::Empty
        } else {
            self.frame.stop_ms = now_ms();
            StopEvent::Frame(self.frame)
        };
        self.reset();
        event
    }

    /// on_errors -- the error flags of one interrupt. A flag other than ARLO drops the partial
    /// frame and returns the target to idle. ARLO alone leaves it: it reports the hardware master
    /// losing arbitration (RM0368 18.6.6, ARLO), after which the peripheral may already be
    /// receiving the winner's frame addressed to this target, and that frame is kept. AF while
    /// transmitting is the normal end of a read (RM0368 18.3.3, slave transmitter).
    pub fn on_errors(&mut self, e: SlaveErrors) -> ErrorEvent {
        let slave_read_end = e.ack_failure && self.phase == Phase::Transmitting;
        if e.bus_error || e.ack_failure || e.overrun || e.timeout || e.other {
            self.reset();
        }
        ErrorEvent { slave_read_end }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    const OWN: u8 = 0x82;

    fn stop_frame(a: &mut FrameAssembler, ms: u32) -> (StopEvent, u32) {
        let calls = Cell::new(0);
        let event = a.on_stop(|| {
            calls.set(calls.get() + 1);
            ms
        });
        (event, calls.get())
    }

    fn only(flag: fn(&mut SlaveErrors)) -> SlaveErrors {
        let mut e = SlaveErrors::default();
        flag(&mut e);
        e
    }

    #[test]
    fn write_frame_posts_address_and_bytes_with_one_clock_read() {
        let mut a = FrameAssembler::new(OWN);
        let e = a.on_address(false, false);
        assert!(!e.restart_drop && !e.slave_read);
        a.on_byte(0x20);
        a.on_byte(0x18);
        let (event, calls) = stop_frame(&mut a, 1234);
        assert_eq!(calls, 1);
        let StopEvent::Frame(f) = event else {
            panic!("no frame");
        };
        assert_eq!(f.data(), &[OWN, 0x20, 0x18]);
        assert_eq!(f.stop_ms, 1234);
        assert!(a.is_idle());
    }

    #[test]
    fn general_call_gives_00_first_byte() {
        let mut a = FrameAssembler::new(OWN);
        a.on_address(false, true);
        a.on_byte(0x55);
        let (StopEvent::Frame(f), _) = stop_frame(&mut a, 1) else {
            panic!("no frame");
        };
        assert_eq!(f.data(), &[0x00, 0x55]);
    }

    #[test]
    fn address_only_frame_is_empty_and_never_reads_the_clock() {
        let mut a = FrameAssembler::new(OWN);
        a.on_address(false, false);
        let (event, calls) = stop_frame(&mut a, 9);
        assert!(matches!(event, StopEvent::Empty));
        assert_eq!(calls, 0);
        assert!(a.is_idle());
    }

    #[test]
    fn stop_without_a_write_is_ignored_and_idles() {
        let mut a = FrameAssembler::new(OWN);
        let (event, calls) = stop_frame(&mut a, 9);
        assert!(matches!(event, StopEvent::Ignored));
        assert_eq!(calls, 0);
        a.on_address(true, false);
        assert!(!a.is_idle());
        let (event, _) = stop_frame(&mut a, 9);
        assert!(matches!(event, StopEvent::Ignored));
        assert!(a.is_idle());
    }

    #[test]
    fn overrun_at_frame_max_plus_one() {
        let mut a = FrameAssembler::new(OWN);
        a.on_address(false, false);
        // The address byte is one; RX_FRAME_MAX - 1 data bytes fill the frame exactly.
        for i in 0..RX_FRAME_MAX - 1 {
            a.on_byte(i as u8);
        }
        let (event, _) = stop_frame(&mut a, 5);
        assert!(matches!(event, StopEvent::Frame(f) if usize::from(f.len) == RX_FRAME_MAX));

        a.on_address(false, false);
        for i in 0..RX_FRAME_MAX {
            a.on_byte(i as u8);
        }
        let (event, calls) = stop_frame(&mut a, 5);
        assert!(matches!(event, StopEvent::Overrun));
        assert_eq!(calls, 0);
        // The overrun flag does not outlive the frame.
        a.on_address(false, false);
        a.on_byte(1);
        assert!(matches!(stop_frame(&mut a, 5).0, StopEvent::Frame(_)));
    }

    #[test]
    fn repeated_start_with_data_is_a_restart_drop() {
        let mut a = FrameAssembler::new(OWN);
        assert!(!a.on_address(false, false).restart_drop);
        a.on_byte(0x20);
        let e = a.on_address(false, false);
        assert!(e.restart_drop);
        a.on_byte(0x30);
        let (StopEvent::Frame(f), _) = stop_frame(&mut a, 1) else {
            panic!("no frame");
        };
        assert_eq!(f.data(), &[OWN, 0x30]);
    }

    #[test]
    fn slave_read_then_ack_failure_is_the_read_end() {
        let mut a = FrameAssembler::new(OWN);
        let e = a.on_address(true, false);
        assert!(e.slave_read && !e.restart_drop);
        a.on_byte(0x77); // ignored while transmitting
        let ev = a.on_errors(only(|e| e.ack_failure = true));
        assert!(ev.slave_read_end);
        assert!(a.is_idle());
        // AF outside a read is an ack failure, not a read end.
        a.on_address(false, false);
        let ev = a.on_errors(only(|e| e.ack_failure = true));
        assert!(!ev.slave_read_end);
    }

    #[test]
    fn arbitration_lost_alone_keeps_the_frame_and_other_flags_drop_it() {
        let mut a = FrameAssembler::new(OWN);
        a.on_address(false, false);
        a.on_byte(0x20);
        a.on_errors(only(|e| e.arbitration_lost = true));
        assert!(!a.is_idle());
        let flags: [fn(&mut SlaveErrors); 5] = [
            |e| e.bus_error = true,
            |e| e.ack_failure = true,
            |e| e.overrun = true,
            |e| e.timeout = true,
            |e| e.other = true,
        ];
        for flag in flags {
            a.on_address(false, false);
            a.on_errors(only(|e| e.arbitration_lost = true));
            assert!(!a.is_idle());
            a.on_errors(only(flag));
            assert!(a.is_idle());
            assert!(matches!(stop_frame(&mut a, 1).0, StopEvent::Ignored));
        }
    }

    #[test]
    fn reset_empties_the_frame() {
        let mut a = FrameAssembler::new(OWN);
        a.on_address(false, false);
        a.on_byte(0x20);
        a.reset();
        assert!(a.is_idle());
        assert!(matches!(stop_frame(&mut a, 1).0, StopEvent::Ignored));
        a.on_address(false, false);
        assert!(matches!(stop_frame(&mut a, 1).0, StopEvent::Empty));
    }
}
