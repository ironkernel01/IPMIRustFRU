// serial.rs - one serial port as the IPMC code sees it: the byte interface and the failure counts.

/// SerialCounters -- snapshot of the failure counts of one port since reset.
#[derive(Clone, Copy, Debug, Default)]
pub struct SerialCounters {
    pub rx_overflow: u32,
    pub tx_timeout: u32,
    pub overrun: u32,
    /// Bytes received with a framing, noise or parity error (FE, NF, PE).
    pub line_errors: u32,
    /// Trace lines dropped because the TX ring had no room for the whole line.
    pub tx_lines_dropped: u32,
}

impl SerialCounters {
    /// lost -- bytes the port has lost or received damaged since reset: overrun, RX ring overflow
    /// and framing, noise or parity errors (wrapping sum). A damaged byte cannot be trusted, so a
    /// move of this count means the line in progress is unreliable.
    pub fn lost(&self) -> u32 {
        self.overrun
            .wrapping_add(self.rx_overflow)
            .wrapping_add(self.line_errors)
    }
}

/// SerialError -- why a waiting write did not finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerialError {
    /// The port is not started.
    NotReady,
    /// The TX ring stayed full for the whole bound.
    TxTimeout,
}

/// SerialPort -- one interrupt-driven UART with RX and TX rings (the console or the application
/// port).
pub trait SerialPort {
    /// is_up -- the port started; before that every call is a no-op, None or 0.
    fn is_up(&self) -> bool;
    /// getc -- the next received byte, None when the ring is empty.
    fn getc(&mut self) -> Option<u8>;
    /// discard_rx -- drops every byte in the RX ring and resumes a stalled receiver.
    fn discard_rx(&mut self);
    /// write -- copies what the TX ring takes and returns the count; never waits.
    fn write(&mut self, bytes: &[u8]) -> usize;
    /// write_all_wait -- copies all bytes, waiting (bounded) while the ring is full; the bytes
    /// not copied are dropped on an error.
    fn write_all_wait(&mut self, bytes: &[u8]) -> Result<(), SerialError>;
    /// tx_free -- the bytes the TX ring takes now.
    fn tx_free(&self) -> usize;
    /// tx_pending -- bytes still in the ring or the shift register.
    fn tx_pending(&self) -> bool;
    /// flush -- waits (bounded) until the last byte has left the shift register.
    fn flush(&mut self);
    /// counters -- the failure counts.
    fn counters(&self) -> SerialCounters;
    /// count_line_dropped -- books one trace line dropped for want of ring room.
    fn count_line_dropped(&mut self);
}

/// CONSOLE_TX_USABLE -- bytes the console TX ring holds at once: a ring of N bytes keeps one slot
/// free. Console output waits while the ring is full, so a command whose whole output is no
/// longer than this never waits for the wire (the console commands bound their output by it).
pub const CONSOLE_TX_USABLE: usize = ipmc_defs::IPMC_UART_TX_BUF - 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_sums_and_wraps() {
        let c = SerialCounters {
            rx_overflow: 1,
            tx_timeout: 100,
            overrun: 2,
            line_errors: 4,
            tx_lines_dropped: 100,
        };
        assert_eq!(c.lost(), 7);
        let wrap = SerialCounters {
            rx_overflow: u32::MAX,
            tx_timeout: 0,
            overrun: 2,
            line_errors: 0,
            tx_lines_dropped: 0,
        };
        assert_eq!(wrap.lost(), 1);
    }
}
