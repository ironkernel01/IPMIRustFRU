// fault.rs - HardFault and DefaultHandler (unhandled interrupt) handlers.
//
// Fail-stop policy: a fault leaves the device in an unknown state, so each handler records what
// happened in a static record (readable with a debugger), prints one line on the console when
// the UART is up, then parks in `wfi` forever. There is no reset: a reset would hide the fault
// from the bench and could restart a bus transaction mid-frame. The same park loop serves the
// initialisation failures in main.rs.
// The console print is best effort and bounded by the UART's per-message transmit timeout. It
// breaks the "no logging from an interrupt handler" rule on purpose: nothing else runs after it.
// cortex-m-rt 0.7.7 requires `unsafe fn` for HardFault and accepts it for DefaultHandler; both
// are defined as `unsafe fn` here.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m_rt::{exception, ExceptionFrame};

use ipmc_sensors::drivers::stm32f401::ports::PortError;

use ipmc_boot::text::{self, TextOut};

use crate::bsp::uart::{Port, PortText};
use crate::bsp::{led, uart};

/// Fault kind codes in `FAULT_KIND`.
const KIND_NONE: u32 = 0;
const KIND_HARDFAULT: u32 = 1;
const KIND_UNHANDLED_IRQ: u32 = 2;

// Written once by the fault handler (exception context), never read by firmware; a debugger reads
// them. `#[used]` keeps them in the image.
#[used]
static FAULT_KIND: AtomicU32 = AtomicU32::new(KIND_NONE);
/// Stacked PC for a HardFault, the IRQ number (sign-extended; negative = system exception) for
/// an unhandled interrupt.
#[used]
static FAULT_VALUE: AtomicU32 = AtomicU32::new(0);

/// park -- the terminal fail-stop loop.
pub fn park() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

/// claim_failed -- reports a failed pin claim and parks. With the console up it prints one
/// line naming the pin and both owners; before that it flashes the LED at
/// IPMC_FAULT_BLINK_CHZ.
pub fn claim_failed(error: PortError) -> ! {
    if uart::is_up(Port::Console) {
        report_claim(error);
    } else {
        led::set_fault(ipmc_defs::IPMC_FAULT_BLINK_CHZ);
    }
    park()
}

/// say -- `pieces` on the console, in order: fixed text and the number writers of
/// `ipmc_boot::text`, no `core::fmt` (both images link this file). A line end is the two bytes
/// CR LF. The console print is best effort, as in `uart::print`.
fn say(pieces: &[Piece]) {
    let mut out = PortText(Port::Console);
    for piece in pieces {
        match *piece {
            Piece::Text(text) => out.put(text),
            Piece::Dec(value) => text::put_dec(&mut out, value),
            Piece::Hex8(value) => text::put_hex(&mut out, value, 8),
            Piece::Char(ch) => out.put(ch.encode_utf8(&mut [0u8; 4])),
        }
    }
}

/// Piece -- one part of a fault line.
#[derive(Clone, Copy)]
enum Piece<'a> {
    Text(&'a str),
    Dec(u32),
    Hex8(u32),
    Char(char),
}

/// report_claim -- prints the claim failure line on the console and waits for it to leave.
fn report_claim(error: PortError) {
    match error {
        PortError::AlreadyClaimed {
            port,
            pin,
            owner,
            claimant,
        } => say(&[
            Piece::Text("\r\nFATAL: pin P"),
            Piece::Char(port.letter()),
            Piece::Dec(u32::from(pin)),
            Piece::Text(" already claimed by '"),
            Piece::Text(owner),
            Piece::Text("', claimed again by '"),
            Piece::Text(claimant),
            Piece::Text("'\r\n"),
        ]),
        other => say(&[
            Piece::Text("\r\nFATAL: pin claim failed: "),
            Piece::Text(match other {
                PortError::AlreadyClaimed { .. } => "AlreadyClaimed",
                PortError::EmptyMask => "EmptyMask",
                PortError::EmptyOwner => "EmptyOwner",
                PortError::InvalidPin => "InvalidPin",
                PortError::NotInitialised => "NotInitialised",
                PortError::AlreadyInitialised => "AlreadyInitialised",
            }),
            Piece::Text("\r\n"),
        ]),
    }
    uart::flush(Port::Console);
}

/// record_print_park -- the shared body of both handlers: records `kind` and `value` in the
/// static record, prints one console line when the UART is up, then parks. `value` is the
/// stacked PC for KIND_HARDFAULT and the sign-extended IRQ number for KIND_UNHANDLED_IRQ.
/// A fault that fires while the console state is borrowed (inside a UART call) prints nothing:
/// the state cannot be taken inside the fault (`with_port` returns None).
fn record_print_park(kind: u32, value: u32) -> ! {
    FAULT_VALUE.store(value, Ordering::Relaxed);
    FAULT_KIND.store(kind, Ordering::Relaxed);
    if uart::is_up(Port::Console) {
        if kind == KIND_HARDFAULT {
            say(&[
                Piece::Text("\r\nHARDFAULT pc=0x"),
                Piece::Hex8(value),
                Piece::Text("\r\n"),
            ]);
        } else {
            // The IRQ number as a signed decimal: system exceptions are negative.
            let irq = value as i32;
            say(&[
                Piece::Text("\r\nUNHANDLED IRQ "),
                Piece::Text(if irq < 0 { "-" } else { "" }),
                Piece::Dec(irq.unsigned_abs()),
                Piece::Text("\r\n"),
            ]);
        }
        // The UART interrupt cannot run here (this handler outranks it); `flush` polls the TX
        // ring out.
        uart::flush(Port::Console);
    }
    park()
}

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    record_print_park(KIND_HARDFAULT, ef.pc())
}

#[exception]
unsafe fn DefaultHandler(irqn: i16) {
    // Sign-extension keeps negative system-exception numbers recognisable in the record.
    record_print_park(KIND_UNHANDLED_IRQ, i32::from(irqn) as u32)
}
