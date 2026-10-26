"""chassis_manager.py - a VITA 46.11 Tier-1 Chassis Manager on python-ipmi (PLAN decision 5).

One system-management layer, two transports:
  --transport aardvark   Total Phase Aardvark, python-ipmi's own `aardvark` interface (IPMB-A)
  --transport emu        `ipmc-emu serve` over TCP, IPMB-A and IPMB-B (ipmc_emu_interface.py)

Steps (each logged with a timestamp, the request and response bytes and the decoded result):
  1 discovery        Get Device ID to every slot address and the Broadcast Get Device ID form
  2 capabilities     Get VSO Capabilities; Set IPMB State enables IPMB-B when the IPMC has two
  3 FRU              Get FRU Inventory Area Info, Read FRU Data in 16-byte chunks, decode
  4 SDR              Get Device SDR Info, Reserve, every record by its next-ID chain, decode
  5 polling          Get Sensor Reading for every sensor each --poll-s for --duration-s, Get Self
                     Test Results once; only changes are printed
The request policy (T3 timeout, C1 retries, same sequence number, bus alternation) is cm_link.py;
the tool-chosen values are cm_settings.py. The exit status is 0 only when every check the run
lists in its summary passed (discovery, capabilities, locator, FRU complete and decoded, SDR walk,
polling).
Run `python chassis_manager.py --help`; the README has the venv command line.
"""

import argparse
import array
import json
import re
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path

from cm_commands import (
    CC_OK, CMD_COLD_RESET, CMD_GET_DEVICE_ID, CMD_GET_DEVICE_LOCATOR_RECORD_ID, CMD_GET_DEVICE_SDR,
    CMD_GET_DEVICE_SDR_INFO, CMD_GET_FRU_AREA_INFO, CMD_GET_SELF_TEST_RESULTS,
    CMD_GET_SENSOR_READING, CMD_GET_VSO_CAPABILITIES, CMD_READ_FRU_DATA, CMD_RESERVE_DEVICE_SDR,
    CMD_SET_EVENT_RECEIVER, CMD_SET_IPMB_STATE, IPMB_STATE_LOCAL_CONTROL, IPMB_STATE_NO_CHANGE, IPMB_STATE_OVERRIDE,
    NETFN_APP, NETFN_GROUP_EXTENSION, NETFN_SENSOR, NETFN_STORAGE, VSO_ID, command_name,
    decode_response, fru_access_unit, ipmb_interfaces, locator_record_id,
)
from cm_events import EventReceiver
from cm_link import C1_MAX, C1_MIN, CM_ADDRESS_DEFAULT, T3_MAX_S, FrameCollector, Link
from cm_settings import (
    AARDVARK_PORT_DEFAULT, AARDVARK_PULLUPS_DEFAULT, BUS_FAIL_STREAK_DEFAULT, CHUNK_BYTES,
    CM_TIER_DEFAULT, COLD_RESET_RECOVERY_S, DISCOVERY_TRIES_DEFAULT, DURATION_S_DEFAULT, POLL_S_DEFAULT,
    SDR_RECORDS_MAX, WITHHOLD_EVENTS_DEFAULT,
)

# Slot addressing (Rule 4.1.4.1.2-1, Rule 4.1.4.2-2): slot N is hardware address 40h + N, IPMB
# address 2 x that: slot 1 = 82h ... slot 31 = BEh.
SLOT_FIRST, SLOT_LAST = 1, 31
IPMB_ADDRESS_OF_SLOT_0 = 0x80

SDR_HEADER_BYTES = 5  # record ID (2), version, type, length
SDR_NEXT_NONE = 0xFFFF  # next-record ID after the last record (IPMI v2.0 section 33.12)
SDR_TYPES_WITH_SENSOR = (0x01, 0x02)  # full and compact sensor records
SDR_TYPE_MC_LOCATOR = 0x12  # Management Controller Device Locator record (IPMI v2.0 Table 37-8)
FRU_DEVICE_ID_IPMC = 0  # the IPMC's own FRU (VITA 46.11 section 10.1.3.11)
CC_RESERVATION_CANCELLED = 0xC5
FRU_OFFSET_BYTES = 2  # Read FRU Data offset, LS byte first

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import defs as defs_reader  # noqa: E402  (tools/defs.py, the one reader of the configuration)


class RunLog:
    """Timestamped lines on stdout."""

    def __init__(self, link: Link) -> None:
        self._link = link

    def line(self, text: str) -> None:
        print(f"[{self._link.now_s():8.3f}] {text}", flush=True)

    def step(self, title: str) -> None:
        self.line(f"== {title}")

    def exchange(self, ex) -> None:
        tx = ex.request_frame.hex(" ").upper()
        rx = ex.response_frame.hex(" ").upper() if ex.response_frame else "-"
        buses = "".join(bus for bus, _ in ex.tries)
        what = "broadcast " if ex.broadcast else ""
        result = ("TIMEOUT" if ex.timed_out
                  else decode_response(ex.netfn, ex.cmd, ex.response))
        rtt = f"{ex.rtt_s * 1000:.1f} ms" if ex.rtt_s is not None else "-"
        self.line(f"{what}{command_name(ex.netfn, ex.cmd)} to {ex.dest:02X}h seq {ex.seq:02X}h "
                  f"bus {buses} ({rtt}) tx {tx} rx {rx} -> {result}")


class Session:
    """The run: owns the link, the log and what was learned about each IPMC."""

    def __init__(self, link: Link, log: RunLog, args) -> None:
        self.link, self.log, self.args = link, log, args
        self.ipmcs: dict = {}
        self.poll_stats = {"cycles": 0, "readings": 0, "lost_readings": 0, "late_cycles": 0,
                           "bad_completion_codes": 0, "sensors": 0,
                           "reset_lost_readings": 0, "reset_recovered": 0}
        self.checks: list = []  # every pass/fail result that decides the exit status
        self._announced: set = set()  # (dest, bus) pairs already told to use the other bus
        # IPMCs sent Cold Reset that have not answered since, with the time the reset was sent
        self._resetting: dict = {}

    def check(self, name: str, ok: bool, detail: str = "", dest: int | None = None) -> bool:
        """Record one result that decides the exit status, and log it."""
        where = "" if dest is None else f" of {dest:02X}h"
        self.checks.append({"dest": None if dest is None else f"0x{dest:02X}", "name": name,
                            "ok": bool(ok), "detail": detail})
        self.log.line(f"check {name}{where}: {'PASS' if ok else 'FAIL'}"
                      + (f" ({detail})" if detail else ""))
        return bool(ok)

    def check_cc(self, name: str, ex, dest: int) -> bool:
        """Check a mandatory command: it answered with completion code 00h."""
        detail = ("timed out" if ex.timed_out else f"completion code {ex.completion_code:02X}h")
        return self.check(name, not ex.timed_out and ex.completion_code == CC_OK, detail, dest)

    def ask(self, dest: int, netfn: int, cmd: int, data: bytes = b"", *, show: bool = True,
            quiet: bool = False, **kwargs):
        """One request through the link policy, logged (`show` False logs only a timeout or a
        retry, `quiet` logs nothing); a newly failed bus is announced."""
        ex = self.link.request(dest, netfn, cmd, data, **kwargs)
        if not quiet and (show or ex.timed_out or ex.retries):
            self.log.exchange(ex)
        self._announce_failed_buses()
        return ex

    def _announce_failed_buses(self) -> None:
        """Section 9.3: tell an IPMC whose bus failed to use the other bus exclusively (Set IPMB
        State to that IPMC only: a bus failed for one target is not failed for another)."""
        for dest in list(self.ipmcs):
            for bus in sorted(self.link.failed_for(dest)):
                if (dest, bus) in self._announced:
                    continue
                self._announced.add((dest, bus))
                self.log.line(f"IPMB-{bus} marked failed for {dest:02X}h "
                              f"({self.link.bus_fail_streak} timeouts answered on the other "
                              f"bus); directing its traffic to IPMB-"
                              f"{self.link.usable_buses(dest)[0]}")
                states = [IPMB_STATE_NO_CHANGE, IPMB_STATE_NO_CHANGE]
                states[0 if bus == "A" else 1] = IPMB_STATE_OVERRIDE
                self.ask(dest, NETFN_GROUP_EXTENSION, CMD_SET_IPMB_STATE,
                         bytes([VSO_ID, *states]))

    # ---- step 1 ----

    def discover(self, slots: list) -> list:
        self.log.step("1 discovery: Get Device ID and Broadcast Get Device ID, slots "
                      + " ".join(f"{a:02X}h" for a in slots))
        found, silent = [], []
        for dest in slots:
            direct = self.ask(dest, NETFN_APP, CMD_GET_DEVICE_ID, quiet=True,
                              tries=self.args.discovery_tries)
            broadcast = self.ask(dest, NETFN_APP, CMD_GET_DEVICE_ID, quiet=True, broadcast=True,
                                 tries=self.args.discovery_tries)
            if direct.timed_out and broadcast.timed_out:
                silent.append(dest)
                continue
            for ex in (direct, broadcast):
                self.log.exchange(ex)
            # An IPMC that answers only one of the two forms is discovered too (Rule 5.2.2-1).
            if any(not ex.timed_out and ex.completion_code == CC_OK for ex in (direct, broadcast)):
                found.append(dest)
                self.ipmcs[dest] = {"address": f"0x{dest:02X}"}
        self.log.line("no response from: " + (" ".join(f"{a:02X}h" for a in silent) or "none"))
        self.log.line("found: " + (" ".join(f"{a:02X}h" for a in found) or "none"))
        self.check("discovery found an IPMC", bool(found))
        return found

    # ---- step 2 ----

    def capabilities(self, dest: int) -> None:
        self.log.step(f"2 capabilities of {dest:02X}h")
        caps = self.ask(dest, NETFN_GROUP_EXTENSION, CMD_GET_VSO_CAPABILITIES, bytes([VSO_ID]))
        info = self.ipmcs[dest]
        info["vso_capabilities"] = caps.response.hex(" ")
        if not self.check_cc("Get VSO Capabilities", caps, dest):
            return
        interfaces = ipmb_interfaces(caps.response)
        info["ipmb_interfaces"] = interfaces
        if interfaces < 2:
            self.log.line("IPMC reports one IPMB interface: IPMB-A only (Rule 4.1.8-2)")
            return
        if "B" not in self.link.interfaces:
            self.log.line("IPMC reports two IPMB interfaces; this transport has IPMB-A only")
            return
        state = self.ask(dest, NETFN_GROUP_EXTENSION, CMD_SET_IPMB_STATE,
                         bytes([VSO_ID, IPMB_STATE_NO_CHANGE, IPMB_STATE_LOCAL_CONTROL]))
        self.check_cc("Set IPMB State (enable IPMB-B)", state, dest)
        if not state.timed_out and state.completion_code == CC_OK:
            self.link.enable_bus("B", dest)
            info["ipmb_b_enabled"] = True
            self.log.line("IPMB-B enabled: messages alternate between IPMB-A and IPMB-B "
                          "(Rule 5.1.1-5)")

    # ---- step 3 ----

    def read_fru(self, dest: int) -> None:
        self.log.step(f"3 FRU of {dest:02X}h")
        area = self.ask(dest, NETFN_STORAGE, CMD_GET_FRU_AREA_INFO, bytes([0]))
        if not self.check_cc("Get FRU Inventory Area Info", area, dest) or len(area.response) < 4:
            self.check("FRU read complete", False, "no FRU size", dest)
            return
        size = area.response[1] | area.response[2] << 8
        # IPMI Tables 34-2/34-3: the offset and the count of Read FRU Data are in words when the
        # device is accessed by words.
        unit = fru_access_unit(area.response)
        data = bytearray()
        while len(data) < size:
            count_bytes = min(CHUNK_BYTES, size - len(data))
            count = -(-count_bytes // unit)
            offset = (len(data) // unit).to_bytes(FRU_OFFSET_BYTES, "little")
            chunk = self.ask(dest, NETFN_STORAGE, CMD_READ_FRU_DATA,
                             bytes([0]) + offset + bytes([count]), show=False)
            got = (chunk.response[2:2 + chunk.response[1] * unit]
                   if len(chunk.response) >= 2 else b"")
            if chunk.timed_out or chunk.completion_code != CC_OK or not got:
                self.log.exchange(chunk)
                break
            data += got[:size - len(data)]
        self.log.line(f"read {len(data)} of {size} FRU bytes in {CHUNK_BYTES}-byte chunks, "
                      f"accessed by {'words' if unit > 1 else 'bytes'}")
        self.check("FRU read complete", size > 0 and len(data) == size,
                   f"{len(data)} of {size} bytes", dest)
        self.ipmcs[dest]["fru_size"] = size
        self.ipmcs[dest]["fru_bytes"] = len(data)
        self.ipmcs[dest]["fru"] = self._decode_fru(bytes(data))
        self.check("FRU decoded", "error" not in self.ipmcs[dest]["fru"],
                   self.ipmcs[dest]["fru"].get("error", ""), dest)

    def _decode_fru(self, data: bytes) -> dict:
        from pyipmi.errors import DecodingError
        from pyipmi.fru import FruInventory
        try:
            inv = FruInventory(array.array("B", data))
        except (DecodingError, IndexError, AttributeError, ValueError) as exc:
            self.log.line(f"FRU decode failed: {exc}")
            return {"error": str(exc)}
        head = getattr(inv, "common_header", None)
        if head is None:
            self.log.line("FRU decode failed: no common header")
            return {"error": "no common header"}
        self.log.line(f"FRU common header v{head.format_version}: internal use "
                      f"{head.internal_use_area_offset}, chassis {head.chassis_info_area_offset}, "
                      f"board {head.board_info_area_offset}, product "
                      f"{head.product_info_area_offset}, multirecord "
                      f"{head.multirecord_area_offset}")
        fields = {}
        board, product = inv.board_info_area, inv.product_info_area
        if board:
            fields["board"] = {
                "mfg_date": str(board.mfg_date), "manufacturer": str(board.manufacturer),
                "product_name": str(board.product_name), "serial": str(board.serial_number),
                "part_number": str(board.part_number), "fru_file_id": str(board.fru_file_id)}
            self.log.line("FRU board: " + ", ".join(f"{k} {v}" for k, v in fields["board"].items()))
        if product:
            fields["product"] = {
                "manufacturer": str(product.manufacturer), "name": str(product.name),
                "part_number": str(product.part_number), "version": str(product.version),
                "serial": str(product.serial_number), "asset_tag": str(product.asset_tag)}
            self.log.line("FRU product: "
                          + ", ".join(f"{k} {v}" for k, v in fields["product"].items()))
        return fields

    # ---- step 4 ----

    def read_sdr(self, dest: int) -> list:
        from pyipmi.sdr import SdrCommon
        self.log.step(f"4 SDR of {dest:02X}h")
        info = self.ask(dest, NETFN_SENSOR, CMD_GET_DEVICE_SDR_INFO)
        info_ok = self.check_cc("Get Device SDR Info", info, dest)
        reservation = self._reserve(dest)
        self.check("Reserve Device SDR Repository", reservation is not None, "", dest)
        sensors, lengths, record_id, locator_ids = [], [], 0, []
        walk_complete, id_mismatches = False, 0
        for _ in range(SDR_RECORDS_MAX):
            if reservation is None:
                break
            got = self._read_sdr_record(dest, reservation, record_id)
            if got is None:
                break
            record, next_id = got
            if record is None:
                reservation = self._reserve(dest)
                continue
            decoded = SdrCommon.from_data(list(record))
            lengths.append(len(record))
            name = getattr(decoded, "device_id_string", "")
            number = getattr(decoded, "number", None)
            sensor_type = getattr(decoded, "sensor_type_code", None)
            self.log.line(f"SDR {decoded.id:04X}h type {decoded.type:02X}h {len(record)} bytes: "
                          + (f"sensor {number} type {sensor_type:02X}h " if number is not None
                             and sensor_type is not None else "") + f'"{name}"')
            if record_id != 0 and decoded.id != record_id:
                id_mismatches += 1
            # The decoded ID of the record header, not the lookup alias (0000h = first record).
            if decoded.type == SDR_TYPE_MC_LOCATOR:
                locator_ids.append(decoded.id)
            if decoded.type in SDR_TYPES_WITH_SENSOR and number is not None:
                sensors.append({"number": number, "name": str(name), "type": sensor_type})
            if next_id == SDR_NEXT_NONE:
                walk_complete = True
                break
            record_id = next_id
        records = len(lengths)
        self.ipmcs[dest]["sdr_records"] = records
        self.ipmcs[dest]["sdr_record_lengths"] = lengths
        self.ipmcs[dest]["sdr_locator_ids"] = locator_ids
        self.ipmcs[dest]["sensors"] = sensors
        self.log.line(f"{records} SDR records read and decoded, {len(sensors)} sensors")
        self.check("SDR walk complete", walk_complete and records > 0 and id_mismatches == 0,
                   f"{records} records, {'ended at FFFFh' if walk_complete else 'no end marker'}"
                   f", {id_mismatches} record ID mismatches", dest)
        if info_ok and len(info.response) >= 2:
            self.check("SDR sensor count equals Get Device SDR Info",
                       len(sensors) == info.response[1],
                       f"{len(sensors)} read, {info.response[1]} reported", dest)
        return sensors

    def locator(self, dest: int) -> None:
        """Step 2b: Get Device Locator Record ID of FRU 0 (VITA 46.11 section 10.1.3.11), asked
        right after the capabilities like the captured Chassis Manager; `check_locator` compares
        it with the SDRs after step 4."""
        self.log.step(f"2b device locator record ID of {dest:02X}h")
        ex = self.ask(dest, NETFN_GROUP_EXTENSION, CMD_GET_DEVICE_LOCATOR_RECORD_ID,
                      bytes([VSO_ID, FRU_DEVICE_ID_IPMC]))
        record = None
        if self.check_cc("Get Device Locator Record ID", ex, dest):
            record = locator_record_id(ex.response)
        self.ipmcs[dest]["locator_record_id"] = record

    def check_locator(self, dest: int) -> None:
        """Step 4b: the record ID of step 2b must be the MC locator record read from the SDRs."""
        self.log.step(f"4b device locator record ID of {dest:02X}h against the SDRs")
        info = self.ipmcs[dest]
        record = info["locator_record_id"]
        info["locator_ok"] = record is not None and record in info["sdr_locator_ids"]
        self.check("device locator record ID matches the SDR", info["locator_ok"], "", dest)
        self.log.line(
            "locator record ID " + ("missing" if record is None else f"{record:04X}h") +
            (" matches" if info["locator_ok"] else " does NOT match") +
            " the MC locator records read from the SDRs "
            + ", ".join(f"{i:04X}h" for i in info["sdr_locator_ids"]))

    def _reserve(self, dest: int):
        ex = self.ask(dest, NETFN_SENSOR, CMD_RESERVE_DEVICE_SDR)
        if ex.timed_out or ex.completion_code != CC_OK or len(ex.response) < 3:
            return None
        return ex.response[1:3]

    def _read_sdr_record(self, dest: int, reservation: bytes, record_id: int):
        """(record bytes, next ID) of one record: the header first, then 16-byte chunks. Returns
        (None, 0) when the reservation was cancelled and None on any other failure."""
        rid = record_id.to_bytes(2, "little")
        head = self.ask(dest, NETFN_SENSOR, CMD_GET_DEVICE_SDR,
                        reservation + rid + bytes([0, SDR_HEADER_BYTES]), show=False)
        if head.completion_code == CC_RESERVATION_CANCELLED:
            return None, 0
        if (head.timed_out or head.completion_code != CC_OK
                or len(head.response) < 3 + SDR_HEADER_BYTES):
            self.log.exchange(head)
            return None
        next_id = head.response[1] | head.response[2] << 8
        record = bytearray(head.response[3:3 + SDR_HEADER_BYTES])
        total = SDR_HEADER_BYTES + record[4]
        while len(record) < total:
            count = min(CHUNK_BYTES, total - len(record))
            chunk = self.ask(dest, NETFN_SENSOR, CMD_GET_DEVICE_SDR,
                             reservation + rid + bytes([len(record), count]), show=False)
            body = chunk.response[3:]
            if chunk.timed_out or chunk.completion_code != CC_OK or not body:
                self.log.exchange(chunk)
                return None
            record += body[:count]
        return bytes(record), next_id

    # ---- step 5 ----

    def poll(self, dest: int, sensors: list) -> None:
        a = self.args
        self.log.step(f"5 polling {dest:02X}h: {len(sensors)} sensors every {a.poll_s} s "
                      f"for {a.duration_s} s")
        self.check_cc("Get Self Test Results", self.ask(dest, NETFN_APP, CMD_GET_SELF_TEST_RESULTS),
                      dest)
        names = {s["number"]: s["name"] for s in sensors}
        self.poll_stats["sensors"] = len(names)
        last: dict = {}
        start = self.link.now_s()
        fault_at = (a.emu_bus_down_at_s if a.emu_bus_down_at_s is not None
                    else a.duration_s / 2)
        fault_sent = a.emu_bus_down is None
        reset_sent = a.cold_reset_at_s is None
        cycle = 0
        while self.link.now_s() - start < a.duration_s:
            slot_end = start + (cycle + 1) * a.poll_s
            if not fault_sent and self.link.now_s() - start >= fault_at:
                self.link_fault(a.emu_bus_down)
                fault_sent = True
            if not reset_sent and self.link.now_s() - start >= a.cold_reset_at_s:
                self.cold_reset(dest)
                reset_sent = True
            for number in names:
                asked_at = self.link.now_s()
                ex = self.ask(dest, NETFN_SENSOR, CMD_GET_SENSOR_READING, bytes([number]),
                              show=False)
                self.poll_stats["readings"] += 1
                state = "timeout" if ex.timed_out else ex.response.hex(" ").upper()
                reset_at = self._resetting.get(dest)
                recovering = (reset_at is not None
                              and asked_at - reset_at <= a.cold_reset_recovery_s)
                if ex.timed_out and recovering:
                    ex.expected_loss = True
                    self.poll_stats["reset_lost_readings"] += 1
                elif ex.timed_out:
                    self.poll_stats["lost_readings"] += 1
                elif ex.completion_code != CC_OK:
                    self.poll_stats["bad_completion_codes"] += 1
                if not ex.timed_out:
                    if recovering:
                        self.poll_stats["reset_recovered"] += 1
                    self._resetting.pop(dest, None)
                if last.get(number) != state:
                    last[number] = state
                    self.log.line(f'sensor {number} "{names[number]}": '
                                  + ("TIMEOUT" if ex.timed_out
                                     else decode_response(ex.netfn, ex.cmd, ex.response))
                                  + f" (bus {ex.response_bus or '-'})")
            self.poll_stats["cycles"] += 1
            cycle += 1
            if self.link.now_s() > slot_end:
                self.poll_stats["late_cycles"] += 1
            else:
                self.link.idle(slot_end - self.link.now_s())
        stats = self.poll_stats
        self.check("polling answered", stats["cycles"] > 0 and stats["readings"] > 0
                   and stats["lost_readings"] == 0 and stats["bad_completion_codes"] == 0,
                   f"{stats['cycles']} cycles, {stats['readings']} readings, "
                   f"{stats['lost_readings']} lost, {stats['bad_completion_codes']} with an "
                   f"error completion code", dest)
        if a.cold_reset_at_s is not None and reset_sent:
            self.check("recovered after Cold Reset", stats["reset_recovered"] > 0,
                       f"{stats['reset_recovered']} readings answered within "
                       f"{a.cold_reset_recovery_s} s of the reset, "
                       f"{stats['reset_lost_readings']} lost inside that time", dest)

    def set_event_receiver(self, dest: int) -> None:
        """Tell `dest` that this Chassis Manager is its Event Receiver (Set Event Receiver, NetFn
        04h cmd 00h: receiver address, LUN 0). A Tier-1 IPMC answers C1h; that is logged, not a
        failure."""
        ex = self.ask(dest, NETFN_SENSOR, CMD_SET_EVENT_RECEIVER,
                      bytes([self.link.cm_address, 0x00]))
        self.log.line(f"Set Event Receiver of {dest:02X}h: "
                      + ("no response" if ex.timed_out
                         else f"completion code {ex.completion_code:02X}h"))

    def cold_reset(self, dest: int) -> None:
        """Send Cold Reset (NetFn 06h cmd 02h) to `dest`, once: the IPMC may reset before it
        answers, so a missing response is expected and is not a failure. The polling that follows
        keeps running; its timeouts until `dest` answers again are expected too, but only within
        `--cold-reset-recovery-s` of the request: after that a timeout is a lost reading, and
        the run fails unless a reading was answered inside that time."""
        self.log.line(f"cold reset of {dest:02X}h")
        sent_at = self.link.now_s()
        ex = self.ask(dest, NETFN_APP, CMD_COLD_RESET, tries=1)
        if ex.timed_out:
            ex.expected_loss = True
            self.log.line(f"no response to Cold Reset from {dest:02X}h (expected)")
        self._resetting[dest] = sent_at

    def link_fault(self, bus: str) -> None:
        """Emulator only: break `bus` (`#bus A down`) to exercise failover."""
        self.log.line(f"emulator: IPMB-{bus} goes down")
        self.args.emu_connection.set_bus_down(bus, True)

    # ---- summary ----

    def summary(self, started: datetime) -> dict:
        commands: dict = {}
        unique: dict = {}
        sequence: list = []
        events = self.link.receiver
        for ex in self.link.exchanges:
            key = f"{ex.netfn:02X}/{ex.cmd:02X} {command_name(ex.netfn, ex.cmd)}"
            c = commands.setdefault(key, {
                "netfn": ex.netfn, "cmd": ex.cmd, "name": command_name(ex.netfn, ex.cmd),
                "requests": 0, "transmissions": 0, "responses": 0, "retries": 0, "timeouts": 0,
                "completion_codes": Counter(), "buses": Counter()})
            c["requests"] += 1
            c["transmissions"] += len(ex.tries)
            c["retries"] += ex.retries
            if ex.timed_out:
                c["timeouts"] += 1
            else:
                c["responses"] += 1
                c["completion_codes"][f"{ex.completion_code:02X}"] += 1
                c["buses"][ex.response_bus] += 1
            ukey = (ex.dest, ex.netfn, ex.cmd, ex.data, ex.broadcast, ex.response)
            u = unique.setdefault(ukey, {
                "dest": ex.dest, "netfn": ex.netfn, "cmd": ex.cmd, "request": ex.data.hex(" "),
                "broadcast": ex.broadcast, "response": ex.response.hex(" "),
                "timed_out": ex.timed_out, "count": 0, "first_t_s": round(ex.t_s, 3)})
            u["count"] += 1
            if sequence and sequence[-1][:2] == [ex.netfn, ex.cmd]:
                sequence[-1][2] += 1
            else:
                sequence.append([ex.netfn, ex.cmd, 1])
        totals = {
            "requests": len(self.link.exchanges),
            "transmissions": sum(len(e.tries) for e in self.link.exchanges),
            "responses": sum(1 for e in self.link.exchanges if not e.timed_out),
            "retries": sum(e.retries for e in self.link.exchanges),
            "timeouts": sum(1 for e in self.link.exchanges if e.timed_out),
            "timeouts_after_discovery": sum(
                1 for e in self.link.exchanges
                if e.timed_out and e.dest in self.ipmcs and not e.expected_loss),
        }
        for c in commands.values():
            c["completion_codes"] = dict(c["completion_codes"])
            c["buses"] = dict(c["buses"])
        return {
            "transport": self.args.transport, "cm_address": f"0x{self.link.cm_address:02X}",
            "started": started.isoformat(timespec="seconds"),
            "duration_s": round(self.link.now_s(), 3),
            "buses_enabled": self.link.enabled, "buses_failed": sorted(self.link.failed),
            "ipmcs": list(self.ipmcs.values()), "poll": self.poll_stats, "totals": totals,
            "run": {"poll_s": self.args.poll_s, "duration_s_requested": self.args.duration_s},
            "checks": self.checks,
            "transmissions": [
                {"dest": e.dest, "buses": "".join(bus for bus, _ in e.tries),
                 "broadcast": e.broadcast, "timed_out": e.timed_out}
                for e in self.link.exchanges],
            "bus_enable_events": [{"after_exchanges": n, "dest": d, "bus": b}
                                  for n, d, b in self.link.enable_events],
            "commands": commands, "exchanges": list(unique.values()), "sequence": sequence,
            "events": events.events if events else [],
            "other_requests": events.other_requests if events else [],
            **(events.counts() if events else EventReceiver(self.link.now_s).counts()),
        }


def default_emu_port() -> int:
    """IPMC_EMU_PORT from defs.rs, the one definition (the emulator reads it too)."""
    return defs_reader.read()["IPMC_EMU_PORT"]


def slot_addresses(text: str | None) -> list:
    """IPMB addresses of the slots in `text` ("1,2"), or all slots 1..31."""
    slots = (range(SLOT_FIRST, SLOT_LAST + 1) if not text
             else [int(s) for s in text.split(",")])
    for slot in slots:
        if not SLOT_FIRST <= slot <= SLOT_LAST:
            raise ValueError(f"slot {slot} is outside {SLOT_FIRST}..{SLOT_LAST}")
    return [IPMB_ADDRESS_OF_SLOT_0 + 2 * slot for slot in slots]


def parse_args(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--transport", choices=("aardvark", "emu"), required=True)
    p.add_argument("--host", default="127.0.0.1", help="emu: ipmc-emu serve host")
    p.add_argument("--port", type=int, default=default_emu_port(), help="emu: ipmc-emu serve TCP port")
    p.add_argument("--aardvark-port", type=int, default=AARDVARK_PORT_DEFAULT,
                   help="aardvark: adapter port number")
    p.add_argument("--aardvark-serial", default=None, help="aardvark: adapter serial number")
    p.add_argument("--aardvark-pullups", type=int, choices=(0, 1),
                   default=AARDVARK_PULLUPS_DEFAULT,
                   help="aardvark: enable the I2C pull-ups (target power stays off)")
    p.add_argument("--cm-address", type=lambda s: int(s, 0), default=CM_ADDRESS_DEFAULT,
                   help="requester IPMB address (default 0x20; 0x10 as the captured manager)")
    p.add_argument("--slots", default=None, help="slot numbers to probe, e.g. 1,2 (default 1..31)")
    p.add_argument("--poll-s", type=float, default=POLL_S_DEFAULT)
    p.add_argument("--duration-s", type=float, default=DURATION_S_DEFAULT)
    p.add_argument("--retries", type=int, default=C1_MIN, help=f"{C1_MIN}..{C1_MAX} (VITA C1)")
    p.add_argument("--timeout-s", type=float, default=T3_MAX_S,
                   help=f"response timeout, at most {T3_MAX_S} (VITA T3 max)")
    p.add_argument("--discovery-tries", type=int, default=DISCOVERY_TRIES_DEFAULT)
    p.add_argument("--bus-fail-streak", type=int, default=BUS_FAIL_STREAK_DEFAULT)
    p.add_argument("--emu-bus-down", choices=("A", "B"), default=None,
                   help="emu: break this bus during polling (failover test)")
    p.add_argument("--emu-bus-down-at-s", type=float, default=None,
                   help="emu: seconds into polling (default half the duration)")
    p.add_argument("--cm-tier", type=int, choices=(1, 2), default=CM_TIER_DEFAULT,
                   help="2: Event Receiver, answers Platform Event 00h; 1: answers C1h (Rule 7-2)")
    p.add_argument("--withhold-events", type=int, default=WITHHOLD_EVENTS_DEFAULT,
                   help="leave the first N Platform Event frames unanswered (retries count)")
    p.add_argument("--set-event-receiver", action="store_true",
                   help="send Set Event Receiver (this manager) to each IPMC found")
    p.add_argument("--cold-reset-at-s", type=float, default=None,
                   help="send Cold Reset to each IPMC found this many seconds into its polling")
    p.add_argument("--cold-reset-recovery-s", type=float, default=COLD_RESET_RECOVERY_S,
                   help="seconds after Cold Reset in which the IPMC must answer again; later "
                        "timeouts are lost readings")
    p.add_argument("--json", default=None, help="write the JSON summary here")
    args = p.parse_args(argv)
    if args.emu_bus_down and args.transport != "emu":
        p.error("--emu-bus-down needs --transport emu")
    if args.withhold_events < 0:
        p.error("--withhold-events must be 0 or more")
    return args


def open_transport(args):
    """(interfaces by bus, collector, cleanup callable) for the chosen transport."""
    collector = FrameCollector()
    if args.transport == "emu":
        from ipmc_emu_interface import EmuConnection, IpmcEmuInterface
        conn = EmuConnection(args.host, args.port)
        conn.wait_ready()
        args.emu_connection = conn
        interfaces = {bus: IpmcEmuInterface(conn, bus, args.cm_address, router=collector)
                      for bus in ("A", "B")}

        def cleanup():
            for iface in interfaces.values():
                iface.close()
            conn.close()
    else:
        from pyipmi.interfaces.aardvark import Aardvark
        iface = Aardvark(slave_address=args.cm_address, port=args.aardvark_port,
                         serial_number=args.aardvark_serial,
                         enable_i2c_pullups=bool(args.aardvark_pullups), router=collector)
        interfaces = {"A": iface}
        cleanup = iface.close
    for bus, iface in interfaces.items():
        collector.register(iface, bus)
        iface.open()
    return interfaces, collector, cleanup


def run(args) -> int:
    started = datetime.now()
    args.emu_connection = None
    interfaces, collector, cleanup = open_transport(args)
    try:
        link = Link(interfaces, collector, args.cm_address, args.retries, args.timeout_s,
                    args.bus_fail_streak)
        log = RunLog(link)
        link.receiver = EventReceiver(link.now_s, args.cm_tier, args.withhold_events,
                                      log.line)
        session = Session(link, log, args)
        log.line(f"Chassis Manager at {args.cm_address:02X}h over {args.transport}; "
                 f"timeout {args.timeout_s * 1000:.0f} ms, retries {args.retries}, "
                 f"Tier-{args.cm_tier} Chassis Manager")
        found = session.discover(slot_addresses(args.slots))
        for dest in found:
            if args.set_event_receiver:
                session.set_event_receiver(dest)
            session.capabilities(dest)
            session.locator(dest)
            session.read_fru(dest)
            sensors = session.read_sdr(dest)
            session.check_locator(dest)
            session.poll(dest, sensors)
        late = sum(1 for e in link.exchanges
                   if e.timed_out and e.dest in session.ipmcs and not e.expected_loss)
        session.check("no request to a found IPMC timed out", late == 0,
                      f"{late} timeouts")
        summary = session.summary(started)
        t = summary["totals"]
        log.line(f"done: {len(found)} IPMC(s), {t['requests']} requests, {t['retries']} retries, "
                 f"{t['timeouts']} timeouts ({t['timeouts_after_discovery']} to a found IPMC), "
                 f"poll cycles {session.poll_stats['cycles']}, lost readings "
                 f"{session.poll_stats['lost_readings']}")
        failed = [c for c in session.checks if not c["ok"]]
        log.line(f"checks: {len(session.checks) - len(failed)} passed, {len(failed)} failed")
        for c in failed:
            log.line(f"FAILED: {c['name']} {c['dest'] or ''} {c['detail']}".replace("  ", " "))
        if args.json:
            with open(args.json, "w", encoding="utf-8") as f:
                json.dump(summary, f, indent=1)
            log.line(f"summary written to {args.json}")
        return 0 if not failed else 1
    finally:
        cleanup()


if __name__ == "__main__":
    sys.exit(run(parse_args()))
