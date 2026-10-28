// fault.rs - the kept fault record as plain data: what a fault handler stores and the next start
// reports, and its encoding as three words.

/// FAULT_MAGIC -- the check value that marks the words as written by a fault handler: the check
/// word is the XOR of the other three words and this constant, which random RAM after a power-on
/// matches with probability 2^-32 (as `KEPT_MAGIC` of the kept request sequence).
pub const FAULT_MAGIC: u32 = 0xFA17_0C0D;

/// Kind codes in the KIND word.
const KIND_HARDFAULT: u32 = 1;
const KIND_UNHANDLED_IRQ: u32 = 2;
const KIND_PANIC: u32 = 3;

/// Fault -- what the record holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// HardFault; `pc` is the stacked program counter.
    HardFault { pc: u32 },
    /// Unhandled interrupt; `irq` is the number (negative: a system exception).
    UnhandledIrq { irq: i32 },
    /// Panic; the source line and column (0 when the location is unknown).
    Panic { line: u32, column: u32 },
}

impl Fault {
    /// to_words -- the record as [KIND, VALUE, AUX].
    pub const fn to_words(self) -> [u32; 3] {
        match self {
            Fault::HardFault { pc } => [KIND_HARDFAULT, pc, 0],
            // The cast reinterprets the bits, so a negative system-exception number stays
            // recognisable.
            Fault::UnhandledIrq { irq } => [KIND_UNHANDLED_IRQ, irq as u32, 0],
            Fault::Panic { line, column } => [KIND_PANIC, line, column],
        }
    }

    /// from_words -- the fault `to_words` encoded; None when the kind is unknown.
    pub fn from_words(words: [u32; 3]) -> Option<Fault> {
        let [kind, value, aux] = words;
        match kind {
            KIND_HARDFAULT => Some(Fault::HardFault { pc: value }),
            // The number was stored as its 32 bits.
            KIND_UNHANDLED_IRQ => Some(Fault::UnhandledIrq { irq: value as i32 }),
            KIND_PANIC => Some(Fault::Panic {
                line: value,
                column: aux,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_including_negative_irq() {
        let faults = [
            Fault::HardFault { pc: 0x0800_1234 },
            Fault::UnhandledIrq { irq: 23 },
            Fault::UnhandledIrq { irq: -5 },
            Fault::Panic {
                line: 77,
                column: 9,
            },
        ];
        for fault in faults {
            assert!(Fault::from_words(fault.to_words()) == Some(fault));
        }
        assert_eq!(Fault::UnhandledIrq { irq: -5 }.to_words()[1], 0xFFFF_FFFB);
    }

    #[test]
    fn unknown_kind_is_none() {
        assert!(Fault::from_words([0, 1, 2]).is_none());
        assert!(Fault::from_words([4, 1, 2]).is_none());
    }
}
