//! ipmc-emu - the IPMC on the PC.
//!
//! The emulator is the IPMC with the I2C drivers replaced by captured traffic. It runs the
//! firmware's own code: `ipmc-runtime` (console, sensors, IPMB link service, FRU log, start-up
//! and main loop) on the simulated board `ipmc_runtime::sim::Sim`, feeds frames into the bus
//! queues and drains the frames the master wrote. A Cold Reset restarts it from the RAM flash and
//! the kept words, as the processor restart does. All of it is Rust and runs on the host; there
//! is no C harness. This documentation is the user documentation of the crate.
//!
//! # Modules
//!
//! | Module | Holds |
//! |---|---|
//! | `vectors` | the one reader of the vector file format (`vectors/sbc_ab.vec`) |
//! | `emulator` | `Emulator`, the board in software (the firmware's runtime on the simulated board), and `EmuConfig` |
//! | `replay` | replay of a vector list through an `Emulator`, the comparison with the captured responses and `ReplayReport` |
//! | `serve` | `Board`, the emulator in real time, and the TCP line protocol of `ipmc-emu serve` |
//! | `trace` | the stdout trace sink and the hex formatter |
//!
//! # Running
//!
//! ```text
//! cargo run -p ipmc-emu -- replay <file.vec> [options]
//! cargo run -p ipmc-emu -- serve [--port N] [--b-fitted 0|1] [--policy alternate|arrival] [--b-power-up strict|nonstrict] [--trace]
//! cargo run -p ipmc-emu -- sdr
//! cargo run -p ipmc-emu -- help
//! ```
//!
//! `sdr` builds the emulator with the `defs.rs` defaults and prints its Device SDR
//! repository, one line per record from 0000h along the next-ID chain: the record ID as four hex
//! digits, then the record bytes in hex separated by spaces. `tools/captured_sdr.py` decodes
//! these lines next to the captured records. Exit status 0, or 1 when the chain names a missing
//! record or does not end.
//!
//! `replay` feeds every request in the file that is addressed to the emulator (IPMB 82h with
//! the default strap) to the board at its capture time, runs the board's main loop and takes the
//! response out (the simulated bus takes every write, except the ones `--fail-every` fails). It
//! then compares each response with the captured response of the same exchange and prints a
//! table per command.
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
//! At Tier 2 and 3 the IPMC sends requests of its own (Platform Event to the Event Receiver at
//! 20h). A frame the IPMC transmits with an even NetFn is such a request (`replay::is_request_frame`,
//! the one classification), an odd one is a response. The replay is no answer-count for a request:
//! it plays the Chassis Manager's Event Receiver, answers a Platform Event with 00h and any other
//! request with C1h one step later on the bus it left on, and reports `ipmc_requests` and `events`
//! (in the summary line and the report only when nonzero, so a Tier-1 report is unchanged).
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
//! Options (defaults come from `defs.rs` through `EmuConfig::from_defs`):
//!
//! | Option | Effect |
//! |---|---|
//! | `--b-fitted 0\|1` | IPMB-B is built in (default `IPMB_BUS_B_ENABLE`) |
//! | `--b-enabled 0\|1` | IPMB-B starts enabled, as after the Chassis Manager's Set IPMB State (default 0, the power-up state; needs `--b-fitted 1`) |
//! | `--policy alternate\|arrival` | response bus policy (default from `IPMB_COMPLIANCE`: `arrival` for NonStrict, `alternate` for Strict) |
//! | `--b-power-up strict\|nonstrict` | IPMB-B before the first Set IPMB State that sets it: `strict` drops its frames (Rule 4.1.1-4), `nonstrict` answers requests that arrive on IPMB-B on IPMB-B (default from `IPMB_COMPLIANCE`: `nonstrict` for NonStrict, `strict` for Strict; `--b-enabled 1` ends the window at once) |
//! | `--fail-every N` | every Nth write fails on the simulated bus, so the Link resends it on the other bus (default 0, none) |
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
//!
//! # Serve mode
//!
//! `serve` runs the emulator in real time (std `Instant`, one step every `IPMC_EMU_SERVE_POLL_MS`
//! in `defs.rs`) and listens on 127.0.0.1. `--port N` sets the TCP port (default
//! `IPMC_EMU_PORT` in `defs.rs`; 0 picks a free port). The first stdout line is `listening
//! 127.0.0.1:<port>`. One client at a time is served; a new client starts a fresh board in its
//! start-up state, as a power cycle (IPMB-B disabled until Set IPMB State, Rule 4.1.1-4; with
//! `--b-power-up nonstrict` its requests are answered until then).
//! `--b-fitted`, `--policy` and `--b-power-up` are as for replay.
//!
//! The protocol is ASCII, one message per line (LF or CR LF), at most `IPMC_EMU_VECTOR_LINE_MAX`
//! characters. Frame bytes are hex, one or two digits each, separated by spaces, read by the one
//! vector reader (`vectors::parse_bus`, `vectors::frame_bytes`).
//!
//! | Direction | Line | Meaning |
//! |---|---|---|
//! | client to emulator | `A <hex bytes>` / `B <hex bytes>` | a frame the Chassis Manager writes on that bus; the first byte is the 8-bit destination address as in the vector format (a Broadcast Get Device ID starts with the general-call byte `00`) |
//! | client to emulator | `#bus A down` / `#bus A up` (and B) | break or repair a bus: frames to and from it are lost and the IPMC's transmissions on it fail (`on_tx_result(false)`, so the Link resends on the other bus, Rule 4.1.1-7) |
//! | client to emulator | `#reset` | power-cycle the board (both buses up again) |
//! | client to emulator | `#quit` | end the session; the server waits for the next client |
//! | emulator to client | `A <hex>` / `B <hex>` | a frame the IPMC transmits, with its bus |
//! | emulator to client | `#ready <hex>` | once at connect and after `#reset`: the IPMC's own IPMB address (8-bit form) |
//! | emulator to client | `# <text>` | a trace line, only with `--trace` |
//! | emulator to client | `#error <line text>: <reason>` | the client line was rejected; the session continues |
//!
//! A frame the Link drops (checksum, wrong address, disabled bus) is not an error line; it is
//! counted by the Link and shown as a trace line.
#![forbid(unsafe_code)]

pub mod emulator;
pub mod replay;
pub mod serve;
pub mod trace;
pub mod vectors;
