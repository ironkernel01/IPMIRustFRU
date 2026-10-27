// fault.rs - HardFault, DefaultHandler (unhandled interrupt) and panic handlers.
//
// Fail-stop policy: a fault leaves the device in an unknown state, so each handler records what
// happened in the kept fault record (bsp/fault_record.rs, NOINIT, readable with a debugger),
// prints one line on the console when the UART is up, then parks in `wfi` forever. In the runtime
// with the watchdog started (bsp/watchdog.rs) the watchdog then resets the processor and the next
// start reports the kept record; in the bootloader (no watchdog) and before the runtime starts the
// watchdog the processor stays parked. The same park loop serves the initialisation failures in
// main.rs.
// The console print is best effort and bounded by the UART's per-message transmit timeout. It
// breaks the "no logging from an interrupt handler" rule on purpose: nothing else runs after it.
// cortex-m-rt 0.7.7 requires `unsafe fn` for HardFault and accepts it for DefaultHandler; both
// are defined as `unsafe fn` here. The panic handler prints the source location only (no message
// formatting: no `core::fmt` is reachable from this file).

use cortex_m_rt::{exception, ExceptionFrame};

use ipmc_sensors::drivers::stm32f401::ports::PortError;

use ipmc_boot::text::{self, TextOut};

use crate::bsp::fault_record::{self, Fault};
use crate::bsp::uart::{Port, PortText};
use crate::bsp::{led, uart};

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

/// record_print_park -- the shared body of the handlers: stores `fault` in the kept record,
/// prints one console line when the UART is up (`file` is the source file a panic names; the
/// record keeps the line and column), then parks (the watchdog, when started, resets
/// the processor). A fault that fires while the console state is borrowed (inside a UART call)
/// prints nothing: the state cannot be taken inside the fault (`with_port` returns None).
fn record_print_park(fault: Fault, file: &str) -> ! {
    fault_record::write(fault);
    if uart::is_up(Port::Console) {
        match fault {
            Fault::HardFault { pc } => say(&[
                Piece::Text(
                    "
HARDFAULT pc=0x",
                ),
                Piece::Hex8(pc),
                Piece::Text(
                    "
",
                ),
            ]),
            // The IRQ number as a signed decimal: system exceptions are negative.
            Fault::UnhandledIrq { irq } => say(&[
                Piece::Text(
                    "
UNHANDLED IRQ ",
                ),
                Piece::Text(if irq < 0 { "-" } else { "" }),
                Piece::Dec(irq.unsigned_abs()),
                Piece::Text(
                    "
",
                ),
            ]),
            Fault::Panic { line, .. } => say(&[
                Piece::Text(
                    "
PANIC ",
                ),
                Piece::Text(file),
                Piece::Char(':'),
                Piece::Dec(line),
                Piece::Text(
                    "
",
                ),
            ]),
        }
        // The UART interrupt cannot run here (this handler outranks it); `flush` polls the TX
        // ring out.
        uart::flush(Port::Console);
    }
    park()
}

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    record_print_park(Fault::HardFault { pc: ef.pc() }, "")
}

#[exception]
unsafe fn DefaultHandler(irqn: i16) {
    // Sign-extension keeps negative system-exception numbers recognisable in the record.
    record_print_park(
        Fault::UnhandledIrq {
            irq: i32::from(irqn),
        },
        "",
    )
}

/// panic -- the panic handler of both images: the same fail-stop path with the source file, line
/// and column (empty and 0 when the location is unknown).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let (file, line, column) = info.location().map_or(("", 0, 0), |location| {
        (location.file(), location.line(), location.column())
    });
    record_print_park(Fault::Panic { line, column }, file)
}
