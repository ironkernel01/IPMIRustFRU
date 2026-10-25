//! ring.rs - the one byte ring of the project: the capture log stores its records in it and the
//! firmware's UART driver keeps its receive and transmit queues in it.
//!
//! `ByteRing<N>` is a first-in first-out queue of bytes in a `[u8; N]`. N is a power of two so the
//! indices wrap with a mask; one slot stays empty (head == tail means empty), so the ring holds
//! at most N - 1 bytes. No allocator, no `unsafe`, no panic: every index is masked and every
//! slice access is checked. The ring has no locking; an owner that shares it with an interrupt
//! wraps it in a critical section.

/// ByteRing -- a FIFO of at most N - 1 bytes (N a power of two).
pub struct ByteRing<const N: usize> {
    buf: [u8; N],
    /// Index of the oldest byte.
    head: usize,
    /// Index one past the newest byte.
    tail: usize,
}

impl<const N: usize> ByteRing<N> {
    /// SIZE_OK -- the indices wrap with `& (N - 1)`, and one slot stays empty.
    const SIZE_OK: () = assert!(
        N.is_power_of_two() && N >= 2,
        "a ByteRing size must be a power of two, at least 2"
    );
    /// MASK -- the index wrap mask.
    const MASK: usize = N - 1;

    /// new -- an empty ring.
    pub const fn new() -> Self {
        let () = Self::SIZE_OK;
        ByteRing {
            buf: [0; N],
            head: 0,
            tail: 0,
        }
    }

    /// capacity -- the most bytes the ring holds (N - 1).
    pub const fn capacity() -> usize {
        N - 1
    }

    /// len -- bytes held.
    pub const fn len(&self) -> usize {
        self.tail.wrapping_sub(self.head) & Self::MASK
    }

    /// is_empty -- no bytes held.
    pub const fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// free -- bytes that still fit.
    pub const fn free(&self) -> usize {
        Self::capacity() - self.len()
    }

    /// clear -- drops every byte.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }

    /// push -- appends one byte; false (nothing written) when the ring is full.
    pub fn push(&mut self, b: u8) -> bool {
        if self.free() == 0 {
            return false;
        }
        if let Some(slot) = self.buf.get_mut(self.tail) {
            *slot = b;
        }
        self.tail = (self.tail + 1) & Self::MASK;
        true
    }

    /// pop -- removes and returns the oldest byte.
    pub fn pop(&mut self) -> Option<u8> {
        let b = self.peek_at(0)?;
        self.head = (self.head + 1) & Self::MASK;
        Some(b)
    }

    /// peek_at -- the i-th oldest byte (0 is the oldest), None past the end.
    pub fn peek_at(&self, i: usize) -> Option<u8> {
        if i >= self.len() {
            return None;
        }
        self.buf.get((self.head + i) & Self::MASK).copied()
    }

    /// push_slice -- appends all of `s` or none of it; false when it does not fit.
    pub fn push_slice(&mut self, s: &[u8]) -> bool {
        if s.len() > self.free() {
            return false;
        }
        for &b in s {
            let _ = self.push(b); // cannot fail: the room was checked
        }
        true
    }

    /// discard -- drops the `n` oldest bytes (at most `len`).
    pub fn discard(&mut self, n: usize) {
        self.head = (self.head + n.min(self.len())) & Self::MASK;
    }
}

impl<const N: usize> Default for ByteRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_ring_is_empty_and_holds_n_minus_one() {
        let ring = ByteRing::<8>::new();
        assert!(ring.is_empty());
        assert_eq!((ring.len(), ring.free()), (0, 7));
        assert_eq!(ByteRing::<8>::capacity(), 7);
        assert_eq!(ring.peek_at(0), None);
    }

    #[test]
    fn push_fills_to_capacity_then_refuses_and_writes_nothing() {
        let mut ring = ByteRing::<8>::new();
        for b in 1..=7u8 {
            assert!(ring.push(b));
        }
        assert!(!ring.push(99));
        assert_eq!((ring.len(), ring.free()), (7, 0));
        let drained: Vec<u8> = std::iter::from_fn(|| ring.pop()).collect();
        assert_eq!(drained, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn indices_wrap_past_the_end_many_times() {
        let mut ring = ByteRing::<8>::new();
        let mut next = 0u8;
        let mut want = 0u8;
        for round in 0..100usize {
            for _ in 0..(round % 5 + 1) {
                assert!(ring.push(next));
                next = next.wrapping_add(1);
            }
            for _ in 0..(round % 5 + 1) {
                assert_eq!(ring.pop(), Some(want));
                want = want.wrapping_add(1);
            }
        }
        assert!(ring.is_empty());
    }

    #[test]
    fn push_slice_is_all_or_nothing() {
        let mut ring = ByteRing::<8>::new();
        assert!(ring.push_slice(&[1, 2, 3, 4, 5]));
        assert!(!ring.push_slice(&[6, 7, 8]));
        assert_eq!(ring.len(), 5, "a refused slice leaves the ring as it was");
        assert!(ring.push_slice(&[6, 7]));
        assert_eq!(ring.free(), 0);
        assert!(ring.push_slice(&[]));
        assert!(!ring.push_slice(&[1]));
    }

    #[test]
    fn push_slice_wraps_the_end_of_the_buffer() {
        let mut ring = ByteRing::<8>::new();
        assert!(ring.push_slice(&[0; 6]));
        ring.discard(6);
        assert!(ring.push_slice(&[1, 2, 3, 4, 5]));
        let got: Vec<u8> = (0..5).filter_map(|i| ring.peek_at(i)).collect();
        assert_eq!(got, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn discard_drops_the_oldest_and_stops_at_len() {
        let mut ring = ByteRing::<8>::new();
        assert!(ring.push_slice(&[1, 2, 3, 4, 5]));
        ring.discard(2);
        assert_eq!((ring.len(), ring.peek_at(0)), (3, Some(3)));
        ring.discard(100);
        assert!(ring.is_empty());
        ring.discard(1);
        assert!(ring.is_empty());
    }

    #[test]
    fn peek_at_reads_the_ith_oldest_without_removing() {
        let mut ring = ByteRing::<8>::new();
        assert!(ring.push_slice(&[10, 20, 30]));
        assert_eq!(
            (ring.peek_at(0), ring.peek_at(2), ring.peek_at(3)),
            (Some(10), Some(30), None)
        );
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn clear_empties_the_ring() {
        let mut ring = ByteRing::<4>::new();
        assert!(ring.push_slice(&[1, 2, 3]));
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.free(), 3);
    }
}
