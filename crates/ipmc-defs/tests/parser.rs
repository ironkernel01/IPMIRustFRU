//! tests/parser.rs - runs the fixture tests of defs_parser.rs, the parser build.rs uses.
//!
//! The file is included, not copied: the build and its tests share one parser.

#[path = "../defs_parser.rs"]
mod defs_parser;

use defs_parser::{parse, render, Value};

// ---- unsigned integer, string and structure fixtures ----
fn rejected(text: &str, line: usize, why_part: &str) {
    let err = parse(text).expect_err("header must be rejected");
    assert_eq!(err.line, line, "{}: {text:?}", err.why);
    assert!(err.why.contains(why_part), "{} lacks {why_part}", err.why);
}

/// define_of -- the parsed value of define `name` in a header made of `body`.
fn define_of(body: &str, name: &str) -> Value {
    let defs = parse(&header(body)).expect("header must parse");
    defs.iter().find(|d| d.name == name).unwrap().value.clone()
}

fn value_of(body: &str, name: &str) -> u32 {
    match define_of(body, name) {
        Value::Int(v) => v,
        other => panic!("{name} is {other:?}"),
    }
}

// ---- every header the parser must accept, one table ----

/// Accept -- one header body, and the value define `name` must parse to.
struct Accept {
    case: &'static str,
    body: &'static str,
    name: &'static str,
    value: Value,
}

fn acc(case: &'static str, body: &'static str, name: &'static str, value: Value) -> Accept {
    Accept {
        case,
        body,
        name,
        value,
    }
}

#[test]
fn accepted_defines_parse_to_their_values() {
    let int = Value::Int;
    let text = |t: &str| Value::Text(t.to_string());
    let signed = Value::Signed;
    let cases = [
        // Unsigned integers and references.
        acc("decimal", "#define A 100000\n", "A", int(100_000)),
        acc("hex", "#define A 0x1F\n", "A", int(0x1F)),
        acc("hex with capital X", "#define A 0XfF\n", "A", int(0xFF)),
        acc("zero", "#define A 0\n", "A", int(0)),
        acc("u32 max", "#define A 4294967295\n", "A", int(u32::MAX)),
        acc("reference", "#define A 7\n#define B A\n", "B", int(7)),
        // Strings.
        acc(
            "string",
            "#define S \"IronKernel\"\n",
            "S",
            text("IronKernel"),
        ),
        acc(
            "every allowed punctuation",
            "#define S \"two words, 0-9 ~!#%&'()*+,-./:;<=>?@[]^_`{|}\"\n",
            "S",
            text("two words, 0-9 ~!#%&'()*+,-./:;<=>?@[]^_`{|}"),
        ),
        acc(
            "padded string",
            "#define S \"  padded  \"\n",
            "S",
            text("  padded  "),
        ),
        acc(
            "string with trailing comment",
            "#define S \"x\" /* trailing */\n",
            "S",
            text("x"),
        ),
        acc(
            "string with extra spaces",
            "#define   S   \"x\"   \n",
            "S",
            text("x"),
        ),
        acc("empty string", "#define S \"\"\n", "S", text("")),
        acc(
            "empty string with comment",
            "#define S \"\" /* none */\n",
            "S",
            text(""),
        ),
        acc(
            "string referenced by a define",
            "#define S \"a\"\n#define T S\n",
            "T",
            text("a"),
        ),
        acc(
            "block comment mark in a string",
            "#define S \"a/*b*/c\"\n",
            "S",
            text("a/*b*/c"),
        ),
        acc(
            "line comment mark in a string",
            "#define S \"a//b\"\n",
            "S",
            text("a//b"),
        ),
        // The comment after the literal still goes.
        acc(
            "comment after an open mark in a string",
            "#define S \"a/*b\" /* c */\n",
            "S",
            text("a/*b"),
        ),
        // Signed ([i32]) values.
        acc(
            "signed negative",
            "#define A -10 /* [i32] lc */\n",
            "A",
            signed(-10),
        ),
        acc("signed zero", "#define A 0 /* [i32] z */\n", "A", signed(0)),
        acc(
            "signed positive",
            "#define A 85 /* [i32] uc */\n",
            "A",
            signed(85),
        ),
        acc(
            "signed tag only",
            "#define A -1 /* [i32] */\n",
            "A",
            signed(-1),
        ),
        acc(
            "signed spaced tag",
            "#define A   -7   /*   [i32]   spaced */\n",
            "A",
            signed(-7),
        ),
        acc(
            "i32 max",
            "#define A 2147483647 /* [i32] */\n",
            "A",
            signed(i32::MAX),
        ),
        acc(
            "i32 min",
            "#define A -2147483648 /* [i32] */\n",
            "A",
            signed(i32::MIN),
        ),
        acc(
            "reference to an earlier signed define",
            "#define A -5 /* [i32] a */\n#define B A /* [i32] b */\n",
            "B",
            signed(-5),
        ),
        // Lines: guard 1-2, comment 3-4, define on line 5.
        acc(
            "tag after a leading multi-line comment",
            "/* x\n y */\n#define A -3 /* [i32] t */\n",
            "A",
            signed(-3),
        ),
        acc(
            "second trailing comment cannot hide a tag",
            "#define A -4 /* [i32] a */ /* b */\n",
            "A",
            signed(-4),
        ),
        acc(
            "comment marks and a tag inside a string",
            "#define S \"/* [i32] text\"\n",
            "S",
            text("/* [i32] text"),
        ),
        acc(
            "bad tag inside a string before a comment",
            "#define S \"/* [i33] text\" /* note */\n",
            "S",
            text("/* [i33] text"),
        ),
        // Whitespace after the closing marker of a multi-line comment is legal.
        acc(
            "whitespace after a multi-line comment",
            "/* a\n b */  \t\n#define B 2\n",
            "B",
            int(2),
        ),
    ];
    for case in cases {
        assert_eq!(define_of(case.body, case.name), case.value, "{}", case.case);
    }
}

#[test]
fn comments_and_strings_keep_line_numbers() {
    // (case, body, define, its value, text appended to the body, line of the rejection, reason)
    let cases = [
        (
            "block comments",
            "/* one\n two */\n#define A 1 /* tail */\n/* c */\n#define B 2\n",
            "B",
            2,
            // Lines: guard 1-2, comment 3-4, A 5, comment 6, B 7, bad line 8.
            "#if 1\n",
            8,
            "not allowed",
        ),
        (
            "string define before a comment",
            "#define S \"a\"\n/* two\n lines */\n#define T 1\n",
            "T",
            1,
            "#define U \"a\\b\"\n",
            7,
            "backslash",
        ),
    ];
    for (case, body, name, value, tail, line, why) in cases {
        assert_eq!(value_of(body, name), value, "{case}");
        rejected(&header(&format!("{body}{tail}")), line, why);
    }
}

#[test]
fn guard_rules() {
    assert!(parse(&header("")).unwrap().is_empty());
    rejected("#define A 1\n#ifndef IPMI_DEFS_H\n", 1, "not allowed");
    rejected(
        "#ifndef IPMI_DEFS_H\n#define IPMI_DEFS_H\n#define A 1\n",
        3,
        "incomplete",
    );
    rejected("", 1, "incomplete");
    rejected(&format!("{}#define B 2\n", header("")), 4, "not allowed");
    rejected(
        "#ifndef OTHER_H\n#define OTHER_H\n#endif\n",
        1,
        "not allowed",
    );
}

#[test]
fn render_writes_typed_consts_in_order() {
    // (case, header body, the rendered text it must end with)
    let cases = [
        (
            "unsigned",
            "#define A 1\n#define B 0x10\n",
            "pub const A: u32 = 1;\npub const B: u32 = 16;\n",
        ),
        (
            "strings",
            "#define N 5\n#define S \"a b\"\n#define E \"\"\n",
            "pub const N: u32 = 5;\npub const S: &str = \"a b\";\npub const E: &str = \"\";\n",
        ),
        (
            "signed",
            "#define A -10 /* [i32] a */\n#define B 7 /* [i32] b */\n#define C 3\n",
            "pub const A: i32 = -10;\npub const B: i32 = 7;\npub const C: u32 = 3;\n",
        ),
    ];
    for (case, body, tail) in cases {
        let text = render(&parse(&header(body)).unwrap());
        assert!(text.ends_with(tail), "{case}: {text}");
    }
}

// ---- signed ([i32]) fixtures ----

fn header(body: &str) -> String {
    format!("#ifndef IPMI_DEFS_H\n#define IPMI_DEFS_H\n{body}#endif\n")
}

#[test]
fn signed_untagged_stays_u32() {
    let defs = parse(&header(
        "#define A 5 /* five */\n#define B 6 /* [note] */\n",
    ));
    assert!(defs.is_err()); // `[note]` is a bracketed first token that is not a tag
    let defs = parse(&header("#define A 5 /* five [i32] later */\n")).unwrap();
    assert_eq!(defs[0].value, Value::Int(5));
}

// ---- constructs C preprocesses differently ----

// ---- type tags come from the lexed trailing comment ----

// ---- every header the parser must refuse, one table ----

/// Reject -- one header the parser refuses: the case name (printed on failure), the text (wrapped
/// in the include guard when `wrapped`), the 1-based line of the fault and a part of the reason.
struct Reject {
    name: &'static str,
    text: &'static str,
    wrapped: bool,
    line: usize,
    why: &'static str,
}

/// w -- a body that `header` wraps in the include guard.
const fn w(name: &'static str, text: &'static str, line: usize, why: &'static str) -> Reject {
    Reject {
        name,
        text,
        wrapped: true,
        line,
        why,
    }
}

/// r -- a complete text, used as written.
const fn r(name: &'static str, text: &'static str, line: usize, why: &'static str) -> Reject {
    Reject {
        name,
        text,
        wrapped: false,
        line,
        why,
    }
}

const REJECTIONS: &[Reject] = &[
    // Integers.
    w("octal 010", "#define A 010\n", 3, "no leading 0"),
    w("octal 00", "\n#define A 00\n", 4, "no leading 0"),
    w("hex sign", "#define A 0x+1\n", 3, "hex or decimal"),
    w("hex no digits", "#define A 0x\n", 3, "hex or decimal"),
    w("hex bad digit", "#define A 0xG\n", 3, "hex or decimal"),
    w("suffix U", "#define A 10U\n", 3, "hex or decimal"),
    w(
        "suffix UL on hex",
        "#define A 0x10UL\n",
        3,
        "hex or decimal",
    ),
    w("char literal", "#define A 'a'\n", 3, "hex or decimal"),
    w(
        "parenthesised value",
        "#define A (1)\n",
        3,
        "hex or decimal",
    ),
    w("expression", "#define A 1 + 2\n", 3, "not allowed"),
    w(
        "negative untagged",
        "#define A -1\n",
        3,
        "needs the [i32] tag",
    ),
    w(
        "negative -10 untagged",
        "#define A -10\n",
        3,
        "negative value needs the [i32] tag",
    ),
    w("negative name", "#define A -x\n", 3, "hex or decimal"),
    w(
        "over u32 decimal",
        "#define A 4294967296\n",
        3,
        "fit in u32",
    ),
    w("over u32 hex", "#define A 0x100000000\n", 3, "fit in u32"),
    // Directives and macros.
    w(
        "function-like macro",
        "#define F(x) x\n",
        3,
        "invalid define name",
    ),
    w(
        "empty function-like macro",
        "#define F(x)\n",
        3,
        "not allowed",
    ),
    w("#if", "#if 1\n", 3, "not allowed"),
    w("#ifdef", "#define A 1\n#ifdef A\n", 4, "not allowed"),
    w("#include", "#include <x.h>\n", 3, "not allowed"),
    w(
        "line comment after value",
        "#define A 1 // note\n",
        3,
        "not allowed",
    ),
    w("line comment alone", "// note\n", 3, "not allowed"),
    w("trailing backslash", "#define A 1 \\\n", 3, "backslash"),
    w(
        "splice hides comment end",
        "/* open *\\\n/ #define A 1\n",
        3,
        "backslash",
    ),
    w(
        "backslash then spaces",
        "#define A 1 \\  \n",
        3,
        "backslash",
    ),
    // Names and references.
    w("duplicate", "#define A 1\n#define A 2\n", 4, "duplicate"),
    w("undefined reference", "#define A B\n", 3, "undefined"),
    w("self reference", "#define A A\n", 3, "undefined"),
    w(
        "string define with parameters",
        "#define S(x) \"a\"\n",
        3,
        "invalid define name",
    ),
    w(
        "duplicate string",
        "#define S \"a\"\n#define S \"b\"\n",
        4,
        "duplicate",
    ),
    w(
        "string then integer",
        "#define S \"a\"\n#define S 1\n",
        4,
        "duplicate",
    ),
    w("unquoted word", "#define S IronKernel\n", 3, "undefined"),
    w("unquoted words", "#define S two words\n", 3, "not allowed"),
    // Comments.
    w(
        "unterminated comment",
        "#define A 1\n/* never closed\n",
        4,
        "unterminated",
    ),
    r("unterminated comment alone", "/* x", 1, "unterminated"),
    // Strings.
    w("backslash n", "#define S \"a\\nb\"\n", 3, "backslash"),
    w("backslash quote", "#define S \"a\\\"b\"\n", 3, "backslash"),
    w("double backslash", "#define S \"\\\\\"\n", 3, "backslash"),
    w(
        "line splice in string",
        "#define S \"abc\\\n",
        3,
        "backslash",
    ),
    w(
        "embedded quote",
        "#define S \"a\"b\"\n",
        3,
        "after the string",
    ),
    w(
        "embedded quote pair",
        "#define S \"a\"\"b\"\n",
        3,
        "adjacent",
    ),
    w(
        "adjacent literals",
        "#define S \"a\" \"b\"\n",
        3,
        "adjacent",
    ),
    w(
        "adjacent literals no space",
        "#define S \"a\"\"b\"\n",
        3,
        "adjacent",
    ),
    w(
        "adjacent across comment",
        "#define S \"a\" /* c */ \"b\"\n",
        3,
        "adjacent",
    ),
    w(
        "adjacent empty literal",
        "#define S \"a\"\n#define T \"x\" \"\"\n",
        4,
        "adjacent",
    ),
    w(
        "number after literal",
        "#define S \"a\" 1\n",
        3,
        "after the string",
    ),
    w(
        "plus after literal",
        "#define S \"a\" + 1\n",
        3,
        "after the string",
    ),
    w(
        "line comment after literal",
        "#define S \"a\" // c\n",
        3,
        "after the string",
    ),
    w(
        "non-ASCII e-acute",
        "#define S \"caf\u{e9}\"\n",
        3,
        "non-ASCII",
    ),
    w(
        "non-ASCII em dash",
        "#define S \"a\u{2014}b\"\n",
        3,
        "non-ASCII",
    ),
    w("tab in string", "#define S \"a\tb\"\n", 3, "non-printable"),
    w(
        "DEL in string",
        "#define S \"a\u{7f}b\"\n",
        3,
        "non-printable",
    ),
    w("CR in string", "#define S \"a\rb\"\n", 3, "non-printable"),
    w(
        "unterminated string",
        "#define S \"abc\n",
        3,
        "unterminated string",
    ),
    w("lone quote", "#define S \"\n", 3, "unterminated string"),
    w(
        "comment opener in unterminated string",
        "#define S \"abc /* x\n#define T 1\n",
        3,
        "unterminated string",
    ),
    // Signed ([i32]) values and tags.
    w(
        "negative with prose comment",
        "#define A -10 /* lower critical */\n",
        3,
        "negative value needs the [i32] tag",
    ),
    w(
        "negative on a later line",
        "#define OK 1\n#define A -1 /* x */\n",
        4,
        "negative value needs the [i32] tag",
    ),
    w(
        "tag [i33]",
        "#define A 1 /* [i33] x */\n",
        3,
        "unknown type tag",
    ),
    w(
        "tag [I32]",
        "#define A 1 /* [I32] x */\n",
        3,
        "unknown type tag",
    ),
    w(
        "tag [u32]",
        "#define A 1 /* [u32] x */\n",
        3,
        "unknown type tag",
    ),
    w(
        "tag missing bracket",
        "#define A 1 /* [i32 x */\n",
        3,
        "malformed type tag",
    ),
    w(
        "tag glued to text",
        "#define A 1 /* [i32]x */\n",
        3,
        "unknown type tag",
    ),
    w(
        "bad tag on a later line",
        "#define OK 2\n#define A 1 /* [i33] x */\n",
        4,
        "unknown type tag",
    ),
    w(
        "signed -0",
        "#define A -0 /* [i32] */\n",
        3,
        "-0 is not allowed",
    ),
    w(
        "signed -010",
        "#define A -010 /* [i32] */\n",
        3,
        "leading 0",
    ),
    w("signed 010", "#define A 010 /* [i32] */\n", 3, "leading 0"),
    w(
        "below i32 min",
        "#define A -2147483649 /* [i32] */\n",
        3,
        "fit in i32",
    ),
    w(
        "above i32 max",
        "#define A 2147483648 /* [i32] */\n",
        3,
        "fit in i32",
    ),
    w(
        "signed lone minus",
        "#define A - /* [i32] */\n",
        3,
        "decimal",
    ),
    w("signed -x", "#define A -x /* [i32] */\n", 3, "decimal"),
    w("signed hex", "#define A 0x10 /* [i32] */\n", 3, "decimal"),
    w(
        "signed double minus",
        "#define A --1 /* [i32] */\n",
        3,
        "decimal",
    ),
    w(
        "signed refs unsigned",
        "#define U 5\n#define S U /* [i32] */\n",
        4,
        "type mismatch",
    ),
    w(
        "unsigned refs signed",
        "#define S -5 /* [i32] */\n#define U S\n",
        4,
        "type mismatch",
    ),
    w(
        "signed refs string",
        "#define T \"a\"\n#define S T /* [i32] */\n",
        4,
        "type mismatch",
    ),
    w(
        "signed refs undefined",
        "#define S Q /* [i32] */\n",
        3,
        "undefined",
    ),
    w(
        "tag on a string",
        "#define S \"a\" /* [i32] */\n",
        3,
        "type tag on a string",
    ),
    w(
        "second comment hides bad tag",
        "#define A 1 /* [i33] a */ /* b */\n",
        3,
        "unknown type tag",
    ),
    w(
        "second comment carries bad tag",
        "#define A 1 /* [i32] a */ /* [i33] */\n",
        3,
        "unknown type tag",
    ),
    w(
        "string tag then comment",
        "#define S \"a\" /* [i32] */ /* x */\n",
        3,
        "type tag on a string",
    ),
    w(
        "string with bad tag",
        "#define S \"ok\" /* [i33] */\n",
        3,
        "unknown type tag",
    ),
    w(
        "string with malformed tag",
        "#define S \"ok\" /* [i32 */\n",
        3,
        "malformed type tag",
    ),
    // The include guard name.
    w(
        "guard name as integer",
        "#define IPMI_DEFS_H 1\n",
        3,
        "include guard name",
    ),
    w(
        "guard name as string",
        "#define IPMI_DEFS_H \"x\"\n",
        3,
        "include guard name",
    ),
    // Constructs C preprocesses differently.
    w(
        "comment open at end of define",
        "#define A 1 /* open\n*/ #define B 2\n",
        3,
        "spans lines after code",
    ),
    w(
        "comment open then blank line",
        "#define OK 1\n#define A 1 /* open\n\n*/\n",
        4,
        "spans lines after code",
    ),
    w(
        "text after multi-line comment",
        "/* open\n*/ #define B 2\n",
        4,
        "after the end of a multi-line comment",
    ),
    w(
        "comment after multi-line comment",
        "/* a\n b */ /* c */\n",
        4,
        "after the end of a multi-line comment",
    ),
    w(
        "trigraph in string",
        "#define S \"a??/nb\"\n",
        3,
        "trigraph",
    ),
    w(
        "trigraph in comment",
        "/* ??/ */\n#define A 1\n",
        3,
        "trigraph",
    ),
    w(
        "trigraph in trailing comment",
        "#define A 1\n#define B 2 /* ??= */\n",
        4,
        "trigraph",
    ),
    r("trigraph as directive", "??=define X 1\n", 1, "trigraph"),
];

#[test]
fn every_rejection_names_its_line_and_reason() {
    for case in REJECTIONS {
        let text = if case.wrapped {
            header(case.text)
        } else {
            case.text.to_string()
        };
        let err = match parse(&text) {
            Ok(_) => panic!("{}: header must be rejected: {text:?}", case.name),
            Err(err) => err,
        };
        assert_eq!(err.line, case.line, "{}: {}", case.name, err.why);
        assert!(
            err.why.contains(case.why),
            "{}: {:?} lacks {:?}",
            case.name,
            err.why,
            case.why
        );
    }
}
