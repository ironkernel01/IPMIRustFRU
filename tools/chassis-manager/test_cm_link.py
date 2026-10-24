"""test_cm_link.py - the Chassis Manager request policy against fake IPMB interfaces, the locator
record decoder and the cross-check's one-interface condition.

Run from tools/chassis-manager with the venv python:  python -m unittest test_cm_link -v
"""

import unittest
from unittest import mock

from pyipmi.interfaces.ipmb import IpmbHeaderReq, IpmbHeaderRsp, encode_ipmb_msg

from cm_commands import command_name, decode_locator_record_id, locator_record_id
from cm_link import C1_MAX, C1_MIN, SEQUENCE_MODULUS, T9_S, FrameCollector, Link

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
    link = Link(buses, collector, retries=retries, timeout_s=TIMEOUT_S, bus_fail_streak=streak)
    if enable_b:
        link.enable_bus("B")
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
                    clock=lambda: now[0])
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
            Link(bus, collector, timeout_s=0.3)


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


class CrossCheckOneInterfaceTests(unittest.TestCase):
    """crosscheck.py: the IPMB-B-not-fitted differences are expected only when our run's Get VSO
    Capabilities reports one IPMB interface."""

    THEIR_CAPS = bytes.fromhex("00 03 01 01 00 01 01 00")  # byte 3 = 01: two interfaces

    def run_crosscheck(self, our_ipmb_byte: int) -> int:
        import crosscheck
        capture = [
            {"t": 0.0, "bus": "A", "netfn": 0x2C, "cmd": 0x00, "data": b"\x03",
             "response": self.THEIR_CAPS},
            {"t": 1.0, "bus": "B", "netfn": 0x2C, "cmd": 0x09, "data": b"\x03\xff\x01",
             "response": bytes.fromhex("00 03")},
        ]
        ours = bytearray(self.THEIR_CAPS)
        ours[3] = our_ipmb_byte
        summary = {
            "exchanges": [{"dest": DEST, "netfn": 0x2C, "cmd": 0x00, "request": "03",
                           "broadcast": False, "response": ours.hex(" "), "timed_out": False,
                           "count": 1, "first_t_s": 0.0}],
            "commands": {"2C/00": {"buses": {"A": 1}}},
        }
        _, unexpected = crosscheck.crosscheck(capture, summary)
        return unexpected

    def test_one_interface_makes_the_missing_ipmb_b_expected(self):
        self.assertEqual(self.run_crosscheck(0x00), 0)

    def test_two_interfaces_keep_a_missing_set_ipmb_state_and_bus_b_unexpected(self):
        # No Set IPMB State and no response on IPMB-B although the IPMC has two interfaces.
        self.assertEqual(self.run_crosscheck(0x01), 2)


if __name__ == "__main__":
    unittest.main()
