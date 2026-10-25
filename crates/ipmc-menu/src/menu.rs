// menu.rs - tiered menu: groups of command tables, matching, dispatch, help and the prompt.
//
// Matching follows MenuBuilder section 6: a command runs only when its own match check says
// `Match::Yes` (token count within [min, max], every keyword position equal ignoring case,
// required parameters present). The handler's return value never decides whether dispatch
// stops; only a match does.

use core::fmt;

use crate::line::Tokens;
use crate::syntax::{
    eq_ignore_case, is_reserved, token_at, validate_syntax, validate_table, Kind, TableError,
};
use crate::{
    report_line_error, Args, CmdError, CmdResult, Command, MenuError, MenuOutput, MENU_TOKENS_MAX,
};

/// Group -- A named sub-menu: a keyword, a one-line description and its own command table. Command
/// syntax inside `commands` is relative to the group (`ON`, not `LED ON`).
pub struct Group<C: 'static> {
    pub name: &'static str,
    pub description: &'static str,
    pub commands: &'static [Command<C>],
}

/// Outcome -- what `process_line` did with one line. Everything user-visible is already printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Blank line.
    Empty,
    /// A help listing was printed.
    Help,
    /// A group was entered.
    Entered,
    /// The current group was left.
    Left,
    /// `UP` or `..` at the top level.
    AtTopLevel,
    /// A command matched and its handler ran with this result.
    Ran(CmdResult),
    /// The line was rejected before matching.
    Rejected(MenuError),
    /// A command's keywords matched but its parameter count did not.
    WrongArguments,
    /// No command matched.
    Unknown,
}

/// Result of checking one command against the input tokens.
// One short-lived value per command tried, on the stack: with no allocator there is nothing to box
// `Args` into, and its size is MENU_TOKENS_MAX tokens.
#[allow(clippy::large_enum_variant)]
enum Match<'a> {
    /// All of: token count in range, keywords equal, required parameters present.
    Yes(Args<'a>),
    /// The keywords present matched but the token count is out of range.
    WrongArguments,
    /// A keyword differs, or the definition is invalid.
    No,
}

/// match_command -- the single matcher. Sets `Yes` only after every check passed.
fn match_command<'a, C>(command: &Command<C>, words: &[&'a str]) -> Match<'a> {
    let Ok(shape) = validate_syntax(command.syntax) else {
        return Match::No; // an invalid definition never matches
    };
    let mut args = Args::empty();
    let mut position = 0;
    while let Some(token) = token_at(command.syntax, position) {
        if token.kind == Kind::Rest {
            // Every remaining word is a parameter; the syntax ends here (validate_syntax).
            for &word in words.get(position..).unwrap_or(&[]) {
                if !args.push(word) {
                    return Match::No; // unreachable: a line has at most MENU_TOKENS_MAX words
                }
            }
            break;
        }
        if let Some(&word) = words.get(position) {
            match token.kind {
                Kind::Keyword => {
                    if !eq_ignore_case(token.text, word) {
                        return Match::No;
                    }
                }
                Kind::Required | Kind::Optional | Kind::Rest => {
                    if !args.push(word) {
                        return Match::No; // unreachable: the shape is at most MENU_TOKENS_MAX
                    }
                }
            }
        }
        position += 1;
    }
    let most = if shape.rest {
        usize::MAX
    } else {
        shape.max_tokens
    };
    if (shape.min_tokens..=most).contains(&words.len()) {
        Match::Yes(args)
    } else {
        Match::WrongArguments
    }
}

/// Adapter so `core::fmt` can write into a `MenuOutput`.
struct FmtOut<'a>(&'a mut dyn MenuOutput);

impl fmt::Write for FmtOut<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s);
        Ok(())
    }
}

/// emit -- formatted write to a `MenuOutput`.
pub(crate) fn emit(out: &mut dyn MenuOutput, args: fmt::Arguments<'_>) {
    // FmtOut never fails, so the result carries no information.
    let _ = fmt::Write::write_fmt(&mut FmtOut(out), args);
}

/// A command whose keywords matched but whose parameters did not, kept for the usage line.
#[derive(Clone, Copy)]
struct UsageHint {
    group: Option<&'static str>,
    syntax: &'static str,
}

fn emit_usage(out: &mut dyn MenuOutput, hint: UsageHint) {
    match hint.group {
        Some(group) => emit(out, format_args!("Usage: {} {}\n", group, hint.syntax)),
        None => emit(out, format_args!("Usage: {}\n", hint.syntax)),
    }
}

/// Built-in help entries (MenuBuilder section 9: HELP and ? are listed like any command).
const HELP_ENTRIES: [(&str, &str); 2] = [("HELP", "List commands"), ("?", "Same as HELP")];
const UP_ENTRIES: [(&str, &str); 2] = [("UP", "Leave this group"), ("..", "Same as UP")];

/// print_help -- one aligned block. The separator column is the longest syntax in `entries`
/// plus the two-space indent, so every `-` sits in the same column for any table.
fn print_help<'a>(
    out: &mut dyn MenuOutput,
    title: fmt::Arguments<'_>,
    entries: impl Iterator<Item = (&'a str, &'a str)> + Clone,
) {
    let width = entries
        .clone()
        .map(|(syntax, _)| syntax.len())
        .max()
        .unwrap_or(0);
    emit(out, format_args!("{}\n", title));
    for (syntax, text) in entries {
        emit(
            out,
            format_args!("  {:<width$} - {}\n", syntax, text, width = width),
        );
    }
}

/// Menu -- the tiered menu. `C` is the context type handed to every handler.
pub struct Menu<C: 'static> {
    root: &'static str,
    top: &'static [Command<C>],
    groups: &'static [Group<C>],
    current: Option<usize>,
}

impl<C: 'static> Menu<C> {
    /// new -- A menu at the top level. `root` is the prompt name (`ipmc` gives `ipmc> `). `top` holds
    /// commands typed without a group; `groups` the sub-menus. Check the tables with
    /// `validate_tables` (compile time in firmware, a test on the host).
    pub const fn new(
        root: &'static str,
        top: &'static [Command<C>],
        groups: &'static [Group<C>],
    ) -> Self {
        Menu {
            root,
            top,
            groups,
            current: None,
        }
    }

    /// validate_tables -- every syntax well formed (a group command at most `MENU_TOKENS_MAX - 1`
    /// tokens, so group word + command always fits one line), optional parameters last, no reserved word
    /// as a command start, no two commands with identical keyword sequences in one table.
    /// Groups: one-word unique names that are not reserved, not the first keyword of a top-level
    /// command, and a non-empty table each.
    pub const fn validate_tables(
        top: &[Command<C>],
        groups: &[Group<C>],
    ) -> Result<(), TableError> {
        if let Err(e) = validate_table(top) {
            return Err(e);
        }
        let mut i = 0;
        while i < groups.len() {
            let name = groups[i].name;
            if validate_syntax(name).is_err()
                || token_at(name, 1).is_some()
                || !matches!(token_at(name, 0), Some(t) if matches!(t.kind, Kind::Keyword))
            {
                return Err(TableError::BadGroupName);
            }
            if is_reserved(name) {
                return Err(TableError::ReservedKeyword);
            }
            if groups[i].commands.is_empty() {
                return Err(TableError::EmptyGroup);
            }
            if let Err(e) = validate_table(groups[i].commands) {
                return Err(e);
            }
            // The full form (group word + command) is one line and must tokenize, so a group
            // command may use at most MENU_TOKENS_MAX - 1 tokens.
            let mut c = 0;
            while c < groups[i].commands.len() {
                match validate_syntax(groups[i].commands[c].syntax) {
                    Ok(shape) if shape.max_tokens < MENU_TOKENS_MAX => {}
                    _ => return Err(TableError::SyntaxTooLong),
                }
                c += 1;
            }
            let mut j = 0;
            while j < top.len() {
                if let Some(first) = token_at(top[j].syntax, 0) {
                    if eq_ignore_case(first.text, name) {
                        return Err(TableError::GroupNameClash);
                    }
                }
                j += 1;
            }
            let mut k = i + 1;
            while k < groups.len() {
                if eq_ignore_case(groups[k].name, name) {
                    return Err(TableError::DuplicateGroup);
                }
                k += 1;
            }
            i += 1;
        }
        Ok(())
    }

    /// current_group -- name of the group the menu is in, or None at the top level.
    pub fn current_group(&self) -> Option<&'static str> {
        self.group_at(self.current).map(|g| g.name)
    }

    fn group_at(&self, index: Option<usize>) -> Option<&'static Group<C>> {
        index.and_then(|i| self.groups.get(i))
    }

    fn find_group(&self, word: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| eq_ignore_case(g.name, word))
    }

    /// write_prompt -- `ipmc> ` at the top level, `ipmc/sensors> ` inside a group.
    pub fn write_prompt(&self, out: &mut dyn MenuOutput) {
        emit(out, format_args!("{}", self.root));
        if let Some(group) = self.group_at(self.current) {
            emit(out, format_args!("/"));
            for c in group.name.chars() {
                emit(out, format_args!("{}", c.to_ascii_lowercase()));
            }
        }
        emit(out, format_args!("> "));
    }

    /// process_line -- runs one input line.
    ///
    /// 1. Split into tokens; reject an overlong or over-tokenised line (printed, nothing runs).
    /// 2. `HELP` / `?` alone lists the current level; `UP` / `..` alone leaves a group.
    /// 3. Inside a group, that group's table is tried first.
    /// 4. A first token equal to a group name enters the group (alone), lists it (`HELP`/`?`)
    ///    or runs the rest of the line against the group's table.
    /// 5. The top-level table is tried.
    /// 6. Nothing matched: a usage line when some command's keywords matched, else the
    ///    unknown-command error and the hint.
    ///
    /// Dispatch stops on a match, never on a handler's return value.
    pub fn process_line(&mut self, line: &str, ctx: &mut C, out: &mut dyn MenuOutput) -> Outcome {
        let tokens = match Tokens::split(line) {
            Ok(tokens) => tokens,
            Err(error) => {
                report_line_error(error, out);
                return Outcome::Rejected(error);
            }
        };
        let words = tokens.as_slice();
        let Some((&first, rest)) = words.split_first() else {
            return Outcome::Empty;
        };
        if rest.is_empty() {
            if eq_ignore_case(first, "HELP") || eq_ignore_case(first, "?") {
                self.print_current_help(out);
                return Outcome::Help;
            }
            if eq_ignore_case(first, "UP") || eq_ignore_case(first, "..") {
                return self.leave(out);
            }
        }

        let mut hint: Option<UsageHint> = None;
        if let Some(group) = self.group_at(self.current) {
            if let Some(outcome) =
                run_table(group.commands, words, Some(group.name), ctx, out, &mut hint)
            {
                return outcome;
            }
        }
        if let Some(index) = self.find_group(first) {
            let Some(group) = self.group_at(Some(index)) else {
                return Outcome::Unknown;
            };
            if rest.is_empty() {
                self.current = Some(index);
                return Outcome::Entered;
            }
            if rest.len() == 1 && (eq_ignore_case(rest[0], "HELP") || eq_ignore_case(rest[0], "?"))
            {
                print_group_help(out, group);
                return Outcome::Help;
            }
            if let Some(outcome) =
                run_table(group.commands, rest, Some(group.name), ctx, out, &mut hint)
            {
                return outcome;
            }
        }
        if let Some(outcome) = run_table(self.top, words, None, ctx, out, &mut hint) {
            return outcome;
        }
        match hint {
            Some(hint) => {
                emit(out, format_args!("Error: wrong arguments\n"));
                emit_usage(out, hint);
                Outcome::WrongArguments
            }
            None => {
                emit(
                    out,
                    format_args!(
                        "Error: unknown command '{}'\nType HELP or ?\n",
                        line.trim_matches(|c: char| c.is_ascii_whitespace())
                    ),
                );
                Outcome::Unknown
            }
        }
    }

    fn leave(&mut self, out: &mut dyn MenuOutput) -> Outcome {
        if self.current.take().is_some() {
            Outcome::Left
        } else {
            emit(out, format_args!("Already at the top level\n"));
            Outcome::AtTopLevel
        }
    }

    fn print_current_help(&self, out: &mut dyn MenuOutput) {
        if let Some(group) = self.group_at(self.current) {
            print_group_help(out, group);
            return;
        }
        let entries = self
            .groups
            .iter()
            .map(|g| (g.name, g.description))
            .chain(self.top.iter().map(|c| (c.syntax, c.help)))
            .chain(HELP_ENTRIES);
        print_help(out, format_args!("Groups and commands:"), entries);
        emit(
            out,
            format_args!(
                "Enter a group by name, or type <GROUP> HELP. Full commands work from any level.\n"
            ),
        );
    }
}

fn print_group_help<C>(out: &mut dyn MenuOutput, group: &Group<C>) {
    let entries = group
        .commands
        .iter()
        .map(|c| (c.syntax, c.help))
        .chain(HELP_ENTRIES)
        .chain(UP_ENTRIES);
    print_help(out, format_args!("{} commands:", group.name), entries);
    emit(
        out,
        format_args!("From any level: {} <COMMAND>\n", group.name),
    );
}

/// run_table -- tries each command of `table` in order. Returns the outcome of the first one
/// that matched; None when none did (a keywords-only match is recorded in `hint`).
fn run_table<C>(
    table: &'static [Command<C>],
    words: &[&str],
    group: Option<&'static str>,
    ctx: &mut C,
    out: &mut dyn MenuOutput,
    hint: &mut Option<UsageHint>,
) -> Option<Outcome> {
    for command in table {
        match match_command(command, words) {
            Match::Yes(args) => {
                let result = (command.handler)(ctx, &args);
                if let Err(CmdError::Arg(error)) = result {
                    emit(out, format_args!("Error: {}\n", error.name()));
                    emit_usage(
                        out,
                        UsageHint {
                            group,
                            syntax: command.syntax,
                        },
                    );
                }
                return Some(Outcome::Ran(result));
            }
            Match::WrongArguments => {
                hint.get_or_insert(UsageHint {
                    group,
                    syntax: command.syntax,
                });
            }
            Match::No => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArgError, MENU_LINE_MAX};

    /// Test context: records which handler ran.
    #[derive(Default)]
    struct Ctx {
        log: Vec<String>,
    }

    /// Collects output text.
    #[derive(Default)]
    struct Sink {
        text: String,
    }

    impl MenuOutput for Sink {
        fn write_str(&mut self, s: &str) {
            self.text.push_str(s);
        }
    }

    fn rec(ctx: &mut Ctx, name: &str, args: &Args<'_>) {
        let mut entry = String::from(name);
        for i in 0..args.len() {
            entry.push(' ');
            entry.push_str(args.get(i).unwrap());
        }
        ctx.log.push(entry);
    }

    fn set_mode(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "set_mode", a);
        Ok(())
    }
    fn set_param(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "set_param", a);
        Ok(())
    }
    fn get_status(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "get_status", a);
        Ok(())
    }
    fn on(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "on", a);
        Ok(())
    }
    fn blink(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "blink", a);
        let _hz = a.u32_at(0)?;
        Ok(())
    }
    fn reported(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "reported", a);
        Err(CmdError::Reported)
    }
    fn show(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "show", a);
        Ok(())
    }
    fn status_alias(c: &mut Ctx, a: &Args<'_>) -> CmdResult {
        rec(c, "alias", a);
        Ok(())
    }

    const fn cmd(
        syntax: &'static str,
        handler: fn(&mut Ctx, &Args<'_>) -> CmdResult,
    ) -> Command<Ctx> {
        Command {
            syntax,
            help: "text",
            handler,
        }
    }

    static LED: [Command<Ctx>; 3] = [
        cmd("ON", on),
        cmd("BLINK [HZ]", blink),
        cmd("REPORTED", reported),
    ];
    static SENSORS: [Command<Ctx>; 2] = [
        cmd("SHOW {NAME}", show),
        cmd("LONG SYNTAX [A] [B] {C}", show),
    ];
    static GROUPS: [Group<Ctx>; 2] = [
        Group {
            name: "LED",
            description: "LED control",
            commands: &LED,
        },
        Group {
            name: "SENSORS",
            description: "Sensor readings",
            commands: &SENSORS,
        },
    ];
    static TOP: [Command<Ctx>; 4] = [
        cmd("SET MODE [MODE] {LEVEL}", set_mode),
        cmd("SET PARAM [KEY]", set_param),
        cmd("GET STATUS {DETAIL}", get_status),
        cmd("STATUS", status_alias),
    ];

    fn menu() -> Menu<Ctx> {
        Menu::new("ipmc", &TOP, &GROUPS)
    }

    fn feed(menu: &mut Menu<Ctx>, line: &str) -> (Outcome, Ctx, String) {
        let (mut ctx, mut sink) = (Ctx::default(), Sink::default());
        let outcome = menu.process_line(line, &mut ctx, &mut sink);
        (outcome, ctx, sink.text)
    }

    #[test]
    fn shipped_test_tables_validate() {
        assert_eq!(Menu::validate_tables(&TOP, &GROUPS), Ok(()));
    }

    /// Text -- what a line must print: nothing is checked for `Any`.
    enum Text {
        Any,
        Exact(&'static str),
        Contains(&'static str),
    }

    /// LineCase -- one line fed to a fresh top-level menu: the outcome, the handlers that ran
    /// (as `rec` logs them) and the printed text.
    struct LineCase {
        name: &'static str,
        line: &'static str,
        outcome: Outcome,
        log: &'static [&'static str],
        text: Text,
    }

    #[test]
    fn single_lines_from_the_top_level_follow_the_table() {
        let cases = [
            LineCase {
                // SET MODE [MODE] and SET PARAM [KEY] both accept 3 tokens. A line for the second
                // must not run the first, whatever order the table has.
                name: "matching is on keywords, not on token count",
                line: "set param abc",
                outcome: Outcome::Ran(Ok(())),
                log: &["set_param abc"],
                text: Text::Any,
            },
            LineCase {
                // A matched command that fails ends dispatch too: it runs once. Reported: the
                // handler printed, the engine adds nothing.
                name: "handler error does not continue dispatch",
                line: "led reported",
                outcome: Outcome::Ran(Err(CmdError::Reported)),
                log: &["reported"],
                text: Text::Exact(""),
            },
            LineCase {
                name: "required parameter must be present",
                line: "set mode",
                outcome: Outcome::WrongArguments,
                log: &[],
                text: Text::Exact("Error: wrong arguments\nUsage: SET MODE [MODE] {LEVEL}\n"),
            },
            LineCase {
                name: "too many tokens for the command is not a match",
                line: "set mode fast 3 extra",
                outcome: Outcome::WrongArguments,
                log: &[],
                text: Text::Contains("Usage: SET MODE [MODE] {LEVEL}"),
            },
            LineCase {
                name: "too many words in the line is rejected",
                // MENU_TOKENS_MAX (40) + 1 words
                line: "a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a a",
                outcome: Outcome::Rejected(MenuError::TooManyTokens),
                log: &[],
                text: Text::Exact("Error: too many words (max 40)\n"),
            },
            LineCase {
                name: "unknown command prints one error line and the hint",
                line: "  frobnicate  now ",
                outcome: Outcome::Unknown,
                log: &[],
                text: Text::Exact("Error: unknown command 'frobnicate  now'\nType HELP or ?\n"),
            },
            LineCase {
                name: "blank line prints nothing",
                line: "   ",
                outcome: Outcome::Empty,
                log: &[],
                text: Text::Exact(""),
            },
            LineCase {
                name: "group help form lists without entering",
                line: "led help",
                outcome: Outcome::Help,
                log: &[],
                text: Text::Exact(concat!(
                    "LED commands:\n",
                    "  ON         - text\n",
                    "  BLINK [HZ] - text\n",
                    "  REPORTED   - text\n",
                    "  HELP       - List commands\n",
                    "  ?          - Same as HELP\n",
                    "  UP         - Leave this group\n",
                    "  ..         - Same as UP\n",
                    "From any level: LED <COMMAND>\n",
                )),
            },
            LineCase {
                name: "a top-level command can alias a group command",
                line: "status",
                outcome: Outcome::Ran(Ok(())),
                log: &["alias"],
                text: Text::Any,
            },
            LineCase {
                name: "argument errors print the fault and usage",
                line: "led blink fast",
                outcome: Outcome::Ran(Err(CmdError::Arg(ArgError::NotANumber))),
                log: &["blink fast"],
                text: Text::Exact("Error: not a number\nUsage: LED BLINK [HZ]\n"),
            },
            LineCase {
                name: "a full-path command works from the top level",
                line: "SENSORS SHOW",
                outcome: Outcome::Ran(Ok(())),
                log: &["show"],
                text: Text::Any,
            },
        ];
        for case in cases {
            let mut m = menu();
            let (outcome, ctx, text) = feed(&mut m, case.line);
            assert_eq!(outcome, case.outcome, "{}", case.name);
            assert_eq!(ctx.log, case.log, "{}", case.name);
            match case.text {
                Text::Any => {}
                Text::Exact(want) => assert_eq!(text, want, "{}", case.name),
                Text::Contains(want) => assert!(text.contains(want), "{}: {text:?}", case.name),
            }
            // A single line from the top level never moves the menu.
            assert_eq!(m.current_group(), None, "{}", case.name);
        }
    }

    #[test]
    fn non_matching_command_success_does_not_stop_dispatch() {
        // The first table entry returns Ok from its handler but never matches; the second must
        // still be reached.
        let mut m = menu();
        let (_, ctx, _) = feed(&mut m, "SET PARAM x");
        assert_eq!(ctx.log.len(), 1);
        assert!(ctx.log[0].starts_with("set_param"));
    }

    #[test]
    fn keywords_are_case_insensitive_and_exact() {
        let mut m = menu();
        for line in ["get status", "GET STATUS", "GeT sTaTuS"] {
            let (outcome, ctx, _) = feed(&mut m, line);
            assert_eq!(outcome, Outcome::Ran(Ok(())), "{line}");
            assert_eq!(ctx.log, ["get_status"]);
        }
        for line in ["get stat", "get statuss", "ge status", "gets tatus"] {
            let (outcome, ctx, _) = feed(&mut m, line);
            assert_eq!(outcome, Outcome::Unknown, "{line}");
            assert!(ctx.log.is_empty(), "{line}");
        }
    }

    #[test]
    fn optional_parameter_may_be_left_out_or_given() {
        let mut m = menu();
        let (_, ctx, _) = feed(&mut m, "set mode fast");
        assert_eq!(ctx.log, ["set_mode fast"]);
        let (_, ctx, _) = feed(&mut m, "set mode fast 3");
        assert_eq!(ctx.log, ["set_mode fast 3"]);
        let (_, ctx, _) = feed(&mut m, "get status");
        assert_eq!(ctx.log, ["get_status"]);
        let (_, ctx, _) = feed(&mut m, "get status full");
        assert_eq!(ctx.log, ["get_status full"]);
    }

    #[test]
    fn overlong_line_is_rejected_not_truncated() {
        let mut m = menu();
        let line = format!("get status {}", "x".repeat(MENU_LINE_MAX));
        let (outcome, ctx, text) = feed(&mut m, &line);
        assert_eq!(outcome, Outcome::Rejected(MenuError::LineTooLong));
        assert!(ctx.log.is_empty());
        assert_eq!(text, "Error: line too long (max 128 characters)\n");
    }

    #[test]
    fn top_level_help_lists_groups_commands_and_aliases() {
        let mut m = menu();
        let (a, _, help) = feed(&mut m, "help");
        let (b, _, question) = feed(&mut m, "?");
        assert_eq!((a, b), (Outcome::Help, Outcome::Help));
        assert_eq!(help, question);
        for needle in [
            "LED",
            "SENSORS",
            "SET MODE [MODE] {LEVEL}",
            "STATUS",
            "HELP",
            "?",
        ] {
            assert!(
                help.lines().any(|l| l.trim_start().starts_with(needle)),
                "{needle}"
            );
        }
        // Group commands are not listed at the top level.
        assert!(!help.contains("BLINK"));
    }

    /// Column of the `-` separator on every table line of a help text.
    fn separator_columns(help: &str) -> Vec<usize> {
        help.lines()
            .filter(|l| l.starts_with("  "))
            .map(|l| l.find(" - ").unwrap() + 1)
            .collect()
    }

    #[test]
    fn help_separator_column_follows_the_longest_syntax() {
        let mut m = menu();
        let (_, _, help) = feed(&mut m, "help");
        let cols = separator_columns(&help);
        assert!(cols.len() >= 8);
        // Longest top-level syntax: "SET MODE [MODE] {LEVEL}" (23); indent 2, one space.
        let width = "SET MODE [MODE] {LEVEL}".len();
        assert!(cols.iter().all(|&c| c == 2 + width + 1), "{cols:?}\n{help}");
        // Descriptions start in one column too.
        let starts: Vec<usize> = help
            .lines()
            .filter(|l| l.starts_with("  "))
            .map(|l| l.find(" - ").unwrap() + 3)
            .collect();
        assert!(starts.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn group_help_aligns_on_its_own_longest_syntax() {
        let mut m = menu();
        let (_, _, help) = feed(&mut m, "sensors help");
        let width = "LONG SYNTAX [A] [B] {C}".len();
        let cols = separator_columns(&help);
        // Two table commands plus HELP, ?, UP and ..
        assert_eq!(cols.len(), 6, "{help}");
        assert!(cols.iter().all(|&c| c == 2 + width + 1), "{cols:?}\n{help}");
        // Aliases are listed the same way as commands.
        for needle in ["HELP", "?", "UP", ".."] {
            assert!(
                help.lines().any(|l| l.trim_start().starts_with(needle)),
                "{needle}"
            );
        }
        let (_, _, led) = feed(&mut m, "led ?");
        let width = "BLINK [HZ]".len();
        let cols = separator_columns(&led);
        // Three table commands plus HELP, ?, UP and ..
        assert_eq!(cols.len(), 7, "{led}");
        assert!(cols.iter().all(|&c| c == 2 + width + 1), "{led}");
        for needle in ["ON", "BLINK [HZ]", "REPORTED", "HELP", "?", "UP", ".."] {
            assert!(
                led.lines().any(|l| l.trim_start().starts_with(needle)),
                "{needle}: {led}"
            );
        }
    }

    #[test]
    fn help_with_extra_words_is_not_help() {
        let mut m = menu();
        let (outcome, _, _) = feed(&mut m, "help me");
        assert_eq!(outcome, Outcome::Unknown);
    }

    #[test]
    fn entering_and_leaving_a_group_changes_the_prompt() {
        let mut m = menu();
        let prompt = |m: &Menu<Ctx>| {
            let mut sink = Sink::default();
            m.write_prompt(&mut sink);
            sink.text
        };
        assert_eq!(prompt(&m), "ipmc> ");
        assert_eq!(feed(&mut m, "SENSORS").0, Outcome::Entered);
        assert_eq!(prompt(&m), "ipmc/sensors> ");
        assert_eq!(m.current_group(), Some("SENSORS"));
        assert_eq!(feed(&mut m, "..").0, Outcome::Left);
        assert_eq!(prompt(&m), "ipmc> ");
        assert_eq!(feed(&mut m, "led").0, Outcome::Entered);
        assert_eq!(feed(&mut m, "up").0, Outcome::Left);
        let (outcome, _, text) = feed(&mut m, "UP");
        assert_eq!(outcome, Outcome::AtTopLevel);
        assert_eq!(text, "Already at the top level\n");
    }

    #[test]
    fn inside_a_group_commands_work_without_the_prefix() {
        let mut m = menu();
        feed(&mut m, "led");
        let (outcome, ctx, _) = feed(&mut m, "blink 5");
        assert_eq!(outcome, Outcome::Ran(Ok(())));
        assert_eq!(ctx.log, ["blink 5"]);
        // HELP lists the group, not the top level.
        let (_, _, help) = feed(&mut m, "help");
        assert!(help.starts_with("LED commands:"));
        assert!(help.contains("BLINK [HZ]"));
    }

    #[test]
    fn full_path_command_works_from_inside_another_group() {
        let mut m = menu();
        feed(&mut m, "sensors");
        let (outcome, ctx, _) = feed(&mut m, "led on");
        assert_eq!(outcome, Outcome::Ran(Ok(())));
        assert_eq!(ctx.log, ["on"]);
        assert_eq!(m.current_group(), Some("SENSORS")); // a full command does not move the menu
                                                        // Top-level commands also work from inside a group.
        let (_, ctx, _) = feed(&mut m, "get status");
        assert_eq!(ctx.log, ["get_status"]);
        // A group's own commands do not leak into another group without the prefix.
        let (outcome, _, _) = feed(&mut m, "blink 5");
        assert_eq!(outcome, Outcome::Unknown);
    }

    #[test]
    fn a_group_name_enters_another_group_directly() {
        let mut m = menu();
        feed(&mut m, "led");
        assert_eq!(feed(&mut m, "sensors").0, Outcome::Entered);
        assert_eq!(m.current_group(), Some("SENSORS"));
    }

    #[test]
    fn unknown_command_inside_a_group_reports_the_line() {
        let mut m = menu();
        feed(&mut m, "led");
        let (outcome, _, text) = feed(&mut m, "spin");
        assert_eq!(outcome, Outcome::Unknown);
        assert!(text.starts_with("Error: unknown command 'spin'\n"));
        let (outcome, _, text) = feed(&mut m, "led spin");
        assert_eq!(outcome, Outcome::Unknown);
        assert!(text.contains("'led spin'"));
    }

    // ---- table validation failures ----

    static BAD_DUP: [Command<Ctx>; 2] = [cmd("A B", on), cmd("a b {X}", on)];
    static BAD_OPT: [Command<Ctx>; 1] = [cmd("A {X} [Y]", on)];
    static BAD_EMPTY: [Command<Ctx>; 1] = [cmd("  ", on)];
    static BAD_RESERVED: [Command<Ctx>; 1] = [cmd("help me", on)];
    static GOOD: [Command<Ctx>; 1] = [cmd("ON", on)];

    fn group(name: &'static str, commands: &'static [Command<Ctx>]) -> Group<Ctx> {
        Group {
            name,
            description: "d",
            commands,
        }
    }

    #[test]
    fn validation_rejects_each_table_fault() {
        let ok: [Group<Ctx>; 0] = [];
        assert_eq!(
            Menu::validate_tables(&BAD_DUP, &ok),
            Err(TableError::DuplicateKeywords)
        );
        assert_eq!(
            Menu::validate_tables(&BAD_OPT, &ok),
            Err(TableError::OptionalNotLast)
        );
        assert_eq!(
            Menu::validate_tables(&BAD_EMPTY, &ok),
            Err(TableError::EmptySyntax)
        );
        assert_eq!(
            Menu::validate_tables(&BAD_RESERVED, &ok),
            Err(TableError::ReservedKeyword)
        );
        // Faults inside a group table are found too.
        let g = [group("X", &BAD_DUP)];
        assert_eq!(
            Menu::validate_tables(&[], &g),
            Err(TableError::DuplicateKeywords)
        );
    }

    #[test]
    fn group_command_syntax_leaves_room_for_the_group_word() {
        let words = |n: usize| -> &'static str {
            let text: Vec<String> = (0..n)
                .map(|i| {
                    format!(
                        "{}{}",
                        (b'A' + (i / 26) as u8) as char,
                        (b'A' + (i % 26) as u8) as char
                    )
                })
                .collect();
            Box::leak(text.join(" ").into_boxed_str())
        };
        let none: [Command<Ctx>; 0] = [];
        // MAX - 1 tokens fit with the group word; MAX tokens do not (but fit at the top level).
        let fits: &'static [Command<Ctx>] =
            Box::leak(Box::new([cmd(words(MENU_TOKENS_MAX - 1), on)]));
        let full: &'static [Command<Ctx>] = Box::leak(Box::new([cmd(words(MENU_TOKENS_MAX), on)]));
        assert_eq!(Menu::validate_tables(&none, &[group("G", fits)]), Ok(()));
        assert_eq!(
            Menu::validate_tables(&none, &[group("G", full)]),
            Err(TableError::SyntaxTooLong)
        );
        assert_eq!(Menu::validate_tables(full, &[]), Ok(()));
        // The accepted full form tokenizes from the root.
        let line = format!("G {}", words(MENU_TOKENS_MAX - 1));
        assert!(Tokens::split(&line).is_ok());
    }

    #[test]
    fn validation_rejects_each_group_fault() {
        let none: [Command<Ctx>; 0] = [];
        let g = [group("TWO WORDS", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::BadGroupName)
        );
        let g = [group("", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::BadGroupName)
        );
        let g = [group("[P]", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::BadGroupName)
        );
        let g = [group("help", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::ReservedKeyword)
        );
        let g = [group("EMPTY", &[])];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::EmptyGroup)
        );
        let g = [group("LED", &GOOD), group("led", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&none, &g),
            Err(TableError::DuplicateGroup)
        );
        let g = [group("STATUS", &GOOD)];
        assert_eq!(
            Menu::validate_tables(&TOP, &g),
            Err(TableError::GroupNameClash)
        );
    }

    #[test]
    fn invalid_definition_never_matches() {
        let bad = cmd("A [", on);
        assert!(matches!(match_command(&bad, &["a", "x"]), Match::No));
    }

    #[test]
    fn rest_token_takes_zero_one_and_many_words() {
        let c = cmd("RUN [A] {WORDS...}", show);
        let args = |words: &[&str]| match match_command(&c, words) {
            Match::Yes(args) => Some(
                (0..args.len())
                    .map(|i| args.get(i).unwrap().to_string())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        };
        assert_eq!(args(&["run", "a"]), Some(vec!["a".to_string()]));
        assert_eq!(
            args(&["run", "a", "b"]),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        let many = ["run", "a", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
        assert_eq!(args(&many).map(|v| v.len()), Some(many.len() - 1));
        assert!(matches!(match_command(&c, &["run"]), Match::WrongArguments));
        assert!(matches!(match_command(&c, &["go", "a"]), Match::No));
    }

    #[test]
    fn rest_token_not_last_fails_table_validation() {
        let none: [Command<Ctx>; 0] = [];
        let bad: &'static [Command<Ctx>] = Box::leak(Box::new([cmd("RUN {W...} [A]", on)]));
        assert_eq!(
            Menu::validate_tables(bad, &[]),
            Err(TableError::RestNotLast)
        );
        assert_eq!(
            Menu::validate_tables(&none, &[group("G", bad)]),
            Err(TableError::RestNotLast)
        );
        let good: &'static [Command<Ctx>] = Box::leak(Box::new([cmd("RUN [A] {W...}", on)]));
        assert_eq!(Menu::validate_tables(&none, &[group("G", good)]), Ok(()));
    }

    #[test]
    fn match_signal_is_explicit_per_command() {
        let c = cmd("SET MODE [M]", set_mode);
        assert!(matches!(
            match_command(&c, &["set", "mode", "x"]),
            Match::Yes(_)
        ));
        assert!(matches!(
            match_command(&c, &["set", "param", "x"]),
            Match::No
        ));
        assert!(matches!(
            match_command(&c, &["set", "mode"]),
            Match::WrongArguments
        ));
        assert!(matches!(match_command(&c, &["set"]), Match::WrongArguments));
        assert!(matches!(
            match_command(&c, &["mode", "set", "x"]),
            Match::No
        ));
        assert!(matches!(
            match_command(&c, &["set", "mode", "x", "y"]),
            Match::WrongArguments
        ));
    }
}
