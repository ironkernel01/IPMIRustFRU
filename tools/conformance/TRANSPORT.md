# Conformance suite transport interface

The Tier-1 conformance suite in this folder is written from the VITA 46.11-2022 and IPMI v2.0
specification texts only. It reaches the IPMC under test through the Python interface below and
nothing else, so the checks do not depend on how the IPMC is implemented. An adapter
(`adapter.py`, written separately) maps this interface onto the bench transports: an Aardvark on
IPMB-A (the board) or the x86 emulator over TCP (both buses, with bus fault injection).

All addresses are 8-bit IPMB slave addresses (BMC = 20h form). Times are milliseconds.

```python
class Response:
    cc: int              # completion code (byte after the command code)
    data: bytes          # response data after the completion code
    bus: str             # 'A' or 'B': the bus the response frame arrived on
    elapsed_ms: float    # request STOP to response STOP as measured by the adapter
    frame: bytes         # the whole response frame, rqSA first, both checksums included
    netfn: int           # response NetFn (request NetFn + 1)
    seq: int             # rqSeq echoed by the responder
    rq_lun: int
    rs_lun: int

class NoResponse(Exception):
    """No response within timeout_ms (after the adapter's own retries, if any)."""

class Target:
    own_address: int            # the IPMC's 8-bit IPMB address (82h on the bench)
    requester_address: int      # the address the adapter answers at (20h unless configured)
    buses: tuple[str, ...]      # buses the adapter can drive: ('A',) or ('A', 'B')
    can_fail_bus: bool          # True when bus_down/bus_up work (emulator only)
    transport: str              # 'aardvark' or 'emu'

    def request(self, netfn: int, cmd: int, data: bytes = b'', *, bus: str = 'A',
                rs_lun: int = 0, seq: int | None = None, timeout_ms: float = 250,
                retries: int = 0) -> Response:
        """One IPMB request to own_address. seq None = the adapter's next sequence number.
        retries > 0 repeats the identical frame (same seq) on the same bus after a timeout.
        Raises NoResponse."""

    def send_frame(self, bus: str, frame: bytes) -> None:
        """Writes a raw frame on `bus` exactly as given (first byte = 8-bit destination address,
        00h for a general-call broadcast). No checksum is added or checked. An address that no
        device acknowledges is not an error: the frame went out and no controller took it."""

    def receive(self, bus: str | None = None, timeout_ms: float = 250) -> bytes | None:
        """The next frame the IPMC transmitted (on `bus`, or either), or None."""

    def bus_down(self, bus: str) -> None:
        """Emulator only. Simulates a broken bus: frames sent to the IPMC on `bus` are lost, and
        every transmission the IPMC attempts on `bus` FAILS as seen by the IPMC (no
        acknowledgement), so a compliant IPMC resends on the other bus (Rule 4.1.1-7). Frames
        are never delivered on a down bus."""
    def bus_up(self, bus: str) -> None: ...

    def reset(self) -> None:
        """Returns the IPMC to its power-up state (emulator: new board; board: not available,
        raises NotImplementedError)."""
```

Helpers the suite may use (also provided by the adapter): `checksum(bytes) -> int` (IPMI
two's-complement zero checksum) and `frame(rs_sa, netfn, rs_lun, rq_sa, seq, rq_lun, cmd, data)
-> bytes` (builds a request frame with both checksums), so tests can build malformed frames on
purpose (bad checksum, wrong address, short frame) for `send_frame`.

Run: `python run_conformance.py --transport emu|aardvark [--port N] [--report out.md]`. Each test
returns PASS, FAIL (with the observed bytes) or NOT-TESTABLE (with the reason, e.g. "needs two
buses" on the Aardvark); the report lists every test with its rule or table number.
