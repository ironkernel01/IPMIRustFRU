//! commands.rs - the command table, the handlers, the banner and the byte-to-answer step.
//!
//! `COMMANDS` is the one list of what the port accepts. `engine::run` finds the row a line
//! starts with and calls its handler; a handler asks the `AppHost` for what it needs, writes the
//! data lines (`KEY: value`) and returns `Ok`, or returns the `Fail` that becomes the final
//! `ERROR` line. `serve` is the step the firmware (and the tests) call for every received byte.

use core::fmt::{self, Write};

use ipmc_core::ipmb::IPMB_MAX_LEN;
use ipmc_core::sensor::{PowerState, Summary};
use ipmc_fru::{FieldError, FruField};

use crate::engine::{
    final_line, reject, run, write_help, Args, Fail, Row, ANSWER_MAX, END, PROMPT, TAG_MAX,
};
use crate::host::{AppHost, SaveState};
use crate::line::{AnswerBuf, Fed, LineEditor};

/// PROTOCOL_VERSION -- the protocol version `INFO` and the banner report. A change that breaks an
/// existing request or answer raises it; a new command does not. Version 2 is the readable
/// format (prompt, `KEY: value` lines, `OK` / `ERROR` final line).
pub const PROTOCOL_VERSION: u32 = 2;

/// VALUE_MAX -- room for the text of one unit field: 63 bytes is the longest FRU text field.
const VALUE_MAX: usize = 64;

/// WRITE_ENABLED -- `FRU SET` and the write requests of `WRITE_REFUSED` are allowed (defs.rs
/// IPMC_APP_FRU_WRITE).
const WRITE_ENABLED: bool = ipmc_defs::IPMC_APP_FRU_WRITE;

/// WRITE_REFUSED -- the (NetFn, command) pairs `IPMI` refuses while IPMC_APP_FRU_WRITE is false:
/// the requests that write the FRU data. Storage NetFn 0Ah, Write FRU Data 12h. A developer who
/// adds a request that changes FRU content from the application port adds its pair here. FRU
/// Control, Set IPMB State and Set Event Receiver are not FRU data writes and still run.
const WRITE_REFUSED: &[(u8, u8)] = &[(0x0A, 0x12)];

/// COMMANDS -- every command of the application port, in the order `HELP` lists them. A new
/// command is one more row (see the recipe in lib.rs). The help sentence is one sentence for a
/// person; the syntax string defines the parameters (see engine.rs).
pub const COMMANDS: &[Row] = &[
    Row {
        syntax: "HELP {COMMAND...}",
        help: "List the commands, or describe one",
        handler: cmd_help,
    },
    Row {
        syntax: "? {COMMAND...}",
        help: "Same as HELP",
        handler: cmd_help,
    },
    Row {
        syntax: "INFO",
        help: "Firmware name, version, Tier and protocol version",
        handler: cmd_info,
    },
    Row {
        syntax: "STATUS",
        help: "IPMB address, GA, FRU state, payload power, health, NVMRO, uptime",
        handler: cmd_status,
    },
    Row {
        syntax: "FRU STATE",
        help: "Operational state of FRU 0 (M0, M1, M4, M5 or M6)",
        handler: cmd_fru_state,
    },
    Row {
        syntax: "FRU FIELDS",
        help: "Every FRU field: current value and maximum length",
        handler: cmd_fru_fields,
    },
    Row {
        syntax: "FRU GET [FIELD]",
        help: "One FRU field",
        handler: cmd_fru_get,
    },
    Row {
        syntax: "FRU SET [FIELD] {VALUE...}",
        help: "Write a FRU field; no value clears it; MFG_DATE as YYYY-MM-DD HH:MM",
        handler: cmd_fru_set,
    },
    Row {
        syntax: "FRU SAVE",
        help: "Whether the FRU image is in flash (a change saves itself after the save delay)",
        handler: cmd_fru_save,
    },
    Row {
        syntax: "IPMI [NETFN] [CMD] {DATA...}",
        help: "Run one IPMI request locally (hex bytes); nothing goes on the IPMB",
        handler: cmd_ipmi,
    },
];

// ---- the size of the longest answer ----

/// FIELD_TEXT_MAX -- the longest text of a FRU field: 63 bytes (IPMI FRU type/length byte, 6 bits).
/// A Chassis Manager can write a field longer than the room this IPMC reserves for it, so the
/// worst case for the size of an answer is this, not `max_len`.
const FIELD_TEXT_MAX: usize = 63;

/// help_worst_case -- bytes of the `HELP` answer without a filter: per row two blanks, the syntax
/// padded to the longest syntax plus two, ` - `, the help sentence and the line end.
const fn help_worst_case() -> usize {
    let (mut widest, mut i) = (0, 0);
    while i < COMMANDS.len() {
        if COMMANDS[i].syntax.len() > widest {
            widest = COMMANDS[i].syntax.len();
        }
        i += 1;
    }
    let (mut total, mut i) = (0, 0);
    while i < COMMANDS.len() {
        total += 2 + (widest + 2) + 3 + COMMANDS[i].help.len() + END.len();
        i += 1;
    }
    total
}

/// fields_worst_case -- bytes of the `FRU FIELDS` answer when every text field holds
/// `FIELD_TEXT_MAX` bytes: per field the name, `: `, the value, ` (max nn)` and the line end.
const fn fields_worst_case() -> usize {
    let (mut total, mut i) = (0, 0);
    while i < FruField::ALL.len() {
        total += FruField::ALL[i].name().len() + 2 + FIELD_TEXT_MAX + " (max 63)".len() + END.len();
        i += 1;
    }
    total
}

/// ECHO_WORST_CASE -- the echo of one pass of typed bytes (`IPMC_APP_BYTES_PER_POLL`), 3 bytes
/// each at worst (a backspace echoes BS SP BS), which comes before an answer in the buffer.
const ECHO_WORST_CASE: usize = ipmc_defs::IPMC_APP_BYTES_PER_POLL * 3;

/// ANSWER_TAIL -- what follows the data lines: `OK`, a blank, `#` and a tag of `TAG_MAX` letters or
/// digits (the longest final line), the line end, and the prompt.
const ANSWER_TAIL: usize = "OK #".len() + TAG_MAX + END.len() + PROMPT.len();

// The longest answers (the `HELP` table and `FRU FIELDS` with 63-byte values) with the echo in
// front and the final line and prompt behind fit the answer buffer. A command added to the table,
// a longer help sentence or a larger `IPMC_APP_BYTES_PER_POLL` that breaks this fails the build
// here, not on a payload CPU.
const _: () = assert!(
    ECHO_WORST_CASE + ANSWER_TAIL + help_worst_case() <= ANSWER_MAX
        && ECHO_WORST_CASE + ANSWER_TAIL + fields_worst_case() <= ANSWER_MAX,
    "ANSWER_MAX is too small for the longest answer (HELP or FRU FIELDS) with the echo, the tag and the prompt"
);

// ---- the byte-to-answer step ----

/// banner -- the start-up banner (not an answer: it has no final line) and the first prompt.
pub fn banner(host: &dyn AppHost, out: &mut dyn Write) -> fmt::Result {
    write!(
        out,
        "IPMC application port, {} v{}, Tier {}, protocol {PROTOCOL_VERSION}. Type HELP or ?.{END}{PROMPT}",
        ipmc_defs::IPMC_FW_NAME,
        host.version(),
        ipmc_defs::IPMC_TIER,
    )
}

/// respond -- answers what the editor returned: a line runs, an empty line answers the prompt
/// alone, and a line that cannot be a request gets its `ERROR` line (the request is lost, so no
/// tag can be echoed). `Fed::Damaged(n)` runs nothing, because a changed request must never act.
/// Every answer ends with the prompt. Returns false, and writes nothing, for `Fed::Nothing`.
///
/// Lines that were complete inside the bytes the firmware discarded after a loss get no answer at
/// all; the payload's answer timeout covers them. A payload that writes a field also compares
/// the `FRU SET` echo with what it sent; a byte lost without any counter (a framing error) shows
/// there.
pub fn respond<const N: usize>(
    fed: &Fed<'_>,
    host: &mut dyn AppHost,
    out: &mut AnswerBuf<N>,
) -> bool {
    match fed {
        Fed::Nothing => return false,
        Fed::Empty => {}
        Fed::Line(line) => run(COMMANDS, line, host, out),
        Fed::TooLong => reject(
            out,
            Fail::new(
                "LINE_TOO_LONG",
                format_args!(
                    "the line is longer than {} bytes; it was dropped",
                    ipmc_defs::IPMC_APP_LINE_MAX
                ),
            ),
        ),
        Fed::BadChar => reject(
            out,
            Fail::new(
                "BAD_CHAR",
                format_args!("the line holds a byte that is not printable ASCII; it was dropped"),
            ),
        ),
        Fed::Damaged(lost) => reject(
            out,
            Fail::new(
                "LINE_DAMAGED",
                format_args!("{lost} bytes lost on the port; the line was dropped"),
            ),
        ),
    }
    // The prompt is short; the buffer keeps room for it (a test checks the longest answers).
    let _ = out.write_str(PROMPT);
    true
}

/// Served -- what `serve` did with one byte.
#[derive(Debug, PartialEq, Eq)]
pub struct Served<'a> {
    /// The state of the line after the byte; `Fed::Nothing` when the line goes on.
    pub fed: Fed<'a>,
    /// Where the answer starts in the output buffer: the echo of the byte comes before it. For
    /// `Fed::Nothing` it is the buffer length after the echo.
    pub answer_at: usize,
}

/// serve -- takes one received byte: appends its echo to `out` and, when the byte ends a line,
/// the answer and the prompt. The caller sends what `out` holds. The firmware logs the request
/// from `Served::fed` and the final line of `out` from `Served::answer_at`.
pub fn serve<'e, const L: usize, const N: usize>(
    editor: &'e mut LineEditor<L>,
    byte: u8,
    host: &mut dyn AppHost,
    out: &mut AnswerBuf<N>,
) -> Served<'e> {
    let edit = editor.push(byte);
    // An echo that does not fit is lost: it is only for the person typing.
    let _ = edit.echo.write_to(out);
    let answer_at = out.len();
    respond(&edit.fed, host, out);
    Served {
        fed: edit.fed,
        answer_at,
    }
}

/// answer_final_line -- the final line of the answer that starts at `answer_at` in `out`.
pub fn answer_final_line<const N: usize>(out: &AnswerBuf<N>, answer_at: usize) -> Option<&str> {
    out.as_str().get(answer_at..).and_then(final_line)
}

// ---- data lines and helpers ----

/// data -- writes the data line `KEY: value`.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn data(out: &mut dyn Write, key: &str, value: fmt::Arguments<'_>) -> Result<(), Fail> {
    Ok(write!(out, "{key}: {value}{END}")?)
}

/// field_of -- the field named by `word`.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn field_of(word: &str) -> Result<FruField, Fail> {
    FruField::from_name(word).ok_or_else(|| {
        Fail::new(
            "ARGS",
            format_args!("unknown field '{word}'; FRU FIELDS lists them"),
        )
    })
}

/// field_error -- the failure of a field access. `field` gives the name and size in the text.
fn field_error(error: FieldError, field: FruField) -> Fail {
    let name = field.name();
    match error {
        FieldError::TooLong => Fail::new(
            "TOO_LONG",
            format_args!("{name} is at most {} characters", field.max_len()),
        ),
        FieldError::BadLength => Fail::new(
            "BAD_LENGTH",
            format_args!("{name} is empty or at least 2 characters"),
        ),
        FieldError::NotAscii => Fail::new(
            "NOT_ASCII",
            format_args!("{name} takes printable ASCII only"),
        ),
        FieldError::NoRoom => Fail::new(
            "NO_ROOM",
            format_args!("the FRU area has no room for that {name}"),
        ),
        FieldError::Nvmro => Fail::new(
            "NVMRO",
            format_args!("the FRU is write protected (NVMRO is asserted)"),
        ),
        FieldError::BadDate => Fail::new("BAD_DATE", format_args!("{}", ipmc_fru::date::DateRange)),
        FieldError::AreaMissing | FieldError::Corrupt => Fail::new(
            "FRU_IMAGE",
            format_args!("the FRU image is damaged ({})", error.name()),
        ),
    }
}

/// stored -- the text of `field` as the host holds it now (nothing for an empty field).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn stored(host: &dyn AppHost, field: FruField) -> Result<AnswerBuf<VALUE_MAX>, Fail> {
    let mut value = AnswerBuf::new();
    host.unit_field(field, &mut value)
        .map_err(|error| field_error(error, field))?;
    Ok(value)
}

/// write_disabled -- the `WRITE_DISABLED` failure of `FRU SET` and of a refused `IPMI` request.
fn write_disabled() -> Fail {
    Fail::new(
        "WRITE_DISABLED",
        format_args!("FRU writes are turned off in this build (IPMC_APP_FRU_WRITE is false)"),
    )
}

// ---- handlers ----

/// cmd_help -- `HELP` and `?` print the aligned table of every command; `HELP <words>` and
/// `? <words>` print the rows that start with those words (MenuBuilder section 9: `?` is HELP).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_help(args: &Args<'_>, _host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    write_help(COMMANDS, args.get(0), out)
}

/// cmd_info -- `INFO` answers FIRMWARE, VERSION, TIER and PROTOCOL.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_info(_args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    data(out, "FIRMWARE", format_args!("{}", ipmc_defs::IPMC_FW_NAME))?;
    data(out, "VERSION", format_args!("{}", host.version()))?;
    data(out, "TIER", format_args!("{}", ipmc_defs::IPMC_TIER))?;
    data(out, "PROTOCOL", format_args!("{PROTOCOL_VERSION}"))
}

/// power_word -- the PAYLOAD_POWER value. A power-on that waits for PGOOD is not on yet.
const fn power_word(power: PowerState) -> &'static str {
    match power {
        PowerState::On => "on",
        PowerState::Off | PowerState::PowerOnRequested => "off",
        PowerState::Fault => "fault",
    }
}

/// health_word -- the HEALTH value. Only a reading of "functioning properly" is `ok`; a bad
/// reading and a missing one (before the first sample) are `fault`.
const fn health_word(health: Summary) -> &'static str {
    match health {
        Summary::Good => "ok",
        Summary::Bad | Summary::Unavailable => "fault",
    }
}

/// cmd_status -- `STATUS` answers IPMB_ADDRESS (`82h`), GA, FRU_STATE (`M4 FRU Active`),
/// PAYLOAD_POWER (`on`, `off`, `fault`), HEALTH (`ok`, `fault`), NVMRO (`asserted`, `deasserted`)
/// and UPTIME (`0d 00:17:48`).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_status(_args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    let status = host.status();
    let nvmro = if status.nvmro {
        "asserted"
    } else {
        "deasserted"
    };
    let s = status.uptime_s;
    data(
        out,
        "IPMB_ADDRESS",
        format_args!("{:02X}h", status.ipmb_addr),
    )?;
    data(out, "GA", format_args!("{}", status.ga))?;
    data(
        out,
        "FRU_STATE",
        format_args!("{}", host.fru_state().name()),
    )?;
    data(
        out,
        "PAYLOAD_POWER",
        format_args!("{}", power_word(status.power)),
    )?;
    data(
        out,
        "HEALTH",
        format_args!("{}", health_word(status.health)),
    )?;
    data(out, "NVMRO", format_args!("{nvmro}"))?;
    data(
        out,
        "UPTIME",
        format_args!(
            "{}d {:02}:{:02}:{:02}",
            s / 86_400,
            s / 3_600 % 24,
            s / 60 % 60,
            s % 60
        ),
    )
}

/// cmd_fru_state -- `FRU STATE` answers FRU_STATE (`M4 FRU Active`).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_fru_state(
    _args: &Args<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
) -> Result<(), Fail> {
    data(
        out,
        "FRU_STATE",
        format_args!("{}", host.fru_state().name()),
    )
}

/// cmd_fru_fields -- `FRU FIELDS` answers one line per field, `<FIELD>: <value> (max <n>)`. An
/// empty text field leaves nothing between the colon and blank and the parenthesis:
/// `ASSET_TAG:  (max 24)`. MFG_DATE is `YYYY-MM-DD HH:MM` (UTC).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_fru_fields(
    _args: &Args<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
) -> Result<(), Fail> {
    for field in FruField::ALL {
        let value = stored(host, field)?;
        data(
            out,
            field.name(),
            format_args!("{} (max {})", value.as_str(), field.max_len()),
        )?;
    }
    Ok(())
}

/// cmd_fru_get -- `FRU GET <field>` answers `<FIELD>: <value>`; an empty field leaves the value
/// empty.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_fru_get(args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    let field = field_of(args.required(0)?)?;
    let value = stored(host, field)?;
    data(out, field.name(), format_args!("{}", value.as_str()))
}

/// cmd_fru_set -- `FRU SET <field> <value>` writes the field and answers `<FIELD>: <value>` with
/// the value as stored now. The value is the rest of the line with the blanks at both ends
/// removed; blanks inside it stay. `FRU SET <field>` alone clears a text field. Refused with
/// `WRITE_DISABLED` when IPMC_APP_FRU_WRITE is false. The other errors are `TOO_LONG`,
/// `BAD_LENGTH`, `NOT_ASCII`, `NO_ROOM`, `NVMRO`, `BAD_DATE` and `FRU_IMAGE`; a refused request
/// leaves the FRU image as it was.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_fru_set(args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    set_field(args, host, out, WRITE_ENABLED)
}

/// set_field -- `cmd_fru_set` with the write permission as a parameter, so a test reaches the
/// `WRITE_DISABLED` answer whatever the build setting is.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
pub(crate) fn set_field(
    args: &Args<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
    write_enabled: bool,
) -> Result<(), Fail> {
    if !write_enabled {
        return Err(write_disabled());
    }
    let field = field_of(args.required(0)?)?;
    host.set_unit_field(field, args.get(1).unwrap_or(""))
        .map_err(|error| field_error(error, field))?;
    let value = stored(host, field)?;
    data(out, field.name(), format_args!("{}", value.as_str()))
}

/// cmd_fru_save -- `FRU SAVE` answers `SAVE: done` when the flash log holds the FRU image in RAM
/// and `SAVE: pending` while a change still waits for its save (not an error: a payload CPU that
/// wrote the unit data repeats `FRU SAVE` until it reads `done` before it removes power).
/// `LOG_OFF` when the flash log is off.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_fru_save(_args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    let word = match host.save_state() {
        SaveState::Saved => "done",
        SaveState::Pending => "pending",
        SaveState::LogOff => {
            return Err(Fail::new(
                "LOG_OFF",
                format_args!("the flash log is off; the FRU image lives in RAM only"),
            ))
        }
    };
    data(out, "SAVE", format_args!("{word}"))
}

/// hex_byte -- one hex byte (`06`, `0x06` and `6` are the same).
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn hex_byte(word: &str) -> Result<u8, Fail> {
    ipmc_wire::hex_word(word)
        .ok_or_else(|| Fail::new("ARGS", format_args!("'{word}' is not a hex byte")))
}

/// cmd_ipmi -- `IPMI <netfn> <cmd> {data...}` runs one request through the IPMI dispatcher as
/// if the Chassis Manager had sent it, and answers CC (`00h`), DATA (hex bytes, empty when none)
/// and COMMAND (the name the capture log prints; no line when the build has none). A completion
/// code other than 00h is still an answer: the request ran and the IPMC said no.
/// `IPMI_FAILED` means it did not run (a NetFn that is odd or above 3Eh, or more data than one
/// frame holds). While IPMC_APP_FRU_WRITE is false, Write FRU Data (`WRITE_REFUSED`) is refused
/// with `WRITE_DISABLED`.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn cmd_ipmi(args: &Args<'_>, host: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
    ipmi_request(args, host, out, WRITE_ENABLED)
}

/// ipmi_request -- `cmd_ipmi` with the write permission as a parameter, so a test reaches the
/// `WRITE_DISABLED` answer whatever the build setting is.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
pub(crate) fn ipmi_request(
    args: &Args<'_>,
    host: &mut dyn AppHost,
    out: &mut dyn Write,
    write_enabled: bool,
) -> Result<(), Fail> {
    let netfn = hex_byte(args.required(0)?)?;
    let cmd = hex_byte(args.required(1)?)?;
    let mut bytes = [0u8; IPMB_MAX_LEN];
    let mut count = 0;
    for word in args.get(2).unwrap_or("").split_ascii_whitespace() {
        let value = hex_byte(word)?;
        let slot = bytes.get_mut(count).ok_or_else(|| {
            Fail::new("ARGS", format_args!("more than {IPMB_MAX_LEN} data bytes"))
        })?;
        *slot = value;
        count += 1;
    }
    if !write_enabled && WRITE_REFUSED.contains(&(netfn, cmd)) {
        return Err(write_disabled());
    }
    let (code, response) = host
        .ipmi(netfn, cmd, bytes.get(..count).unwrap_or(&[]))
        .map_err(|error| Fail::new("IPMI_FAILED", format_args!("{}", error.sentence())))?;
    data(out, "CC", format_args!("{code:02X}h"))?;
    write!(out, "DATA: ")?;
    for (index, byte) in response.as_bytes().iter().enumerate() {
        let separator = if index == 0 { "" } else { " " };
        write!(out, "{separator}{byte:02X}")?;
    }
    write!(out, "{END}")?;
    match host.command_name(netfn, cmd) {
        Some(name) => data(out, "COMMAND", format_args!("{name}")),
        None => Ok(()),
    }
}
