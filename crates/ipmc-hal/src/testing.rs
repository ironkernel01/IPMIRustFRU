// testing.rs - host test doubles of the ipmc-hal traits shared by the crates' tests: a serial port
// that records what is written, a settable clock and cycle counter, strap levels and a status LED.
// Behind the `testing` feature (std); every handle is a clone that shares its state, so a test
// reads what the code under test did.

extern crate std;

use core::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

use crate::serial::{SerialCounters, SerialError, SerialPort};
use crate::system::{StatusLed, Straps};
use crate::time::{Clock, CycleCounter};
use ipmc_sensors::eval::led::{self, Mode, Shown};
/// PortState -- what a `FakePort` and its clones share.
struct PortState {
    sent: Vec<u8>,
    room: usize,
    dropped: u32,
    flushes: u32,
    up: bool,
    counters: SerialCounters,
    /// Bytes the test "received" on the port, for `getc`.
    rx: VecDeque<u8>,
}

/// FakePort -- a console ring of `room` bytes that never drains; it keeps every byte written. A
/// clone shares the ring, so a test reads what the code under test wrote.
#[derive(Clone)]
pub struct FakePort {
    inner: Rc<RefCell<PortState>>,
}

impl FakePort {
    /// new -- a ring that takes `room` bytes.
    pub fn new(room: usize) -> Self {
        FakePort {
            inner: Rc::new(RefCell::new(PortState {
                sent: Vec::new(),
                room,
                dropped: 0,
                flushes: 0,
                up: true,
                counters: SerialCounters::default(),
                rx: VecDeque::new(),
            })),
        }
    }

    /// roomy -- a ring no test fills.
    pub fn roomy() -> Self {
        FakePort::new(1 << 20)
    }

    /// text -- what was written.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.inner.borrow().sent).into_owned()
    }

    /// sent_len -- bytes written.
    pub fn sent_len(&self) -> usize {
        self.inner.borrow().sent.len()
    }

    /// dropped -- trace lines dropped for want of room.
    pub fn dropped(&self) -> u32 {
        self.inner.borrow().dropped
    }

    /// flushes -- calls of `flush`.
    pub fn flushes(&self) -> u32 {
        self.inner.borrow().flushes
    }

    /// set_up -- the port is started or not (`is_up`).
    pub fn set_up(&self, up: bool) {
        self.inner.borrow_mut().up = up;
    }

    /// set_counters -- the failure counts the port reports.
    pub fn set_counters(&self, counters: SerialCounters) {
        self.inner.borrow_mut().counters = counters;
    }

    /// feed -- bytes arrive on the port.
    pub fn feed(&self, text: &str) {
        self.inner.borrow_mut().rx.extend(text.bytes());
    }

    /// rx_pending -- received bytes `getc` has not taken.
    pub fn rx_pending(&self) -> usize {
        self.inner.borrow().rx.len()
    }

    /// take_text -- what was written since the last call.
    pub fn take_text(&self) -> String {
        let mut state = self.inner.borrow_mut();
        let text = String::from_utf8_lossy(&state.sent).into_owned();
        state.sent.clear();
        text
    }
}

impl SerialPort for FakePort {
    fn is_up(&self) -> bool {
        self.inner.borrow().up
    }
    fn getc(&mut self) -> Option<u8> {
        self.inner.borrow_mut().rx.pop_front()
    }
    fn discard_rx(&mut self) {
        self.inner.borrow_mut().rx.clear();
    }
    fn write(&mut self, bytes: &[u8]) -> usize {
        let mut state = self.inner.borrow_mut();
        let n = bytes.len().min(state.room - state.sent.len());
        state.sent.extend_from_slice(&bytes[..n]);
        n
    }
    fn write_all_wait(&mut self, bytes: &[u8]) -> Result<(), SerialError> {
        if self.write(bytes) == bytes.len() {
            Ok(())
        } else {
            Err(SerialError::TxTimeout)
        }
    }
    fn tx_free(&self) -> usize {
        let state = self.inner.borrow();
        state.room - state.sent.len()
    }
    fn tx_pending(&self) -> bool {
        false
    }
    fn flush(&mut self) {
        self.inner.borrow_mut().flushes += 1;
    }
    fn counters(&self) -> SerialCounters {
        self.inner.borrow().counters
    }
    fn count_line_dropped(&mut self) {
        self.inner.borrow_mut().dropped += 1;
    }
}

/// TestClock -- the 1 ms tick, set by the test; clones share it.
#[derive(Clone, Default)]
pub struct TestClock(Rc<Cell<u32>>);

impl TestClock {
    /// set -- the time in milliseconds.
    pub fn set(&self, ms: u32) {
        self.0.set(ms);
    }

    /// advance -- moves the time on by `ms`.
    pub fn advance(&self, ms: u32) {
        self.0.set(self.0.get().wrapping_add(ms));
    }
}

impl Clock for TestClock {
    fn millis(&self) -> u32 {
        self.0.get()
    }
    fn seconds(&self) -> u32 {
        self.0.get() / 1000
    }
}

/// TEST_CYCLES_HZ -- the count rate of `TestCycles`: 1000 counts per millisecond.
pub const TEST_CYCLES_HZ: u32 = 1_000_000;

/// TestCycles -- a cycle counter that advances by `step` on every read; clones share it.
#[derive(Clone, Default)]
pub struct TestCycles {
    count: Rc<Cell<u32>>,
    step: Rc<Cell<u32>>,
}

impl TestCycles {
    /// set -- the count.
    pub fn set(&self, count: u32) {
        self.count.set(count);
    }

    /// set_step -- counts added by each read.
    pub fn set_step(&self, step: u32) {
        self.step.set(step);
    }
}

impl CycleCounter for TestCycles {
    fn now(&self) -> u32 {
        self.count
            .replace(self.count.get().wrapping_add(self.step.get()))
    }
    fn hz(&self) -> u32 {
        TEST_CYCLES_HZ
    }
}

/// TestStraps -- the strap pin levels and the BOOTLOADER pin; clones share them.
#[derive(Clone, Default)]
pub struct TestStraps(Rc<Cell<u8>>, Rc<Cell<bool>>);

impl TestStraps {
    /// set -- the raw strap levels (bit n = strap n, the `ipmc_core::addr` convention).
    pub fn set(&self, raw: u8) {
        self.0.set(raw);
    }

    /// set_bootloader_pin -- the BOOTLOADER strap level.
    pub fn set_bootloader_pin(&self, high: bool) {
        self.1.set(high);
    }
}

impl Straps for TestStraps {
    fn read_raw(&self) -> u8 {
        self.0.get()
    }
    fn bootloader_pin_high(&self) -> bool {
        self.1.get()
    }
}

/// TestLed -- a status LED that keeps its mode (shown at time 0, with no activity); clones share it.
#[derive(Clone)]
pub struct TestLed(Rc<Cell<Mode>>);

impl Default for TestLed {
    fn default() -> Self {
        TestLed(Rc::new(Cell::new(Mode::Auto)))
    }
}

impl TestLed {
    /// mode -- the mode last set.
    pub fn mode(&self) -> Mode {
        self.0.get()
    }
}

impl StatusLed for TestLed {
    fn set_mode(&mut self, mode: Mode) {
        self.0.set(mode);
    }
    fn shown(&self) -> Shown {
        led::shown(self.0.get(), 0, led::NO_ACTIVITY_MS)
    }
    fn activity(&self) {}
}
