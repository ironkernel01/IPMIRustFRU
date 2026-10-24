"""Raw broadcast, malformed-frame and duplicate-request probes."""
from run_conformance import NotTestable, check, test, unavailable


def request_frame(c, seq, destination=None):
    return c.adapter.frame(c.target.own_address if destination is None else destination,
                           6, 0, c.target.requester_address, seq, 0, 1, b"")


@test("BROADCAST-ADDRESSED", "Table 10.1.1-1; IPMI §20.9, Fig.20-1", "172; IPMI 280",
      "General-call prefix plus embedded own address gives exactly one correctly sourced Device ID response")
def test_broadcast_addressed(c):
    """VITA Table 10.1.1-1 p.172; IPMI §20.9 and Fig.20-1 p.280."""
    expected = c.ok(6, 1)
    seq = c.seq()
    # 00h PREFIXES an ordinary complete IPMB request. It does not replace rsSA
    # and is not included in either checksum (Fig.20-1).
    raw = c.raw_reply(b"\x00" + request_frame(c, seq), 6, 1, seq)
    check(raw[6] == 0 and raw[7:-1] == expected, "IPMI §20.9 p.280",
          f"broadcast differs from ordinary Device ID: {raw.hex(' ')}")
    check(raw[3] == c.target.own_address, "IPMI §20.9 p.280",
          f"broadcast response source {raw[3]:02x} differs from own address")


@test("BROADCAST-OTHER-ADDRESS", "IPMI §20.9, Fig.20-1", "IPMI 280",
      "Controller ignores a broadcast with an embedded address other than its own")
def test_broadcast_other(c):
    """IPMI §20.9 and Fig.20-1 p.280."""
    c.ok(6, 1)
    other = next(a for a in range(0x82, 0xC0, 2)
                 if a not in (c.target.own_address, c.target.requester_address))
    seq = c.seq()
    c.raw_reply(b"\x00" + request_frame(c, seq, other), 6, 1, seq, expect_reply=False)
    c.ok(6, 1)


def malformed_case(name, mutation):
    def body(c):
        c.ok(6, 1)
        seq = c.seq()
        raw = bytearray(request_frame(c, seq))
        if mutation == "header":
            raw[2] ^= 1
        elif mutation == "body":
            raw[-1] ^= 1
        elif mutation == "short":
            raw = raw[:5]
        elif mutation == "destination":
            other = next(a for a in range(0x82, 0xC0, 2)
                         if a not in (c.target.own_address, c.target.requester_address))
            raw = request_frame(c, seq, other)
        c.raw_reply(bytes(raw), 6, 1, seq, expect_reply=False)
        c.ok(6, 1)
    body.__name__ = "test_malformed_" + name.lower().replace("-", "_")
    body.__doc__ = "VITA Rule 2.1-1 p.21; IPMI message framing Fig.13-4 pp.163-164; §20.9 p.280."
    test("FRAME-" + name, "Rule 2.1-1; IPMI message framing", "21; IPMI 163-164; 280",
         "Ignore " + name.lower().replace("-", " ") + " frame and remain responsive")(body)


for name, mutation in (("BAD-HEADER-CHECKSUM", "header"), ("BAD-DATA-CHECKSUM", "body"),
                       ("SHORT", "short"), ("WRONG-DESTINATION", "destination")):
    malformed_case(name, mutation)


@test("DUPLICATE-READ", "IPMI Table 5-2, §5.3; Obs.4.1.1-6", "IPMI 70-72; 30",
      "Identical Device ID request and sequence receive an equivalent response or legal duplicate error")
def test_duplicate_read(c):
    """IPMI Table 5-2 p.70 and §5.3 pp.71-72; VITA Obs.4.1.1-6 p.30."""
    seq = c.seq()
    first = c.request(6, 1, seq=seq)
    c.expect(first)
    second = c.request(6, 1, seq=seq)
    c.expect(second, (0, 0xCF, 0xFF), "IPMI Table 5-2 p.70; §5.3 p.71")
    if second.cc == 0:
        check(second.data == first.data, "IPMI Table 5-2 p.70",
              f"duplicate read differs: {first.data.hex(' ')} / {second.data.hex(' ')}")


@test("DUPLICATE-WRITE-REPLY", "IPMI Table 5-2; Table 34-4; Obs.4.1.1-6", "IPMI 70; 478; 30",
      "Repeat an identical checksum-preserving FRU write with the same sequence; restore and verify image")
def test_duplicate_write(c):
    """IPMI Table 5-2 p.70, Table 34-4 p.478; VITA Obs.4.1.1-6 p.30."""
    original = c.writable_snapshot()
    check(sum(original[:8]) & 255 == 0, "Rule 8-2 p.137", "header checksum invalid; duplicate write withheld")
    request = b"\x00\x00\x00" + original[:8]
    try:
        seq = c.seq()
        first = c.request(0x0A, 0x12, request, seq=seq)
        c.expect(first, (0, 0x80))
        second = c.request(0x0A, 0x12, request, seq=seq)
        c.expect(second, (first.cc, 0xCF, 0xFF), "IPMI Table 5-2 p.70; §5.3 p.71")
        if first.cc == second.cc == 0:
            check(len(first.data) == 1 and first.data == second.data and 0 < first.data[0] <= 8,
                  "IPMI Table 34-4 p.478", f"duplicate write counts {first.data.hex(' ')} / {second.data.hex(' ')}")
        check(c.fru_read(0, 8) == original[:8], "IPMI Table 34-4 p.478", "duplicate write altered original bytes")
    finally:
        try:
            c.expect(c.request(0x0A, 0x12, request), (0, 0x80))
        finally:
            c.restore_fru(original)


@test("EVENT-NO-GENERATION", "Rule 7-4", "134",
      "Observe a bounded quiet capture window and reject any IPMC Platform Event request")
def test_no_events(c):
    """VITA Rule 7-4 p.134; §4.1.7 p.63."""
    import time
    c.ok(6, 1)
    c.drain()
    until = time.monotonic() + 1
    while time.monotonic() < until:
        raw = c.target.receive(timeout_ms=min(250, max(1, (until - time.monotonic()) * 1000)))
        if raw is None:
            continue
        c.trace.append("EVENT WINDOW " + raw.hex(" "))
        check(not (len(raw) >= 6 and raw[3] == c.target.own_address and raw[1] >> 2 == 4 and raw[5] == 2),
              "Rule 7-4 p.134", f"Tier-1 emitted event request {raw.hex(' ')}")


unavailable("DUPLICATE-WRITE-NO-REEXECUTION", "IPMI Table 5-2; Obs.4.1.1-6", "IPMI 70; 30",
            "Determine whether a repeated write was executed internally more than once",
            "Target exposes no write counter or non-idempotent safe write; equal FRU bytes cannot prove execution count. Supplied texts explicitly suppress duplicate event logging, not arbitrary FRU writes; CFh is permitted for duplicate replies")
unavailable("EVENT-NO-GENERATION-STIMULATED", "Rule 7-4", "134",
            "Absence of event generation through all sensor/state transitions",
            "Target cannot stimulate hardware sensor/power transitions; bounded idle capture and event-disabled reading flags are tested")
