//! tests.rs - the host tests of the application protocol over a fake host.
//!
//! `FakeHost` implements `AppHost` over a real `FruStore`, so the field editor's own answers
//! (too long, one byte, not ASCII) come from the real code. `REQUEST_TABLE` is the one table of
//! request and answer pairs: a new command adds a row. An answer is written here as the lines a
//! person sees, ended by `\n` (the wire has CR LF), without the prompt.

use core::fmt::Write;

use ipmc_core::fru_state::State;
use ipmc_core::identity::{additional_support, DEVICE_IDENTITY};
use ipmc_core::sensor::{PowerState, Summary};
use ipmc_fru::{FieldError, FruField, FruStore};

use crate::commands::{ipmi_request, set_field};
use crate::engine::{
    check_table, find, write_help, Fail, Row, Syntax, SyntaxError, TableError, FAIL_TEXT_MAX,
};
use crate::{
    answer_final_line, banner, final_line, respond, run, serve, AnswerBuf, AppError, AppHost, Args,
    Echo, Fed, IpmiData, LineEditor, SaveState, Status, ANSWER_MAX, COMMANDS, PROMPT,
};

/// LONG_TEXT -- the longest text of a FRU field, bytes (type/length byte, 6 bits).
const LONG_TEXT: usize = 63;

/// FakeHost -- the board for the tests.
struct FakeHost {
    store: FruStore,
    nvmro: bool,
    power: PowerState,
    health: Summary,
    save: SaveState,
    state: State,
    /// The next `set_unit_field` fails with this error instead of editing the store.
    forced_set_error: Option<FieldError>,
    /// What `ipmi` returns: the completion code and data, or the error.
    ipmi_result: Result<(u8, Vec<u8>), AppError>,
    /// The requests `ipmi` received: (netfn, cmd, data).
    ipmi_calls: Vec<(u8, u8, Vec<u8>)>,
    /// The name `command_name` gives.
    name: Option<&'static str>,
    /// Every text field reads as `LONG_TEXT` bytes of text: a host whose FRU image a Chassis
    /// Manager filled with the longest values.
    long_text: bool,
}

impl FakeHost {
    fn new() -> FakeHost {
        FakeHost {
            store: FruStore::new(),
            nvmro: false,
            power: PowerState::On,
            health: Summary::Good,
            save: SaveState::Saved,
            state: State::M4,
            forced_set_error: None,
            ipmi_result: Ok((0, DEVICE_IDENTITY.response_data().to_vec())),
            ipmi_calls: Vec::new(),
            name: Some("App Get Device ID"),
            long_text: false,
        }
    }
}

impl AppHost for FakeHost {
    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn status(&self) -> Status {
        Status {
            ipmb_addr: 0x82,
            ga: 1,
            power: self.power,
            health: self.health,
            nvmro: self.nvmro,
            uptime_s: 1068,
        }
    }

    fn fru_state(&self) -> State {
        self.state
    }

    fn unit_field(&self, f: FruField, out: &mut dyn core::fmt::Write) -> Result<(), FieldError> {
        if self.long_text && f != FruField::MfgDate {
            return out
                .write_str(&"v".repeat(LONG_TEXT))
                .map_err(|_| FieldError::TooLong);
        }
        ipmc_fru::write_unit_field(self.store.bytes(), f, out)
    }

    fn set_unit_field(&mut self, f: FruField, value: &str) -> Result<(), FieldError> {
        if let Some(error) = self.forced_set_error {
            return Err(error);
        }
        if self.nvmro {
            return Err(FieldError::Nvmro);
        }
        self.store.set_unit_field(f, value)
    }

    fn ipmi(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<(u8, IpmiData), AppError> {
        self.ipmi_calls.push((netfn, cmd, data.to_vec()));
        match &self.ipmi_result {
            Ok((code, bytes)) => Ok((*code, IpmiData::from_slice(bytes).unwrap())),
            Err(error) => Err(*error),
        }
    }

    fn command_name(&self, _netfn: u8, _cmd: u8) -> Option<&'static str> {
        self.name
    }

    fn save_state(&self) -> SaveState {
        self.save
    }
}

/// reply -- the answer to one request line as the lines a person sees: LF line ends, no prompt.
fn reply(host: &mut FakeHost, line: &str) -> String {
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    run(COMMANDS, line, host, &mut out);
    assert!(!out.overflowed(), "{line:?} overflowed the answer buffer");
    assert!(out.as_str().ends_with("\r\n"), "{line:?}");
    out.as_str().replace("\r\n", "\n")
}

/// expected -- the answer text of a table row: `{fw}` and `{tier}` stand for the build values,
/// `{help}` for the HELP table and `{fields}` for the FRU FIELDS lines.
fn expected(text: &str) -> String {
    text.replace("{fw}", ipmc_defs::IPMC_FW_NAME)
        .replace("{tier}", &ipmc_defs::IPMC_TIER.to_string())
        .replace("{help}", HELP_TABLE)
        .replace("{fields}", FIELD_LINES)
        .replace("{devid}", &hex_text(&DEVICE_IDENTITY.response_data()))
}

/// hex_text -- bytes as the `DATA:` value of an IPMI answer: two hex digits each, one blank
/// between. The test host answers Get Device ID with the bytes the board build answers
/// (`DEVICE_IDENTITY`), so the documented session matches the board.
fn hex_text(bytes: &[u8]) -> String {
    let words: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    words.join(" ")
}

/// FIELD_LINES -- the data lines of `FRU FIELDS` on the built FRU image.
const FIELD_LINES: &str = "BOARD_MFR: IronKernel (max 24)
BOARD_PRODUCT: VPX Tier-1 IPMC (max 32)
BOARD_SERIAL: IK-T1-000001 (max 24)
BOARD_PART: IK-IPMC-T1-NUC (max 24)
BOARD_FILE_ID: ipmc-fru-v1 (max 16)
MFG_DATE: 2026-10-23 00:00 (max 16)
PRODUCT_MFR: IronKernel (max 24)
PRODUCT_NAME: Tier-1 IPMC Reference (max 32)
PRODUCT_PART: IK-IPMC-T1 (max 24)
PRODUCT_VERSION: 0.1 (max 16)
PRODUCT_SERIAL: IK-T1-000001 (max 24)
ASSET_TAG:  (max 24)
PRODUCT_FILE_ID:  (max 16)
";

/// HELP_TABLE -- the aligned rows of `HELP`: the syntax column is the longest syntax (28) plus
/// two wide, then ` - `; every dash is in the same column.
const HELP_TABLE: &str = "  HELP {COMMAND...}              - List the commands, or describe one
  ? {COMMAND...}                 - Same as HELP
  INFO                           - Firmware name, version, Tier and protocol version
  STATUS                         - IPMB address, GA, FRU state, payload power, health, NVMRO, uptime
  FRU STATE                      - Operational state of FRU 0 (M0, M1, M4, M5 or M6)
  FRU FIELDS                     - Every FRU field: current value and maximum length
  FRU GET [FIELD]                - One FRU field
  FRU SET [FIELD] {VALUE...}     - Write a FRU field; no value clears it; MFG_DATE as YYYY-MM-DD HH:MM
  FRU SAVE                       - Whether the FRU image is in flash (a change saves itself after the save delay)
  IPMI [NETFN] [CMD] {DATA...}   - Run one IPMI request locally (hex bytes); nothing goes on the IPMB
";

/// REQUEST_TABLE -- (request line, answer) on a fresh `FakeHost`: FRU state M4, power on, the
/// built FRU image, IPMI answering CC 00h with data 20h 01h 02h and the name `App Get Device
/// ID`, save state Saved, uptime 1068 s.
const REQUEST_TABLE: &[(&str, &str)] = &[
    // INFO, STATUS, FRU STATE
    (
        "INFO",
        "FIRMWARE: {fw}\nVERSION: 0.1.0\nTIER: {tier}\nPROTOCOL: 2\nOK\n",
    ),
    (
        "STATUS",
        "IPMB_ADDRESS: 82h\nGA: 1\nFRU_STATE: M4 FRU Active\nPAYLOAD_POWER: on\nHEALTH: ok\nNVMRO: deasserted\nUPTIME: 0d 00:17:48\nOK\n",
    ),
    ("FRU STATE", "FRU_STATE: M4 FRU Active\nOK\n"),
    ("fru   state", "FRU_STATE: M4 FRU Active\nOK\n"),
    // FRU GET
    ("FRU GET BOARD_SERIAL", "BOARD_SERIAL: IK-T1-000001\nOK\n"),
    ("fru get board_part", "BOARD_PART: IK-IPMC-T1-NUC\nOK\n"),
    ("FRU GET ASSET_TAG", "ASSET_TAG: \nOK\n"),
    ("FRU GET MFG_DATE", "MFG_DATE: 2026-10-23 00:00\nOK\n"),
    (
        "FRU GET PRODUCT_NAME",
        "PRODUCT_NAME: Tier-1 IPMC Reference\nOK\n",
    ),
    (
        "FRU GET NOPE",
        "ERROR ARGS: unknown field 'NOPE'; FRU FIELDS lists them\n",
    ),
    ("FRU GET", "ERROR ARGS: usage: FRU GET [FIELD]\n"),
    (
        "FRU GET BOARD_PART X",
        "ERROR ARGS: usage: FRU GET [FIELD]\n",
    ),
    // FRU SET
    (
        "FRU SET BOARD_SERIAL 00:11:22:33:44:55",
        "BOARD_SERIAL: 00:11:22:33:44:55\nOK\n",
    ),
    (
        "fru set product_serial 00:11:22:33:44:55",
        "PRODUCT_SERIAL: 00:11:22:33:44:55\nOK\n",
    ),
    (
        "FRU SET BOARD_PART PN-0001 rev B",
        "BOARD_PART: PN-0001 rev B\nOK\n",
    ),
    (
        "FRU SET ASSET_TAG rack 7  slot 3  ",
        "ASSET_TAG: rack 7  slot 3\nOK\n",
    ),
    ("FRU SET\tASSET_TAG\tabc", "ASSET_TAG: abc\nOK\n"),
    (
        "FRU SET MFG_DATE 2026-10-25 08:30",
        "MFG_DATE: 2026-10-25 08:30\nOK\n",
    ),
    (
        "FRU SET MFG_DATE 9999-12-31 23:59",
        "MFG_DATE: 9999-12-31 23:59\nOK\n",
    ),
    // No value clears the date; minute 0 (1996-01-01 00:00) is "unspecified", not a date.
    ("FRU SET MFG_DATE", "MFG_DATE: unspecified\nOK\n"),
    ("FRU SET MFG_DATE   ", "MFG_DATE: unspecified\nOK\n"),
    (
        "FRU SET MFG_DATE 1996-01-01 00:00",
        "ERROR BAD_DATE: use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59\n",
    ),
    (
        "FRU SET MFG_DATE 1996-01-01 00:01",
        "MFG_DATE: 1996-01-01 00:01\nOK\n",
    ),
    ("FRU SET BOARD_FILE_ID fid-9", "BOARD_FILE_ID: fid-9\nOK\n"),
    ("FRU SET BOARD_PART", "BOARD_PART: \nOK\n"),
    ("FRU SET BOARD_PART   ", "BOARD_PART: \nOK\n"),
    (
        "FRU SET ASSET_TAG X",
        "ERROR BAD_LENGTH: ASSET_TAG is empty or at least 2 characters\n",
    ),
    (
        "FRU SET BOARD_SERIAL 1234567890123456789012345",
        "ERROR TOO_LONG: BOARD_SERIAL is at most 24 characters\n",
    ),
    (
        "FRU SET ASSET_TAG caf\u{e9}",
        "ERROR NOT_ASCII: ASSET_TAG takes printable ASCII only\n",
    ),
    (
        "FRU SET MFG_DATE yesterday",
        "ERROR BAD_DATE: use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59\n",
    ),
    (
        "FRU SET MFG_DATE 2026-02-30 00:00",
        "ERROR BAD_DATE: use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59\n",
    ),
    ("FRU SET", "ERROR ARGS: usage: FRU SET [FIELD] {VALUE...}\n"),
    (
        "FRU SET NOPE x",
        "ERROR ARGS: unknown field 'NOPE'; FRU FIELDS lists them\n",
    ),
    // FRU FIELDS and FRU SAVE
    ("FRU FIELDS", "{fields}OK\n"),
    ("FRU FIELDS X", "ERROR ARGS: usage: FRU FIELDS\n"),
    ("FRU SAVE", "SAVE: done\nOK\n"),
    // IPMI
    (
        "IPMI 06 01",
        "CC: 00h\nDATA: {devid}\nCOMMAND: App Get Device ID\nOK\n",
    ),
    (
        "ipmi 0x06 0x01 ff",
        "CC: 00h\nDATA: {devid}\nCOMMAND: App Get Device ID\nOK\n",
    ),
    (
        "IPMI 6 1",
        "CC: 00h\nDATA: {devid}\nCOMMAND: App Get Device ID\nOK\n",
    ),
    (
        "IPMI 06",
        "ERROR ARGS: usage: IPMI [NETFN] [CMD] {DATA...}\n",
    ),
    ("IPMI", "ERROR ARGS: usage: IPMI [NETFN] [CMD] {DATA...}\n"),
    ("IPMI 06 zz", "ERROR ARGS: 'zz' is not a hex byte\n"),
    ("IPMI 06 01 100", "ERROR ARGS: '100' is not a hex byte\n"),
    // HELP, ?
    ("HELP", "{help}OK\n"),
    ("help", "{help}OK\n"),
    ("?", "{help}OK\n"),
    (
        "HELP INFO",
        "  INFO                           - Firmware name, version, Tier and protocol version\nOK\n",
    ),
    (
        "help fru set",
        "  FRU SET [FIELD] {VALUE...}     - Write a FRU field; no value clears it; MFG_DATE as YYYY-MM-DD HH:MM\nOK\n",
    ),
    (
        "HELP FRU SET BOARD_SERIAL",
        "  FRU SET [FIELD] {VALUE...}     - Write a FRU field; no value clears it; MFG_DATE as YYYY-MM-DD HH:MM\nOK\n",
    ),
    ("HELP ?", "  ? {COMMAND...}                 - Same as HELP\nOK\n"),
    (
        "HELP NOPE",
        "ERROR UNKNOWN: no command 'NOPE'; type HELP or ?\n",
    ),
    // `? <words>` is `HELP <words>` (MenuBuilder section 9).
    ("? X", "ERROR UNKNOWN: no command 'X'; type HELP or ?\n"),
    (
        "? fru set",
        "  FRU SET [FIELD] {VALUE...}     - Write a FRU field; no value clears it; MFG_DATE as YYYY-MM-DD HH:MM\nOK\n",
    ),
    (
        "? INFO",
        "  INFO                           - Firmware name, version, Tier and protocol version\nOK\n",
    ),
    // unknown commands and empty requests
    (
        "BOGUS x y",
        "ERROR UNKNOWN: unknown command 'BOGUS'; type HELP or ?\n",
    ),
    (
        "bogus",
        "ERROR UNKNOWN: unknown command 'bogus'; type HELP or ?\n",
    ),
    (
        "FRU",
        "ERROR UNKNOWN: unknown command 'FRU'; type HELP or ?\n",
    ),
    (
        "  FRU  BOGUS x",
        "ERROR UNKNOWN: unknown command 'FRU  BOGUS'; type HELP or ?\n",
    ),
    (
        "FRUSET BOARD_PART",
        "ERROR UNKNOWN: unknown command 'FRUSET'; type HELP or ?\n",
    ),
    (
        "INFOX",
        "ERROR UNKNOWN: unknown command 'INFOX'; type HELP or ?\n",
    ),
    ("INFO X", "ERROR ARGS: usage: INFO\n"),
    ("", "ERROR ARGS: empty request; type HELP or ?\n"),
    // tags
    (
        "#a1 INFO",
        "FIRMWARE: {fw}\nVERSION: 0.1.0\nTIER: {tier}\nPROTOCOL: 2\nOK #a1\n",
    ),
    (
        "#ABCDEFGH fru state",
        "FRU_STATE: M4 FRU Active\nOK #ABCDEFGH\n",
    ),
    ("#a7 FRU SAVE", "SAVE: done\nOK #a7\n"),
    (
        "#a1 BOGUS",
        "ERROR #a1 UNKNOWN: unknown command 'BOGUS'; type HELP or ?\n",
    ),
    ("#a1", "ERROR #a1 ARGS: empty request; type HELP or ?\n"),
    (
        "#a7 FRU SET BOARD_SERIAL 1234567890123456789012345",
        "ERROR #a7 TOO_LONG: BOARD_SERIAL is at most 24 characters\n",
    ),
    (
        "#abcdefghi INFO",
        "ERROR ARGS: a tag is # and 1 to 8 letters or digits\n",
    ),
    (
        "#a-1 INFO",
        "ERROR ARGS: a tag is # and 1 to 8 letters or digits\n",
    ),
    (
        "# INFO",
        "ERROR ARGS: a tag is # and 1 to 8 letters or digits\n",
    ),
];

#[test]
fn every_request_row_gets_its_answer() {
    for (request, want) in REQUEST_TABLE {
        let mut host = FakeHost::new();
        assert_eq!(
            reply(&mut host, request),
            expected(want),
            "request {request:?}"
        );
    }
}

#[test]
fn the_help_table_text_is_exact_and_aligned() {
    // Every dash is in the same column: 2 + the longest syntax (28) + 2.
    let width = COMMANDS.iter().map(|r| r.syntax.len()).max().unwrap();
    assert_eq!(width, 28);
    for line in HELP_TABLE.lines() {
        assert_eq!(line.find(" - "), Some(2 + width + 2), "{line}");
    }
    assert_eq!(HELP_TABLE.lines().count(), COMMANDS.len());
    // The text is generated from the table: a row's syntax and help sentence are in its line.
    for (line, command) in HELP_TABLE.lines().zip(COMMANDS) {
        assert_eq!(line, format!("  {:<30} - {}", command.syntax, command.help));
    }
}

// ---- the engine ----

#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn no_op(_: &Args<'_>, _: &mut dyn AppHost, _: &mut dyn Write) -> Result<(), Fail> {
    Ok(())
}

fn row(syntax: &'static str) -> Row {
    Row {
        syntax,
        help: "h",
        handler: no_op,
    }
}

#[test]
fn the_command_table_is_well_formed() {
    assert_eq!(check_table(COMMANDS), Ok(()));
}

#[test]
fn a_malformed_syntax_string_is_rejected_with_its_rule() {
    let rows = [
        ("", SyntaxError::Empty),
        ("   ", SyntaxError::Empty),
        ("[NAME]", SyntaxError::NoKeyword),
        ("{NAME}", SyntaxError::NoKeyword),
        ("fru set", SyntaxError::BadKeyword),
        ("SET-MODE", SyntaxError::BadKeyword),
        ("SET [name]", SyntaxError::BadParameter),
        ("SET []", SyntaxError::BadParameter),
        ("SET [NAME", SyntaxError::BadParameter),
        ("SET NAME]", SyntaxError::BadParameter),
        ("SET {NAME", SyntaxError::BadParameter),
        ("SET [NAME...]", SyntaxError::BadParameter),
        ("SET {...}", SyntaxError::BadParameter),
        ("SET [A] KEY", SyntaxError::KeywordAfterParameter),
        ("SET {A} [B]", SyntaxError::RequiredAfterOptional),
        ("SET {A...} [B]", SyntaxError::RestNotLast),
        ("SET {A...} {B}", SyntaxError::RestNotLast),
        ("SET [A] [B] [C] [D] [E]", SyntaxError::TooManyParameters),
    ];
    for (text, error) in rows {
        assert_eq!(Syntax::parse(text), Err(error), "{text:?}");
    }
    // Well formed strings and their shapes: (keywords, required, optional, rest).
    let good = [
        ("?", (1, 0, 0, false)),
        ("FRU SET [FIELD] {VALUE...}", (2, 1, 0, true)),
        ("A [X] [Y] {Z} {W}", (1, 2, 2, false)),
        ("A {REST...}", (1, 0, 0, true)),
    ];
    for (text, (keywords, required, optional, rest)) in good {
        let want = Syntax {
            keywords,
            required,
            optional,
            rest,
        };
        assert_eq!(Syntax::parse(text), Ok(want), "{text:?}");
    }
}

#[test]
fn a_bad_table_is_rejected_for_a_bad_row_a_missing_help_and_a_collision() {
    assert_eq!(
        check_table(&[row("A"), row("b")]),
        Err(TableError::Syntax(1, SyntaxError::BadKeyword))
    );
    let mut silent = row("A");
    silent.help = "";
    assert_eq!(check_table(&[silent]), Err(TableError::NoHelp(0)));
    // Same keyword sequence: parameters do not tell rows apart.
    assert_eq!(
        check_table(&[row("A B"), row("C"), row("A B [X]")]),
        Err(TableError::Collision(0, 2))
    );
    // A shorter keyword sequence is not a collision: the longest one wins.
    assert_eq!(check_table(&[row("A"), row("A B"), row("A B C")]), Ok(()));
}

#[test]
fn the_longest_keyword_sequence_wins_and_keywords_are_whole_words() {
    let rows = [row("A"), row("A B"), row("A B C [X]"), row("D")];
    let cases = [
        ("a", Some("A")),
        ("A x", Some("A")),
        ("A b", Some("A B")),
        ("a B c 1", Some("A B C [X]")),
        ("A BC", Some("A")),
        ("AB", None),
        ("A  \t B", Some("A B")),
        ("", None),
        ("E", None),
    ];
    for (line, want) in cases {
        let found = find(&rows, line).map(|m| m.row.syntax);
        assert_eq!(found, want, "{line:?}");
    }
    // The tail is what follows the keywords.
    assert_eq!(find(&rows, "A B  rest of it").unwrap().tail, " rest of it");
}

#[test]
fn arguments_split_into_words_and_a_rest_value() {
    let shape = Syntax::parse("K [A] {B} {C...}").unwrap();
    let cases: [(&str, Option<[Option<&str>; 3]>); 7] = [
        ("1", Some([Some("1"), None, None])),
        ("1 2", Some([Some("1"), Some("2"), None])),
        ("1 2 3", Some([Some("1"), Some("2"), Some("3")])),
        (
            "  1  2   3  4 \t 5  ",
            Some([Some("1"), Some("2"), Some("3  4 \t 5")]),
        ),
        ("", None),
        ("   ", None),
        ("1 2 3 4", Some([Some("1"), Some("2"), Some("3 4")])),
    ];
    for (tail, want) in cases {
        let args = Args::parse(&shape, tail);
        match want {
            Some(values) => {
                let args = args.unwrap_or_else(|| panic!("{tail:?}"));
                let got: Vec<Option<&str>> = (0..3).map(|i| args.get(i)).collect();
                assert_eq!(got, values, "{tail:?}");
            }
            None => assert!(args.is_none(), "{tail:?}"),
        }
    }
    // Without a rest parameter a surplus word is refused; blanks at the end are not a word.
    let plain = Syntax::parse("K [A] {B}").unwrap();
    let counts = [
        ("1 2", true),
        ("1 2   ", true),
        ("1 2 3", false),
        ("", false),
    ];
    for (tail, fits) in counts {
        assert_eq!(Args::parse(&plain, tail).is_some(), fits, "{tail:?}");
    }
    // The index past the parameters, and a missing optional one, are values and not panics.
    let args = Args::parse(&plain, "1").unwrap();
    assert_eq!(args.get(7), None);
    assert!(args.required(1).is_err());
}

#[test]
fn a_failing_handler_takes_back_its_data_lines() {
    #[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
    fn partial(_: &Args<'_>, _: &mut dyn AppHost, out: &mut dyn Write) -> Result<(), Fail> {
        write!(out, "KEY: value\r\n")?;
        Err(Fail::new("BROKEN", format_args!("it stopped")))
    }
    let rows = [Row {
        syntax: "GO",
        help: "h",
        handler: partial,
    }];
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    out.write_str("echo\r\n").unwrap();
    run(&rows, "#t1 GO", &mut FakeHost::new(), &mut out);
    assert_eq!(out.as_str(), "echo\r\nERROR #t1 BROKEN: it stopped\r\n");
}

#[test]
fn an_answer_that_does_not_fit_is_replaced_by_answer_too_long() {
    let mut host = FakeHost::new();
    // The HELP table needs about 1 KB; a 200 byte buffer cannot hold it.
    let mut small = AnswerBuf::<200>::new();
    run(COMMANDS, "#x HELP", &mut host, &mut small);
    assert_eq!(
        small.as_str(),
        format!("ERROR #x ANSWER_TOO_LONG: the answer is longer than {ANSWER_MAX} bytes\r\n")
    );
    assert!(!small.overflowed());
    let mut tiny = AnswerBuf::<10>::new();
    assert!(write_help(COMMANDS, None, &mut tiny).is_err());
}

#[test]
fn a_failure_sentence_keeps_the_pieces_written_before_it_ran_out_of_room() {
    let long = "X".repeat(FAIL_TEXT_MAX);
    let fail = Fail::new("CODE", format_args!("before {long}"));
    assert_eq!(fail.text(), "before ");
    assert_eq!(fail.code(), "CODE");
}

// ---- the commands ----

#[test]
fn ipmi_passes_the_bytes_to_the_host() {
    let mut host = FakeHost::new();
    reply(&mut host, "IPMI 0x06 0x01 ff 7 00");
    assert_eq!(host.ipmi_calls, [(0x06, 0x01, vec![0xFF, 0x07, 0x00])]);
}

#[test]
fn ipmi_reports_a_completion_code_without_data_without_a_name_and_a_failed_request() {
    let mut host = FakeHost::new();
    host.ipmi_result = Ok((0xC1, vec![]));
    host.name = None;
    assert_eq!(reply(&mut host, "IPMI 06 99"), "CC: C1h\nDATA: \nOK\n");
    host.ipmi_result = Err(AppError::Dispatch);
    assert_eq!(
        reply(&mut host, "IPMI 07 01"),
        "ERROR IPMI_FAILED: the dispatcher gave no response (a NetFn that is odd or above 3Eh)\n"
    );
    host.ipmi_result = Err(AppError::TooLong);
    assert_eq!(
        reply(&mut host, "IPMI 06 01"),
        "ERROR IPMI_FAILED: the request or its response does not fit one IPMB frame\n"
    );
    // More bytes than one IPMB frame holds are refused before the host runs.
    let long = format!(
        "IPMI 06 01{}",
        " 00".repeat(ipmc_core::ipmb::IPMB_MAX_LEN + 1)
    );
    host.ipmi_calls.clear();
    assert_eq!(
        reply(&mut host, &long),
        format!(
            "ERROR ARGS: more than {} data bytes\n",
            ipmc_core::ipmb::IPMB_MAX_LEN
        )
    );
    assert!(host.ipmi_calls.is_empty());
}

#[test]
fn status_words_follow_the_power_and_health_states() {
    struct Case(PowerState, Summary, &'static str, &'static str);
    let cases = [
        Case(PowerState::Off, Summary::Good, "off", "ok"),
        Case(PowerState::PowerOnRequested, Summary::Good, "off", "ok"),
        Case(PowerState::On, Summary::Bad, "on", "fault"),
        Case(PowerState::Fault, Summary::Unavailable, "fault", "fault"),
    ];
    for Case(power, health, power_word, health_word) in cases {
        let mut host = FakeHost::new();
        (host.power, host.health, host.nvmro, host.state) = (power, health, true, State::M0);
        assert_eq!(
            reply(&mut host, "STATUS"),
            format!(
                "IPMB_ADDRESS: 82h\nGA: 1\nFRU_STATE: M0 IPMC Inactive\nPAYLOAD_POWER: {power_word}\nHEALTH: {health_word}\nNVMRO: asserted\nUPTIME: 0d 00:17:48\nOK\n"
            )
        );
    }
}

#[test]
fn the_uptime_has_days_hours_minutes_and_seconds() {
    // (seconds, text): 1 d 1 h 1 min 1 s, and 100 days.
    for (seconds, text) in [(90_061_u32, "1d 01:01:01"), (8_640_000, "100d 00:00:00")] {
        struct Clock(u32);
        impl AppHost for Clock {
            fn version(&self) -> &'static str {
                ""
            }
            fn status(&self) -> Status {
                Status {
                    uptime_s: self.0,
                    ..FakeHost::new().status()
                }
            }
            fn fru_state(&self) -> State {
                State::M4
            }
            fn unit_field(
                &self,
                _: FruField,
                _: &mut dyn core::fmt::Write,
            ) -> Result<(), FieldError> {
                Ok(())
            }
            fn set_unit_field(&mut self, _: FruField, _: &str) -> Result<(), FieldError> {
                Ok(())
            }
            fn ipmi(&mut self, _: u8, _: u8, _: &[u8]) -> Result<(u8, IpmiData), AppError> {
                Err(AppError::Dispatch)
            }
            fn command_name(&self, _: u8, _: u8) -> Option<&'static str> {
                None
            }
            fn save_state(&self) -> SaveState {
                SaveState::Saved
            }
        }
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        run(COMMANDS, "STATUS", &mut Clock(seconds), &mut out);
        assert!(out.as_str().contains(&format!("UPTIME: {text}\r\n")));
    }
}

#[test]
fn fru_save_answers_each_state() {
    let rows = [
        (SaveState::Saved, "SAVE: done\nOK\n"),
        (SaveState::Pending, "SAVE: pending\nOK\n"),
        (
            SaveState::LogOff,
            "ERROR LOG_OFF: the flash log is off; the FRU image lives in RAM only\n",
        ),
    ];
    for (state, want) in rows {
        let mut host = FakeHost::new();
        host.save = state;
        assert_eq!(reply(&mut host, "FRU SAVE"), want);
    }
}

#[test]
fn every_field_error_has_its_error_code_and_sentence() {
    let rows = [
        (
            FieldError::TooLong,
            "ERROR TOO_LONG: BOARD_SERIAL is at most 24 characters\n",
        ),
        (
            FieldError::BadLength,
            "ERROR BAD_LENGTH: BOARD_SERIAL is empty or at least 2 characters\n",
        ),
        (
            FieldError::NotAscii,
            "ERROR NOT_ASCII: BOARD_SERIAL takes printable ASCII only\n",
        ),
        (
            FieldError::NoRoom,
            "ERROR NO_ROOM: the FRU area has no room for that BOARD_SERIAL\n",
        ),
        (
            FieldError::Nvmro,
            "ERROR NVMRO: the FRU is write protected (NVMRO is asserted)\n",
        ),
        (
            FieldError::BadDate,
            "ERROR BAD_DATE: use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59\n",
        ),
        (
            FieldError::Corrupt,
            "ERROR FRU_IMAGE: the FRU image is damaged (Corrupt)\n",
        ),
        (
            FieldError::AreaMissing,
            "ERROR FRU_IMAGE: the FRU image is damaged (AreaMissing)\n",
        ),
    ];
    for (error, want) in rows {
        let mut host = FakeHost::new();
        host.forced_set_error = Some(error);
        assert_eq!(
            reply(&mut host, "FRU SET BOARD_SERIAL ABCDEF"),
            want,
            "{error:?}"
        );
    }
}

#[test]
fn set_while_nvmro_is_asserted_is_refused_and_changes_nothing() {
    let mut host = FakeHost::new();
    host.nvmro = true;
    assert_eq!(
        reply(&mut host, "FRU SET BOARD_SERIAL ABCDEF"),
        "ERROR NVMRO: the FRU is write protected (NVMRO is asserted)\n"
    );
    host.nvmro = false;
    assert_eq!(
        reply(&mut host, "FRU GET BOARD_SERIAL"),
        "BOARD_SERIAL: IK-T1-000001\nOK\n"
    );
}

#[test]
fn set_is_refused_when_writing_is_disabled() {
    let mut host = FakeHost::new();
    let shape = Syntax::parse("FRU SET [FIELD] {VALUE...}").unwrap();
    let args = Args::parse(&shape, "ASSET_TAG abc").unwrap();
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    let fail = set_field(&args, &mut host, &mut out, false).unwrap_err();
    assert_eq!(fail.code(), "WRITE_DISABLED");
    assert_eq!(
        fail.text(),
        "FRU writes are turned off in this build (IPMC_APP_FRU_WRITE is false)"
    );
    assert!(out.is_empty());
    assert_eq!(reply(&mut host, "FRU GET ASSET_TAG"), "ASSET_TAG: \nOK\n");
}

#[test]
fn a_set_value_is_stored_and_marks_the_store_dirty() {
    let mut host = FakeHost::new();
    assert!(!host.store.is_dirty());
    assert_eq!(
        reply(&mut host, "FRU SET BOARD_SERIAL 00:11:22:33:44:55"),
        "BOARD_SERIAL: 00:11:22:33:44:55\nOK\n"
    );
    assert!(host.store.is_dirty());
    assert_eq!(
        reply(&mut host, "FRU GET BOARD_SERIAL"),
        "BOARD_SERIAL: 00:11:22:33:44:55\nOK\n"
    );
    // The other serial is untouched.
    assert_eq!(
        reply(&mut host, "FRU GET PRODUCT_SERIAL"),
        "PRODUCT_SERIAL: IK-T1-000001\nOK\n"
    );
    assert_eq!(ipmc_fru::verify(host.store.bytes()), Ok(()));
}

#[test]
fn ipmi_write_fru_data_is_refused_when_writing_is_disabled() {
    let mut host = FakeHost::new();
    // (request, runs while writing is disabled): Write FRU Data (0Ah 12h) is refused; Get Device
    // ID, Get FRU Inventory Area Info and Chassis Control still run.
    let cases = [
        ("0A 12 00 00 00", false),
        ("0a 12", false),
        ("06 01", true),
        ("0A 10 00", true),
        ("00 08 00", true),
    ];
    let shape = Syntax::parse("IPMI [NETFN] [CMD] {DATA...}").unwrap();
    for (tail, runs) in cases {
        let args = Args::parse(&shape, tail).unwrap();
        host.ipmi_calls.clear();
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        let result = ipmi_request(&args, &mut host, &mut out, false);
        assert_eq!(host.ipmi_calls.len(), usize::from(runs), "{tail}");
        if !runs {
            assert_eq!(result.unwrap_err().code(), "WRITE_DISABLED");
        }
        // With the permission every request runs.
        host.ipmi_calls.clear();
        ipmi_request(&args, &mut host, &mut out, true).unwrap();
        assert_eq!(host.ipmi_calls.len(), 1, "{tail}");
    }
}

// ---- the line editor ----

/// typed -- feeds `bytes` to an editor of `N` bytes, answers each finished line on a fresh host
/// and returns everything the device sends back, banner and prompts included.
fn typed<const N: usize>(bytes: &[u8]) -> String {
    let mut editor = LineEditor::<N>::new();
    let mut host = FakeHost::new();
    let mut out = AnswerBuf::<{ ANSWER_MAX * 4 }>::new();
    banner(&host, &mut out).unwrap();
    for byte in bytes {
        serve(&mut editor, *byte, &mut host, &mut out);
    }
    assert!(!out.overflowed());
    out.as_str().to_string()
}

/// BAD_CHAR_LINE -- the final line of a line with a byte that is not printable ASCII.
const BAD_CHAR_LINE: &str =
    "ERROR BAD_CHAR: the line holds a byte that is not printable ASCII; it was dropped";

#[test]
fn the_editor_echoes_erases_and_ends_lines() {
    // (bytes sent, what the device sends back after the banner and the first prompt). BS is 08h
    // and the device answers BS SP BS.
    let echoes: [(&[u8], String); 15] = [
        (b"IN", "IN".into()),
        (b"\t", "\t".into()),
        (b"INFO\x08", "INFO\x08 \x08".into()),
        (b"INFO\x7f\x7f", "INFO\x08 \x08\x08 \x08".into()),
        (b"\x08\x7f", "".into()),
        (b"\r", "\r\napp> ".into()),
        (b"\n", "\r\napp> ".into()),
        (b"\r\n", "\r\napp> ".into()),
        (b"\n\r", "\r\napp> ".into()),
        (b"\r\r", "\r\napp> \r\napp> ".into()),
        (b"\n\n", "\r\napp> \r\napp> ".into()),
        (b"A\x00B\r", format!("AB\r\n{BAD_CHAR_LINE}\r\napp> ")),
        (b"\xc3\xa9\n", format!("\r\n{BAD_CHAR_LINE}\r\napp> ")),
        (b"\x1b[A\n", format!("[A\r\n{BAD_CHAR_LINE}\r\napp> ")),
        (
            b"x\r\n",
            "x\r\nERROR UNKNOWN: unknown command 'x'; type HELP or ?\r\napp> ".into(),
        ),
    ];
    let banner_text = {
        let host = FakeHost::new();
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        banner(&host, &mut out).unwrap();
        out.as_str().to_string()
    };
    for (bytes, want) in echoes {
        assert_eq!(
            typed::<32>(bytes),
            format!("{banner_text}{want}"),
            "{bytes:?}"
        );
    }
}

#[test]
fn backspace_edits_the_line_before_it_runs() {
    // "INFX" BS "O" is "INFO"; the answer is INFO's.
    let text = typed::<32>(b"INFX\x08O\r\n");
    assert!(text.contains("INFX\x08 \x08O\r\n"), "{text:?}");
    assert!(text.contains("PROTOCOL: 2\r\nOK\r\napp> "), "{text:?}");
    // Erasing everything and typing again.
    let text = typed::<32>(b"AB\x08\x08STATUS\r");
    assert!(text.contains("IPMB_ADDRESS: 82h"), "{text:?}");
}

#[test]
fn every_line_end_form_is_one_line() {
    // (bytes, prompts printed): the banner's prompt, the one after the answer, and one more for
    // each empty line. A CR LF pair adds no empty line; CR LF CR LF is a line and an empty line.
    let forms: [(&[u8], usize); 5] = [
        (b"FRU STATE\r\n", 2),
        (b"FRU STATE\n\r", 2),
        (b"FRU STATE\r", 2),
        (b"FRU STATE\n", 2),
        (b"FRU STATE\r\n\r\n", 3),
    ];
    for (bytes, prompts) in forms {
        let text = typed::<32>(bytes);
        assert_eq!(
            text.matches("FRU_STATE: M4 FRU Active").count(),
            1,
            "{bytes:?}"
        );
        assert_eq!(text.matches(PROMPT).count(), prompts, "{bytes:?}");
    }
}

#[test]
fn the_editor_gives_the_same_output_for_any_split_of_the_bytes() {
    let whole: &[u8] = b"INFO\r\nFRU STATE\nFRU SAVE\r\n";
    let joined = typed::<32>(whole);
    assert_eq!(joined.matches("OK\r\n").count(), 3);
    for cut in 0..=whole.len() {
        let mut editor = LineEditor::<32>::new();
        let mut host = FakeHost::new();
        let mut out = AnswerBuf::<{ ANSWER_MAX * 4 }>::new();
        banner(&host, &mut out).unwrap();
        for chunk in [&whole[..cut], &whole[cut..]] {
            for byte in chunk {
                serve(&mut editor, *byte, &mut host, &mut out);
            }
        }
        assert_eq!(out.as_str(), joined, "cut {cut}");
    }
}

#[test]
fn an_overlong_line_is_answered_once_when_its_end_arrives() {
    // N = 8: a line of 8 bytes passes, 9 do not; the bytes past 8 are not echoed.
    let text = typed::<8>(b"STATUS\r\n");
    assert!(text.contains("IPMB_ADDRESS: 82h"), "{text:?}");
    let text = typed::<8>(b"HELP     \nINFO\n");
    let error = format!(
        "ERROR LINE_TOO_LONG: the line is longer than {} bytes; it was dropped\r\n",
        ipmc_defs::IPMC_APP_LINE_MAX
    );
    assert!(
        text.contains(&format!("HELP    \r\n{error}app> ")),
        "{text:?}"
    );
    assert!(text.contains("PROTOCOL: 2\r\nOK\r\napp> "), "{text:?}");
    // Nothing is answered before the end of the overlong line arrives; backspace does nothing on
    // it; the editor reports TooLong once.
    let mut editor = LineEditor::<4>::new();
    for byte in b"0123456789\x08\x08" {
        assert_eq!(editor.push(*byte).fed, Fed::Nothing);
    }
    assert_eq!(editor.push(b'\n').fed, Fed::TooLong);
    // A second line end alike is an empty line (only CR LF and LF CR are one line end).
    assert_eq!(editor.push(b'\n').fed, Fed::Empty);
}

#[test]
fn a_line_of_blanks_is_an_empty_line() {
    let mut editor = LineEditor::<32>::new();
    for byte in b"  \t  " {
        editor.push(*byte);
    }
    assert_eq!(editor.push(b'\r').fed, Fed::Empty);
    // The session shows only the prompt after it, as after an empty line.
    let text = typed::<32>(b"   \r\r");
    let prompt_only = format!("{PROMPT}   \r\n{PROMPT}\r\n{PROMPT}");
    assert!(text.ends_with(&prompt_only), "{text:?}");
    assert!(!text.contains("ERROR"), "{text:?}");
}

#[test]
fn the_editor_returns_the_text_of_the_line() {
    let mut editor = LineEditor::<32>::new();
    for byte in b"FRU GET ASSET_TAG" {
        assert_eq!(editor.push(*byte).fed, Fed::Nothing);
    }
    assert_eq!(editor.push(b'\r').fed, Fed::Line("FRU GET ASSET_TAG"));
    assert_eq!(editor.push(b'\n').fed, Fed::Nothing);
    for byte in b"INFO" {
        editor.push(*byte);
    }
    let edit = editor.push(b'\n');
    assert_eq!((edit.echo, edit.fed), (Echo::Newline, Fed::Line("INFO")));
}

/// feed_all -- pushes every byte of `bytes` through `serve` and collects the final line of each
/// finished answer.
fn feed_all<const N: usize>(
    editor: &mut LineEditor<N>,
    host: &mut FakeHost,
    bytes: &[u8],
) -> Vec<String> {
    let mut answers = Vec::new();
    for byte in bytes {
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        let served = serve(editor, *byte, host, &mut out);
        if served.fed != Fed::Nothing {
            let line = answer_final_line(&out, served.answer_at).unwrap_or("(none)");
            answers.push(line.to_string());
        }
    }
    answers
}

#[test]
fn a_damaged_line_is_answered_without_a_tag_and_not_run() {
    let mut host = FakeHost::new();
    let mut editor = LineEditor::<64>::new();
    // Bytes of a request arrived, then the port lost 3 and the ring was discarded.
    assert!(feed_all(&mut editor, &mut host, b"#a7 FRU SET ASSET_").is_empty());
    editor.damage(3);
    // The rest of the line ends: answered once, no tag, not run; the CR LF pair adds nothing.
    let answers = feed_all(&mut editor, &mut host, b"TAG abc\r\n");
    assert_eq!(
        answers,
        ["ERROR LINE_DAMAGED: 3 bytes lost on the port; the line was dropped"]
    );
    assert_eq!(reply(&mut host, "FRU GET ASSET_TAG"), "ASSET_TAG: \nOK\n");
    // The line after it runs normally.
    assert_eq!(feed_all(&mut editor, &mut host, b"FRU SAVE\r\n"), ["OK"]);
    // A loss that arrives before the line end adds up; a bare line end answers too.
    editor.damage(2);
    editor.damage(5);
    let answers = feed_all(&mut editor, &mut host, b"\n");
    assert_eq!(
        answers,
        ["ERROR LINE_DAMAGED: 7 bytes lost on the port; the line was dropped"]
    );
}

#[test]
fn many_requests_then_a_damaged_set_never_runs_the_set() {
    // A burst of complete requests overran the ring, the firmware discarded the ring (no answer
    // for the lines inside it), and the SET that follows lost bytes.
    let mut host = FakeHost::new();
    let mut editor = LineEditor::<64>::new();
    editor.damage(40);
    let answers = feed_all(&mut editor, &mut host, b"ASSET_TAG ABCD\r\n");
    assert_eq!(
        answers,
        ["ERROR LINE_DAMAGED: 40 bytes lost on the port; the line was dropped"]
    );
    let answers = feed_all(
        &mut editor,
        &mut host,
        b"FRU SET ASSET_TAG ABCD\r\nFRU GET ASSET_TAG\r\n",
    );
    assert_eq!(answers, ["OK", "OK"]);
    assert_eq!(
        reply(&mut host, "FRU GET ASSET_TAG"),
        "ASSET_TAG: ABCD\nOK\n"
    );
}

#[test]
fn every_line_state_and_host_failure_has_its_error_line() {
    let mut host = FakeHost::new();
    let states = [
        (Fed::TooLong, "ERROR LINE_TOO_LONG: "),
        (Fed::BadChar, "ERROR BAD_CHAR: "),
        (Fed::Damaged(2), "ERROR LINE_DAMAGED: 2 bytes"),
    ];
    for (fed, start) in states {
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        assert!(respond(&fed, &mut host, &mut out));
        assert!(out.as_str().starts_with(start), "{}", out.as_str());
        assert!(out.as_str().ends_with("\r\napp> "));
    }
    let forced = [
        (FieldError::NoRoom, "NO_ROOM"),
        (FieldError::Nvmro, "NVMRO"),
        (FieldError::Corrupt, "FRU_IMAGE"),
        (FieldError::BadDate, "BAD_DATE"),
    ];
    for (error, code) in forced {
        let mut host = FakeHost::new();
        host.forced_set_error = Some(error);
        let text = reply(&mut host, "FRU SET BOARD_PART ab");
        assert!(text.starts_with(&format!("ERROR {code}: ")), "{text}");
    }
    host.ipmi_result = Err(AppError::TooLong);
    assert!(reply(&mut host, "IPMI 06 01").starts_with("ERROR IPMI_FAILED: "));
    host.save = SaveState::LogOff;
    assert!(reply(&mut host, "FRU SAVE").starts_with("ERROR LOG_OFF: "));
}

#[test]
fn respond_writes_nothing_for_nothing_and_the_prompt_for_an_empty_line() {
    let mut host = FakeHost::new();
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    assert!(!respond(&Fed::Nothing, &mut host, &mut out));
    assert!(out.is_empty());
    assert!(respond(&Fed::Empty, &mut host, &mut out));
    assert_eq!(out.as_str(), PROMPT);
}

// ---- sizes and the parsing rule ----

#[test]
fn the_longest_answers_fit_the_answer_buffer_with_room_for_the_echo() {
    let mut host = FakeHost::new();
    // The echo of a full pass of typed bytes (3 bytes each at worst) comes before an answer, the
    // prompt and the line end after it.
    let echo = ipmc_defs::IPMC_APP_BYTES_PER_POLL * 3;
    let after = PROMPT.len() + 2;
    let longest = ["HELP", "FRU FIELDS", "#abcdefgh IPMI 06 01", "STATUS"]
        .iter()
        .map(|request| reply(&mut host, request).len())
        .max()
        .unwrap();
    assert!(
        longest + echo + after <= ANSWER_MAX,
        "{longest} + {echo} + {after} > {ANSWER_MAX}"
    );
    // An unknown command as long as a request line, tag included: the word is not cut.
    let word = "X".repeat(ipmc_defs::IPMC_APP_LINE_MAX - 10);
    let long = reply(&mut host, &format!("#abcdefgh {word}"));
    assert!(long.contains(&word));
    assert!(long.len() + echo + after <= ANSWER_MAX);
    // Every text field at 63 bytes (a host with long values): FRU FIELDS and FRU GET still fit,
    // and no line is cut.
    host.long_text = true;
    for request in ["#abcdefgh FRU FIELDS", "#abcdefgh FRU GET BOARD_MFR"] {
        let text = reply(&mut host, request);
        assert!(text.ends_with("OK #abcdefgh\n"), "{request}: {text}");
        assert!(text.len() + echo + after <= ANSWER_MAX, "{request}");
    }
    let fields = reply(&mut host, "FRU FIELDS");
    let value = "v".repeat(LONG_TEXT);
    assert_eq!(fields.matches(&format!(": {value} (max ")).count(), 12);
    host.long_text = false;
    // A full frame of response data.
    host.ipmi_result = Ok((0, vec![0xAB; ipmc_core::ipmb::IPMB_RESPONSE_DATA_MAX]));
    let full = reply(&mut host, "#abcdefgh IPMI 06 01");
    assert!(full.len() + echo + after <= ANSWER_MAX);
}

#[test]
fn the_answer_buffer_stores_whole_writes_only() {
    let mut buf = AnswerBuf::<8>::new();
    assert!(buf.is_empty());
    write!(buf, "abcd").unwrap();
    assert!(write!(buf, "efghi").is_err());
    assert!(buf.overflowed());
    assert_eq!(buf.as_str(), "abcd");
    write!(buf, "efgh").unwrap();
    assert_eq!(buf.as_str(), "abcdefgh");
    assert_eq!(buf.as_bytes(), b"abcdefgh");
    buf.truncate(3);
    assert_eq!((buf.as_str(), buf.len()), ("abc", 3));
    buf.truncate(9);
    assert_eq!(buf.len(), 3);
    buf.clear();
    assert!(buf.is_empty() && !buf.overflowed());
}

#[test]
fn the_payload_parsing_rule_finds_the_data_lines_and_the_final_line() {
    // Skip lines that start with the prompt (prompt and echo), collect KEY: value lines, the
    // answer ends at the first line that is OK / starts with OK or ERROR.
    let session = "app> STATUS\nGA: 1\nHEALTH: ok\nOK #t1\napp> FRU GET X\nERROR ARGS: bad\napp> ";
    let mut data = Vec::new();
    let mut last = None;
    for line in session.lines().filter(|l| !l.starts_with(PROMPT)) {
        if final_line(line).is_some() {
            last = Some(line);
            break;
        }
        data.push(line);
    }
    assert_eq!(data, ["GA: 1", "HEALTH: ok"]);
    assert_eq!(last, Some("OK #t1"));
    assert_eq!(
        final_line("GA: 1\nOKAY: 1\nERROR ARGS: x\n"),
        Some("ERROR ARGS: x")
    );
    assert_eq!(final_line("OK\n"), Some("OK"));
    assert_eq!(final_line("app> OK\n"), None);
}

// ---- a whole session ----

/// SESSION_INPUT -- what a person types for the session test: HELP, STATUS, FRU FIELDS, a FRU SET
/// that fails, an IPMI request, a request that works.
const SESSION_INPUT: &[u8] = b"HELP\rSTATUS\rFRU FIELDS\rFRU SET BOARD_SERIAL 1234567890123456789012345\rIPMI 06 01\rFRU SAVE\r";

#[test]
fn a_session_prints_as_the_documents_show_it() {
    let text = typed::<{ ipmc_defs::IPMC_APP_LINE_MAX }>(SESSION_INPUT).replace("\r\n", "\n");
    println!("{text}");
    let (fw, tier) = (ipmc_defs::IPMC_FW_NAME, ipmc_defs::IPMC_TIER);
    let devid = hex_text(&DEVICE_IDENTITY.response_data());
    let want = format!(
        "IPMC application port, {fw} v0.1.0, Tier {tier}, protocol 2. Type HELP or ?.\n\
         app> HELP\n{HELP_TABLE}OK\n\
         app> STATUS\n{}\
         app> FRU FIELDS\n{FIELD_LINES}OK\n\
         app> FRU SET BOARD_SERIAL 1234567890123456789012345\n\
         ERROR TOO_LONG: BOARD_SERIAL is at most 24 characters\n\
         app> IPMI 06 01\nCC: 00h\nDATA: {devid}\nCOMMAND: App Get Device ID\nOK\n\
         app> FRU SAVE\nSAVE: done\nOK\napp> ",
        "IPMB_ADDRESS: 82h\nGA: 1\nFRU_STATE: M4 FRU Active\nPAYLOAD_POWER: on\nHEALTH: ok\nNVMRO: deasserted\nUPTIME: 0d 00:17:48\nOK\n"
    );
    assert_eq!(text, want);
}

#[test]
fn the_session_in_the_crate_comment_is_what_the_code_prints() {
    let source = include_str!("lib.rs");
    let doc: Vec<&str> = source
        .lines()
        .skip_while(|line| !line.starts_with("//! IPMC application port"))
        .take_while(|line| *line != "//! ```")
        .map(|line| line.trim_start_matches("//! ").trim_end())
        .collect();
    let input = b"STATUS\r#a7 FRU SET BOARD_SERIAL 1234567890123456789012345\rFRU SET BOARD_SERIAL 00:11:22:33:44:55\rIPMI 06 01\r";
    let printed = typed::<{ ipmc_defs::IPMC_APP_LINE_MAX }>(input).replace("\r\n", "\n");
    // The comment shows the Tier-1 build; the banner of another build names its own Tier.
    let printed = printed.replace(&format!("Tier {},", ipmc_defs::IPMC_TIER), "Tier 1,");
    // So does the Get Device ID answer: the Additional Device Support byte (06h of the data) is
    // 09h in the Tier-1 build.
    let mut tier1 = DEVICE_IDENTITY.response_data();
    tier1[5] = additional_support(1);
    let printed = printed.replace(
        &hex_text(&DEVICE_IDENTITY.response_data()),
        &hex_text(&tier1),
    );
    let printed: Vec<&str> = printed.lines().map(str::trim_end).collect();
    assert_eq!(doc, printed);
}
