//! serve.rs - the emulator in real time behind a TCP line protocol (`ipmc-emu serve`).
//!
//! `Board` is the protocol logic over an `Emulator` with an explicit clock, so it is tested
//! without sockets. `run` is the loop: accept one client at a time on 127.0.0.1, build a fresh
//! `Board` for it (a power cycle), read lines, step the board every POLL_PERIOD and write the
//! frames the IPMC transmits. The line protocol is documented in the crate documentation.

use crate::emulator::{EmuConfig, EmuError, Emulator};
use crate::replay::is_request_frame;
use crate::trace::Hex;
use crate::vectors::{self, VectorErrorKind, LINE_MAX_CHARS};
use core::fmt::Arguments;
use ipmc_core::clock::at_or_after;
use ipmc_core::queue::Bus;
use ipmc_core::trace::Trace;
use ipmc_fru::RamFlash;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// POLL_PERIOD -- main-loop period: one `Board::step` per period (also the socket read timeout
/// that paces the loop), from defs.rs IPMC_EMU_SERVE_POLL_MS.
const POLL_PERIOD: Duration = Duration::from_millis(ipmc_defs::IPMC_EMU_SERVE_POLL_MS as u64);
/// READ_CHUNK_BYTES -- socket read size: one longest line (any size works; the splitter joins
/// the pieces).
const READ_CHUNK_BYTES: usize = LINE_MAX_CHARS;
/// TX_DONE_DELAY_MS -- time from the start to the end of one transmission, from defs.rs
/// IPMC_EMU_TX_DONE_DELAY_MS (the replay uses the same value).
const TX_DONE_DELAY_MS: u32 = ipmc_defs::IPMC_EMU_TX_DONE_DELAY_MS;
/// ERROR_ECHO_CHARS -- most characters of a rejected line echoed in its `#error` reply, from
/// defs.rs IPMC_EMU_ERROR_ECHO_MAX.
const ERROR_ECHO_CHARS: usize = ipmc_defs::IPMC_EMU_ERROR_ECHO_MAX;

/// ServeConfig -- what `serve` runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeConfig {
    /// The board to emulate.
    pub emu: EmuConfig,
    /// TCP port on 127.0.0.1; 0 asks the system for a free port.
    pub port: u16,
    /// Send the trace lines to the client as `# text`.
    pub trace: bool,
}

impl ServeConfig {
    /// from_defs -- the defs.rs values (IPMC_EMU_PORT) with tracing off.
    pub const fn from_defs() -> ServeConfig {
        ServeConfig {
            emu: EmuConfig::from_defs(),
            // Guarded 1024..=65535 in ipmc-defs.
            port: ipmc_defs::IPMC_EMU_PORT,
            trace: false,
        }
    }
}

/// ClientMsg -- one parsed line from the Chassis Manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMsg {
    /// A frame written on a bus, destination address byte first.
    Frame(Bus, Vec<u8>),
    /// `#bus A|B down|up`: break (true) or repair (false) a bus.
    BusDown(Bus, bool),
    /// `#reset`: power-cycle the board.
    Reset,
    /// `#quit`: end the session.
    Quit,
}

/// parse_client_line -- one client line to a message; Err(reason) for the `#error` reply.
/// A blank line is Ok(None). Frames use the one vector reader (`vectors::parse_bus`,
/// `vectors::frame_bytes`).
pub fn parse_client_line(line: &str) -> Result<Option<ClientMsg>, String> {
    let text = line.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut fields = text.split_whitespace();
    let first = fields.next().unwrap_or_default();
    if let Some(bus) = vectors::parse_bus(first) {
        let bytes = vectors::frame_bytes(fields).map_err(|kind| kind.name().to_string())?;
        return Ok(Some(ClientMsg::Frame(bus, bytes)));
    }
    let rest: Vec<&str> = fields.collect();
    match (first, rest.as_slice()) {
        ("#bus", [tag, state]) => {
            let bus = vectors::parse_bus(tag).ok_or(VectorErrorKind::BadBus.name())?;
            match *state {
                "down" => Ok(Some(ClientMsg::BusDown(bus, true))),
                "up" => Ok(Some(ClientMsg::BusDown(bus, false))),
                _ => Err("bus state is not down or up".to_string()),
            }
        }
        ("#bus", _) => Err("usage: #bus A|B down|up".to_string()),
        ("#reset", []) => Ok(Some(ClientMsg::Reset)),
        ("#quit", []) => Ok(Some(ClientMsg::Quit)),
        _ => Err("unknown message".to_string()),
    }
}

/// is_request_line -- true when the protocol line `A <hex>` / `B <hex>` is a request the IPMC
/// originated (Tier 2 and 3: a Platform Event to the Event Receiver), told from a response by the
/// one classification `replay::is_request_frame`. False for responses and for `#` lines.
pub fn is_request_line(line: &str) -> bool {
    matches!(
        parse_client_line(line),
        Ok(Some(ClientMsg::Frame(_, bytes))) if is_request_frame(&bytes)
    )
}

/// LineTrace -- collects the emulator's trace lines for the client; discards them when off.
struct LineTrace {
    on: bool,
    lines: Vec<String>,
}

impl Trace for LineTrace {
    fn line(&mut self, args: Arguments) {
        if self.on {
            self.lines.push(format!("# {args}"));
        }
    }
}

/// Inflight -- the transmission handed to the "master driver", done at `done_ms`.
struct Inflight {
    bus: Bus,
    bytes: Vec<u8>,
    done_ms: u32,
}

/// Board -- the emulated IPMC with its simulated buses.
pub struct Board {
    emu: Emulator,
    down: [bool; 2],
    inflight: Option<Inflight>,
    trace: LineTrace,
}

impl Board {
    /// new -- a board in its start-up state for `config`, both buses up, its flash erased.
    pub fn new(config: &ServeConfig) -> Result<Board, EmuError> {
        Board::new_with_flash(config, RamFlash::new())
    }

    /// new_with_flash -- `new` for a board whose flash log holds `flash`.
    pub fn new_with_flash(config: &ServeConfig, flash: RamFlash) -> Result<Board, EmuError> {
        Ok(Board {
            emu: Emulator::new_with_flash(config.emu, flash)?,
            down: [false; 2],
            inflight: None,
            trace: LineTrace {
                on: config.trace,
                lines: Vec::new(),
            },
        })
    }

    /// own_address -- the IPMB address (8-bit form) the board answers at.
    pub fn own_address(&self) -> u8 {
        self.emu.own_address()
    }

    /// flash -- the flash log a power cycle keeps (`Emulator::flash`).
    pub fn flash(&mut self) -> RamFlash {
        self.emu.flash()
    }

    fn is_down(&self, bus: Bus) -> bool {
        self.down[bus.index()]
    }

    /// handle -- applies one message at `now_ms`. Returns false after `#quit`. `Reset` needs a
    /// new `Board` and is the caller's job (`handle_line`); here it changes nothing.
    pub fn handle(&mut self, msg: ClientMsg, now_ms: u32) -> bool {
        match msg {
            ClientMsg::Frame(bus, bytes) => {
                if self.is_down(bus) {
                    self.trace.line(format_args!(
                        "IPMB-{} down, frame lost: {}",
                        bus.name(),
                        Hex(&bytes)
                    ));
                } else if let Err(drop) = self.emu.rx(bus, &bytes, now_ms) {
                    self.trace.line(format_args!("rx dropped: {}", drop.name()));
                }
            }
            ClientMsg::BusDown(bus, down) => {
                self.down[bus.index()] = down;
                self.trace.line(format_args!(
                    "IPMB-{} {}",
                    bus.name(),
                    if down { "down" } else { "up" }
                ));
            }
            ClientMsg::Reset => {}
            ClientMsg::Quit => return false,
        }
        true
    }

    /// step -- one main-loop step at `now_ms`: the emulator's poll, then the transmission state:
    /// a frame handed out by the link is "on the wire" for TX_DONE_DELAY_MS, then reported done
    /// (failed when its bus is down, which makes the link resend on the other bus). Appends the
    /// protocol lines to send (`A <hex>` / `B <hex>` for frames that left, `# text` for traces).
    pub fn step(&mut self, now_ms: u32, out: &mut Vec<String>) {
        self.emu.poll(now_ms, &mut self.trace);
        if self.inflight.is_none() {
            if let Some((bus, bytes)) = self.emu.next_tx() {
                self.inflight = Some(Inflight {
                    bus,
                    bytes: bytes.to_vec(),
                    done_ms: now_ms.wrapping_add(TX_DONE_DELAY_MS),
                });
            }
        }
        if self
            .inflight
            .as_ref()
            .is_some_and(|tx| at_or_after(now_ms, tx.done_ms))
        {
            if let Some(tx) = self.inflight.take() {
                let ok = !self.is_down(tx.bus);
                if ok {
                    out.push(format!("{} {}", tx.bus.name(), Hex(&tx.bytes)));
                }
                if let Err(e) = self.emu.tx_done(tx.bus, ok, now_ms, &mut self.trace) {
                    self.trace.line(format_args!("tx_done: {}", e.name()));
                }
            }
        }
        out.append(&mut self.trace.lines);
    }
}

/// bind -- listens on 127.0.0.1:`port` (0 picks a free port).
pub fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

/// run -- serves clients one after the other, forever. Prints `listening 127.0.0.1:<port>` first.
/// Err only when the listener fails.
pub fn run(listener: &TcpListener, config: &ServeConfig) -> io::Result<()> {
    let addr = listener.local_addr()?;
    println!("listening {addr}");
    io::stdout().flush()?;
    loop {
        let (stream, peer) = listener.accept()?;
        if let Err(e) = serve_client(stream, config) {
            eprintln!("client {peer}: {e}");
        }
    }
}

/// LineSplitter -- bytes in, complete lines out; a line over LINE_MAX_CHARS is dropped whole and
/// reported once as `Err(())`.
#[derive(Default)]
struct LineSplitter {
    buf: Vec<u8>,
    discarding: bool,
}

impl LineSplitter {
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<String, ()>> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if self.discarding {
                    lines.push(Err(()));
                } else {
                    lines.push(Ok(String::from_utf8_lossy(&self.buf).into_owned()));
                }
                self.buf.clear();
                self.discarding = false;
            } else if !self.discarding {
                if self.buf.len() >= LINE_MAX_CHARS {
                    self.discarding = true;
                    self.buf.clear();
                } else {
                    self.buf.push(byte);
                }
            }
        }
        lines
    }
}

/// serve_client -- one session: `#ready`, then read lines and step the board until `#quit` or the
/// client closes. A write error means the client is gone.
fn serve_client(mut stream: TcpStream, config: &ServeConfig) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(POLL_PERIOD))?;
    let mut start = Instant::now();
    let mut board = Board::new(config).map_err(io::Error::other)?;
    let mut out = vec![ready_line(&board)];
    let mut splitter = LineSplitter::default();
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let read = stream.read(&mut chunk);
        // Taken after the read (which waits up to POLL_PERIOD), so a frame is stamped with its
        // arrival time. Wraps after about 49 days; every compare uses wrapping arithmetic.
        let mut now_ms = start.elapsed().as_millis() as u32;
        match read {
            Ok(0) => return Ok(()),
            Ok(n) => {
                for line in splitter.push(&chunk[..n]) {
                    let keep = match line {
                        Ok(text) => handle_line(
                            &mut board,
                            config,
                            &text,
                            &mut Clock {
                                start: &mut start,
                                now_ms: &mut now_ms,
                            },
                            &mut out,
                        ),
                        Err(()) => {
                            out.push("#error line too long".to_string());
                            true
                        }
                    };
                    if !keep {
                        flush(&mut stream, &mut out)?;
                        return Ok(());
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
        board.step(now_ms, &mut out);
        flush(&mut stream, &mut out)?;
    }
}

/// Clock -- the session's time origin and the current time in milliseconds since it. A reset
/// moves the origin to the reset, like a power cycle: the new board's clock starts at zero.
struct Clock<'a> {
    start: &'a mut Instant,
    now_ms: &'a mut u32,
}

/// handle_line -- one client line: errors answer `#error`, `#reset` rebuilds the board (its flash
/// log carried over, as a power cycle keeps it) and restarts the clock at zero. Returns false
/// for `#quit`.
fn handle_line(
    board: &mut Board,
    config: &ServeConfig,
    text: &str,
    clock: &mut Clock,
    out: &mut Vec<String>,
) -> bool {
    match parse_client_line(text) {
        Ok(None) => true,
        Ok(Some(ClientMsg::Reset)) => {
            match Board::new_with_flash(config, board.flash()) {
                Ok(fresh) => {
                    *board = fresh;
                    *clock.start = Instant::now();
                    *clock.now_ms = 0;
                    out.push(ready_line(board));
                }
                Err(e) => out.push(format!("#error {}: {e}", echo(text))),
            }
            true
        }
        Ok(Some(msg)) => board.handle(msg, *clock.now_ms),
        Err(reason) => {
            out.push(format!("#error {}: {reason}", echo(text)));
            true
        }
    }
}

fn ready_line(board: &Board) -> String {
    format!("#ready {:02X}", board.own_address())
}

/// echo -- the rejected line, trimmed and cut for the `#error` reply.
fn echo(text: &str) -> String {
    text.trim().chars().take(ERROR_ECHO_CHARS).collect()
}

/// flush -- writes the pending lines, one per line, and empties `out`.
fn flush(stream: &mut TcpStream, out: &mut Vec<String>) -> io::Result<()> {
    if out.is_empty() {
        return Ok(());
    }
    let mut text = out.join("\n");
    text.push('\n');
    out.clear();
    stream.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipmc_core::ipmb;

    fn config(b_fitted: bool) -> ServeConfig {
        let mut config = ServeConfig::from_defs();
        config.emu.b_fitted = b_fitted;
        config
    }

    /// request_line -- a `A`/`B` client line for a request to 82h from 20h, built with the
    /// ipmc-core codec (checksums included).
    fn request_line(bus: &str, seq: u8, netfn: u8, cmd: u8, data: &[u8]) -> String {
        let header = ipmb::Header {
            dest: 0x82,
            netfn,
            dest_lun: 0,
            src: 0x20,
            seq,
            src_lun: 0,
            cmd,
        };
        let mut frame = [0u8; ipmb::IPMB_MAX_LEN];
        let len = ipmb::encode_request(&header, data, &mut frame).expect("encode request");
        format!("{bus} {}", Hex(&frame[..len]))
    }

    /// frames_out -- the `A`/`B` response lines of an output batch (the IPMC's own requests, sent
    /// at Tier 2 and 3, are no responses).
    fn frames_out(out: &[String]) -> Vec<&String> {
        out.iter()
            .filter(|l| !l.starts_with('#') && !is_request_line(l))
            .collect()
    }

    /// run_ms -- steps the board once per millisecond from `from` to `to` (exclusive).
    fn run_ms(board: &mut Board, from: u32, to: u32) -> Vec<String> {
        let mut out = Vec::new();
        for t in from..to {
            board.step(t, &mut out);
        }
        out
    }

    /// send -- hands one client line to the board at `now_ms`.
    fn send(board: &mut Board, line: &str, now_ms: u32) {
        let msg = parse_client_line(line).expect("parses").expect("not blank");
        board.handle(msg, now_ms);
    }

    #[test]
    fn client_lines_parse_through_the_one_reader() {
        let cases: [(&str, Result<Option<ClientMsg>, &str>); 11] = [
            ("", Ok(None)),
            (
                "A 82 18",
                Ok(Some(ClientMsg::Frame(Bus::A, vec![0x82, 0x18]))),
            ),
            (
                "B 0 1 FF",
                Ok(Some(ClientMsg::Frame(Bus::B, vec![0, 1, 0xFF]))),
            ),
            ("#bus A down", Ok(Some(ClientMsg::BusDown(Bus::A, true)))),
            ("#bus B up", Ok(Some(ClientMsg::BusDown(Bus::B, false)))),
            ("#reset", Ok(Some(ClientMsg::Reset))),
            ("#quit", Ok(Some(ClientMsg::Quit))),
            ("A", Err("no frame bytes")),
            ("A 82 XY", Err("bad hex byte")),
            ("#bus C down", Err("bus tag is not A or B")),
            ("hello", Err("unknown message")),
        ];
        for (line, want) in cases {
            assert_eq!(
                parse_client_line(line),
                want.map_err(str::to_string),
                "line {line:?}"
            );
        }
    }

    #[test]
    fn a_down_bus_loses_requests_and_the_other_bus_still_answers() {
        let mut board = Board::new(&config(true)).expect("board");
        assert_eq!(ready_line(&board), "#ready 82");
        send(&mut board, &request_line("A", 9, 0x06, 0x01, &[]), 0);
        let out = run_ms(&mut board, 0, 10);
        let frames = frames_out(&out);
        assert_eq!(frames.len(), 1, "{out:?}");
        assert!(frames[0].starts_with("A 20 1C "), "{}", frames[0]);

        // Set IPMB State: IPMB-A no change (FFh), IPMB-B local control (01h), VSO id 03h.
        send(
            &mut board,
            &request_line("A", 1, 0x2C, 0x09, &[0x03, 0xFF, 0x01]),
            10,
        );
        let out = run_ms(&mut board, 10, 20);
        assert_eq!(frames_out(&out).len(), 1, "{out:?}");

        send(&mut board, "#bus A down", 20);
        send(&mut board, &request_line("A", 2, 0x06, 0x01, &[]), 21);
        let out = run_ms(&mut board, 21, 40);
        assert!(frames_out(&out).is_empty(), "A is down: {out:?}");

        send(&mut board, &request_line("B", 3, 0x06, 0x01, &[]), 40);
        let out = run_ms(&mut board, 40, 60);
        let frames = frames_out(&out);
        assert_eq!(frames.len(), 1, "{out:?}");
        assert!(
            frames[0].starts_with("B 20 "),
            "response on B: {}",
            frames[0]
        );
    }

    #[test]
    fn the_line_splitter_drops_an_overlong_line_whole() {
        let mut splitter = LineSplitter::default();
        let mut long = vec![b'A'; LINE_MAX_CHARS + 10];
        long.push(b'\n');
        long.extend_from_slice(b"#quit\n");
        let lines = splitter.push(&long);
        assert_eq!(lines, vec![Err(()), Ok("#quit".to_string())]);
        let mut parts = splitter.push(b"A 82");
        parts.extend(splitter.push(b" 18\r\n"));
        assert_eq!(parts, vec![Ok("A 82 18\r".to_string())]);
    }

    #[test]
    fn reset_restarts_the_clock_at_zero_like_a_power_cycle() {
        let config = config(true);
        let mut board = Board::new(&config).expect("board");
        // A session that has run for a long time (past any start-up timeout).
        let old_ms = ipmc_defs::IPMC_PGOOD_TIMEOUT_MS + 5000;
        let mut start = Instant::now();
        if let Some(earlier) = start.checked_sub(Duration::from_millis(u64::from(old_ms))) {
            start = earlier;
        }
        let mut now_ms = old_ms;
        let before = start;
        let mut out = Vec::new();
        let keep = handle_line(
            &mut board,
            &config,
            "#reset",
            &mut Clock {
                start: &mut start,
                now_ms: &mut now_ms,
            },
            &mut out,
        );
        assert!(keep);
        assert_eq!(out, vec!["#ready 82".to_string()]);
        assert_eq!(now_ms, 0, "the new board's clock starts at the reset");
        assert!(start > before, "the time origin moved to the reset");
        // The next steps are stamped from the reset: a fresh board waits its whole start-up
        // interval instead of finding it already expired.
        let mut out = Vec::new();
        board.step(now_ms, &mut out);
        assert!(out.iter().all(|l| !l.contains("timeout")), "{out:?}");

        // Other messages leave the clock alone.
        let mut now_ms = 1234;
        let mut start2 = Instant::now();
        let mut out = Vec::new();
        handle_line(
            &mut board,
            &config,
            "#bus A down",
            &mut Clock {
                start: &mut start2,
                now_ms: &mut now_ms,
            },
            &mut out,
        );
        assert_eq!(now_ms, 1234);
    }
}
