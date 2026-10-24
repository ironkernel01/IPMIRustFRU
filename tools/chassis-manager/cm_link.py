"""cm_link.py - the Chassis Manager's IPMB request policy (ANSI/VITA 46.11-2022 section 4.1.1/5.1.1).

`Link.request` sends one request and applies the spec's rules for the requester:
 - 250 ms response timeout (T3 max, Rule 4.1.1-9);
 - a timed-out request is retried up to C1 times (3..10, Rule 4.1.1-10), every try with the same
   sequence number (Obs 4.1.1-6), on the other bus when both buses are enabled (Rule 5.1.1-6);
 - messages alternate between the enabled buses (Rule 5.1.1-5), the first on IPMB-A;
 - IPMB-A only until the target's IPMB-B support is known (Rule 4.1.8-2);
 - a sequence number is not used again for the same target and command within T9 (5 s) of its
   last transmission, so a compliant IPMC never takes a new request for a retry of an old one
   (its duplicate cache would answer it with the old response); the request waits instead.
A bus that times out for a request which the other bus then answered, `bus_fail_streak` times in
a row, is added to `Link.failed` and no longer chosen for a first try; the caller tells the targets
to use the other bus (section 9.3: the Chassis Manager directs traffic to the other bus).

Frames are built and checked with python-ipmi (`IpmbHeaderReq`, `encode_ipmb_msg`, `rx_filter`);
the transports are python-ipmi `IpmbInterface` objects, one per bus.
"""

import queue
import time
from dataclasses import dataclass, field

from pyipmi.interfaces.ipmb import IpmbHeaderReq, encode_ipmb_msg, rx_filter

# Table 4.1.1-1 (section 4.1.1 p.31) and the rules next to each value.
T3_MAX_S = 0.250  # response time-out, T3 max; Rule 4.1.1-9 "timed out after T3max"
T6_MIN_S = 0.060  # time between request retries, T6 min (used after a transmit error)
C1_MIN = 3  # request retry count, C1 min
C1_MAX = 10  # request retry count, C1 max (Rule 4.1.1-10)
T9_S = 5.0  # sequence value expiration interval, T9 (min = max = 5 s)
SEQUENCE_MODULUS = 64  # the sequence number is 6 bits (IPMB message format)
GENERAL_CALL_ADDRESS = 0x00  # I2C general call, 7-bit form (Broadcast Get Device ID)
BUS_TAGS = ("A", "B")  # IPMB-A is mandatory (Rule 4.1.1-1)
CM_ADDRESS_DEFAULT = 0x20  # Chassis Manager IPMB address (Rule 4.1.4.2-5)


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

    @property
    def timed_out(self) -> bool:
        return self.response_frame is None

    @property
    def retries(self) -> int:
        return max(0, len(self.tries) - 1)

    @property
    def response(self) -> bytes:
        """Response data from the completion code on (no framing); empty after a timeout."""
        return bytes(self.response_frame[6:-1]) if self.response_frame else b""

    @property
    def completion_code(self) -> int | None:
        return self.response[0] if self.response else None


class Link:
    """Requester over one or two python-ipmi IPMB interfaces."""

    def __init__(self, interfaces: dict, collector, cm_address: int = CM_ADDRESS_DEFAULT,
                 retries: int = C1_MIN, timeout_s: float = T3_MAX_S, bus_fail_streak: int = 2,
                 clock=time.monotonic) -> None:
        if not C1_MIN <= retries <= C1_MAX:
            raise ValueError(f"retries must be {C1_MIN}..{C1_MAX} (VITA 46.11 C1)")
        if timeout_s > T3_MAX_S:
            raise ValueError(f"timeout must not exceed {T3_MAX_S} s (VITA 46.11 T3 max)")
        if "A" not in interfaces:
            raise ValueError("IPMB-A is mandatory")
        self.interfaces = interfaces
        self.collector = collector
        self.cm_address = cm_address
        self.retries = retries
        self.timeout_s = timeout_s
        self.bus_fail_streak = bus_fail_streak
        self._clock = clock
        self._t0 = clock()
        self.enabled = ["A"]  # Rule 4.1.8-2: IPMB-A only until IPMB-B is known to work
        self.failed: set = set()
        self._timeouts = dict.fromkeys(interfaces, 0)  # consecutive, with the other bus answering
        self._last_first_bus: str | None = None
        self._seq = 0
        # (dest, netfn, cmd, seq) -> clock() of the last transmission: the fields a responder
        # matches to recognise a retry (the requester address is fixed per link).
        self._seq_last_sent: dict = {}
        self.exchanges: list = []

    def now_s(self) -> float:
        """Seconds since the link was created."""
        return self._clock() - self._t0

    def enable_bus(self, bus: str) -> bool:
        """Use `bus` too (after Set IPMB State enabled it on the target); False when this
        transport has no such bus."""
        if bus not in self.interfaces:
            return False
        if bus not in self.enabled:
            self.enabled.append(bus)
        return True

    def usable_buses(self) -> list:
        """Enabled buses that are not marked failed (all enabled ones when every bus failed)."""
        usable = [b for b in self.enabled if b not in self.failed]
        return usable or list(self.enabled)

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

    def _first_bus(self) -> str:
        usable = self.usable_buses()
        if self._last_first_bus in usable and len(usable) > 1:
            bus = usable[(usable.index(self._last_first_bus) + 1) % len(usable)]
        else:
            bus = usable[0]
        self._last_first_bus = bus
        return bus

    def _alternate(self, bus: str) -> str:
        usable = self.usable_buses()
        if len(usable) < 2 or bus not in usable:
            return bus
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
        bus = self._first_bus()
        timed_out_on: list = []
        self._drain()
        for attempt in range(total):
            ex.tries.append((bus, self.now_s()))
            sent_at = self._clock()
            self._seq_last_sent[t9_key] = sent_at
            if self._transmit(bus, frame, broadcast):
                answered = self._wait(header, sent_at + self.timeout_s)
                if answered is not None:
                    ex.response_bus, ex.response_frame = answered
                    ex.rtt_s = self._clock() - sent_at
                    break
            else:
                time.sleep(T6_MIN_S)  # the transmit itself failed (address NAK): pace the retry
            timed_out_on.append(bus)
            if attempt + 1 < total:
                bus = self._alternate(bus)
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
        """Drop frames left from earlier requests (late responses carry an older sequence)."""
        while self.collector.poll() is not None:
            pass

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
            if rx_filter(header, frame, rq_sa=True, rs_sa=True):
                return bus, frame

    def _account(self, ex: Exchange, timed_out_on: list) -> None:
        """Bus health: a bus that timed out while the other bus then answered is suspect; a
        response on a bus clears its streak."""
        if ex.response_bus is None:
            return
        self._timeouts[ex.response_bus] = 0
        for bus in set(timed_out_on):
            if bus == ex.response_bus:
                continue
            self._timeouts[bus] += 1
            if self._timeouts[bus] >= self.bus_fail_streak and bus not in self.failed:
                self.failed.add(bus)


class FrameCollector:
    """python-ipmi "router" that queues every received frame with its bus tag.

    Passed as `router=` to the interfaces: they call `handle_frame` from their receive thread.
    """

    def __init__(self) -> None:
        self._queue: queue.Queue = queue.Queue()
        self._bus_of: dict = {}

    def register(self, interface, bus: str) -> None:
        """Name the bus `interface` belongs to."""
        self._bus_of[id(interface)] = bus

    def handle_frame(self, interface, frame: bytes) -> None:
        self._queue.put((self._bus_of[id(interface)], bytes(frame)))

    def close(self) -> None:
        """Nothing to stop: the collector has no thread."""

    def get(self, timeout: float):
        try:
            return self._queue.get(timeout=timeout)
        except queue.Empty:
            return None

    def poll(self):
        try:
            return self._queue.get_nowait()
        except queue.Empty:
            return None
