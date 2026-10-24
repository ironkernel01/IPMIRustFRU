// vectors.rs - the vector reader accepts the real capture and rejects each kind of bad line with
// the right line number.

mod common;
use common::capture_path;
use ipmc_core::queue::Bus;
use ipmc_emu::vectors::{self, VectorError, VectorErrorKind, FRAME_MAX_BYTES};

/// Frames in vectors/sbc_ab.vec: 309 requests to the IPMC and 310 responses.
const CAPTURE_FRAMES: usize = 619;

fn err(text: &str) -> VectorError {
    vectors::parse(text).expect_err("the text is rejected")
}

#[test]
fn the_real_capture_is_accepted() {
    if common::capture_missing() {
        return;
    }
    let all = vectors::load(&capture_path()).expect("capture loads");
    assert_eq!(all.len(), CAPTURE_FRAMES);
    assert!(all.iter().all(|v| !v.bytes.is_empty()));
    assert!(all.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
}

#[test]
fn a_good_line_gives_time_bus_and_bytes() {
    let v = vectors::parse("# comment\n\n1.2345 B 82 1c 0\n").expect("parses");
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].line, v[0].time_ms, v[0].bus), (3, 1235, Bus::B));
    assert_eq!(v[0].bytes, [0x82, 0x1C, 0x00]);
}

/// Reject -- a text the reader refuses: the 1-based line and the kind of the error.
struct Reject {
    name: &'static str,
    text: &'static str,
    line: usize,
    kind: VectorErrorKind,
}

#[test]
fn bad_lines_are_rejected_with_their_kind_and_line() {
    use VectorErrorKind::{BadBus, BadHex, BadTime, EmptyFrame, MissingBus};
    let at = |name, text, line, kind| Reject {
        name,
        text,
        line,
        kind,
    };
    let cases = [
        at(
            "bad hex digits",
            "# header\n0.1 A 82 20\n0.2 A 82 GG\n",
            3,
            BadHex,
        ),
        // A sign, a prefix and a three-digit token are not hex bytes either.
        at("signed byte", "0.1 A +1\n", 1, BadHex),
        at("prefixed byte", "0.1 A 0x82\n", 1, BadHex),
        at("three-digit byte", "0.1 A 082\n", 1, BadHex),
        at("unknown bus tag", "0.1 A 82\n\n0.2 C 82\n", 3, BadBus),
        at("lower-case bus tag", "0.1 a 82\n", 1, BadBus),
        // The first field is then the bus tag, which is not a number.
        at("missing time", "0.1 A 82\nA 82 20\n", 2, BadTime),
        at("negative time", "-1 A 82\n", 1, BadTime),
        at("NaN time", "nan A 82\n", 1, BadTime),
        at(
            "a line with only a time has no bus",
            "0.1 A 82\n0.2\n",
            2,
            MissingBus,
        ),
        at("a line with no bytes", "0.1 A\n", 1, EmptyFrame),
    ];
    for case in cases {
        let e = err(case.text);
        assert_eq!(e.kind, case.kind, "{}", case.name);
        assert_eq!(e.line, case.line, "{}", case.name);
    }
}

#[test]
fn a_too_long_frame_is_rejected_with_its_line() {
    let ok = vec!["00"; FRAME_MAX_BYTES].join(" ");
    let long = vec!["00"; FRAME_MAX_BYTES + 1].join(" ");
    assert!(vectors::parse(&format!("0.1 A {ok}\n")).is_ok());
    let e = err(&format!("0.1 A {ok}\n# c\n0.2 B {long}\n"));
    assert_eq!((e.line, e.kind), (3, VectorErrorKind::FrameTooLong));
}

#[test]
fn a_too_long_line_is_rejected() {
    let line = format!("0.1 A 82 {}", " ".repeat(vectors::LINE_MAX_CHARS));
    let e = err(&line);
    assert_eq!((e.line, e.kind), (1, VectorErrorKind::LineTooLong));
}

#[test]
fn the_error_text_names_the_line() {
    assert_eq!(
        err("0.1 Z 82\n").to_string(),
        "line 1: bus tag is not A or B"
    );
}
