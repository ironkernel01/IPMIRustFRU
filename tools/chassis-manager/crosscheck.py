"""crosscheck.py - compares a Chassis Manager run with the captured Chassis Manager.

Validation stage of PLAN decision 5. Inputs: `vectors/sbc_ab.vec` (the capture; local only, when it
is absent the tool prints "skipped" and exits 0) and the JSON summary of a run
(`chassis_manager.py --json`). It compares

  (a) the command sequence and mix: first-appearance order and request counts per command, and
  (b) the IPMC's responses to the same requests, where both runs hold the request.

Every difference is tagged `expected:<reason>` (the tables below: each entry names its reason) or
`unexpected`. Exit status 1 when any difference is unexpected, 0 otherwise.

One condition is read from our run: when our IPMC's Get VSO Capabilities reports one IPMB
interface (IPMB-B not fitted, PLAN decision 6), the differences that follow from it are expected
(`ONE_INTERFACE_*`); with two interfaces they stay unexpected.

No difference is excused wider than its reason:
 - a reason covers only the response bits it names (reserved bits are never excused), and where
   the reason implies the value of our bits, only those values;
 - a response may lack bytes only past the command's mandatory length (`LENGTH_VARIES`);
 - a request answered in one run and timed out in the other is unexpected, and so is any request
   of ours to the IPMC that timed out (absent slots are other addresses and are not compared);
 - a request-count difference is expected only when the count it explains is verified from the
   run data (`COUNT_CHECKS`), and bus alternation only when our transmissions actually alternate.

Frame layout (IPMI v2.0 IPMB message, first byte = 8-bit destination address):
  request  rsSA, netFn<<2|rsLUN, chk1, rqSA, seq<<2|rqLUN, cmd, data..., chk2
  response rqSA, netFn<<2|rqLUN, chk1, rsSA, seq<<2|rsLUN, cmd, cc, data..., chk2
"""

import argparse
import json
import sys
from pathlib import Path

from cm_commands import CC_OK, command_name, ipmb_interfaces
from cm_link import IPMB_DATA_INDEX, frame_payload

ROOT = Path(__file__).resolve().parents[2]
CAPTURE = ROOT / "vectors" / "sbc_ab.vec"
IPMC_ADDRESS = 0x82  # the captured IPMC and ours (strap 1)
IDX_NETFN, IDX_SEQ, IDX_CMD = 1, 4, 5
NETFN_SHIFT, SEQ_SHIFT = 2, 2
NETFN_RESPONSE_BIT = 1
CC_INVALID_COMMAND = 0xC1
SDR_NETFN, GET_SDR_CMD = 0x04, 0x21
RESERVATION_ID_BYTES = 2
GET_VSO_CAPABILITIES = (0x2C, 0x00)
SET_IPMB_STATE = (0x2C, 0x09)
GET_SENSOR_READING = (0x04, 0x2D)
ALL_BITS = 0xFF
LAST_INDEX = 99  # "to the end of the response" in an index range
STATE_1_BYTE = 3  # Get Sensor Reading response byte 4 (index 3): the first state byte
SDR_HEADER_BYTES = 5  # record ID (2), version, type, length
SDR_CHUNK_BYTES = 16  # both managers read a record in 16-byte chunks
CAPTURE_SDR_READ_MAX = 64  # the captured manager reads at most 64 bytes of a record
FRU_CHUNK_BYTES = 16

# ---- expected-difference tables: every entry is a reason, nothing is silently tolerated ----

# Commands only the capture sends: NA or optional for a Tier-1 IPMC. Our IPMC answers C1h to
# them, which is also the only completion-code difference these commands may show.
CAPTURE_ONLY_REASON = {
    (0x04, 0x00): "NA for a Tier-1 IPMC (Rule 7-4, Rule 4.2-2): Set Event Receiver; the 2015-era "
                  "manager sends it, our IPMC answers C1h",
    (0x04, 0x28): "NA for a Tier-1 IPMC (Rule 4.2-2): Set Sensor Event Enable; answered C1h",
    (0x2C, 0x1E): "optional for Tier-1 (Table 10.1.2-1): FRU Control Capabilities; answered C1h",
    (0x2C, 0x44): "optional for Tier-1 (Table 10.1.2-1): Get Mandatory Sensor Numbers; answered "
                  "C1h",
    (0x0A, 0x49): "optional (Table 10.1.1-1): Set SEL Time; answered C1h",
    (0x2C, 0x0C): "optional (Table 10.1.2-1): Set FRU Activation; answered C1h",
}
OURS_ONLY_REASON = {
    (0x06, 0x04): "mandatory for the IPMC (Table 10.1.1-1): Get Self Test Results; the captured "
                  "manager did not send it",
}
# Commands whose position in the sequence differs; they are left out of the order comparison.
ORDER_FREE_REASON = {
    SET_IPMB_STATE: "Set IPMB State: ours enables IPMB-B right after Get VSO Capabilities (Obs "
                    "4.1.1-2), the capture's manager did so after polling",
}

def sdr_requests(rows) -> list:
    """The Get Device SDR requests as (offset, count, response or None) from rows of (request
    data, response)."""
    return [(d[4], d[5], r) for d, r in rows if len(d) >= 6]


def theirs_sdr(capture: list) -> list:
    """Captured Get Device SDR rows: (request data, response)."""
    return [(e["data"], e["response"]) for e in capture
            if (e["netfn"], e["cmd"]) == (SDR_NETFN, GET_SDR_CMD)]


def theirs_records(capture: list) -> list:
    """Record body lengths of the captured SDR records (the length byte of each header read at
    offset 0), in capture order."""
    return [r[3 + SDR_HEADER_BYTES - 1] for off, _, r in sdr_requests(theirs_sdr(capture))
            if off == 0 and r is not None and len(r) > 3 + SDR_HEADER_BYTES - 1]


def our_ipmc(summary: dict) -> dict:
    """The run's record of the IPMC at IPMC_ADDRESS (empty when none was found)."""
    for info in summary.get("ipmcs", []):
        if info.get("address") == f"0x{IPMC_ADDRESS:02X}":
            return info
    return {}


def our_cancelled_reads(summary: dict) -> int:
    """Get Device SDR requests of ours answered C5h (reservation cancelled): each is one more
    request and, with its re-reservation, one more Reserve."""
    return sum(e["count"] for e in summary["exchanges"]
               if e["dest"] == IPMC_ADDRESS and (e["netfn"], e["cmd"]) == (SDR_NETFN, GET_SDR_CMD)
               and not e["timed_out"] and e["response"][:2].lower() == "c5")


def ceil_div(a: int, b: int) -> int:
    return -(-a // b)


def check_reserve(capture: list, summary: dict, theirs: int, ours: int) -> list:
    problems = []
    records = len(theirs_records(capture))
    if theirs != records:
        problems.append(f"captured Reserve count {theirs} is not one per captured record "
                        f"({records})")
    expected = 1 + our_cancelled_reads(summary)
    if ours != expected:
        problems.append(f"our Reserve count {ours} is not one plus the cancelled reservations "
                        f"({expected})")
    return problems


def capture_sdr_holes(capture: list) -> list:
    """(record ID, offset) of every 16-byte read the captured manager must have sent but the
    capture lacks: a read missing in front of a later read of the same record (the manager reads
    a record front to back), the capture having lost that frame."""
    holes, record, offsets = [], None, []

    def close():
        if offsets:
            for offset in range(0, max(offsets), SDR_CHUNK_BYTES):
                if offset not in offsets:
                    holes.append((record, offset))

    for data, _ in theirs_sdr(capture):
        if len(data) < 6:
            continue
        if data[4] == 0:
            close()
            record, offsets = data[2] | data[3] << 8, []
        offsets.append(data[4])
    close()
    return holes


def check_get_sdr(capture: list, summary: dict, theirs: int, ours: int) -> list:
    problems = []
    holes = capture_sdr_holes(capture)
    expected_theirs = sum(ceil_div(min(SDR_HEADER_BYTES + body, CAPTURE_SDR_READ_MAX),
                                   SDR_CHUNK_BYTES) for body in theirs_records(capture))
    expected_theirs -= len(holes)
    if theirs != expected_theirs:
        problems.append(f"captured Get Device SDR count {theirs} is not the {expected_theirs} "
                        f"16-byte chunks of its {len(theirs_records(capture))} records (at most "
                        f"{CAPTURE_SDR_READ_MAX} bytes each, {len(holes)} lost in the capture)")
    lengths = our_ipmc(summary).get("sdr_record_lengths")
    if lengths is None:
        problems.append("our run lists no SDR record lengths")
    else:
        expected_ours = (sum(1 + ceil_div(n - SDR_HEADER_BYTES, SDR_CHUNK_BYTES) for n in lengths)
                         + our_cancelled_reads(summary))
        if ours != expected_ours:
            problems.append(f"our Get Device SDR count {ours} is not the {expected_ours} requests "
                            f"of {len(lengths)} records (header read, then 16-byte chunks, plus "
                            f"cancelled reads)")
    return problems


def check_sdr_info(capture: list, summary: dict, theirs: int, ours: int) -> list:
    """Manager choice: ours reads the SDR info once per IPMC."""
    found = 1 if our_ipmc(summary) else 0
    return [] if ours == found else [f"our Get Device SDR Info count {ours} is not one per IPMC "
                                     f"found ({found})"]


def check_polling(capture: list, summary: dict, theirs: int, ours: int) -> list:
    """Ours: every sensor once per cycle, the cycles from the run parameters."""
    poll, run = summary.get("poll", {}), summary.get("run", {})
    problems = []
    cycles, sensors = poll.get("cycles"), poll.get("sensors")
    if cycles is None or sensors is None or "poll_s" not in run:
        return ["our run lists no poll cycles, sensors or interval"]
    if ours != cycles * sensors:
        problems.append(f"our reading count {ours} is not {cycles} cycles x {sensors} sensors")
    expected_cycles = ceil_div(round(run["duration_s_requested"] * 1000),
                               round(run["poll_s"] * 1000))
    if poll.get("late_cycles", 0) == 0 and cycles != expected_cycles:
        problems.append(f"{cycles} poll cycles, {expected_cycles} expected for "
                        f"{run['duration_s_requested']} s every {run['poll_s']} s")
    if cycles > expected_cycles:
        problems.append(f"{cycles} poll cycles, at most {expected_cycles} fit the run")
    return problems


def check_read_fru(capture: list, summary: dict, theirs: int, ours: int) -> list:
    problems = []
    size = our_ipmc(summary).get("fru_size")
    if size is None:
        problems.append("our run lists no FRU size")
    elif ours != ceil_div(size, FRU_CHUNK_BYTES):
        problems.append(f"our Read FRU Data count {ours} is not the {ceil_div(size, 16)} "
                        f"16-byte chunks of {size} bytes")
    areas = [e["response"] for e in capture if (e["netfn"], e["cmd"]) == (0x0A, 0x10)
             and e["response"] is not None and len(e["response"]) >= 3]
    if len(areas) != 1:
        problems.append(f"{len(areas)} captured FRU area infos")
    else:
        theirs_size = areas[0][1] | areas[0][2] << 8
        if theirs != ceil_div(theirs_size, FRU_CHUNK_BYTES):
            problems.append(f"captured Read FRU Data count {theirs} is not the "
                            f"{ceil_div(theirs_size, 16)} 16-byte chunks of {theirs_size} bytes")
    return problems


def capture_notes(key: tuple, capture: list) -> str:
    """Extra facts to print beside an expected count."""
    if key == (SDR_NETFN, GET_SDR_CMD):
        holes = capture_sdr_holes(capture)
        if holes:
            return ("; the capture lacks " + ", ".join(
                f"the read at offset {off} of record {rid:04X}h" for rid, off in holes)
                + " although a later read of the record follows (a frame the capture lost)")
    return ""


# Request counts that differ for commands both send: (reason, check). The reason is expected
# only when the check, run on the capture and our summary, finds no problem; each problem it
# returns makes the count unexpected.
COUNT_CHECKS = {
    (0x04, 0x22): ("the capture reserves the SDR repository before every record; ours once, "
                   "again only after a cancelled reservation", check_reserve),
    (0x04, 0x21): ("SDR size: our records vs the captured ones, read in 16-byte chunks", check_get_sdr),
    (0x04, 0x20): ("manager choice: the captured manager reads the SDR info twice, ours once",
                   check_sdr_info),
    GET_SENSOR_READING: ("polling: ours every sensor once per cycle for the run parameters; the "
                         "capture's interval and duration differ", check_polling),
    (0x0A, 0x11): ("FRU size: our image vs the captured one, read in 16-byte chunks",
                   check_read_fru),
}

# Response bytes that differ for a good reason. Key: (netFn, cmd), or (netFn, cmd, request data)
# for one request (Get Sensor Reading of one sensor); both apply to that request. Entry:
# (first index, last index, bits, reason[, allowed]), index 0 = completion code; the reason covers
# only the given bits of those bytes, and any other differing bit is unexpected. `allowed`, when
# given, is the set of values our bits (`ours & bits`) may take for the reason to hold; any other
# value is unexpected.
CONTENT_REASON = {
    (0x06, 0x01): [  # Get Device ID, IPMI v2.0 Table 20-2; our values are PLAN decision 1
        (1, 1, ALL_BITS, "our device ID (PLAN decision 1)"),
        (2, 2, 0x0F, "our device revision (PLAN decision 1)"),
        (3, 3, 0x7F, "our firmware revision (PLAN decision 1)"),
        (4, 4, ALL_BITS, "our firmware revision (PLAN decision 1)"),
        (6, 6, 0xF6, "additional device support: ours is a sensor and FRU inventory device; the "
                     "captured IPMC also claims SEL device and IPMB event receiver and "
                     "generator, NA for a Tier-1 IPMC (Rule 7-4)"),
        (7, 9, ALL_BITS, "our manufacturer ID (PLAN decision 1)"),
        (10, 11, ALL_BITS, "our product ID (PLAN decision 1)"),
        (12, 15, ALL_BITS, "auxiliary firmware revision: ours 0, the captured IPMC its own "
                           "(optional field, IPMI v2.0 Table 20-2)"),
    ],
    (0x0A, 0x10): [(1, 2, ALL_BITS, "our FRU image size")],
    (0x0A, 0x11): [(2, LAST_INDEX, ALL_BITS, "our FRU image content (the capture's is redacted)")],
    (0x04, 0x20): [(1, LAST_INDEX, ALL_BITS,
                    "our SDR repository (7 sensors, own flags) vs the captured 40")],
    (0x04, 0x21): [(1, LAST_INDEX, ALL_BITS, "our SDR records and record IDs")],
    (0x04, 0x22): [(1, 2, ALL_BITS, "reservation ID values")],
    GET_SENSOR_READING: [
        (2, 2, 0x80, "Tier-1 generates no events (Rule 7-4): ours reports event messages "
                     "disabled, the capture's IPMC had them enabled")],
    # State byte 1 (index 3) per sensor: only the bits the sensor's table defines are excused;
    # health, voltage, test results and test status define [1:0] = 01b / 10b (Tables 10.2.3-1,
    # 10.2.4-1, 10.2.6-1, 10.2.7-1), temperature bits [5:0] (Table 10.2.5-1).
    (*GET_SENSOR_READING, b"\x00"): [
        (STATE_1_BYTE, STATE_1_BYTE, 0xF3,
         "live FRU State of our board vs the captured board: ours activates (M1 to M4) within "
         "the first second after power-up and is polled while it does; one state bit set "
         "(Table 10.2.1-1: M0, M1, M4 to M7; bits 3:2 reserved)",
         frozenset({0x01, 0x02, 0x10, 0x20, 0x40, 0x80}))],
    (*GET_SENSOR_READING, b"\x02"): [
        (STATE_1_BYTE, STATE_1_BYTE, 0x03,
         "live FRU Health state of our board vs the captured board (Table 10.2.3-1)",
         frozenset({0x01, 0x02}))],
    (*GET_SENSOR_READING, b"\x03"): [
        (STATE_1_BYTE, STATE_1_BYTE, 0x03,
         "live FRU Voltage state of our board vs the captured board (Table 10.2.4-1)",
         frozenset({0x01, 0x02}))],
    (*GET_SENSOR_READING, b"\x04"): [
        (STATE_1_BYTE, STATE_1_BYTE, 0x3F,
         "live FRU Temperature state of our board vs the captured board (Table 10.2.5-1)")],
    (*GET_SENSOR_READING, b"\x05"): [
        (2, 2, 0x20, "Payload Test Results: our board has run no payload test, so the reading "
                     "is unavailable (Table 10.2.6-1)"),
        (STATE_1_BYTE, STATE_1_BYTE, 0x03,
         "Payload Test Results: no test has run on ours; the captured payload passed its last "
         "test (Table 10.2.6-1)", frozenset({0x00, 0x01, 0x02}))],
    (*GET_SENSOR_READING, b"\x06"): [
        (STATE_1_BYTE, STATE_1_BYTE, 0x03,
         "Payload Test Status: the captured payload had a test in progress, ours runs none "
         "(Table 10.2.7-1)", frozenset({0x01, 0x02}))],
    GET_VSO_CAPABILITIES: [
        (2, 2, 0x03, "IPMC tier bits [1:0]: ours is Tier-1 (00b), the captured IPMC reports "
                     "Tier-2; the layer and the reserved bits are not excused (section 10.1.3.1)",
         frozenset({0x00})),
        (5, 5, ALL_BITS, "VSO standard revision: ours 22h (2022), the captured IPMC 01h (2015)"),
        (6, 6, ALL_BITS, "max FRU ID: the captured IPMC manages a second FRU, ours only FRU 0")],
    (0x2C, 0x0D): [(2, 3, ALL_BITS, "record ID of our management controller locator SDR record")],
}
# Commands whose response length may differ: (mandatory length including the completion code,
# reason). Only bytes past the mandatory length may be missing; a shorter response of either run
# is unexpected.
LENGTH_VARIES = {
    (0x04, 0x21): (3, "Get Device SDR: completion code and next record ID (2) are mandatory; the "
                      "record bytes follow as the request asks and the record holds (IPMI v2.0 "
                      "Get Device SDR response)"),
    (0x04, 0x20): (3, "Get Device SDR Info: completion code, number of sensors and flags are "
                      "mandatory; the dynamic-population flag adds the 4 timestamp bytes (IPMI "
                      "v2.0 Get Device SDR Info response)"),
    GET_VSO_CAPABILITIES: (8, "Get VSO Capabilities: bytes 1-8 (completion code to FRU Device ID "
                              "of the IPMC) are mandatory; later bytes would be additions "
                              "(section 10.1.3.1)"),
    GET_SENSOR_READING: (4, "Get Sensor Reading: completion code, reading, status and state byte "
                            "1 are mandatory; the fifth byte (state byte 2) is optional (IPMI "
                            "v2.0 Get Sensor Reading response)"),
}

# The differences that follow from IPMB-B not fitted, expected only when our run's Get VSO
# Capabilities reports one IPMB interface.
ONE_INTERFACE = "IPMB-B not fitted: our IPMC reports one IPMB interface (PLAN decision 6)"
ONE_INTERFACE_CAPTURE_ONLY = {
    SET_IPMB_STATE: ONE_INTERFACE + "; no IPMB-B to enable, so no Set IPMB State (Rule 4.1.8-2)",
}
ONE_INTERFACE_CONTENT = {
    GET_VSO_CAPABILITIES: [(3, 3, 0x03, ONE_INTERFACE + ", the captured IPMC two")],
    (*GET_SENSOR_READING, b"\x01"): [
        (1, 1, 0xF0, ONE_INTERFACE + ": the IPMB-B half of the IPMB Link reading "
                     "(Table 10.2.2-1)"),
        (3, 3, 0x0F, ONE_INTERFACE + ": IPMB link state A enabled, B disabled "
                     "(Table 10.2.2-1)")],
}
ONE_INTERFACE_BUSES = (ONE_INTERFACE + ": every message on IPMB-A (Rule 4.1.8-2), the capture "
                       "alternated IPMB-A/B")
TWO_INTERFACES_BUSES = ("bus alternation: ours alternates IPMB-A/B after Set IPMB State (Rule "
                        "5.1.1-5), the capture answered on the arrival bus")


def transmission_buses(summary: dict) -> list | None:
    """Every transmission of our run as bus letters in order, or None when the summary has no
    transmissions."""
    tx = summary.get("transmissions")
    return None if tx is None else [list(t["buses"]) for t in tx]


def all_on_a(summary: dict) -> bool:
    """Every transmission of our run went out on IPMB-A."""
    tx = transmission_buses(summary)
    return bool(tx) and all(set(buses) <= {"A"} for buses in tx)


def alternates_after_set_ipmb_state(summary: dict) -> bool:
    """From the Set IPMB State that enabled IPMB-B on, consecutive transmissions of our run
    (retries included) alternate between IPMB-A and IPMB-B: every neighbouring pair differs and
    both buses appear (Rule 5.1.1-5)."""
    tx = transmission_buses(summary)
    events = [e for e in summary.get("bus_enable_events", []) if e["bus"] == "B"]
    if not tx or not events:
        return False
    start = max(0, min(e["after_exchanges"] for e in events) - 1)  # the Set IPMB State itself
    flat = [bus for buses in tx[start:] for bus in buses]
    return (len(flat) > 1 and set(flat) == {"A", "B"}
            and all(a != b for a, b in zip(flat, flat[1:])))


def read_capture(path: Path) -> list:
    """Captured exchanges to the IPMC: dicts with netfn, cmd, data, response, bus, t."""
    frames = []
    for line in path.read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split()
        frames.append((float(fields[0]), fields[1], bytes(int(x, 16) for x in fields[2:])))
    exchanges = []
    for index, (t, bus, frame) in enumerate(frames):
        if len(frame) < IPMB_DATA_INDEX + 1 or frame[IDX_NETFN] >> NETFN_SHIFT & NETFN_RESPONSE_BIT:
            continue
        if frame[0] != IPMC_ADDRESS:
            continue
        netfn = frame[IDX_NETFN] >> NETFN_SHIFT
        seq, cmd = frame[IDX_SEQ] >> SEQ_SHIFT, frame[IDX_CMD]
        response, rsp_bus = None, None
        for t2, bus2, other in frames[index + 1:index + 12]:
            if (len(other) > IPMB_DATA_INDEX and other[3] == IPMC_ADDRESS
                    and other[IDX_NETFN] >> NETFN_SHIFT == netfn | NETFN_RESPONSE_BIT
                    and other[IDX_SEQ] >> SEQ_SHIFT == seq and other[IDX_CMD] == cmd):
                response, rsp_bus = frame_payload(other), bus2
                break
        exchanges.append({"t": t, "bus": rsp_bus or bus, "netfn": netfn, "cmd": cmd,
                          "data": frame_payload(frame), "response": response})
    return exchanges


def first_order(items) -> list:
    """Distinct (netfn, cmd) in order of first appearance."""
    seen = []
    for key in items:
        if key not in seen:
            seen.append(key)
    return seen


def request_key(netfn: int, cmd: int, data: bytes) -> tuple:
    """Identity of a request for the response comparison; the SDR reservation ID is a value of
    the session, not of the request."""
    if (netfn, cmd) == (SDR_NETFN, GET_SDR_CMD):
        return netfn, cmd, bytes(data[RESERVATION_ID_BYTES:])
    return netfn, cmd, bytes(data)


def one_ipmb_interface(summary: dict) -> bool:
    """Our run's Get VSO Capabilities to the IPMC answered 00h and reports one IPMB interface."""
    for e in summary["exchanges"]:
        if (e["dest"] == IPMC_ADDRESS and not e["broadcast"] and not e["timed_out"]
                and (e["netfn"], e["cmd"]) == GET_VSO_CAPABILITIES):
            response = bytes.fromhex(e["response"])
            return response[:1] == bytes([CC_OK]) and ipmb_interfaces(response) == 1
    return False


def content_entries(key: tuple, one_interface: bool) -> list:
    """The CONTENT_REASON entries (and, with one interface, ONE_INTERFACE_CONTENT entries) of the
    request `key` = (netfn, cmd, data): those of its command, then those of the request itself."""
    tables = [CONTENT_REASON] + ([ONE_INTERFACE_CONTENT] if one_interface else [])
    return [entry for table in tables for k in (key[:2], key) for entry in table.get(k, [])]


def byte_reasons(entries: list, index: int, diff: int, ours_byte: int) -> tuple:
    """(reasons, uncovered bits) for the differing bits `diff` of response byte `index`, our
    value `ours_byte`. An entry with an `allowed` set excuses its bits only when our bits take
    one of those values."""
    reasons = []
    for low, high, bits, reason, *rest in entries:
        if not low <= index <= high or not diff & bits:
            continue
        if rest and ours_byte & bits not in rest[0]:
            continue
        reasons.append(reason)
        diff &= ~bits
    return reasons, diff


def compare_response(key: tuple, ours: bytes, theirs: bytes, one_interface: bool) -> list:
    """Differences between two responses (completion code first) as (reason or None, text); a
    reason is the entry of an expected table, None marks an unexpected difference."""
    netfn, cmd = key[0], key[1]
    if ours == theirs:
        return []
    if ours[:1] != theirs[:1]:
        na = ((netfn, cmd) in CAPTURE_ONLY_REASON and ours[:1] == bytes([CC_INVALID_COMMAND]))
        return [(CAPTURE_ONLY_REASON[(netfn, cmd)] if na else None,
                 f"completion code ours {ours[:1].hex().upper()} theirs "
                 f"{theirs[:1].hex().upper()}")]
    out = []
    entries = content_entries(key, one_interface)
    if len(ours) != len(theirs):
        if (netfn, cmd) not in LENGTH_VARIES:
            out.append((None, f"length ours {len(ours)} theirs {len(theirs)}"))
        else:
            mandatory, reason = LENGTH_VARIES[(netfn, cmd)]
            short = [who for who, rsp in (("ours", ours), ("theirs", theirs))
                     if len(rsp) < mandatory]
            if short:
                out.append((None, f"length ours {len(ours)} theirs {len(theirs)}: "
                                  f"{' and '.join(short)} below the mandatory {mandatory} bytes"))
            else:
                out.append((reason, f"length ours {len(ours)} theirs {len(theirs)}"))
    for index in range(min(len(ours), len(theirs))):
        if ours[index] == theirs[index]:
            continue
        text = f"byte {index} ours {ours[index]:02X} theirs {theirs[index]:02X}"
        reasons, uncovered = byte_reasons(entries, index, ours[index] ^ theirs[index],
                                          ours[index])
        out.extend((reason, text) for reason in reasons)
        if uncovered:
            out.append((None, f"{text} (bits {uncovered:02X}h)"))
    return out


def crosscheck(capture: list, summary: dict) -> tuple:
    """(report lines, unexpected count)."""
    lines, unexpected = [], 0
    ours = [e for e in summary["exchanges"]
            if e["dest"] == IPMC_ADDRESS and not e["broadcast"]]
    ours_broadcast = [e for e in summary["exchanges"] if e["broadcast"]]
    one_interface = one_ipmb_interface(summary)
    capture_only = {**CAPTURE_ONLY_REASON,
                    **(ONE_INTERFACE_CAPTURE_ONLY if one_interface else {})}

    def tag(kind_reason, text):
        nonlocal unexpected
        if kind_reason is None:
            unexpected += 1
            lines.append(f"  unexpected  {text}")
        else:
            lines.append(f"  expected:{kind_reason}  {text}")

    lines.append("our IPMC reports " + ("one IPMB interface (IPMB-B not fitted)" if one_interface
                                        else "two IPMB interfaces (or none read)"))
    their_count, our_count = {}, {}
    for e in capture:
        k = (e["netfn"], e["cmd"])
        their_count[k] = their_count.get(k, 0) + 1
    for e in ours:
        k = (e["netfn"], e["cmd"])
        our_count[k] = our_count.get(k, 0) + e["count"]
    lines.append("(a) command mix: captured vs ours (requests to the IPMC at 82h)")
    for k in first_order([(e["netfn"], e["cmd"]) for e in capture] + list(our_count)):
        c, o = their_count.get(k, 0), our_count.get(k, 0)
        row = f"{command_name(*k):<32} captured {c:>4}  ours {o:>4}"
        if c == o:
            lines.append(f"  same        {row}")
        elif o == 0:
            tag(capture_only.get(k), f"{row}  (only the capture sends it)")
        elif c == 0:
            tag(OURS_ONLY_REASON.get(k), f"{row}  (only ours sends it)")
        else:
            entry = COUNT_CHECKS.get(k)
            if entry is None:
                tag(None, row + "  (no explanation for the count)")
                continue
            reason, check = entry
            problems = check(capture, summary, c, o)
            if problems:
                tag(None, row + "  (" + "; ".join(problems) + ")")
            else:
                tag(reason + capture_notes(k, capture), row)
    if ours_broadcast:
        lines.append(f"  expected:ours probes the broadcast form of Get Device ID "
                     f"(Table 10.1.1-1)  Broadcast Get Device ID  ours "
                     f"{sum(e['count'] for e in ours_broadcast)}")

    theirs_order = first_order([(e["netfn"], e["cmd"]) for e in capture])
    ours_order = first_order([(e["netfn"], e["cmd"])
                              for e in sorted(ours, key=lambda e: e["first_t_s"])])
    common = [k for k in theirs_order if k in ours_order]
    in_our_order = [k for k in ours_order if k in common]
    lines.append("(a) order of first appearance, commands both send")
    names = " > ".join(command_name(*k) for k in common)
    if common == in_our_order:
        lines.append("  same        " + names)
    else:
        free = [k for k in common if k in ORDER_FREE_REASON]
        rest_theirs = [k for k in common if k not in ORDER_FREE_REASON]
        rest_ours = [k for k in in_our_order if k not in ORDER_FREE_REASON]
        if rest_theirs == rest_ours:
            for k in free:
                lines.append(f"  expected:{ORDER_FREE_REASON[k]}")
            lines.append(f"              otherwise the same: {names}")
        else:
            tag(None, "theirs " + names + " | ours "
                + " > ".join(command_name(*k) for k in in_our_order))

    lines.append("(a) bus of the responses")
    their_buses, our_buses = {}, {}
    for e in capture:
        their_buses[e["bus"]] = their_buses.get(e["bus"], 0) + 1
    for c in summary["commands"].values():
        for bus, n in c["buses"].items():
            our_buses[bus] = our_buses.get(bus, 0) + n
    row = f"captured {dict(sorted(their_buses.items()))} ours {dict(sorted(our_buses.items()))}"
    if their_buses == our_buses:
        lines.append(f"  same        {row}")
    elif one_interface:
        tag(ONE_INTERFACE_BUSES if set(our_buses) == {"A"} and all_on_a(summary) else None,
            row + ("" if all_on_a(summary) else "  (a transmission went out on IPMB-B)"))
    else:
        alternating = alternates_after_set_ipmb_state(summary)
        tag(TWO_INTERFACES_BUSES if set(our_buses) == {"A", "B"} and alternating else None,
            row + ("" if alternating else "  (our transmissions do not alternate IPMB-A/B after "
                                          "Set IPMB State)"))

    lines.append("(b) responses to the same requests")
    theirs_by_key = {}
    for e in capture:
        if e["response"] is not None:
            theirs_by_key.setdefault(request_key(e["netfn"], e["cmd"], e["data"]), e["response"])
    theirs_timed_out = {request_key(e["netfn"], e["cmd"], e["data"]) for e in capture
                        if e["response"] is None}
    compared = identical = 0
    groups: dict = {}  # (command name, reason) -> [requests, [texts]]
    for e in sorted(ours, key=lambda e: e["first_t_s"]):
        key = request_key(e["netfn"], e["cmd"], bytes.fromhex(e["request"]))
        if e["timed_out"]:
            # No response of ours to compare. A timeout of a request to the IPMC has no excuse:
            # the capture answered it, or never sent it.
            how = ("the capture answered it" if key in theirs_by_key
                   else "the capture never sent it" if key not in theirs_timed_out
                   else "the capture timed out too")
            tag(None, f"{command_name(key[0], key[1])} {key[2].hex(' ')}: request timed out in "
                      f"ours; {how}")
            continue
        if key in theirs_timed_out and key not in theirs_by_key:
            tag(None, f"{command_name(key[0], key[1])} {key[2].hex(' ')}: answered in ours, "
                      f"timed out in the capture")
            continue
        if key not in theirs_by_key:
            continue
        compared += 1
        diffs = compare_response(key, bytes.fromhex(e["response"]), theirs_by_key[key],
                                 one_interface)
        if not diffs:
            identical += 1
        for reason, text in diffs:
            name = command_name(key[0], key[1])
            if reason is None:
                unexpected += 1
                lines.append(f"  unexpected  {name} {key[2].hex(' ')}: {text}")
            else:
                entry = groups.setdefault((name, reason), [set(), set()])
                entry[0].add(key[2])
                entry[1].add(text.split(" ours")[0])
    for (name, reason), (requests, what) in groups.items():
        lines.append(f"  expected:{reason}  {name}: {len(requests)} request(s), "
                     f"{', '.join(sorted(what, key=lambda w: (len(w), w)))}")
    lines.append(f"  {compared} request(s) in both runs, {identical} identical response(s)")
    lines.append(f"RESULT: {unexpected} unexpected difference(s)")
    return lines, unexpected


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("summary", help="JSON summary of a chassis_manager.py run")
    parser.add_argument("--vec", default=str(CAPTURE), help="the capture (default vectors/sbc_ab.vec)")
    args = parser.parse_args(argv)
    if not Path(args.vec).exists():
        print(f"skipped: {args.vec} is absent (the capture is local only)")
        return 0
    summary = json.loads(Path(args.summary).read_text(encoding="utf-8"))
    lines, unexpected = crosscheck(read_capture(Path(args.vec)), summary)
    print("\n".join(lines))
    return 1 if unexpected else 0


if __name__ == "__main__":
    sys.exit(main())
