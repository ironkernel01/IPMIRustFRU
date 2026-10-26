//! session - the command session on the chosen UART (design 8) as a pure state machine over the
//! `BootHw` trait: one complete line in, the answer text out, the caller performs `Run` and
//! `Reset` after it has flushed the port. The line editor, the echo, the UART and the start
//! sequence are the firmware's.

use crate::commands::{find, put_help, words, Cmd};
use crate::decision::Decision;
use crate::error::{put_err, put_err_head, put_err_word, BootError};
use crate::geometry::Target;
use crate::header::{check, ImageError, Report};
use crate::load::{erase, FlashOps, LoadStep, Loader};
use crate::request::Interface;
use crate::text::{put_dec, TextOut, CRLF};
use crate::trace;
use ipmc_defs::{FIELD_BUILD, IPMC_BOOT_PROMPT, IPMC_FWU_LOAD_IDLE_S};

/// MS_PER_S -- milliseconds per second.
const MS_PER_S: u32 = 1000;
/// ERASE_PROMPT_APP, ERASE_PROMPT_FRU -- the question before `CONFIRM`.
const ERASE_PROMPT_APP: &str = "Erase sectors 2-7 (runtime firmware)? Type CONFIRM";
const ERASE_PROMPT_FRU: &str = "Erase sector 1 (FRU flash store)? Type CONFIRM";

/// BootHw -- what the session needs from the board: the flash (erase, program, read) and a
/// millisecond clock that keeps counting through a flash stall (SysTick with the stall catch-up
/// in the firmware, a counter in the tests).
pub trait BootHw: FlashOps {
    fn now_ms(&self) -> u32;
    /// activity -- one record was accepted in `LOAD` (the firmware retriggers the LED).
    fn activity(&self);
}

/// StartRecord -- what `INFO` prints of this reset; the firmware fills it at start-up.
pub struct StartRecord<'a> {
    /// The bootloader version text (`0.1.0`), as the banner prints it.
    pub version: &'a str,
    pub reset_cause: &'a str,
    pub pin_high: bool,
    pub request: Option<Interface>,
    pub decision: Decision,
    /// The session port: its name (`CLI` or `APP`), baud rate and RTS/CTS use.
    pub port_name: &'static str,
    pub baud: u32,
    pub flow: bool,
    /// APP_UART is enabled (the decision line names one or both ports).
    pub app_enabled: bool,
}

/// Action -- what the caller does after the answer has left the port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Start the runtime (the image check passed).
    Run,
    /// Restart the processor.
    Reset,
}

/// Mode -- command lines, or Intel HEX records of `LOAD`.
#[derive(Clone, Copy)]
enum Mode {
    Command,
    Record(Loader),
}

/// Session -- the state of one session: a pending erase, what was erased, the mode and the time
/// of the last line (the `LOAD` idle timeout).
pub struct Session {
    pending: Option<Target>,
    erased_fru: bool,
    erased_app: bool,
    mode: Mode,
    last_line_ms: u32,
}

/// is_wakeup -- the port-choice rule for a line before the session exists: the one token is
/// `WAKEUP` in any letter case.
pub fn is_wakeup(line: &str) -> bool {
    find(line) == Ok(Cmd::Wakeup)
}

/// ms_since -- milliseconds from `start` to now (wrapping).
fn ms_since(hw: &dyn BootHw, start: u32) -> u32 {
    hw.now_ms().wrapping_sub(start)
}

/// put_prompt -- the prompt, no line end.
fn put_prompt(out: &mut dyn TextOut) {
    out.put(IPMC_BOOT_PROMPT);
}

/// put_ms -- ` <ms> ms` and CR LF: the end of a status line that reports a time.
fn put_ms(out: &mut dyn TextOut, ms: u32) {
    out.put(" ");
    put_dec(out, ms);
    out.put(" ms");
    out.put(CRLF);
}

/// ok -- the status line `OK`.
fn ok(out: &mut dyn TextOut) {
    out.put("OK");
    out.put(CRLF);
}

/// put_image_error -- `ERR IMAGE <reason> computed <64 hex|none> stored <64 hex|none>`.
fn put_image_error(out: &mut dyn TextOut, reason: ImageError, report: &Report) {
    put_err_head(out, BootError::Image(reason));
    out.put(" computed ");
    trace::digest_or_none(out, report.computed);
    out.put(" stored ");
    trace::digest_or_none(out, report.header.map(|header| header.digest));
    out.put(CRLF);
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub const fn new() -> Self {
        Session {
            pending: None,
            erased_fru: false,
            erased_app: false,
            mode: Mode::Command,
            last_line_ms: 0,
        }
    }

    /// echo -- true in command mode (the caller echoes the editor's Echo and Backspace events).
    pub fn echo(&self) -> bool {
        matches!(self.mode, Mode::Command)
    }

    /// open -- the WAKEUP answer: `OK` and the prompt.
    pub fn open(&mut self, out: &mut dyn TextOut) {
        ok(out);
        put_prompt(out);
    }

    /// line_too_long -- the editor dropped a line: `ERR LINE_TOO_LONG`. Record mode stays; command
    /// mode cancels a pending erase and prints the prompt.
    pub fn line_too_long(&mut self, out: &mut dyn TextOut) {
        put_err(out, BootError::LineTooLong);
        if matches!(self.mode, Mode::Command) {
            self.pending = None;
            put_prompt(out);
        }
    }

    /// poll -- the `LOAD` idle timeout: no line for `IPMC_FWU_LOAD_IDLE_S` seconds ends record
    /// mode with `ERR TIMEOUT` and the prompt. Call every main-loop pass.
    pub fn poll(&mut self, hw: &dyn BootHw, out: &mut dyn TextOut) {
        let limit = IPMC_FWU_LOAD_IDLE_S.saturating_mul(MS_PER_S);
        if matches!(self.mode, Mode::Record(_)) && ms_since(hw, self.last_line_ms) >= limit {
            put_err(out, BootError::Timeout);
            self.leave_record(out);
        }
    }

    /// leave_record -- back to command mode with the prompt.
    fn leave_record(&mut self, out: &mut dyn TextOut) {
        self.mode = Mode::Command;
        put_prompt(out);
    }

    /// line -- one complete line in; the answer goes to `out`; the caller performs Run and Reset
    /// after flushing the port. Prints the prompt where the protocol says.
    pub fn line(
        &mut self,
        line: &str,
        hw: &mut dyn BootHw,
        start: &StartRecord,
        out: &mut dyn TextOut,
    ) -> Action {
        self.last_line_ms = hw.now_ms();
        match self.mode {
            Mode::Record(_) => {
                self.record_line(line, hw, out);
                Action::None
            }
            Mode::Command => self.command_line(line, hw, start, out),
        }
    }

    /// record_line -- one line in `LOAD` record mode (design 8.4).
    fn record_line(&mut self, line: &str, hw: &mut dyn BootHw, out: &mut dyn TextOut) {
        let Mode::Record(loader) = &mut self.mode else {
            return;
        };
        if words(line).next().is_none() {
            return; // an empty line is ignored
        }
        if !line.starts_with(':') {
            put_err(out, BootError::Aborted);
            self.leave_record(out);
            return;
        }
        match loader.record(line, hw) {
            Ok(LoadStep::Record) => {
                hw.activity();
                ok(out);
            }
            Ok(LoadStep::End { records, bytes }) => {
                match check(hw).result {
                    Ok(()) => {
                        out.put("OK END ");
                        put_dec(out, records);
                        out.put(" records ");
                        put_dec(out, bytes);
                        out.put(" bytes, image valid");
                        out.put(CRLF);
                    }
                    Err(reason) => put_err(out, BootError::Image(reason)),
                }
                self.leave_record(out);
            }
            Err(error) => put_err(out, error),
        }
    }

    /// command_line -- one line in command mode: an armed erase takes `CONFIRM` or is cancelled
    /// (and the line is not run); otherwise the command of the line.
    fn command_line(
        &mut self,
        line: &str,
        hw: &mut dyn BootHw,
        start: &StartRecord,
        out: &mut dyn TextOut,
    ) -> Action {
        let Some(first) = words(line).next() else {
            put_prompt(out);
            return Action::None;
        };
        match (self.pending.take(), find(line)) {
            (Some(target), Ok(Cmd::Confirm)) => self.run_erase(target, hw, out),
            (Some(_), _) => put_err(out, BootError::Cancelled),
            (None, Ok(cmd)) => return self.run_command(cmd, hw, start, out),
            (None, Err(error)) => put_err_word(out, error, first),
        }
        put_prompt(out);
        Action::None
    }

    /// run_erase -- the pending erase, timed: `OK ERASED <APP|FRU> <ms> ms` or the error. A failed
    /// erase clears the "erased" mark of its target: the sectors are in an unknown state.
    fn run_erase(&mut self, target: Target, hw: &mut dyn BootHw, out: &mut dyn TextOut) {
        let started = hw.now_ms();
        let result = erase(target, hw);
        let done = match target {
            Target::Fru => &mut self.erased_fru,
            Target::App => &mut self.erased_app,
        };
        *done = result.is_ok();
        match result {
            Ok(()) => {
                out.put("OK ERASED ");
                out.put(target.name());
                put_ms(out, ms_since(hw, started));
            }
            Err(error) => put_err(out, error),
        }
    }

    /// run_command -- the answer of one command, then the prompt unless the command leaves the
    /// bootloader or starts record mode.
    fn run_command(
        &mut self,
        cmd: Cmd,
        hw: &mut dyn BootHw,
        start: &StartRecord,
        out: &mut dyn TextOut,
    ) -> Action {
        let mut action = Action::None;
        let mut prompt = true;
        match cmd {
            Cmd::Wakeup => ok(out),
            Cmd::Help => {
                put_help(out);
                ok(out);
            }
            Cmd::Info => self.info(hw, start, out),
            Cmd::EraseApp | Cmd::EraseFru => {
                let (target, question) = if cmd == Cmd::EraseApp {
                    (Target::App, ERASE_PROMPT_APP)
                } else {
                    (Target::Fru, ERASE_PROMPT_FRU)
                };
                out.put(question);
                out.put(CRLF);
                out.put("OK PENDING");
                out.put(CRLF);
                self.pending = Some(target);
            }
            Cmd::Confirm => put_err(out, BootError::NothingPending),
            Cmd::Load => {
                if self.erased_app {
                    out.put("OK LOAD APP: send records");
                    out.put(CRLF);
                    self.mode = Mode::Record(Loader::new());
                    prompt = false;
                } else {
                    put_err(out, BootError::NotErasedFirst);
                }
            }
            Cmd::Verify => {
                let started = hw.now_ms();
                let report = check(hw);
                match report.result {
                    Ok(()) => {
                        out.put("OK VERIFY valid SHA-256 ");
                        trace::digest_or_none(out, report.computed);
                        put_ms(out, ms_since(hw, started));
                    }
                    Err(reason) => put_image_error(out, reason, &report),
                }
            }
            Cmd::Run => match check(hw).result {
                Ok(()) => {
                    out.put("OK RUN");
                    out.put(CRLF);
                    action = Action::Run;
                    prompt = false;
                }
                Err(reason) => put_err(out, BootError::Image(reason)),
            },
            Cmd::Reset => {
                out.put("OK RESET");
                out.put(CRLF);
                action = Action::Reset;
                prompt = false;
            }
        }
        if prompt {
            put_prompt(out);
        }
        action
    }

    /// info -- the start-up record with a fresh image check, then the bootloader's own lines.
    fn info(&self, hw: &mut dyn BootHw, start: &StartRecord, out: &mut dyn TextOut) {
        trace::banner(out, start.version, FIELD_BUILD);
        trace::reset_cause(out, start.reset_cause);
        trace::pin(out, start.pin_high);
        trace::request(out, start.request);
        let started = hw.now_ms();
        let report = check(hw);
        let ms = ms_since(hw, started);
        trace::runtime_image(out, &report);
        trace::image_check_full(out, &report, ms);
        trace::stored_digest(out, &report);
        trace::decision(out, &start.decision, start.app_enabled);
        trace::bootloader(out, start.version);
        trace::build(out, FIELD_BUILD);
        trace::port(out, start.port_name, start.baud, start.flow);
        trace::erased(out, self.erased_fru, self.erased_app);
        ok(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::put_help;
    use crate::hex::tests::record_line;
    use crate::testflash::{good_image, hex_image, Ram};
    use ipmc_fru::persist::FlashError;
    use std::format;
    use std::string::{String, ToString};

    const PROMPT: &str = "BOOT> ";

    fn start() -> StartRecord<'static> {
        StartRecord {
            version: "0.1.0",
            reset_cause: "power-on",
            pin_high: false,
            request: None,
            decision: Decision::Stay(crate::decision::Reason::NoImage),
            port_name: "CLI",
            baud: 115_200,
            flow: false,
            app_enabled: true,
        }
    }

    /// Bench -- a session over a flash model; `send` returns the answer text and the action.
    struct Bench {
        session: Session,
        ram: Ram,
    }

    impl Bench {
        fn new(ram: Ram) -> Self {
            Bench {
                session: Session::new(),
                ram,
            }
        }

        fn send(&mut self, line: &str) -> (String, Action) {
            let mut out = String::new();
            let action = self.session.line(line, &mut self.ram, &start(), &mut out);
            (out, action)
        }

        fn text(&mut self, line: &str) -> String {
            self.send(line).0
        }

        /// erase_app -- ERASE APP and CONFIRM, which must both succeed.
        fn erase_app(&mut self) {
            self.text("ERASE APP");
            assert!(self.text("CONFIRM").starts_with("OK ERASED APP"));
        }

        /// load_image -- LOAD and every record of `image`; returns the answer to the last one.
        fn load_image(&mut self, image: &[u8], size: usize) -> String {
            assert!(self.text("LOAD").starts_with("OK LOAD APP"));
            let mut last = String::new();
            for line in hex_image(image, size) {
                last = self.text(&line);
            }
            last
        }

        fn poll(&mut self) -> String {
            let mut out = String::new();
            self.session.poll(&self.ram, &mut out);
            out
        }
    }

    fn crlf(lines: &[&str]) -> String {
        lines.iter().map(|line| format!("{line}\r\n")).collect()
    }

    /// digests -- the computed and the stored digest of the flash as 64 hex digits or `none`.
    fn digests(ram: &Ram) -> (String, String) {
        let report = check(ram);
        let (mut computed, mut stored) = (String::new(), String::new());
        trace::digest_or_none(&mut computed, report.computed);
        trace::digest_or_none(&mut stored, report.header.map(|header| header.digest));
        (computed, stored)
    }

    #[test]
    fn answers_of_the_small_commands() {
        let mut help = String::new();
        put_help(&mut help);
        let rows: [(&str, String); 10] = [
            ("WAKEUP", format!("OK\r\n{PROMPT}")),
            ("wakeup", format!("OK\r\n{PROMPT}")),
            ("HELP", format!("{help}OK\r\n{PROMPT}")),
            ("?", format!("{help}OK\r\n{PROMPT}")),
            ("", PROMPT.to_string()),
            ("   ", PROMPT.to_string()),
            ("FOO bar", format!("ERR UNKNOWN FOO\r\n{PROMPT}")),
            ("RUN NOW", format!("ERR ARGS RUN\r\n{PROMPT}")),
            ("erase", format!("ERR ARGS erase\r\n{PROMPT}")),
            ("CONFIRM", format!("ERR NOTHING_PENDING\r\n{PROMPT}")),
        ];
        for (line, want) in rows {
            let mut bench = Bench::new(Ram::new());
            assert_eq!(bench.send(line), (want, Action::None), "{line:?}");
        }
    }

    #[test]
    fn open_answers_ok_and_the_prompt() {
        let mut out = String::new();
        Session::new().open(&mut out);
        assert_eq!(out, format!("OK\r\n{PROMPT}"));
    }

    #[test]
    fn is_wakeup_rows() {
        for (line, want) in [
            ("WAKEUP", true),
            ("wakeup", true),
            ("  WakeUp   ", true),
            ("", false),
            ("WAKEUP now", false),
            ("WAKEUPS", false),
            ("HELP", false),
            ("OK", false),
        ] {
            assert_eq!(is_wakeup(line), want, "{line:?}");
        }
    }

    #[test]
    fn erase_then_confirm_runs_the_erase() {
        let rows: [(&str, &str, &str, &[u8]); 2] = [
            (
                "ERASE APP",
                "Erase sectors 2-7 (runtime firmware)? Type CONFIRM",
                "APP",
                &[2, 3, 4, 5, 6, 7],
            ),
            (
                "erase fru",
                "Erase sector 1 (FRU flash store)? Type CONFIRM",
                "FRU",
                &[1],
            ),
        ];
        for (line, question, name, sectors) in rows {
            let mut bench = Bench::new(Ram::new());
            bench.ram.step = 7;
            assert_eq!(
                bench.text(line),
                format!("{question}\r\nOK PENDING\r\n{PROMPT}")
            );
            assert!(bench.ram.erased.is_empty(), "nothing erased before CONFIRM");
            assert_eq!(
                bench.text("confirm"),
                format!("OK ERASED {name} 7 ms\r\n{PROMPT}")
            );
            assert_eq!(bench.ram.erased, sectors);
            assert_eq!(
                bench.text("CONFIRM"),
                format!("ERR NOTHING_PENDING\r\n{PROMPT}")
            );
        }
    }

    #[test]
    fn another_line_cancels_a_pending_erase_and_is_not_run() {
        for other in ["INFO", "FOO", "ERASE FRU", "RESET", "ERASE APP x"] {
            let mut bench = Bench::new(Ram::new());
            bench.text("ERASE APP");
            assert_eq!(
                bench.send(other),
                (format!("ERR CANCELLED\r\n{PROMPT}"), Action::None),
                "{other}"
            );
            assert_eq!(
                bench.text("CONFIRM"),
                format!("ERR NOTHING_PENDING\r\n{PROMPT}")
            );
            assert!(bench.ram.erased.is_empty());
        }
    }

    #[test]
    fn an_empty_line_keeps_the_erase_pending() {
        let mut bench = Bench::new(Ram::new());
        bench.text("ERASE FRU");
        assert_eq!(bench.text(""), PROMPT);
        assert!(bench.text("CONFIRM").starts_with("OK ERASED FRU"));
    }

    #[test]
    fn a_line_too_long_cancels_a_pending_erase() {
        let mut bench = Bench::new(Ram::new());
        bench.text("ERASE FRU");
        let mut out = String::new();
        bench.session.line_too_long(&mut out);
        assert_eq!(out, format!("ERR LINE_TOO_LONG\r\n{PROMPT}"));
        assert_eq!(
            bench.text("CONFIRM"),
            format!("ERR NOTHING_PENDING\r\n{PROMPT}")
        );
    }

    #[test]
    fn erase_faults_are_reported_and_clear_the_erased_mark() {
        let rows = [
            (FlashError::NvmroAsserted, "ERR NVMRO".to_string()),
            (
                FlashError::Timeout,
                format!("ERR FLASH {}", FlashError::Timeout.name()),
            ),
        ];
        for (fault, want) in rows {
            let mut bench = Bench::new(Ram::new());
            bench.erase_app();
            bench.ram.erase_fault_at = Some(4);
            bench.ram.erase_error = fault;
            bench.text("ERASE APP");
            assert_eq!(bench.text("CONFIRM"), format!("{want}\r\n{PROMPT}"));
            assert_eq!(
                bench.text("LOAD"),
                format!("ERR NOT_ERASED erase APP first\r\n{PROMPT}")
            );
        }
    }

    #[test]
    fn load_needs_an_erase_of_app_in_this_session() {
        let mut bench = Bench::new(Ram::new());
        let refused = format!("ERR NOT_ERASED erase APP first\r\n{PROMPT}");
        assert_eq!(bench.text("LOAD"), refused);
        bench.text("ERASE FRU");
        bench.text("CONFIRM");
        assert_eq!(bench.text("LOAD"), refused);
        assert!(bench.session.echo());
        bench.erase_app();
        assert_eq!(bench.text("LOAD"), "OK LOAD APP: send records\r\n");
        assert!(!bench.session.echo());
    }

    #[test]
    fn a_full_update_loads_verifies_and_runs() {
        let image = good_image();
        let mut bench = Bench::new(Ram::new());
        bench.erase_app();
        let last = bench.load_image(&image, 16);
        let records = image.len().div_ceil(16);
        assert_eq!(
            last,
            format!(
                "OK END {records} records {} bytes, image valid\r\n{PROMPT}",
                image.len()
            )
        );
        assert!(bench.session.echo(), "command mode again");
        assert_eq!(
            bench.ram.activity.get() as usize,
            records + 3,
            "data, two 04 and the 05 record"
        );
        let (hex, _) = digests(&bench.ram);
        bench.ram.step = 3;
        assert_eq!(
            bench.text("VERIFY"),
            format!("OK VERIFY valid SHA-256 {hex} 3 ms\r\n{PROMPT}")
        );
        assert_eq!(bench.send("RUN"), ("OK RUN\r\n".to_string(), Action::Run));
    }

    #[test]
    fn a_wrong_digest_is_refused_at_the_end_record_on_verify_and_on_run() {
        let mut image = good_image();
        image[1000] ^= 0x55;
        let mut bench = Bench::new(Ram::new());
        bench.erase_app();
        let last = bench.load_image(&image, 32);
        assert_eq!(last, format!("ERR IMAGE digest mismatch\r\n{PROMPT}"));
        let (computed, stored) = digests(&bench.ram);
        assert_ne!(computed, stored);
        assert_eq!(
            bench.text("VERIFY"),
            format!("ERR IMAGE digest mismatch computed {computed} stored {stored}\r\n{PROMPT}")
        );
        assert_eq!(
            bench.send("RUN"),
            (
                format!("ERR IMAGE digest mismatch\r\n{PROMPT}"),
                Action::None
            )
        );
    }

    #[test]
    fn verify_and_run_without_an_image() {
        let mut bench = Bench::new(Ram::new());
        assert_eq!(
            bench.text("VERIFY"),
            format!("ERR IMAGE missing computed none stored none\r\n{PROMPT}")
        );
        assert_eq!(
            bench.send("RUN"),
            (format!("ERR IMAGE missing\r\n{PROMPT}"), Action::None)
        );
    }

    #[test]
    fn reset_answers_and_asks_for_the_reset() {
        let mut bench = Bench::new(Ram::new());
        assert_eq!(
            bench.send("reset"),
            ("OK RESET\r\n".to_string(), Action::Reset)
        );
    }

    /// loading -- a bench in record mode after ERASE APP and LOAD, with the 04 record sent.
    fn loading() -> Bench {
        let mut bench = Bench::new(Ram::new());
        bench.erase_app();
        bench.text("LOAD");
        assert_eq!(bench.text(&record_line(0, 0x04, &[8, 0])), "OK\r\n");
        bench
    }

    #[test]
    fn record_mode_lines() {
        let data = record_line(0x8000, 0, &[1, 2, 3, 4]);
        let mut damaged = data.clone();
        damaged.replace_range(9..11, "FF");
        // errors leave record mode on, an empty line is ignored
        let rows: [(&str, &str); 6] = [
            ("", ""),
            ("  ", ""),
            (&damaged, "ERR CHECKSUM\r\n"),
            (":zz", "ERR FORMAT\r\n"),
            (&data, "OK\r\n"),
            (&data, "ERR NOT_ERASED\r\n"),
        ];
        let mut bench = loading();
        for (line, want) in rows {
            assert_eq!(bench.text(line), want, "{line:?}");
            assert!(!bench.session.echo(), "still in record mode after {line:?}");
        }
        let mut out = String::new();
        bench.session.line_too_long(&mut out);
        assert_eq!(out, "ERR LINE_TOO_LONG\r\n");
        assert!(!bench.session.echo());
    }

    #[test]
    fn a_line_without_a_colon_aborts_record_mode_and_is_not_run() {
        for line in ["RESET", "RUN", "x", " :00000001FF"] {
            let mut bench = loading();
            assert_eq!(
                bench.send(line),
                (format!("ERR ABORTED\r\n{PROMPT}"), Action::None),
                "{line:?}"
            );
            assert!(bench.session.echo());
            assert_eq!(bench.text("WAKEUP"), format!("OK\r\n{PROMPT}"));
        }
    }

    #[test]
    fn the_load_idle_timeout() {
        let limit = ipmc_defs::IPMC_FWU_LOAD_IDLE_S * 1000;
        let mut bench = Bench::new(Ram::new());
        assert_eq!(bench.poll(), "", "command mode has no timeout");
        bench.erase_app();
        bench.ram.now.set(1_000);
        bench.text("LOAD");
        for (now, want) in [
            (1_000, ""),
            (1_000 + limit - 1, ""),
            (1_000 + limit, "ERR TIMEOUT\r\nBOOT> "),
            (1_000 + limit + 5, ""),
        ] {
            bench.ram.now.set(now);
            assert_eq!(bench.poll(), want, "{now} ms");
        }
        assert!(bench.session.echo());
        // a record line restarts the idle time
        bench.ram.now.set(2_000);
        bench.text("LOAD");
        bench.ram.now.set(2_000 + limit - 1);
        bench.text(&record_line(0, 0x04, &[8, 0]));
        bench.ram.now.set(2_000 + limit + 100);
        assert_eq!(bench.poll(), "");
        bench.ram.now.set(2_000 + 2 * limit - 1);
        assert_eq!(bench.poll(), "ERR TIMEOUT\r\nBOOT> ");
    }

    #[test]
    fn info_for_a_flash_without_an_image() {
        let mut bench = Bench::new(Ram::new());
        bench.text("ERASE FRU");
        bench.text("CONFIRM");
        let want = crlf(&[
            "ipmc-boot v0.1.0 (sector 0)",
            "Reset cause:    power-on",
            "BOOTLOADER pin: 0",
            "Request:        none",
            "Runtime image:  header at 0x0800_8200: missing",
            "Image check:    not run",
            "Stored digest:  none",
            "Decision:       staying in the bootloader: no image, waiting for WAKEUP on CLI or APP",
            "Bootloader:     ipmc-boot v0.1.0",
            "Build:          bench",
            "Port:           CLI 115200 8N1 RTS/CTS off",
            "Erased:         FRU",
            "OK",
        ]);
        assert_eq!(bench.text("INFO"), format!("{want}{PROMPT}"));
    }

    #[test]
    fn info_for_a_valid_image_prints_both_digests_in_full() {
        let image = good_image();
        let mut bench = Bench::new(Ram::with_image(&image));
        bench.ram.step = 4;
        let (hex, stored) = digests(&bench.ram);
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, stored);
        let length = check(&bench.ram).header.unwrap().length;
        let want = crlf(&[
            "ipmc-boot v0.1.0 (sector 0)",
            "Reset cause:    power-on",
            "BOOTLOADER pin: 0",
            "Request:        none",
            &format!("Runtime image:  header at 0x0800_8200: found, v0.1.0, {length} bytes, flags 00h"),
            &format!("Image check:    SHA-256 {hex} match, 4 ms"),
            &format!("Stored digest:  {hex}"),
            "Decision:       staying in the bootloader: no image, waiting for WAKEUP on CLI or APP",
            "Bootloader:     ipmc-boot v0.1.0",
            "Build:          bench",
            "Port:           CLI 115200 8N1 RTS/CTS off",
            "Erased:         none",
            "OK",
        ]);
        assert_eq!(bench.text("INFO"), format!("{want}{PROMPT}"));
    }

    #[test]
    fn echo_is_on_in_command_mode_only() {
        let mut bench = loading();
        assert!(!bench.session.echo());
        bench.text("x");
        assert!(bench.session.echo());
    }
}
