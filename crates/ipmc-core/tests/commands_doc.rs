//! commands_doc.rs - Documentation/ipmi_commands.md must show exactly the COMMANDS rows as
//! implemented.
//!
//! The document's generated parts (build list, last column of the IPMI and VSO tables) come
//! from `python -I tools/sync_ipmi_commands.py`. This test fails, naming the command, while
//! (a) a COMMANDS row is not shown as implemented, (b) the document shows a command as
//! implemented that is not in COMMANDS, or (c) an implemented row carries a command name or a
//! spec reference that differs from commands.rs. Fix: run the sync tool. The Broadcast "Get Device ID"
//! row shares its NetFn and command code with Get Device ID but belongs to the link layer, so
//! it is checked by the sync tool (link.rs flag), not here.

use ipmc_core::commands::COMMANDS;
use ipmc_core::ipmb::NETFN_GROUP_EXT;
use std::fs;
use std::path::Path;

/// One command row of the document: where it was found and what it says.
struct DocRow {
    section: u8,
    netfn: u8,
    cmd: u8,
    name: String,
    implemented: bool,
    /// Spec reference in the last cell `implemented (<handler>; <name>; <spec>)`; None for the
    /// build list (section 1), whose last cell is only the status.
    spec: Option<String>,
    /// Name in the last cell, as above.
    cell_name: Option<String>,
}

/// status_parts -- (name, spec) of an `implemented (<handler>; <name>; <spec>)` cell.
fn status_parts(cell: &str) -> Option<(String, String)> {
    let inner = cell.strip_prefix("implemented (")?.strip_suffix(')')?;
    let mut parts = inner.split("; ");
    let (_handler, name, spec) = (parts.next()?, parts.next()?, parts.next()?);
    parts
        .next()
        .is_none()
        .then(|| (name.to_string(), spec.to_string()))
}

/// cell_byte -- "0Ah" -> Some(0x0A): exactly two hex digits and the `h` suffix.
fn cell_byte(cell: &str) -> Option<u8> {
    let digits = cell.strip_suffix('h')?;
    if digits.len() != 2 {
        return None;
    }
    ipmc_wire::hex_byte(digits)
}

/// doc_rows -- every command row of sections 1 to 3 of the document, without Broadcast rows.
fn doc_rows(text: &str) -> Vec<DocRow> {
    let mut rows = Vec::new();
    let mut section = 0u8;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            section = match rest.chars().next() {
                Some('1') => 1,
                Some('2') => 2,
                Some('3') => 3,
                _ => 0,
            };
            continue;
        }
        let line = line.trim();
        if section == 0 || !line.starts_with('|') || !line.ends_with('|') {
            continue;
        }
        let cells: Vec<&str> = line[1..line.len() - 1].split('|').map(str::trim).collect();
        let (Some(first), Some(last)) = (cells.first(), cells.last()) else {
            continue;
        };
        let (netfn, cmd, name) = match section {
            1 => match (cell_byte(first), cells.get(1).and_then(|c| cell_byte(c))) {
                (Some(n), Some(c)) => (n, c, cells.get(2).copied().unwrap_or("")),
                _ => continue,
            },
            2 => {
                let Some((n, c)) = first.split_once('/') else {
                    continue;
                };
                match (cell_byte(n), cell_byte(c)) {
                    (Some(n), Some(c)) => (n, c, cells.get(1).copied().unwrap_or("")),
                    _ => continue,
                }
            }
            _ => match cell_byte(first) {
                Some(c) => (NETFN_GROUP_EXT, c, cells.get(1).copied().unwrap_or("")),
                None => continue,
            },
        };
        if name.contains("Broadcast") {
            continue;
        }
        let parts = if section == 1 {
            None
        } else {
            status_parts(last)
        };
        rows.push(DocRow {
            section,
            netfn,
            cmd,
            name: name.to_string(),
            implemented: last.starts_with("implemented"),
            spec: parts.as_ref().map(|p| p.1.clone()),
            cell_name: parts.map(|p| p.0),
        });
    }
    rows
}

fn read_doc() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Documentation/ipmi_commands.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_commands_row_is_shown_as_implemented() {
    let rows = doc_rows(&read_doc());
    assert!(
        !rows.is_empty(),
        "no command rows found in ipmi_commands.md"
    );
    let mut missing = Vec::new();
    for command in COMMANDS {
        for section in [1u8, 2, 3] {
            let mut in_section = rows
                .iter()
                .filter(|r| {
                    r.section == section && (r.netfn, r.cmd) == (command.netfn, command.cmd)
                })
                .peekable();
            // Section 1 lists the command; exactly one of sections 2 and 3 holds its table row.
            let shown = in_section.peek().is_some();
            let all_implemented = in_section.all(|r| r.implemented);
            let needed = section == 1 || (section == 2) == (command.netfn != NETFN_GROUP_EXT);
            if needed && !(shown && all_implemented) {
                missing.push(format!(
                    "{} ({:02X}h/{:02X}h) is in COMMANDS but section {section} of ipmi_commands.md does not show it as implemented",
                    command.name, command.netfn, command.cmd
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "run `python -I tools/sync_ipmi_commands.py`:\n{}",
        missing.join("\n")
    );
}

#[test]
fn implemented_rows_carry_the_name_and_spec_of_commands_rs() {
    let rows = doc_rows(&read_doc());
    let mut wrong = Vec::new();
    for command in COMMANDS {
        for r in rows
            .iter()
            .filter(|r| (r.netfn, r.cmd) == (command.netfn, command.cmd) && r.implemented)
        {
            let cell_ok = r.section == 1
                || (r.cell_name.as_deref() == Some(command.name)
                    && r.spec.as_deref() == Some(command.spec));
            if r.name != command.name || !cell_ok {
                wrong.push(format!(
                    "{} ({:02X}h/{:02X}h): section {} shows name {:?} / cell {:?} / spec {:?}, commands.rs has {:?} / {:?}",
                    command.name, command.netfn, command.cmd, r.section, r.name,
                    r.cell_name, r.spec, command.name, command.spec
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "run `python -I tools/sync_ipmi_commands.py`:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_extra_command_is_shown_as_implemented() {
    let extra: Vec<String> = doc_rows(&read_doc())
        .iter()
        .filter(|r| r.implemented)
        .filter(|r| {
            !COMMANDS
                .iter()
                .any(|c| (c.netfn, c.cmd) == (r.netfn, r.cmd))
        })
        .map(|r| {
            format!(
                "{} ({:02X}h/{:02X}h) is shown as implemented in section {} of ipmi_commands.md but is not in COMMANDS",
                r.name, r.netfn, r.cmd, r.section
            )
        })
        .collect();
    assert!(
        extra.is_empty(),
        "run `python -I tools/sync_ipmi_commands.py`:\n{}",
        extra.join("\n")
    );
}
