//! tests.rs - the host tests of the application protocol over a fake host.
//!
//! `FakeHost` implements `AppHost` over a real `FruStore`, so the field editor's own answers
//! (too long, one byte, not ASCII) come from the real code. `REQUEST_TABLE` is the one table of
//! request and answer pairs: a new command adds a row.

use core::fmt::Write;

use ipmc_core::fru_state::State;
use ipmc_core::sensor::{PowerState, Summary};
use ipmc_fru::{FieldError, FruField, FruStore};

use crate::commands::{ipmi_request, set_field};
use crate::{
    answer, respond, AnswerBuf, AppError, AppHost, Fed, IpmiData, LineAssembler, Request,
    SaveState, Status, ANSWER_MAX, COMMANDS,
};

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
            ipmi_result: Ok((0, vec![0x20, 0x01, 0x02])),
            ipmi_calls: Vec::new(),
        }
    }
}

impl AppHost for FakeHost {
    fn version(&self) -> &'static str {
        "9.9.9"
    }

    fn status(&self) -> Status {
        Status {
            ipmb_addr: 0x82,
            ga: 1,
            power: self.power,
            health: self.health,
            nvmro: self.nvmro,
            uptime_s: 77,
        }
    }

    fn fru_state(&self) -> State {
        self.state
    }

    fn unit_field(&self, f: FruField, out: &mut dyn core::fmt::Write) -> Result<(), FieldError> {
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

    fn save_state(&self) -> SaveState {
        self.save
    }
}

/// reply -- the answer to one request line.
fn reply(host: &mut FakeHost, line: &str) -> String {
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    answer(line, host, &mut out);
    assert!(!out.overflowed(), "{line:?} overflowed the answer buffer");
    out.as_str().to_string()
}

/// expected -- the answer text of a table row: `{fw}` and `{tier}` stand for the build values.
fn expected(text: &str) -> String {
    text.replace("{fw}", ipmc_defs::IPMC_FW_NAME)
        .replace("{tier}", &ipmc_defs::IPMC_TIER.to_string())
}

/// REQUEST_TABLE -- (request line, answer line) on a fresh `FakeHost`: FRU state M4, power on,
/// the built FRU image, IPMI answering CC 00h with data 20h 01h 02h, save state Saved.
const REQUEST_TABLE: &[(&str, &str)] = &[
    // HELP and HELLO
    (
        "HELP",
        "OK HELP HELP HELLO STATUS FRU GET SET FIELDS IPMI SAVE",
    ),
    (
        "help set",
        "OK HELP SET <field> [value] - write a FRU field; no value clears it",
    ),
    (
        "HELP STATUS",
        "OK HELP STATUS - IPMB address, GA, FRU state, power, health, NVMRO, uptime",
    ),
    ("HELP NOPE", "ERR UNKNOWN NOPE"),
    ("HELLO", "OK IPMC-APP 1 {fw} v9.9.9 tier={tier}"),
    ("hello", "OK IPMC-APP 1 {fw} v9.9.9 tier={tier}"),
    ("  \tHELLO", "OK IPMC-APP 1 {fw} v9.9.9 tier={tier}"),
    // tags
    ("#a1 HELLO", "#a1 OK IPMC-APP 1 {fw} v9.9.9 tier={tier}"),
    (
        "#ABCDEFGH status",
        "#ABCDEFGH OK STATUS ipmb=82h ga=1 fru=M4 power=on health=ok nvmro=0 uptime_s=77",
    ),
    ("#a1", "#a1 ERR ARGS empty request"),
    ("#a1 BOGUS", "#a1 ERR UNKNOWN BOGUS"),
    (
        "#abcdefghi HELLO",
        "ERR ARGS a tag is # and 1 to 8 letters or digits",
    ),
    (
        "#a-1 HELLO",
        "ERR ARGS a tag is # and 1 to 8 letters or digits",
    ),
    (
        "# HELLO",
        "ERR ARGS a tag is # and 1 to 8 letters or digits",
    ),
    // STATUS and FRU STATE
    (
        "STATUS",
        "OK STATUS ipmb=82h ga=1 fru=M4 power=on health=ok nvmro=0 uptime_s=77",
    ),
    ("FRU STATE", "OK FRU M4 FRU Active"),
    ("fru state", "OK FRU M4 FRU Active"),
    ("FRU", "ERR ARGS usage: FRU STATE"),
    ("FRU BOGUS", "ERR ARGS usage: FRU STATE"),
    ("FRU STATE X", "ERR ARGS usage: FRU STATE"),
    // GET
    ("GET BOARD_SERIAL", "OK BOARD_SERIAL IK-T1-000001"),
    ("get board_part", "OK BOARD_PART IK-IPMC-T1-NUC"),
    ("GET PRODUCT_PART", "OK PRODUCT_PART IK-IPMC-T1"),
    ("GET PRODUCT_SERIAL", "OK PRODUCT_SERIAL IK-T1-000001"),
    ("GET ASSET_TAG", "OK ASSET_TAG"),
    ("GET MFG_DATE", "OK MFG_DATE 2026-10-23 00:00"),
    ("GET BOARD_MFR", "OK BOARD_MFR IronKernel"),
    ("GET BOARD_PRODUCT", "OK BOARD_PRODUCT VPX Tier-1 IPMC"),
    ("GET BOARD_FILE_ID", "OK BOARD_FILE_ID ipmc-fru-v1"),
    ("GET PRODUCT_MFR", "OK PRODUCT_MFR IronKernel"),
    ("GET PRODUCT_NAME", "OK PRODUCT_NAME Tier-1 IPMC Reference"),
    ("GET PRODUCT_VERSION", "OK PRODUCT_VERSION 0.1"),
    ("GET PRODUCT_FILE_ID", "OK PRODUCT_FILE_ID"),
    (
        "GET NOPE",
        "ERR ARGS unknown field NOPE (FIELDS lists them)",
    ),
    ("GET", "ERR ARGS usage: GET <field>"),
    ("GET BOARD_PART X", "ERR ARGS usage: GET <field>"),
    // SET
    (
        "SET BOARD_SERIAL 00:11:22:33:44:55",
        "OK BOARD_SERIAL 00:11:22:33:44:55",
    ),
    (
        "set product_serial 00:11:22:33:44:55",
        "OK PRODUCT_SERIAL 00:11:22:33:44:55",
    ),
    (
        "SET BOARD_PART PN-0001 rev B",
        "OK BOARD_PART PN-0001 rev B",
    ),
    ("SET PRODUCT_PART MODEL-X", "OK PRODUCT_PART MODEL-X"),
    (
        "SET ASSET_TAG rack 7  slot 3",
        "OK ASSET_TAG rack 7  slot 3",
    ),
    ("SET\tASSET_TAG\tabc", "OK ASSET_TAG abc"),
    (
        "SET MFG_DATE 2026-10-25 08:30",
        "OK MFG_DATE 2026-10-25 08:30",
    ),
    ("SET BOARD_MFR Other Maker", "OK BOARD_MFR Other Maker"),
    ("SET PRODUCT_NAME New name", "OK PRODUCT_NAME New name"),
    ("SET PRODUCT_VERSION 2.1", "OK PRODUCT_VERSION 2.1"),
    ("SET BOARD_FILE_ID fid-9", "OK BOARD_FILE_ID fid-9"),
    ("SET PRODUCT_FILE_ID pfid-3", "OK PRODUCT_FILE_ID pfid-3"),
    (
        "SET MFG_DATE 2100-03-01 00:00",
        "OK MFG_DATE 2100-03-01 00:00",
    ),
    (
        "SET MFG_DATE 9999-12-31 23:59",
        "OK MFG_DATE 9999-12-31 23:59",
    ),
    ("SET BOARD_PART", "OK BOARD_PART"),
    ("SET BOARD_PART ", "OK BOARD_PART"),
    (
        "SET ASSET_TAG X",
        "ERR BAD_LENGTH a text of one byte is not allowed",
    ),
    (
        "SET ASSET_TAG 1234567890123456789012345",
        "ERR TOO_LONG max=24",
    ),
    (
        "SET ASSET_TAG caf\u{e9}",
        "ERR NOT_ASCII printable ASCII only",
    ),
    (
        "SET MFG_DATE yesterday",
        "ERR BAD_DATE use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:00 to 9999-12-31 23:59",
    ),
    (
        "SET MFG_DATE 2026-02-30 00:00",
        "ERR BAD_DATE use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:00 to 9999-12-31 23:59",
    ),
    ("SET", "ERR ARGS usage: SET <field> [value]"),
    (
        "SET NOPE x",
        "ERR ARGS unknown field NOPE (FIELDS lists them)",
    ),
    // IPMI
    ("IPMI 06 01", "OK IPMI CC=00 DATA=20 01 02"),
    ("ipmi 0x06 0x01 ff", "OK IPMI CC=00 DATA=20 01 02"),
    ("IPMI 6 1", "OK IPMI CC=00 DATA=20 01 02"),
    ("IPMI 06", "ERR ARGS usage: IPMI <netfn> <cmd> [data...]"),
    ("IPMI", "ERR ARGS usage: IPMI <netfn> <cmd> [data...]"),
    ("IPMI 06 zz", "ERR ARGS 'zz' is not a hex byte"),
    ("IPMI 06 100", "ERR ARGS '100' is not a hex byte"),
    // SAVE, and the rest
    ("SAVE", "OK SAVED"),
    ("BOGUS x y", "ERR UNKNOWN BOGUS"),
    ("bogus", "ERR UNKNOWN bogus"),
    ("", "ERR ARGS empty request"),
    ("   ", "ERR ARGS empty request"),
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
fn ipmi_passes_the_bytes_to_the_host() {
    let mut host = FakeHost::new();
    reply(&mut host, "IPMI 0x06 0x01 ff 7 00");
    assert_eq!(host.ipmi_calls, [(0x06, 0x01, vec![0xFF, 0x07, 0x00])]);
}

#[test]
fn ipmi_reports_a_completion_code_without_data_and_a_failed_request() {
    let mut host = FakeHost::new();
    host.ipmi_result = Ok((0xC1, vec![]));
    assert_eq!(reply(&mut host, "IPMI 06 99"), "OK IPMI CC=C1 DATA=");
    host.ipmi_result = Err(AppError::Dispatch);
    assert_eq!(reply(&mut host, "IPMI 07 01"), "ERR IPMI_FAILED DISPATCH");
    host.ipmi_result = Err(AppError::TooLong);
    assert_eq!(reply(&mut host, "IPMI 06 01"), "ERR IPMI_FAILED TOO_LONG");
    // More bytes than one IPMB frame holds are refused before the host runs.
    let long = format!("IPMI 06 01{}", " 00".repeat(ipmc_core::ipmb::IPMB_MAX_LEN));
    host.ipmi_calls.clear();
    assert_eq!(
        reply(&mut host, &long),
        format!("ERR ARGS more than {} bytes", ipmc_core::ipmb::IPMB_MAX_LEN)
    );
    assert!(host.ipmi_calls.is_empty());
}

#[test]
fn status_words_follow_the_power_and_health_states() {
    struct Case(PowerState, Summary, &'static str);
    let cases = [
        Case(PowerState::Off, Summary::Good, "power=off health=ok"),
        Case(
            PowerState::PowerOnRequested,
            Summary::Good,
            "power=off health=ok",
        ),
        Case(PowerState::On, Summary::Bad, "power=on health=fault"),
        Case(
            PowerState::Fault,
            Summary::Unavailable,
            "power=fault health=fault",
        ),
    ];
    for Case(power, health, words) in cases {
        let mut host = FakeHost::new();
        (host.power, host.health, host.nvmro, host.state) = (power, health, true, State::M0);
        assert_eq!(
            reply(&mut host, "STATUS"),
            format!("OK STATUS ipmb=82h ga=1 fru=M0 {words} nvmro=1 uptime_s=77")
        );
    }
}

#[test]
fn fields_lists_every_field_with_its_longest_value() {
    let mut host = FakeHost::new();
    let want: Vec<String> = FruField::ALL
        .iter()
        .map(|f| format!("{}={}", f.name(), f.max_len()))
        .collect();
    assert_eq!(
        reply(&mut host, "FIELDS"),
        format!("OK FIELDS {}", want.join(" "))
    );
    assert!(reply(&mut host, "FIELDS").contains("MFG_DATE=16"));
}

#[test]
fn save_answers_each_state() {
    let rows = [
        (SaveState::Saved, "OK SAVED"),
        (SaveState::Pending, "ERR PENDING a save is waiting"),
        (SaveState::LogOff, "ERR LOG_OFF the flash log is off"),
    ];
    for (state, want) in rows {
        let mut host = FakeHost::new();
        host.save = state;
        assert_eq!(reply(&mut host, "SAVE"), want);
    }
}

#[test]
fn every_field_error_has_its_error_code() {
    let rows = [
        (FieldError::TooLong, "ERR TOO_LONG max=24"),
        (
            FieldError::BadLength,
            "ERR BAD_LENGTH a text of one byte is not allowed",
        ),
        (FieldError::NotAscii, "ERR NOT_ASCII printable ASCII only"),
        (
            FieldError::NoRoom,
            "ERR NO_ROOM the FRU area has no room for it",
        ),
        (FieldError::Nvmro, "ERR NVMRO the FRU is write protected"),
        (
            FieldError::BadDate,
            "ERR BAD_DATE use YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:00 to 9999-12-31 23:59",
        ),
        (FieldError::Corrupt, "ERR FRU_IMAGE Corrupt"),
        (FieldError::AreaMissing, "ERR FRU_IMAGE AreaMissing"),
    ];
    for (error, want) in rows {
        let mut host = FakeHost::new();
        host.forced_set_error = Some(error);
        assert_eq!(
            reply(&mut host, "SET BOARD_SERIAL ABCDEF"),
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
        reply(&mut host, "SET BOARD_SERIAL ABCDEF"),
        "ERR NVMRO the FRU is write protected"
    );
    host.nvmro = false;
    assert_eq!(
        reply(&mut host, "GET BOARD_SERIAL"),
        "OK BOARD_SERIAL IK-T1-000001"
    );
}

#[test]
fn set_is_refused_when_writing_is_disabled() {
    let mut host = FakeHost::new();
    let request = Request {
        word: "SET",
        rest: "ASSET_TAG abc",
    };
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    set_field(&request, &mut host, &mut out, false).unwrap();
    assert_eq!(out.as_str(), "ERR WRITE_DISABLED IPMC_APP_FRU_WRITE is 0");
    assert_eq!(reply(&mut host, "GET ASSET_TAG"), "OK ASSET_TAG");
    // With the build setting, SET works (the default build allows it).
    assert_eq!(ipmc_defs::IPMC_APP_FRU_WRITE, 1);
}

#[test]
fn a_set_value_is_stored_and_marks_the_store_dirty() {
    let mut host = FakeHost::new();
    assert!(!host.store.is_dirty());
    assert_eq!(
        reply(&mut host, "SET BOARD_SERIAL 00:11:22:33:44:55"),
        "OK BOARD_SERIAL 00:11:22:33:44:55"
    );
    assert!(host.store.is_dirty());
    assert_eq!(
        reply(&mut host, "GET BOARD_SERIAL"),
        "OK BOARD_SERIAL 00:11:22:33:44:55"
    );
    // The other serial is untouched.
    assert_eq!(
        reply(&mut host, "GET PRODUCT_SERIAL"),
        "OK PRODUCT_SERIAL IK-T1-000001"
    );
    assert_eq!(ipmc_fru::verify(host.store.bytes()), Ok(()));
}

#[test]
fn the_command_table_is_well_formed() {
    let mut host = FakeHost::new();
    for (index, command) in COMMANDS.iter().enumerate() {
        assert_eq!(command.keyword, command.keyword.to_ascii_uppercase());
        assert!(!command.keyword.contains(' '));
        assert!(!command.help.is_empty(), "{}", command.keyword);
        assert!(
            COMMANDS[index + 1..]
                .iter()
                .all(|c| c.keyword != command.keyword),
            "{} twice",
            command.keyword
        );
        // HELP lists it and describes it.
        assert!(reply(&mut host, "HELP").contains(&format!(" {}", command.keyword)));
        let described = reply(&mut host, &format!("HELP {}", command.keyword));
        assert!(
            described.starts_with(&format!("OK HELP {}", command.keyword)),
            "{described}"
        );
        assert!(described.ends_with(command.help), "{described}");
    }
}

// ---- LineAssembler and respond ----

/// lines -- feeds `chunks` to an assembler of `N` bytes and answers each request on a fresh
/// host; the answers in order.
fn run<const N: usize>(chunks: &[&[u8]]) -> Vec<String> {
    let mut assembler = LineAssembler::<N>::new();
    let mut host = FakeHost::new();
    let mut answers = Vec::new();
    for chunk in chunks {
        for byte in *chunk {
            let mut out = AnswerBuf::<ANSWER_MAX>::new();
            if respond(assembler.push(*byte), &mut host, &mut out) {
                answers.push(out.as_str().to_string());
            }
        }
    }
    answers
}

#[test]
fn the_assembler_gives_the_same_lines_for_split_and_joined_chunks() {
    let whole: &[u8] = b"HELLO\r\nFRU STATE\nSAVE\r\n";
    let joined = run::<32>(&[whole]);
    assert_eq!(joined.len(), 3);
    assert_eq!(joined[1], "OK FRU M4 FRU Active");
    // Every split point of the stream, and one byte at a time.
    for cut in 0..=whole.len() {
        assert_eq!(
            run::<32>(&[&whole[..cut], &whole[cut..]]),
            joined,
            "cut {cut}"
        );
    }
    let singles: Vec<&[u8]> = whole.chunks(1).collect();
    assert_eq!(run::<32>(&singles), joined);
}

#[test]
fn cr_lf_cr_and_lf_each_end_a_line_and_empty_lines_are_not_requests() {
    assert_eq!(run::<32>(&[b"SAVE\r\n\r\n\n\rSAVE\rSAVE\n"]).len(), 3);
    assert!(run::<32>(&[b"\r\n\r\n"]).is_empty());
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
    for (rest, runs) in cases {
        host.ipmi_calls.clear();
        let request = Request { word: "IPMI", rest };
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        ipmi_request(&request, &mut host, &mut out, false).unwrap();
        assert_eq!(host.ipmi_calls.len(), usize::from(runs), "{rest}");
        if !runs {
            assert_eq!(out.as_str(), "ERR WRITE_DISABLED IPMC_APP_FRU_WRITE is 0");
        }
        // With the permission every request runs.
        host.ipmi_calls.clear();
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        ipmi_request(&request, &mut host, &mut out, true).unwrap();
        assert_eq!(host.ipmi_calls.len(), 1, "{rest}");
    }
}

/// feed_all -- pushes every byte of `bytes` and collects the answers (fresh answer buffer each).
fn feed_all<const N: usize>(
    assembler: &mut LineAssembler<N>,
    host: &mut FakeHost,
    bytes: &[u8],
) -> Vec<String> {
    let mut answers = Vec::new();
    for byte in bytes {
        let mut out = AnswerBuf::<ANSWER_MAX>::new();
        if respond(assembler.push(*byte), host, &mut out) {
            answers.push(out.as_str().to_string());
        }
    }
    answers
}

#[test]
fn a_damaged_line_is_answered_without_a_tag_and_not_run() {
    let mut host = FakeHost::new();
    let mut assembler = LineAssembler::<64>::new();
    // Bytes of a request arrived, then the port lost 3 and the ring was discarded.
    assert!(feed_all(&mut assembler, &mut host, b"#a7 SET ASSET_").is_empty());
    assembler.damage(3);
    // The rest of the line ends: answered once, no tag, not run; the CR LF pair adds nothing.
    let answers = feed_all(
        &mut assembler,
        &mut host,
        b"TAG abc
",
    );
    assert_eq!(answers, ["ERR LINE_DAMAGED 3 bytes lost on the port"]);
    assert_eq!(reply(&mut host, "GET ASSET_TAG"), "OK ASSET_TAG");
    // The line after it runs normally.
    let answers = feed_all(
        &mut assembler,
        &mut host,
        b"SAVE
",
    );
    assert_eq!(answers, ["OK SAVED"]);
    // A loss that arrives before the line end adds up; a bare line end answers too.
    assembler.damage(2);
    assembler.damage(5);
    let answers = feed_all(
        &mut assembler,
        &mut host,
        b"
",
    );
    assert_eq!(answers, ["ERR LINE_DAMAGED 7 bytes lost on the port"]);
    // Nothing is answered before a line end.
    let mut out = AnswerBuf::<ANSWER_MAX>::new();
    assert!(!respond(Fed::Nothing, &mut host, &mut out));
    assert!(out.is_empty());
}

#[test]
fn many_requests_then_a_damaged_set_never_runs_the_set() {
    // The Codex scenario: a burst of complete requests overran the ring, the firmware discarded
    // the ring (no answer for the lines inside it), and the SET that follows lost bytes.
    let mut host = FakeHost::new();
    let mut assembler = LineAssembler::<64>::new();
    assembler.damage(40);
    let answers = feed_all(
        &mut assembler,
        &mut host,
        b"ASSET_TAG ABCD
",
    );
    assert_eq!(answers, ["ERR LINE_DAMAGED 40 bytes lost on the port"]);
    let answers = feed_all(
        &mut assembler,
        &mut host,
        b"SET ASSET_TAG ABCD
GET ASSET_TAG
",
    );
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0], "OK ASSET_TAG ABCD");
    assert_eq!(answers[1], "OK ASSET_TAG ABCD");
}

#[test]
fn an_overlong_line_is_answered_once_when_its_end_arrives() {
    // N = 8: a line of 8 bytes passes, 9 do not.
    let answers = run::<8>(&[b"STATUS\r\n"]);
    assert_eq!(
        answers,
        ["OK STATUS ipmb=82h ga=1 fru=M4 power=on health=ok nvmro=0 uptime_s=77"]
    );
    let answers = run::<8>(&[b"HELP    \n", b"HELP     \n", b"SAVE\n"]);
    assert_eq!(answers.len(), 3);
    assert!(answers[0].starts_with("OK HELP"));
    assert_eq!(
        answers[1],
        format!("ERR LINE_TOO_LONG max={}", ipmc_defs::IPMC_APP_LINE_MAX)
    );
    assert_eq!(answers[2], "OK SAVED");
    // Nothing is answered before the end of the overlong line arrives.
    let mut assembler = LineAssembler::<4>::new();
    for byte in b"0123456789" {
        assert_eq!(assembler.push(*byte), Fed::Nothing);
    }
    assert_eq!(assembler.push(b'\n'), Fed::TooLong);
    assert_eq!(assembler.push(b'\n'), Fed::Nothing);
}

#[test]
fn a_line_with_a_byte_that_is_not_printable_ascii_gets_one_error() {
    let answers = run::<32>(&[b"HEL\x00LO\nSAVE\n", b"SA\xC3\xA9VE\n"]);
    assert_eq!(
        answers,
        [
            "ERR ARGS only printable ASCII is accepted",
            "OK SAVED",
            "ERR ARGS only printable ASCII is accepted"
        ]
    );
    // A tab inside a line is fine.
    assert_eq!(run::<32>(&[b"FRU\tSTATE\n"]), ["OK FRU M4 FRU Active"]);
}

#[test]
fn the_assembler_returns_the_text_of_the_line() {
    let mut assembler = LineAssembler::<16>::new();
    for byte in b"GET ASSET_TAG" {
        assert_eq!(assembler.push(*byte), Fed::Nothing);
    }
    assert_eq!(assembler.push(b'\r'), Fed::Line("GET ASSET_TAG"));
    assert_eq!(assembler.push(b'\n'), Fed::Nothing);
    for byte in b"HELLO" {
        assembler.push(*byte);
    }
    assert_eq!(assembler.push(b'\n'), Fed::Line("HELLO"));
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
    buf.clear();
    assert!(buf.is_empty() && !buf.overflowed());
}

#[test]
fn the_longest_answers_fit_the_answer_buffer() {
    let mut host = FakeHost::new();
    // An unknown word as long as a request line.
    let word = "X".repeat(ipmc_defs::IPMC_APP_LINE_MAX as usize - 10);
    let long = reply(&mut host, &format!("#abcdefgh {word}"));
    assert!(long.len() < ANSWER_MAX);
    // A full frame of response data.
    host.ipmi_result = Ok((0, vec![0xAB; ipmc_core::ipmb::IPMB_RESPONSE_DATA_MAX]));
    let full = reply(&mut host, "#abcdefgh IPMI 06 01");
    assert!(full.len() < ANSWER_MAX, "{}", full.len());
}
