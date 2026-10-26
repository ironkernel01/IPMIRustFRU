//! commands - the small command table of the bootloader session (design 8.2, PLAN decision 34):
//! the keywords, their help texts and the lookup of a line. The answers are `session`.

use crate::error::BootError;
use crate::text::{TextOut, CRLF};
use ipmc_menu::eq_ignore_case;

/// Cmd -- the commands of the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Wakeup,
    Help,
    Info,
    EraseApp,
    EraseFru,
    Confirm,
    Load,
    Verify,
    Run,
    Reset,
}

/// Row -- one command: its keywords (all of them, in order), its help text and its command.
pub struct Row {
    pub words: &'static [&'static str],
    pub help: &'static str,
    pub cmd: Cmd,
}

const fn row(words: &'static [&'static str], help: &'static str, cmd: Cmd) -> Row {
    Row { words, help, cmd }
}

/// COMMANDS -- the table; `HELP` and `?` are two rows of one command.
pub const COMMANDS: &[Row] = &[
    row(&["WAKEUP"], "open the session (answers OK)", Cmd::Wakeup),
    row(&["HELP"], "this table", Cmd::Help),
    row(&["?"], "this table", Cmd::Help),
    row(&["INFO"], "start-up record, image check, port", Cmd::Info),
    row(
        &["ERASE", "APP"],
        "erase sectors 2-7, the runtime firmware (then CONFIRM)",
        Cmd::EraseApp,
    ),
    row(
        &["ERASE", "FRU"],
        "erase sector 1, the FRU flash store (then CONFIRM)",
        Cmd::EraseFru,
    ),
    row(&["CONFIRM"], "run the pending erase", Cmd::Confirm),
    row(
        &["LOAD"],
        "receive the runtime as Intel HEX records (after ERASE APP)",
        Cmd::Load,
    ),
    row(&["VERIFY"], "check the runtime image", Cmd::Verify),
    row(&["RUN"], "start the runtime", Cmd::Run),
    row(&["RESET"], "restart the processor", Cmd::Reset),
];

/// words -- the tokens of a line: split on spaces, empty pieces dropped (the line editor stores
/// printable ASCII only).
pub fn words(line: &str) -> impl Iterator<Item = &str> + Clone {
    let mut rest = line;
    core::iter::from_fn(move || {
        let tail = rest.get(rest.bytes().position(|byte| byte != b' ')?..)?;
        let end = tail.bytes().position(|byte| byte == b' ').unwrap_or(tail.len());
        let (word, after) = tail.split_at_checked(end)?;
        rest = after;
        Some(word)
    })
}

/// find -- the command of a line: each keyword compared with `eq_ignore_case`, all words of a row
/// and no more. `Unknown` when the first word starts no row, `Args` when it does but the words do
/// not match the row (missing, extra or wrong).
pub fn find(line: &str) -> Result<Cmd, BootError> {
    let mut known = false;
    for row in COMMANDS {
        let first = words(line).next().zip(row.words.first());
        if !first.is_some_and(|(word, keyword)| eq_ignore_case(word, keyword)) {
            continue;
        }
        known = true;
        if words(line).count() == row.words.len()
            && words(line)
                .zip(row.words)
                .all(|(word, keyword)| eq_ignore_case(word, keyword))
        {
            return Ok(row.cmd);
        }
    }
    Err(if known {
        BootError::Args
    } else {
        BootError::Unknown
    })
}

/// SPACES -- padding source; a column never needs more.
const SPACES: &str = "                ";

/// text_len -- characters of a row's keywords written with single spaces between them.
fn text_len(row: &Row) -> usize {
    row.words.iter().map(|word| word.len()).sum::<usize>() + row.words.len().saturating_sub(1)
}

/// put_keywords -- the keywords of a row with single spaces.
fn put_keywords(out: &mut dyn TextOut, row: &Row) {
    for (at, word) in row.words.iter().enumerate() {
        if at > 0 {
            out.put(" ");
        }
        out.put(word);
    }
}

/// put_help -- one line per row: the keywords, padded to the longest keyword text plus two
/// spaces, then the help text.
pub fn put_help(out: &mut dyn TextOut) {
    let column = COMMANDS.iter().map(text_len).max().unwrap_or(0) + 2;
    for row in COMMANDS {
        put_keywords(out, row);
        out.put(SPACES.get(..column - text_len(row)).unwrap_or(" "));
        out.put(row.help);
        out.put(CRLF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    #[test]
    fn lines_find_their_command_in_any_case() {
        let rows: [(&str, Result<Cmd, BootError>); 22] = [
            ("WAKEUP", Ok(Cmd::Wakeup)),
            ("wakeup", Ok(Cmd::Wakeup)),
            ("  WaKeUp  ", Ok(Cmd::Wakeup)),
            ("HELP", Ok(Cmd::Help)),
            ("?", Ok(Cmd::Help)),
            ("info", Ok(Cmd::Info)),
            ("erase app", Ok(Cmd::EraseApp)),
            ("ERASE   FRU", Ok(Cmd::EraseFru)),
            ("Erase Fru", Ok(Cmd::EraseFru)),
            ("confirm", Ok(Cmd::Confirm)),
            ("Load", Ok(Cmd::Load)),
            ("verify", Ok(Cmd::Verify)),
            ("run", Ok(Cmd::Run)),
            ("reset", Ok(Cmd::Reset)),
            ("", Err(BootError::Unknown)),
            ("FOO", Err(BootError::Unknown)),
            ("RUNNING", Err(BootError::Unknown)),
            ("RUN NOW", Err(BootError::Args)),
            ("ERASE", Err(BootError::Args)),
            ("ERASE ALL", Err(BootError::Args)),
            ("ERASE APP FRU", Err(BootError::Args)),
            ("? ?", Err(BootError::Args)),
        ];
        for (line, want) in rows {
            assert_eq!(find(line), want, "{line:?}");
        }
    }

    #[test]
    fn the_help_table_text() {
        let mut out = String::new();
        put_help(&mut out);
        let want = "\
WAKEUP     open the session (answers OK)\r\n\
HELP       this table\r\n\
?          this table\r\n\
INFO       start-up record, image check, port\r\n\
ERASE APP  erase sectors 2-7, the runtime firmware (then CONFIRM)\r\n\
ERASE FRU  erase sector 1, the FRU flash store (then CONFIRM)\r\n\
CONFIRM    run the pending erase\r\n\
LOAD       receive the runtime as Intel HEX records (after ERASE APP)\r\n\
VERIFY     check the runtime image\r\n\
RUN        start the runtime\r\n\
RESET      restart the processor\r\n";
        assert_eq!(out, want);
    }
}
