"""board_test_report.py - run the Tier-1 IPMC board tests over I2C and write the report.

    python tools/board_test_report.py [--duration-s 10] [--skip-persistence]

Writes Documentation/board_test_report.md (generated, overwritten on every run) and exits 1 when
any row is FAIL. The board sits on IPMB-A behind a Total Phase Aardvark; the firmware console is
COM3 (one program at a time). Sections:

    A environment           D conformance suite          G summary
    B command line          E link and driver health
    C Chassis Manager       F FRU persistence

The command-line rows are read from the table in tools/python-ipmi/README.md, so the report runs
exactly the lines that README documents. The Chassis Manager, its cross-check and the conformance
runner are run as programs, never copied. Python 3.11 or later, standard library only; the python-ipmi
virtual environment supplies ipmitool.exe and the interpreter for the child tools.
"""
from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENV = ROOT / "tools" / "python-ipmi" / ".venv" / "Scripts"
IPMITOOL = VENV / "ipmitool.exe"
VENV_PYTHON = VENV / "python.exe"
README = ROOT / "tools" / "python-ipmi" / "README.md"
sys.path.insert(0, str(ROOT / "tools"))
import defs as defs_reader  # noqa: E402  (tools/defs.py, the one reader of the configuration)
REPORT = ROOT / "Documentation" / "board_test_report.md"
FIRMWARE_DIR = ROOT / "crates" / "ipmc-nucleo-f401re"
FIRMWARE_BIN = FIRMWARE_DIR / "target" / "thumbv7em-none-eabihf" / "release" / "ipmc-nucleo-f401re.bin"
GEN_PS1 = FIRMWARE_DIR / "gen.ps1"
SCRATCH = ROOT / ".scratch"
CM_SCRIPT = ROOT / "tools" / "chassis-manager" / "chassis_manager.py"
CROSSCHECK_SCRIPT = ROOT / "tools" / "chassis-manager" / "crosscheck.py"
CONFORMANCE_SCRIPT = ROOT / "tools" / "conformance" / "run_conformance.py"
CONFORMANCE_REPORT = ROOT / "tools" / "conformance" / "conformance_board.md"

IPMITOOL_PREFIX = ["-I", "aardvark", "-t", "0x82"]  # the prefix documented in the README
IPMITOOL_GAP_S = 3.5          # python-ipmi restarts sequence numbers per process; the IPMC duplicate cache answers a repeat
IPMITOOL_TIMEOUT_S = 40
CONSOLE_TIMEOUT_S = 60
RESET_SETTLE_S = 2.5          # boot plus the first power-on events after "sys reset"
PERSIST_MARGIN_S = 1.5        # wait beyond IPMC_FRU_PERSIST_DELAY_MS before the reset
PHASE_GAP_S = 4.0             # between two programs that each start sequence numbers low
CONFORMANCE_TIMEOUT_S = 1800
CHILD_TIMEOUT_MARGIN_S = 120
FRU_READ_CHUNK = 16           # Read FRU Data bytes per request (the Chassis Manager uses the same size)
OBSERVED_MAX = 220            # characters of observed text kept in a table cell
SELF_TEST_OK = "00 55 00"
LINK_SENSOR_OK = "08 40 02"   # sensor 1, IPMB-A enabled, B not fitted (VITA Table 10.2.2-1)
STATUSES = ("PASS", "FAIL", "NOT-TESTABLE")

NOT_COVERED = (
    "PLAN section 7 #1: scope check of SCL/SDA timing on IPMB-A, both master modes",
    "PLAN section 7 #2 and #4: clock-stretch and stuck-line jumper tests, start-up fault recovery",
    "PLAN section 7 #3: arbitration loss and hand-over with a second master",
    "PLAN section 7 #7: IPMB-B on hardware",
    "PLAN section 7 #8: NVMRO from the strap pin",
    "PLAN section 7 #9: heartbeat LED by eye",
    "PLAN section 7 #10: power and fault input jumper tests",
    "PLAN section 7 #13: hardware watchdog",
)


@dataclass
class Row:
    id: str
    what: str
    how: str
    expected: str
    observed: str
    status: str


class Report:
    """Sections in run order; rendering writes them by letter."""

    def __init__(self):
        self.sections: dict[str, dict] = {}

    def section(self, letter: str, title: str, intro: str = "", headers=None):
        self.sections[letter] = {"title": title, "intro": intro, "rows": [],
                                 "headers": headers or ("ID", "What", "How", "Expected", "Observed", "Result")}
        return self.sections[letter]["rows"]

    def all_rows(self):
        for letter, sec in self.sections.items():
            for row in sec["rows"]:
                yield letter, row


def cell(text, limit=OBSERVED_MAX) -> str:
    text = " ".join(str(text).split())
    if len(text) > limit:
        text = text[: limit - 3] + "..."
    return text.replace("|", "\\|")


def md_table(headers, rows) -> list[str]:
    """The one table writer: header row, separator, one line per row."""
    lines = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for row in rows:
        lines.append("| " + " | ".join(cell(v) for v in (row.id, row.what, row.how, row.expected, row.observed, row.status)) + " |")
    return lines


def verdict(ok: bool) -> str:
    return "PASS" if ok else "FAIL"


def run(args, timeout, cwd=ROOT):
    """Run a program; return (returncode, stdout + stderr text). A timeout or a missing program is rc -1."""
    try:
        done = subprocess.run([str(a) for a in args], cwd=cwd, capture_output=True, text=True,
                              timeout=timeout, encoding="utf-8", errors="replace")
    except (subprocess.TimeoutExpired, OSError) as error:
        return -1, f"{type(error).__name__}: {error}"
    return done.returncode, done.stdout + done.stderr


_last_ipmitool = [0.0]


def ipmitool(arguments: str):
    """Run ipmitool.exe with the documented prefix; waits IPMITOOL_GAP_S after the previous run."""
    wait = _last_ipmitool[0] + IPMITOOL_GAP_S - time.monotonic()
    if wait > 0:
        time.sleep(wait)
    result = run([IPMITOOL, *IPMITOOL_PREFIX, *arguments.split()], IPMITOOL_TIMEOUT_S)
    _last_ipmitool[0] = time.monotonic()
    return result


def console(*commands: str) -> str:
    """Send each command to the firmware console through gen.ps1 and return the whole reply text."""
    rc, text = run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", GEN_PS1, "console", *commands],
                   CONSOLE_TIMEOUT_S)
    return text if rc == 0 else f"console failed (rc {rc}): {text}"


def hex_tokens(text: str) -> list[str]:
    return text.strip().lower().split()


def normalise(text: str) -> str:
    return " ".join(text.split())


def need(text: str, *wanted: str):
    """(ok, note): every wanted string occurs in the whitespace-normalised text."""
    flat = normalise(text)
    missing = [w for w in wanted if normalise(w) not in flat]
    return (not missing), ("all present" if not missing else "missing: " + "; ".join(missing))


def read_defs() -> dict:
    """NAME -> value of every item of crates/ipmc-defs/src/defs.rs (int, bool, str)."""
    return defs_reader.read()


def git_state():
    rc, rev = run(["git", "rev-parse", "--short", "HEAD"], 30)
    rc2, status = run(["git", "status", "--porcelain"], 30)
    dirty = bool(status.strip()) if rc2 == 0 else None
    return (rev.strip() if rc == 0 else "unknown"), dirty


# ---------------------------------------------------------------- the README command table

def backticks(text: str) -> list[str]:
    return re.findall(r"`([^`]*)`", text)


def read_command_table():
    """The README tables as dictionaries: command rows (C..) and error probes (E..)."""
    commands, probes = [], []
    for line in README.read_text(encoding="utf-8").splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split(" | ")] if line.startswith("| ") else []
        if len(cells) == 6 and re.fullmatch(r"C\d\d", cells[0]):
            commands.append({"id": cells[0], "name": cells[1], "named": backticks(cells[2]),
                             "raw": (backticks(cells[3]) or [None])[0], "expected": cells[5]})
        elif len(cells) == 4 and re.fullmatch(r"E\d\d", cells[0]):
            probes.append({"id": cells[0], "name": cells[1], "raw": backticks(cells[2])[0],
                           "expected": backticks(cells[3])[0]})
    return commands, probes


def match_bytes(expected: str, observed: str) -> bool:
    """Equal hex byte lists; the tokens xx and yy match any byte."""
    want, got = hex_tokens(expected), hex_tokens(observed)
    return len(want) == len(got) and all(w in ("xx", "yy") or w == g for w, g in zip(want, got))


def named_checks(defs: dict) -> dict:
    """Named command -> function(text) -> (ok, note); the strings come from defs.rs or the VITA tables."""
    sensors = ("FRU State", "IPMB Link", "FRU Health", "FRU Voltage", "FRU Temperature", "Payload Results",
               "Payload Status")

    def sdr_list(text):
        return need(text, *sensors, "IPMC")

    def sdr_showall(text):
        ok, note = need(text, *(f"Device Id string: {s}" for s in sensors), "Device Id string: IPMC",
                        "SDR type: 0x12")
        full = len(re.findall(r"SDR type:\s+0x02", text))
        return ok and full == len(sensors), f"{note}; {full} sensor records"

    fru_fields = (f"Manufacturer: {defs['IPMC_FRU_BOARD_MFR']}", f"Product Name: {defs['IPMC_FRU_BOARD_PRODUCT']}",
                  f"Serial Number: {defs['IPMC_FRU_BOARD_SERIAL']}", f"Part Number: {defs['IPMC_FRU_BOARD_PART']}",
                  f"FRU File ID: {defs['IPMC_FRU_FILE_ID']}", f"Manufacturer: {defs['IPMC_FRU_PRODUCT_MFR']}",
                  f"Name: {defs['IPMC_FRU_PRODUCT_NAME']}", f"Part/Model Number: {defs['IPMC_FRU_PRODUCT_PART']}",
                  f"Version: {defs['IPMC_FRU_PRODUCT_VERSION']}", f"Serial Number: {defs['IPMC_FRU_PRODUCT_SERIAL']}")
    return {
        "bmc info": lambda t: need(t, "Device ID: 1 (0x01)", "Firmware Revision: 0.1", "IPMI Version: 2.0",
                                   "Product ID: 18763 (0x494b)", "Sensor Device", "FRU Inventory Device"),
        "fru print": lambda t: need(t, "Board Info Area:", "Product Info Area:", *fru_fields),
        "sdr list": sdr_list,
        "sdr showall": sdr_showall,
        "vita properties": lambda t: need(t, "Tier 1", "Layer 1", "100kHz", "1 IPMB interface supported",
                                          "VITA 46.11", "Max FRU Device ID : 0x00"),
        "vita addrinfo": lambda t: need(t, "Hardware Address : 0x41", "IPMB-0 Address : 0x82", "FRU ID : 0x00",
                                        "Site ID : 0x01"),
    }


# ---------------------------------------------------------------- sections

def section_a(report: Report, defs: dict) -> str:
    """Environment. Returns the console text of the reset (the banner)."""
    rows = report.section("A", "Environment")
    now = datetime.datetime.now().astimezone()
    rows.append(Row("A01", "Run date and time", "datetime.now()", "a timestamp", now.strftime("%Y-%m-%d %H:%M:%S %Z"), "PASS"))
    rev, dirty = git_state()
    rows.append(Row("A02", "Source revision", "git rev-parse --short HEAD; git status --porcelain",
                    "a revision and the tree state",
                    f"{rev}, tree {'dirty' if dirty else 'clean' if dirty is not None else 'unknown'}",
                    verdict(rev != "unknown")))
    if FIRMWARE_BIN.exists():
        info = FIRMWARE_BIN.stat()
        built = datetime.datetime.fromtimestamp(info.st_mtime).strftime("%Y-%m-%d %H:%M:%S")
        rows.append(Row("A03", "Firmware image", f"{FIRMWARE_BIN.relative_to(ROOT)}", "a built .bin",
                        f"{info.st_size} bytes, built {built}", "PASS"))
    else:
        rows.append(Row("A03", "Firmware image", f"{FIRMWARE_BIN.relative_to(ROOT)}", "a built .bin", "missing", "FAIL"))
    banner = console("sys reset")
    time.sleep(RESET_SETTLE_S)
    wanted = {"Firmware version": r"ipmc-nucleo-f401re v\S+ \(ipmc-core v\S+\)", "IPMB address": r"IPMB address:\s+0x82",
              "Self test": r"Self test:\s+55h 00h \(passed\)"}
    for n, (label, pattern) in enumerate(wanted.items(), start=4):
        found = re.search(pattern, banner)
        rows.append(Row(f"A{n:02d}", f"Console banner: {label}", "gen.ps1 console 'sys reset', read COM3",
                        pattern, found.group(0) if found else "not found", verdict(bool(found))))
    master = "bit-bang" if defs["IPMB_MASTER_MODE"] == "BitBang" else "hardware I2C"
    rows.append(Row("A07", "Build configuration", "defs.rs IPMB_MASTER_MODE, IPMB_BUS_A_ENABLE, IPMB_BUS_B_ENABLE",
                    "master mode and bus enables",
                    f"{master} master, bus A enable {int(defs['IPMB_BUS_A_ENABLE'])}, bus B enable {int(defs['IPMB_BUS_B_ENABLE'])}",
                    "PASS"))
    return banner


def ipmb_counters(text: str) -> dict[str, dict[str, str]]:
    """'IPMB-A tx:    ok=1 fail=0' lines -> {'A tx': {'ok': '1', 'fail': '0'}, ...}."""
    groups = {}
    for line in text.splitlines():
        found = re.match(r"IPMB-([AB]) (rx err|rx|tx|drv err|drv|fault|master):\s*(.*)", line)
        if found:
            groups[f"{found.group(1)} {found.group(2)}"] = dict(re.findall(r"([\w-]+)=(\S+)", found.group(3)))
    return groups


def health_rows(rows: list[Row], tag: str, stats_text: str, state_text: str, check_traffic: bool):
    """Section E rows for one snapshot (tag 'before' or 'after')."""
    c = ipmb_counters(stats_text)
    rx_ok, tx_ok = c.get("A rx", {}).get("ok"), c.get("A tx", {}).get("ok")
    traffic = int(rx_ok or 0) > 0
    stats_how = "gen.ps1 console 'ipmb stats'"
    bad_tx = [k for k in ("fail", "fail-streak", "dropped") if c.get("A tx", {}).get(k) != "0"]
    rows.append(Row(f"E-{tag}-1", f"IPMB-A requests answered ({tag} the run)", stats_how,
                    "rx ok = tx ok; replays <= rx ok; tx fail, fail-streak and dropped 0" + ("; traffic seen" if check_traffic else ""),
                    f"rx ok={rx_ok} tx ok={tx_ok} replays={c.get('A tx', {}).get('replays')} bad: {bad_tx or 'none'}",
                    verdict(rx_ok == tx_ok and rx_ok is not None and not bad_tx and (traffic or not check_traffic)
                            and int(c.get("A tx", {}).get("replays", "0")) <= int(rx_ok))))
    err_groups = {"A drv err": None, "A master": {"ok"}}
    nonzero = []
    for group, ignore in err_groups.items():
        nonzero += [f"{group} {k}={v}" for k, v in c.get(group, {}).items() if v != "0" and k not in (ignore or ())]
    nonzero += [f"A drv {k}={v}" for k, v in c.get("A drv", {}).items() if k in ("overrun", "q-full") and v != "0"]
    rows.append(Row(f"E-{tag}-2", f"Master and driver error counters ({tag})", stats_how,
                    "all 0 (master ok excluded)", ", ".join(nonzero) or "all 0", verdict(bool(c) and not nonzero)))
    fault = c.get("A fault", {})
    rows.append(Row(f"E-{tag}-3", f"Fault state ({tag})", "gen.ps1 console 'ipmb stats'", "state=ok, streak 0/N",
                    f"streak={fault.get('streak')} state={fault.get('state')} busy-errata-resets={fault.get('busy-errata-resets')}",
                    verdict(fault.get("state") == "ok" and str(fault.get("streak", "")).startswith("0/"))))
    link = re.search(r"Link sensor data \(sensor 1\):\s*([0-9a-f ]+)", state_text)
    rows.append(Row(f"E-{tag}-4", f"Link sensor bytes ({tag})", "gen.ps1 console 'ipmb state'", LINK_SENSOR_OK,
                    normalise(link.group(1)) if link else "not found", verdict(bool(link) and normalise(link.group(1)) == LINK_SENSOR_OK)))


def snapshot_health(report: Report, tag: str, check_traffic: bool):
    rows = report.sections["E"]["rows"]
    stats = console("ipmb stats")
    state = console("ipmb state")
    health_rows(rows, tag, stats, state, check_traffic)


def section_c(report: Report, duration_s: float):
    """Chassis Manager on the Aardvark, then its cross-check. Returns (log text, summary dict)."""
    rows = report.section("C", "Chassis Manager (Phase 7)")
    SCRATCH.mkdir(exist_ok=True)
    summary_path = SCRATCH / "board_report_cm.json"
    summary_path.unlink(missing_ok=True)
    cm_cmd = [VENV_PYTHON, CM_SCRIPT, "--transport", "aardvark", "--slots", "1", "--duration-s", duration_s,
              "--json", summary_path]
    how = "chassis_manager.py --transport aardvark --slots 1 --duration-s " + f"{duration_s:g}"
    rc, log = run(cm_cmd, duration_s + CHILD_TIMEOUT_MARGIN_S)
    summary = json.loads(summary_path.read_text(encoding="utf-8")) if summary_path.exists() else {}
    ipmc = (summary.get("ipmcs") or [{}])[0]
    poll, totals = summary.get("poll", {}), summary.get("totals", {})
    cycles_wanted = max(1, int(duration_s) - 1)
    board = ipmc.get("fru", {}).get("board", {})
    checks = (
        ("Chassis Manager exit status", "exit 0", f"exit {rc}", rc == 0),
        ("Discovery: IPMC found at 82h", "found: 82h", "found: 82h" if "found: 82h" in log else "not found", "found: 82h" in log),
        ("Discovery: Broadcast Get Device ID answered", "broadcast ... -> ok",
         "answered" if re.search(r"broadcast Get Device ID .* -> ok", log) else "no answer",
         bool(re.search(r"broadcast Get Device ID .* -> ok", log))),
        ("Get VSO Capabilities", "ok, Layer 1 IPMC, Tier-1, one IPMB interface",
         (re.search(r"Get VSO Capabilities .* -> (ok .*)", log) or [None, "no response"])[1],
         bool(re.search(r"Get VSO Capabilities .* -> ok Layer 1 IPMC, Tier-1, ipmb interfaces 1", log))),
        ("FRU read", "256 of 256 bytes", (re.search(r"read \d+ of \d+ FRU bytes", log) or ["no read"])[0],
         "read 256 of 256 FRU bytes" in log),
        ("FRU board area decoded", "manufacturer and serial from defs.rs",
         f"{board.get('manufacturer')}, {board.get('serial')}",
         bool(board.get("manufacturer")) and bool(board.get("serial"))),
        ("SDR walk", "8 SDR records, 7 sensors", (re.search(r"\d+ SDR records read and decoded, \d+ sensors", log) or ["no walk"])[0],
         "8 SDR records read and decoded, 7 sensors" in log),
        ("Locator record ID matches the MC locator SDR", "match", "match" if "matches the MC locator" in log else "no match",
         "matches the MC locator" in log),
        ("Polling cycles", f">= {cycles_wanted}", poll.get("cycles"), (poll.get("cycles") or 0) >= cycles_wanted),
        ("Polling lost readings", "0", poll.get("lost_readings"), poll.get("lost_readings") == 0),
        ("Request timeouts", "0", f"timeouts {totals.get('timeouts')}, after discovery {totals.get('timeouts_after_discovery')}",
         totals.get("timeouts") == 0 and totals.get("timeouts_after_discovery") == 0),
    )
    for n, (what, expected, observed, ok) in enumerate(checks, start=1):
        rows.append(Row(f"C{n:02d}", what, how, expected, observed, verdict(bool(ok))))
    time.sleep(PHASE_GAP_S)
    rc, text = run([VENV_PYTHON, CROSSCHECK_SCRIPT, summary_path], CHILD_TIMEOUT_MARGIN_S)
    result = re.search(r"RESULT: (\d+) unexpected difference\(s\)", text)
    rows.append(Row(f"C{len(checks) + 1:02d}", "Cross-check against the captured Chassis Manager",
                    "crosscheck.py .scratch/board_report_cm.json", "RESULT: 0 unexpected difference(s)",
                    result.group(0) if result else f"no result (rc {rc})", verdict(bool(result) and result.group(1) == "0" and rc == 0)))
    return log, summary


def section_b(report: Report, defs: dict, cm_log: str):
    """Command line rows: the README table, run through ipmitool.exe."""
    rows = report.section("B", "Command line (Phase 8)")
    commands, probes = read_command_table()
    checks = named_checks(defs)
    if len(commands) != 14 or len(probes) < 6:
        rows.append(Row("B00", "README tables", str(README.relative_to(ROOT)), "14 command rows, 6 error probes",
                        f"{len(commands)} command rows, {len(probes)} error probes", "FAIL"))
    for entry in commands:
        cid = entry["id"]
        if entry["raw"] is None and not entry["named"]:
            bcast = bool(re.search(r"broadcast Get Device ID .* -> ok", cm_log))
            rows.append(Row(f"B-{cid}", f"{entry['name']}: no command-line form", "section C discovery step",
                            "answered through the Chassis Manager",
                            "answered in section C" if bcast else "no answer in section C", verdict(bcast)))
            continue
        expected_raw = (backticks(entry["expected"]) or [""])[0]
        if entry["raw"]:
            rc, text = ipmitool(entry["raw"])
            ok = rc == 0 and match_bytes(expected_raw, text)
            if cid == "C05":
                tokens = hex_tokens(text)
                ok = ok and len(tokens) == 3 and tokens[1:] != ["00", "00"]
            rows.append(Row(f"B-{cid}r", f"{entry['name']} (raw)", f"ipmitool {' '.join(IPMITOOL_PREFIX)} {entry['raw']}",
                            expected_raw, f"rc {rc}: {normalise(text)}", verdict(ok)))
        for index, named in enumerate(entry["named"], start=1):
            rc, text = ipmitool(named)
            ok, note = checks[named](text)
            rows.append(Row(f"B-{cid}n{index}", f"{entry['name']} ({named})", f"ipmitool {' '.join(IPMITOOL_PREFIX)} {named}",
                            f"decoded fields of the {named} output", f"rc {rc}: {note}",
                            verdict(rc == 0 and ok and "Traceback" not in text)))
    for probe in probes:
        rc, text = ipmitool(probe["raw"])
        rows.append(Row(f"B-{probe['id']}", f"Error probe: {probe['name']}", f"ipmitool {' '.join(IPMITOOL_PREFIX)} {probe['raw']}",
                        probe["expected"], f"rc {rc}: {normalise(text)}",
                        verdict(rc == 0 and match_bytes(probe["expected"], text))))


def section_d(report: Report):
    rows = report.section("D", "Conformance (Phase 7)",
                          headers=("ID", "Rule", "How", "Expected", "Note", "Result"))
    how = "run_conformance.py --transport aardvark"
    CONFORMANCE_REPORT.unlink(missing_ok=True)
    rc, text = run([VENV_PYTHON, CONFORMANCE_SCRIPT, "--transport", "aardvark", "--report", CONFORMANCE_REPORT],
                   CONFORMANCE_TIMEOUT_S, cwd=CONFORMANCE_SCRIPT.parent)
    if not CONFORMANCE_REPORT.exists():
        rows.append(Row("D00", "Conformance run", how, "a report", f"no report (rc {rc}): {text[-300:]}", "FAIL"))
        return
    for line in CONFORMANCE_REPORT.read_text(encoding="utf-8").splitlines():
        parts = [p.strip() for p in re.split(r"(?<!\\)\|", line.strip().strip("|"))]
        if len(parts) == 5 and parts[3] in STATUSES:
            rows.append(Row(parts[0], f"{parts[1]} (page {parts[2]})", how, "PASS", parts[4], parts[3]))
    counts = {s: sum(r.status == s for r in rows) for s in STATUSES}
    totals = re.search(r"Requirements: \d+; tests: \d+; .*", text)
    rows.append(Row("D-total", "Conformance totals", how, "FAIL 0", totals.group(0) if totals else str(counts),
                    verdict(counts["FAIL"] == 0 and rc == 0)))


def read_bytes(offset: int, count: int) -> list[str] | None:
    """Read FRU 0 through ipmitool raw; the data bytes as hex tokens, or None."""
    rc, text = ipmitool(f"raw 0x0a 0x11 0x00 {offset & 255:#04x} {offset >> 8:#04x} {count:#04x}")
    tokens = hex_tokens(text)
    return tokens[2:] if rc == 0 and len(tokens) == count + 2 and tokens[:2] == ["00", f"{count:02x}"] else None


def read_area(offset: int, length: int) -> list[int] | None:
    data = []
    while len(data) < length:
        chunk = read_bytes(offset + len(data), min(FRU_READ_CHUNK, length - len(data)))
        if chunk is None:
            return None
        data += [int(t, 16) for t in chunk]
    return data


def section_f(report: Report, defs: dict):
    """Write FRU Data persistence across a reset, then restore the built image."""
    rows = report.section("F", "FRU persistence (Phase 6b)")
    delay_s = int(defs["IPMC_FRU_PERSIST_DELAY_MS"]) / 1000 + PERSIST_MARGIN_S
    serial = defs["IPMC_FRU_BOARD_SERIAL"]
    next_id = [1]

    def add(what, how, expected, observed, ok):
        rows.append(Row(f"F{next_id[0]:02d}", what, how, expected, observed, verdict(ok)))
        next_id[0] += 1
        return ok

    header = read_bytes(0, 8)
    board_off = int(header[3], 16) * 8 if header else 0
    length_byte = read_bytes(board_off, 2) if board_off else None
    length = int(length_byte[1], 16) * 8 if length_byte else 0
    area = read_area(board_off, length) if length else None
    if not add("Read the board area", f"raw 0x0a 0x11 ... ({FRU_READ_CHUNK}-byte reads)", "area read, checksum sums to 0",
               f"offset {board_off}, {length} bytes, sum {sum(area) % 256 if area else 'n/a'}", bool(area) and sum(area) % 256 == 0):
        return
    text = bytes(area)
    at = text.find(serial.encode())
    if at < 0:
        add("Locate the serial number in the board area", "search the area bytes", serial, "not found", False)
        return
    pos = at + len(serial) - 1                      # last character of the serial number
    csum = length - 1                               # the area checksum byte
    original = (area[pos], area[csum])
    changed_char = ord("2") if chr(area[pos]) != "2" else ord("3")
    new_area = list(area)
    new_area[pos] = changed_char
    new_area[csum] = (-sum(new_area[:csum])) & 255
    offset = board_off + pos
    restored = False
    try:
        # One byte per write: an IPMB message is at most 32 bytes, so the span from the serial
        # character to the checksum does not fit in one request.
        writes = ((offset, new_area[pos]), (board_off + csum, new_area[csum]))
        results = []
        for at, value in writes:
            command = f"raw 0x0a 0x12 0x00 {at & 255:#04x} {at >> 8:#04x} {value:#04x}"
            rc, out = ipmitool(command)
            results.append((command, rc == 0 and hex_tokens(out) == ["00", "01"], normalise(out)))
        add("Write FRU Data: changed serial character, then corrected checksum",
            " ; ".join(c for c, _, _ in results), "00 01 for each write",
            " ; ".join(o for _, _, o in results), all(ok for _, ok, _ in results))
        time.sleep(delay_s)
        store = console("fru store")
        add("Flash log saved the write", "console 'fru store'", "write pending no, failures 0",
            (re.search(r"write pending \w+.*failures \d+", store) or ["not found"])[0],
            bool(re.search(r"write pending no.*failures 0", store)))
        console("sys reset")
        time.sleep(RESET_SETTLE_S)
        back = (read_bytes(offset, 1), read_bytes(board_off + csum, 1))
        survived = back[0] == [f"{changed_char:02x}"] and back[1] == [f"{new_area[csum]:02x}"]
        add("The write survived a reset", "sys reset, then raw 0x0a 0x11 ...",
            f"char {changed_char:02x}, checksum {new_area[csum]:02x}", f"{back}", survived)
        show = console("fru show")
        new_serial = serial[:-1] + chr(changed_char)
        board_sum = re.search(r"Board area at.*checksum (\w+)", show)
        add("Firmware decodes the changed image", "console 'fru show'", f"serial {new_serial}, board checksum OK",
            f"{'serial changed' if new_serial in show else 'serial unchanged'}, {board_sum.group(1) if board_sum else 'n/a'}",
            new_serial in show and bool(board_sum) and board_sum.group(1) == "OK")
    finally:
        console("fru restore")
        console("sys reset")
        time.sleep(RESET_SETTLE_S)
        restored = True
    back = (read_bytes(offset, 1), read_bytes(board_off + csum, 1))
    add("Restore: original bytes back after fru restore and reset", "console 'fru restore', 'sys reset', raw read",
        f"char {original[0]:02x}, checksum {original[1]:02x}", f"{back}",
        restored and back[0] == [f"{original[0]:02x}"] and back[1] == [f"{original[1]:02x}"])
    show = console("fru show")
    add("Restore: firmware decodes the built image", "console 'fru show'", f"serial {serial}, checksums OK",
        f"{'serial restored' if serial in show else 'serial missing'}, {len(re.findall('checksum OK', show))} checksums OK",
        serial in show and "checksum BAD" not in show)
    rc, out = ipmitool("raw 0x06 0x04")
    add("Get Self Test Results at the end", "ipmitool raw 0x06 0x04", SELF_TEST_OK, f"rc {rc}: {normalise(out)}",
        rc == 0 and match_bytes(SELF_TEST_OK, out))


def section_g(report: Report) -> list[str]:
    lines = ["## G. Summary", "", "| Section | PASS | FAIL | NOT-TESTABLE |", "|---|---|---|---|"]
    totals = dict.fromkeys(STATUSES, 0)
    failures = []
    for letter, sec in sorted(report.sections.items()):
        if letter == "G":
            continue
        counts = {s: sum(r.status == s for r in sec["rows"]) for s in STATUSES}
        lines.append(f"| {letter} {sec['title']} | " + " | ".join(str(counts[s]) for s in STATUSES) + " |")
        for s in STATUSES:
            totals[s] += counts[s]
        failures += [(letter, r) for r in sec["rows"] if r.status == "FAIL"]
    lines.append("| Total | " + " | ".join(str(totals[s]) for s in STATUSES) + " |")
    lines += ["", "FAIL rows: " + (", ".join(f"{letter}:{r.id}" for letter, r in failures) if failures else "none"), ""]
    if failures:
        lines += md_table(("ID", "What", "How", "Expected", "Observed", "Result"), [r for _, r in failures]) + [""]
    lines += ["Open later checks (PLAN.md section 7) that this report does not cover:", ""]
    lines += [f"- not covered here: {item}" for item in NOT_COVERED] + [""]
    return lines


def render(report: Report, started: str) -> str:
    lines = ["# Board test report", "",
             f"Generated by `tools/board_test_report.py` on {started}. The file is overwritten on every run; do not edit it.", ""]
    for letter in sorted(report.sections):
        sec = report.sections[letter]
        lines += [f"## {letter}. {sec['title']}", ""]
        if sec["intro"]:
            lines += [sec["intro"], ""]
        lines += md_table(sec["headers"], sec["rows"]) + [""]
    lines += section_g(report)
    return "\n".join(lines)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--duration-s", type=float, default=10.0, help="Chassis Manager polling length")
    parser.add_argument("--skip-persistence", action="store_true", help="skip section F (FRU write and reset)")
    args = parser.parse_args(argv)
    started = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    defs = read_defs()
    report = Report()
    section_a(report, defs)
    report.section("E", "Link and driver health")
    snapshot_health(report, "before", check_traffic=False)
    time.sleep(PHASE_GAP_S)
    cm_log, _ = section_c(report, args.duration_s)
    time.sleep(PHASE_GAP_S)
    section_b(report, defs, cm_log)
    time.sleep(PHASE_GAP_S)
    section_d(report)
    snapshot_health(report, "after", check_traffic=True)
    if not args.skip_persistence:
        time.sleep(PHASE_GAP_S)
        section_f(report, defs)
    text = render(report, started)
    REPORT.parent.mkdir(exist_ok=True)
    REPORT.write_text(text, encoding="utf-8")
    failed = sum(r.status == "FAIL" for _, r in report.all_rows())
    print(text.split("## G. Summary")[1] if "## G. Summary" in text else "")
    print(f"report written to {REPORT}; FAIL rows: {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
