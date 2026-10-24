"""cm_link.py - the Chassis Manager's IPMB request policy (ANSI/VITA 46.11-2022 section 4.1.1/5.1.1).

`Link.request` sends one request and applies the spec's rules for the requester:
 - 60..250 ms response timeout (T3 min..max, Rule 4.1.1-9);
 - a timed-out request is retried up to C1 times (3..10, Rule 4.1.1-10), every try with the same
   sequence number (Obs 4.1.1-6), on the other bus when both buses are enabled (Rule 5.1.1-6);
 - messages alternate between the enabled buses (Rule 5.1.1-5), the first on IPMB-A; the
   alternation follows the bus of the last transmission, retries included;
 - IPMB-A only until the target's IPMB-B support is known (Rule 4.1.8-2);
 - a sequence number is not used again for the same target and command within T9 (5 s) of its
   last transmission, so a compliant IPMC never takes a new request for a retry of an old one
   (its duplicate cache would answer it with the old response); the request waits instead.
IPMB-B is per target: it is used for a target only after that target reported two interfaces and
its Set IPMB State succeeded (`enable_bus(bus, dest)`). A bus that times out for a request which
the other bus then answered, `bus_fail_streak` times in a row, is added to the failed set of that
target and no longer chosen for its first try; the caller tells that target to use the other bus
(section 9.3: the Chassis Manager directs traffic to the other bus).

Frames are built and checked with python-ipmi (`IpmbHeaderReq`, `encode_ipmb_msg`, `rx_filter`);
the transports are python-ipmi `IpmbInterface` objects, one per bus. Tool-chosen values are in
`cm_settings.py`.
"""

import math
import queue
import threading
import time
from dataclasses import dataclass, field

from cm_settings import BUS_FAIL_STREAK_DEFAULT, SERVICE_PERIOD_S
from pyipmi.interfaces.ipmb import (
    IpmbHeaderReq, IpmbHeaderRsp, checksum, encode_ipmb_msg, rx_filter,
)

# Table 4.1.1-1 (section 4.1.1 p.31) and the rules next to each value.
T3_MIN_S = 0.060  # response time-out, T3 min: a shorter wait is not a time-out
T3_MAX_S = 0.250  # response time-out, T3 max; Rule 4.1.1-9 "timed out after T3max"
T6_MIN_S = 0.060  # time between request retries, T6 min (used after a transmit error)
C1_MIN = 3  # request retry count, C1 min
C1_MAX = 10  # request retry count, C1 max (Rule 4.1.1-10)
T9_S = 5.0  # sequence value expiration interval, T9 (min = max = 5 s)
SEQUENCE_MODULUS = 64  # the sequence number is 6 bits (IPMB message format)
GENERAL_CALL_ADDRESS = 0x00  # I2C general call, 7-bit form (Broadcast Get Device ID)
BUS_TAGS = ("A", "B")  # the one definition of the bus names; IPMB-A is mandatory (Rule 4.1.1-1)
CM_ADDRESS_DEFAULT = 0x20  # Chassis Manager IPMB address (Rule 4.1.4.2-5)
# Index of the first byte after the 6-byte IPMB header (rsSA/rqSA, netFn/LUN, checksum, rqSA/rsSA,
# seq/LUN, cmd): the data of a request, the completion code of a response. The frame ends with
# the data checksum (IPMB message format).
IPMB_DATA_INDEX = 6


def frame_payload(frame: bytes) -> bytes:
    """The bytes between the IPMB header and the trailing checksum: request data, or a
    response's completion code and data. The one slice every tool uses."""
    return bytes(frame[IPMB_DATA_INDEX:-1])


IPMB_MIN_FRAME = IPMB_DATA_INDEX + 1  # header (6 bytes) plus the data checksum
NETFN_SHIFT = 2  # the NetFn sits in bits 7:2 of byte 1, the LUN in bits 1:0
NETFN_RESPONSE_BIT = 1  # a response NetFn is odd (request NetFn + 1)
SEQ_SHIFT = 2  # the sequence number sits in bits 7:2 of byte 4, the LUN in bits 1:0
LUN_MASK = 0x03


def request_to(frame: bytes, address: int) -> bool:
    """True when `frame` is a well-formed request (even NetFn, both checksums valid) addressed to
    `address` (8-bit form): the frames a Chassis Manager answers rather than collects."""
    return (len(frame) >= IPMB_MIN_FRAME and frame[0] == address
            and not frame[1] >> NETFN_SHIFT & NETFN_RESPONSE_BIT
            and checksum(frame[:2]) == frame[2] and checksum(frame[3:-1]) == frame[-1])


def response_to(request: bytes, payload: bytes) -> bytes:
    """The response frame to the well-formed request `request`, with `payload` (completion code
    and data): rqSA and rsSA swapped, NetFn + 1, the request's sequence number, LUNs and command,
    both checksums. The one response builder of the tool."""
    req = IpmbHeaderReq(data=request)
    rsp = IpmbHeaderRsp()
    rsp.from_req_header(req)
    return bytes(encode_ipmb_msg(rsp, bytes(payload)))


@dataclass
class Exchange:
    """One request with its tries and its outcome."""

    t_s: float  # seconds since the run start, at the first transmission
    dest: int
    netfn: int
    cmd: int
    seq: int
    data: bytes
    broadcast: bool
    request_frame: bytes = b""
    tries: list = field(default_factory=list)  # [(bus, t_s)] per transmission
    response_bus: str | None = None
    response_frame: bytes | None = None
    rtt_s: float | None = None
    expected_loss: bool = False  # a missing response the run expects (the IPMC is resetting)

    @property
    def timed_out(self) -> bool:
        return self.response_frame is None

    @property
    def retries(self) -> int:
        return max(0, len(self.tries) - 1)

    @property
    def response(self) -> bytes:
        """Response data from the completion code on (no framing); empty after a timeout."""
        return frame_payload(self.response_frame) if self.response_frame else b""

    @property
    def completion_code(self) -> int | None:
        return self.response[0] if self.response else None


class Link:
    """Requester over one or two python-ipmi IPMB interfaces."""

    def __init__(self, interfaces: dict, collector, cm_address: int = CM_ADDRESS_DEFAULT,
                 retries: int = C1_MIN, timeout_s: float = T3_MAX_S,
                 bus_fail_streak: int = BUS_FAIL_STREAK_DEFAULT, clock=time.monotonic,
                 test_mode: bool = False, receiver=None) -> None:
        """`receiver` answers the requests that arrive for `cm_address` (an object with
        `handle(bus, frame)` returning the response frame or None; without one such frames are
        dropped). `test_mode` is for unit tests only: it lifts the T3 minimum so a test need not wait a
        production time-out, and paces a retry after a failed transmission by the (short)
        time-out instead of T6. Production runs never set it."""
        if not C1_MIN <= retries <= C1_MAX:
            raise ValueError(f"retries must be {C1_MIN}..{C1_MAX} (VITA 46.11 C1)")
        if not math.isfinite(timeout_s) or timeout_s <= 0 or timeout_s > T3_MAX_S:
            raise ValueError(f"timeout must be positive and at most {T3_MAX_S} s "
                             "(VITA 46.11 T3 max)")
        if not test_mode and timeout_s < T3_MIN_S:
            raise ValueError(f"timeout must be at least {T3_MIN_S} s (VITA 46.11 T3 min): an "
                             "earlier retry could overlap a response still on its way (T6)")
        if "A" not in interfaces:
            raise ValueError("IPMB-A is mandatory")
        self.interfaces = interfaces
        self.collector = collector
        self.receiver = receiver
        self.cm_address = cm_address
        self.retries = retries
        self.timeout_s = timeout_s
        self.bus_fail_streak = bus_fail_streak
        self._retry_spacing_s = timeout_s if test_mode else T6_MIN_S
        self._clock = clock
        self._t0 = clock()
        # Rule 4.1.8-2: per target, IPMB-A only until that target's IPMB-B is known to work.
        self._enabled: dict = {}  # dest -> buses in the order they were enabled
        self._failed: dict = {}  # dest -> set of buses marked failed for that target
        self._timeouts: dict = {}  # dest -> {bus: consecutive timeouts, the other bus answering}
        self._last_tx_bus: str | None = None  # bus of the last transmission, retries included
        self.enable_events: list = []  # (exchanges so far, dest, bus) when a bus was enabled
        self._seq = 0
        # (dest, netfn, cmd, seq) -> clock() of the last transmission: the fields a responder
        # matches to recognise a retry (the requester address is fixed per link).
        self._seq_last_sent: dict = {}
        self.exchanges: list = []

    def now_s(self) -> float:
        """Seconds since the link was created."""
        return self._clock() - self._t0

    def enabled_for(self, dest: int) -> list:
        """Buses used for `dest`: IPMB-A, then those enabled for it."""
        return self._enabled.get(dest, ["A"])

    def failed_for(self, dest: int) -> set:
        """Buses marked failed for `dest`."""
        return self._failed.get(dest, set())

    @property
    def failed(self) -> set:
        """Buses marked failed for any target (for the run summary)."""
        return {bus for buses in self._failed.values() for bus in buses}

    @property
    def enabled(self) -> list:
        """Buses enabled for any target, IPMB-A first (for the run summary)."""
        out = ["A"]
        for buses in self._enabled.values():
            out.extend(b for b in buses if b not in out)
        return out

    def enable_bus(self, bus: str, dest: int) -> bool:
        """Use `bus` for `dest` too, after Set IPMB State enabled it on that target; False when
        this transport has no such bus."""
        if bus not in self.interfaces:
            return False
        buses = self._enabled.setdefault(dest, ["A"])
        if bus not in buses:
            buses.append(bus)
            self.enable_events.append((len(self.exchanges), dest, bus))
        return True

    def usable_buses(self, dest: int) -> list:
        """Buses enabled for `dest` that are not marked failed for it (all enabled ones when
        every bus failed)."""
        failed = self.failed_for(dest)
        usable = [b for b in self.enabled_for(dest) if b not in failed]
        return usable or list(self.enabled_for(dest))

    def _next_seq(self) -> int:
        self._seq = (self._seq + 1) % SEQUENCE_MODULUS
        return self._seq

    def _wait_t9(self, key: tuple) -> None:
        """Wait out what is left of T9 since `key` (dest, netfn, cmd, seq) was last sent: at most
        T9_S, and only when SEQUENCE_MODULUS requests went by within T9 and this one repeats the
        target and command of the one that used the number last."""
        last = self._seq_last_sent.get(key)
        if last is not None:
            wait = last + T9_S - self._clock()
            if wait > 0:
                time.sleep(wait)

    def _alternate(self, dest: int, bus: str | None) -> str:
        """The usable bus for `dest` that follows `bus`; the first usable one when `bus` is not
        one of them (Rule 5.1.1-5)."""
        usable = self.usable_buses(dest)
        if bus not in usable:
            return usable[0]
        return usable[(usable.index(bus) + 1) % len(usable)]

    def request(self, dest: int, netfn: int, cmd: int, data: bytes = b"", *, lun: int = 0,
                broadcast: bool = False, tries: int | None = None) -> Exchange:
        """Send a request to IPMB address `dest` (8-bit form) and wait for its response.

        `tries` overrides the number of transmissions (default: 1 + retries). A broadcast
        request is written to the I2C general-call address.
        """
        header = IpmbHeaderReq()
        header.rs_sa = dest
        header.netfn = netfn
        header.rs_lun = lun
        header.rq_sa = self.cm_address
        header.rq_seq = self._next_seq()
        t9_key = (dest, netfn, cmd, header.rq_seq)
        self._wait_t9(t9_key)
        header.rq_lun = 0
        header.cmdid = cmd
        frame = encode_ipmb_msg(header, bytes(data))
        ex = Exchange(self.now_s(), dest, netfn, cmd, header.rq_seq, bytes(data), broadcast, frame)
        total = (1 + self.retries) if tries is None else tries
        bus = self._alternate(dest, self._last_tx_bus)
        timed_out_on: list = []
        self._drain()
        for attempt in range(total):
            ex.tries.append((bus, self.now_s()))
            self._last_tx_bus = bus
            sent_at = self._clock()
            self._seq_last_sent[t9_key] = sent_at
            if self._transmit(bus, frame, broadcast):
                answered = self._wait(header, sent_at + self.timeout_s)
                if answered is not None:
                    ex.response_bus, ex.response_frame = answered
                    ex.rtt_s = self._clock() - sent_at
                    break
            else:
                time.sleep(self._retry_spacing_s)  # transmit failed (address NAK): pace the retry
            timed_out_on.append(bus)
            if attempt + 1 < total:
                bus = self._alternate(dest, bus)
        self._account(ex, timed_out_on)
        self.exchanges.append(ex)
        return ex

    def _transmit(self, bus: str, frame: bytes, broadcast: bool) -> bool:
        iface = self.interfaces[bus]
        try:
            if broadcast:
                iface.raw_write(GENERAL_CALL_ADDRESS, frame)
            else:
                iface.send_frame(frame)
        except OSError:
            return False
        return True

    def _drain(self) -> None:
        """Drop frames left from earlier requests (late responses carry an older sequence);
        requests to the Chassis Manager are answered."""
        self.service()

    def _receive_request(self, bus: str, frame: bytes) -> bool:
        """Answer `frame` on `bus` when it is a request to this Chassis Manager and there is a
        receiver; True when it was one (it is not a response to anything we wait for)."""
        if self.receiver is None or not request_to(frame, self.cm_address):
            return False
        answer = self.receiver.handle(bus, frame)
        if answer is not None:
            self._transmit(bus, answer, False)
        return True

    def service(self) -> None:
        """Answer every request to the Chassis Manager that is queued; drop other frames."""
        while (item := self.collector.poll()) is not None:
            self._receive_request(*item)

    def idle(self, seconds: float) -> None:
        """Wait `seconds`, receiving: a request is answered within SERVICE_PERIOD_S of its
        arrival."""
        end = self._clock() + seconds
        while (remaining := end - self._clock()) > 0:
            item = self.collector.get(min(remaining, SERVICE_PERIOD_S))
            if item is not None:
                self._receive_request(*item)

    def _wait(self, header: IpmbHeaderReq, deadline: float):
        """The (bus, frame) that answers `header` before `deadline`, or None."""
        while True:
            remaining = deadline - self._clock()
            if remaining <= 0:
                return None
            item = self.collector.get(remaining)
            if item is None:
                return None
            bus, frame = item
            if self._receive_request(bus, frame):
                continue
            if rx_filter(header, frame, rq_sa=True, rs_sa=True):
                return bus, frame

    def _account(self, ex: Exchange, timed_out_on: list) -> None:
        """Bus health, per target: a bus that timed out while the other bus then answered is
        suspect; a response on a bus clears its streak."""
        if ex.response_bus is None:
            return
        streaks = self._timeouts.setdefault(ex.dest, dict.fromkeys(self.interfaces, 0))
        streaks[ex.response_bus] = 0
        for bus in set(timed_out_on):
            if bus == ex.response_bus:
                continue
            streaks[bus] += 1
            if streaks[bus] >= self.bus_fail_streak:
                self._failed.setdefault(ex.dest, set()).add(bus)


class FrameCollector:
    """python-ipmi "router" that queues every received frame with its bus tag.

    Passed as `router=` to the interfaces: they call `handle_frame` from their receive thread.
    A frame may carry an `epoch` attribute (the emulator stream's board generation, see
    `ipmc_emu_interface.EpochFrame`); `drop_before` makes the collector refuse and forget the
    frames of earlier generations, whichever thread delivers them.
    """

    def __init__(self) -> None:
        self._queue: queue.Queue = queue.Queue()  # (bus, frame, epoch or None)
        self._bus_of: dict = {}
        self._lock = threading.Lock()  # orders `handle_frame` against `drop_before`
        self._min_epoch = 0

    def register(self, interface, bus: str) -> None:
        """Name the bus `interface` belongs to."""
        self._bus_of[id(interface)] = bus

    def handle_frame(self, interface, frame: bytes) -> None:
        epoch = getattr(frame, "epoch", None)
        with self._lock:
            if epoch is not None and epoch < self._min_epoch:
                return  # a frame of a board that was reset away
            self._queue.put((self._bus_of[id(interface)], bytes(frame), epoch))

    def drop_before(self, epoch: int) -> None:
        """From now on refuse frames of an epoch below `epoch`, and drop the queued ones. Frames
        without an epoch and of epoch `epoch` or later are kept."""
        with self._lock:
            self._min_epoch = max(self._min_epoch, epoch)
            kept = []
            while True:
                try:
                    item = self._queue.get_nowait()
                except queue.Empty:
                    break
                if item[2] is None or item[2] >= self._min_epoch:
                    kept.append(item)
            for item in kept:
                self._queue.put(item)

    def close(self) -> None:
        """Nothing to stop: the collector has no thread."""

    def get(self, timeout: float):
        try:
            return self._queue.get(timeout=timeout)[:2]
        except queue.Empty:
            return None

    def poll(self):
        try:
            return self._queue.get_nowait()[:2]
        except queue.Empty:
            return None
