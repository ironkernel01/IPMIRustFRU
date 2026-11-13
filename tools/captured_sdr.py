#!/usr/bin/env python3
"""captured_sdr.py - local_only/Documentation/captured_sdr.md: the SDR records of the captured live system.

Reassembles every Device SDR record the captured SBC IPMC returned to the Chassis Manager from
the two Saleae exports (IPMB-A, IPMB-B), decodes each field per the IPMI v2.0 tables and the
VITA 46.11-2022 sensor definitions, checks the records against those rules, and compares the
first seven sensors and the locator with the records of this project's IPMC
(`cargo run -q -p ipmc-emu -- sdr`, decoded by the same decoder). That run sets IPMC_TIER23=0:
the document compares the captured system with the public Tier-1 records, whatever extension
folder sits beside this repository.

Inputs (read only):  local_only/References/CodeReferences/ipmi-pic32/traces/sbca.csv and sbcb.csv
Output:              local_only/Documentation/captured_sdr.md
Name list:           local_only/.scratch/banned_names.txt (local only, one term per line). Every match inside
                     a decoded string becomes REDACTED. A missing list is exit 2: the tool never
                     writes unredacted output.

Usage:  python -I tools/captured_sdr.py           rewrite the document
        python -I tools/captured_sdr.py --check   write nothing; exit 1 when the document is stale
Exit codes: 0 up to date (or written), 1 stale under --check, 2 input error.

Frame layout used (IPMI v2.0 IPMB message, first byte = 8-bit destination address):
  request  rsSA, netFn<<2|rsLUN, chk1, rqSA, seq<<2|rqLUN, cmd, data..., chk2
  response rqSA, netFn<<2|rqLUN, chk1, rsSA, seq<<2|rsLUN, cmd, cc, data..., chk2
"""

import difflib
import importlib.util
import math
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TRACE_A = ROOT / "local_only" / "References" / "CodeReferences" / "ipmi-pic32" / "traces" / "sbca.csv"
TRACE_B = ROOT / "local_only" / "References" / "CodeReferences" / "ipmi-pic32" / "traces" / "sbcb.csv"
NAMES = ROOT / "local_only" / ".scratch" / "banned_names.txt"
DOC = ROOT / "local_only" / "Documentation" / "captured_sdr.md"
TRACE2VEC = ROOT / "tools" / "trace2vec.py"

CM_ADDR = 0x10  # Chassis Manager, 8-bit IPMB address
SBC_ADDR = 0x82  # captured SBC IPMC, 8-bit IPMB address
NETFN_SE_REQ = 0x04
NETFN_SE_RSP = 0x05
CMD_GET_SDR_INFO = 0x20
CMD_GET_SDR = 0x21
CMD_RESERVE = 0x22
CMD_GET_READING = 0x2D
IDX_NETFN, IDX_SEQ, IDX_CMD, IDX_DATA = 1, 4, 5, 6
NO_NEXT = 0xFFFF
REDACTED = "REDACTED"

# ----------------------------------------------------------------------------- code tables

SENSOR_TYPES = {
    0x01: "Temperature", 0x02: "Voltage", 0x03: "Current", 0x04: "Fan",
    0x05: "Physical Security (Chassis Intrusion)", 0x06: "Platform Security Violation Attempt",
    0x07: "Processor", 0x08: "Power Supply", 0x09: "Power Unit", 0x0A: "Cooling Device",
    0x0B: "Other Units-based Sensor", 0x0C: "Memory", 0x0D: "Drive Slot (Bay)",
    0x0E: "POST Memory Resize", 0x0F: "System Firmware Progress",
    0x10: "Event Logging Disabled", 0x11: "Watchdog 1", 0x12: "System Event",
    0x13: "Critical Interrupt", 0x14: "Button / Switch", 0x15: "Module / Board",
    0x16: "Microcontroller / Coprocessor", 0x17: "Add-in Card", 0x18: "Chassis",
    0x19: "Chip Set", 0x1A: "Other FRU", 0x1B: "Cable / Interconnect", 0x1C: "Terminator",
    0x1D: "System Boot / Restart Initiated", 0x1E: "Boot Error",
    0x1F: "Base OS Boot / Installation Status", 0x20: "OS Stop / Shutdown",
    0x21: "Slot / Connector", 0x22: "System ACPI Power State", 0x23: "Watchdog 2",
    0x24: "Platform Alert", 0x25: "Entity Presence", 0x26: "Monitor ASIC / IC", 0x27: "LAN",
    0x28: "Management Subsystem Health", 0x29: "Battery", 0x2A: "Session Audit",
    0x2B: "Version Change", 0x2C: "FRU State",
    0xF0: "FRU State (VITA)", 0xF1: "System IPMB Link (VITA)", 0xF2: "FRU Health (VITA)",
    0xF3: "FRU Temperature (VITA)", 0xF4: "Payload Test Results (VITA)",
    0xF5: "Payload Test Status (VITA)", 0xF6: "reserved for an external standard (VITA)",
    0xF7: "Payload Mode (VITA)", 0xF8: "IPMC Reset Type (VITA)",
}
EVENT_TYPES = {
    0x00: "unspecified", 0x01: "threshold", 0x02: "DMI-based usage state (generic discrete)",
    0x03: "digital discrete: state deasserted / asserted",
    0x04: "digital discrete: predictive failure deasserted / asserted",
    0x05: "digital discrete: limit not exceeded / exceeded",
    0x06: "digital discrete: performance met / lags", 0x07: "severity (generic discrete)",
    0x08: "digital discrete: device removed / inserted",
    0x09: "digital discrete: device disabled / enabled",
    0x0A: "availability state (generic discrete)", 0x0B: "redundancy (generic discrete)",
    0x0C: "ACPI device power state (generic discrete)", 0x6F: "sensor-specific discrete",
}
# Number of defined state offsets of the generic discrete types (IPMI Table 42-2); the types
# that are not listed are not checked.
EVENT_OFFSETS = {0x02: 3, 0x03: 2, 0x04: 2, 0x05: 2, 0x06: 2, 0x07: 8, 0x08: 2, 0x09: 2}
# VITA 46.11-2022 Table 10.2-1: sensor number -> (sensor type, event/reading type), FRU 0 LUN 0.
VITA_SENSORS = {
    0: (0xF0, 0x6F), 1: (0xF1, 0x6F), 2: (0xF2, 0x04), 3: (0x02, 0x05),
    4: (0xF3, 0x6F), 5: (0xF4, 0x04), 6: (0xF5, 0x03),
}
ENTITIES = {
    0x00: "unspecified", 0x01: "other", 0x02: "unknown", 0x03: "processor",
    0x04: "disk or disk bay", 0x05: "peripheral bay", 0x06: "system management module",
    0x07: "system board", 0x08: "memory module", 0x09: "processor module",
    0x0A: "power supply", 0x0B: "add-in card", 0x0C: "front panel board",
    0x0D: "back panel board", 0x0E: "power system board", 0x0F: "drive backplane",
    0x10: "system internal expansion board", 0x11: "other system board",
    0x12: "processor board", 0x13: "power unit / power domain",
    0x14: "power module / DC-to-DC converter", 0x15: "power management / distribution board",
    0x16: "chassis back panel board", 0x17: "system chassis", 0x18: "sub-chassis",
    0x19: "other chassis board", 0x1A: "disk drive bay", 0x1B: "peripheral bay",
    0x1C: "device bay", 0x1D: "fan / cooling device", 0x1E: "cooling unit / cooling domain",
    0x1F: "cable / interconnect", 0x20: "memory device", 0x21: "system management software",
    0x22: "system firmware", 0x23: "operating system", 0x24: "system bus", 0x25: "group",
    0x26: "remote (out of band) management communication device",
    0x27: "external environment", 0x28: "battery", 0x29: "processing blade",
    0x2A: "connectivity switch", 0x2B: "processor / memory module", 0x2C: "I/O module",
    0x2D: "processor / IO module", 0x2E: "management controller firmware",
    0x2F: "IPMI channel", 0x30: "PCI bus", 0x31: "PCI Express bus", 0x32: "SCSI bus",
    0x33: "SATA / SAS bus", 0x34: "processor / front-side bus", 0x35: "real time clock",
    0x37: "air inlet", 0x40: "air inlet", 0x41: "processor / CPU", 0x42: "baseboard",
}
UNITS = [
    "unspecified", "degrees C", "degrees F", "degrees K", "Volts", "Amps", "Watts", "Joules",
    "Coulombs", "VA", "Nits", "lumen", "lux", "Candela", "kPa", "PSI", "Newton", "CFM", "RPM",
    "Hz", "microsecond", "millisecond", "second", "minute", "hour", "day", "week", "mil",
    "inches", "feet", "cu in", "cu feet", "mm", "cm", "m", "cu cm", "cu m", "liters",
    "fluid ounce", "radians", "steradians", "revolutions", "cycles", "gravities", "ounce",
    "pound", "ft-lb", "oz-in", "gauss", "gilberts", "henry", "millihenry", "farad",
    "microfarad", "ohms", "siemens", "mole", "becquerel", "PPM (parts/million)", None,
    "Decibels", "DbA", "DbC", "gray", "sievert", "color temp deg K", "bit", "kilobit",
    "megabit", "gigabit", "byte", "kilobyte", "megabyte", "gigabyte", "word (data)", "dword",
    "qword", "line (re. mem. line)", "hit", "miss", "retry", "reset", "overrun / overflow",
    "underrun", "collision", "packets", "messages", "characters", "error",
    "correctable error", "uncorrectable error", "fatal error", "grams",
]
RATES = {0: "none", 1: "per microsecond", 2: "per ms", 3: "per s", 4: "per minute",
         5: "per hour", 6: "per day"}
LINEARIZATION = {0: "linear", 1: "ln", 2: "log10", 3: "log2", 4: "e", 5: "exp10", 6: "exp2",
                 7: "1/x", 8: "sqr(x)", 9: "cube(x)", 10: "sqrt(x)", 11: "cube-1(x)"}
THRESH_NAMES = ["lower non-critical", "lower critical", "lower non-recoverable",
                "upper non-critical", "upper critical", "upper non-recoverable"]
# Threshold event bits (IPMI Table 43-1 bytes 15:18): bit n -> name.
THRESH_EVENT_BITS = [
    "lower non-critical going low", "lower non-critical going high",
    "lower critical going low", "lower critical going high",
    "lower non-recoverable going low", "lower non-recoverable going high",
    "upper non-critical going low", "upper non-critical going high",
    "upper critical going low", "upper critical going high",
    "upper non-recoverable going low", "upper non-recoverable going high",
]
VITA_FRU_STATES = {0: "M0", 1: "M1", 4: "M4", 5: "M5", 6: "M6", 7: "M7"}

# ----------------------------------------------------------------------------- helpers


class InputError(Exception):
    pass


def hexs(data):
    return " ".join("%02X" % b for b in data)


def le16(data, i):
    return data[i] | data[i + 1] << 8


def signed(value, bits):
    return value - (1 << bits) if value & (1 << (bits - 1)) else value


def fmt_val(v):
    if v is None:
        return "n/a"
    text = "%.6g" % v
    return "0" if text == "-0" else text


def md_cell(text):
    return str(text).replace("|", "\\|").replace("\n", " ")


def name_of(table, code, what="reserved"):
    value = table.get(code) if isinstance(table, dict) else (
        table[code] if 0 <= code < len(table) else None)
    return value if value else "%s (%02Xh)" % (what, code)


def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class Redactor:
    """Replaces every match of the local name list inside a string with REDACTED."""

    def __init__(self):
        if not NAMES.is_file():
            raise InputError("%s is missing: refusing to write unredacted output" % NAMES)
        terms = [t.strip() for t in NAMES.read_text(encoding="utf-8").splitlines() if t.strip()]
        if not terms:
            raise InputError("%s holds no terms" % NAMES)
        self.pattern = re.compile("|".join(re.escape(t) for t in terms), re.IGNORECASE)
        self.count = 0

    def __call__(self, text):
        out, n = self.pattern.subn(REDACTED, text)
        self.count += n > 0
        return out

    def hit(self, text):
        return bool(self.pattern.search(text))


# ----------------------------------------------------------------------------- capture


class Exchange:
    def __init__(self, t_req, t_rsp, bus_req, bus_rsp, req, rsp):
        self.t_req, self.t_rsp, self.bus_req, self.bus_rsp = t_req, t_rsp, bus_req, bus_rsp
        self.req, self.rsp = req, rsp  # frames, bytes

    @property
    def cc(self):
        return self.rsp[IDX_DATA]

    @property
    def req_data(self):
        return self.req[IDX_DATA:-1]

    @property
    def rsp_data(self):
        return self.rsp[IDX_DATA + 1:-1]  # after the completion code


def read_exchanges():
    """Every request to the SBC with the response that answers it (same seq and cmd, next in
    time, from the SBC). Returns (exchanges in request order, unanswered request count, responses
    whose request is not in the capture)."""
    t2v = load_module(TRACE2VEC, "trace2vec_for_sdr")
    counts = t2v.Counts()
    frames = t2v.read_transactions(str(TRACE_A), "A", counts)
    frames += t2v.read_transactions(str(TRACE_B), "B", counts)
    frames.sort(key=lambda r: r[0])
    pending = {}
    done = []
    orphans = []
    for t, bus, frame in frames:
        if not t2v.frame_valid(frame):
            continue
        netfn = frame[IDX_NETFN] >> 2
        seq = frame[IDX_SEQ] >> 2
        cmd = frame[IDX_CMD]
        if frame[0] == SBC_ADDR and frame[3] == CM_ADDR and netfn == NETFN_SE_REQ:
            pending[(seq, cmd)] = (t, bus, frame)
        elif frame[0] == CM_ADDR and frame[3] == SBC_ADDR and netfn == NETFN_SE_RSP:
            req = pending.pop((seq, cmd), None)
            if req is not None:
                done.append(Exchange(req[0], t, req[1], bus, req[2], frame))
            else:
                orphans.append((t, bus, frame))
    done.sort(key=lambda e: e.t_req)
    return done, len(pending), orphans


class Capture:
    """The SDR traffic of the capture."""

    def __init__(self):
        exchanges, self.unanswered, self.orphans = read_exchanges()
        self.inferred = []
        self.info = [e for e in exchanges if e.req[IDX_CMD] == CMD_GET_SDR_INFO]
        self.reserve = [e for e in exchanges if e.req[IDX_CMD] == CMD_RESERVE]
        self.reads = [e for e in exchanges if e.req[IDX_CMD] == CMD_GET_SDR]
        self.readings = [e for e in exchanges if e.req[IDX_CMD] == CMD_GET_READING]
        self.chunks = []  # dicts: exchange, reservation, record, offset, count, cc, next, data
        self.records = {}  # requested id -> bytearray(len) filled, plus mask
        self.next_ids = {}
        self.conflicts = 0
        self.stale = []  # reads with a non-zero offset whose reservation is not the latest
        self.cc_errors = []
        self._reassemble()

    def _reassemble(self):
        latest = 0
        events = [(e.t_rsp, "reserve", e) for e in self.reserve]
        events += [(e.t_req, "read", e) for e in self.reads]
        events.sort(key=lambda x: x[0])
        buckets = {}
        for _, kind, e in events:
            if kind == "reserve":
                if e.cc == 0 and len(e.rsp_data) >= 2:
                    latest = le16(e.rsp_data, 0)
                continue
            d = e.req_data
            if len(d) != 6:
                continue
            res, rid, off, cnt = le16(d, 0), le16(d, 2), d[4], d[5]
            chunk = {"ex": e, "res": res, "rid": rid, "off": off, "cnt": cnt, "cc": e.cc,
                     "next": None, "data": b"", "latest": latest}
            if e.cc != 0:
                self.cc_errors.append(chunk)
            else:
                chunk["next"] = le16(e.rsp_data, 0)
                chunk["data"] = bytes(e.rsp_data[2:])
                self.next_ids.setdefault(rid, chunk["next"])
                if off != 0 and res != latest:
                    self.stale.append(chunk)
                buckets.setdefault(rid, []).append(chunk)
            self.chunks.append(chunk)
        # A Get Device SDR response whose request is not in the capture continues the record of
        # the latest read that returned the same next-record ID, at the end of that record's
        # data so far. Such a chunk is marked inferred.
        for t, bus, frame in sorted(self.orphans, key=lambda o: o[0]):
            if frame[IDX_CMD] != CMD_GET_SDR or frame[IDX_DATA] != 0 or len(frame) < 11:
                continue
            nxt = le16(frame, IDX_DATA + 1)
            prior = [c for c in self.chunks if c["cc"] == 0 and c["next"] == nxt
                     and c["ex"].t_req < t and not c.get("inferred")]
            if not prior:
                continue
            prev = max(prior, key=lambda c: c["ex"].t_req)
            rid = prev["rid"]
            end = prev["off"] + len(prev["data"])
            data = bytes(frame[IDX_DATA + 3:-1])
            chunk = {"ex": None, "res": prev["res"], "rid": rid, "off": end, "cnt": len(data),
                     "cc": 0, "next": nxt, "data": data, "latest": latest, "inferred": True,
                     "t": t, "bus": bus}
            buckets[rid].append(chunk)
            self.chunks.append(chunk)
            self.inferred.append(chunk)
        self.assembled = {}
        for rid, chunks in buckets.items():
            first = next((c for c in chunks if c["off"] == 0 and len(c["data"]) >= 5), None)
            if first is None:
                self.assembled[rid] = (bytearray(), set())
                continue
            total = first["data"][4] + 5
            buf = bytearray(total)
            have = set()
            for c in chunks:
                for i, b in enumerate(c["data"]):
                    pos = c["off"] + i
                    if pos >= total:
                        continue
                    if pos in have and buf[pos] != b:
                        self.conflicts += 1
                    buf[pos] = b
                    have.add(pos)
            self.assembled[rid] = (buf, have)

    def gaps(self):
        out = {}
        for rid, (buf, have) in self.assembled.items():
            missing = [i for i in range(len(buf)) if i not in have]
            if missing or not buf:
                out[rid] = missing
        return out


# ----------------------------------------------------------------------------- decoder


class Field:
    def __init__(self, start, end, name, meaning, issues=None):
        self.start, self.end, self.name, self.meaning = start, end, name, meaning
        self.issues = issues or []  # list of check ids
        self.value = None  # effective value of a mask field


class Decoded:
    """One record decoded: fields, derived values and the failed checks."""

    def __init__(self, data):
        self.data = bytes(data)
        self.fields = []
        self.checks = []  # (field name, check id)
        self.sensor_number = None
        self.sensor_type = None
        self.event_type = None
        self.share = 1
        self.name_bytes = b""
        self.unit = ""
        self.nominal_text = ""
        self.thr_text = ""
        self.record_type = data[3] if len(data) > 3 else None
        self.raw_value = {}
        self.masks = {}
        self.evidence = []
        self.mask_order = "le"

    def raw(self, start, end):
        return hexs(self.data[start:end + 1])

    def add(self, start, end, name, meaning, *issues):
        f = Field(start, end, name, meaning, [i for i in issues if i])
        self.fields.append(f)
        for i in f.issues:
            self.checks.append((name, i))
        return f

    def field(self, name):
        return next((f for f in self.fields if f.name == name), None)


# Check table: id -> (observation text, citation). Every observation in the document comes from
# a failed check of this table.
CHECKS = {
    "version": ("SDR Version is not 51h.", "IPMI v2.0 Table 43-1 byte 3"),
    "sensor-ff": ("Sensor number FFh is used.", "IPMI v2.0 Table 43-1 byte 8 (FFh reserved)"),
    "full-chan-res": ("Reserved bits [3:2] of the channel/LUN byte are set.",
                      "IPMI v2.0 Table 43-1 byte 7"),
    "entity-reserved": ("Entity ID is not a defined code and not in an OEM or chassis-specific "
                        "range.", "IPMI v2.0 Table 43-13"),
    "init-events": ("Init Events is set although the event message control field says no "
                    "events.", "IPMI v2.0 Table 43-1 bytes 11 and 12"),
    "init-thr-discrete": ("Init Thresholds is set on a discrete sensor.",
                          "IPMI v2.0 Table 43-1 byte 11 (thresholds apply to threshold-based "
                          "sensors)"),
    "init-hyst": ("Init Hysteresis is set although the capabilities report no hysteresis.",
                  "IPMI v2.0 Table 43-1 bytes 11 and 12"),
    "caps-thr-discrete": ("Threshold access support is not 00b on a discrete sensor "
                          "(\"readable and settable thresholds\" on a sensor with none).",
                          "IPMI v2.0 Table 43-1 byte 12 [3:2] (00b = no thresholds)"),
    "caps-hyst-discrete": ("Hysteresis support is not 00b on a discrete sensor.",
                           "IPMI v2.0 Table 43-1 byte 12 [5:4] (00b = no hysteresis)"),
    "caps-events": ("Event message control is not 11b (no events), so the sensor advertises "
                    "event messages.", "VITA 46.11-2022 Rule 7-4 (a Tier-1 IPMC does not act as "
                    "an Event Generator); IPMI v2.0 Table 43-1 byte 12 [1:0]"),
    "type-reserved": ("Sensor type code is reserved (not defined in Table 42-3, or F6h, which VITA "
                      "reserves for an external standard).", "IPMI v2.0 Table 42-3; VITA "
                      "46.11-2022 Table 10.2-3"),
    "event-reserved": ("Event/reading type code is reserved or unspecified.",
                       "IPMI v2.0 Table 42-1"),
    "vita-type": ("Sensor type or event/reading type differs from the mandatory sensor of "
                  "that number.", "VITA 46.11-2022 Table 10.2-1"),
    "mask-order": ("The event and reading masks are stored MS byte first; the tables define "
                   "them LS byte first.", "IPMI v2.0 Table 43-1 bytes 15:20, Table 43-2 bytes "
                   "15:20 (LS byte first)"),
    "mask-reading-empty": ("The discrete reading mask is 0000h, so no state is declared "
                           "readable although Get Sensor Reading returns states.",
                           "IPMI v2.0 Table 43-2 bytes 19:20 (reading mask: discrete readings "
                           "the sensor can return)"),
    "caps-thr-mask": ("Threshold access support disagrees with the settable/readable "
                      "threshold masks (for example fixed unreadable thresholds with a readable "
                      "mask).", "IPMI v2.0 Table 43-1 byte 12 [3:2] and bytes 19:20"),
    "units-modifier-stray": ("The modifier unit code is not 00h although the modifier field "
                             "says none.", "IPMI v2.0 Table 43-1 byte 23 (00h if unused)"),
    "mask-res15": ("Reserved bit 15 of an event or reading mask is set.",
                   "IPMI v2.0 Table 43-1 bytes 15:20, Table 43-2 bytes 15:20 ([15] reserved, "
                   "write as 0b)"),
    "mask-res-settable": ("Reserved bits of the settable/readable threshold mask are set.",
                          "IPMI v2.0 Table 43-1 bytes 19:20 ([15:14] and [7:6] reserved)"),
    "mask-events": ("An assertion or deassertion event mask is not zero, so the sensor "
                    "advertises event generation.", "VITA 46.11-2022 Rule 7-4; IPMI v2.0 "
                    "Table 43-1 bytes 15:18"),
    "mask-undefined": ("A mask bit selects a state offset the event/reading type does not "
                       "define.", "IPMI v2.0 Table 42-2"),
    "units-compact": ("Sensor Units 1 bits [7:6] of a compact record are not 11b.",
                      "IPMI v2.0 Table 43-2 byte 21 ([7:6] reserved, write as 11b, as printed in the "
                      "repository's copy of the specification)"),
    "units-full-none": ("Sensor Units 1 is not 00h although the record provides neither "
                        "thresholds nor an analog reading.",
                        "IPMI v2.0 Table 43-1 byte 21 note ** (write 00h)"),
    "units-reserved": ("A units field holds a reserved code (rate 111b, modifier 11b, or a "
                       "base or modifier unit code that is not defined).",
                       "IPMI v2.0 Table 43-1 byte 21, Table 43-15"),
    "share-zero": ("Share count is 0.", "IPMI v2.0 Table 43-2 byte 24 [3:0] (number of sensors "
                   "sharing the record)"),
    "share-reserved": ("Sensor direction is 11b or the instance modifier type is reserved.",
                       "IPMI v2.0 Table 43-2 byte 24"),
    "reserved-bytes": ("A reserved byte is not 00h.", "IPMI v2.0 Tables 43-1, 43-2, 43-8 "
                       "(reserved, write as 00h)"),
    "idstr-type": ("ID string type is not 8-bit ASCII + Latin 1, or its length is the reserved "
                   "value 1.", "IPMI v2.0 section 43.15"),
    "idstr-long": ("ID string is longer than 16 bytes.", "IPMI v2.0 Tables 43-1, 43-2, 43-8 "
                   "(16 bytes maximum)"),
    "idstr-overrun": ("ID string runs past the end of the record.",
                      "IPMI v2.0 section 43.15; Table 43-1 byte 5"),
    "full-m-zero": ("M is 0, so every raw reading converts to the same value.",
                    "IPMI v2.0 section 36.3"),
    "full-minmax": ("Sensor maximum is below the sensor minimum.", "IPMI v2.0 Table 43-1 "
                    "bytes 35:36"),
    "full-nominal": ("Nominal reading, or a normal limit, lies outside the sensor range or the "
                     "normal range is inverted.", "IPMI v2.0 Table 43-1 bytes 32:36"),
    "full-thr-order": ("Thresholds are not in order lower non-recoverable <= lower critical <= "
                       "lower non-critical <= upper non-critical <= upper critical <= upper "
                       "non-recoverable.", "IPMI v2.0 section 36.5.1 and Table 43-1 bytes "
                       "37:42"),
    "full-thr-range": ("A threshold lies outside the sensor minimum/maximum range.",
                       "IPMI v2.0 Table 43-1 bytes 35:42"),
    "full-flags-res": ("Reserved bits of the analog characteristic flags are set.",
                       "IPMI v2.0 Table 43-1 byte 31 ([7:3] reserved)"),
    "full-dir": ("Sensor direction is the reserved value 11b.", "IPMI v2.0 Table 43-1 byte 29"),
    "full-lin": ("Linearization code is reserved.", "IPMI v2.0 Table 43-1 byte 24"),
    "loc-addr": ("Device slave address bit 0 is set.", "IPMI v2.0 Table 43-8 byte 6 ([0] "
                 "reserved)"),
    "loc-chan": ("Channel number byte has reserved bits [7:4] set.",
                 "IPMI v2.0 Table 43-8 byte 7"),
    "loc-power": ("Power state notification / global initialization has a reserved bit or the "
                  "reserved encoding 11b set.", "IPMI v2.0 Table 43-8 byte 8"),
    "loc-eventgen": ("Device capabilities report an IPMB Event Generator.",
                     "VITA 46.11-2022 Rule 7-4; IPMI v2.0 Table 43-8 byte 9 [5]"),
    "loc-init": ("Global initialization enables event message generation from the controller "
                 "(00b).", "VITA 46.11-2022 Rule 7-4; IPMI v2.0 Table 43-8 byte 8 [1:0]"),
}


def entity_valid(code):
    return code in ENTITIES or 0x90 <= code <= 0xFF


def idstring(d, tl_idx, field_start):
    """Adds the ID string fields. Returns the name bytes."""
    tl = d.data[tl_idx] if tl_idx < len(d.data) else 0
    kind, length = tl >> 6, tl & 0x1F
    issues = []
    if kind != 3 or length == 1 or tl & 0x20:
        issues.append("idstr-type")
    if length > 16:
        issues.append("idstr-long")
    end = tl_idx + 1 + length
    if end > len(d.data):
        issues.append("idstr-overrun")
        length = max(0, len(d.data) - tl_idx - 1)
    kinds = {0: "Unicode", 1: "BCD plus", 2: "6-bit ASCII, packed", 3: "8-bit ASCII + Latin 1"}
    d.add(tl_idx, tl_idx, "ID string type/length",
          "type %s; length %d" % (kinds[kind], tl & 0x1F), *issues[:1],
          *[i for i in issues[1:] if i != "idstr-overrun"])
    raw = d.data[tl_idx + 1:tl_idx + 1 + length]
    d.name_bytes = raw
    d.name_span = (tl_idx + 1, tl_idx + length)
    if length:
        d.add(tl_idx + 1, tl_idx + length, "ID string", "<NAME>",
              *(["idstr-overrun"] if "idstr-overrun" in issues else []))
    trailing = d.data[tl_idx + 1 + length:]
    if trailing:
        d.add(tl_idx + 1 + length, len(d.data) - 1, "bytes after the ID string",
              "unspecified, ignored by software")
    return raw


def mask_text(discrete, which, value):
    """Meaning of a mask value (bit 0 = state/event offset 0)."""
    if discrete:
        bits = [b for b in range(15) if value >> b & 1]
        return "states " + (", ".join(str(b) for b in bits) if bits else "none")
    if which in ("assert", "deassert"):
        side = "lower" if which == "assert" else "upper"
        names = ["%s non-critical" % side, "%s critical" % side, "%s non-recoverable" % side]
        got = [names[i] for i in range(3) if value >> (12 + i) & 1]
        ev = [THRESH_EVENT_BITS[b] for b in range(12) if value >> b & 1]
        word = "assertion" if which == "assert" else "deassertion"
        return "%s threshold comparison returned: %s; %s events: %s" % (
            side, ", ".join(got) or "none", word, ", ".join(ev) or "none")
    settable = [THRESH_NAMES[i] for i in range(6) if value >> (8 + i) & 1]
    readable = [THRESH_NAMES[i] for i in range(6) if value >> i & 1]
    return "settable: %s; readable: %s" % (", ".join(settable) or "none",
                                           ", ".join(readable) or "none")


def mask_reserved(discrete, which, value):
    """The reserved bits of a mask that are set."""
    if not discrete and which == "reading":
        return value & 0xC0C0
    return value & 0x8000


def mask_invalid(d, discrete, which, value):
    """True when the value sets a reserved bit or selects an undefined state offset."""
    if mask_reserved(discrete, which, value):
        return True
    limit = EVENT_OFFSETS.get(d.event_type) if discrete else None
    return limit is not None and any(value >> b & 1 for b in range(limit, 15))


def decode_mask_pair(d, discrete, start, name, which):
    """Describes one of the three 16-bit masks (bytes 14:15, 16:17, 18:19). The tables define
    them LS byte first. `d.mask_order` is "le" (as defined) or "be" (MS byte first, chosen for
    the whole capture when the evidence below shows it). In "be" mode every mask whose two
    bytes differ is flagged mask-order. Evidence counts go to d.evidence: a mask that is invalid
    read one way and valid read the other."""
    lo, hi = d.data[start], d.data[start + 1]
    le, be = hi << 8 | lo, lo << 8 | hi
    le_bad = mask_invalid(d, discrete, which, le)
    be_bad = mask_invalid(d, discrete, which, be)
    if le_bad and not be_bad:
        d.evidence.append("be")
    elif be_bad and not le_bad:
        d.evidence.append("le")
    issues = []
    if d.mask_order == "be":
        value = be
        text = "stored MS byte first (defined: LS byte first): %04Xh; %s" % (
            be, mask_text(discrete, which, be))
        if lo != hi:
            issues.append("mask-order")
    else:
        value = le
        text = mask_text(discrete, which, le)
        if mask_reserved(discrete, which, le):
            issues.append("mask-res-settable" if not discrete and which == "reading"
                          else "mask-res15")
    if discrete:
        limit = EVENT_OFFSETS.get(d.event_type)
        if limit is not None and any(b >= limit for b in range(15) if value >> b & 1):
            issues.append("mask-undefined")
        if which in ("assert", "deassert") and value & 0x7FFF:
            issues.append("mask-events")
        if which == "reading" and value == 0:
            issues.append("mask-reading-empty")
    elif which in ("assert", "deassert") and value & 0x0FFF:
        issues.append("mask-events")
    f = d.add(start, start + 1, name, text, *issues)
    f.value = value
    d.masks[which] = value
    return value


def decode_record(data, mask_order="le"):
    d = Decoded(data)
    d.mask_order = mask_order
    n = len(data)
    if n < 5:
        d.add(0, n - 1, "short record", "fewer than 5 bytes")
        return d
    rtype = data[3]
    d.add(0, 1, "Record ID", "%04Xh" % le16(data, 0))
    d.add(2, 2, "SDR version", "%d.%d" % (data[2] & 0xF, data[2] >> 4),
          "version" if data[2] != 0x51 else None)
    tnames = {1: "Full Sensor Record", 2: "Compact Sensor Record",
              0x12: "Management Controller Device Locator Record"}
    d.add(3, 3, "Record type", "%02Xh %s" % (rtype, tnames.get(rtype, "not decoded")))
    d.add(4, 4, "Record length", "%d bytes follow (%d bytes in all)" % (data[4], data[4] + 5))
    if rtype in (1, 2):
        decode_sensor(d)
    elif rtype == 0x12:
        decode_locator(d)
    else:
        d.add(5, n - 1, "record body", "not decoded")
    return d


def decode_sensor(d):
    data = d.data
    full = data[3] == 1
    need = 48 if full else 32
    if len(data) < need:
        d.add(5, len(data) - 1, "record body", "shorter than the layout (%d bytes needed)" % need)
        return
    chan, lun = data[6] >> 4, data[6] & 3
    owner = data[5]
    d.add(5, 5, "Sensor owner ID", "%02Xh: %s %02Xh (8-bit form)" % (
        owner, "IPMB slave address" if not owner & 1 else "system software ID", owner & 0xFE))
    d.add(6, 6, "Channel / LUN", "channel %d; %sowner LUN %d" % (
        chan, "" if full else "FRU inventory device owner LUN %d; sensor " % (data[6] >> 2 & 3),
        lun), "full-chan-res" if full and data[6] & 0x0C else None)
    d.sensor_number = data[7]
    d.add(7, 7, "Sensor number", "%d" % data[7], "sensor-ff" if data[7] == 0xFF else None)
    ent = data[8]
    d.add(8, 8, "Entity ID", "%02Xh %s" % (ent, ENTITIES.get(ent) or (
        "chassis-specific" if 0x90 <= ent <= 0xAF else "board-set specific"
        if 0xB0 <= ent <= 0xCF else "OEM system integrator" if ent >= 0xD0 else
        "reserved (%02Xh)" % ent)), "entity-reserved" if not entity_valid(ent) else None)
    inst = data[9]
    d.add(9, 9, "Entity instance", "%02Xh: %s entity; %s-relative instance %d" % (
        inst, "logical container" if inst & 0x80 else "physical",
        "device" if 0x60 <= (inst & 0x7F) else "system", inst & 0x7F))
    init, caps = data[10], data[11]
    d.sensor_type, d.event_type = data[12], data[13]
    discrete = d.event_type != 0x01
    issues = []
    thr_acc, hyst_acc, evctl = caps >> 2 & 3, caps >> 4 & 3, caps & 3
    if init & 0x20 and evctl == 3:
        issues.append("init-events")
    if init & 0x10 and discrete:
        issues.append("init-thr-discrete")
    if init & 0x08 and hyst_acc == 0:
        issues.append("init-hyst")
    d.add(10, 10, "Sensor initialization", "settable %d; init scanning %d; init events %d; "
          "init thresholds %d; init hysteresis %d; init sensor type %d; power-up event "
          "generation %s; power-up scanning %s" % (
              init >> 7, init >> 6 & 1, init >> 5 & 1, init >> 4 & 1, init >> 3 & 1,
              init >> 2 & 1, "enabled" if init & 2 else "disabled",
              "enabled" if init & 1 else "disabled"), *issues)
    issues = []
    if discrete and thr_acc:
        issues.append("caps-thr-discrete")
    if discrete and hyst_acc:
        issues.append("caps-hyst-discrete")
    if evctl != 3:
        issues.append("caps-events")
    capf = d.add(11, 11, "Sensor capabilities",
          "ignore if entity absent %d; auto re-arm %s; hysteresis %s; thresholds %s; "
          "event control %s" % (
              caps >> 7, "yes" if caps & 0x40 else "no (manual)",
              ["none or built in", "readable", "readable and settable",
               "fixed, unreadable"][hyst_acc],
              ["none", "readable", "readable and settable", "fixed, unreadable"][thr_acc],
              ["per threshold/state", "entire sensor only", "global disable only",
               "no events"][evctl]), *issues)
    st = d.sensor_type
    sissues = []
    if (st not in SENSOR_TYPES and not 0xC0 <= st <= 0xEF) or st == 0xF6:
        sissues.append("type-reserved")
    if d.event_type not in EVENT_TYPES and not 0x70 <= d.event_type <= 0x7F:
        eissues = ["event-reserved"]
    elif d.event_type == 0:
        eissues = ["event-reserved"]
    else:
        eissues = []
    vita = VITA_SENSORS.get(d.sensor_number) if owner == SBC_ADDR else None
    if vita is not None and vita != (st, d.event_type):
        sissues.append("vita-type")
        eissues.append("vita-type")
    d.add(12, 12, "Sensor type", "%02Xh %s" % (st, "OEM reserved" if 0xC0 <= st <= 0xEF
                                            else name_of(SENSOR_TYPES, st)), *sissues)
    d.add(13, 13, "Event/reading type", "%02Xh %s%s" % (
        d.event_type, name_of(EVENT_TYPES, d.event_type) if not 0x70 <= d.event_type <= 0x7F
        else "OEM", "" if not vita else "; VITA Table 10.2-1 requires %02Xh/%02Xh" % vita),
        *eissues)
    decode_mask_pair(d, discrete, 14, "Assertion mask" if discrete else
                     "Lower threshold reading mask / assertion events", "assert")
    decode_mask_pair(d, discrete, 16, "Deassertion mask" if discrete else
                     "Upper threshold reading mask / deassertion events", "deassert")
    decode_mask_pair(d, discrete, 18, "Reading mask" if discrete else
                     "Settable / readable threshold mask", "reading")
    if not discrete:
        reading = d.masks.get("reading", 0)
        settable, readable = reading >> 8 & 0x3F, reading & 0x3F
        if (thr_acc != 2 and settable) or (thr_acc in (0, 3) and readable):
            capf.issues.append("caps-thr-mask")
            d.checks.append(("Sensor capabilities", "caps-thr-mask"))
    if full:
        decode_full_body(d)
    else:
        decode_compact_body(d)


def unit_text(u1, base, mod):
    rate = u1 >> 3 & 7
    mu = u1 >> 1 & 3
    text = name_of(UNITS, base) if base < len(UNITS) else "reserved (%02Xh)" % base
    if text is None:
        text = "reserved (%02Xh)" % base
    if mu == 1 and mod:
        text += " / " + (name_of(UNITS, mod) or "reserved")
    elif mu == 2 and mod:
        text += " * " + (name_of(UNITS, mod) or "reserved")
    if rate and rate in RATES:
        text += " " + RATES[rate]
    if u1 & 1:
        text += " (percent)"
    return text


def unit_issues(u1, base, mod):
    """Check ids for the three units fields: reserved codes, and a stray modifier code."""
    mode = u1 >> 1 & 3
    bad = (u1 >> 3 & 7) == 7 or mode == 3
    codes = [base] + ([mod] if mode in (1, 2) else [])
    for code in codes:
        if code and (code >= len(UNITS) or UNITS[code] is None):
            bad = True
    out = ["units-reserved"] if bad else []
    if mode == 0 and mod:
        out.append("units-modifier-stray")
    return out


def decode_compact_body(d):
    data = d.data
    u1, base, mod = data[20], data[21], data[22]
    d.add(20, 20, "Sensor units 1", "bits 7:6 = %d%d; rate %s; modifier %s; percentage %s" % (
        u1 >> 7, u1 >> 6 & 1, RATES.get(u1 >> 3 & 7, "reserved"),
        ["none", "base / modifier", "base * modifier", "reserved"][u1 >> 1 & 3],
        "yes" if u1 & 1 else "no"),
          "units-compact" if u1 >> 6 != 3 else None, *unit_issues(u1, base, mod))
    d.add(21, 21, "Base unit", "%02Xh %s" % (base, name_of(UNITS, base)))
    d.add(22, 22, "Modifier unit", "%02Xh %s" % (mod, name_of(UNITS, mod) if mod else "unused"))
    s1, s2 = data[23], data[24]
    d.share = s1 & 0xF
    d.add(23, 23, "Sensor direction / instance modifier type / share count",
          "direction %s; instance modifier %s; share count %d" % (
              ["unspecified", "input", "output", "reserved"][s1 >> 6],
              ["numeric", "alpha", "reserved", "reserved"][s1 >> 4 & 3], s1 & 0xF),
          "share-zero" if s1 & 0xF == 0 else None,
          "share-reserved" if s1 >> 6 == 3 or s1 >> 4 & 3 > 1 else None)
    d.add(24, 24, "Entity instance sharing / instance modifier offset",
          "entity instance %s; modifier offset %d" % (
              "increments for each shared record" if s2 & 0x80 else "same for all shared",
              s2 & 0x7F))
    d.add(25, 25, "Positive-going hysteresis", "%d counts" % data[25])
    d.add(26, 26, "Negative-going hysteresis", "%d counts" % data[26])
    d.add(27, 29, "Reserved", "must be 00h", "reserved-bytes" if any(data[27:30]) else None)
    d.add(30, 30, "OEM", "%02Xh" % data[30])
    d.unit = unit_text(u1, base, mod)
    idstring(d, 31, 32)


def to_raw(x, fmt):
    if fmt == 2:
        return signed(x, 8)
    if fmt == 1:
        return -(~x & 0xFF) if x & 0x80 else x
    return x


def convert(x, d):
    """y = L[(M*x + B*10^Bexp) * 10^Rexp] for a raw byte x of the record d."""
    r = d.raw_value
    if r["fmt"] == 3:
        return None
    xv = to_raw(x, r["fmt"])
    try:
        y = (r["m"] * xv + r["b"] * 10.0 ** r["bexp"]) * 10.0 ** r["rexp"]
        lin = r["lin"]
        funcs = {0: lambda v: v, 1: math.log, 2: math.log10, 3: math.log2, 4: math.exp,
                 5: lambda v: 10.0 ** v, 6: lambda v: 2.0 ** v, 7: lambda v: 1.0 / v,
                 8: lambda v: v * v, 9: lambda v: v ** 3, 10: math.sqrt,
                 11: lambda v: math.copysign(abs(v) ** (1.0 / 3.0), v)}
        return funcs[lin](y) if lin in funcs else None
    except (ValueError, ZeroDivisionError, OverflowError):
        return None


def decode_full_body(d):
    data = d.data
    u1, base, mod, lin = data[20], data[21], data[22], data[23]
    fmt = u1 >> 6
    thr_mask = le16(data, 18)
    has_thr = d.event_type == 0x01 and thr_mask & 0x3F3F
    issues = list(unit_issues(u1, base, mod))
    if fmt == 3 and not has_thr and u1 != 0:
        issues.append("units-full-none")
    d.add(20, 20, "Sensor units 1", "format %s; rate %s; modifier %s; percentage %s" % (
        ["unsigned", "1's complement", "2's complement", "no analog reading"][fmt],
        RATES.get(u1 >> 3 & 7, "reserved"),
        ["none", "base / modifier", "base * modifier", "reserved"][u1 >> 1 & 3],
        "yes" if u1 & 1 else "no"), *issues)
    d.add(21, 21, "Base unit", "%02Xh %s" % (base, name_of(UNITS, base)))
    d.add(22, 22, "Modifier unit", "%02Xh %s" % (mod, name_of(UNITS, mod) if mod else "unused"))
    lname = LINEARIZATION.get(lin & 0x7F) or (
        "non-linear" if lin & 0x7F == 0x70 else "OEM non-linear" if 0x71 <= lin & 0x7F <= 0x7F
        else "reserved (%02Xh)" % lin)
    d.add(23, 23, "Linearization", "%02Xh %s" % (lin, lname),
          "full-lin" if lname.startswith("reserved") or lin & 0x80 else None)
    m = signed(data[24] | (data[25] & 0xC0) << 2, 10)
    tol = data[25] & 0x3F
    b = signed(data[26] | (data[27] & 0xC0) << 2, 10)
    acc = (data[27] & 0x3F) | (data[28] & 0xF0) << 2
    accexp = data[28] >> 2 & 3
    direction = data[28] & 3
    rexp = signed(data[29] >> 4, 4)
    bexp = signed(data[29] & 0xF, 4)
    d.raw_value = {"fmt": fmt, "m": m, "b": b, "bexp": bexp, "rexp": rexp,
                   "lin": lin & 0x7F if lin & 0x7F <= 11 else 0x70}
    d.add(24, 25, "M and tolerance", "M = %d; tolerance +-%g raw counts" % (
        m, tol / 2.0), "full-m-zero" if m == 0 and fmt != 3 else None)
    d.add(26, 27, "B (accuracy low bits share byte 27)", "B = %d" % b)
    d.add(27, 28, "Accuracy", "accuracy %d x 10^%d hundredths of a percent" % (acc, accexp))
    d.add(28, 28, "Sensor direction", ["unspecified", "input", "output", "reserved"][direction],
          "full-dir" if direction == 3 else None)
    d.add(29, 29, "R exponent / B exponent", "Rexp = %d; Bexp = %d" % (rexp, bexp))
    flags = data[30]
    d.add(30, 30, "Analog characteristic flags", "nominal %s; normal max %s; normal min %s" % (
        "specified" if flags & 1 else "unspecified", "specified" if flags & 2 else "unspecified",
        "specified" if flags & 4 else "unspecified"),
          "full-flags-res" if flags & 0xF8 else None)
    names = ["Nominal reading", "Normal maximum", "Normal minimum", "Sensor maximum",
             "Sensor minimum", "Upper non-recoverable threshold", "Upper critical threshold",
             "Upper non-critical threshold", "Lower non-recoverable threshold",
             "Lower critical threshold", "Lower non-critical threshold"]
    unit = unit_text(u1, base, mod)
    d.unit = unit
    vals = {}
    for i, nm in enumerate(names):
        raw = data[31 + i]
        y = convert(raw, d)
        vals[nm] = (raw, y)
        extra = ""
        if nm in ("Nominal reading", "Normal maximum", "Normal minimum"):
            bit = {"Nominal reading": 1, "Normal maximum": 2, "Normal minimum": 4}[nm]
            extra = "" if flags & bit else " (not flagged as specified)"
        d.add(31 + i, 31 + i, nm, "raw %d (%02Xh) = %s %s%s" % (
            to_raw(raw, fmt) if fmt != 3 else raw, raw, fmt_val(y), unit, extra))
    # Value checks, on converted values when an analog reading exists.
    if fmt != 3:
        def conv(nm):
            return vals[nm][1]
        mx, mn = conv("Sensor maximum"), conv("Sensor minimum")
        if mx is not None and mn is not None and mx < mn:
            d.checks.append(("Sensor maximum", "full-minmax"))
            d.field("Sensor maximum").issues.append("full-minmax")
        flagged = [(nm, bit) for nm, bit in (("Nominal reading", 1), ("Normal maximum", 2),
                                             ("Normal minimum", 4)) if flags & bit]
        bad = False
        for nm, _ in flagged:
            y = conv(nm)
            if (y is not None and mx is not None and mn is not None and mx >= mn
                    and not mn <= y <= mx):
                bad = True
        if flags & 6 == 6 and conv("Normal maximum") < conv("Normal minimum"):
            bad = True
        if bad:
            d.checks.append(("Nominal reading", "full-nominal"))
            d.field("Nominal reading").issues.append("full-nominal")
        order = ["Lower non-recoverable threshold", "Lower critical threshold",
                 "Lower non-critical threshold", "Upper non-critical threshold",
                 "Upper critical threshold", "Upper non-recoverable threshold"]
        bit_of = {"Lower non-critical threshold": 0, "Lower critical threshold": 1,
                  "Lower non-recoverable threshold": 2, "Upper non-critical threshold": 3,
                  "Upper critical threshold": 4, "Upper non-recoverable threshold": 5}
        mask = d.masks.get("reading", 0)
        active = ((mask >> 8) | mask) & 0x3F
        shown = [nm for nm in order if active >> bit_of[nm] & 1]
        ys = [conv(nm) for nm in shown]
        if None not in ys and any(a > b for a, b in zip(ys, ys[1:])):
            d.checks.append((shown[0], "full-thr-order"))
            d.field(shown[0]).issues.append("full-thr-order")
        if mx is not None and mn is not None and mx >= mn:
            for nm in shown:
                y = conv(nm)
                if y is not None and not mn <= y <= mx:
                    d.checks.append((nm, "full-thr-range"))
                    d.field(nm).issues.append("full-thr-range")
        d.nominal_text = "%s %s" % (fmt_val(conv("Nominal reading")), unit)
        d.thr_text = ", ".join("%s %s" % (short_thr(nm), fmt_val(conv(nm))) for nm in order)
    d.add(42, 42, "Positive-going hysteresis", "raw %d%s" % (data[42], hyst_eng(d, data[42])))
    d.add(43, 43, "Negative-going hysteresis", "raw %d%s" % (data[43], hyst_eng(d, data[43])))
    d.add(44, 45, "Reserved", "must be 00h", "reserved-bytes" if any(data[44:46]) else None)
    d.add(46, 46, "OEM", "%02Xh" % data[46])
    idstring(d, 47, 48)


def short_thr(nm):
    return {"Lower non-recoverable threshold": "LNR", "Lower critical threshold": "LC",
            "Lower non-critical threshold": "LNC", "Upper non-critical threshold": "UNC",
            "Upper critical threshold": "UC", "Upper non-recoverable threshold": "UNR"}[nm]


def hyst_eng(d, counts):
    r = d.raw_value
    if not r or r["fmt"] == 3 or counts == 0:
        return ""
    # hysteresis counts convert with the slope only: counts * M * 10^Rexp
    return " (= %s %s)" % (fmt_val(counts * r["m"] * 10.0 ** r["rexp"]), d.unit)


def decode_locator(d):
    data = d.data
    if len(data) < 16:
        d.add(5, len(data) - 1, "record body", "shorter than the layout (16 bytes needed)")
        return
    d.add(5, 5, "Device slave address", "%02Xh: 7-bit address %02Xh (8-bit form %02Xh)" % (
        data[5], data[5] >> 1, data[5] & 0xFE), "loc-addr" if data[5] & 1 else None)
    d.add(6, 6, "Channel number", "channel %d" % (data[6] & 0xF),
          "loc-chan" if data[6] & 0xF0 else None)
    p = data[7]
    gi = p & 3
    d.add(7, 7, "Power state notification / global initialization",
          "ACPI system power notification %s; ACPI device power notification %s; %s "
          "controller; logs init agent errors %d; log init agent errors accessing this "
          "controller %d; event message generation %s" % (
              "required" if p & 0x80 else "not required",
              "required" if p & 0x40 else "not required",
              "static" if p & 0x20 else "dynamic", p >> 3 & 1, p >> 2 & 1,
              ["enabled", "disabled", "not initialized", "reserved"][gi]),
          "loc-power" if p & 0x10 or gi == 3 else None,
          "loc-init" if gi == 0 else None)
    c = data[8]
    caps = ["Chassis Device", "Bridge", "IPMB Event Generator", "IPMB Event Receiver",
            "FRU Inventory Device", "SEL Device", "SDR Repository Device", "Sensor Device"]
    d.add(8, 8, "Device capabilities", "; ".join(
        caps[i] for i in range(8) if c >> (7 - i) & 1) or "none",
          "loc-eventgen" if c & 0x20 else None)
    d.add(9, 11, "Reserved", "must be 00h", "reserved-bytes" if any(data[9:12]) else None)
    ent = data[12]
    d.add(12, 12, "Entity ID", "%02Xh %s" % (ent, ENTITIES.get(ent) or (
        "chassis-specific" if 0x90 <= ent <= 0xAF else "board-set specific"
        if 0xB0 <= ent <= 0xCF else "OEM system integrator" if ent >= 0xD0 else
        "reserved (%02Xh)" % ent)), "entity-reserved" if not entity_valid(ent) else None)
    inst = data[13]
    d.add(13, 13, "Entity instance", "%02Xh: %s-relative instance %d" % (
        inst, "device" if inst & 0x7F >= 0x60 else "system", inst & 0x7F))
    d.add(14, 14, "OEM", "%02Xh" % data[14])
    idstring(d, 15, 16)


# ----------------------------------------------------------------------------- this IPMC


def our_records():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
    try:
        out = subprocess.run([cargo, "run", "-q", "-p", "ipmc-emu", "--", "sdr"], cwd=ROOT,
                             env={**os.environ, "IPMC_TIER23": "0"},
                             capture_output=True, text=True, check=False)
    except OSError as exc:
        raise InputError("cannot run cargo: %s" % exc)
    if out.returncode != 0:
        raise InputError("ipmc-emu sdr failed (exit %d): %s" % (out.returncode, out.stderr))
    records = []
    for line in out.stdout.splitlines():
        parts = line.split()
        if len(parts) < 6:
            raise InputError("bad ipmc-emu sdr line: %r" % line)
        records.append((int(parts[0], 16), bytes(int(p, 16) for p in parts[1:])))
    return records


# ----------------------------------------------------------------------------- readings


def sensor_reading_text(ex, rec):
    """Decodes one Get Sensor Reading exchange against the sensor's record (or None)."""
    data = ex.rsp_data
    if ex.cc != 0 or len(data) < 2:
        return "completion code %02Xh" % ex.cc
    reading, status = data[0], data[1]
    st = "events %s, scanning %s, reading %s" % (
        "enabled" if status & 0x80 else "disabled", "enabled" if status & 0x40 else "disabled",
        "unavailable" if status & 0x20 else "valid")
    states = data[2:]
    parts = []
    if rec is not None and rec.event_type == 0x01 and rec.raw_value:
        y = convert(reading, rec)
        parts.append("reading %02Xh = %s %s" % (reading, fmt_val(y), rec.unit))
        if states:
            parts.append("threshold status %02Xh" % states[0])
    elif rec is not None:
        parts.append("reading byte %02Xh" % reading)
        if states:
            parts.append(discrete_text(rec.sensor_type, states[0], data))
    else:
        parts.append("reading byte %02Xh; state bytes %s" % (reading, hexs(states)))
    return "; ".join(parts + [st])


def discrete_text(stype, st, data):
    if stype == 0xF0:
        named = [VITA_FRU_STATES[b] for b in VITA_FRU_STATES if st >> b & 1]
        return "current state mask %02Xh = %s%s" % (st, ", ".join(named) or "none",
                                                    " (reserved bits set)" if st & 0x0C else "")
    if stype == 0xF1:
        a = "IPMB-A local status %d, override %s" % (
            data[0] & 7, "local control" if data[0] & 8 else "isolated")
        b = "IPMB-B local status %d, override %s" % (
            data[0] >> 4 & 7, "local control" if data[0] & 0x80 else "isolated")
        link = ["A disabled / B disabled", "A enabled / B disabled", "A disabled / B enabled",
                "A enabled / B enabled"]
        got = [link[i] for i in range(4) if st >> i & 1]
        speed = [n for bit, n in ((5, "IPMB-B 400 kHz"), (4, "IPMB-A 400 kHz")) if st >> bit & 1]
        return "%s; %s; link state %02Xh: %s" % (a, b, st, ", ".join(got + speed) or "none")
    if stype == 0xF3:
        names = ["<= lower non-critical", "<= lower critical", "<= lower non-recoverable",
                 ">= upper non-critical", ">= upper critical", ">= upper non-recoverable"]
        got = [names[i] for i in range(6) if st >> i & 1]
        return "temperature status %02Xh: %s" % (st, ", ".join(got) or "within limits")
    meaning = {
        0xF2: ("functioning properly", "not functioning properly"),
        0x02: ("all monitored voltages within range", "at least one voltage out of range"),
        0xF4: ("last test succeeded", "last test failed"),
        0xF5: ("test not in progress", "test in progress"),
    }.get(stype)
    if meaning:
        low = st & 3
        text = meaning[0] if low == 1 else meaning[1] if low == 2 else "state bits %d" % low
        return "state byte %02Xh: %s" % (st, text)
    return "state byte %02Xh" % st


# ----------------------------------------------------------------------------- document


def build_document(redact):
    cap = Capture()
    gaps = cap.gaps()
    records = {}  # record id -> Decoded
    order = []
    probe = {rid: decode_record(buf) for rid, (buf, _) in cap.assembled.items() if buf}
    ev = [e for r in probe.values() for e in r.evidence]
    mask_order = "be" if ev.count("be") and not ev.count("le") else "le"
    order_note = "%d masks are invalid read LS byte first and valid read MS byte first, %d "                  "the other way round" % (ev.count("be"), ev.count("le"))
    for rid in sorted(cap.assembled):
        buf, have = cap.assembled[rid]
        if not buf:
            continue
        rec = decode_record(buf, mask_order)
        rec.requested = rid
        rec.have = have
        records[rid] = rec
        order.append(rid)
    ours = [(rid, decode_record(b)) for rid, b in our_records()]
    for _, rec in ours:
        rec.requested = None

    out = []
    w = out.append
    redactions = [0]

    def safe_name(rec):
        text = rec.name_bytes.decode("latin-1")
        shown = redact(text)
        rec.name_redacted = shown != text
        rec.name_text = shown
        return shown

    for rid in order:
        safe_name(records[rid])
    for _, rec in ours:
        safe_name(rec)
    redactions[0] = sum(1 for r in records.values() if r.name_redacted)

    def meaning(rec, f):
        return rec.name_text if f.name == "ID string" else f.meaning

    def hexdump(rec):
        data = bytearray(rec.data)
        lines = []
        cells = ["%02X" % b for b in data]
        if getattr(rec, "name_redacted", False):
            s, e = rec.name_span
            for i in range(s, e + 1):
                cells[i] = "XX"
        for i in range(0, len(cells), 16):
            lines.append(" ".join(cells[i:i + 16]))
        return "\n".join(lines)

    nrec = len(order)
    by_type = {}
    for rid in order:
        by_type.setdefault(records[rid].record_type, []).append(rid)
    type_names = {1: "01h Full Sensor Record", 2: "02h Compact Sensor Record",
                  0x12: "12h Management Controller Device Locator Record"}

    w("# Captured SDR records")
    w("")
    w("This document records the %d Device SDR records that the captured SBC IPMC returned to "
      "the live Chassis Manager. The capture is a live VPX chassis: the Chassis Manager at IPMB "
      "address 10h (8-bit form) and an SBC IPMC at 82h in slot 1, on both IPMB buses, exported "
      "from a Saleae logic analyzer (`local_only/References/CodeReferences/ipmi-pic32/traces/sbca.csv` for IPMB-A, "
      "`sbcb.csv` for IPMB-B; `local_only/vectors/sbc_ab.vec` comes from the same capture). The records "
      "are real-world SDR examples next to the IPMI v2.0 and VITA 46.11-2022 tables." % nrec)
    w("")
    w("`python -I tools/captured_sdr.py` regenerates this file from the two CSV files and from "
      "`cargo run -q -p ipmc-emu -- sdr`; `--check` exits 1 when the file is stale. Strings that "
      "match the project's name list are replaced by `%s`; the hex dumps mask the bytes of such "
      "a string with `XX`. Offsets are 0-based record offsets (the IPMI table byte number is "
      "offset + 1)." % REDACTED)
    w("")

    # ---- section 1: summary of the repository
    w("## 1. Repository summary")
    w("")
    w("| Record type | Count | Record IDs |")
    w("|---|---|---|")
    for t in sorted(by_type):
        ids = by_type[t]
        w("| %s | %d | %s |" % (type_names.get(t, "%02Xh not decoded" % t), len(ids),
                                 summarize_ids(["%04X" % i for i in ids])))
    w("| total | %d | |" % nrec)
    w("")
    sensors = sum(max(r.share, 1) for r in records.values() if r.record_type in (1, 2))
    w("Get Device SDR Info (cmd 20h):")
    w("")
    for ex in cap.info:
        op = ex.req_data[0] if ex.req_data else None
        data = ex.rsp_data
        if ex.cc != 0 or len(data) < 2:
            w("- t = %.3f s, IPMB-%s, completion code %02Xh" % (ex.t_req, ex.bus_req, ex.cc))
            continue
        flags = data[1]
        w("- t = %.3f s, IPMB-%s: operation %s, result %d (%02Xh), flags %02Xh = %s population, "
          "LUNs with sensors: %s%s" % (
              ex.t_req, ex.bus_req,
              "none (sensor count)" if op is None else "%02Xh (%s)" % (
                  op, "SDR count" if op & 1 else "sensor count"),
              data[0], data[0], flags, "dynamic" if flags & 0x80 else "static",
              ", ".join(str(l) for l in range(4) if flags >> l & 1) or "none",
              "" if len(data) == 2 or not flags & 0x80 else "; change indicator %s" % hexs(
                  data[2:])))
    w("")
    w("The records hold %d sensors (%d sensor records; a share count of 0 counts as one sensor) "
      "and %d records in all." % (sensors, len(by_type.get(1, [])) + len(by_type.get(2, [])),
                                  nrec))
    w("")
    rsv = cap.reserve
    ids = [le16(e.rsp_data, 0) for e in rsv if e.cc == 0 and len(e.rsp_data) >= 2]
    real = [c for c in cap.chunks if not c.get("inferred")]
    res_sets = {}
    for c in real:
        res_sets.setdefault(c["rid"], set()).add(c["res"])
    single = sum(1 for v in res_sets.values() if len(v) == 1)
    w("Reserve Device SDR Repository (cmd 22h): %d requests, %d answered with a reservation ID "
      "(%s), each ID one more than the one before: %s. %d of %d records were read under a "
      "single reservation ID for all their chunks. %d reads with a non-zero offset carried a "
      "reservation ID other than the latest one; %d reads returned a non-zero completion "
      "code." % (
          len(rsv), len(ids), "%04Xh to %04Xh" % (ids[0], ids[-1]) if ids else "none",
          "yes" if all(b == a + 1 for a, b in zip(ids, ids[1:])) else "no",
          single, len(res_sets), len(cap.stale), len(cap.cc_errors)))
    w("")
    sizes = {}
    for c in real:
        if c["cc"] == 0:
            sizes[(c["off"], c["cnt"])] = sizes.get((c["off"], c["cnt"]), 0) + 1
    max_cnt = max((c["cnt"] for c in real), default=0)
    max_end = max((c["off"] + c["cnt"] for c in real), default=0)
    last_short = 0
    over = []
    for rid in order:
        total = len(records[rid].data)
        mine = [c for c in real if c["rid"] == rid and c["cc"] == 0]
        if not mine:
            continue
        last = max(mine, key=lambda c: c["off"])
        if last["cnt"] < 16 and last["off"] + last["cnt"] >= total:
            last_short += 1
        if total > max_end:
            over.append(rid)
    first_ids = [c["rid"] for c in real if c["off"] == 0]
    ascending = first_ids == sorted(first_ids)
    w("Get Device SDR (cmd 21h): %d reads were captured with their requests and %d response(s) "
      "were captured without their request (their offset is inferred from the read before; "
      "marked below). Each read asks for at most %d bytes (%02Xh). The Chassis Manager reads a "
      "record from offset 00h upward in steps of 16 and never requests a byte at or past "
      "offset %d (%02Xh); the final chunk of %d records asks for fewer than 16 bytes, exactly "
      "the bytes left in the record. Records are read in %s ID order starting at record 0000h "
      "and every read is answered with completion code 00h unless noted above." % (
          len(real), len(cap.inferred), max_cnt, max_cnt, max_end, max_end, last_short,
          "ascending" if ascending else "non-ascending"))
    w("")
    w("| Offset | Count requested | Reads |")
    w("|---|---|---|")
    for (off, cnt), n in sorted(sizes.items()):
        w("| %02Xh | %02Xh | %d |" % (off, cnt, n))
    w("")
    for c in cap.inferred:
        w("- Inferred read: record %04Xh offset %02Xh, %d bytes, response at t = %.3f s on "
          "IPMB-%s (request frame not in the capture)." % (
              c["rid"], c["off"], len(c["data"]), c["t"], c["bus"]))
    if cap.inferred:
        w("")
    hard = {}
    trailing = {}
    for rid, missing in gaps.items():
        rec = records.get(rid)
        end_name = rec.name_span[1] if rec and hasattr(rec, "name_span") else -1
        inside = [i for i in missing if i <= end_name or end_name < 0]
        after = [i for i in missing if i > end_name >= 0]
        if inside:
            hard[rid] = inside
        if after:
            trailing[rid] = after
    if hard:
        w("Gaps inside a record: %s." % "; ".join(
            "record %04Xh offsets %s" % (r, hexs(bytes(g)).replace(" ", ",")) for r, g in
            hard.items()))
    else:
        w("Reassembly found no byte of any field or ID string left unread: %d of %d records "
          "are complete." % (nrec - len(trailing), nrec))
    if trailing:
        w("")
        w("The record length byte of %d records reaches beyond the last byte the Chassis Manager "
          "requested: %s. These bytes follow the ID string and the specification marks them "
          "unspecified (IPMI v2.0 Table 43-1 byte 49+N); they are shown as 00h in the field "
          "tables below." % (len(trailing), "; ".join(
              "record %04Xh offsets %s" % (r, ", ".join(str(x) for x in g))
              for r, g in trailing.items())))
    w("")
    w("Overlapping reads that disagree: %d. Requests with no response in the capture: %d." % (
        cap.conflicts, cap.unanswered))
    w("")

    # ---- section 2: summary table
    w("## 2. Summary table")
    w("")
    w("Engineering values of full records come from each record's own formula "
      "y = L[(M x + B 10^Bexp) 10^Rexp]. U = upper, L = lower, NC = non-critical, C = critical, "
      "NR = non-recoverable.")
    w("")
    w("| ID | Type | Sensor # | Sensor type | Reading type | Name | Unit | Nominal | "
      "Thresholds | Notes |")
    w("|---|---|---|---|---|---|---|---|---|---|")
    for rid in order:
        r = records[rid]
        if r.record_type in (1, 2):
            notes = []
            if r.share > 1:
                notes.append("shared by %d sensors" % r.share)
            if r.checks:
                notes.append("%d check(s) failed" % len(r.checks))
            w("| %04X | %02Xh | %d | %02Xh %s | %02Xh %s | %s | %s | %s | %s | %s |" % (
                rid, r.record_type, r.sensor_number, r.sensor_type,
                md_cell(name_of(SENSOR_TYPES, r.sensor_type)), r.event_type,
                md_cell(name_of(EVENT_TYPES, r.event_type)), md_cell(r.name_text),
                md_cell(r.unit if r.record_type == 1 else ""),
                md_cell(r.nominal_text), md_cell(r.thr_text), "; ".join(notes)))
        else:
            notes = "%d check(s) failed" % len(r.checks) if r.checks else ""
            w("| %04X | %02Xh | - | - | - | %s | - | - | - | %s |" % (
                rid, r.record_type, md_cell(r.name_text), notes))
    w("")

    # ---- section 3: one section per record
    w("## 3. Records")
    w("")
    for rid in order:
        r = records[rid]
        w("### Record %04Xh: %s" % (rid, md_cell(r.name_text)))
        w("")
        w("| Offset | Field | Raw | Meaning |")
        w("|---|---|---|---|")
        for f in r.fields:
            off = "%d" % f.start if f.start == f.end else "%d-%d" % (f.start, f.end)
            flag = " **[check failed]**" if f.issues else ""
            raw = "".join([]) or r.raw(f.start, f.end)
            if f.name == "ID string" and r.name_redacted:
                raw = " ".join("XX" for _ in range(f.start, f.end + 1))
            w("| %s | %s | %s | %s%s |" % (off, md_cell(f.name), raw, md_cell(meaning(r, f)),
                                          flag))
        w("")
        w("```text")
        w(hexdump(r))
        w("```")
        w("")

    # ---- section 4: readings
    w("## 4. Readings seen")
    w("")
    w("Get Sensor Reading (cmd 2Dh) exchanges of the capture, decoded with the sensor's own "
      "record.")
    w("")
    by_sensor = {}
    for rid in order:
        r = records[rid]
        if r.record_type in (1, 2):
            for k in range(max(r.share, 1)):
                by_sensor[r.sensor_number + k] = r
    w("| Time (s) | Bus | Sensor | Name | Response | Decoded |")
    w("|---|---|---|---|---|---|")
    for ex in cap.readings:
        num = ex.req_data[0] if ex.req_data else None
        rec = by_sensor.get(num)
        w("| %.3f | %s | %s | %s | %s | %s |" % (
            ex.t_req, ex.bus_rsp, num, md_cell(rec.name_text) if rec else "no record",
            hexs(ex.rsp[IDX_DATA:-1]), md_cell(sensor_reading_text(ex, rec))))
    w("")
    if not cap.readings:
        w("The capture holds no Get Sensor Reading exchange.")
        w("")

    # ---- section 5: observations
    w("## 5. Observations against IPMI v2.0 and VITA 46.11-2022")
    w("")
    w("Each item comes from an explicit check in `tools/captured_sdr.py`. Records are listed "
      "by record ID.")
    w("")
    cite_no = {}
    obs = []

    def collect(recs, label_of):
        found = {}
        for rid, r in recs:
            for fname, cid in r.checks:
                found.setdefault(cid, {}).setdefault(fname, []).append(label_of(rid, r))
        return found

    found = collect([(rid, records[rid]) for rid in order], lambda rid, r: "%04X" % rid)
    repo = repository_checks(cap, records, order, sensors, nrec)
    n = 0
    for cid in CHECKS:
        if cid in found:
            n += 1
            cite_no[cid] = n
            text, cite = CHECKS[cid]
            affected = sorted({x for lst in found[cid].values() for x in lst})
            fields = sorted(found[cid])
            w("%d. %s%s Field: %s. Records: %s (%d). Rule: %s." % (
                n, text, " Evidence: %s." % order_note if cid == "mask-order" else "",
                ", ".join(fields), summarize_ids(affected), len(affected), cite))
    for text, cite in repo:
        n += 1
        w("%d. %s Rule: %s." % (n, text, cite))
    if n == 0:
        w("No check failed.")
    w("")
    clean = [k for k in CHECKS if k not in found]
    w("Checks that failed on no captured record (`CHECKS` table of the tool): %s. Repository "
      "checks (record chain, next-record IDs, record IDs, mandatory sensors 0-6 present, "
      "locator present, unique sensor numbers, reservation use) found nothing else." % (
          ", ".join("`%s`" % k for k in clean) or "none"))
    w("")

    # ---- section 6: comparison with this IPMC
    w("## 6. Comparison with this IPMC")
    w("")
    w("This IPMC's records come from `cargo run -q -p ipmc-emu -- sdr` and are decoded by the "
      "same decoder as the captured ones. Its record n (n = 0-6) is compared with captured "
      "record n, its locator (its last record, after the analog sensors' Full records, which have no captured counterpart) with captured record 0027h (39). Only fields whose "
      "bytes differ are listed. The verdict names the side that fails a spec check on that "
      "field (the observation numbers refer to section 5 for the captured side and to the list "
      "below the table for this IPMC); when neither side fails a check, the field is a design "
      "choice that the tables leave open.")
    w("")
    ours_found = collect([(i, r) for i, (_, r) in enumerate(ours)], lambda i, r: "%04X" % i)
    own_no = {}
    pairs = [(i, i) for i in range(7)] + [(len(ours) - 1, 39)]
    w("| This IPMC | Captured | Field | Captured raw | This IPMC raw | Verdict |")
    w("|---|---|---|---|---|---|")
    own_checks_used = {}
    for i, j in pairs:
        o = ours[i][1]
        c = records.get(j)
        if c is None:
            w("| %04X | %04X | - | record not in the capture | - | - |" % (i, j))
            continue
        cf = {f.name: f for f in c.fields}
        of = {f.name: f for f in o.fields}
        rows = []
        for f in o.fields:
            g = cf.get(f.name)
            if g is None:
                continue
            ro = o.raw(f.start, f.end)
            rc = c.raw(g.start, g.end)
            if f.value is not None and g.value is not None:
                ro += " (%04Xh)" % f.value
                rc += " (%04Xh)" % g.value
            if f.name in ("ID string",):
                ro = "(%s)" % o.name_text
                rc = "(%s)" % c.name_text
            elif f.name == "Record ID":
                continue
            if o.raw(f.start, f.end) == c.raw(g.start, g.end) and f.name != "ID string":
                continue
            if f.name == "ID string" and o.name_text == c.name_text:
                continue
            cs = [CHECKS[k][0] for k in g.issues]
            os_ = f.issues
            cn = ["captured obs %d" % cite_no[k] for k in g.issues if k in cite_no]
            on = []
            for k in os_:
                own_checks_used.setdefault(k, len(own_checks_used) + 1)
                on.append("this IPMC note %d" % own_checks_used[k])
            if g.issues and f.issues:
                verdict = "both fail (%s; %s)" % (", ".join(cn), ", ".join(on))
            elif g.issues:
                verdict = "this IPMC follows the spec; captured fails (%s)" % ", ".join(cn)
            elif f.issues:
                verdict = "captured follows the spec; this IPMC fails (%s)" % ", ".join(on)
            else:
                verdict = "design choice, neither fails a check"
            del cs
            rows.append((f.name, rc, ro, verdict))
        if not rows:
            w("| %04X | %04X | all fields | identical | identical | - |" % (i, j))
        for k, (nm, rc, ro, verdict) in enumerate(rows):
            w("| %s | %s | %s | %s | %s | %s |" % (
                "%04X" % i if k == 0 else "", "%04X" % j if k == 0 else "", md_cell(nm),
                md_cell(rc), md_cell(ro), md_cell(verdict)))
    w("")
    if not ours_found:
        w("No check of section 5 fails on any of this IPMC's records.")
        w("")
    if own_checks_used:
        w("Checks that fail on this IPMC's records:")
        w("")
        for k, num in sorted(own_checks_used.items(), key=lambda x: x[1]):
            w("%d. %s Rule: %s." % (num, CHECKS[k][0], CHECKS[k][1]))
        w("")
    other = {k: v for k, v in ours_found.items() if k not in own_checks_used}
    if other:
        w("Checks that fail on this IPMC's records in a field that equals the captured value: "
          "%s." % "; ".join("%s (%s)" % (CHECKS[k][0].rstrip("."), CHECKS[k][1])
                            for k in sorted(other)))
        w("")
    w("This IPMC has %d records (sensors 0-6, the analog sensors and the locator); the capture has %d records "
      "(sensors 0-%d and the locator)." % (len(ours), nrec, sensors - 1))
    w("")
    return "\n".join(out), cap, redactions[0], nrec, by_type, found, repo, ours_found


def summarize_ids(ids):
    """Record IDs (4-digit hex strings) as ranges: 0000-0007, 0026."""
    nums = sorted(int(x, 16) for x in ids)
    runs = []
    for v in nums:
        if runs and v == runs[-1][1] + 1:
            runs[-1][1] = v
        else:
            runs.append([v, v])
    return ", ".join("%04X" % a if a == b else "%04X-%04X" % (a, b) for a, b in runs)


def repository_checks(cap, records, order, sensors, nrec):
    """Observations about the repository as a whole: list of (text, citation)."""
    out = []
    for ex in cap.info:
        op = ex.req_data[0] if ex.req_data else None
        if ex.cc != 0 or not ex.rsp_data:
            continue
        got = ex.rsp_data[0]
        want = nrec if op is not None and op & 1 else sensors
        if got != want:
            out.append(("Get Device SDR Info %s returned %d but the repository holds %d %s "
                        "(IPMB-%s, t = %.3f s)." % (
                            "sensor count" if not op else "SDR count", got, want,
                            "sensors" if not op else "records", ex.bus_req, ex.t_req),
                        "IPMI v2.0 Table 35-2"))
    ids = [rid for rid in order]
    if ids != list(range(ids[0], ids[0] + len(ids))):
        out.append(("Record IDs are not consecutive.", "IPMI v2.0 section 33.3"))
    for k, rid in enumerate(ids):
        want = ids[k + 1] if k + 1 < len(ids) else NO_NEXT
        got = cap.next_ids.get(rid)
        if got is not None and got != want:
            out.append(("Record %04Xh returns next record ID %04Xh, expected %04Xh." % (
                rid, got, want), "IPMI v2.0 Table 35-3"))
    for rid in order:
        buf = records[rid].data
        if le16(buf, 0) != rid:
            out.append(("Record requested as %04Xh carries record ID %04Xh." % (
                rid, le16(buf, 0)), "IPMI v2.0 Table 43-1 bytes 1:2"))
    if not any(r.record_type == 0x12 for r in records.values()):
        out.append(("No Management Controller Device Locator Record.", "VITA 46.11-2022 "
                    "Rule 6.2-7"))
    present = {r.sensor_number for r in records.values() if r.record_type in (1, 2)}
    missing = [s for s in VITA_SENSORS if s not in present]
    if missing:
        out.append(("Mandatory sensors %s have no record." % missing, "VITA 46.11-2022 "
                    "Table 10.2-1"))
    seen = {}
    for rid in order:
        r = records[rid]
        if r.record_type in (1, 2):
            for k in range(max(r.share, 1)):
                seen.setdefault(r.sensor_number + k, []).append(rid)
    for num, rids in sorted(seen.items()):
        if len(rids) > 1:
            out.append(("Sensor number %d is used by records %s." % (
                num, ", ".join("%04X" % x for x in rids)), "IPMI v2.0 Table 43-1 byte 8"))
    if cap.stale:
        out.append(("%d partial reads used a reservation ID other than the latest one." % len(
            cap.stale), "IPMI v2.0 Table 35-3"))
    return out


def main(argv):
    check = "--check" in argv
    if [a for a in argv[1:] if a != "--check"]:
        print("usage: python -I tools/captured_sdr.py [--check]", file=sys.stderr)
        return 2
    try:
        redact = Redactor()
        text, cap, redacted, nrec, by_type, found, repo, ours_found = build_document(redact)
    except InputError as exc:
        print("error: %s" % exc, file=sys.stderr)
        return 2
    print("strings redacted: %d" % redacted, file=sys.stderr)
    text = text.rstrip("\n") + "\n"
    if check:
        have = DOC.read_text(encoding="utf-8") if DOC.is_file() else ""
        if have == text:
            return 0
        diff = list(difflib.unified_diff(have.splitlines(), text.splitlines(), "on disk",
                                         "generated", lineterm="", n=0))
        print("%s is stale (%d diff lines); run python -I tools/captured_sdr.py" % (
            DOC.relative_to(ROOT), len(diff)))
        for line in diff[:20]:
            print(line)
        return 1
    DOC.parent.mkdir(parents=True, exist_ok=True)
    DOC.write_text(text, encoding="utf-8", newline="\n")
    print("wrote %s: %d records (%s)" % (DOC.relative_to(ROOT), nrec, ", ".join(
        "%02Xh x%d" % (t, len(v)) for t, v in sorted(by_type.items()))))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
