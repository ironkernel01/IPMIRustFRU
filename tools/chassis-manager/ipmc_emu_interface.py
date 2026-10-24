"""ipmc_emu_interface.py - python-ipmi IPMB transport into `ipmc-emu serve`.

One `EmuConnection` is one TCP connection to the emulator. A reader thread splits the received
lines and queues each IPMC frame by its bus tag. `IpmcEmuInterface` is one bus over that
connection: a subclass of python-ipmi's `IpmbInterface`, so the Chassis Manager drives it like the
`aardvark` interface. The line protocol is documented in the ipmc-emu crate documentation
(`crates/ipmc-emu/src/lib.rs`).
"""

import logging
import queue
import socket
import threading

from pyipmi.interfaces.ipmb import IpmbInterface

logger = logging.getLogger(__name__)

BUS_TAGS = ("A", "B")
# Protocol line prefixes (crates/ipmc-emu/src/lib.rs, "Serve mode").
READY_PREFIX = "#ready "
ERROR_PREFIX = "#error "
TRACE_PREFIX = "# "
CONTROL_PREFIX = "#"
# One 7-bit address in an 8-bit IPMB address byte (IPMI writes slave addresses in 8-bit form).
ADDRESS_SHIFT = 1
RECV_TIMEOUT_S = 0.2  # socket read granularity of the reader thread
READY_TIMEOUT_S = 5.0
CONNECT_TIMEOUT_S = 5.0


class EmuConnection:
    """A TCP connection to `ipmc-emu serve`, shared by the per-bus interfaces."""

    def __init__(self, host: str, port: int) -> None:
        self._sock = socket.create_connection((host, port), timeout=CONNECT_TIMEOUT_S)
        self._sock.settimeout(RECV_TIMEOUT_S)
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self._queues = {tag: queue.Queue() for tag in BUS_TAGS}
        self._ready = threading.Event()
        self.own_address: int | None = None
        self.errors: list[str] = []
        self.traces: list[str] = []
        self.closed_by_peer = False
        self._stop = threading.Event()
        self._write_lock = threading.Lock()
        self._reader = threading.Thread(target=self._read_loop, name="ipmc-emu-rx", daemon=True)
        self._reader.start()

    def wait_ready(self, timeout: float = READY_TIMEOUT_S) -> int:
        """Wait for `#ready <addr>` and return the IPMC's own IPMB address (8-bit form)."""
        if not self._ready.wait(timeout) or self.own_address is None:
            raise TimeoutError("no #ready line from ipmc-emu serve")
        return self.own_address

    def send_frame(self, bus: str, frame: bytes) -> None:
        """Write one frame on `bus` (destination address byte first)."""
        self.write_line(f"{bus} {frame.hex(' ').upper()}")

    def write_line(self, text: str) -> None:
        """Write one protocol line (a frame or a `#` control line)."""
        data = (text + "\n").encode("ascii")
        with self._write_lock:
            self._sock.sendall(data)

    def reset(self, timeout: float = READY_TIMEOUT_S) -> int:
        """Power-cycle the emulated board (`#reset`) and wait for its new `#ready` line; returns
        the IPMC's own IPMB address (8-bit form). Frames the old board sent before the reset may
        still sit in the bus queues; the caller drains them."""
        self._ready.clear()
        self.write_line("#reset")
        return self.wait_ready(timeout)

    def set_bus_down(self, bus: str, down: bool) -> None:
        """Break or repair `bus` in the emulator (`#bus A down` / `#bus A up`)."""
        self.write_line(f"#bus {bus} {'down' if down else 'up'}")

    def read_frame(self, bus: str, timeout: float) -> bytes | None:
        """The next IPMC frame received for `bus`, or None after `timeout` seconds."""
        try:
            return self._queues[bus].get(timeout=timeout)
        except queue.Empty:
            if self.closed_by_peer:
                raise ConnectionError("ipmc-emu serve closed the connection") from None
            return None

    def close(self) -> None:
        """Send `#quit` and close the socket."""
        try:
            self.write_line("#quit")
        except OSError:
            pass
        self._stop.set()
        self._reader.join(timeout=2 * RECV_TIMEOUT_S)
        self._sock.close()

    def _read_loop(self) -> None:
        pending = b""
        while not self._stop.is_set():
            try:
                chunk = self._sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            if not chunk:
                self.closed_by_peer = True
                break
            pending += chunk
            *lines, pending = pending.split(b"\n")
            for raw in lines:
                self._on_line(raw.decode("ascii", errors="replace").strip())

    def _on_line(self, line: str) -> None:
        if line.startswith(READY_PREFIX):
            self.own_address = int(line[len(READY_PREFIX):], 16)
            self._ready.set()
        elif line.startswith(ERROR_PREFIX):
            self.errors.append(line)
            logger.warning("ipmc-emu: %s", line)
        elif line.startswith(TRACE_PREFIX):
            self.traces.append(line[len(TRACE_PREFIX):])
        elif line[:1] in BUS_TAGS and line[1:2] == " ":
            try:
                frame = bytes(int(tok, 16) for tok in line[2:].split())
            except ValueError:  # a bad hex token or a value above FFh: keep the reader running
                logger.warning("ipmc-emu: bad frame line %r", line)
                return
            self._queues[line[0]].put(frame)
        elif line:
            logger.warning("ipmc-emu: unrecognised line %r", line)


class IpmcEmuInterface(IpmbInterface):
    """One IPMB bus ("A" or "B") of an `EmuConnection`, as a python-ipmi IPMB interface."""

    NAME = "ipmc-emu"

    def __init__(self, connection: EmuConnection, bus: str, slave_address: int = 0x20,
                 router=None) -> None:
        super().__init__(slave_address, router)
        if bus not in BUS_TAGS:
            raise ValueError(f"bus must be A or B, not {bus!r}")
        self.connection = connection
        self.bus = bus

    def open(self) -> None:
        """Start the receive thread."""
        self._start_receiver()

    def close(self) -> None:
        """Stop the receive thread (the connection is closed by its owner)."""
        self._stop_receiver()
        super().close()

    def send_frame(self, frame: bytes) -> None:
        """Send a complete IPMB message, starting with rsSA."""
        self.connection.send_frame(self.bus, bytes(frame))

    def raw_write(self, address: int, data: bytes) -> None:
        """Write `data` to the 7-bit I2C `address`: the same call as the `aardvark` interface.

        Address 0 is the I2C general call (Broadcast Get Device ID); the emulator takes the
        address byte first, as the IPMC's I2C slave sees it.
        """
        self.connection.send_frame(self.bus, bytes((address << ADDRESS_SHIFT,)) + bytes(data))

    def _read_frame(self, timeout: float) -> bytes | None:
        return self.connection.read_frame(self.bus, timeout)
