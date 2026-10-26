"""test_app_cli.py - the line handling of app_cli.py against a fake transport.

Run from tools/app-port:  python -m unittest test_app_cli -v
"""

import unittest

import app_cli
import app_settings


class FakeTransport:
    """Records writes; read_line returns scripted lines, then None (a timeout)."""

    def __init__(self, answers):
        self.written = []
        self._answers = list(answers)
        self.closed = False

    def write(self, data):
        self.written.append(data)

    def read_line(self):
        return self._answers.pop(0) if self._answers else None

    def close(self):
        self.closed = True


class ClientTests(unittest.TestCase):
    def test_a_request_goes_out_with_a_tag_and_crlf_and_the_tagged_answer_returns(self):
        transport = FakeTransport(["#t0001 OK SAVED"])
        client = app_cli.Client(transport)
        self.assertEqual(client.request("SAVE"), "#t0001 OK SAVED")
        self.assertEqual(transport.written, [b"#t0001 SAVE\r\n"])

    def test_an_answer_with_another_tag_is_skipped(self):
        transport = FakeTransport(["#t0009 OK OLD", "#t0001 OK STATUS x"])
        self.assertEqual(app_cli.Client(transport).request("STATUS"), "#t0001 OK STATUS x")

    def test_tags_count_up_per_request_and_fit_the_protocol_tag(self):
        transport = FakeTransport(["#t0001 OK", "#t0002 OK"])
        client = app_cli.Client(transport)
        client.request("A")
        client.request("B")
        self.assertEqual(transport.written, [b"#t0001 A\r\n", b"#t0002 B\r\n"])
        last = f"#{app_settings.TAG_PREFIX}{app_settings.TAG_COUNTER_MODULUS - 1:04x} "
        self.assertRegex(last, app_cli.TAG_PATTERN)

    def test_without_tags_the_first_line_is_the_answer(self):
        transport = FakeTransport(["OK IPMC-APP 1"])
        client = app_cli.Client(transport, use_tags=False)
        self.assertEqual(client.request("HELLO"), "OK IPMC-APP 1")
        self.assertEqual(transport.written, [b"HELLO\r\n"])

    def test_a_timeout_returns_none(self):
        self.assertIsNone(app_cli.Client(FakeTransport([])).request("HELLO"))


class LineLengthTests(unittest.TestCase):
    def test_the_tag_counts_toward_the_limit_and_nothing_is_written_when_it_is_exceeded(self):
        limit = app_settings.line_max()
        # A line that fits on its own but not with the 7-byte tag.
        line = "X" * (limit - 3)
        transport = FakeTransport(["#t0001 OK"])
        client = app_cli.Client(transport)
        with self.assertRaises(app_cli.LineTooLong):
            client.request(line)
        self.assertEqual(transport.written, [])
        # The same line without a tag fits, and the longest tagged line goes out.
        plain = app_cli.Client(FakeTransport(["OK"]), use_tags=False)
        self.assertEqual(plain.request(line), "OK")
        longest = "X" * (limit - app_cli.Client.TAG_LEN)
        self.assertEqual(client.request(longest), "#t0001 OK")
        self.assertEqual(len(transport.written), 1)
        self.assertEqual(len(transport.written[0]), limit + len(app_cli.REQUEST_END))

    def test_interactive_mode_refuses_the_line_and_goes_on(self):
        limit = app_settings.line_max()
        lines = iter(["X" * (limit + 1), "SAVE", "quit"])
        transport = FakeTransport(["OK SAVED"])
        client = app_cli.Client(transport, use_tags=False)
        printed = []
        ok = app_cli.run_interactive(client, lambda _prompt: next(lines), printed.append)
        self.assertFalse(ok)
        self.assertEqual(client.refused, 1)
        self.assertEqual(transport.written, [b"SAVE\r\n"])

    def test_interactive_mode_does_not_send_a_tagged_line_that_is_too_long(self):
        limit = app_settings.line_max()
        lines = iter(["X" * (limit - 2), "quit"])
        transport = FakeTransport([])
        client = app_cli.Client(transport)
        printed = []
        self.assertFalse(app_cli.run_interactive(client, lambda _p: next(lines), printed.append))
        self.assertEqual(transport.written, [])
        self.assertEqual(client.refused, 1)
        self.assertIn("not sent", printed[-1])

    def test_main_returns_2_when_a_tagged_request_is_too_long(self):
        def factory(*_):
            raise AssertionError("the port must not open")

        line = "X" * (app_settings.line_max() - 2)
        self.assertEqual(app_cli.main([line], factory), 2)


class OkTests(unittest.TestCase):
    def test_ok_and_err_with_and_without_a_tag(self):
        rows = [
            ("OK HELLO", True),
            ("#t0001 OK HELLO", True),
            ("ERR ARGS x", False),
            ("#t0001 ERR PENDING a save is waiting", False),
            ("", False),
            (None, False),
        ]
        for answer, want in rows:
            self.assertEqual(app_cli.is_ok(answer), want, answer)


class RunTests(unittest.TestCase):
    def test_requests_print_and_report_failure(self):
        transport = FakeTransport(["OK A", "ERR UNKNOWN B"])
        client = app_cli.Client(transport, use_tags=False)
        lines = []
        self.assertFalse(app_cli.run_requests(client, ["A", "B", "C"], lines.append))
        self.assertEqual(
            lines, ["> A", "< OK A", "> B", "< ERR UNKNOWN B", "> C", "< (no answer)"]
        )

    def test_interactive_stops_at_quit_and_skips_blank_lines(self):
        transport = FakeTransport(["OK A"])
        client = app_cli.Client(transport, use_tags=False)
        typed = iter(["", "A", "quit", "never"])
        lines = []
        ok = app_cli.run_interactive(client, lambda _prompt: next(typed), lines.append)
        self.assertTrue(ok)
        self.assertEqual(lines, ["> A", "< OK A"])

    def test_interactive_ends_at_end_of_input(self):
        def eof(_prompt):
            raise EOFError

        client = app_cli.Client(FakeTransport([]), use_tags=False)
        self.assertTrue(app_cli.run_interactive(client, eof, lambda _line: None))


class MainTests(unittest.TestCase):
    def test_main_sends_each_argument_and_returns_zero_when_all_are_ok(self):
        made = []

        def factory(port, baud, timeout):
            made.append((port, baud, timeout))
            return FakeTransport(["OK A", "OK B"])

        self.assertEqual(app_cli.main(["--no-tags", "--port", "COM9", "A", "B"], factory), 0)
        self.assertEqual(made[0][0], "COM9")

    def test_main_returns_1_on_an_error_answer_and_closes_the_port(self):
        holder = []

        def factory(*_):
            holder.append(FakeTransport(["ERR UNKNOWN A"]))
            return holder[0]

        self.assertEqual(app_cli.main(["--no-tags", "A"], factory), 1)
        self.assertTrue(holder[0].closed)

    def test_a_request_longer_than_the_line_limit_is_refused_before_the_port_opens(self):
        def factory(*_):
            raise AssertionError("the port must not open")

        long_line = "X" * (app_settings.line_max() + 1)
        self.assertEqual(app_cli.main([long_line], factory), 2)


class SettingsTests(unittest.TestCase):
    def test_the_header_values_are_read(self):
        self.assertGreater(app_settings.baud_default(), 0)
        self.assertGreater(app_settings.line_max(), 16)

    def test_a_missing_header_gives_the_default(self):
        self.assertEqual(app_settings.define_from_header("X", 7, "no/such/file.h"), 7)


if __name__ == "__main__":
    unittest.main()
