//! commands.rs - the command table, the handlers and `answer`.
//!
//! One request line comes in, one answer line goes out. `answer` splits the line into an
//! optional tag, a command word and the rest, finds the row of `COMMANDS` whose keyword matches
//! the word, and calls that row's handler. A handler reads what it needs from the `AppHost` and
//! writes the whole answer line (`OK ...` or `ERR <CODE> <text>`) to `out`. A handler writes
//! nothing until it knows which of the two it sends, so an answer is never half of each.

use core::fmt::{self, Write};

use ipmc_core::fru_state::State;
use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::sensor::{PowerState, Summary};
use ipmc_fru::{FieldError, FruField};

use crate::host::{AppHost, SaveState};
use crate::line::{AnswerBuf, Fed};

/// PROTOCOL_VERSION -- the protocol version `HELLO` reports. A change that breaks an existing
/// request or answer raises it; a new command does not.
pub const PROTOCOL_VERSION: u32 = 1;

/// ANSWER_MAX -- the size of the buffer an answer line is formatted into, bytes. The longest
/// answers are `ERR UNKNOWN <word>` for a word as long as a request line and the `IPMI` answer
/// with a full frame of data (3 characters per byte).
pub const ANSWER_MAX: usize = 256;
// The longest error answer that echoes a request word: "#" + 8 tag characters + a blank, then
// "ERR UNKNOWN " (12 bytes) and the word (at most a whole request line).
const _: () = assert!(
    ANSWER_MAX >= 10 + 12 + ipmc_defs::IPMC_APP_LINE_MAX as usize,
    "ANSWER_MAX must hold the longest error answer that echoes a request word"
);

/// VALUE_MAX -- room for the text of one unit field: 63 bytes is the longest FRU text field.
const VALUE_MAX: usize = 64;

/// WRITE_ENABLED -- `SET` and the write requests of `WRITE_REFUSED` are allowed (ipmi_defs.h
/// IPMC_APP_FRU_WRITE).
const WRITE_ENABLED: bool = ipmc_defs::IPMC_APP_FRU_WRITE == 1;

/// WRITE_REFUSED -- the (NetFn, command) pairs `IPMI` refuses while IPMC_APP_FRU_WRITE is 0:
/// the requests that write the FRU data. Storage NetFn 0Ah, Write FRU Data 12h. A developer who
/// adds a request that changes FRU content from the application port adds its pair here. FRU
/// Control, Set IPMB State and Set Event Receiver are not FRU data writes and still run.
const WRITE_REFUSED: &[(u8, u8)] = &[(0x0A, 0x12)];

/// Request -- the parts of a request line after the tag, handed to a handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    /// The command word as typed (any case).
    pub word: &'a str,
    /// The rest of the line after one space or tab, spaces inside and at the end kept; empty
    /// when the word ends the line.
    pub rest: &'a str,
}

impl<'a> Request<'a> {
    /// tokens -- the rest of the line split at spaces and tabs.
    pub fn tokens(&self) -> impl Iterator<Item = &'a str> {
        self.rest.split_ascii_whitespace()
    }
}

/// Handler -- a command handler: reads the request, asks the host, writes one answer line to `out`.
/// The Err case is a full answer buffer, which the caller reports as one dropped answer.
pub type Handler = fn(&Request<'_>, &mut dyn AppHost, &mut dyn Write) -> fmt::Result;

/// AppCommand -- one row of `COMMANDS`.
pub struct AppCommand {
    /// The command word, upper case; matching ignores case.
    pub keyword: &'static str,
    /// The arguments in one line for `HELP <keyword>`: `[]` optional, `<>` required; empty for
    /// none.
    pub args: &'static str,
    /// One sentence for `HELP <keyword>`.
    pub help: &'static str,
    /// The function that answers the request.
    pub handler: Handler,
}

/// COMMANDS -- every command of the application port, in the order `HELP` lists them. A new
/// command is one more row (see the recipe in lib.rs).
pub const COMMANDS: &[AppCommand] = &[
    AppCommand {
        keyword: "HELP",
        args: "[keyword]",
        help: "list the commands, or describe one",
        handler: cmd_help,
    },
    AppCommand {
        keyword: "HELLO",
        args: "",
        help: "firmware name, version, Tier and protocol version",
        handler: cmd_hello,
    },
    AppCommand {
        keyword: "STATUS",
        args: "",
        help: "IPMB address, GA, FRU state, power, health, NVMRO, uptime",
        handler: cmd_status,
    },
    AppCommand {
        keyword: "FRU",
        args: "STATE",
        help: "the Operational State of FRU 0 (M0, M1, M4, M5, M6)",
        handler: cmd_fru,
    },
    AppCommand {
        keyword: "GET",
        args: "<field>",
        help: "read a FRU field as stored",
        handler: cmd_get,
    },
    AppCommand {
        keyword: "SET",
        args: "<field> [value]",
        help: "write a FRU field; no value clears it",
        handler: cmd_set,
    },
    AppCommand {
        keyword: "FIELDS",
        args: "",
        help: "the FRU fields and the longest value of each",
        handler: cmd_fields,
    },
    AppCommand {
        keyword: "IPMI",
        args: "<netfn> <cmd> [data...]",
        help: "run an IPMI request locally; hex bytes; nothing goes on the IPMB",
        handler: cmd_ipmi,
    },
    AppCommand {
        keyword: "SAVE",
        args: "",
        help: "OK once the FRU image is in the flash log",
        handler: cmd_save,
    },
];

// ---- answer ----

/// err -- writes `ERR <code> <text>`.
fn err(out: &mut dyn Write, code: &str, text: fmt::Arguments<'_>) -> fmt::Result {
    write!(out, "ERR {code} {text}")
}

/// valid_tag -- a tag is 1 to 8 ASCII letters or digits (the part after the `#`).
fn valid_tag(tag: &str) -> bool {
    (1..=8).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// is_blank -- a space or a tab.
const fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// answer -- answers one request line (no CR or LF) with one answer line written to `out`.
///
/// 1. Leading blanks are skipped. A first word that starts with `#` is the tag: 1 to 8 letters
///    or digits after the `#`. It is echoed, followed by one space, at the start of the answer.
///    A bad tag is answered `ERR ARGS` without a tag.
/// 2. The next word is the command, matched against `COMMANDS` ignoring case. The text after
///    one blank is the rest of the line (`Request::rest`).
/// 3. The matching handler writes the answer. No match is `ERR UNKNOWN <word>`; a line with a
///    tag and nothing else is `ERR ARGS`.
///
/// `out` gets exactly one line without a line end; the caller adds CR LF. A write error from
/// `out` (a full buffer) ends the answer early; the caller checks its buffer for that.
pub fn answer(line: &str, host: &mut dyn AppHost, out: &mut dyn Write) {
    // A write error means the caller's buffer is full; the caller reads that from the buffer.
    let _ = answer_line(line, host, out);
}

/// leading_tag -- the first word of `line` (leading blanks skipped) when it starts with `#`:
/// the text after the `#` and the rest of the line. The tag is not checked here (`valid_tag`).
fn leading_tag(line: &str) -> Option<(&str, &str)> {
    let tagged = line.trim_start_matches(is_blank).strip_prefix('#')?;
    Some(tagged.split_once(is_blank).unwrap_or((tagged, "")))
}

/// answer_line -- `answer` with the write errors returned.
fn answer_line(line: &str, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    let line = line.trim_start_matches(is_blank);
    let line = if let Some((tag, after)) = leading_tag(line) {
        if !valid_tag(tag) {
            return err(
                out,
                "ARGS",
                format_args!("a tag is # and 1 to 8 letters or digits"),
            );
        }
        write!(out, "#{tag} ")?;
        after.trim_start_matches(is_blank)
    } else {
        line
    };
    let (word, rest) = line.split_once(is_blank).unwrap_or((line, ""));
    if word.is_empty() {
        return err(out, "ARGS", format_args!("empty request"));
    }
    let request = Request { word, rest };
    match COMMANDS
        .iter()
        .find(|command| command.keyword.eq_ignore_ascii_case(word))
    {
        Some(command) => (command.handler)(&request, host, out),
        None => err(out, "UNKNOWN", format_args!("{word}")),
    }
}

/// respond -- answers what `LineAssembler::push` returned. A line goes to `answer`; a line that
/// was too long or held a byte other than printable ASCII gets its own `ERR` answer (the request
/// is lost, so no tag can be echoed). `Fed::Damaged(n)` answers `ERR LINE_DAMAGED <n> bytes lost
/// on the port` without a tag (the tag may be among the lost bytes) and runs nothing, because a
/// changed request must never act. Returns true when an answer was written, false for
/// `Fed::Nothing`.
///
/// Lines that were complete inside the bytes the firmware discarded after a loss get no answer
/// at all; the payload's answer timeout covers them. A payload that writes a field also compares
/// the `OK <FIELD> <value>` echo of `SET` with what it sent; a byte lost without any counter (a
/// framing error) shows there.
pub fn respond(fed: Fed<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> bool {
    let _ = match fed {
        Fed::Nothing => return false,
        Fed::Line(line) => {
            answer(line, host, out);
            Ok(())
        }
        Fed::TooLong => err(
            out,
            "LINE_TOO_LONG",
            format_args!("max={}", ipmc_defs::IPMC_APP_LINE_MAX),
        ),
        Fed::BadChar => err(
            out,
            "ARGS",
            format_args!("only printable ASCII is accepted"),
        ),
        Fed::Damaged(lost) => err(
            out,
            "LINE_DAMAGED",
            format_args!("{lost} bytes lost on the port"),
        ),
    };
    true
}

// ---- handlers ----

/// cmd_help -- `HELP` lists the keywords: `OK HELP HELP HELLO STATUS ...`. `HELP <keyword>`
/// describes one: `OK HELP SET <field> [value] - write a FRU field`. An unknown keyword
/// is `ERR UNKNOWN <word>`.
fn cmd_help(request: &Request<'_>, _host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    let mut tokens = request.tokens();
    let Some(word) = tokens.next() else {
        write!(out, "OK HELP")?;
        return COMMANDS
            .iter()
            .try_for_each(|command| write!(out, " {}", command.keyword));
    };
    match COMMANDS
        .iter()
        .find(|command| command.keyword.eq_ignore_ascii_case(word))
    {
        Some(command) if command.args.is_empty() => {
            write!(out, "OK HELP {} - {}", command.keyword, command.help)
        }
        Some(command) => write!(
            out,
            "OK HELP {} {} - {}",
            command.keyword, command.args, command.help
        ),
        None => err(out, "UNKNOWN", format_args!("{word}")),
    }
}

/// cmd_hello -- `HELLO` answers `OK IPMC-APP 1 <firmware name> v<version> tier=<n>`. A payload CPU
/// sends it first to learn that an IPMC answers and which protocol version it speaks.
fn cmd_hello(_request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    write!(
        out,
        "OK IPMC-APP {PROTOCOL_VERSION} {} v{} tier={}",
        ipmc_defs::IPMC_FW_NAME,
        host.version(),
        ipmc_defs::IPMC_TIER
    )
}

/// power_word -- the `power=` value. A power-on that waits for PGOOD is not on yet.
const fn power_word(power: PowerState) -> &'static str {
    match power {
        PowerState::On => "on",
        PowerState::Off | PowerState::PowerOnRequested => "off",
        PowerState::Fault => "fault",
    }
}

/// health_word -- the `health=` value. Only a reading of "functioning properly" is `ok`; a bad
/// reading and a missing one (before the first sample) are `fault`.
const fn health_word(health: Summary) -> &'static str {
    match health {
        Summary::Good => "ok",
        Summary::Bad | Summary::Unavailable => "fault",
    }
}

/// cmd_status -- `STATUS` answers
/// `OK STATUS ipmb=<addr>h ga=<n> fru=M<n> power=<on|off|fault> health=<ok|fault> nvmro=<0|1> uptime_s=<n>`.
fn cmd_status(_request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    let status = host.status();
    write!(
        out,
        "OK STATUS ipmb={:02X}h ga={} fru=M{} power={} health={} nvmro={} uptime_s={}",
        status.ipmb_addr,
        status.ga,
        host.fru_state().number(),
        power_word(status.power),
        health_word(status.health),
        u8::from(status.nvmro),
        status.uptime_s
    )
}

/// cmd_fru -- `FRU STATE` answers `OK FRU M<n> <name>`, for example `OK FRU M4 FRU Active`.
/// Another word after `FRU` is `ERR ARGS`.
fn cmd_fru(request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    let mut tokens = request.tokens();
    match (tokens.next(), tokens.next()) {
        (Some(sub), None) if sub.eq_ignore_ascii_case("STATE") => {
            let state: State = host.fru_state();
            write!(out, "OK FRU {}", state.name())
        }
        _ => err(out, "ARGS", format_args!("usage: FRU STATE")),
    }
}

/// field_error -- writes the `ERR` line of a failed field access. `field` gives the `max=` of
/// `TOO_LONG`.
fn field_error(out: &mut dyn Write, error: FieldError, field: FruField) -> fmt::Result {
    match error {
        FieldError::TooLong => err(out, "TOO_LONG", format_args!("max={}", field.max_len())),
        FieldError::BadLength => err(
            out,
            "BAD_LENGTH",
            format_args!("a text of one byte is not allowed"),
        ),
        FieldError::NotAscii => err(out, "NOT_ASCII", format_args!("printable ASCII only")),
        FieldError::NoRoom => err(
            out,
            "NO_ROOM",
            format_args!("the FRU area has no room for it"),
        ),
        FieldError::Nvmro => err(out, "NVMRO", format_args!("the FRU is write protected")),
        FieldError::BadDate => err(
            out,
            "BAD_DATE",
            format_args!("{}", ipmc_fru::date::DateRange),
        ),
        FieldError::AreaMissing | FieldError::Corrupt => {
            err(out, "FRU_IMAGE", format_args!("{}", error.name()))
        }
    }
}

/// field_of -- the field named by `word`, or the `ERR ARGS` line.
fn field_of(word: &str, out: &mut dyn Write) -> Result<FruField, fmt::Result> {
    FruField::from_name(word).ok_or_else(|| {
        err(
            out,
            "ARGS",
            format_args!("unknown field {word} (FIELDS lists them)"),
        )
    })
}

/// echo_field -- writes `OK <FIELD> <value>` for the value the host holds now; an empty value
/// prints nothing after the name. On a host error nothing of the `OK` line is written.
fn echo_field(host: &dyn AppHost, field: FruField, out: &mut dyn Write) -> fmt::Result {
    let mut value = AnswerBuf::<VALUE_MAX>::new();
    if let Err(error) = host.unit_field(field, &mut value) {
        return field_error(out, error, field);
    }
    if value.is_empty() {
        write!(out, "OK {}", field.name())
    } else {
        write!(out, "OK {} {}", field.name(), value.as_str())
    }
}

/// cmd_get -- `GET <field>` answers `OK <FIELD> <value>`; the value is as stored, and an empty
/// field prints nothing after the name. An unknown field is `ERR ARGS`.
fn cmd_get(request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    let mut tokens = request.tokens();
    let (Some(word), None) = (tokens.next(), tokens.next()) else {
        return err(out, "ARGS", format_args!("usage: GET <field>"));
    };
    match field_of(word, out) {
        Ok(field) => echo_field(host, field, out),
        Err(written) => written,
    }
}

/// write_disabled -- the `ERR WRITE_DISABLED` answer of `SET` and of a refused `IPMI` request.
fn write_disabled(out: &mut dyn Write) -> fmt::Result {
    err(
        out,
        "WRITE_DISABLED",
        format_args!("IPMC_APP_FRU_WRITE is 0"),
    )
}

/// cmd_set -- `SET <field> <value>` writes the field and answers `OK <FIELD> <value>` with the
/// value as stored now. The value is the rest of the line after the one space that follows the
/// field name; spaces inside it stay. `SET <field>` alone clears a text field. Refused with
/// `ERR WRITE_DISABLED` when IPMC_APP_FRU_WRITE is 0. The errors are `TOO_LONG max=<n>`,
/// `BAD_LENGTH`, `NOT_ASCII`, `NO_ROOM`, `NVMRO`, `BAD_DATE` and `FRU_IMAGE`; a refused request
/// leaves the FRU image as it was.
fn cmd_set(request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    set_field(request, host, out, WRITE_ENABLED)
}

/// set_field -- `cmd_set` with the write permission as a parameter, so a test reaches the
/// `WRITE_DISABLED` answer whatever the build setting is.
pub(crate) fn set_field(
    request: &Request<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
    write_enabled: bool,
) -> fmt::Result {
    if !write_enabled {
        return write_disabled(out);
    }
    let rest = request.rest.trim_start_matches(is_blank);
    let (word, value) = rest.split_once(is_blank).unwrap_or((rest, ""));
    if word.is_empty() {
        return err(out, "ARGS", format_args!("usage: SET <field> [value]"));
    }
    let field = match field_of(word, out) {
        Ok(field) => field,
        Err(written) => return written,
    };
    match host.set_unit_field(field, value) {
        Ok(()) => echo_field(host, field, out),
        Err(error) => field_error(out, error, field),
    }
}

/// cmd_fields -- `FIELDS` answers `OK FIELDS BOARD_MFR=<max> BOARD_PRODUCT=<max> ...` (all thirteen): every
/// field with the longest value it takes, in bytes.
fn cmd_fields(_request: &Request<'_>, _host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    write!(out, "OK FIELDS")?;
    FruField::ALL
        .iter()
        .try_for_each(|field| write!(out, " {}={}", field.name(), field.max_len()))
}

/// cmd_ipmi -- `IPMI <netfn> <cmd> [data...]` runs one request through the IPMI dispatcher as
/// if the Chassis Manager had sent it, and answers `OK IPMI CC=<cc> DATA=<hex bytes>`. Every
/// number is a hex byte: `06`, `0x06` and `6` are the same. A completion code other than 00h is
/// still an `OK` answer: the request ran and the IPMC said no. `ERR IPMI_FAILED <why>` means it
/// did not run (a NetFn that is odd or above 3Eh, or more data than one frame holds). While
/// IPMC_APP_FRU_WRITE is 0, Write FRU Data (`WRITE_REFUSED`) answers `ERR WRITE_DISABLED`.
fn cmd_ipmi(request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    ipmi_request(request, host, out, WRITE_ENABLED)
}

/// ipmi_request -- `cmd_ipmi` with the write permission as a parameter, so a test reaches the
/// `WRITE_DISABLED` answer whatever the build setting is. With the permission off, a pair of
/// `WRITE_REFUSED` answers `ERR WRITE_DISABLED` and does not run.
pub(crate) fn ipmi_request(
    request: &Request<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
    write_enabled: bool,
) -> fmt::Result {
    let mut bytes = [0u8; IPMB_MAX_LEN];
    let mut count = 0;
    for token in request.tokens() {
        let Some(value) = ipmc_wire::hex_word(token) else {
            return err(out, "ARGS", format_args!("'{token}' is not a hex byte"));
        };
        let Some(slot) = bytes.get_mut(count) else {
            return err(out, "ARGS", format_args!("more than {IPMB_MAX_LEN} bytes"));
        };
        *slot = value;
        count += 1;
    }
    let [netfn, cmd, data @ ..] = bytes.get(..count).unwrap_or(&[]) else {
        return err(
            out,
            "ARGS",
            format_args!("usage: IPMI <netfn> <cmd> [data...]"),
        );
    };
    if !write_enabled && WRITE_REFUSED.contains(&(*netfn, *cmd)) {
        return write_disabled(out);
    }
    match host.ipmi(*netfn, *cmd, data) {
        Ok((code, response)) => {
            write!(out, "OK IPMI CC={code:02X} DATA=")?;
            for (index, byte) in response.as_bytes().iter().enumerate() {
                let separator = if index == 0 { "" } else { " " };
                write!(out, "{separator}{byte:02X}")?;
            }
            Ok(())
        }
        Err(error) => err(out, "IPMI_FAILED", format_args!("{}", error.name())),
    }
}

/// cmd_save -- `SAVE` answers `OK SAVED` when the flash log holds the FRU image in RAM,
/// `ERR PENDING` while a change still waits for its save, and `ERR LOG_OFF` when the flash log
/// is off. A payload CPU that wrote the unit data repeats `SAVE` until it reads `OK` before it
/// removes power.
fn cmd_save(_request: &Request<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    match host.save_state() {
        SaveState::Saved => write!(out, "OK SAVED"),
        SaveState::Pending => err(out, "PENDING", format_args!("a save is waiting")),
        SaveState::LogOff => err(out, "LOG_OFF", format_args!("the flash log is off")),
    }
}
