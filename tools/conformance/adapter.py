"""adapter.py - maps the TRANSPORT.md Target interface onto the bench transports.

emu:      the TCP line protocol of `ipmc-emu serve` (both buses, bus fault injection). With no
          port given, this module starts `cargo run -q -p ipmc-emu -- serve --port 0 --b-fitted 1`
          itself and reads the `listening <host>:<port>` line.
aardvark: python-ipmi's `aardvark` interface on IPMB-A.

The transports are the Chassis Manager's: `EmuConnection`/`IpmcEmuInterface` from
tools/chassis-manager/ipmc_emu_interface.py and python-ipmi's `Aardvark`; frames from every
interface land in the Chassis Manager's `FrameCollector` tagged with their bus. Run with the
python-ipmi venv interpreter.
"""
from __future__ import annotations

import atexit
import os
import queue
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tools" / "chassis-manager"))

from cm_link import SEQUENCE_MODULUS, FrameCollector  # noqa: E402
# The one IPMB checksum and request encoder (python-ipmi's, as the Chassis Manager uses them);
# `checksum` is also the TRANSPORT.md helper of that name.
from pyipmi.interfaces.ipmb import IpmbHeaderReq, checksum, encode_ipmb_msg  # noqa: E402,F401

OWN_ADDRESS_BENCH = 0x82  # the IPMC's 8-bit IPMB address on the bench (GA strapped)
REQUESTER_ADDRESS = 0x20  # Chassis Manager address (Rule 4.1.4.2-5)
EMU_HOST = "127.0.0.1"
EMU_START_TIMEOUT_S = 180.0  # first cargo build can be slow
EMU_RESET_TIMEOUT_S = 5.0
# RESET_SETTLE_S -- after `#ready`, time for python-ipmi's receiver threads to hand the frames
# the old board sent before the reset to the collector, so the drain that follows removes them.
RESET_SETTLE_S = 0.05
# IPMB header field widths (IPMB v1.0 message format): netFn and rqSeq 6 bits, LUNs 2 bits.
NETFN_MASK, SEQ_MASK, LUN_MASK = 0x3F, 0x3F, 0x03
# I2C_STATUS_SLA_NACK -- Aardvark status for an address no device acknowledged (pyaardvark
# constants.I2C_STATUS_SLA_NACK; kept local so the emulator transport needs no pyaardvark import).
# pyaardvark raises IOError(status, text), which Python maps to the OSError subclass of that errno.
I2C_STATUS_SLA_NACK = 3


class NoResponse(Exception):
    """No response within timeout_ms (after the adapter's own retries, if any)."""


@dataclass
class Response:
    cc: int
    data: bytes
    bus: str
    elapsed_ms: float
    frame: bytes
    netfn: int
    seq: int
    rq_lun: int
    rs_lun: int


def frame(rs_sa, netfn, rs_lun, rq_sa, seq, rq_lun, cmd, data=b"") -> bytes:
    """A request frame, rsSA first, both checksums included (python-ipmi's encoder), each header
    field cut to its IPMB width."""
    header = IpmbHeaderReq()
    header.rs_sa, header.netfn, header.rs_lun = rs_sa, netfn & NETFN_MASK, rs_lun & LUN_MASK
    header.rq_sa, header.rq_seq, header.rq_lun = rq_sa, seq & SEQ_MASK, rq_lun & LUN_MASK
    header.cmdid = cmd
    return encode_ipmb_msg(header, bytes(data))


class Target:
    def __init__(self, transport, interfaces, collector, own_address, cleanup,
                 connection=None, buses=("A",), can_fail_bus=False):
        self.transport = transport
        self.own_address = own_address
        self.requester_address = REQUESTER_ADDRESS
        self.buses = tuple(buses)
        self.can_fail_bus = can_fail_bus
        self._interfaces = interfaces
        self._collector = collector
        self._connection = connection
        self._cleanup = cleanup
        self._pending = []  # (bus, frame) received but not yet claimed
        self._seq = 0
        atexit.register(self.close)

    def close(self):
        cleanup, self._cleanup = self._cleanup, None
        if cleanup:
            cleanup()

    # -- frames -------------------------------------------------------------------------
    def send_frame(self, bus, frame_bytes):
        """Writes the frame as given. An address no device acknowledges is not an error: the
        frame went out and no controller took it, which is how a frame for another address
        looks on a real bus (TRANSPORT.md send_frame)."""
        self._write(bus, frame_bytes)

    def _write(self, bus, frame_bytes) -> bool:
        """send_frame's write; False when no device acknowledged the address (the Aardvark
        reports I2C_STATUS_SLA_NACK). Every other write error is raised."""
        try:
            self._interfaces[bus].send_frame(bytes(frame_bytes))
        except OSError as error:
            if error.errno != I2C_STATUS_SLA_NACK:
                raise
            return False
        return True

    def receive(self, bus=None, timeout_ms=250):
        deadline = time.monotonic() + timeout_ms / 1000.0
        while True:
            for item in self._pending:
                if bus is None or item[0] == bus:
                    self._pending.remove(item)
                    return item[1]
            got = self._collector.get(max(0.0, deadline - time.monotonic()))
            if got is None:
                return None
            self._pending.append(got)

    def request(self, netfn, cmd, data=b"", *, bus="A", rs_lun=0, seq=None,
                timeout_ms=250, retries=0):
        if seq is None:
            seq = self._seq
            self._seq = (self._seq + 1) % SEQUENCE_MODULUS
        raw = frame(self.own_address, netfn, rs_lun, self.requester_address, seq, 0, cmd, data)
        # A try whose address nobody acknowledged still waits its timeout (no response can come,
        # and the wait keeps the retry spacing); the count goes into the NoResponse text.
        unacknowledged = 0
        for _ in range(retries + 1):
            if not self._write(bus, raw):
                unacknowledged += 1
            start = time.monotonic()
            deadline = start + timeout_ms / 1000.0
            while True:
                got = self._collector.get(max(0.0, deadline - time.monotonic()))
                if got is None:
                    break
                rx_bus, rx = got
                arrived = time.monotonic()
                if (len(rx) >= 8 and rx[0] == self.requester_address
                        and rx[3] == self.own_address and rx[4] >> 2 == seq and rx[5] == cmd
                        and rx[1] >> 2 == netfn + 1):
                    return Response(cc=rx[6], data=bytes(rx[7:-1]), bus=rx_bus,
                                    elapsed_ms=(arrived - start) * 1000.0, frame=bytes(rx),
                                    netfn=rx[1] >> 2, seq=rx[4] >> 2, rq_lun=rx[1] & 3,
                                    rs_lun=rx[4] & 3)
                self._pending.append(got)  # not ours: keep it for receive()
                if len(self._pending) > 4096:
                    del self._pending[:2048]
        nack = (f"; address {self.own_address:02x} not acknowledged on {unacknowledged} of "
                f"{retries + 1} tries" if unacknowledged else "")
        raise NoResponse(f"no response to nf={netfn:02x} cmd={cmd:02x} seq={seq} on bus {bus} "
                         f"within {timeout_ms} ms ({retries} retries){nack}")

    # -- faults and reset -----------------------------------------------------------------
    def bus_down(self, bus):
        if not self.can_fail_bus:
            raise NotImplementedError("bus fault injection needs the emulator")
        self._connection.set_bus_down(bus, True)

    def bus_up(self, bus):
        if not self.can_fail_bus:
            raise NotImplementedError("bus fault injection needs the emulator")
        self._connection.set_bus_down(bus, False)

    def reset(self):
        if self.transport != "emu":
            raise NotImplementedError("the board cannot be reset from this interface")
        # The new board starts with both buses up (`#reset` in the ipmc-emu line protocol).
        self._connection.reset(EMU_RESET_TIMEOUT_S)
        time.sleep(RESET_SETTLE_S)
        self._pending.clear()
        while self._collector.poll() is not None:
            pass
        self._connection.errors.clear()


def _start_emu():
    env = dict(os.environ)
    env["PATH"] = str(Path.home() / ".cargo" / "bin") + os.pathsep + env.get("PATH", "")
    proc = subprocess.Popen(
        ["cargo", "run", "-q", "-p", "ipmc-emu", "--", "serve", "--port", "0", "--b-fitted", "1"],
        cwd=REPO, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = queue.Queue()

    import threading

    def pump():
        for line in proc.stdout:
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()
    deadline = time.monotonic() + EMU_START_TIMEOUT_S
    seen = []
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=1.0)
        except queue.Empty:
            continue
        if line is None:
            break
        seen.append(line)
        match = re.search(r"listening \S+:(\d+)", line)
        if match:
            return proc, int(match.group(1))
    proc.kill()
    raise RuntimeError("ipmc-emu serve did not report a port: " + "".join(seen)[-500:])


def connect(transport, port=None):
    collector = FrameCollector()
    if transport == "emu":
        from ipmc_emu_interface import EmuConnection, IpmcEmuInterface
        proc = None
        if not port:
            proc, port = _start_emu()
        conn = EmuConnection(EMU_HOST, int(port))
        own = conn.wait_ready()
        interfaces = {b: IpmcEmuInterface(conn, b, REQUESTER_ADDRESS, router=collector)
                      for b in ("A", "B")}
        for b, iface in interfaces.items():
            collector.register(iface, b)
            iface.open()

        def cleanup():
            for iface in interfaces.values():
                iface.close()
            conn.close()
            if proc is not None:
                proc.terminate()

        return Target("emu", interfaces, collector, own, cleanup, conn,
                      buses=("A", "B"), can_fail_bus=True)
    if transport == "aardvark":
        from pyipmi.interfaces.aardvark import Aardvark
        iface = Aardvark(slave_address=REQUESTER_ADDRESS, port=int(port or 0),
                         enable_i2c_pullups=True, router=collector)
        collector.register(iface, "A")
        iface.open()
        return Target("aardvark", {"A": iface}, collector, OWN_ADDRESS_BENCH, iface.close,
                      buses=("A",), can_fail_bus=False)
    raise ValueError(f"unknown transport {transport!r}")
