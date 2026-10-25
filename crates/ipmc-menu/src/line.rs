// line.rs - line input: byte-at-a-time editor and the bounded tokenizer.
//
// The editor never writes output. `push_byte` returns an event and the caller echoes, erases
// or runs the line with its own output path.

use crate::{MenuOutput, MENU_LINE_MAX, MENU_TOKENS_MAX};

/// MenuError -- why a line was rejected before matching. Variants name the fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuError {
    /// The line holds more than `MENU_LINE_MAX` bytes. Nothing runs.
    LineTooLong,
    /// The line holds more than `MENU_TOKENS_MAX` tokens. Nothing runs.
    TooManyTokens,
}

impl MenuError {
    /// name -- log name of the line error.
    pub fn name(self) -> &'static str {
        match self {
            MenuError::LineTooLong => "LineTooLong",
            MenuError::TooManyTokens => "TooManyTokens",
        }
    }
}

/// report_line_error -- prints the one-line message for a rejected line.
pub fn report_line_error(error: MenuError, out: &mut dyn MenuOutput) {
    match error {
        MenuError::LineTooLong => crate::menu::emit(
            out,
            format_args!("Error: line too long (max {} characters)\n", MENU_LINE_MAX),
        ),
        MenuError::TooManyTokens => crate::menu::emit(
            out,
            format_args!("Error: too many words (max {})\n", MENU_TOKENS_MAX),
        ),
    }
}

/// Tokens -- the tokens of one line, borrowed from it.
pub(crate) struct Tokens<'a> {
    words: [&'a str; MENU_TOKENS_MAX],
    count: usize,
}

impl<'a> Tokens<'a> {
    /// split -- whitespace-separated tokens of `line`. Rejects (never truncates) a line longer
    /// than `MENU_LINE_MAX` bytes or with more than `MENU_TOKENS_MAX` tokens.
    pub(crate) fn split(line: &'a str) -> Result<Self, MenuError> {
        if line.len() > MENU_LINE_MAX {
            return Err(MenuError::LineTooLong);
        }
        let mut tokens = Tokens {
            words: [""; MENU_TOKENS_MAX],
            count: 0,
        };
        for word in line.split_ascii_whitespace() {
            let slot = tokens
                .words
                .get_mut(tokens.count)
                .ok_or(MenuError::TooManyTokens)?;
            *slot = word;
            tokens.count += 1;
        }
        Ok(tokens)
    }

    /// as_slice -- the tokens as a slice.
    pub(crate) fn as_slice(&self) -> &[&'a str] {
        self.words.get(..self.count).unwrap_or(&[])
    }
}

/// EditEvent -- what one input byte did to the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditEvent<'a> {
    /// Nothing for the caller to do (ignored byte, CR LF second half, dropped overflow byte).
    None,
    /// The byte was stored; echo it.
    Echo(u8),
    /// One stored byte was removed; erase one character on screen.
    Backspace,
    /// A line ended. It stays valid until the next `push_byte`.
    Line(&'a str),
    /// A line ended after more than `MENU_LINE_MAX` printable bytes. The line is discarded.
    LineTooLong,
}

/// LineEditor -- line editor with a fixed buffer. Printable ASCII is stored; CR, LF and CR LF end a line;
/// backspace (0x08) and delete (0x7F) remove the last byte; every other byte is ignored.
pub struct LineEditor {
    buf: [u8; MENU_LINE_MAX],
    len: usize,
    last: u8,
    overflowed: bool,
    finished: bool,
}

const BACKSPACE: u8 = 0x08;
const DELETE: u8 = 0x7F;
const CR: u8 = b'\r';
const LF: u8 = b'\n';

impl LineEditor {
    /// new -- an empty line editor.
    pub const fn new() -> Self {
        LineEditor {
            buf: [0; MENU_LINE_MAX],
            len: 0,
            last: 0,
            overflowed: false,
            finished: false,
        }
    }

    /// push_byte -- feeds one received byte.
    ///
    /// A printable byte past `MENU_LINE_MAX` is dropped and marks the line; the line end then
    /// reports `LineTooLong` instead of a truncated line.
    pub fn push_byte(&mut self, byte: u8) -> EditEvent<'_> {
        if self.finished {
            self.len = 0;
            self.overflowed = false;
            self.finished = false;
        }
        let previous = self.last;
        self.last = byte;
        match byte {
            CR | LF => {
                if byte == LF && previous == CR {
                    return EditEvent::None; // second half of CR LF
                }
                self.finished = true;
                if self.overflowed {
                    return EditEvent::LineTooLong;
                }
                match self.buf.get(..self.len).map(core::str::from_utf8) {
                    Some(Ok(line)) => EditEvent::Line(line),
                    // Only printable ASCII is stored, so this arm is unreachable in practice.
                    _ => EditEvent::None,
                }
            }
            BACKSPACE | DELETE => {
                if self.len == 0 {
                    return EditEvent::None;
                }
                self.len -= 1;
                EditEvent::Backspace
            }
            b' '..=b'~' => match self.buf.get_mut(self.len) {
                Some(slot) => {
                    *slot = byte;
                    self.len += 1;
                    EditEvent::Echo(byte)
                }
                None => {
                    self.overflowed = true;
                    EditEvent::None
                }
            },
            _ => EditEvent::None,
        }
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `bytes`, returns the completed lines (owned) and the echoed bytes.
    fn run(editor: &mut LineEditor, bytes: &[u8]) -> (Vec<String>, Vec<u8>, usize) {
        let (mut lines, mut echoed, mut erased) = (Vec::new(), Vec::new(), 0);
        for &b in bytes {
            match editor.push_byte(b) {
                EditEvent::Line(l) => lines.push(l.to_string()),
                EditEvent::LineTooLong => lines.push("<too long>".to_string()),
                EditEvent::Echo(c) => echoed.push(c),
                EditEvent::Backspace => erased += 1,
                EditEvent::None => {}
            }
        }
        (lines, echoed, erased)
    }

    #[test]
    fn cr_lf_and_crlf_each_end_one_line() {
        for ending in [&b"\r"[..], b"\n", b"\r\n"] {
            let mut e = LineEditor::new();
            let mut input = b"abc".to_vec();
            input.extend_from_slice(ending);
            let (lines, echoed, _) = run(&mut e, &input);
            assert_eq!(lines, ["abc"]);
            assert_eq!(echoed, b"abc");
        }
    }

    #[test]
    fn cr_cr_is_two_lines_and_crlf_lf_is_two() {
        let mut e = LineEditor::new();
        let (lines, _, _) = run(&mut e, b"a\r\rb\r\n\n");
        assert_eq!(lines, ["a", "", "b", ""]);
    }

    /// EditCase -- bytes fed to a fresh editor and everything that must come out: the lines, the
    /// echoed bytes and the number of erase events.
    struct EditCase {
        name: &'static str,
        input: &'static [u8],
        lines: &'static [&'static str],
        echoed: &'static [u8],
        erased: usize,
    }

    #[test]
    fn editing_bytes_follow_the_table() {
        let cases = [
            EditCase {
                name: "backspace and delete remove bytes",
                input: b"abx\x08c\x7F\x7F\x7F\x7Fd\r",
                lines: &["d"],
                echoed: b"abxcd",
                // One backspace after "abx", then three effective deletes; the fourth finds it empty.
                erased: 4,
            },
            EditCase {
                name: "backspace on an empty line does nothing",
                input: b"\x08\x7F\r",
                lines: &[""],
                echoed: b"",
                erased: 0,
            },
            EditCase {
                name: "control and non-ASCII bytes are ignored",
                input: b"a\x01\x09\x1b\xC3\xA9b\r",
                lines: &["ab"],
                echoed: b"ab",
                erased: 0,
            },
        ];
        for case in cases {
            let mut e = LineEditor::new();
            let (lines, echoed, erased) = run(&mut e, case.input);
            assert_eq!(lines, case.lines, "{}", case.name);
            assert_eq!(echoed, case.echoed, "{}", case.name);
            assert_eq!(erased, case.erased, "{}", case.name);
        }
    }

    #[test]
    fn full_line_is_accepted_and_one_more_byte_rejects_it() {
        let mut e = LineEditor::new();
        let mut at_limit = vec![b'x'; MENU_LINE_MAX];
        at_limit.push(b'\r');
        let (lines, echoed, _) = run(&mut e, &at_limit);
        assert_eq!(lines[0].len(), MENU_LINE_MAX);
        assert_eq!(echoed.len(), MENU_LINE_MAX);

        let mut over = vec![b'x'; MENU_LINE_MAX + 5];
        over.push(b'\r');
        let (lines, echoed, _) = run(&mut e, &over);
        assert_eq!(lines, ["<too long>"]);
        assert_eq!(echoed.len(), MENU_LINE_MAX); // overflow bytes are not echoed
    }

    #[test]
    fn editor_recovers_after_an_overlong_line() {
        let mut e = LineEditor::new();
        let mut input = vec![b'x'; MENU_LINE_MAX + 1];
        input.extend_from_slice(b"\rok\r");
        let (lines, _, _) = run(&mut e, &input);
        assert_eq!(lines, ["<too long>", "ok"]);
    }

    #[test]
    fn tokenizer_splits_on_runs_of_whitespace() {
        let t = Tokens::split("  led \t blink   5 ").unwrap();
        assert_eq!(t.as_slice(), ["led", "blink", "5"]);
        assert!(Tokens::split("   ").unwrap().as_slice().is_empty());
    }

    #[test]
    fn tokenizer_rejects_too_many_tokens() {
        let at_limit = "a ".repeat(MENU_TOKENS_MAX);
        assert_eq!(
            Tokens::split(&at_limit).unwrap().as_slice().len(),
            MENU_TOKENS_MAX
        );
        let over = "a ".repeat(MENU_TOKENS_MAX + 1);
        assert!(matches!(
            Tokens::split(&over),
            Err(MenuError::TooManyTokens)
        ));
    }

    #[test]
    fn tokenizer_rejects_overlong_line() {
        let long = "a ".repeat(MENU_LINE_MAX);
        assert!(matches!(Tokens::split(&long), Err(MenuError::LineTooLong)));
        let exact = "a".repeat(MENU_LINE_MAX);
        assert!(Tokens::split(&exact).is_ok());
    }
}
