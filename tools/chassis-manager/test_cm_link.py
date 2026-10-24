"""test_cm_link.py - the Chassis Manager request policy against fake IPMB interfaces, the
Session steps (discovery, FRU, SDR, polling, exit checks), the response decoders, the emulator
connection and the cross-check's expected-difference rules.

Run from tools/chassis-manager with the venv python:  python -m unittest test_cm_link -v
"""

import argparse
import unittest
from unittest import mock

from pyipmi.interfaces.ipmb import IpmbHeaderReq, IpmbHeaderRsp, encode_ipmb_msg

from chassis_manager import RunLog, Session
from cm_commands import (
    command_name, decode_device_id, decode_locator_record_id, fru_access_unit, ipmi_version,
    locator_record_id,
)
from cm_link import (
    C1_MAX, C1_MIN, SEQUENCE_MODULUS, T3_MAX_S, T3_MIN_S, T9_S, FrameCollector, Link,
    frame_payload,
)

TIMEOUT_S = 0.02  # far below T3 max: the tests wait this long for a silent bus
DEST = 0x82


class FakeBus:
    """An IPMB interface whose IPMC answers requests unless `silent`."""

    def __init__(self, collector: FrameCollector, bus: str, silent: bool = False) -> None:
        self.collector, self.bus, self.silent = collector, bus, silent
        self.sent: list = []
        self.general_call: list = []
        collector.register(self, bus)

    def send_frame(self, frame: bytes) -> None:
        self.sent.append(bytes(frame))
        if self.silent:
            return
        req = IpmbHeaderReq(data=frame)
        rsp = IpmbHeaderRsp()
        rsp.from_req_header(req)
        self.collector.handle_frame(self, encode_ipmb_msg(rsp, bytes([0x00, 0x42])))

    def raw_write(self, address: int, data: bytes) -> None:
        self.general_call.append((address, bytes(data)))
        self.send_frame(data)


def make_link(a_silent=False, b_silent=False, retries=C1_MIN, streak=2, enable_b=True):
    collector = FrameCollector()
    buses = {"A": FakeBus(collector, "A", a_silent), "B": FakeBus(collector, "B", b_silent)}
    link = Link(buses, collector, retries=retries, timeout_s=TIMEOUT_S, bus_fail_streak=streak,
                test_mode=True)
    if enable_b:
        link.enable_bus("B", DEST)
    return link, buses


class LinkPolicy(unittest.TestCase):
    def test_a_request_gets_its_response_and_checks_the_completion_code(self):
        link, _ = make_link()
        ex = link.request(DEST, 0x06, 0x01)
        self.assertFalse(ex.timed_out)
        self.assertEqual((ex.completion_code, ex.response), (0x00, bytes([0x00, 0x42])))

    def test_messages_alternate_between_the_enabled_buses_starting_on_a(self):
        link, _ = make_link()
        used = [link.request(DEST, 0x06, 0x01).response_bus for _ in range(4)]
        self.assertEqual(used, ["A", "B", "A", "B"])

    def test_ipmb_a_only_until_b_is_enabled(self):
        link, buses = make_link(enable_b=False)
        used = [link.request(DEST, 0x06, 0x01).response_bus for _ in range(3)]
        self.assertEqual(used, ["A", "A", "A"])
        self.assertEqual(buses["B"].sent, [])

    def test_a_retry_keeps_the_sequence_number_and_goes_to_the_other_bus(self):
        link, buses = make_link(a_silent=True)
        ex = link.request(DEST, 0x06, 0x01)
        self.assertEqual([bus for bus, _ in ex.tries], ["A", "B"])
        self.assertEqual(ex.response_bus, "B")
        self.assertEqual(buses["A"].sent, buses["B"].sent)  # same frame, same sequence number
        self.assertEqual(ex.retries, 1)

    def test_a_silent_target_is_retried_c1_times_then_times_out(self):
        link, buses = make_link(a_silent=True, b_silent=True, retries=C1_MIN)
        ex = link.request(DEST, 0x06, 0x01)
        self.assertTrue(ex.timed_out)
        self.assertEqual(len(ex.tries), 1 + C1_MIN)
        self.assertEqual(len(buses["A"].sent) + len(buses["B"].sent), 1 + C1_MIN)
        self.assertEqual(len({f for b in buses.values() for f in b.sent}), 1)

    def test_a_single_bus_retries_on_that_bus(self):
        link, buses = make_link(a_silent=True, enable_b=False, retries=C1_MIN)
        ex = link.request(DEST, 0x06, 0x01)
        self.assertEqual(len(buses["A"].sent), 1 + C1_MIN)
        self.assertTrue(ex.timed_out)

    def test_tries_override_for_discovery(self):
        link, buses = make_link(a_silent=True, b_silent=True)
        link.request(DEST, 0x06, 0x01, tries=1)
        self.assertEqual(len(buses["A"].sent) + len(buses["B"].sent), 1)

    def test_a_bus_that_keeps_timing_out_while_the_other_answers_is_marked_failed(self):
        link, _ = make_link(a_silent=True, streak=2)
        for _ in range(4):
            link.request(DEST, 0x06, 0x01)
        self.assertEqual(link.failed, {"A"})
        ex = link.request(DEST, 0x06, 0x01)
        self.assertEqual([bus for bus, _ in ex.tries], ["B"])  # no more first tries on A

    def test_a_silent_target_does_not_mark_a_bus_failed(self):
        link, _ = make_link(a_silent=True, b_silent=True, streak=1)
        link.request(DEST, 0x06, 0x01)
        self.assertEqual(link.failed, set())

    def test_a_broadcast_goes_to_the_general_call_address(self):
        link, buses = make_link(enable_b=False)
        ex = link.request(DEST, 0x06, 0x01, broadcast=True)
        self.assertFalse(ex.timed_out)
        self.assertEqual([addr for addr, _ in buses["A"].general_call], [0])

    def test_sequence_numbers_count_up_modulo_64(self):
        link, _ = make_link(enable_b=False)
        with mock.patch("cm_link.time.sleep"):  # the T9 wait of the reused numbers
            seqs = [link.request(DEST, 0x06, 0x01).seq for _ in range(66)]
        self.assertEqual(seqs[:3], [1, 2, 3])
        self.assertEqual(seqs[63:66], [0, 1, 2])

    def t9_run(self, second_cmd: int):
        """64 Get Device ID requests at time 0, then at 1 s a request with `second_cmd` that
        reuses the first sequence number; returns (that exchange, the sleeps)."""
        now = [0.0]
        slept = []

        def sleep(seconds):
            slept.append(seconds)
            now[0] += seconds

        collector = FrameCollector()
        link = Link({"A": FakeBus(collector, "A")}, collector, timeout_s=TIMEOUT_S,
                    clock=lambda: now[0], test_mode=True)
        with mock.patch("cm_link.time.sleep", sleep):
            for _ in range(SEQUENCE_MODULUS):
                link.request(DEST, 0x06, 0x01)
            self.assertEqual(slept, [])
            now[0] += 1.0
            ex = link.request(DEST, 0x06, second_cmd)
        self.assertEqual(ex.seq, 1)
        return ex, slept

    def test_the_same_command_waits_out_t9_before_its_sequence_number_is_used_again(self):
        ex, slept = self.t9_run(0x01)
        self.assertEqual(slept, [T9_S - 1.0])
        self.assertEqual(ex.tries[0][1], T9_S)

    def test_another_command_may_reuse_the_sequence_number_within_t9(self):
        ex, slept = self.t9_run(0x04)
        self.assertEqual(slept, [])
        self.assertEqual(ex.tries[0][1], 1.0)

    def test_the_spec_limits_are_enforced(self):
        collector = FrameCollector()
        bus = {"A": FakeBus(collector, "A")}
        for retries in (C1_MIN - 1, C1_MAX + 1):
            with self.assertRaises(ValueError):
                Link(bus, collector, retries=retries)
        with self.assertRaises(ValueError):
            Link(bus, collector, timeout_s=0.3, test_mode=True)

    def test_production_timeouts_stay_within_t3(self):
        collector = FrameCollector()
        bus = {"A": FakeBus(collector, "A")}
        for bad in (0.0, -1.0, 0.001, T3_MIN_S - 0.001, T3_MAX_S + 0.001, float("nan"),
                    float("inf")):
            with self.assertRaises(ValueError, msg=str(bad)):
                Link(bus, collector, timeout_s=bad)
        for good in (T3_MIN_S, 0.1, T3_MAX_S):
            Link(bus, collector, timeout_s=good)
        # The test mode lifts only the lower bound; zero and negative stay refused.
        Link(bus, collector, timeout_s=0.001, test_mode=True)
        for bad in (0.0, -1.0, T3_MAX_S + 0.001):
            with self.assertRaises(ValueError, msg=str(bad)):
                Link(bus, collector, timeout_s=bad, test_mode=True)

    def test_alternation_follows_the_last_transmission_including_retries(self):
        # A is silent for the first request only: A, B (retry, answered). The next request goes
        # on the bus after B, which is A (Rule 5.1.1-5), not B again.
        link, buses = make_link()
        buses["A"].silent = True
        first = link.request(DEST, 0x06, 0x01)
        buses["A"].silent = False
        second = link.request(DEST, 0x06, 0x01)
        third = link.request(DEST, 0x06, 0x01)
        self.assertEqual([bus for bus, _ in first.tries], ["A", "B"])
        self.assertEqual([second.response_bus, third.response_bus], ["A", "B"])
        flat = [bus for ex in link.exchanges for bus, _ in ex.tries]
        self.assertEqual(flat, ["A", "B", "A", "B"])


class PerTargetBusTests(unittest.TestCase):
    """IPMB-B is enabled per target (Rule 4.1.8-2) and a bus fails per target."""

    OTHER = 0x84

    def test_ipmb_b_is_used_only_for_the_target_it_was_enabled_for(self):
        link, buses = make_link(enable_b=False)
        link.enable_bus("B", DEST)
        used_other = [link.request(self.OTHER, 0x06, 0x01).response_bus for _ in range(4)]
        self.assertEqual(used_other, ["A"] * 4)
        used = [link.request(DEST, 0x06, 0x01).response_bus for _ in range(4)]
        self.assertEqual(set(used), {"A", "B"})
        self.assertEqual(link.enabled_for(self.OTHER), ["A"])
        self.assertEqual(link.enabled_for(DEST), ["A", "B"])

    def test_a_timeout_for_one_target_marks_no_bus_failed_for_another(self):
        link, buses = make_link(a_silent=True, streak=1, enable_b=False)
        link.enable_bus("B", DEST)
        link.request(DEST, 0x06, 0x01)  # A silent, B answers: A suspect for DEST
        self.assertEqual(link.failed_for(DEST), {"A"})
        self.assertEqual(link.failed_for(self.OTHER), set())
        ex = link.request(self.OTHER, 0x06, 0x01, tries=1)
        self.assertEqual([bus for bus, _ in ex.tries], ["A"])  # B was never enabled for it

    def test_the_enable_events_name_the_exchange_count(self):
        link, _ = make_link(enable_b=False)
        link.request(DEST, 0x06, 0x01)
        link.enable_bus("B", DEST)
        link.enable_bus("B", DEST)  # again: no second event
        self.assertEqual(link.enable_events, [(1, DEST, "B")])


class ScriptedBus:
    """An IPMB interface whose IPMC answers with `responder(netfn, cmd, data)` -> the response
    bytes from the completion code on, or None to stay silent. `general_call_only` answers only
    the I2C general-call write (the Broadcast Get Device ID)."""

    def __init__(self, collector, responder, general_call_only=False):
        self.collector, self.responder = collector, responder
        self.general_call_only = general_call_only
        self.requests: list = []
        collector.register(self, "A")

    def _answer(self, frame):
        req = IpmbHeaderReq(data=frame)
        data = frame_payload(frame)
        self.requests.append((req.netfn, req.cmdid, data))
        payload = self.responder(req.netfn, req.cmdid, data)
        if payload is None:
            return
        rsp = IpmbHeaderRsp()
        rsp.from_req_header(req)
        self.collector.handle_frame(self, encode_ipmb_msg(rsp, bytes(payload)))

    def send_frame(self, frame):
        if not self.general_call_only:
            self._answer(frame)

    def raw_write(self, address, data):
        self._answer(data)


def make_session(responder, general_call_only=False):
    collector = FrameCollector()
    bus = ScriptedBus(collector, responder, general_call_only)
    link = Link({"A": bus}, collector, timeout_s=TIMEOUT_S, test_mode=True)
    args = argparse.Namespace(discovery_tries=1, poll_s=0.02, duration_s=0.05,
                              emu_bus_down=None, emu_bus_down_at_s=None, transport="test")
    session = Session(link, RunLog(link), args)
    session.ipmcs[DEST] = {"address": f"0x{DEST:02X}"}
    return session, bus


def check_named(session, name):
    found = [c for c in session.checks if c["name"] == name]
    assert len(found) == 1, (name, session.checks)
    return found[0]["ok"]


class DiscoveryTests(unittest.TestCase):
    def test_an_ipmc_that_answers_only_the_broadcast_form_is_discovered(self):
        session, _ = make_session(lambda nf, cmd, d: bytes([0x00, 0x42]), general_call_only=True)
        session.ipmcs.clear()
        self.assertEqual(session.discover([DEST]), [DEST])
        self.assertIn(DEST, session.ipmcs)
        self.assertTrue(check_named(session, "discovery found an IPMC"))

    def test_an_ipmc_that_answers_an_error_to_both_forms_is_not_discovered(self):
        session, _ = make_session(lambda nf, cmd, d: bytes([0xC1]))
        session.ipmcs.clear()
        self.assertEqual(session.discover([DEST]), [])
        self.assertFalse(check_named(session, "discovery found an IPMC"))

    def test_a_silent_address_is_not_discovered(self):
        session, _ = make_session(lambda nf, cmd, d: None)
        session.ipmcs.clear()
        self.assertEqual(session.discover([DEST]), [])


def fru_responder(size, access_by_words, image=None, area_cc=0x00, read_cc=0x00):
    """Storage commands of a FRU of `size` bytes; the byte at address n is n unless `image`."""
    image = image if image is not None else bytes(i & 0xFF for i in range(size))
    unit = 2 if access_by_words else 1
    log = []

    def respond(netfn, cmd, data):
        if (netfn, cmd) == (0x0A, 0x10):
            return bytes([area_cc, size & 0xFF, size >> 8, 1 if access_by_words else 0])
        if (netfn, cmd) == (0x0A, 0x11):
            offset, count = (data[1] | data[2] << 8) * unit, data[3] * unit
            log.append((data[1] | data[2] << 8, data[3]))
            if read_cc:
                return bytes([read_cc])
            chunk = image[offset:offset + count]
            return bytes([0x00, len(chunk) // unit]) + chunk
        return None

    return respond, log


class FruAccessTests(unittest.TestCase):
    def test_word_addressed_fru_is_read_with_offsets_and_counts_in_words(self):
        respond, log = fru_responder(32, access_by_words=True)
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertEqual(log, [(0, 8), (8, 8)])  # 16 bytes = 8 words per read
        self.assertEqual(session.ipmcs[DEST]["fru_bytes"], 32)
        self.assertTrue(check_named(session, "FRU read complete"))

    def test_byte_addressed_fru_is_read_with_offsets_and_counts_in_bytes(self):
        respond, log = fru_responder(40, access_by_words=False)
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertEqual(log, [(0, 16), (16, 16), (32, 8)])
        self.assertTrue(check_named(session, "FRU read complete"))

    def test_an_odd_size_in_words_is_read_whole_and_cut_to_the_size(self):
        respond, log = fru_responder(17, access_by_words=True, image=bytes(18))
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertEqual(log, [(0, 8), (8, 1)])
        self.assertEqual(session.ipmcs[DEST]["fru_bytes"], 17)

    def test_the_access_unit_comes_from_bit_0_of_the_area_info(self):
        self.assertEqual(fru_access_unit(bytes([0, 32, 0, 1])), 2)
        self.assertEqual(fru_access_unit(bytes([0, 32, 0, 0])), 1)
        self.assertEqual(fru_access_unit(bytes([0, 32, 0])), 1)

    def test_a_failed_read_makes_the_run_fail(self):
        respond, _ = fru_responder(32, access_by_words=False, read_cc=0xCB)
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertFalse(check_named(session, "FRU read complete"))

    def test_a_failed_area_info_makes_the_run_fail(self):
        respond, _ = fru_responder(32, access_by_words=False, area_cc=0xC1)
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertFalse(check_named(session, "Get FRU Inventory Area Info"))
        self.assertFalse(check_named(session, "FRU read complete"))

    def test_a_fru_that_does_not_decode_makes_the_run_fail(self):
        respond, _ = fru_responder(32, access_by_words=False, image=bytes([0xFF]) * 32)
        session, _ = make_session(respond)
        session.read_fru(DEST)
        self.assertTrue(check_named(session, "FRU read complete"))
        self.assertFalse(check_named(session, "FRU decoded"))


MC_LOCATOR_RECORD_ID = 0x0010
# Management Controller Device Locator record (IPMI v2.0 Table 37-8): header (ID, version, type,
# length), then slave address, channel, power state, capabilities, 3 reserved, entity ID and
# instance, OEM, and the 8-bit ASCII ID string (type/length byte C3h, "abc").
MC_LOCATOR = (bytes([MC_LOCATOR_RECORD_ID & 0xFF, MC_LOCATOR_RECORD_ID >> 8, 0x51, 0x12, 14])
              + bytes([0x20, 0x00, 0x00, 0x00, 0, 0, 0, 0x00, 0x00, 0x00, 0xC3]) + b"abc")


def sdr_responder(records, info_sensors=0):
    """Device SDR commands over `records` (a list of record bytes); record 0000h is the first."""
    ids = [r[0] | r[1] << 8 for r in records]

    def respond(netfn, cmd, data):
        if (netfn, cmd) == (0x04, 0x20):
            return bytes([0x00, info_sensors, 0x00])
        if (netfn, cmd) == (0x04, 0x22):
            return bytes([0x00, 0x01, 0x00])
        if (netfn, cmd) == (0x04, 0x21):
            wanted = data[2] | data[3] << 8
            index = 0 if wanted == 0 else ids.index(wanted)
            nxt = ids[index + 1] if index + 1 < len(ids) else 0xFFFF
            offset, count = data[4], data[5]
            return bytes([0x00, nxt & 0xFF, nxt >> 8]) + records[index][offset:offset + count]
        return None

    return respond


class SdrLocatorTests(unittest.TestCase):
    def test_the_locator_check_uses_the_record_id_of_the_header_not_the_lookup_alias(self):
        session, _ = make_session(sdr_responder([MC_LOCATOR]))
        session.read_sdr(DEST)  # looked up as record 0000h, the first record
        self.assertEqual(session.ipmcs[DEST]["sdr_locator_ids"], [MC_LOCATOR_RECORD_ID])
        session.ipmcs[DEST]["locator_record_id"] = MC_LOCATOR_RECORD_ID
        session.check_locator(DEST)
        self.assertTrue(session.ipmcs[DEST]["locator_ok"])
        self.assertTrue(check_named(session, "SDR walk complete"))

    def test_a_locator_id_that_is_not_in_the_sdr_fails(self):
        session, _ = make_session(sdr_responder([MC_LOCATOR]))
        session.read_sdr(DEST)
        session.ipmcs[DEST]["locator_record_id"] = 0x0000  # the alias is not a record ID
        session.check_locator(DEST)
        self.assertFalse(session.ipmcs[DEST]["locator_ok"])
        self.assertFalse(check_named(session, "device locator record ID matches the SDR"))

    def test_a_walk_that_stops_early_fails(self):
        def respond(netfn, cmd, data):
            if (netfn, cmd) == (0x04, 0x21):
                return bytes([0xCB])  # no such record
            return sdr_responder([MC_LOCATOR])(netfn, cmd, data)
        session, _ = make_session(respond)
        session.read_sdr(DEST)
        self.assertFalse(check_named(session, "SDR walk complete"))


class PollingChecksTests(unittest.TestCase):
    def poll(self, responder):
        session, _ = make_session(responder)
        session.poll(DEST, [{"number": 1, "name": "S1", "type": 1}])
        return session

    def test_a_good_poll_passes(self):
        session = self.poll(lambda nf, cmd, d: bytes([0, 1, 0xC0, 0, 0]))
        self.assertTrue(check_named(session, "polling answered"))
        self.assertTrue(check_named(session, "Get Self Test Results"))
        self.assertGreater(session.poll_stats["cycles"], 0)

    def test_an_error_completion_code_in_the_poll_fails(self):
        def respond(netfn, cmd, data):
            return bytes([0xC1]) if cmd == 0x2D else bytes([0, 0x55, 0])
        session = self.poll(respond)
        self.assertFalse(check_named(session, "polling answered"))
        self.assertGreater(session.poll_stats["bad_completion_codes"], 0)

    def test_a_failed_self_test_command_fails(self):
        session = self.poll(lambda nf, cmd, d: bytes([0xC1]) if cmd == 0x04
                            else bytes([0, 1, 0xC0, 0, 0]))
        self.assertFalse(check_named(session, "Get Self Test Results"))

    def test_the_summary_lists_transmissions_checks_and_run_parameters(self):
        import datetime
        session = self.poll(lambda nf, cmd, d: bytes([0, 1, 0xC0, 0, 0]))
        summary = session.summary(datetime.datetime.now())
        self.assertEqual(summary["run"], {"poll_s": 0.02, "duration_s_requested": 0.05})
        self.assertEqual(len(summary["transmissions"]), summary["totals"]["requests"])
        self.assertTrue(all(c["ok"] for c in summary["checks"]))
        self.assertEqual(summary["poll"]["sensors"], 1)


class DecoderTests(unittest.TestCase):
    def test_the_ipmi_version_has_the_major_in_the_low_nibble(self):
        self.assertEqual(ipmi_version(0x02), "2.0")  # IPMI v2.0
        self.assertEqual(ipmi_version(0x51), "1.5")  # IPMI v1.5
        self.assertEqual(ipmi_version(0x91), "1.9")

    def test_get_device_id_prints_the_version(self):
        rsp = bytes([0x00, 0x01, 0x80, 0x01, 0x02, 0x02, 0x09, 0x01, 0x02, 0x03, 0x04, 0x05])
        self.assertIn("ipmi 2.0", decode_device_id(rsp))


class EmuConnectionDrainTests(unittest.TestCase):
    def test_drain_frames_empties_every_bus_queue(self):
        import socket
        import time
        from ipmc_emu_interface import EmuConnection
        server = socket.socket()
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        conn = EmuConnection("127.0.0.1", server.getsockname()[1])
        peer, _ = server.accept()
        try:
            peer.sendall(b"A 20 1C\nB 20 1D\nA 01\n")
            for _ in range(200):
                if all(not conn._queues[b].empty() for b in "AB"):
                    break
                time.sleep(0.01)
            self.assertFalse(conn._queues["A"].empty())
            conn.drain_frames()
            self.assertIsNone(conn.read_frame("A", 0.01))
            self.assertIsNone(conn.read_frame("B", 0.01))
        finally:
            peer.close()
            server.close()
            conn._stop.set()
            conn._sock.close()


class AdapterResetTests(unittest.TestCase):
    """The conformance adapter's reset drops every frame of the old board."""

    def test_reset_drains_the_connection_queues_the_collector_and_the_pending_list(self):
        import pathlib
        import sys
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "conformance"))
        import adapter
        collector = FrameCollector()
        calls = []

        class Conn:
            errors = ["old"]

            def reset(self, timeout):
                calls.append("reset")

            def drain_frames(self):
                calls.append("drain")

        target = adapter.Target("emu", {}, collector, 0x82, None, Conn(), buses=("A",))
        target._pending.append(("A", b"old"))
        collector._queue.put(("A", b"old collected"))
        with mock.patch.object(adapter.time, "sleep"):
            target.reset()
        self.assertEqual(calls, ["reset", "drain", "drain"])
        self.assertEqual(target._pending, [])
        self.assertIsNone(collector.poll())


class LocatorRecordIdTests(unittest.TestCase):
    """Get Device Locator Record ID (section 10.1.3.11): cc, VSO id, record ID LS byte, MS byte."""

    def test_the_record_id_is_read_little_endian(self):
        self.assertEqual(locator_record_id(bytes([0x00, 0x03, 0x07, 0x01])), 0x0107)

    def test_a_short_response_has_no_record_id(self):
        self.assertIsNone(locator_record_id(bytes([0x00, 0x03, 0x07])))
        self.assertEqual(decode_locator_record_id(bytes([0x00, 0x03])), "short response (2 bytes)")

    def test_the_decoded_text_names_the_record(self):
        self.assertEqual(decode_locator_record_id(bytes([0x00, 0x03, 0x07, 0x00])),
                         "record 0007h")
        self.assertEqual(command_name(0x2C, 0x0D), "Get Device Locator Record ID")


THEIR_CAPS = bytes.fromhex("00 03 01 01 00 01 01 00")  # tier 01b = Tier-2; byte 3: two interfaces
OUR_CAPS = bytes.fromhex("00 03 00 00 00 22 00 00")  # Tier-1, one interface
SENSOR_REQUEST = b"\x02"  # Get Sensor Reading of sensor 2 (FRU Health)


def ex(netfn, cmd, request=b"", response=None, count=1, t=0.0, timed_out=False):
    """One entry of the run summary's `exchanges` (to the IPMC)."""
    return {"dest": DEST, "netfn": netfn, "cmd": cmd, "request": request.hex(" "),
            "broadcast": False, "response": (response or b"").hex(" "), "timed_out": timed_out,
            "count": count, "first_t_s": t}


def cap(netfn, cmd, data, response, bus="A", t=0.0):
    """One captured exchange (crosscheck.read_capture's dict)."""
    return {"t": t, "bus": bus, "netfn": netfn, "cmd": cmd, "data": data, "response": response}


def run_crosscheck(capture, exchanges, transmissions=None, events=None, ours_buses=None,
                   ipmcs=None, poll=None, run=None):
    """(report lines, unexpected count) of crosscheck.crosscheck over the given runs. Unless
    `ours_buses` names the response buses per command, our responses are on the captured buses
    (the bus line is then the same)."""
    import crosscheck
    commands = {}
    if ours_buses is None:
        buses = {}
        for e in capture:
            buses[e["bus"]] = buses.get(e["bus"], 0) + 1
        commands["all"] = {"buses": buses}
    for e in exchanges:
        key = f"{e['netfn']:02X}/{e['cmd']:02X}"
        buses = commands.setdefault(key, {"buses": {}})["buses"]
        for bus in (ours_buses or {}).get(key, []):
            buses[bus] = buses.get(bus, 0) + 1
    summary = {"exchanges": exchanges, "commands": commands,
               "transmissions": transmissions if transmissions is not None
               else [{"buses": "A"}] * len(exchanges),
               "bus_enable_events": events or [], "ipmcs": ipmcs or [], "poll": poll or {},
               "run": run or {}}
    return crosscheck.crosscheck(capture, summary)


class CrossCheckOneInterfaceTests(unittest.TestCase):
    """crosscheck.py: the IPMB-B-not-fitted differences are expected only when our run's Get VSO
    Capabilities reports one IPMB interface."""

    def run_crosscheck(self, our_ipmb_byte: int) -> int:
        capture = [cap(0x2C, 0x00, b"\x03", THEIR_CAPS),
                   cap(0x2C, 0x09, b"\x03\xff\x01", bytes.fromhex("00 03"), bus="B", t=1.0)]
        ours = bytearray(THEIR_CAPS)
        ours[3] = our_ipmb_byte
        _, unexpected = run_crosscheck(capture, [ex(0x2C, 0x00, b"\x03", bytes(ours))],
                                       ours_buses={"2C/00": ["A"]})
        return unexpected

    def test_one_interface_makes_the_missing_ipmb_b_expected(self):
        self.assertEqual(self.run_crosscheck(0x00), 0)

    def test_two_interfaces_keep_a_missing_set_ipmb_state_and_bus_b_unexpected(self):
        # No Set IPMB State and no response on IPMB-B although the IPMC has two interfaces.
        self.assertEqual(self.run_crosscheck(0x01), 2)

    def test_one_interface_with_a_transmission_on_b_is_unexpected(self):
        capture = [cap(0x2C, 0x00, b"\x03", THEIR_CAPS),
                   cap(0x2C, 0x09, b"\x03\xff\x01", bytes.fromhex("00 03"), bus="B", t=1.0)]
        ours = bytearray(THEIR_CAPS)
        ours[3] = 0x00
        lines, unexpected = run_crosscheck(capture, [ex(0x2C, 0x00, b"\x03", bytes(ours))],
                                           transmissions=[{"buses": "AB"}],
                                           ours_buses={"2C/00": ["A"]})
        self.assertEqual(unexpected, 1, lines)


class CrossCheckMaskTests(unittest.TestCase):
    """A reason excuses only the bits it names."""

    def caps(self, ours: bytes) -> int:
        capture = [cap(0x2C, 0x00, b"\x03", THEIR_CAPS)]
        # two interfaces and the bus counts equal, so only the response bytes are compared
        lines, unexpected = run_crosscheck(capture, [ex(0x2C, 0x00, b"\x03", ours)])
        return unexpected

    def patched(self, **changes) -> bytes:
        rsp = bytearray(OUR_CAPS)
        rsp[3] = THEIR_CAPS[3]  # two interfaces, as the capture
        for index, value in changes.items():
            rsp[int(index[1:])] = value
        return bytes(rsp)

    def test_tier_1_against_tier_2_is_expected(self):
        self.assertEqual(self.caps(self.patched(b2=0x00)), 0)

    def test_ffh_in_the_identifier_byte_is_unexpected(self):
        self.assertGreater(self.caps(self.patched(b2=0xFF)), 0)

    def test_a_reserved_bit_in_the_identifier_byte_is_unexpected(self):
        for reserved in (0x80, 0x40, 0x08, 0x04):
            self.assertGreater(self.caps(self.patched(b2=reserved)), 0, hex(reserved))

    def test_a_different_layer_is_unexpected(self):
        self.assertGreater(self.caps(self.patched(b2=0x10)), 0)

    def test_an_invalid_tier_value_is_unexpected(self):
        # Tier bits 11b are reserved: the tier reason holds only for our Tier-1 value.
        self.assertGreater(self.caps(self.patched(b2=0x03)), 0)

    def sensor(self, ours_state: int, theirs_state: int = 0x01, ours_status: int = 0xC0) -> int:
        capture = [cap(0x04, 0x2D, SENSOR_REQUEST, bytes([0, 0, 0xC0, theirs_state]))]
        _, unexpected = run_crosscheck(capture, [ex(0x04, 0x2D, SENSOR_REQUEST,
                                                    bytes([0, 0, ours_status, ours_state]))])
        return unexpected

    def test_a_defined_health_state_difference_is_expected(self):
        self.assertEqual(self.sensor(0x02), 0)

    def test_a_reserved_sensor_bit_is_unexpected(self):
        self.assertGreater(self.sensor(0x81), 0)
        self.assertGreater(self.sensor(0x05), 0)

    def test_an_invalid_health_value_is_unexpected(self):
        self.assertGreater(self.sensor(0x03), 0)
        self.assertGreater(self.sensor(0x00), 0)

    def fru_state(self, ours_state: int) -> int:
        capture = [cap(0x04, 0x2D, b"\x00", bytes([0, 0, 0xC0, 0x10]))]
        return run_crosscheck(capture, [ex(0x04, 0x2D, b"\x00",
                                           bytes([0, 0, 0xC0, ours_state]))])[1]

    def test_the_fru_state_activating_is_expected_but_not_a_reserved_or_second_state_bit(self):
        self.assertEqual(self.fru_state(0x02), 0)  # M1 while ours activates; the capture is M4
        self.assertGreater(self.fru_state(0x04), 0)  # bit 2 is reserved
        self.assertGreater(self.fru_state(0x08), 0)  # bit 3 is reserved
        self.assertGreater(self.fru_state(0x12), 0)  # two states at once
        self.assertGreater(self.fru_state(0x00), 0)  # no state

    def test_event_enable_is_excused_but_scanning_enabled_is_not(self):
        self.assertEqual(self.sensor(0x01, ours_status=0x40), 0)  # bit 7 differs: Rule 7-4
        self.assertGreater(self.sensor(0x01, ours_status=0x80), 0)  # bit 6 differs too


class CrossCheckLengthTests(unittest.TestCase):
    def caps(self, ours: bytes) -> int:
        capture = [cap(0x2C, 0x00, b"\x03", THEIR_CAPS)]
        _, unexpected = run_crosscheck(capture, [ex(0x2C, 0x00, b"\x03", ours)])
        return unexpected

    def two_interface_caps(self) -> bytearray:
        return bytearray(THEIR_CAPS)

    def test_a_capabilities_response_of_only_the_completion_code_is_unexpected(self):
        self.assertGreater(self.caps(b"\x00"), 0)

    def test_a_capabilities_response_shorter_than_the_mandatory_length_is_unexpected(self):
        self.assertGreater(self.caps(THEIR_CAPS[:7]), 0)

    def test_extra_trailing_bytes_after_the_mandatory_length_are_expected(self):
        self.assertEqual(self.caps(THEIR_CAPS + b"\x00"), 0)

    def test_a_short_sensor_reading_is_unexpected_and_a_missing_fifth_byte_is_expected(self):
        capture = [cap(0x04, 0x2D, b"\x00", bytes([0, 0, 0xC0, 0x01, 0x80]))]
        short = run_crosscheck(capture, [ex(0x04, 0x2D, b"\x00", bytes([0, 0, 0xC0]))])[1]
        fifth = run_crosscheck(capture, [ex(0x04, 0x2D, b"\x00", bytes([0, 0, 0xC0, 0x01]))])[1]
        self.assertGreater(short, 0)
        self.assertEqual(fifth, 0)

    def test_a_response_of_a_command_without_a_length_exception_must_match_in_length(self):
        capture = [cap(0x0A, 0x10, b"\x00", bytes([0, 0xF0, 0, 0]))]
        lines, unexpected = run_crosscheck(capture,
                                           [ex(0x0A, 0x10, b"\x00", bytes([0, 0xF0, 0]))])
        self.assertGreater(unexpected, 0)


class CrossCheckTimeoutTests(unittest.TestCase):
    def test_a_request_that_timed_out_in_ours_but_was_answered_in_the_capture_is_unexpected(self):
        capture = [cap(0x04, 0x2D, b"\x02", bytes([0, 0, 0xC0, 0x01]))]
        lines, unexpected = run_crosscheck(
            capture, [ex(0x04, 0x2D, b"\x02", timed_out=True, count=70)])
        self.assertGreater(unexpected, 0)
        self.assertTrue(any("timed out in ours" in line for line in lines), lines)

    def test_all_polls_timing_out_is_not_a_clean_run(self):
        capture = [cap(0x04, 0x2D, bytes([n]), bytes([0, 0, 0xC0, 0x01])) for n in range(7)]
        exchanges = [ex(0x04, 0x2D, bytes([n]), timed_out=True, count=10) for n in range(7)]
        _, unexpected = run_crosscheck(
            capture, exchanges, poll={"cycles": 10, "sensors": 7, "late_cycles": 0},
            run={"poll_s": 1.0, "duration_s_requested": 10.0})
        self.assertGreaterEqual(unexpected, 7)

    def test_a_request_answered_in_ours_but_timed_out_in_the_capture_is_unexpected(self):
        capture = [cap(0x04, 0x2D, b"\x02", None)]
        _, unexpected = run_crosscheck(capture, [ex(0x04, 0x2D, b"\x02",
                                                    bytes([0, 0, 0xC0, 0x01]))])
        self.assertGreater(unexpected, 0)

    def test_a_timeout_the_capture_never_sent_is_unexpected(self):
        capture = [cap(0x06, 0x01, b"", bytes([0, 1]))]
        _, unexpected = run_crosscheck(capture, [ex(0x06, 0x01, b"", bytes([0, 1])),
                                                 ex(0x06, 0x04, b"", timed_out=True)])
        self.assertGreater(unexpected, 0)


class CrossCheckAlternationTests(unittest.TestCase):
    EVENT = [{"after_exchanges": 2, "dest": DEST, "bus": "B"}]

    def tx(self, *buses):
        return [{"buses": b} for b in buses]

    def test_strict_alternation_after_set_ipmb_state_is_recognised(self):
        import crosscheck
        summary = {"transmissions": self.tx("A", "A", "B", "A", "B", "AB", "A"),
                   "bus_enable_events": self.EVENT}
        self.assertTrue(crosscheck.alternates_after_set_ipmb_state(summary))

    def test_one_answer_on_a_then_all_on_b_is_not_alternation(self):
        import crosscheck
        summary = {"transmissions": self.tx("A", "A", "A", "B", "B", "B"),
                   "bus_enable_events": self.EVENT}
        self.assertFalse(crosscheck.alternates_after_set_ipmb_state(summary))

    def test_a_retry_on_the_same_bus_breaks_the_alternation(self):
        import crosscheck
        summary = {"transmissions": self.tx("A", "A", "B", "BB", "A"),
                   "bus_enable_events": self.EVENT}
        self.assertFalse(crosscheck.alternates_after_set_ipmb_state(summary))

    def test_without_transmissions_or_an_enable_event_alternation_is_not_established(self):
        import crosscheck
        self.assertFalse(crosscheck.alternates_after_set_ipmb_state({}))
        self.assertFalse(crosscheck.alternates_after_set_ipmb_state(
            {"transmissions": self.tx("A", "B", "A")}))

    def bus_run(self, transmissions):
        capture = [cap(0x06, 0x01, b"", bytes([0, 1])), cap(0x06, 0x04, b"", bytes([0, 1]))]
        exchanges = [ex(0x06, 0x01, b"", bytes([0, 1])), ex(0x06, 0x04, b"", bytes([0, 1]))]
        return run_crosscheck(capture, exchanges, transmissions=transmissions,
                              events=self.EVENT, ours_buses={"06/01": ["A"], "06/04": ["B"]})

    def test_the_bus_difference_is_expected_only_when_ours_alternates(self):
        lines, unexpected = self.bus_run(self.tx("A", "A", "B", "A", "B"))
        self.assertEqual(unexpected, 0, lines)
        lines, unexpected = self.bus_run(self.tx("A", "A", "B", "B", "B"))
        self.assertEqual(unexpected, 1, lines)


class CrossCheckCountTests(unittest.TestCase):
    def sdr_capture(self, body_lengths):
        """Captured Get Device SDR rows: a manager reading each record in 16-byte chunks, at
        most 64 bytes, after one Reserve per record."""
        rows = []
        for index, body in enumerate(body_lengths):
            total = min(5 + body, 64)
            header = bytes([index, 0, 0x51, 0x02, body])
            rows.append(cap(0x04, 0x22, b"", bytes([0, 1, 0])))
            offset = 0
            while offset < total:
                count = min(16, total - offset)
                rsp = bytes([0, index + 1 & 0xFF, 0]) + (header + bytes(60))[offset:offset + count]
                rows.append(cap(0x04, 0x21, bytes([1, 0, index, 0, offset, count]), rsp))
                offset += count
        return rows

    def our_sdr(self, lengths, cancelled=0):
        info = {"address": f"0x{DEST:02X}", "sdr_record_lengths": lengths}
        chunks = sum(1 + -(-(n - 5) // 16) for n in lengths) + cancelled
        exchanges = [ex(0x04, 0x22, b"", bytes([0, 1, 0]), count=1 + cancelled),
                     ex(0x04, 0x21, bytes([1, 0, 0, 0, 0, 5]), bytes([0, 1, 0]) + bytes(5),
                        count=chunks - cancelled)]
        if cancelled:
            exchanges.append(ex(0x04, 0x21, bytes([1, 0, 0, 0, 0, 5]), bytes([0xC5]),
                                count=cancelled))
        return info, exchanges

    def counts(self, capture, info, exchanges):
        lines, unexpected = run_crosscheck(capture, exchanges, ipmcs=[info])
        return lines, unexpected

    def test_sdr_counts_that_follow_from_the_record_lengths_are_expected(self):
        capture = self.sdr_capture([42, 60, 20, 44])
        info, exchanges = self.our_sdr([19, 25])
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertEqual(unexpected, 0, lines)

    def test_a_cancelled_reservation_is_counted_in_both_commands(self):
        capture = self.sdr_capture([42, 60])
        info, exchanges = self.our_sdr([19, 25], cancelled=1)
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertEqual(unexpected, 0, lines)

    def test_a_read_the_capture_lost_in_front_of_a_later_read_is_noted_and_expected(self):
        capture = self.sdr_capture([42, 60])
        gap = [i for i, e in enumerate(capture)
               if e["cmd"] == 0x21 and e["data"][2] == 1 and e["data"][4] == 16][0]
        del capture[gap]  # the read at offset 16 of record 1
        info, exchanges = self.our_sdr([19, 25])
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertEqual(unexpected, 0, lines)
        self.assertTrue(any("lacks the read at offset 16 of record 0001h" in line
                            for line in lines), lines)

    def test_a_missing_last_read_of_a_captured_record_is_unexpected(self):
        capture = self.sdr_capture([42, 60])
        del capture[-1]  # the last read of the last record: no later read shows it was sent
        info, exchanges = self.our_sdr([19, 25])
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertGreater(unexpected, 0, lines)

    def test_a_missing_chunk_in_ours_is_unexpected(self):
        capture = self.sdr_capture([42, 60])
        info, exchanges = self.our_sdr([19, 25])
        exchanges[1]["count"] -= 1
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertGreater(unexpected, 0, lines)

    def test_extra_reserves_in_ours_are_unexpected(self):
        capture = self.sdr_capture([42, 60])
        info, exchanges = self.our_sdr([19, 25])
        exchanges[0]["count"] = 5
        lines, unexpected = self.counts(capture, info, exchanges)
        self.assertGreater(unexpected, 0, lines)

    def poll_run(self, readings, cycles=10, sensors=7, late=0):
        capture = [cap(0x04, 0x2D, bytes([n]), bytes([0, 0, 0xC0, 1])) for n in range(7)] * 2
        exchanges = [ex(0x04, 0x2D, bytes([n]), bytes([0, 0, 0xC0, 1]), count=readings // 7)
                     for n in range(7)]
        return run_crosscheck(
            capture, exchanges, poll={"cycles": cycles, "sensors": sensors, "late_cycles": late},
            run={"poll_s": 1.0, "duration_s_requested": 10.0})

    def test_the_poll_count_follows_from_the_run_parameters(self):
        lines, unexpected = self.poll_run(70)
        self.assertEqual(unexpected, 0, lines)

    def test_a_poll_count_that_is_not_cycles_times_sensors_is_unexpected(self):
        lines, unexpected = self.poll_run(63)
        self.assertGreater(unexpected, 0, lines)

    def test_a_cycle_count_that_does_not_fit_the_duration_is_unexpected(self):
        lines, unexpected = self.poll_run(63, cycles=9)
        self.assertEqual(unexpected, 1, lines)  # 9 x 7 = 63 reads, but 10 cycles fit 10 s
        lines, unexpected = self.poll_run(63, cycles=9, late=1)
        self.assertEqual(unexpected, 0, lines)  # a late cycle explains a missing one

    def test_a_count_difference_without_an_explanation_is_unexpected(self):
        capture = [cap(0x06, 0x01, b"", bytes([0, 1]))] * 2
        _, unexpected = run_crosscheck(capture, [ex(0x06, 0x01, b"", bytes([0, 1]))])
        self.assertGreater(unexpected, 0)


if __name__ == "__main__":
    unittest.main()
