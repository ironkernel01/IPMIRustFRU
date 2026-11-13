#!/usr/bin/env python3
"""Publish executed PASS/FAIL checks, retaining full raw reports locally."""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from sync_ipmi_commands import split_cells  # noqa: E402


def main():
    archive = ROOT / "local_only" / "Documentation"
    archive.mkdir(parents=True, exist_ok=True)
    for transport in ("emu", "board"):
        destination = ROOT / "tools" / "conformance" / f"conformance_{transport}.md"
        raw_path = archive / f"conformance_{transport}_raw.md"
        current = destination.read_text(encoding="utf-8")
        if not current.startswith("# Executed"):
            raw_path.write_text(current, encoding="utf-8", newline="\n")
        source = raw_path.read_text(encoding="utf-8")
        rows = []
        for line in source.splitlines():
            cells = split_cells(line)
            if cells and len(cells) == 5 and cells[3] in ("PASS", "FAIL"):
                rows.append(line)
        if not rows:
            raise ValueError(f"no executed checks in {raw_path}")
        passed = sum(split_cells(row)[3] == "PASS" for row in rows)
        failed = len(rows) - passed
        setup = ("Strict Tier-1 emulator: both simulated buses, alternate responses, strict IPMB-B power-up."
                 if transport == "emu" else
                 "NUCLEO-F401RE with one Aardvark connected to IPMB-A.")
        output = ["# Executed Tier-1 conformance checks", "", "Validation date: 11-13-2026.",
                  "", setup, "", "This customer report lists executed PASS/FAIL checks.", "",
                  "| ID | Rule/table/section | Page | Result | Detail |",
                  "|---|---|---|---|---|", *rows, "",
                  f"PASS: {passed}; FAIL: {failed}", ""]
        destination.write_text("\n".join(output), encoding="utf-8", newline="\n")
        print(f"{destination.relative_to(ROOT)}: {passed} PASS, {failed} FAIL")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
