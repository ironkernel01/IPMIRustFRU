"""Response timing and limits of requester-side timing observations."""
from run_conformance import check, test, unavailable


@test("TIMING-T5", "Rule 4.1.1-11; Table 4.1.1-1 T5", "30-31",
      "227 ms STOP-to-STOP deadline per request, with no adapter retries masking slow responses")
def test_t5(c):
    """VITA Rule 4.1.1-11 p.30; Table 4.1.1-1 T5 p.31."""
    # Context.request enforces T5 for ALL requests, including command errors,
    # FRU reads, partial SDR reads, state changes and restoration requests.
    for nf, cmd, data in ((6, 1, b""), (6, 4, b""), (4, 0x20, b""),
                          (4, 0x22, b""), (4, 0x2D, b"\x01"),
                          (0x0A, 0x10, b"\x00"),
                          (0x0A, 0x11, b"\x00\x00\x00\x08"),
                          (0x2C, 0, b"\x03"), (0x2C, 9, b"\x03\xff\xff"),
                          (0x2C, 0x0D, b"\x03\x00"), (0x2C, 0x40, b"\x03")):
        c.expect(c.request(nf, cmd, data))
    for identifier, nf, cmd, elapsed in c.timings:
        check(elapsed is not None and 0 <= elapsed <= 227,
              "Rule 4.1.1-11 p.30; Table 4.1.1-1 p.31",
              f"{identifier} NetFn {nf:02x} cmd {cmd:02x}: {elapsed!r} ms")


unavailable("TIMING-T5-RAW", "Rule 4.1.1-11; Table 4.1.1-1 T5", "30-31",
            "T5 for raw broadcast and duplicate/malformed frame probes",
            "send_frame/receive return neither request STOP nor response STOP timestamps; host elapsed time includes I/O latency and is not a T5 measurement")
unavailable("TIMING-T1-T2-T7-T8-T10", "Table 4.1.1-1", "31",
            "Message duration, bus-free timeout, byte duration, clock-low hold and request lockout",
            "Target has no electrical edge timestamps, arbitration/bus-free instrumentation or processing lockout observation")
unavailable("TIMING-OUTBOUND-RETRY", "Rules 4.1.1-8/-9/-10; Table 4.1.1-1 T3/T6/C1", "29-31",
            "IPMC-originated requests: response timeout, retry interval/count and retained sequence",
            "Target cannot trigger arbitrary IPMC-originated requests; Tier-1 is forbidden to generate events; adapter retries would measure the requester instead")
unavailable("TIMING-T9-EXPIRATION", "Table 4.1.1-1 T9; Obs.4.1.1-6", "30-31",
            "IPMC sequence expiration exactly five seconds",
            "No observable non-idempotent write or request-cache introspection; identical replies cannot establish cache expiration; runner avoids incidental reuse within five seconds")

