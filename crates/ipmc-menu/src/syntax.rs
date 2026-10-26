// syntax.rs - MenuBuilder syntax strings: token classification and table validation.
//
// Every function here is `const fn`, so firmware validates its tables at compile time with
// `const _: () = assert!(Menu::validate_tables(..).is_ok())`. The matcher and the help printer
// call the same `token_at`, so a syntax string has one parser.

use crate::{Command, MENU_TOKENS_MAX};

/// Kind -- kind of one syntax token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    Required,
    Optional,
    /// `{NAME...}`: zero or more remaining words; the last token of a syntax only.
    Rest,
}

/// SynToken -- one syntax token and its kind. `text` is the token as written (brackets included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SynToken<'a> {
    pub kind: Kind,
    pub text: &'a str,
}

/// TableError -- why a command table fails validation. Variants name the fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    EmptySyntax,
    FirstTokenNotKeyword,
    MalformedToken,
    OptionalNotLast,
    RestNotLast,
    SyntaxTooLong,
    DuplicateKeywords,
    ReservedKeyword,
    BadGroupName,
    DuplicateGroup,
    GroupNameClash,
    EmptyGroup,
}

impl TableError {
    /// name -- log name of the table error.
    pub fn name(self) -> &'static str {
        match self {
            TableError::EmptySyntax => "EmptySyntax",
            TableError::FirstTokenNotKeyword => "FirstTokenNotKeyword",
            TableError::MalformedToken => "MalformedToken",
            TableError::OptionalNotLast => "OptionalNotLast",
            TableError::RestNotLast => "RestNotLast",
            TableError::SyntaxTooLong => "SyntaxTooLong",
            TableError::DuplicateKeywords => "DuplicateKeywords",
            TableError::ReservedKeyword => "ReservedKeyword",
            TableError::BadGroupName => "BadGroupName",
            TableError::DuplicateGroup => "DuplicateGroup",
            TableError::GroupNameClash => "GroupNameClash",
            TableError::EmptyGroup => "EmptyGroup",
        }
    }
}

/// Shape -- token counts a syntax accepts: keywords + required, and that plus the optional ones
/// (a trailing `{NAME...}` counts as one token here and accepts any number of words, `rest`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub min_tokens: usize,
    pub max_tokens: usize,
    pub rest: bool,
}

/// REST_SUFFIX -- the end of a `{NAME...}` token: three dots and the closing brace.
const REST_SUFFIX: &[u8] = b"...}";

/// RESERVED_WORDS -- words the engine owns at every level. No table may start a command with one of them.
pub(crate) const RESERVED_WORDS: [&str; 4] = ["HELP", "?", "UP", ".."];

/// ends_with_rest_suffix -- `word` ends in `...}`.
const fn ends_with_rest_suffix(word: &[u8]) -> bool {
    let Some((_, mut tail)) = word.split_at_checked(word.len().saturating_sub(REST_SUFFIX.len()))
    else {
        return false;
    };
    let mut suffix = REST_SUFFIX;
    // Slice patterns walk both without indexing; the lengths are equal unless `word` is shorter.
    while let ([t, t_rest @ ..], [s, s_rest @ ..]) = (tail, suffix) {
        if *t != *s {
            return false;
        }
        tail = t_rest;
        suffix = s_rest;
    }
    word.len() >= REST_SUFFIX.len()
}

/// token_at -- the `n`th whitespace-separated token of `s` (0-based), classified by its first
/// byte: `[` required, `{` optional (`{NAME...}` the rest), anything else a keyword. None past
/// the last token.
pub const fn token_at(s: &str, n: usize) -> Option<SynToken<'_>> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut index = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if index == n {
            let (_, tail) = bytes.split_at(start);
            let (word, _) = tail.split_at(i - start);
            let Ok(text) = core::str::from_utf8(word) else {
                return None;
            };
            let kind = match word[0] {
                b'[' => Kind::Required,
                b'{' if ends_with_rest_suffix(word) => Kind::Rest,
                b'{' => Kind::Optional,
                _ => Kind::Keyword,
            };
            return Some(SynToken { kind, text });
        }
        index += 1;
    }
    None
}

/// eq_ignore_case -- ASCII case-insensitive equality; exact length required (no prefixes).
pub const fn eq_ignore_case(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if !a[i].eq_ignore_ascii_case(&b[i]) {
            return false;
        }
        i += 1;
    }
    true
}

const fn is_bracket(b: u8) -> bool {
    matches!(b, b'[' | b']' | b'{' | b'}')
}

/// A well-formed parameter is `[NAME]`, `{NAME}` or `{NAME...}`: a non-empty name with no
/// bracket inside. A keyword contains no bracket at all.
const fn token_well_formed(t: SynToken<'_>) -> bool {
    let bytes = t.text.as_bytes();
    let (from, to) = match t.kind {
        Kind::Keyword => (0, bytes.len()),
        Kind::Required | Kind::Optional => {
            let close = if matches!(t.kind, Kind::Required) {
                b']'
            } else {
                b'}'
            };
            if bytes.len() < 3 || bytes[bytes.len() - 1] != close {
                return false;
            }
            (1, bytes.len() - 1)
        }
        Kind::Rest => {
            // `{` + name + `...}`: the name needs one byte.
            if bytes.len() < 1 + 1 + REST_SUFFIX.len() {
                return false;
            }
            (1, bytes.len() - REST_SUFFIX.len())
        }
    };
    let mut i = from;
    while i < to {
        if is_bracket(bytes[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// validate_syntax -- checks one syntax string and returns its token-count shape.
///
/// Rejects: no token; first token not a keyword; a malformed parameter; anything after an
/// optional parameter that is not itself optional; anything after a `{NAME...}`; more than
/// `MENU_TOKENS_MAX` tokens.
pub const fn validate_syntax(s: &str) -> Result<Shape, TableError> {
    let mut n = 0;
    let mut min_tokens = 0;
    let mut seen_optional = false;
    let mut seen_rest = false;
    while let Some(t) = token_at(s, n) {
        if n >= MENU_TOKENS_MAX {
            return Err(TableError::SyntaxTooLong);
        }
        if seen_rest {
            return Err(TableError::RestNotLast);
        }
        if !token_well_formed(t) {
            return Err(TableError::MalformedToken);
        }
        if n == 0 && !matches!(t.kind, Kind::Keyword) {
            return Err(TableError::FirstTokenNotKeyword);
        }
        match t.kind {
            Kind::Optional => seen_optional = true,
            Kind::Rest => {
                seen_optional = true;
                seen_rest = true;
            }
            Kind::Keyword | Kind::Required => {
                if seen_optional {
                    return Err(TableError::OptionalNotLast);
                }
                min_tokens += 1;
            }
        }
        n += 1;
    }
    if n == 0 {
        return Err(TableError::EmptySyntax);
    }
    Ok(Shape {
        min_tokens,
        max_tokens: n,
        rest: seen_rest,
    })
}

/// Next keyword at or after token index `from`; returns (keyword, index after it).
const fn next_keyword(s: &str, from: usize) -> Option<(&str, usize)> {
    let mut n = from;
    while let Some(t) = token_at(s, n) {
        if matches!(t.kind, Kind::Keyword) {
            return Some((t.text, n + 1));
        }
        n += 1;
    }
    None
}

/// Two syntaxes have identical keyword sequences when their keywords, read in order, are the
/// same words (case-insensitive) and the same count. Parameters do not count.
const fn same_keywords(a: &str, b: &str) -> bool {
    let (mut ia, mut ib) = (0, 0);
    loop {
        match (next_keyword(a, ia), next_keyword(b, ib)) {
            (None, None) => return true,
            (Some((ka, na)), Some((kb, nb))) => {
                if !eq_ignore_case(ka, kb) {
                    return false;
                }
                ia = na;
                ib = nb;
            }
            _ => return false,
        }
    }
}

/// is_reserved -- True when `word` is one of the engine's own words (HELP, ?, UP, ..).
pub(crate) const fn is_reserved(word: &str) -> bool {
    let mut i = 0;
    while i < RESERVED_WORDS.len() {
        if eq_ignore_case(RESERVED_WORDS[i], word) {
            return true;
        }
        i += 1;
    }
    false
}

/// validate_table -- checks every syntax in `table`, that no command starts with a reserved
/// word, and that no two commands have identical keyword sequences.
pub const fn validate_table<C>(table: &[Command<C>]) -> Result<(), TableError> {
    let mut i = 0;
    while i < table.len() {
        if let Err(e) = validate_syntax(table[i].syntax) {
            return Err(e);
        }
        if let Some(first) = token_at(table[i].syntax, 0) {
            if is_reserved(first.text) {
                return Err(TableError::ReservedKeyword);
            }
        }
        let mut j = i + 1;
        while j < table.len() {
            if same_keywords(table[i].syntax, table[j].syntax) {
                return Err(TableError::DuplicateKeywords);
            }
            j += 1;
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Args, CmdResult};

    fn nop(_c: &mut (), _a: &Args<'_>) -> CmdResult {
        Ok(())
    }

    const fn cmd(syntax: &'static str) -> Command<()> {
        Command {
            syntax,
            help: "h",
            handler: nop,
        }
    }

    #[test]
    fn classifies_tokens_by_first_byte() {
        let t = |n| token_at("SET MODE [M] {L}", n).map(|t| t.kind);
        assert_eq!(t(0), Some(Kind::Keyword));
        assert_eq!(t(1), Some(Kind::Keyword));
        assert_eq!(t(2), Some(Kind::Required));
        assert_eq!(t(3), Some(Kind::Optional));
        assert_eq!(t(4), None);
    }

    #[test]
    fn rest_token_is_the_last_token_only() {
        let kind = |s, n| token_at(s, n).map(|t| t.kind);
        assert_eq!(kind("RUN {WORDS...}", 1), Some(Kind::Rest));
        assert_eq!(kind("RUN {WORDS}", 1), Some(Kind::Optional));
        let s = validate_syntax("RUN [A] {WORDS...}").unwrap();
        assert_eq!((s.min_tokens, s.max_tokens, s.rest), (2, 3, true));
        let s = validate_syntax("RUN {A} {WORDS...}").unwrap();
        assert_eq!((s.min_tokens, s.max_tokens, s.rest), (1, 3, true));
        assert!(!validate_syntax("RUN {A}").unwrap().rest);
        for bad in [
            "RUN {W...} [A]",
            "RUN {W...} {A}",
            "RUN {W...} {V...}",
            "RUN {W...} NOW",
        ] {
            assert_eq!(validate_syntax(bad), Err(TableError::RestNotLast), "{bad}");
        }
        for bad in ["RUN {...}", "RUN {W...", "RUN {W{...}"] {
            assert_eq!(
                validate_syntax(bad),
                Err(TableError::MalformedToken),
                "{bad}"
            );
        }
    }

    #[test]
    fn shape_counts_keywords_required_optional() {
        let s = validate_syntax("SET MODE [MODE] {LEVEL}").unwrap();
        assert_eq!((s.min_tokens, s.max_tokens), (3, 4));
        let s = validate_syntax("GET STATUS").unwrap();
        assert_eq!((s.min_tokens, s.max_tokens), (2, 2));
    }

    #[test]
    fn rejects_bad_syntax() {
        assert_eq!(validate_syntax(""), Err(TableError::EmptySyntax));
        assert_eq!(validate_syntax("   "), Err(TableError::EmptySyntax));
        assert_eq!(
            validate_syntax("[X] GO"),
            Err(TableError::FirstTokenNotKeyword)
        );
        assert_eq!(validate_syntax("GO [X"), Err(TableError::MalformedToken));
        assert_eq!(validate_syntax("GO []"), Err(TableError::MalformedToken));
        assert_eq!(validate_syntax("GO [A}"), Err(TableError::MalformedToken));
        assert_eq!(validate_syntax("G]O"), Err(TableError::MalformedToken));
        assert_eq!(
            validate_syntax("GO {A} [B]"),
            Err(TableError::OptionalNotLast)
        );
        assert_eq!(
            validate_syntax("GO {A} NOW"),
            Err(TableError::OptionalNotLast)
        );
        let tokens = |n: usize| "A ".repeat(n);
        assert_eq!(
            validate_syntax(&tokens(MENU_TOKENS_MAX + 1)),
            Err(TableError::SyntaxTooLong)
        );
        assert!(validate_syntax(&tokens(MENU_TOKENS_MAX)).is_ok());
    }

    #[test]
    fn table_rejects_duplicate_keyword_sequences() {
        let t = [cmd("LED ON"), cmd("led on")];
        assert_eq!(validate_table(&t), Err(TableError::DuplicateKeywords));
        let t = [cmd("LED BLINK"), cmd("LED BLINK {HZ}")];
        assert_eq!(validate_table(&t), Err(TableError::DuplicateKeywords));
        let t = [cmd("LED ON"), cmd("LED OFF"), cmd("LED")];
        assert_eq!(validate_table(&t), Ok(()));
    }

    #[test]
    fn table_rejects_reserved_first_keyword() {
        for word in ["help", "?", "Up", ".."] {
            let t = [cmd(word)];
            assert_eq!(validate_table(&t), Err(TableError::ReservedKeyword));
        }
    }

    #[test]
    fn table_reports_the_first_bad_syntax() {
        let t = [cmd("OK"), cmd("BAD [")];
        assert_eq!(validate_table(&t), Err(TableError::MalformedToken));
    }

    #[test]
    fn eq_ignore_case_is_exact() {
        assert!(eq_ignore_case("Led", "LED"));
        assert!(!eq_ignore_case("LE", "LED"));
        assert!(!eq_ignore_case("LEDS", "LED"));
    }

    #[test]
    fn validation_runs_in_const_context() {
        const T: [Command<()>; 1] = [cmd("OK")];
        const _: () = assert!(validate_table(&T).is_ok());
    }
}
