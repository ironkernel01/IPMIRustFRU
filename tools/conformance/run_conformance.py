"""Independent bus conformance runner; Python 3.11+, standard library only.

Run with --transport emu or the physical transport named in TRANSPORT.md.
No adapter is imported until main(), so metadata can be inspected off-bench.
"""
from __future__ import annotations

import argparse
import importlib
import math
from pathlib import Path
import sys
import time
import traceback
from dataclasses import dataclass


TOPICS = ("commands", "sdr", "sensors", "fru", "link_buses", "timing",
          "broadcast", "nvmro")
TESTS = []


class NotTestable(Exception):
    """A bench capability or specification prerequisite is unavailable."""


class RestoreError(AssertionError):
    """Stop the run when original FRU bytes cannot be verified."""


@dataclass(frozen=True)
class Case:
    id: str
    rule: str
    page: str
    description: str
    function: object
    limitation: str = ""


def test(identifier, rule, page, description, *, limitation=""):
    def register(function):
        if any(t.id == identifier for t in TESTS):
            raise ValueError(f"Duplicate requirement ID: {identifier}")
        if not function.__doc__:
            raise ValueError(f"Missing cited docstring: {identifier}")
        TESTS.append(Case(identifier, rule, str(page), description, function, limitation))
        return function
    return register


def check(condition, rule, detail):
    if not condition:
        raise AssertionError(f"{rule}: {detail}")


def le16(value):
    return int(value).to_bytes(2, "little")


def u16(data):
    return int.from_bytes(data, "little")


def unavailable(identifier, rule, page, description, reason):
    def body(context):
        raise NotTestable(reason)
    body.__name__ = "test_" + identifier.lower().replace("-", "_").replace(".", "_")
    body.__doc__ = f"{rule}, p.{page}. {description}"
    test(identifier, rule, page, description, limitation=reason)(body)


class Context:
    def __init__(self, target, adapter):
        self.target, self.adapter = target, adapter
        self.trace = []
        self.timings = []
        self.sequence_times = [float("-inf")] * 64
        self.sequence_cursor = 0
        self.cache = {}
        self.current = "setup"

    def seq(self):
        # Avoid unintentional duplicates during T9, including rapid emulator runs.
        seq = self.sequence_cursor
        delay = self.sequence_times[seq] + 5.01 - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        self.sequence_times[seq] = time.monotonic()
        self.sequence_cursor = (seq + 1) % 64
        return seq

    def validate_frame(self, raw, netfn, cmd, seq, rs_lun=0, rq_lun=0):
        rule = "IPMI message fields; Fig.13-4 p.163; VITA Rule 2.1-1 p.21"
        check(isinstance(raw, bytes) and len(raw) >= 8, rule,
              f"response too short: {raw!r}")
        check(self.adapter.checksum(raw[:2]) == raw[2], rule,
              f"header checksum: {raw.hex(' ')}")
        check(self.adapter.checksum(raw[3:-1]) == raw[-1], rule,
              f"data checksum: {raw.hex(' ')}")
        expected = (self.target.requester_address, ((netfn + 1) << 2) | rq_lun,
                    self.target.own_address, (seq << 2) | rs_lun, cmd)
        actual = (raw[0], raw[1], raw[3], raw[4], raw[5])
        check(actual == expected, rule,
              f"address/NetFn/LUN/sequence/command {actual!r}, expected {expected!r}; {raw.hex(' ')}")
        return raw[6], raw[7:-1]

    def request(self, netfn, cmd, data=b"", *, bus="A", rs_lun=0, seq=None):
        seq = self.seq() if seq is None else seq
        self.trace.append(f"TX {bus} nf={netfn:02x} cmd={cmd:02x} lun={rs_lun} seq={seq} data={data.hex(' ')}")
        try:
            response = self.target.request(netfn, cmd, data, bus=bus, rs_lun=rs_lun,
                                           seq=seq, timeout_ms=250, retries=0)
        except self.adapter.NoResponse as error:
            self.timings.append((self.current, netfn, cmd, None))
            raise AssertionError(f"Rule 4.1.1-11 p.30/T5 p.31: no response within 250 ms; {error}") from error
        self.trace.append(f"RX {response.bus} {response.elapsed_ms:.3f} ms {response.frame.hex(' ')}")
        self.timings.append((self.current, netfn, cmd, response.elapsed_ms))
        cc, payload = self.validate_frame(response.frame, netfn, cmd, seq, rs_lun)
        check((cc, payload) == (response.cc, response.data), "TRANSPORT response consistency",
              f"decoded cc/data differs from frame {response.frame.hex(' ')}")
        check((response.netfn, response.seq, response.rq_lun, response.rs_lun) ==
              (netfn + 1, seq, 0, rs_lun), "IPMI message fields p.163",
              f"response metadata differs from request; {response.frame.hex(' ')}")
        check(response.bus in self.target.buses, "Rule 4.1.1-5 p.28",
              f"response on uncapturable bus {response.bus!r}")
        check(math.isfinite(response.elapsed_ms) and 0 <= response.elapsed_ms <= 227,
              "Rule 4.1.1-11 p.30; Table 4.1.1-1 T5 p.31",
              f"STOP-to-STOP response time {response.elapsed_ms} ms exceeds 227 ms")
        if netfn == 0x2C and data:
            check(payload[:1] == data[:1], "VITA §10.1.2 p.177; IPMI §5.3.1 p.71",
                  f"group identifier missing or not echoed: {payload.hex(' ')}")
        if cc == 0xC0 or (netfn == 0x0A and cmd in (0x11, 0x12) and cc == 0x81):
            raise NotTestable(f"Controller temporarily busy ({cc:02X}h); functional condition cannot be established")
        if cc in (0xD0, 0xD1, 0xD2):
            raise NotTestable(f"Controller initialization/repository/firmware update mode ({cc:02X}h)")
        return response

    def expect(self, response, codes=(0,), rule="IPMI §5.3 pp.71-72"):
        # Section 5.3 permits a device-specific unspecified error where the
        # completion code is not mandatory. Callers signal this with FFh.
        allowed = response.cc in codes or (0xFF in codes and 1 <= response.cc <= 0x7E)
        check(allowed, rule,
              f"completion {response.cc:02X}h, expected {[f'{n:02X}h' for n in codes]}; frame={response.frame.hex(' ')}")
        return response.data

    def ok(self, netfn, cmd, data=b"", **kwargs):
        return self.expect(self.request(netfn, cmd, data, **kwargs))

    def optional(self, cmd, data=b"\x03\x00"):
        response = self.request(0x2C, cmd, data)
        if response.cc == 0xC1:
            raise NotTestable("Optional command is unsupported (C1h), a compliant Tier-1 choice")
        return self.expect(response)

    def capabilities(self):
        data = self.ok(0x2C, 0x00, b"\x03")
        check(len(data) == 7, "Table 10.1.3.1-1 pp.183-184", f"capabilities length {len(data)}")
        return data

    def reserve(self):
        data = self.ok(4, 0x22)
        check(len(data) == 2 and u16(data) != 0, "IPMI Table 35-4 p.483; Rule 6.2-5 p.123",
              f"reservation must be two nonzero-ID bytes: {data.hex(' ')}")
        return u16(data)

    def sdr_part(self, reservation, record, offset, count):
        response = self.request(4, 0x21, le16(reservation) + le16(record) + bytes((offset, count)))
        if response.cc in (0x80, 0xC5):
            self.cache.pop("records", None)
            raise NotTestable("SDR reservation/population changed during read; run against a stable population")
        data = self.expect(response)
        check(len(data) == count + 2, "IPMI Table 35-3 p.483",
              f"record={record:04x} offset={offset} count={count}, response={data.hex(' ')}")
        return u16(data[:2]), data[2:]

    def records(self):
        if "records" in self.cache:
            return self.cache["records"]
        # Nonzero offsets always carry the current reservation. Never assume IDs
        # are consecutive or that a whole record fits in an IPMB response.
        reservation = self.reserve()
        records, selector, seen = [], 0, set()
        for _ in range(65535):
            next_id, header = self.sdr_part(reservation, selector, 0, 5)
            record_id, size = u16(header[:2]), 5 + header[4]
            check(record_id not in seen, "IPMI §33.12 p.471", f"cyclic/duplicate SDR ID {record_id:04x}")
            check(selector == 0 or selector == record_id, "IPMI Table 35-3 p.483",
                  f"selector {selector:04x} returned header ID {record_id:04x}")
            seen.add(record_id)
            full = bytearray(header)
            while len(full) < size:
                offset = len(full)
                check(offset <= 255, "IPMI Table 35-3 p.483", "record cannot be addressed with byte offset")
                count = min(16, size - offset)
                # The final request can cover record bytes beyond offset 255.
                if offset + count < size and offset + count > 255:
                    count = 255 - offset
                following, part = self.sdr_part(reservation, record_id, offset, count)
                check(following == next_id, "IPMI §35.3 pp.482-483", "next ID changed during reserved read")
                full.extend(part)
            records.append(bytes(full))
            if next_id == 0xFFFF:
                self.cache["records"] = records
                return records
            check(next_id not in seen, "IPMI §33.12 p.471", f"next-ID cycle at {next_id:04x}")
            selector = next_id
        raise AssertionError("IPMI §33.12 p.471: next-ID chain never terminates at FFFFh")

    def sensor_records(self):
        result = {}
        for record in self.records():
            if record[3] not in (1, 2, 3):
                continue
            check(len(record) >= 8, "IPMI §43 pp.551-565", "sensor record lacks key bytes")
            shared = max(1, record[23] & 15) if record[3] == 2 and len(record) > 23 else 1
            for number in range(record[7], record[7] + shared):
                check(number < 255, "IPMI Table 43-2 pp.558/562", "shared sensor number reaches reserved FFh")
                key = (record[5], record[6] & 3, number)
                check(key not in result, "Rules 6.2-8/6.2-11 p.124", f"duplicate sensor key {key}")
                result[key] = record
        return result

    def reading(self, number, **kwargs):
        data = self.ok(4, 0x2D, bytes((number,)), **kwargs)
        rule = f"Table 10.2-{1} p.236; IPMI Table 35-15 p.497"
        check(len(data) in (3, 4), rule, f"sensor {number} reading length {len(data)}: {data.hex(' ')}")
        check(not data[1] & 0x80, "Rule 7-4 p.134", f"sensor {number} has event generation enabled")
        if len(data) == 4:
            check(data[3] & 0x7F == 0, rule, f"sensor {number}: unexpected high state bits {data.hex(' ')}")
        return data

    def fru_info(self):
        data = self.ok(0x0A, 0x10, b"\x00")
        check(len(data) == 3, "IPMI Table 34-2 p.476", f"area info {data.hex(' ')}")
        size, unit = u16(data[:2]), 1 + (data[2] & 1)
        check(size >= 8, "Rule 8.1-2 pp.138-139", f"FRU size {size} cannot hold common header")
        return size, unit

    def fru_read(self, offset, count):
        if "fru_info" not in self.cache:
            self.cache["fru_info"] = self.fru_info()
        size, unit = self.cache["fru_info"]
        check(0 <= offset <= size and offset + count <= size, "suite FRU read bounds", f"{offset}+{count}>{size}")
        output = bytearray()
        while len(output) < count:
            position = offset + len(output)
            skip = position % unit
            requested = min(16, count - len(output) + skip)
            response = self.request(0x0A, 0x11, b"\x00" + le16(position // unit) + bytes((requested,)))
            data = self.expect(response)
            check(len(data) >= 2 and data[0] == len(data) - 1 and skip < data[0] <= requested,
                  "IPMI Table 34-3 p.477", f"read offset {position}, count {requested}: {data.hex(' ')}")
            output.extend(data[1 + skip:])
        return bytes(output)

    def fru_image(self, *, fresh=False):
        if fresh or "fru_image" not in self.cache:
            size, _ = self.fru_info()
            self.cache["fru_image"] = self.fru_read(0, size)
        return self.cache["fru_image"]

    def writable_snapshot(self):
        image = self.fru_image(fresh=True)
        reason = "Preexisting FRU format/checksum is invalid; write probe withheld (Rule 8-2 p.137; FRU §§8/11/12/16)"
        if len(image) < 8 or image[0] != 1 or sum(image[:8]) & 255:
            raise NotTestable(reason)
        for index in (3, 4):
            start = image[index] * 8
            if start < 8 or start + 2 > len(image):
                raise NotTestable(reason)
            end = start + image[start + 1] * 8
            if end <= start or end > len(image) or sum(image[start:end]) & 255:
                raise NotTestable(reason)
        if image[5]:
            position = image[5] * 8
            while True:
                h = image[position:position + 5]
                if len(h) != 5 or sum(h) & 255:
                    raise NotTestable(reason)
                data = image[position + 5:position + 5 + h[2]]
                if len(data) != h[2] or (sum(data) + h[3]) & 255:
                    raise NotTestable(reason)
                position += 5 + len(data)
                if h[1] & 0x80:
                    break
        return image

    def restore_fru(self, original):
        # All routine writes repeat original bytes. If a failed negative test
        # nevertheless changes storage, repair only differing aligned spans.
        try:
            current = self.fru_image(fresh=True)
            _, unit = self.fru_info()
            for offset in range(0, len(original), 16):
                old = original[offset:offset + 16]
                if current[offset:offset + len(old)] == old:
                    continue
                position = offset
                while position < offset + len(old):
                    remaining = original[position:offset + len(old)]
                    data = self.ok(0x0A, 0x12, b"\x00" + le16(position // unit) + remaining)
                    check(len(data) == 1 and 0 < data[0] <= len(remaining), "Rule 8.1-5 p.139",
                          f"restore count invalid: {data.hex(' ')}")
                    check(data[0] % unit == 0 or position + data[0] == len(original),
                          "IPMI Table 34-4 p.478", "partial word restoration cannot advance safely")
                    position += data[0]
            check(self.fru_image(fresh=True) == original, "FRU restoration", "original image differs after restore")
        except Exception as error:
            raise RestoreError(f"FRU restoration could not be verified: {error}") from error

    def drain(self):
        for _ in range(64):
            frame = self.target.receive(timeout_ms=1)
            if frame is None:
                return
            self.trace.append("DRAIN " + frame.hex(" "))
            check(not (len(frame) >= 6 and frame[3] == self.target.own_address and
                       frame[1] >> 2 == 4 and frame[5] == 2),
                  "Rule 7-4 p.134", f"Tier-1 generated event: {frame.hex(' ')}")
        raise NotTestable("Capture stream is continuously busy; cannot establish a quiet raw-frame test")

    def raw_reply(self, request, netfn, cmd, seq, *, bus="A", expect_reply=True):
        self.drain()
        self.trace.append("RAW TX " + bus + " " + request.hex(" "))
        start = time.monotonic()
        self.target.send_frame(bus, request)
        deadline, found = start + 0.250, None
        while time.monotonic() < deadline:
            raw = self.target.receive(timeout_ms=max(1, (deadline - time.monotonic()) * 1000))
            if raw is None:
                break
            self.trace.append("RAW RX " + raw.hex(" "))
            check(not (len(raw) >= 6 and raw[3] == self.target.own_address and raw[1] >> 2 == 4 and raw[5] == 2),
                  "Rule 7-4 p.134", "unsolicited Platform Event request observed")
            if len(raw) >= 6 and raw[3] == self.target.own_address and raw[4] >> 2 == seq:
                check(expect_reply, "IPMI message validation; Rule 2.1-1 p.21",
                      f"answered an invalid or differently addressed frame: {raw.hex(' ')}")
                self.validate_frame(raw, netfn, cmd, seq)
                check(found is None, "IPMI §20.9 p.280", f"multiple replies for broadcast selector: {raw.hex(' ')}")
                found = raw
        if expect_reply:
            check(found is not None, "IPMI §20.9 p.280", "no matching response within 250 ms")
        return found


def load_tests():
    if not TESTS:
        for topic in TOPICS:
            importlib.import_module("tests_" + topic)
    return TESTS


def markdown_report(results, context):
    def cell(value):
        return str(value).replace("|", "\\|").replace("\n", "<br>")
    lines = ["# Tier-1 bus conformance results", "",
             f"Transport: `{context.target.transport}`; buses: {context.target.buses}; fault injection: {context.target.can_fail_bus}.", "",
             "Each request uses retries=0. T5 is checked against adapter STOP-to-STOP timestamps.",
             "Raw send/receive has no STOP timestamps or receive-bus metadata; those limits are listed explicitly.", "",
             "| ID | Rule/table/section | Page | Result | Detail |", "|---|---|---|---|---|"]
    for case, status, detail, trace in results:
        lines.append("| " + " | ".join(cell(x) for x in (case.id, case.rule, case.page, status, detail)) + " |")
    for case, status, detail, trace in results:
        if trace and status != "PASS":
            lines.extend(["", f"## {case.id}: observed frames", "", "```text", *trace, "```"])
    totals = {s: sum(r[1] == s for r in results) for s in ("PASS", "FAIL", "NOT-TESTABLE")}
    lines.extend(["", "; ".join(f"{key}: {value}" for key, value in totals.items()), ""])
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--transport", required=True, help="Transport name from TRANSPORT.md")
    parser.add_argument("--port", type=int)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args(argv)
    if args.report:
        destination = args.report.resolve()
        root = Path(__file__).resolve().parent
        if not destination.is_relative_to(root):
            parser.error("--report must be inside tools/conformance/")
    cases = load_tests()
    adapter = importlib.import_module("adapter")
    target = adapter.connect(args.transport, args.port)
    context, results, stopped = Context(target, adapter), [], False
    for case in cases:
        context.current, context.trace = case.id, []
        try:
            if stopped:
                raise NotTestable("Run stopped after a FRU restoration failure; inspect and restore storage before continuing")
            case.function(context)
            status, detail = "PASS", case.description
        except NotTestable as error:
            status, detail = "NOT-TESTABLE", str(error)
        except RestoreError as error:
            status, detail, stopped = "FAIL", str(error), True
        except AssertionError as error:
            status, detail = "FAIL", str(error)
        except Exception as error:
            status, detail = "FAIL", f"{type(error).__name__}: {error}"
            context.trace.append(traceback.format_exc())
        results.append((case, status, detail, list(context.trace)))
        print(f"{status:12} {case.id}: {case.rule} p.{case.page}: {detail}", flush=True)
        if status == "FAIL":
            for line in context.trace:
                print("  " + line, flush=True)
    report = markdown_report(results, context)
    if args.report:
        destination.write_text(report, encoding="utf-8")
    counts = {s: sum(r[1] == s for r in results) for s in ("PASS", "FAIL", "NOT-TESTABLE")}
    print(f"Requirements: {len(cases)}; tests: {len(cases)}; " + "; ".join(f"{s}: {n}" for s, n in counts.items()))
    return 1 if counts["FAIL"] else 0


if __name__ == "__main__":
    sys.modules["run_conformance"] = sys.modules[__name__]
    raise SystemExit(main())
