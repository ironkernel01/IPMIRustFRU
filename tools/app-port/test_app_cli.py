"""test_app_cli.py - the parsing and the line handling of app_cli.py against a fake transport.

Run from tools/app-port:  python -m unittest test_app_cli -v
"""

import unittest

import app_cli
import app_settings


class FakeTransport:
    """Records writes; read_line returns scripted lines, then None (a timeout)."""

    def __init__(self, answers, available=()):
        self.written = []
        self._answers = list(answers)
        self._available = list(available)
        self.closed = False

    def write(self, data):
        self.written.append(data)

    def read_line(self):
        return self._answers.pop(0) if self._answers else None

    def read_available(self):
        return self._available.pop(0) if self._available else b""

    def close(self):
        self.closed = True


# What the IPMC prints for `STATUS` typed at the prompt: the prompt and the echo share a line.
STATUS_ANSWER = [
    "app> STATUS",
    "IPMB_ADDRESS: 82h",
    "GA: 1",
    "FRU_STATE: M4 FRU Active",
    "OK",
]


class ParsingTests(unittest.TestCase):
    def test_prompt_and_echo_lines_are_skipped_data_is_collected_and_ok_ends_the_answer(self):
        transport = FakeTransport(STATUS_ANSWER + ["GA: 9", "OK"])
        client = app_cli.Client(transport, use_tags=False)
        answer = client.request("STATUS")
        self.assertTrue(answer.ok)
        self.assertEqual(
            answer.data,
            [("IPMB_ADDRESS", "82h"), ("GA", "1"), ("FRU_STATE", "M4 FRU Active")],
        )
        self.assertEqual(answer.final, "OK")
        # The line after the final line belongs to the next request: it was not read.
        self.assertEqual(transport.read_line(), "GA: 9")

    def test_an_error_line_ends_the_answer_with_its_code_and_sentence(self):
        transport = FakeTransport(
            ["app> FRU GET X", "ERROR ARGS: unknown field 'X'; FRU FIELDS lists them"]
        )
        answer = app_cli.Client(transport, use_tags=False).request("FRU GET X")
        self.assertFalse(answer.ok)
        self.assertEqual(answer.code, "ARGS")
        self.assertEqual(answer.sentence, "unknown field 'X'; FRU FIELDS lists them")
        self.assertEqual(answer.data, [])

    def test_an_empty_value_and_the_two_blank_empty_field_line_are_data_lines(self):
        transport = FakeTransport(["ASSET_TAG: ", "PRODUCT_FILE_ID:  (max 16)", "OK"])
        answer = app_cli.Client(transport, use_tags=False).request("FRU FIELDS")
        self.assertEqual(answer.data, [("ASSET_TAG", ""), ("PRODUCT_FILE_ID", " (max 16)")])

    def test_help_rows_are_lines_of_the_answer_but_not_data(self):
        rows = ["  INFO                           - Firmware name", "  ?  - Same as HELP"]
        transport = FakeTransport(rows + ["OK"])
        answer = app_cli.Client(transport, use_tags=False).request("HELP")
        self.assertEqual(answer.lines, rows)
        self.assertEqual(answer.data, [])

    def test_a_data_key_that_starts_like_ok_does_not_end_the_answer(self):
        transport = FakeTransport(["OKAY_COUNT: 3", "OK"])
        answer = app_cli.Client(transport, use_tags=False).request("X")
        self.assertEqual(answer.data, [("OKAY_COUNT", "3")])

    def test_is_final_accepts_ok_ok_with_a_tag_and_error_only(self):
        rows = [
            ("OK", True),
            ("OK #a7", True),
            ("ERROR ARGS: x", True),
            ("ERROR #a7 TOO_LONG: x", True),
            ("OKAY: 1", False),
            ("ERROR_COUNT: 1", False),
            ("GA: 1", False),
            ("  OK", False),
            ("", False),
        ]
        for line, want in rows:
            self.assertEqual(app_cli.is_final(line), want, line)

    def test_the_final_line_words_are_read(self):
        rows = [
            ("OK", ("OK", None, None, None)),
            ("OK #t0001", ("OK", "t0001", None, None)),
            ("ERROR ARGS: bad", ("ERROR", None, "ARGS", "bad")),
            ("ERROR #a7 TOO_LONG: BOARD_SERIAL is at most 24 characters",
             ("ERROR", "a7", "TOO_LONG", "BOARD_SERIAL is at most 24 characters")),
        ]
        for final, (status, tag, code, sentence) in rows:
            answer = app_cli.Answer([], final)
            self.assertEqual((answer.status, answer.tag, answer.code, answer.sentence),
                             (status, tag, code, sentence), final)


class TagTests(unittest.TestCase):
    def test_a_request_goes_out_with_a_tag_and_crlf_and_the_tagged_answer_returns(self):
        transport = FakeTransport(["app> #t0001 FRU SAVE", "SAVE: done", "OK #t0001"])
        client = app_cli.Client(transport, counter_start=0)
        answer = client.request("FRU SAVE")
        self.assertEqual(answer.final, "OK #t0001")
        self.assertEqual(answer.data, [("SAVE", "done")])
        self.assertEqual(transport.written, [b"#t0001 FRU SAVE\r\n"])

    def test_an_answer_with_another_tag_is_skipped_with_its_data_lines(self):
        transport = FakeTransport(["GA: 0", "OK #t0009", "GA: 1", "OK #t0001"])
        answer = app_cli.Client(transport, counter_start=0).request("STATUS")
        self.assertEqual(answer.data, [("GA", "1")])
        self.assertEqual(answer.final, "OK #t0001")

    def test_an_error_answer_is_matched_by_its_tag_too(self):
        transport = FakeTransport(["ERROR #t0001 UNKNOWN: unknown command 'X'; type HELP or ?"])
        answer = app_cli.Client(transport, counter_start=0).request("X")
        self.assertEqual((answer.ok, answer.code, answer.tag), (False, "UNKNOWN", "t0001"))

    def test_tags_count_up_per_request_and_fit_the_protocol_tag(self):
        transport = FakeTransport(["OK #t0001", "OK #t0002"])
        client = app_cli.Client(transport, counter_start=0)
        client.request("A")
        client.request("B")
        self.assertEqual(transport.written, [b"#t0001 A\r\n", b"#t0002 B\r\n"])
        last = f"#{app_settings.TAG_PREFIX}{app_settings.TAG_COUNTER_MODULUS - 1:04x} "
        self.assertRegex(last, app_cli.TAG_PATTERN)

    def test_without_tags_the_first_final_line_is_the_answer(self):
        transport = FakeTransport(["INFO_X: 1", "OK"])
        client = app_cli.Client(transport, use_tags=False)
        self.assertEqual(client.request("INFO").final, "OK")
        self.assertEqual(transport.written, [b"INFO\r\n"])

    def test_a_timeout_returns_none(self):
        self.assertIsNone(app_cli.Client(FakeTransport([])).request("INFO"))
        # Data lines without a final line are a timeout too.
        self.assertIsNone(app_cli.Client(FakeTransport(["GA: 1"])).request("STATUS"))


    def test_the_tags_of_a_run_start_from_the_clock_not_from_one(self):
        # Two runs a second apart do not share a first tag, and the tag stays inside the pattern.
        starts = []
        for now in (1_000_000, 1_000_001):
            real = app_settings.time.time
            app_settings.time.time = lambda value=now: value
            try:
                starts.append(app_settings.tag_counter_start())
            finally:
                app_settings.time.time = real
        self.assertNotEqual(starts[0], starts[1])
        transport = FakeTransport(["OK"])
        client = app_cli.Client(transport, counter_start=starts[0])
        client.request("A")
        self.assertRegex(transport.written[0].decode("ascii"), app_cli.TAG_PATTERN)
        self.assertNotEqual(transport.written[0], b"#t0001 A\r\n")

    def test_an_untagged_error_for_a_lost_request_ends_this_requests_answer(self):
        for code, sentence in [
            ("LINE_DAMAGED", "3 bytes lost on the port; the line was dropped"),
            ("BAD_CHAR", "the line holds a byte that is not printable ASCII; it was dropped"),
            ("LINE_TOO_LONG", "the line is longer than 128 bytes; it was dropped"),
        ]:
            transport = FakeTransport(["GA: 0", "OK #t0009", f"ERROR {code}: {sentence}", "OK #t0001"])
            answer = app_cli.Client(transport, counter_start=0).request("STATUS")
            self.assertEqual((answer.ok, answer.code, answer.tag), (False, code, None), code)
            # The next request starts after it, so a late answer is skipped by its tag.
            self.assertEqual(transport.read_line(), "OK #t0001")

    def test_an_untagged_error_of_another_code_does_not_end_the_answer(self):
        transport = FakeTransport(["ERROR ARGS: a tag is # and 1 to 8 letters or digits", "OK #t0001"])
        answer = app_cli.Client(transport, counter_start=0).request("STATUS")
        self.assertEqual(answer.final, "OK #t0001")


class LineLengthTests(unittest.TestCase):
    def test_the_tag_counts_toward_the_limit_and_nothing_is_written_when_it_is_exceeded(self):
        limit = app_settings.line_max()
        # A line that fits on its own but not with the 7-byte tag.
        line = "X" * (limit - 3)
        transport = FakeTransport(["OK #t0001"])
        client = app_cli.Client(transport, counter_start=0)
        with self.assertRaises(app_cli.LineTooLong):
            client.request(line)
        self.assertEqual(transport.written, [])
        # The same line without a tag fits, and the longest tagged line goes out.
        plain = app_cli.Client(FakeTransport(["OK"]), use_tags=False)
        self.assertEqual(plain.request(line).final, "OK")
        longest = "X" * (limit - app_cli.Client.TAG_LEN)
        self.assertEqual(client.request(longest).final, "OK #t0001")
        self.assertEqual(len(transport.written), 1)
        self.assertEqual(len(transport.written[0]), limit + len(app_cli.REQUEST_END))

    def test_interactive_mode_refuses_the_line_and_goes_on(self):
        limit = app_settings.line_max()
        lines = iter(["X" * (limit + 1), "FRU SAVE", "quit"])
        transport = FakeTransport(["OK"])
        client = app_cli.Client(transport, use_tags=False)
        printed = []
        ok = app_cli.run_interactive(client, lambda _prompt: next(lines), printed.append)
        self.assertFalse(ok)
        self.assertEqual(client.refused, 1)
        self.assertEqual(transport.written, [b"FRU SAVE\r\n"])

    def test_interactive_mode_does_not_send_a_tagged_line_that_is_too_long(self):
        limit = app_settings.line_max()
        lines = iter(["X" * (limit - 2), "quit"])
        transport = FakeTransport([])
        client = app_cli.Client(transport, counter_start=0)
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


class NonAsciiTests(unittest.TestCase):
    def test_a_non_ascii_request_is_refused_and_nothing_is_written(self):
        transport = FakeTransport(["OK"])
        client = app_cli.Client(transport, use_tags=False)
        with self.assertRaises(app_cli.NotAscii):
            client.request("FRU SET ASSET_TAG caf\u00e9")
        self.assertEqual(transport.written, [])

    def test_run_requests_prints_the_message_and_goes_on(self):
        transport = FakeTransport(["OK"])
        client = app_cli.Client(transport, use_tags=False)
        lines = []
        self.assertFalse(app_cli.run_requests(client, ["X caf\u00e9", "INFO"], lines.append))
        self.assertEqual(lines, ["(not sent: a request holds a character that is not ASCII)", "OK"])
        self.assertEqual(client.refused, 1)

    def test_main_returns_2_with_a_message_and_does_not_open_the_port(self):
        def factory(*_):
            raise AssertionError("the port must not open")

        self.assertEqual(app_cli.main(["FRU SET ASSET_TAG caf\u00e9"], factory), 2)


class SerialOpenTests(unittest.TestCase):
    def test_opening_the_port_clears_what_arrived_before(self):
        import sys
        import types

        calls = []

        class FakeSerial:
            def __init__(self, port, baud, timeout):
                calls.append(("open", port, baud, timeout))

            def reset_input_buffer(self):
                calls.append(("reset_input_buffer",))

        fake = types.ModuleType("serial")
        fake.Serial = FakeSerial
        saved = sys.modules.get("serial")
        sys.modules["serial"] = fake
        try:
            app_cli.SerialTransport("COM9", 115200, 1.0)
        finally:
            if saved is None:
                del sys.modules["serial"]
            else:
                sys.modules["serial"] = saved
        self.assertEqual(calls, [("open", "COM9", 115200, 1.0), ("reset_input_buffer",)])


class RunTests(unittest.TestCase):
    def test_requests_print_the_data_lines_and_the_final_line_and_report_failure(self):
        transport = FakeTransport(
            ["app> A", "KEY: 1", "OK", "app> B", "ERROR UNKNOWN: unknown command 'B'; type HELP or ?"]
        )
        client = app_cli.Client(transport, use_tags=False)
        lines = []
        self.assertFalse(app_cli.run_requests(client, ["A", "B", "C"], lines.append))
        self.assertEqual(
            lines,
            [
                "KEY: 1",
                "OK",
                "ERROR UNKNOWN: unknown command 'B'; type HELP or ?",
                "(no answer)",
            ],
        )

    def test_interactive_stops_at_quit_and_skips_blank_lines(self):
        transport = FakeTransport(["V: 1", "OK"])
        client = app_cli.Client(transport, use_tags=False)
        typed = iter(["", "A", "quit", "never"])
        lines = []
        ok = app_cli.run_interactive(client, lambda _prompt: next(typed), lines.append)
        self.assertTrue(ok)
        self.assertEqual(lines, ["V: 1", "OK"])

    def test_interactive_ends_at_end_of_input(self):
        def eof(_prompt):
            raise EOFError

        client = app_cli.Client(FakeTransport([]), use_tags=False)
        self.assertTrue(app_cli.run_interactive(client, eof, lambda _line: None))


class RawTests(unittest.TestCase):
    def test_raw_passes_keys_to_the_port_and_bytes_back_until_ctrl_right_bracket(self):
        transport = FakeTransport([], available=[b"app> ", b"", b"INFO\r\n"])
        keys = iter([b"I", None, b"N", app_cli.RAW_QUIT, b"never"])
        shown = []
        app_cli.run_raw(transport, lambda: next(keys), shown.append)
        self.assertEqual(transport.written, [b"I", b"N"])
        self.assertEqual(shown, [b"app> ", b"INFO\r\n"])

    def test_raw_ends_when_idle_says_so(self):
        transport = FakeTransport([])
        calls = []

        def idle():
            calls.append(1)
            return len(calls) == 3

        app_cli.run_raw(transport, lambda: None, lambda _data: None, idle)
        self.assertEqual(len(calls), 3)

    def test_main_raw_uses_the_terminal_and_closes_the_port(self):
        holder = []

        def factory(*_):
            holder.append(FakeTransport([], available=[b"x"]))
            return holder[0]

        keys = iter([b"A", app_cli.RAW_QUIT])
        self.assertEqual(app_cli.main(["--raw"], factory, lambda: next(keys)), 0)
        self.assertEqual(holder[0].written, [b"A"])
        self.assertTrue(holder[0].closed)


class MainTests(unittest.TestCase):
    def test_main_sends_each_argument_and_returns_zero_when_all_are_ok(self):
        made = []

        def factory(port, baud, timeout):
            made.append((port, baud, timeout))
            return FakeTransport(["OK", "OK"])

        self.assertEqual(app_cli.main(["--no-tags", "--port", "COM9", "A", "B"], factory), 0)
        self.assertEqual(made[0][0], "COM9")

    def test_main_returns_1_on_an_error_answer_and_closes_the_port(self):
        holder = []

        def factory(*_):
            holder.append(FakeTransport(["ERROR UNKNOWN: unknown command 'A'; type HELP or ?"]))
            return holder[0]

        self.assertEqual(app_cli.main(["--no-tags", "A"], factory), 1)
        self.assertTrue(holder[0].closed)

    def test_main_returns_1_when_nothing_answers(self):
        self.assertEqual(app_cli.main(["--no-tags", "A"], lambda *_: FakeTransport([])), 1)

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
