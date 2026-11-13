"""test_coverage.py - run every test suite and write the test coverage documents.

    python -I tools/test_coverage.py [--skip-ext] [--no-write]

Runs, in this order:

    1. cargo test --workspace, with the Tier-2/3 extension (when ../IPMIRustFRU_Tier23 exists)
    2. cargo test --workspace with IPMC_TIER23=0 (the public Tier-1 build)
    3. the Python suites (tools/test_defs.py, app-port, chassis-manager, fwupdate, capture and
       this tool's own tests)
    4. the emulator replay gate (CLAUDE.md gate 9), with and without the extension

and reads the board results that need the board and are not run here: the Phase 12 bench table
(Documentation/ipmc_boot_plan.md 13.3), Documentation/board_test_report.md (written by
tools/board_test_report.py) and the open verification gaps (PLAN.md section 7).

Writes Documentation/ipmc_test_coverage.md: the Tier-1 build first, then, when the extension
folder is present, the Tier-2/3 extension (a Rust case that exists only in the extension build,
because the Tier-1 run does not list it, the extension replay and the stored Tier-2/3 conformance
reports; owner 10-26-2026: the documents show the Tier-2/3 capabilities and testing). The file is
overwritten on every run. Exit status 1 when any case failed or any suite could not run.

Python 3.11 or later, standard library only; cargo is found through ~/.cargo/bin.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TIER23_ROOT = ROOT.parent / "IPMIRustFRU_Tier23"
PUBLIC_DOC = ROOT / "Documentation" / "ipmc_test_coverage.md"
BOOT_PLAN = ROOT / "Documentation" / "ipmc_boot_plan.md"
BOARD_REPORT = ROOT / "Documentation" / "board_test_report.md"
PLAN = ROOT / ".claude" / "PLAN.md"
REPLAY_VECTOR = ROOT / "local_only" / "vectors" / "sbc_ab.vec"
VENV_PYTHON = ROOT / "tools" / "python-ipmi" / ".venv" / "Scripts" / "python.exe"
CARGO_BIN = Path.home() / ".cargo" / "bin"
CARGO = str(CARGO_BIN / "cargo.exe")
CONFORMANCE_REPORTS = (
    TIER23_ROOT / "tools" / "conformance_t2" / "conformance_t2_emu.md",
    TIER23_ROOT / "tools" / "conformance_t2" / "conformance_t2_board.md",
    TIER23_ROOT / "tools" / "conformance_t3" / "conformance_t3_emu.md",
    TIER23_ROOT / "tools" / "conformance_t3" / "conformance_t3_board.md",
)

CARGO_TIMEOUT_S = 3600
PYTHON_TIMEOUT_S = 900
REPLAY_TIMEOUT_S = 900
REPLAY_ARGS = ["replay", "local_only/vectors/sbc_ab.vec", "--b-fitted", "1", "--b-enabled", "1",
               "--policy", "alternate", "--quiet"]
# CLAUDE.md gate 9: the replay summary of each build (match, same_cc, cc_diff, not_implemented)
REPLAY_EXPECTED = {"tier1": (44, 61, 115, 89), "tier23": (78, 64, 167, 0)}

# (title, directory under ROOT, "venv" or "system", test file pattern)
PYTHON_SUITES = (
    ("tools/test_defs.py", "tools", "system", "test_defs.py"),
    ("tools/app-port", "tools/app-port", "system", "test_app_cli.py"),
    ("tools/chassis-manager", "tools/chassis-manager", "venv", "test_cm_link.py"),
    ("tools/fwupdate", "tools/fwupdate", "system", "test_fwupdate.py"),
    ("tools/capture", "tools/capture", "system", "test_capture2pcap.py"),
    ("tools/test_test_coverage.py", "tools", "system", "test_test_coverage.py"),
)

PASS, FAIL, IGNORED = "PASS", "FAIL", "IGNORED"
RESULT_WORDS = {"ok": PASS, "FAILED": FAIL, "ignored": IGNORED}
TIER23_PREFIX = "tier23::"
REVISION = datetime.date.today().strftime("%m-%d-%Y")


@dataclass
class Case:
    """One test case: where it ran and what it did."""
    package: str
    target: str
    name: str
    result: str


@dataclass
class Suite:
    """One program run: its cases, or the error that kept it from producing them."""
    title: str
    cases: list[Case] = field(default_factory=list)
    error: str = ""
    note: str = ""

    def count(self, result: str) -> int:
        return sum(c.result == result for c in self.cases)


@dataclass
class Replay:
    label: str
    summary: str
    status: str
    detail: str


# ------------------------------------------------------------------ running programs

def child_env(**extra: str) -> dict[str, str]:
    env = dict(os.environ)
    env["PATH"] = f"{CARGO_BIN}{os.pathsep}{env.get('PATH', '')}"
    env["PYTHONUNBUFFERED"] = "1"
    env.pop("IPMC_TIER23", None)
    env.update(extra)
    return env


def run_merged(args: list[str], timeout: int, env: dict[str, str], cwd: Path = ROOT) -> tuple[int, str]:
    """Run a program with stderr merged into stdout, so headers and results keep their order."""
    try:
        done = subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              timeout=timeout, text=True, encoding="utf-8", errors="replace")
    except (subprocess.TimeoutExpired, OSError) as error:
        return -1, f"{type(error).__name__}: {error}"
    return done.returncode, done.stdout


# ------------------------------------------------------------------ parsers

def normal_path(text: str) -> str:
    return text.replace("\\", "/")


def read_targets(metadata_text: str) -> list[tuple[str, str, str]]:
    """(package, target name with '-' as '_', source path) of every target in `cargo metadata`."""
    found = []
    for package in json.loads(metadata_text)["packages"]:
        for target in package["targets"]:
            found.append((package["name"], target["name"].replace("-", "_"), normal_path(target["src_path"])))
    return found


def target_label(path: str, exe: str) -> str:
    """The label of a test binary: 'unit tests' for src files, the file stem for tests/ files."""
    path = normal_path(path)
    if path.startswith("tests/"):
        return Path(path).stem
    return "unit tests" if exe else "doc tests"


def find_package(targets: list[tuple[str, str, str]], name: str, path: str, used: set) -> str:
    """The package whose target is called `name` and whose source file ends with `path`."""
    wanted = normal_path(path)
    options = [t for t in targets if t[1] == name and (not wanted or t[2].endswith("/" + wanted))]
    options = options or [t for t in targets if t[1] == name]
    for option in options:
        if (option, wanted) not in used:
            used.add((option, wanted))
            return option[0]
    return options[0][0] if options else name.replace("_", "-")


def parse_cargo(text: str, targets: list[tuple[str, str, str]]) -> list[Case]:
    """Every `test <name> ... ok|FAILED|ignored` line, with the package and target of its binary."""
    cases: list[Case] = []
    package, target = "", ""
    used: set = set()
    for line in text.splitlines():
        running = re.match(r"\s*Running (?:unittests )?(.+?) \((.+)\)\s*$", line)
        doc = re.match(r"\s*Doc-tests (\S+)", line)
        result = re.match(r"test (.+?) \.\.\. (ok|FAILED|ignored)(?:, .*)?$", line)
        if running:
            path, exe = running.group(1), normal_path(running.group(2))
            name = re.sub(r"-[0-9a-f]{16}(?:\.exe)?$", "", exe.rsplit("/", 1)[-1])
            package = find_package(targets, name, path, used)
            target = target_label(path, exe if "unittests" in line else "")
        elif doc:
            package = find_package(targets, doc.group(1), "src/lib.rs", set())
            target = "doc tests"
        elif result:
            cases.append(Case(package, target, result.group(1), RESULT_WORDS[result.group(2)]))
    return cases


UNITTEST_STATUS = re.compile(r"^(ok|FAIL|ERROR|skipped .*|expected failure|unexpected success)$")
UNITTEST_START = re.compile(r"^(\w+) \(([\w.]+)\) \.\.\. ?(.*)$")
UNITTEST_RESULTS = {"ok": PASS, "FAIL": FAIL, "ERROR": FAIL, "expected failure": PASS,
                    "unexpected success": FAIL}


def parse_unittest(text: str, title: str) -> Suite:
    """Cases of a `python -m unittest -v` run. A test that prints leaves its result on a later
    line (alone, or glued to the end of the text it printed without a newline), so a case closes
    at its own `ok`, or at the next start or the end of the results by the last word of the
    last line; `Ran N tests` is the cross-check."""
    suite = Suite(title)
    pending, last = "", ""

    def close(word: str) -> None:
        nonlocal pending
        if pending and word:
            result = IGNORED if word.startswith("skipped") else UNITTEST_RESULTS[word]
            module, _, test = pending.rpartition(".")
            suite.cases.append(Case(title, module, test, result))
        pending = ""

    def glued(line: str) -> str:
        return next((w for w in ("ok", "FAIL", "ERROR") if line.endswith(w)), "")

    for raw in text.splitlines():
        line = raw
        started = UNITTEST_START.match(line)
        if started or line.startswith("-----"):
            close(glued(last))
        if started:
            pending, tail = started.group(2), started.group(3).strip()
            line = tail if UNITTEST_STATUS.match(tail) else ""
        if pending and UNITTEST_STATUS.match(line.strip()):
            close(line.strip())
        last = raw.strip() or last
    ran = re.search(r"^Ran (\d+) tests? in", text, re.M)
    if not ran:
        suite.error = "no 'Ran N tests' line"
    elif int(ran.group(1)) != len(suite.cases):
        suite.error = f"parsed {len(suite.cases)} cases, unittest ran {ran.group(1)}"
    return suite


REPLAY_LINE = re.compile(r"^(PASS|FAIL) requests=(\d+) answered=(\d+) match=(\d+) same_cc=(\d+) "
                         r"cc_diff=(\d+) not_implemented=(\d+)")


def parse_replay(text: str, expected: tuple[int, int, int, int], label: str) -> Replay:
    """The replay summary line against the expected (match, same_cc, cc_diff, not_implemented)."""
    for line in reversed(text.splitlines()):
        found = REPLAY_LINE.match(line)
        if found:
            got = tuple(int(found.group(i)) for i in range(4, 8))
            ok = found.group(1) == "PASS" and got == expected
            detail = (f"match {got[0]} same_cc {got[1]} cc_diff {got[2]} not_implemented {got[3]}; "
                      f"expected {expected[0]}/{expected[1]}/{expected[2]}/{expected[3]}")
            return Replay(label, line.strip(), PASS if ok else FAIL, detail)
    return Replay(label, "", FAIL, "no summary line in the output")


def table_cells(line: str) -> list[str]:
    """Cells of a Markdown table row (split on ' | ', so a pipe inside a cell does not count)."""
    return [c.strip() for c in line.strip().strip("|").split(" | ")]


def section_lines(text: str, start: str, level: str) -> list[str]:
    """The lines after the heading that starts with `start`, up to the next heading of `level`."""
    lines, inside = [], False
    for line in text.splitlines():
        if inside and line.startswith(level + " "):
            break
        if line.startswith(start):
            inside = True
        elif inside:
            lines.append(line)
    return lines


def parse_boot_plan(text: str) -> list[tuple[str, str, str]]:
    """(id, test, status) of each row of the 13.3 bench table; status from the Result cell."""
    rows = []
    for line in section_lines(text, "### 13.3 ", "##"):
        cells = table_cells(line) if line.startswith("| B") else []
        if len(cells) >= 4 and re.fullmatch(r"B\d+", cells[0]):
            word = cells[-1].lower()
            status = PASS if word.startswith("pass") else FAIL if word.startswith("fail") else "DEFERRED"
            rows.append((cells[0], cells[1], status))
    return rows


def parse_board_report(text: str) -> tuple[str, list[tuple[str, int, int, int]]]:
    """(generation time, [(section, PASS, FAIL, NOT-TESTABLE)]) of the board test report."""
    when = re.search(r" on (\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)\.", text)
    sections: list[tuple[str, int, int, int]] = []
    title = ""
    counts = [0, 0, 0]
    for line in text.splitlines() + ["## END"]:
        if line.startswith("## "):
            if title and not title.startswith("G."):
                sections.append((title, *counts))
            title, counts = line[3:].strip(), [0, 0, 0]
        elif line.startswith("| "):
            last = table_cells(line)[-1]
            for index, word in enumerate((PASS, FAIL, "NOT-TESTABLE")):
                counts[index] += last == word
    return (when.group(1) if when else "unknown"), sections


def parse_gaps(text: str) -> list[tuple[str, str, str]]:
    """(number, check, why open) of the rows of PLAN.md section 7."""
    rows = []
    for line in section_lines(text, "## 7. ", "##"):
        cells = table_cells(line) if line.startswith("| ") else []
        if len(cells) >= 3 and cells[0].isdigit():
            rows.append((cells[0], cells[1], cells[2]))
    return rows


def parse_conformance_report(text: str) -> tuple[int, int, int]:
    """(PASS, FAIL, NOT-TESTABLE) from the 'PASS: n; FAIL: n; NOT-TESTABLE: n' line, else counted
    from the Result column."""
    found = re.search(r"PASS: (\d+); FAIL: (\d+); NOT-TESTABLE: (\d+)", text)
    if found:
        return int(found.group(1)), int(found.group(2)), int(found.group(3))
    counts = [0, 0, 0]
    for line in text.splitlines():
        if line.startswith("| ") and len(table_cells(line)) >= 4:
            for index, word in enumerate((PASS, FAIL, "NOT-TESTABLE")):
                counts[index] += table_cells(line)[3] == word
    return counts[0], counts[1], counts[2]


# ------------------------------------------------------------------ areas and splitting

def area_of(case: Case) -> str:
    """A short description from the module path: 'ipmc-core link' for link::tests::name."""
    package, parts = case.package, case.name.split("::")[:-1]
    if parts[:1] == [TIER23_PREFIX.rstrip(":")]:
        parts = parts[1:]
        package = "ipmc-tier23"
        if parts[:1] in (["tier2"], ["tier3"]):
            package, parts = "ipmc-" + parts[0], parts[1:]
    modules = [p for p in parts if p != "tests" and not p.endswith("_tests")]
    if case.target not in ("unit tests", "doc tests"):
        modules = [case.target] + modules
    if case.target == "doc tests":
        modules = ["doc test"]
    if modules:
        return " ".join([package] + modules)
    return f"{package} table and glue" if package == "ipmc-tier23" else f"{package} crate root"


def split_cases(extension: list[Case], tier1: list[Case]) -> tuple[list[Case], list[Case]]:
    """(cases only the extension build has, the extension build's cases that the Tier-1 build
    also has)."""
    keys = {(c.package, c.target, c.name) for c in tier1}
    only = [c for c in extension if (c.package, c.target, c.name) not in keys]
    shared = [c for c in extension if (c.package, c.target, c.name) in keys]
    return only, shared


# ------------------------------------------------------------------ Markdown writers

BANNER = ('<table style="width:100%; border:none; margin:0; padding:0;">\n<tr>\n'
          '<td style="text-align:left; border:none; padding:0; width:50%;"><strong>CONFIDENTIAL DOCUMENT &#8211; DO NOT DISCLOSE</strong></td>\n'
          '<td style="text-align:right; border:none; padding:0; width:50%;"><strong>Revision {rev}</strong></td>\n'
          '</tr>\n</table>\n<hr style="border:none; border-top:2px solid #000; margin:4px 0 12px 0;">\n')
BREAK = '<div style="page-break-after: always;"></div>\n'


def md_cell(text: object) -> str:
    return " ".join(str(text).split()).replace("|", "\\|")


def md_table(headers: tuple[str, ...], rows: list[tuple]) -> list[str]:
    lines = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    lines += ["| " + " | ".join(md_cell(v) for v in row) + " |" for row in rows]
    return lines + [""]


def title_page(title: str, subtitle: str) -> list[str]:
    return [BANNER.format(rev=REVISION),
            '<div style="height: 8.2in; display: flex; flex-direction: column; justify-content: center; text-align: center;">',
            f'<h1 style="border: none;">{title}</h1>',
            f'<p style="font-size: 16px;">{subtitle}</p>',
            f'<p style="font-size: 16px;"><strong>Revision {REVISION}</strong></p>',
            "</div>", ""]


def section(title: str) -> list[str]:
    """A page break, the banner and a `##` heading."""
    return [BREAK, BANNER.format(rev=REVISION), f"## {title}", ""]


def counts_row(label: str, cases: list[Case]) -> tuple:
    return (label, len(cases), *(sum(c.result == r for c in cases) for r in (PASS, FAIL, IGNORED)))


def by_key(cases: list[Case], key) -> dict:
    grouped: dict = {}
    for case in cases:
        grouped.setdefault(key(case), []).append(case)
    return grouped


def rust_sections(cases: list[Case], first: int, title: str = "Rust tests") -> list[str]:
    """One `##` section per package, one `###` table per target."""
    lines: list[str] = []
    for number, (package, in_package) in enumerate(by_key(cases, lambda c: c.package).items(), first):
        lines += section(f"{number}. {title}: {package}")
        for target, in_target in by_key(in_package, lambda c: c.target).items():
            lines += [f"### {target} ({len(in_target)} cases)", ""]
            lines += md_table(("Test", "Area", "Result"),
                              [(f"`{c.name}`", area_of(c), c.result) for c in in_target])
    return lines


def python_section(number: int, suites: list[Suite]) -> list[str]:
    lines = section(f"{number}. Python suites")
    for suite in suites:
        lines += [f"### {suite.title} ({len(suite.cases)} cases)", ""]
        if suite.error:
            lines += [f"Suite error: {suite.error}", ""]
        lines += md_table(("Test", "Area", "Result"),
                          [(f"`{c.name}`", f"{suite.title} {c.target.rsplit('.', 1)[-1]}", c.result)
                           for c in suite.cases])
    return lines


def replay_section(number: int, replays: list[Replay]) -> list[str]:
    lines = section(f"{number}. Emulator replay")
    lines += ["The replay of the captured Chassis Manager traffic (`local_only/vectors/sbc_ab.vec`, local only) "
              "through the emulator, compared with the numbers of CLAUDE.md gate 9.", ""]
    rows = [(r.label, f"`{r.summary}`" if r.summary else "none", r.detail, r.status) for r in replays]
    return lines + md_table(("Build", "Summary line", "Against the expected numbers", "Result"), rows)


def board_section(number: int, bench: list[tuple[str, str, str]], board: tuple) -> list[str]:
    lines = section(f"{number}. Board tests")
    lines += ["### Phase 12 bench table (`Documentation/ipmc_boot_plan.md` 13.3)", ""]
    lines += md_table(("Test", "What", "Result"), [(i, t, r) for i, t, r in bench])
    when, sections = board
    lines += [f"### Board test report (`Documentation/board_test_report.md`, written {when})", "",
              "The report is generated by `tools/board_test_report.py` against the board; this tool reads "
              "it and does not run it.", ""]
    rows = [(title, p, f, n) for title, p, f, n in sections]
    rows.append(("Total", sum(r[1] for r in rows), sum(r[2] for r in rows), sum(r[3] for r in rows)))
    return lines + md_table(("Section", "PASS", "FAIL", "NOT-TESTABLE"), rows)


def gaps_section(number: int, gaps: list[tuple[str, str, str]]) -> list[str]:
    lines = section(f"{number}. Not covered on hardware")
    lines += ["The open verification gaps of PLAN.md section 7 (items 1 to 14, item 14 being the "
              "deferred bootloader bench tests). Each needs an owner action or extra equipment.", ""]
    return lines + md_table(("#", "Check", "Why it is open"), gaps)


def summary_rows(cases: list[Case], python: list[Suite], replays: list[Replay], bench: list, board: tuple) -> list[tuple]:
    rows = [counts_row(package, in_package) for package, in_package in by_key(cases, lambda c: c.package).items()]
    rows += [counts_row(f"Python: {s.title}", s.cases) for s in python]
    rows.append(("Emulator replay", len(replays), sum(r.status == PASS for r in replays),
                 sum(r.status == FAIL for r in replays), 0))
    rows.append(("Board: Phase 12 bench table", len(bench), sum(r[2] == PASS for r in bench),
                 sum(r[2] == FAIL for r in bench), sum(r[2] == "DEFERRED" for r in bench)))
    sections = board[1]
    rows.append(("Board: board test report rows", sum(p + f + n for _, p, f, n in sections),
                 sum(p for _, p, _, _ in sections), sum(f for _, _, f, _ in sections),
                 sum(n for _, _, _, n in sections)))
    return rows


def totals_row(rows: list[tuple]) -> tuple:
    return ("Total", *(sum(r[i] for r in rows) for i in range(1, 5)))


def overview(started: str, kind: str, extra: list[str]) -> list[str]:
    lines = section("1. Summary")
    lines += [f"Generated by `tools/test_coverage.py` on {started}; do not edit. {kind}", "",
              "The Ignored column counts ignored Rust tests, skipped Python tests, bench rows deferred by the "
              "owner and board report rows marked NOT-TESTABLE.", ""]
    return lines + extra


def public_document(started, tier1, python, replays, bench, board, gaps, ext) -> str:
    """The whole document. `ext` is None without the extension, else (only, shared, replays,
    reports): the extension-only cases, the public cases of the extension run, its replay and the
    stored conformance reports."""
    rows = summary_rows(tier1, python, replays, bench, board)
    if ext:
        only, shared, ext_replays, _ = ext
        rows += [counts_row(f"Tier-2/3: {package}", in_package)
                 for package, in_package in by_key(only, lambda c: c.package).items()]
        rows.append(("Tier-2/3: emulator replay", len(ext_replays), sum(r.status == PASS for r in ext_replays),
                     sum(r.status == FAIL for r in ext_replays), 0))
    lines = title_page("IPMC Test Coverage",
                       "IPMIRustFRU: every test case of the Tier-1 build and the Tier-2/3 extension, and its result")
    kind = ("This document covers the Tier-1 build and the Tier-2/3 extension." if ext
            else "This document covers the Tier-1 build; the Tier-2/3 extension was not built in this run.")
    lines += overview(started, kind,
                      md_table(("Suite", "Cases", "Passed", "Failed", "Ignored"), rows + [totals_row(rows)]))
    lines += rust_sections(tier1, 2)
    first = 2 + len(by_key(tier1, lambda c: c.package))
    lines += python_section(first, python) + replay_section(first + 1, replays)
    lines += board_section(first + 2, bench, board) + gaps_section(first + 3, gaps)
    if ext:
        lines += extension_sections(first + 4, *ext)
    return "\n".join(lines) + "\n"


def extension_sections(first, only, shared, ext_replays, reports) -> list[str]:
    """The Tier-2/3 part: the extension-only cases by crate and target, the public cases of the
    same run, the extension replay and the stored conformance reports."""
    lines = section(f"{first}. Tier-2/3 extension")
    lines += [f"The build with the extension (`IPMC_TIER23=1`) runs {len(only) + len(shared)} cases: the "
              f"{len(shared)} public cases above and {len(only)} that exist only with the extension, listed "
              "here by crate and target.", ""]
    failed = [c for c in shared if c.result == FAIL]
    lines += [f"Public cases in the extension run: {len(shared)}, {len(failed)} failed.", ""]
    if failed:
        lines += md_table(("Test", "Area", "Result"), [(f"`{c.name}`", area_of(c), c.result) for c in failed])
    lines += rust_sections(only, first + 1, "Tier-2/3 Rust tests")
    number = first + 1 + len(by_key(only, lambda c: c.package))
    lines += replay_section(number, ext_replays)
    lines += section(f"{number + 1}. Tier-2/3 conformance reports")
    lines += ["The Tier-2 and Tier-3 conformance suites (`tools/conformance_t2`, `tools/conformance_t3` of "
              "`../IPMIRustFRU_Tier23`) need the emulator server or the board; this tool reads the stored "
              "reports and does not run them.", ""]
    return lines + md_table(("Report", "Written", "PASS", "FAIL", "NOT-TESTABLE"), reports)


# ------------------------------------------------------------------ the runs

def cargo_suite(title: str, env: dict[str, str], targets: list) -> Suite:
    code, text = run_merged([CARGO, "test", "--workspace", "--no-fail-fast", "--color", "never",
                             "--", "--test-threads=1"], CARGO_TIMEOUT_S, env)
    suite = Suite(title, parse_cargo(text, targets))
    if not suite.cases:
        suite.error = f"no test lines (cargo exit {code}): {text[-300:]}"
    elif code != 0 and not suite.count(FAIL):
        suite.error = f"cargo exit {code} with no failed case: {text[-300:]}"
    return suite


def python_suite(title: str, directory: str, which: str, pattern: str) -> Suite:
    interpreter = VENV_PYTHON if which == "venv" else Path(sys.executable)
    code, text = run_merged([str(interpreter), "-I", "-m", "unittest", "discover", "-s", directory,
                             "-p", pattern, "-v"], PYTHON_TIMEOUT_S, child_env())
    suite = parse_unittest(text, title)
    if code != 0 and not suite.count(FAIL) and not suite.error:
        suite.error = f"exit {code} with no failed case"
    return suite


def replay(label: str, key: str, env: dict[str, str]) -> Replay:
    if not REPLAY_VECTOR.exists():
        return Replay(label, "", IGNORED, "skipped: local_only/vectors/sbc_ab.vec is absent (local only)")
    code, text = run_merged([CARGO, "run", "-q", "-p", "ipmc-emu", "--", *REPLAY_ARGS], REPLAY_TIMEOUT_S, env)
    return parse_replay(text, REPLAY_EXPECTED[key], label)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8") if path.exists() else ""


def report_rows() -> list[tuple]:
    rows = []
    for path in CONFORMANCE_REPORTS:
        if path.exists():
            written = datetime.datetime.fromtimestamp(path.stat().st_mtime).strftime("%Y-%m-%d %H:%M")
            rows.append((path.name, written, *parse_conformance_report(read_text(path))))
    return rows


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--skip-ext", action="store_true", help="no extension run (Tier-1 only)")
    parser.add_argument("--no-write", action="store_true", help="run and print the summary only")
    args = parser.parse_args(argv)
    started = datetime.datetime.now().strftime("%m-%d-%Y %H:%M:%S")
    with_ext = TIER23_ROOT.exists() and not args.skip_ext
    code, meta = run_merged([CARGO, "metadata", "--no-deps", "--format-version", "1"], 300, child_env())
    targets = read_targets(meta[meta.index("{"):]) if code == 0 and "{" in meta else []
    suites = []
    extension = Suite("extension build")
    if with_ext:
        extension = cargo_suite("cargo test, extension build", child_env(IPMC_TIER23="1"), targets)
        suites.append(extension)
    tier1 = cargo_suite("cargo test, Tier-1 build", child_env(IPMC_TIER23="0"), targets)
    suites.append(tier1)
    python = [python_suite(*spec) for spec in PYTHON_SUITES]
    replays = [replay("Tier-1 (IPMC_TIER23=0)", "tier1", child_env(IPMC_TIER23="0"))]
    ext_replays = [replay("Tier-2/3 extension", "tier23", child_env(IPMC_TIER23="1"))] if with_ext else []
    only, shared = split_cases(extension.cases, tier1.cases) if with_ext else ([], [])
    bench = parse_boot_plan(read_text(BOOT_PLAN))
    board = parse_board_report(read_text(BOARD_REPORT))
    gaps = parse_gaps(read_text(PLAN))
    ext = (only, shared, ext_replays, report_rows()) if with_ext else None
    public = public_document(started, tier1.cases, python, replays, bench, board, gaps, ext)
    errors = [f"{s.title}: {s.error}" for s in suites + python if s.error]
    failed = sum(s.count(FAIL) for s in suites + python) + sum(r.status == FAIL for r in replays + ext_replays)
    failed += sum(r[2] == FAIL for r in bench)
    if not args.no_write:
        PUBLIC_DOC.write_text(public, encoding="utf-8")
    for suite in suites + python:
        print(f"{suite.title}: {len(suite.cases)} cases, {suite.count(PASS)} passed, "
              f"{suite.count(FAIL)} failed, {suite.count(IGNORED)} ignored")
    if with_ext:
        print(f"extension-only cases: {len(only)}; public cases in the extension run: {len(shared)}")
    for item in replays + ext_replays:
        print(f"replay {item.label}: {item.status} {item.detail}")
    print(f"bench rows: {len(bench)}; board report written {board[0]}; open gaps: {len(gaps)}")
    for error in errors:
        print("ERROR " + error)
    print(f"FAILED cases: {failed}; suite errors: {len(errors)}")
    return 1 if failed or errors else 0


if __name__ == "__main__":
    sys.exit(main())
