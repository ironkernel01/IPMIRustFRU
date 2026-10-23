// bsp/static_slot.rs - the one access pattern of the driver statics: a `Mutex<RefCell<Option<T>>>`
// filled by the driver's `init` and used inside a critical section. Every driver that keeps its
// peripheral or pins in such a static installs and reaches them through these two functions.

use core::cell::RefCell;
use cortex_m::interrupt::{free, Mutex};

/// StaticSlot -- driver state shared between thread and interrupt context; `None` before `init`.
pub type StaticSlot<T> = Mutex<RefCell<Option<T>>>;

/// EMPTY -- the value of a slot before `init`.
pub const fn empty<T>() -> StaticSlot<T> {
    Mutex::new(RefCell::new(None))
}

/// install -- stores `value` in `slot` (replacing any earlier one) inside a critical section.
pub fn install<T>(slot: &StaticSlot<T>, value: T) {
    free(|cs| {
        slot.borrow(cs).replace(Some(value));
    });
}

/// with_mut -- runs `f` on the state inside a critical section; `None` (and `f` not run) before
/// `init`.
pub fn with_mut<T, R>(slot: &StaticSlot<T>, f: impl FnOnce(&mut T) -> R) -> Option<R> {
    free(|cs| slot.borrow(cs).borrow_mut().as_mut().map(f))
}
