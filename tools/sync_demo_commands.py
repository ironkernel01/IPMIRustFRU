#!/usr/bin/env python3
"""Generate the demo command appendix from local_only/vita_commmands.md.

The local catalog is editable and is never overwritten. Run with --check to
detect drift without writing. Regenerate the PDF after updating the appendix.
"""

import argparse
import difflib
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from sync_ipmi_commands import split_cells  # noqa: E402

SOURCE = ROOT / "local_only" / "vita_commmands.md"
BRIEF = ROOT / "Documentation" / "ipmc_product_brief.md"
BEGIN = "<!-- generated: sync_demo_commands.py -->"
END = "<!-- end generated: sync_demo_commands.py -->"
HEADER = ["NetFn/Cmd", "Command", "Reference", "Req T1", "Req T2", "Req T3",
          "Impl T1", "Impl T2", "Impl T3"]
SUPPORT = {"Yes", "No", "Transmit only"}


def appendix(source):
    """Validate and render the catalog and notes, excluding local provenance."""
    output, rows = [], []
    active = False
    seen = set()
    for line in source.splitlines():
        if line.startswith("## IPMI commands:"):
            active = True
        if not active:
            continue
        cells = split_cells(line)
        if cells and cells != HEADER and not all(re.fullmatch(r":?-+:?", c) for c in cells):
            if len(cells) != len(HEADER):
                raise ValueError(f"expected {len(HEADER)} columns: {line}")
            if not re.fullmatch(r"[0-9A-F]{2}h/[0-9A-F]{2}h(?:-[0-9A-F]{2}h)?", cells[0]):
                raise ValueError(f"invalid command code: {cells[0]}")
            if any(value not in SUPPORT for value in cells[6:]):
                raise ValueError(f"invalid implementation status: {line}")
            if any(not re.fullmatch(r"(?:M|O|O/M|NA|N/A)(?:\[[\d,]+\])?|â€”|-", value)
                   for value in cells[3:6]):
                raise ValueError(f"invalid requirement status: {line}")
            key = (cells[0], cells[1])
            if key in seen:
                raise ValueError(f"duplicate catalog row: {key}")
            seen.add(key)
            rows.append(cells)
        output.append("#" + line if line.startswith("##") else line)
    if not rows or "## VSO commands:" not in source:
        raise ValueError("both IPMI and VSO command catalogs are required")
    counts = [sum(row[index] == "Yes" for row in rows) for index in (6, 7, 8)]
    missing = [f"Tier-{tier}: {row[0]} {row[1]}"
               for row in rows for tier in (1, 2, 3)
               if re.fullmatch(r"M(?:\[[\d,]+\])?", row[2 + tier])
               and row[5 + tier] == "No"]
    coverage = ("**Mandatory-command gaps:** " + "; ".join(missing) + "." if missing else
                "**Mandatory-command coverage:** no entry marked M or M[n] is marked No. "
                "Conditionally mandatory O/M entries must be assessed against the scope below; "
                "this table is an implementation statement, not a conformance certificate.")
    intro = [
        '<div style="page-break-after: always;"></div>', "",
        "## Complete VITA 46.11-2022 command support", "",
        "The catalog follows the IPMI and VSO groups of VITA 46.11-2022 Tables",
        "10.1.1-1 and 10.1.2-1. Unsupported, optional, reserved and Chassis-Manager-only",
        "entries remain visible. This product is an IPMC, not a Chassis Manager.", "",
        "**Req T1/T2/T3:** M = mandatory; O = optional; NA = not applicable;",
        "O/M = conditional mandatory support. Bracketed numbers refer to the notes",
        "after the tables. Requirements and implementation are separate columns.", "",
        "**Impl T1/T2/T3:** Yes = received command implemented; No = not implemented;",
        "Transmit only = outbound operation, with incoming requests unsupported.",
        "Unimplemented addressed commands normally return C1h. Reserved entries are",
        "not commands. Conditional requirements depend on the interfaces and FRUs fitted.", "",
        f"Implemented received command forms: **{counts[0]} at Tier-1, {counts[1]} at Tier-2,",
        f"{counts[2]} at Tier-3**, counting Broadcast Get Device ID separately from unicast.",
        "Outbound Platform Event generation is additional at Tier-2/3. The paid",
        "Tier-2/3 package includes both configurations; Tier-2 is not sold separately.", "",
        coverage, "",
        "IPMI references retain the source table's version and section notation.",
        "VSO references identify VITA 46.11-2022 sections; VSO requests use NetFn 2Ch",
        "and group identifier 03h. See the public",
        "[command document](ipmi_commands.md) for further context.", "",
    ]
    return "\n".join(intro + output).rstrip() + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        generated = appendix(SOURCE.read_text(encoding="utf-8"))
        old = BRIEF.read_text(encoding="utf-8")
        if old.count(BEGIN) != 1 or old.count(END) != 1:
            raise ValueError("brief needs exactly one pair of command-appendix markers")
        before, rest = old.split(BEGIN)
        _, after = rest.split(END)
        new = (before + BEGIN + "\n" + generated + END + after).rstrip() + "\n"
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if args.check:
        if old == new:
            print("demo command appendix is up to date")
            return 0
        print("demo command appendix is stale; run python -I tools/sync_demo_commands.py")
        for line in list(difflib.unified_diff(old.splitlines(), new.splitlines(), n=1))[:30]:
            print(line)
        return 1
    BRIEF.write_text(new, encoding="utf-8", newline="\n")
    print(f"updated {BRIEF.relative_to(ROOT)} from {SOURCE.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
