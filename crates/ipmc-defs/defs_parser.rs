// defs_parser.rs - the ipmi_defs.h parser, shared by build.rs and the unit tests (std only).
//
// build.rs includes this file with `#[path]` and writes OUT_DIR/ipmi_defs.rs from `render`;
// tests/parser.rs includes it so the fixture tests below run with `cargo test`.
// One parser, two users: the rules cannot drift between the build and its tests.
//
// Accepted lines: blank, C block comments, the include guard (#ifndef/#define/#endif of
// IPMI_DEFS_H) and `#define NAME <value>` where the value is one of
//   - an integer: 0x hex or decimal without a leading 0 (Rust type u32);
//   - a signed integer: a decimal with an optional leading minus, no leading 0, `-0` refused,
//     within i32, on a define whose trailing block comment starts with the tag `[i32]`
//     (Rust type i32). A negative value without the tag is refused;
//   - an earlier NAME (integer or string); an [i32] define and an untagged one never refer to
//     each other;
//   - one string literal: printable ASCII 0x20..0x7E, no backslash, no embedded quote, no
//     adjacent literal ("a" "b"), an optional trailing block comment. The empty string is legal.
// Everything else is rejected with its line number, so a C compiler that includes the header
// (it stays valid C) cannot see a different configuration. A block comment
// inside a string literal is text, as in C. Forms C preprocesses differently are rejected with
// their line: `??` anywhere (trigraphs), a block comment that spans lines after code, text
// after the `*/` of a multi-line comment. The type tag is read from the lexed trailing
// comments of the define.

const GUARD: &str = "IPMI_DEFS_H";

/// The value of one define: an integer or a string literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(u32),
    /// An integer define tagged `[i32]` in its trailing comment.
    Signed(i32),
    Text(String),
}

/// One accepted `#define NAME value` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Define {
    pub name: String,
    pub value: Value,
}

/// A rejected header: the 1-based line and the rule it broke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub why: &'static str,
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// literal_value -- None when `s` is not a plain integer literal; Some(Err) when it exceeds u32.
/// A decimal with a leading 0 is refused: C reads it as octal, so the two builds would differ.
fn literal_value(s: &str) -> Option<Result<u32, &'static str>> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return Some(u32::from_str_radix(hex, 16).map_err(|_| "value does not fit in u32"));
    }
    let digits_ok = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits_ok || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    Some(s.parse().map_err(|_| "value does not fit in u32"))
}

/// signed_literal -- the value of a tagged define: a decimal with an optional leading minus.
/// No leading 0 after the sign (C reads it as octal) and `-0` is refused.
fn signed_literal(s: &str) -> Result<i32, &'static str> {
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("tagged value is not a decimal integer with an optional leading minus");
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err("decimal has a leading 0 (C reads it as octal)");
    }
    if negative && digits == "0" {
        return Err("-0 is not allowed");
    }
    let text = if negative { s } else { digits };
    text.parse().map_err(|_| "value does not fit in i32")
}

/// tag_of -- whether the trailing block comments of a define carry the `[i32]` tag.
/// Err for any other bracketed first token: unknown (`[i33]`, `[I32]`) or unclosed (`[i32`).
/// The tag comes from the lexed comments, so a `/*` inside a string literal is text and a
/// second comment cannot hide a tag.
fn tag_of(comments: &[String]) -> Result<bool, &'static str> {
    let mut tagged = false;
    for comment in comments {
        let comment = comment.trim_start();
        if !comment.starts_with('[') {
            continue;
        }
        let token = comment.split_whitespace().next().unwrap_or("");
        if token == "[i32]" {
            tagged = true;
        } else if token.contains(']') {
            return Err("unknown type tag (the only tag is [i32])");
        } else {
            return Err("malformed type tag");
        }
    }
    Ok(tagged)
}

/// reject_trigraphs -- C11 5.2.1.1 replaces `??x` sequences before anything else (`??/` is a
/// backslash), so the file may hold no `??` at all, comments and strings included.
fn reject_trigraphs(raw: &str) -> Result<(), ParseError> {
    reject_line_where(
        raw,
        |line| line.contains("??"),
        "?? sequence (C11 trigraphs are not supported)",
    )
}

/// reject_line_where -- Err naming the first line of `raw` for which `bad` holds, with `why`.
fn reject_line_where(
    raw: &str,
    bad: impl Fn(&str) -> bool,
    why: &'static str,
) -> Result<(), ParseError> {
    match raw.lines().position(bad) {
        Some(idx) => Err(ParseError { line: idx + 1, why }),
        None => Ok(()),
    }
}

/// reject_line_splices -- C joins a line that ends in a backslash with the next one before it
/// removes comments, so this check runs on the raw text, before comment removal.
fn reject_line_splices(raw: &str) -> Result<(), ParseError> {
    reject_line_where(
        raw,
        |line| line.trim_end().ends_with('\\'),
        "line ends in a backslash (C line splicing is not supported)",
    )
}

/// Lexed -- the header with comments removed, and the comments that follow code on a line.
struct Lexed {
    /// The text with each block comment replaced by one space; newlines kept for line numbers.
    text: String,
    /// `trailing[n - 1]`: bodies of the comments that start on line `n` after code on that line.
    trailing: Vec<Vec<String>>,
}

/// strip_comments -- replaces each block comment with one space, keeping its newlines so
/// line numbers stay true, and records the comments that follow code on their line. Text inside
/// a string literal is not scanned for comments (a literal ends at its closing quote or at the
/// end of its line). Err: an unterminated comment (the line where it starts); a comment that
/// follows code and spans lines (C joins the next lines into that directive); text after the
/// closing `*/` of a multi-line comment (C reads it as a directive of its own).
fn strip_comments(text: &str) -> Result<Lexed, ParseError> {
    let mut out = String::new();
    let mut trailing: Vec<Vec<String>> = vec![Vec::new()];
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut in_string = false;
    let mut line_has_code = false;
    while let Some(c) = chars.next() {
        if c == '\n' {
            line += 1;
            trailing.push(Vec::new());
            line_has_code = false;
            out.push(c);
            in_string = false;
        } else if in_string {
            out.push(c);
            in_string = c != '"';
        } else if c == '"' {
            out.push(c);
            in_string = true;
            line_has_code = true;
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            out.push(' ');
            let (start_line, after_code) = (line, line_has_code);
            let (mut body, mut prev, mut closed) = (String::new(), ' ', false);
            for ch in chars.by_ref() {
                if ch == '\n' {
                    out.push('\n');
                    line += 1;
                    trailing.push(Vec::new());
                    line_has_code = false;
                }
                if prev == '*' && ch == '/' {
                    body.pop();
                    closed = true;
                    break;
                }
                body.push(ch);
                prev = ch;
            }
            if !closed {
                return Err(ParseError {
                    line: start_line,
                    why: "unterminated comment",
                });
            }
            if line != start_line {
                if after_code {
                    return Err(ParseError {
                        line: start_line,
                        why: "block comment spans lines after code (it must close on its own line)",
                    });
                }
                let rest_blank = chars
                    .clone()
                    .take_while(|&r| r != '\n')
                    .all(|r| r == ' ' || r == '\t' || r == '\r');
                if !rest_blank {
                    return Err(ParseError {
                        line,
                        why: "text after the end of a multi-line comment",
                    });
                }
            } else if after_code {
                trailing[start_line - 1].push(body);
            }
        } else {
            out.push(c);
            if !c.is_whitespace() {
                line_has_code = true;
            }
        }
    }
    Ok(Lexed {
        text: out,
        trailing,
    })
}

/// string_literal -- parses `rest`, which starts with the opening quote, into the literal text.
/// Err names the rule broken: backslash, non-printable or non-ASCII character, unterminated
/// literal, an adjacent second literal, or other text after the closing quote.
fn string_literal(rest: &str) -> Result<String, &'static str> {
    let mut text = String::new();
    let mut chars = rest.chars().skip(1);
    loop {
        match chars.next() {
            None => return Err("unterminated string literal"),
            Some('"') => break,
            Some('\\') => return Err("string literal contains a backslash"),
            Some(c) if !(' '..='~').contains(&c) => {
                return Err("string literal has a non-printable or non-ASCII character")
            }
            Some(c) => text.push(c),
        }
    }
    let tail: String = chars.collect();
    let tail = tail.trim();
    if tail.is_empty() {
        Ok(text)
    } else if tail.starts_with('"') {
        Err("adjacent string literals are not supported")
    } else {
        Err("text after the string literal")
    }
}

/// parse -- parses the header text; returns the defines in file order or the first error.
pub fn parse(raw: &str) -> Result<Vec<Define>, ParseError> {
    reject_trigraphs(raw)?;
    reject_line_splices(raw)?;
    let lexed = strip_comments(raw)?;
    let text = lexed.text.as_str();
    let no_comments: Vec<String> = Vec::new();
    let mut defines: Vec<Define> = Vec::new();
    let (mut ifndef, mut guard_define, mut endif) = (false, false, false);
    let mut last_line = 1;
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        last_line = line_no;
        let words: Vec<&str> = line.split_whitespace().collect();
        let fail = |why: &'static str| ParseError { line: line_no, why };
        match words.as_slice() {
            [] => {}
            ["#ifndef", g] if *g == GUARD && !ifndef => ifndef = true,
            ["#endif"] if ifndef && !endif => endif = true,
            ["#define", g] if *g == GUARD && ifndef && !guard_define => guard_define = true,
            ["#define", name, first, ..] if guard_define && !endif => {
                if !is_ident(name) {
                    return Err(fail("invalid define name"));
                }
                // C would redefine the guard macro (a diagnostic, and a value the guard does not
                // have), so the guard name never carries a value.
                if *name == GUARD {
                    return Err(fail("the include guard name cannot be redefined"));
                }
                if defines.iter().any(|d| d.name == *name) {
                    return Err(fail("duplicate define"));
                }
                let comments = lexed.trailing.get(idx).unwrap_or(&no_comments);
                let tagged = tag_of(comments).map_err(fail)?;
                let value = if first.starts_with('"') {
                    if tagged {
                        return Err(fail("type tag on a string define"));
                    }
                    // The literal may hold spaces: take the line text after the name.
                    let after_define = line.trim_start().strip_prefix("#define").unwrap_or("");
                    let after_name = after_define.trim_start().strip_prefix(*name).unwrap_or("");
                    Value::Text(string_literal(after_name.trim_start()).map_err(fail)?)
                } else if words.len() != 3 {
                    return Err(fail("line is not allowed"));
                } else {
                    let looks_negative = first
                        .strip_prefix('-')
                        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()));
                    if tagged {
                        match defines.iter().find(|d| d.name == *first) {
                            Some(d) => match d.value {
                                Value::Signed(_) => d.value.clone(),
                                _ => {
                                    return Err(fail(
                                        "type mismatch: [i32] define refers to an untagged one",
                                    ))
                                }
                            },
                            None if is_ident(first) => return Err(fail("undefined name")),
                            None => Value::Signed(signed_literal(first).map_err(fail)?),
                        }
                    } else if looks_negative {
                        return Err(fail("negative value needs the [i32] tag"));
                    } else {
                        match literal_value(first) {
                            Some(Ok(v)) => Value::Int(v),
                            Some(Err(why)) => return Err(fail(why)),
                            None if is_ident(first) => {
                                match defines.iter().find(|d| d.name == *first) {
                                    Some(d) if matches!(d.value, Value::Signed(_)) => {
                                        return Err(fail("type mismatch: untagged define refers to an [i32] one"))
                                    }
                                    Some(d) => d.value.clone(),
                                    None => return Err(fail("undefined name")),
                                }
                            }
                            None => return Err(fail(
                                "value is not a hex or decimal integer (no leading 0), a string literal or a defined name",
                            )),
                        }
                    }
                };
                defines.push(Define {
                    name: (*name).to_string(),
                    value,
                });
            }
            _ => return Err(fail("line is not allowed")),
        }
    }
    if !(ifndef && guard_define && endif) {
        return Err(ParseError {
            line: last_line,
            why: "include guard (#ifndef/#define/#endif IPMI_DEFS_H) is incomplete at end of file",
        });
    }
    Ok(defines)
}

/// render -- the generated Rust source: `pub const NAME: u32` per integer define and
/// `pub const NAME: &str` per string define (a string holds no quote or backslash).
pub fn render(defines: &[Define]) -> String {
    let mut out = String::from("// Generated by build.rs from ipmi_defs.h. Do not edit.\n");
    for d in defines {
        match &d.value {
            Value::Int(v) => out.push_str(&format!("pub const {}: u32 = {};\n", d.name, v)),
            Value::Signed(v) => out.push_str(&format!("pub const {}: i32 = {};\n", d.name, v)),
            Value::Text(t) => out.push_str(&format!("pub const {}: &str = \"{}\";\n", d.name, t)),
        }
    }
    out
}
