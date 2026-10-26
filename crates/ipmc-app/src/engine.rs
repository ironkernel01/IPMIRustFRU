//! engine.rs - the command engine: syntax strings, matching, arguments, HELP and the final line.
//!
//! The engine is the same idea as the MenuBuilder menu (Z:\AI-LinuxScripts\agent-markdowns\
//! embedded\MenuBuilder.md), written for this crate so it stands alone. A command is one `Row`
//! of a flat table: a syntax string, one sentence of help and a handler.
//!
//! # Syntax strings
//!
//! Words separated by blanks. First the keywords in capitals (one or more: `FRU SET`), then the
//! parameters:
//!
//! | Written | Meaning |
//! |---|---|
//! | `[NAME]` | a required parameter: one word |
//! | `{NAME}` | an optional parameter: one word |
//! | `{NAME...}` | the rest of the line as one value; optional; last |
//!
//! Optional parameters come after the required ones. `Syntax::parse` checks one string and
//! `check_table` checks a whole table (a test runs it over `COMMANDS`).
//!
//! # Matching
//!
//! Keywords are whole words, compared without regard to case. The row with the longest keyword
//! sequence that the line starts with wins (`FRU SET` beats a row `FRU`). `find` reports the
//! match as a value (`Option<Match>`): a handler runs only after a row matched, and the number of
//! words is checked against the row after that. A line that matches no row, or whose argument
//! count does not fit the row, never reaches a handler.
//!
//! # The answer
//!
//! `run` writes the data lines a handler wrote, then exactly one final line: `OK` or
//! `ERROR <CODE>: <sentence>`, with `#tag` after the `OK` or `ERROR` when the request had a tag.
//! A handler that fails after it wrote data lines has them taken back, so an answer never mixes
//! data lines with an error.

use core::fmt::{self, Write};

use crate::host::AppHost;
use crate::line::AnswerBuf;

/// ANSWER_MAX -- the size of the buffer one answer is formatted into, bytes: the echo of a few
/// typed bytes, the data lines, the final line and the prompt. The longest answers are the `HELP`
/// table and `FRU FIELDS` (a test checks both against this size, with room left over).
pub const ANSWER_MAX: usize = 1280;

/// TAG_MAX -- the longest tag, letters or digits after the `#` of a request.
pub const TAG_MAX: usize = 8;

/// MAX_PARAMS -- the most parameters a row may declare.
pub const MAX_PARAMS: usize = 4;

/// FAIL_TEXT_MAX -- room for the sentence of one failure: a whole request line (an unknown
/// command, a bad word) and 96 bytes of fixed text around it.
pub const FAIL_TEXT_MAX: usize = ipmc_defs::IPMC_APP_LINE_MAX + 96;
// The longest fixed sentence is the BAD_DATE range text (66 bytes); a bad word of a whole request
// line goes into the sentence of UNKNOWN or ARGS (about 40 bytes of fixed text).
const _: () = assert!(
    FAIL_TEXT_MAX >= ipmc_defs::IPMC_APP_LINE_MAX + 66,
    "FAIL_TEXT_MAX must hold a whole request line and the fixed text around it"
);

/// PROMPT -- the prompt, written after the start-up banner and after every answer.
pub const PROMPT: &str = "app> ";

/// END -- the end of every line the device sends.
pub const END: &str = "\r\n";

/// Fail -- why a request got no `OK`: the code and the sentence of the final `ERROR` line.
pub struct Fail {
    code: &'static str,
    text: AnswerBuf<FAIL_TEXT_MAX>,
}

impl Fail {
    /// new -- a failure with the given code (capitals and underscores) and sentence. A sentence
    /// longer than `FAIL_TEXT_MAX` loses its last pieces.
    pub fn new(code: &'static str, text: fmt::Arguments<'_>) -> Fail {
        let mut buf = AnswerBuf::new();
        // A sentence that does not fit keeps the pieces written before the one that overflowed.
        let _ = buf.write_fmt(text);
        Fail { code, text: buf }
    }

    /// code -- the code word of the `ERROR` line.
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// text -- the sentence of the `ERROR` line.
    pub fn text(&self) -> &str {
        self.text.as_str()
    }
}

impl fmt::Debug for Fail {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "Fail({}: {})", self.code, self.text())
    }
}

impl From<fmt::Error> for Fail {
    /// A write that failed means the answer buffer is full.
    fn from(_: fmt::Error) -> Fail {
        Fail::new(
            "ANSWER_TOO_LONG",
            format_args!("the answer is longer than {ANSWER_MAX} bytes"),
        )
    }
}

/// Handler -- a command handler: reads its arguments, asks the host, writes the data lines of
/// the answer to `out` and returns `Ok`, or returns the `Fail` that becomes the final line.
/// A write error from `out` converts into `Fail` with `?`.
pub type Handler = fn(&Args<'_>, &mut dyn AppHost, &mut dyn Write) -> Result<(), Fail>;

/// Row -- one command of a table.
pub struct Row {
    /// The syntax string (see the module comment).
    pub syntax: &'static str,
    /// One sentence for `HELP`.
    pub help: &'static str,
    /// The function that answers the command.
    pub handler: Handler,
}

/// SyntaxError -- why a syntax string is not well formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxError {
    /// The string holds no word.
    Empty,
    /// The first word is a parameter: a row needs at least one keyword.
    NoKeyword,
    /// A keyword uses a character other than A-Z, 0-9, `_` and `?`.
    BadKeyword,
    /// A parameter is not `[NAME]`, `{NAME}` or `{NAME...}` with NAME in A-Z, 0-9 and `_`.
    BadParameter,
    /// A keyword follows a parameter.
    KeywordAfterParameter,
    /// A required parameter follows an optional one.
    RequiredAfterOptional,
    /// A word follows the rest parameter `{NAME...}`.
    RestNotLast,
    /// More than `MAX_PARAMS` parameters.
    TooManyParameters,
}

/// Syntax -- the shape of one syntax string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// Leading keywords.
    pub keywords: usize,
    /// Required parameters.
    pub required: usize,
    /// Optional single-word parameters (the rest parameter is counted in `rest`).
    pub optional: usize,
    /// The last parameter takes the rest of the line.
    pub rest: bool,
}

/// is_name_char -- a character of a parameter name or a keyword: A-Z, 0-9 and `_`.
const fn is_name_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
}

/// is_name -- a non-empty run of name characters.
fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

/// is_blank -- a space or a tab.
pub const fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

impl Syntax {
    /// parse -- checks `text` and returns its shape, or the first rule it breaks.
    pub fn parse(text: &str) -> Result<Syntax, SyntaxError> {
        let mut shape = Syntax {
            keywords: 0,
            required: 0,
            optional: 0,
            rest: false,
        };
        let mut words = text.split_ascii_whitespace().peekable();
        words.peek().ok_or(SyntaxError::Empty)?;
        for word in words {
            if shape.rest {
                return Err(SyntaxError::RestNotLast);
            }
            let inner = |open: char, close: char| word.strip_prefix(open)?.strip_suffix(close);
            if let Some(name) = inner('[', ']') {
                if !is_name(name) {
                    return Err(SyntaxError::BadParameter);
                }
                if shape.optional > 0 {
                    return Err(SyntaxError::RequiredAfterOptional);
                }
                shape.required += 1;
            } else if let Some(name) = inner('{', '}') {
                match name.strip_suffix("...") {
                    Some(rest) if is_name(rest) => shape.rest = true,
                    Some(_) => return Err(SyntaxError::BadParameter),
                    None if is_name(name) => shape.optional += 1,
                    None => return Err(SyntaxError::BadParameter),
                }
            } else if word.starts_with(['[', '{']) || word.ends_with([']', '}']) {
                return Err(SyntaxError::BadParameter);
            } else if shape.params() > 0 {
                return Err(SyntaxError::KeywordAfterParameter);
            } else if word == "?" || is_name(word) {
                shape.keywords += 1;
            } else {
                return Err(SyntaxError::BadKeyword);
            }
        }
        if shape.keywords == 0 {
            return Err(SyntaxError::NoKeyword);
        }
        if shape.params() > MAX_PARAMS {
            return Err(SyntaxError::TooManyParameters);
        }
        Ok(shape)
    }

    /// params -- the number of parameters.
    pub const fn params(&self) -> usize {
        self.required + self.optional + self.rest as usize
    }
}

/// TableError -- why a table is not well formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// The syntax string of row `.0` breaks a rule.
    Syntax(usize, SyntaxError),
    /// The help sentence of row `.0` is empty.
    NoHelp(usize),
    /// Rows `.0` and `.1` have the same keyword sequence.
    Collision(usize, usize),
}

/// keywords -- the leading keywords of a row's syntax string.
fn keywords(syntax: &'static str) -> impl Iterator<Item = &'static str> {
    syntax
        .split_ascii_whitespace()
        .take_while(|word| !word.starts_with(['[', '{']))
}

/// check_table -- Ok when every row has a well formed syntax and a help sentence and no two rows
/// have the same keyword sequence (case ignored).
pub fn check_table(rows: &[Row]) -> Result<(), TableError> {
    for (index, row) in rows.iter().enumerate() {
        Syntax::parse(row.syntax).map_err(|error| TableError::Syntax(index, error))?;
        if row.help.is_empty() {
            return Err(TableError::NoHelp(index));
        }
        let later = rows.iter().enumerate().skip(index + 1);
        for (other, row_other) in later {
            let mut a = keywords(row.syntax);
            let mut b = keywords(row_other.syntax);
            let same = loop {
                match (a.next(), b.next()) {
                    (None, None) => break true,
                    (Some(x), Some(y)) if x.eq_ignore_ascii_case(y) => {}
                    _ => break false,
                }
            };
            if same {
                return Err(TableError::Collision(index, other));
            }
        }
    }
    Ok(())
}

/// next_word -- the first word of `text` (leading blanks skipped) and the text after it.
fn next_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches(is_blank);
    if text.is_empty() {
        return None;
    }
    Some(text.split_once(is_blank).unwrap_or((text, "")))
}

/// Match -- a row that a line starts with.
pub struct Match<'r, 'l> {
    /// The row.
    pub row: &'r Row,
    /// Its shape.
    pub syntax: Syntax,
    /// The line after the keywords.
    pub tail: &'l str,
}

/// strip_keywords -- `line` after the keywords of `row` when it starts with all of them.
fn strip_keywords<'l>(row: &Row, line: &'l str) -> Option<&'l str> {
    let mut rest = line;
    for keyword in keywords(row.syntax) {
        let (word, after) = next_word(rest)?;
        if !word.eq_ignore_ascii_case(keyword) {
            return None;
        }
        rest = after;
    }
    Some(rest)
}

/// find -- the row with the longest keyword sequence that `line` starts with. The result is the
/// match itself: nothing is inferred from a handler's return value.
pub fn find<'r, 'l>(rows: &'r [Row], line: &'l str) -> Option<Match<'r, 'l>> {
    let mut best: Option<Match<'r, 'l>> = None;
    for row in rows {
        let Ok(syntax) = Syntax::parse(row.syntax) else {
            continue;
        };
        if best
            .as_ref()
            .is_some_and(|b| b.syntax.keywords >= syntax.keywords)
        {
            continue;
        }
        if let Some(tail) = strip_keywords(row, line) {
            best = Some(Match { row, syntax, tail });
        }
    }
    best
}

/// Args -- the parameter values of one request, by position in the row's syntax string.
pub struct Args<'a> {
    values: [Option<&'a str>; MAX_PARAMS],
}

impl<'a> Args<'a> {
    /// parse -- splits `tail` into the parameters of `syntax`: one word each, the rest parameter
    /// taking what is left with the blanks at both ends removed. None when a required parameter
    /// is missing or words are left over.
    pub fn parse(syntax: &Syntax, tail: &'a str) -> Option<Args<'a>> {
        let mut values = [None; MAX_PARAMS];
        let mut rest = tail;
        for (index, slot) in values.iter_mut().enumerate().take(syntax.params()) {
            if syntax.rest && index + 1 == syntax.params() {
                let value = rest.trim_matches(is_blank);
                *slot = (!value.is_empty()).then_some(value);
                rest = "";
            } else if let Some((word, after)) = next_word(rest) {
                *slot = Some(word);
                rest = after;
            }
        }
        let missing = values
            .iter()
            .take(syntax.required)
            .any(|value| value.is_none());
        (!missing && rest.trim_matches(is_blank).is_empty()).then_some(Args { values })
    }

    /// get -- parameter `index` (0 is the first parameter after the keywords), None when an
    /// optional parameter was left out.
    pub fn get(&self, index: usize) -> Option<&'a str> {
        self.values.get(index).copied().flatten()
    }

    /// required -- parameter `index` of a row that declares it required. The missing case cannot
    /// happen after `parse`; it is a `Fail` and not a panic so a wrong index in a handler shows
    /// as an error line.
    #[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
    pub fn required(&self, index: usize) -> Result<&'a str, Fail> {
        self.get(index)
            .ok_or_else(|| Fail::new("ARGS", format_args!("parameter {index} is missing")))
    }
}

/// parse_tag -- splits `line` into the tag and the request. A first word that starts with `#`
/// is the tag: 1 to 8 letters or digits after the `#`.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
fn parse_tag(line: &str) -> Result<(Option<&str>, &str), Fail> {
    let line = line.trim_start_matches(is_blank);
    if !line.starts_with('#') {
        return Ok((None, line));
    }
    let (word, after) = next_word(line).unwrap_or(("#", ""));
    let tag = word.strip_prefix('#').unwrap_or(word);
    if (1..=TAG_MAX).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok((Some(tag), after.trim_start_matches(is_blank)))
    } else {
        Err(Fail::new(
            "ARGS",
            format_args!("a tag is # and 1 to 8 letters or digits"),
        ))
    }
}

/// Tagged -- ` #tag` when the request had a tag, nothing otherwise.
struct Tagged<'a>(Option<&'a str>);

impl fmt::Display for Tagged<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(tag) => write!(out, " #{tag}"),
            None => Ok(()),
        }
    }
}

/// unknown -- the failure of a line no row matches. The quoted text is the first word, or the
/// first two when the first is the first keyword of some row (`FRU BOGUS`).
fn unknown(rows: &[Row], line: &str) -> Fail {
    let Some((first, after_first)) = next_word(line) else {
        return Fail::new("ARGS", format_args!("empty request; type HELP or ?"));
    };
    let leads = rows.iter().any(|row| {
        keywords(row.syntax)
            .next()
            .is_some_and(|keyword| keyword.eq_ignore_ascii_case(first))
    });
    let shown = match next_word(after_first) {
        Some((_, after_second)) if leads => line
            .get(..line.len() - after_second.len())
            .unwrap_or(first)
            .trim_matches(is_blank),
        _ => first,
    };
    Fail::new(
        "UNKNOWN",
        format_args!("unknown command '{shown}'; type HELP or ?"),
    )
}

/// write_help -- the aligned table of the rows that start with the words of `filter` (all rows
/// when `filter` is None), one line per row: two blanks, the syntax padded to the longest
/// syntax of the table plus two, ` - ` and the help sentence. A filter that matches no row is
/// `UNKNOWN`. The filter matches a row when its words equal the row's keywords as far as both
/// go, so `FRU` lists the `FRU` rows and `FRU SET X` the row `FRU SET`.
#[allow(clippy::result_large_err)] // Fail holds its sentence inline (no allocator); it is returned on the failure path only
pub fn write_help(rows: &[Row], filter: Option<&str>, out: &mut dyn Write) -> Result<(), Fail> {
    let width = rows.iter().map(|row| row.syntax.len()).max().unwrap_or(0) + 2;
    let mut printed = 0;
    for row in rows {
        let shown = filter.is_none_or(|text| {
            let mut typed = text.split_ascii_whitespace();
            keywords(row.syntax).all(|keyword| {
                typed
                    .next()
                    .is_none_or(|word| word.eq_ignore_ascii_case(keyword))
            })
        });
        if shown {
            write!(out, "  {:<width$} - {}{END}", row.syntax, row.help)?;
            printed += 1;
        }
    }
    match (printed, filter) {
        (0, Some(text)) => Err(Fail::new(
            "UNKNOWN",
            format_args!("no command '{text}'; type HELP or ?"),
        )),
        _ => Ok(()),
    }
}

/// finish -- ends an answer that starts at byte `mark` of `out`: the final line of `result`. A
/// failure takes back the data lines after `mark`; a final line that does not fit does the same
/// and answers `ANSWER_TOO_LONG`.
fn finish<const N: usize>(
    out: &mut AnswerBuf<N>,
    mark: usize,
    tag: Option<&str>,
    result: Result<(), Fail>,
) {
    #[allow(clippy::result_large_err)]
    // Fail holds its sentence inline (no allocator); it is returned on the failure path only
    let result = result.and_then(|()| Ok(write!(out, "OK{}{END}", Tagged(tag))?));
    if let Err(fail) = result {
        out.truncate(mark);
        // The error line is short; it fits the room the data lines had.
        let _ = write!(
            out,
            "ERROR{} {}: {}{END}",
            Tagged(tag),
            fail.code(),
            fail.text()
        );
    }
}

/// reject -- writes the final `ERROR` line of `fail` without running a request (a line that
/// could not be read: too long, a bad byte, lost bytes).
pub fn reject<const N: usize>(out: &mut AnswerBuf<N>, fail: Fail) {
    finish(out, out.len(), None, Err(fail));
}

/// run -- answers one request line (no CR or LF): the optional tag, the row that matches, its
/// handler, the final line. `out` gets the data lines and the final line, each ended by CR LF.
pub fn run<const N: usize>(
    rows: &[Row],
    line: &str,
    host: &mut dyn AppHost,
    out: &mut AnswerBuf<N>,
) {
    let mark = out.len();
    let (tag, request) = match parse_tag(line) {
        Ok(parts) => parts,
        Err(fail) => return finish(out, mark, None, Err(fail)),
    };
    let result = match find(rows, request) {
        None => Err(unknown(rows, request)),
        Some(found) => match Args::parse(&found.syntax, found.tail) {
            None => Err(Fail::new(
                "ARGS",
                format_args!("usage: {}", found.row.syntax),
            )),
            Some(args) => (found.row.handler)(&args, host, &mut *out),
        },
    };
    finish(out, mark, tag, result);
}

/// final_line -- the final line of an answer in `text` (the parsing rule of the payload CPU): the
/// first line that is `OK`, starts with `OK `, or starts with `ERROR `. Lines that start with
/// the prompt are skipped.
pub fn final_line(text: &str) -> Option<&str> {
    text.lines().find(|line| {
        !line.starts_with(PROMPT)
            && (*line == "OK" || line.starts_with("OK ") || line.starts_with("ERROR "))
    })
}
