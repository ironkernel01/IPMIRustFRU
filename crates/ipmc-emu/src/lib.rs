//! ipmc-emu - the IPMC on the PC.
//!
//! The emulator is the IPMC with the I2C drivers replaced by captured traffic. It builds the
//! `ipmc-core` Link and dispatch Context and the `ipmc-sensors` Monitor over the simulated
//! backends exactly as the firmware does, then feeds frames in and collects frames out through
//! the same main-loop calls the drivers will make. All of it is Rust and runs on the host; there
//! is no C harness. This documentation is the user documentation of the crate.
//!
//! # Modules
//!
//! | Module | Holds |
//! |---|---|
//! | `vectors` | the one reader of the vector file format (`vectors/sbc_ab.vec`) |
//! | `emulator` | `Emulator`, the board in software, and `EmuConfig` |
//! | `replay` | replay of a vector list through an `Emulator`, the comparison with the captured responses and `ReplayReport` |
//! | `trace` | the stdout trace sink and the hex formatter |
//!
//! # Running
//!
//! ```text
//! cargo run -p ipmc-emu -- replay <file.vec> [options]
//! cargo run -p ipmc-emu -- sdr
//! cargo run -p ipmc-emu -- help
//! ```
//!
//! `sdr` builds the emulator with the `ipmi_defs.h` defaults and prints its Device SDR
//! repository, one line per record from 0000h along the next-ID chain: the record ID as four hex
//! digits, then the record bytes in hex separated by spaces. `tools/captured_sdr.py` decodes
//! these lines next to the captured records. Exit status 0, or 1 when the chain names a missing
//! record or does not end.
//!
//! `replay` feeds every request in the file that is addressed to the emulator (IPMB 82h with
//! the default strap) to the Link at its capture time, polls, takes the response out and
//! reports the transmission done 1 ms later. It then compares each response with the captured
//! response of the same exchange and prints a table per command.
//!
//! | Class | Meaning |
//! |---|---|
//! | Match | byte-identical frames |
//! | SameCC | same completion code, data differs |
//! | CCDiff | different completion codes |
//! | NotImpl | the emulator answers C1h, the captured controller did not |
//! | Unans | the emulator sent no response |
//! | NoCap | the emulator answered, the file holds no response to compare |
//! | NoBus | the request arrived on a bus that is not fitted in this build; expected, not a fault |
//!
//! Under the table the report prints these lines. Each states a designed behaviour of the
//! changed conditions, not a defect:
//!
//! - `responses on another bus than captured: N`, always informational. With the alternate
//!   policy and both buses enabled the IPMC alternates its transmissions (Rule 4.1.1-6) while the
//!   captured board answered on the arrival bus; the line names that rule only in such a run.
//! - `injected transmit failures: F, resent on the other bus: R, responses lost: L`, printed
//!   with `--fail-every` (Rule 4.1.1-7). F, R and L are the link's `tx_fail`, `resends` and
//!   `tx_dropped` counters summed over both buses; a resend that fails again counts in F and R
//!   once more. L above 0 fails the replay.
//! - `requests on IPMB-B, not fitted in this build: N`, printed when IPMB-B is not fitted. A
//!   Chassis Manager resends such a request on IPMB-A (Rule 5.1.1-6); that resend belongs to
//!   the Phase 7 Chassis Manager emulator. A request on a fitted but disabled bus stays Unans.
//!
//! The last line is `PASS`, or `FAIL: ` and the reasons (unanswered, answered more than once,
//! dispatch errors, egress dropped, responses lost) with their counts.
//!
//! Options (defaults come from `ipmi_defs.h` through `EmuConfig::from_defs`):
//!
//! | Option | Effect |
//! |---|---|
//! | `--b-fitted 0\|1` | IPMB-B is built in (default `IPMB_BUS_B_ENABLE`) |
//! | `--b-enabled 0\|1` | IPMB-B starts enabled, as after the Chassis Manager's Set IPMB State (default 0, the power-up state; needs `--b-fitted 1`) |
//! | `--policy alternate\|arrival` | response bus policy (default `IPMB_TX_POLICY`) |
//! | `--fail-every N` | every Nth transmission is reported failed, so the Link resends it on the other bus (default 0, none) |
//! | `--trace` | print every received and transmitted frame and every trace line to stdout |
//! | `--quiet` | print only a one-line summary |
//!
//! Exit status: 0 when the replay passed (every request on a fitted bus answered exactly once, no
//! dispatch error, no response dropped from the egress queue, no response lost to an injected
//! transmit failure), 1 when it did not, 2 for a usage or file
//! error (message on stderr).
//!
//! The capture was made with IPMB-B fitted and alternating buses, so
//! `--b-fitted 1 --b-enabled 1 --policy arrival` reproduces its traffic. The build default
//! (IPMB-B not fitted) counts the IPMB-B requests as NoBus and passes, which is the bench
//! situation with one Aardvark.
#![forbid(unsafe_code)]

pub mod emulator;
pub mod replay;
pub mod trace;
pub mod vectors;
