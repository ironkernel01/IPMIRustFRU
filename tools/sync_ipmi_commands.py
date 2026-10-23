#!/usr/bin/env python3
"""sync_ipmi_commands.py - keep Documentation/ipmi_commands.md in step with the code.

Source of truth: the COMMANDS rows of crates/ipmc-core/src/commands.rs, one line each:
    cmd(NETFN_APP, 0x01, "Get Device ID", "IPMI 2.0 20.1", app::get_device_id),
The NetFn names map to numbers through the NETFN_* consts of crates/ipmc-core/src/ipmb.rs, read
from the source. The tool rewrites only the generated regions of the document, each between
    <!-- generated: sync_ipmi_commands.py -->   and   <!-- end generated -->
  - section 1: the build list table (every Tier-1 mandatory command with its Status, plus any
    implemented command that is not mandatory);
  - sections 2 and 3: the last column "This IPMC (Tier 1)" of the IPMI and VSO tables.
Spec columns (M/O/NA, sections, pages) and everything outside the regions are kept byte for
byte. The first run inserts the markers around the three tables.

Last column values:
  implemented (<handler path>; <name>; <spec>)
                                     a COMMANDS row: the handler path, the log name and the spec
                                     reference, all three read from commands.rs (the section 1
                                     name cell of an implemented row is rewritten from it too;
                                     crates/ipmc-core/tests/commands_doc.rs compares them)
  planned (Tier-1 mandatory)         Tier-1 cell is M and no COMMANDS row yet
  C1h (NA for Tier 1)                Tier-1 cell is NA
  C1h (optional, not implemented)    any other Tier-1 cell
The Broadcast "Get Device ID" row is implemented only when crates/ipmc-core/src/link.rs exists
and contains `pub const BROADCAST_GET_DEVICE_ID: bool = true;`, else
`planned (Phase 3 link layer)`.

Usage:  python -I tools/sync_ipmi_commands.py           rewrite the document
        python -I tools/sync_ipmi_commands.py --check   write nothing; exit 1 with a diff
                                                        summary when the document is stale
Exit codes: 0 up to date (or written), 1 stale under --check, 2 input error.
"""

import difflib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COMMANDS_RS = ROOT / "crates" / "ipmc-core" / "src" / "commands.rs"
IPMB_RS = ROOT / "crates" / "ipmc-core" / "src" / "ipmb.rs"
LINK_RS = ROOT / "crates" / "ipmc-core" / "src" / "link.rs"
DOC = ROOT / "Documentation" / "ipmi_commands.md"

BEGIN = "<!-- generated: sync_ipmi_commands.py -->"
END = "<!-- end generated -->"
BROADCAST_FLAG = "pub const BROADCAST_GET_DEVICE_ID: bool = true;"
BROADCAST_PATH = "link::BROADCAST_GET_DEVICE_ID"
NETFN_GROUP_EXT = 0x2C  # VITA 46.11 VSO requests (Table 10.1.2-1)

COL_STATUS = "This IPMC (Tier 1)"
BUILD_HEADER = "| NetFn | Cmd | Command | Tier-1 IPMC | Defined in | Page | Status |"
BUILD_SEPARATOR = "|---|---|---|---|---|---|---|"

# Table headers that start each generated table, by section.
SEC1_HEAD = "| NetFn | Cmd | Command | Tier-1 IPMC |"
SEC2_HEAD = "| Cmd | Command | IPMI ref |"
SEC3_HEAD = "| Cmd | Command | Defined in |"


class InputError(Exception):
    pass


# ---------------------------------------------------------------- source side


def read_netfns(text):
    """NETFN_NAME -> number, from `pub const NETFN_X: u8 = 0xNN;` lines of ipmb.rs."""
    found = dict(
        (m.group(1), int(m.group(2), 16))
        for m in re.finditer(r"const\s+(NETFN_[A-Z0-9_]+)\s*:\s*u8\s*=\s*0x([0-9A-Fa-f]+)\s*;", text)
    )
    if not found:
        raise InputError(f"{IPMB_RS}: no NETFN_* consts found")
    return found


def read_commands(text, netfns):
    """List of (netfn, cmd, name, handler_path, spec) from the COMMANDS table of commands.rs."""
    modules = {}
    for m in re.finditer(r"^use\s+crate::([\w:]+);", text, re.M):
        path = m.group(1)
        modules[path.split("::")[-1]] = path
    start = text.find("pub const COMMANDS")
    if start < 0:
        raise InputError(f"{COMMANDS_RS}: COMMANDS not found")
    end = text.find("];", start)
    rows = []
    row_re = re.compile(
        r'^cmd\(\s*(NETFN_[A-Z0-9_]+)\s*,\s*0x([0-9A-Fa-f]+)\s*,\s*"([^"]*)"\s*,\s*"([^"]*)"\s*,\s*([\w:]+)\s*,?\s*\)\s*,?\s*(//.*)?$'
    )
    for raw in text[start:end].splitlines():
        line = raw.strip()
        if not line.startswith("cmd("):
            continue  # a comment (including a commented-out row) or the declaration line
        m = row_re.match(line)
        if not m:
            raise InputError(f"{COMMANDS_RS}: cannot read COMMANDS row: {line}")
        netfn_name, cmd, name = m.group(1), int(m.group(2), 16), m.group(3)
        spec, handler = m.group(4), m.group(5)
        if ";" in name or ";" in spec or "|" in name or "|" in spec:
            raise InputError(f"{COMMANDS_RS}: name and spec may not hold ';' or '|': {line}")
        if netfn_name not in netfns:
            raise InputError(f"{COMMANDS_RS}: {netfn_name} is not a NETFN_* const of ipmb.rs")
        head, _, rest = handler.partition("::")
        if rest and head in modules:
            handler = modules[head] + "::" + rest
        rows.append((netfns[netfn_name], cmd, name, handler, spec))
    keys = [(r[0], r[1]) for r in rows]
    if len(set(keys)) != len(keys):
        raise InputError(f"{COMMANDS_RS}: duplicate (netfn, cmd) rows")
    return rows


def broadcast_implemented():
    return LINK_RS.is_file() and BROADCAST_FLAG in LINK_RS.read_text(encoding="utf-8")


# ---------------------------------------------------------------- document side


def split_cells(line):
    """Cells of a table row, stripped; None when the line is not a row."""
    s = line.strip()
    if not (s.startswith("|") and s.endswith("|")):
        return None
    return [c.strip() for c in s[1:-1].split("|")]


def replace_last_cell(line, value):
    """The row with its last cell replaced; everything before it is untouched."""
    eol = line[len(line.rstrip("\r\n")):]
    body = line.rstrip("\r\n").rstrip()
    assert body.endswith("|")
    body = body[:-1]
    cut = body.rfind("|")
    return body[: cut + 1] + " " + value + " |" + eol


def tier1_kind(cell):
    if re.fullmatch(r"M(\[[^\]]*\])?", cell):
        return "M"
    if re.fullmatch(r"NA(\[[^\]]*\])?", cell):
        return "NA"
    return "O"


def parse_row_key(cells, section):
    """(netfn, cmd, is_broadcast) for a data row of section 2 or 3; None for other rows."""
    if not cells or not cells[0]:
        return None
    if section == 2:
        m = re.fullmatch(r"([0-9A-F]{2})h/([0-9A-F]{2})h", cells[0])
        if not m:
            return None
        return int(m.group(1), 16), int(m.group(2), 16), "Broadcast" in cells[1]
    m = re.fullmatch(r"([0-9A-F]{2})h", cells[0])
    if not m:
        return None
    return NETFN_GROUP_EXT, int(m.group(1), 16), False


def find_table(lines, head):
    """(first, last) line indexes of the table whose header line starts with `head`."""
    first = next((i for i, l in enumerate(lines) if l.startswith(head)), None)
    if first is None:
        raise InputError(f"{DOC}: table with header '{head}' not found")
    last = first
    while last + 1 < len(lines) and lines[last + 1].lstrip().startswith("|"):
        last += 1
    return first, last


def ensure_markers(lines, head, eol):
    """Insert the begin/end markers around the table at `head` when they are absent."""
    first, last = find_table(lines, head)
    if first > 0 and lines[first - 1].strip() == BEGIN:
        return lines
    return (
        lines[:first]
        + [BEGIN + eol]
        + lines[first : last + 1]
        + [eol, END + eol]
        + lines[last + 1 :]
    )


def region(lines, head):
    """(table_first, table_last) of the table at `head`, which must sit inside markers."""
    first, last = find_table(lines, head)
    after = last + 1
    while after < len(lines) and not lines[after].strip():
        after += 1
    if not (first > 0 and lines[first - 1].strip() == BEGIN and after < len(lines)
            and lines[after].strip() == END):
        raise InputError(f"{DOC}: generated markers around '{head}' are missing or misplaced")
    return first, last


def collect_table_rows(lines, first, last, section):
    """Data rows of a section 2/3 table: list of (index, cells, key)."""
    header = split_cells(lines[first])
    if "IPMC T1" not in header:
        raise InputError(f"{DOC}: no 'IPMC T1' column in the table at line {first + 1}")
    t1 = header.index("IPMC T1")
    rows = []
    for i in range(first + 2, last + 1):
        cells = split_cells(lines[i])
        if cells is None or len(cells) != len(header):
            continue
        key = parse_row_key(cells, section)
        if key is None or not cells[t1]:
            continue
        rows.append((i, cells, key, tier1_kind(cells[t1]), cells[t1]))
    return rows


def last_column_value(key, kind, implemented, bcast_ok):
    netfn, cmd, is_bcast = key
    if is_bcast:
        return (f"implemented ({BROADCAST_PATH})" if bcast_ok
                else "planned (Phase 3 link layer)")
    if (netfn, cmd) in implemented:
        handler, name, spec = implemented[(netfn, cmd)]
        return f"implemented ({handler}; {name}; {spec})"
    if kind == "M":
        return "planned (Tier-1 mandatory)"
    if kind == "NA":
        return "C1h (NA for Tier 1)"
    return "C1h (optional, not implemented)"


def defined_in_ipmi(ref):
    m = re.fullmatch(r"(\d\.\d):([\d.]+)", ref)
    return f"IPMI {m.group(1)} &sect;{m.group(2)}" if m else ref


def sync_text(doc_text, commands):
    eol = "\r\n" if "\r\n" in doc_text else "\n"
    lines = doc_text.splitlines(keepends=True)
    for head in (SEC1_HEAD, SEC2_HEAD, SEC3_HEAD):
        lines = ensure_markers(lines, head, eol)
    implemented = {(c[0], c[1]): (c[3], c[2], c[4]) for c in commands}
    bcast_ok = broadcast_implemented()

    # Sections 2 and 3: the last column.
    all_rows = []   # every keyed row of sections 2 and 3, for the build list
    for section, head in ((2, SEC2_HEAD), (3, SEC3_HEAD)):
        first, last = region(lines, head)
        rows = collect_table_rows(lines, first, last, section)
        for i, cells, key, kind, t1cell in rows:
            lines[i] = replace_last_cell(lines[i], last_column_value(key, kind, implemented, bcast_ok))
            all_rows.append((section, cells, key, kind, t1cell))
    known = {(r[2][0], r[2][1]) for r in all_rows}
    for netfn, cmd, name, _, _ in commands:
        if (netfn, cmd) not in known:
            raise InputError(
                f"COMMANDS row {name} ({netfn:02X}h/{cmd:02X}h) has no row in the document tables"
            )

    # Section 1: the build list.
    first, last = region(lines, SEC1_HEAD)
    old = []
    for i in range(first + 2, last + 1):
        cells = split_cells(lines[i])
        if cells and len(cells) >= 6 and re.fullmatch(r"[0-9A-F]{2}h", cells[0]) \
                and re.fullmatch(r"[0-9A-F]{2}h", cells[1]):
            key = (int(cells[0][:2], 16), int(cells[1][:2], 16), "Broadcast" in cells[2])
            old.append((key[0], key[1], key[2], cells[:6]))
    old_by_key = {(o[0], o[1], o[2]): o[3] for o in old}

    def derived(section, cells, key, t1cell):
        netfn, cmd, _ = key
        if section == 2:
            defined, page = defined_in_ipmi(cells[2]), "see section 2"
        else:
            defined, page = "VITA " + cells[2], cells[3]
        return [f"{netfn:02X}h", f"{cmd:02X}h", cells[1], t1cell, defined, page]

    wanted = []  # (key, six cells, implemented?)
    mandatory_keys = set()
    for section, cells, key, kind, t1cell in all_rows:
        if kind == "M":
            mandatory_keys.add(key)
            wanted.append((key, old_by_key.get(key) or derived(section, cells, key, t1cell)))
    for section, cells, key, kind, t1cell in all_rows:
        if kind != "M" and (key[0], key[1]) in implemented and not key[2]:
            wanted.append((key, old_by_key.get(key) or derived(section, cells, key, t1cell)))
    # Existing order first, new rows after.
    order = {k: n for n, k in enumerate(old_by_key)}
    wanted.sort(key=lambda w: (order.get(w[0], len(order)),))
    body = [BUILD_HEADER, BUILD_SEPARATOR]
    for key, six in wanted:
        if not key[2] and (key[0], key[1]) in implemented:
            six = six[:2] + [implemented[(key[0], key[1])][1]] + six[3:]
        if key[2]:
            status = "implemented" if bcast_ok else "planned"
        else:
            status = "implemented" if (key[0], key[1]) in implemented else "planned"
        body.append("| " + " | ".join(six + [status]) + " |")
    lines[first : last + 1] = [b + eol for b in body]
    return "".join(lines)


def main(argv):
    check = "--check" in argv
    unknown = [a for a in argv if a not in ("--check",)]
    if unknown:
        print(f"usage: sync_ipmi_commands.py [--check] (unknown: {' '.join(unknown)})", file=sys.stderr)
        return 2
    try:
        netfns = read_netfns(IPMB_RS.read_text(encoding="utf-8"))
        commands = read_commands(COMMANDS_RS.read_text(encoding="utf-8"), netfns)
        # newline="" keeps CRLF/LF exactly as stored.
        with open(DOC, encoding="utf-8", newline="") as f:
            old = f.read()
        new = sync_text(old, commands)
    except (InputError, OSError) as e:
        print(f"sync_ipmi_commands.py: {e}", file=sys.stderr)
        return 2
    if new == old:
        print(f"{DOC.name} is up to date ({len(commands)} implemented commands)")
        return 0
    if check:
        diff = [
            l for l in difflib.unified_diff(
                old.splitlines(), new.splitlines(), "ipmi_commands.md (current)",
                "ipmi_commands.md (generated)", lineterm="", n=0)
        ]
        changed = sum(1 for l in diff if l[:1] in "+-" and l[:3] not in ("+++", "---"))
        print(f"{DOC.name} is out of date: {changed} changed lines. "
              "Run: python -I tools/sync_ipmi_commands.py")
        for l in diff[:60]:
            print(l)
        if len(diff) > 60:
            print(f"... {len(diff) - 60} more diff lines")
        return 1
    with open(DOC, "w", encoding="utf-8", newline="") as f:
        f.write(new)
    print(f"{DOC.name} updated ({len(commands)} implemented commands)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
