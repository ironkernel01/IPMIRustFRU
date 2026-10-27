//! queue.rs - fixed-capacity FIFOs: the generic `Fifo` and the egress queue of response frames.
//!
//! The dispatcher pushes responses; the link layer hands the oldest one to the bus driver and
//! keeps it queued until the driver reports the result. A full FIFO rejects the new item and
//! counts it; it never overwrites a queued item. The link layer's ingress FIFO and drop log
//! use the same `Fifo`.
//! Egress capacity: IPMB_TXQ_DEPTH frames of IPMB_MAX_LEN bytes (defs.rs; no C peer, the
//! firmware drains this queue). Too small a depth drops responses under bursts and the
//! Chassis Manager retries the request.

use crate::ipmb::IPMB_MAX_LEN;

/// TXQ_DEPTH -- queue capacity in frames, from defs.rs.
pub const TXQ_DEPTH: usize = ipmc_defs::IPMB_TXQ_DEPTH;

/// Bus -- the IPMB a frame arrived on or leaves on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    /// IPMB-A (mandatory, VITA 46.11 Rule 4.1.1-1).
    A,
    /// IPMB-B.
    B,
}

impl Bus {
    /// BOTH -- the two buses in index order.
    pub const BOTH: [Bus; 2] = [Bus::A, Bus::B];

    /// index -- the bus as an array index: A is 0, B is 1 (the order of `BOTH`).
    pub const fn index(self) -> usize {
        match self {
            Bus::A => 0,
            Bus::B => 1,
        }
    }

    /// name -- log name of the bus.
    pub const fn name(self) -> &'static str {
        match self {
            Bus::A => "A",
            Bus::B => "B",
        }
    }

    /// other -- the alternate bus (VITA 46.11 Rules 4.1.1-6 and 4.1.1-7).
    pub const fn other(self) -> Bus {
        match self {
            Bus::A => Bus::B,
            Bus::B => Bus::A,
        }
    }
}

/// QueueError -- why a push was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    /// All slots hold an item; the new item was dropped and counted.
    Full,
    /// The frame is longer than IPMB_MAX_LEN.
    FrameTooLong,
}

impl QueueError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            QueueError::Full => "Full",
            QueueError::FrameTooLong => "FrameTooLong",
        }
    }
}

/// Fifo -- ring of N items, oldest first. `new` takes the value that fills unused slots.
pub struct Fifo<T: Copy, const N: usize> {
    slots: [T; N],
    head: usize,
    len: usize,
    dropped: u32,
}

impl<T: Copy, const N: usize> Fifo<T, N> {
    /// new -- an empty FIFO whose slots start as `empty`.
    pub const fn new(empty: T) -> Self {
        Fifo {
            slots: [empty; N],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// push -- appends `item`. Err(Full) counts a drop and keeps every queued item.
    pub fn push(&mut self, item: T) -> Result<(), QueueError> {
        self.reserve()?;
        let tail = (self.head + self.len) % N;
        let slot = self.slots.get_mut(tail).ok_or(QueueError::Full)?;
        *slot = item;
        self.len += 1;
        Ok(())
    }

    /// reserve -- checks a slot is free without taking it. Err(Full) counts a drop, exactly as
    /// a full `push` does.
    pub fn reserve(&mut self) -> Result<(), QueueError> {
        if self.len == N {
            self.dropped = self.dropped.saturating_add(1);
            return Err(QueueError::Full);
        }
        Ok(())
    }

    /// pop -- removes and returns the oldest item; None when empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let item = *self.slots.get(self.head)?;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(item)
    }

    /// peek -- the oldest item, left queued.
    pub fn peek(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }
        self.slots.get(self.head)
    }

    /// peek_mut -- the oldest item for in-place change, left queued.
    pub fn peek_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            return None;
        }
        self.slots.get_mut(self.head)
    }

    /// len -- items waiting.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// is_empty -- true when no item waits.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// is_full -- true when every slot holds an item.
    pub const fn is_full(&self) -> bool {
        self.len == N
    }

    /// dropped -- items rejected because the FIFO was full, since start.
    pub const fn dropped(&self) -> u32 {
        self.dropped
    }
}

/// EgressFrame -- one queued frame with the bus it leaves on and the request it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EgressFrame {
    bytes: [u8; IPMB_MAX_LEN],
    len: usize,
    resends: u8,
    /// Bus the frame leaves on next. It starts as the arrival bus; the link layer chooses the
    /// transmit bus when it first hands the frame to a driver (`assigned`) and changes it for a
    /// resend.
    pub bus: Bus,
    /// Bus the request arrived on (the ARRIVAL transmit policy sends the response there).
    pub arrival: Bus,
    /// Millisecond counter at the request STOP; the response deadline (T5max) is measured from it
    /// to the end of the transmission.
    pub arrival_ms: u32,
    /// The link layer has chosen the transmit bus (`bus`); false until `Link::next_tx` hands the
    /// frame out for the first time.
    pub assigned: bool,
    /// The request was accepted on IPMB-B inside the IPMB-B power-up window
    /// (`Link::b_window_open`); `Link::next_tx` keeps such a response on IPMB-B while IPMB-B
    /// stays in the window or becomes enabled.
    pub window: bool,
}

impl EgressFrame {
    const EMPTY: EgressFrame = EgressFrame {
        bytes: [0; IPMB_MAX_LEN],
        len: 0,
        resends: 0,
        bus: Bus::A,
        arrival: Bus::A,
        arrival_ms: 0,
        assigned: false,
        window: false,
    };

    /// as_bytes -- the frame as it goes on the wire.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    /// resends -- how many times the link layer has resent this frame after a failed transmit.
    pub const fn resends(&self) -> u8 {
        self.resends
    }

    /// resend_on -- retargets the frame to `bus` and counts one resend.
    pub fn resend_on(&mut self, bus: Bus) {
        self.bus = bus;
        self.resends = self.resends.saturating_add(1);
    }
}

/// EgressQueue -- FIFO of TXQ_DEPTH response frames.
pub struct EgressQueue {
    frames: Fifo<EgressFrame, TXQ_DEPTH>,
}

impl EgressQueue {
    /// new -- an empty queue.
    pub const fn new() -> Self {
        EgressQueue {
            frames: Fifo::new(EgressFrame::EMPTY),
        }
    }

    /// push -- appends a copy of `frame`, the response to a request that arrived on `arrival` at
    /// `arrival_ms`; the transmit bus is not chosen yet (`assigned` false, `bus` = `arrival`).
    /// Err(Full) counts a drop and keeps every queued frame; Err(FrameTooLong) changes nothing.
    pub fn push(&mut self, frame: &[u8], arrival: Bus, arrival_ms: u32) -> Result<(), QueueError> {
        self.push_flagged(frame, arrival, arrival_ms, false)
    }

    /// push_flagged -- `push` for a response whose request was accepted inside the IPMB-B power-up
    /// window when `window` is true (`EgressFrame::window`).
    pub fn push_flagged(
        &mut self,
        frame: &[u8],
        arrival: Bus,
        arrival_ms: u32,
        window: bool,
    ) -> Result<(), QueueError> {
        let mut item = EgressFrame::EMPTY;
        let dst = item
            .bytes
            .get_mut(..frame.len())
            .ok_or(QueueError::FrameTooLong)?;
        dst.copy_from_slice(frame);
        item.len = frame.len();
        item.bus = arrival;
        item.arrival = arrival;
        item.arrival_ms = arrival_ms;
        item.window = window;
        self.frames.push(item)
    }

    /// reserve -- checks a slot is free without taking it. Err(Full) counts a drop, exactly as
    /// a full `push` does; the dispatcher calls it before running a handler.
    pub fn reserve(&mut self) -> Result<(), QueueError> {
        self.frames.reserve()
    }

    /// pop -- removes and returns the oldest frame; None when empty.
    pub fn pop(&mut self) -> Option<EgressFrame> {
        self.frames.pop()
    }

    /// peek -- the oldest frame, left queued (the one in flight on the bus).
    pub fn peek(&self) -> Option<&EgressFrame> {
        self.frames.peek()
    }

    /// peek_mut -- the oldest frame for retargeting, left queued.
    pub fn peek_mut(&mut self) -> Option<&mut EgressFrame> {
        self.frames.peek_mut()
    }

    /// len -- frames waiting.
    pub const fn len(&self) -> usize {
        self.frames.len()
    }

    /// is_empty -- true when no frame waits.
    pub const fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// is_full -- true when a push would be rejected.
    pub const fn is_full(&self) -> bool {
        self.frames.is_full()
    }

    /// dropped -- frames rejected because the queue was full, since start.
    pub const fn dropped(&self) -> u32 {
        self.frames.dropped()
    }
}

impl Default for EgressQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(tag: u8) -> [u8; 8] {
        [tag; 8]
    }

    #[test]
    fn fifo_order_and_bus_tags() {
        let mut q = EgressQueue::new();
        assert!(q.is_empty());
        q.push(&frame(1), Bus::A, 10).unwrap();
        q.push(&frame(2), Bus::B, 20).unwrap();
        assert_eq!(q.len(), 2);
        let first = q.pop().unwrap();
        assert_eq!((first.as_bytes(), first.bus), (&frame(1)[..], Bus::A));
        assert_eq!(
            (first.arrival, first.arrival_ms, first.assigned),
            (Bus::A, 10, false)
        );
        let second = q.pop().unwrap();
        assert_eq!((second.as_bytes(), second.bus), (&frame(2)[..], Bus::B));
        assert_eq!(
            (second.arrival, second.arrival_ms, second.assigned),
            (Bus::B, 20, false)
        );
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn full_queue_rejects_counts_and_keeps_old_frames() {
        let mut q = EgressQueue::new();
        for tag in 0..TXQ_DEPTH {
            q.push(&frame(tag as u8), Bus::A, 0).unwrap();
        }
        assert!(q.is_full());
        assert_eq!(q.push(&frame(0xEE), Bus::A, 0), Err(QueueError::Full));
        assert_eq!(q.push(&frame(0xEF), Bus::B, 0), Err(QueueError::Full));
        assert_eq!(q.dropped(), 2);
        assert_eq!(q.len(), TXQ_DEPTH);
        for tag in 0..TXQ_DEPTH {
            assert_eq!(q.pop().unwrap().as_bytes(), &frame(tag as u8)[..]);
        }
        assert!(q.is_empty());
    }

    #[test]
    fn wrap_around_keeps_order() {
        let mut q = EgressQueue::new();
        let mut next_push = 0u8;
        let mut next_pop = 0u8;
        // Walk the head around the ring more than twice, keeping 1..=DEPTH frames queued.
        for round in 0..(3 * TXQ_DEPTH) {
            let burst = 1 + round % TXQ_DEPTH;
            for _ in 0..burst {
                q.push(&frame(next_push), Bus::A, 0).unwrap();
                next_push += 1;
            }
            for _ in 0..burst {
                let f = q.pop().unwrap();
                assert_eq!(f.as_bytes(), &frame(next_pop)[..]);
                next_pop += 1;
            }
        }
        assert_eq!(next_push, next_pop);
        assert!(usize::from(next_pop) > 2 * TXQ_DEPTH);
    }

    #[test]
    fn oversize_frame_is_rejected_without_a_drop_count() {
        let mut q = EgressQueue::new();
        assert_eq!(
            q.push(&[0u8; IPMB_MAX_LEN + 1], Bus::A, 0),
            Err(QueueError::FrameTooLong)
        );
        assert_eq!((q.len(), q.dropped()), (0, 0));
    }

    #[test]
    fn peek_leaves_the_frame_and_resend_retargets_it() {
        let mut q = EgressQueue::new();
        assert!(q.peek().is_none());
        q.push(&frame(3), Bus::A, 0).unwrap();
        assert_eq!(q.peek().map(|f| (f.bus, f.resends())), Some((Bus::A, 0)));
        if let Some(head) = q.peek_mut() {
            head.resend_on(Bus::B);
        }
        let sent = q.pop().unwrap();
        assert_eq!((sent.bus, sent.resends()), (Bus::B, 1));
        assert_eq!(sent.as_bytes(), &frame(3)[..]);
    }

    #[test]
    fn generic_fifo_peek_full_and_drop_count() {
        let mut f: Fifo<u8, 2> = Fifo::new(0);
        assert_eq!(f.peek(), None);
        f.push(7).unwrap();
        f.push(8).unwrap();
        assert!(f.is_full());
        assert_eq!(f.push(9), Err(QueueError::Full));
        assert_eq!(f.peek(), Some(&7));
        if let Some(head) = f.peek_mut() {
            *head = 6;
        }
        assert_eq!((f.pop(), f.pop(), f.pop()), (Some(6), Some(8), None));
        assert_eq!(f.dropped(), 1);
    }

    #[test]
    fn bus_helpers_alternate() {
        assert_eq!((Bus::A.other(), Bus::B.other()), (Bus::B, Bus::A));
        assert_eq!(Bus::BOTH, [Bus::A, Bus::B]);
    }
}
