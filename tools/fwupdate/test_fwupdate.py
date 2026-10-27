"""test_fwupdate.py - fwupdate.py against a fake bootloader (no board, no serial port).

Run from tools/fwupdate:  python -m unittest test_fwupdate -v
"""

import contextlib
import hashlib
import io
import os
import tempfile
import unittest
from pathlib import Path

import fwu_settings
import fwupdate

PROMPT = "BOOT> "
LOW = 0x0800_C000
HEADER = 0x200  # offset of the header in the image


def good_image(length=0x400):
    """A valid image of `length` bytes: vectors, header, digest (computed here, independent of the
    tool), filler. Bytes 0x300..0x340 stay FFh so one chunk is skipped by the re-chunker."""
    image = bytearray(b"\xAA" * length)
    image[0x300:0x340] = b"\xFF" * 0x40
    image[0:4] = (0x2001_0000).to_bytes(4, "little")
    image[4:8] = (LOW + 0x241).to_bytes(4, "little")
    image[HEADER : HEADER + 64] = bytes(64)
    image[HEADER : HEADER + 4] = (0x434D_5049).to_bytes(4, "little")
    image[HEADER + 4 : HEADER + 6] = (1).to_bytes(2, "little")
    image[HEADER + 6 : HEADER + 8] = (64).to_bytes(2, "little")
    image[HEADER + 8 : HEADER + 12] = length.to_bytes(4, "little")
    image[HEADER + 0x14 : HEADER + 0x18] = LOW.to_bytes(4, "little")
    digest = hashlib.sha256(bytes(image[:0x220]) + bytes(image[0x240:length])).digest()
    image[HEADER + 0x20 : HEADER + 0x40] = digest
    return image


def hex_text(image, low=LOW, per_record=16, extra=()):
    """Intel HEX text for `image`: an 04 record, data records, any `extra` lines, the 01 record."""
    lines = [fwupdate.record_line(4, 0, (low >> 16).to_bytes(2, "big"))]
    for offset in range(0, len(image), per_record):
        lines.append(fwupdate.record_line(0, (low + offset) & 0xFFFF, image[offset : offset + per_record]))
    lines.extend(extra)
    lines.append(fwupdate.record_line(1, 0, b""))
    return "\n".join(lines) + "\n"


def checked(text):
    records = fwupdate.parse_records(text)
    return fwupdate.build_image(fwupdate.build_memory(records))


class FakeBoot:
    """Plays the bootloader. `wake_after` WAKEUP lines are ignored before the OK (it stays silent
    before the port choice); a command is echoed as `BOOT> <command>`; record mode has no echo.
    `record_answers` maps a record number (1 based) to the list of answers for its attempts."""

    def __init__(
        self, wake_after=1, record_answers=None, erase_answer=True, preamble=(), double_ok=False
    ):
        self._double_ok = double_ok
        self.now = 0.0
        self.written = []
        self.baud = None
        self.closed = False
        self._queue = list(preamble)
        self._wakes = 0
        self._wake_after = wake_after
        self._awake = False
        self._records = False
        self._pending = False
        self._record_number = 0
        self._record_answers = record_answers or {}
        self._erase_answer = erase_answer
        self.record_attempts = []
        self._aborted = False
        self._skip_04 = False

    def clock(self):
        return self.now

    def write(self, data):
        line = data.decode("ascii")
        assert line.endswith("\r\n") and line.strip(), "every line ends in CR LF and is not empty"
        line = line.rstrip("\r\n")
        self.written.append(line)
        if not self._awake:
            if line.upper() == "WAKEUP":
                self._wakes += 1
                if self._wakes >= self._wake_after:
                    self._awake = True
                    self._queue.append("OK")
                    if self._double_ok:
                        self._queue.append("OK")  # the answer to a WAKEUP already in flight
            return
        if self._records:
            self._on_record(line)
        else:
            self._on_command(line)

    def _reply(self, *lines):
        self._queue.extend(lines)

    def _on_command(self, line):
        self._queue.append(PROMPT + line)
        if self._pending and line != "CONFIRM":
            self._pending = False
            self._reply("ERR CANCELLED")
            return
        if line == "WAKEUP":
            self._reply("OK")
        elif line in ("ERASE APP", "ERASE FRU"):
            self._pending = line.split()[1]
            self._reply("Erase sectors? Type CONFIRM", "OK PENDING")
        elif line == "CONFIRM":
            if self._pending and self._erase_answer:
                self._reply(f"OK ERASED {self._pending} 7000 ms")
            elif not self._pending:
                self._reply("ERR NOTHING_PENDING")
            self._pending = False
        elif line == "LOAD":
            self._records = True
            if self._aborted:
                self._skip_04 = True  # the resent 04 record is not a new record number
            else:
                self._record_number = 0
            self._reply("OK LOAD APP: send records")
        elif line == "INFO":
            self._reply("Bootloader: ipmc-boot v0.1.0", "OK")
        elif line == "VERIFY":
            self._reply("OK VERIFY valid SHA-256 " + "0" * 64 + " 40 ms")
        elif line == "RUN":
            self._reply("OK RUN")
        else:
            self._reply("OK")

    def _on_record(self, line):
        if self._skip_04:
            self._skip_04 = False
            if line[7:9] == "04":
                self._reply("OK")
                return
        self._record_number += 1
        self.record_attempts.append(self._record_number)
        answers = self._record_answers.get(self._record_number)
        if answers:
            answer = answers.pop(0)
            self._reply(answer)
            if answer == "ERR ABORTED":
                self._records = False  # the line ended record mode; ERASE APP stays in force
                self._aborted = True
                self._record_number -= 1
                return
            if answer.startswith("ERR") and answers:
                self._record_number -= 1  # the record is sent again
            return
        if line[7:9] == "01":
            self._records = False
            self._reply("OK END 3 records 96 bytes, image valid")
        else:
            self._reply("OK")

    def read_line(self, timeout):
        if self._queue:
            return self._queue.pop(0)
        self.now += timeout
        return None

    def wait_prompt(self, prompt, timeout):
        assert prompt == PROMPT
        return True

    def settle(self, seconds):
        self._queue.clear()
        self.now += seconds

    def close(self):
        self.closed = True

    def factory(self, port, baud):
        self.port, self.baud = port, baud
        return self


class FakeRuntime:
    """The runtime port: answers each command line with the lines `answers` holds for it."""

    def __init__(self, answers):
        self._answers = answers
        self.written = []
        self._queue = []

    def write(self, data):
        line = data.decode("ascii").rstrip("\r\n")
        self.written.append(line)
        self._queue.extend(self._answers.get(line, []))

    def read_line(self, timeout):
        return self._queue.pop(0) if self._queue else None

    def close(self):
        pass


def run_main(fake, *argv):
    out = []
    with contextlib.redirect_stdout(io.StringIO()):
        code = fwupdate.main(list(argv), fake.factory, out.append, fake.clock, lambda prompt: "")
    return code, out


class HexAndPrecheckTests(unittest.TestCase):
    def test_good_image_passes_and_round_trips(self):
        image = good_image()
        low, built = checked(hex_text(image))
        self.assertEqual((low, bytes(built)), (LOW, bytes(image)))
        fwupdate.precheck(low, built)

    def refused(self, text, fragment):
        with self.assertRaises(fwupdate.Refused) as caught:
            low, built = checked(text)
            fwupdate.precheck(low, built)
        self.assertIn(fragment, str(caught.exception))

    def test_broken_files_are_refused_naming_the_rule(self):
        def edited(offset, value):
            image = good_image()
            image[offset : offset + len(value)] = value
            return hex_text(image)

        def damaged_checksum():
            lines = hex_text(good_image()).splitlines()
            lines[2] = lines[2][:-2] + ("00" if lines[2][-2:] != "00" else "01")
            return chr(10).join(lines) + chr(10)

        cases = {
            "bad checksum": (damaged_checksum(), "checksum"),
            "type 02": (hex_text(good_image(), extra=[fwupdate.record_line(2, 0, b"\x00\x10")]), "type 02"),
            "type 03": (hex_text(good_image(), extra=[fwupdate.record_line(3, 0, bytes(4))]), "type 03"),
            "magic": (edited(HEADER, b"\x00"), "magic"),
            "length": (edited(HEADER + 8, (0x401).to_bytes(4, "little")), "length"),
            "length small": (edited(HEADER + 8, (0x100).to_bytes(4, "little")), "length"),
            "stack": (edited(0, (0x2002_0000).to_bytes(4, "little")), "stack"),
            "stack alignment": (edited(0, (0x2001_0004).to_bytes(4, "little")), "stack"),
            "reset even": (edited(4, (LOW + 0x240).to_bytes(4, "little")), "reset"),
            "reset low": (edited(4, (LOW + 0x101).to_bytes(4, "little")), "reset"),
            "vector address": (edited(HEADER + 0x14, bytes(4)), "vector table address"),
            "digest": (edited(HEADER + 0x20, b"\x00"), "SHA-256"),
            "payload": (edited(0x100, b"\x00"), "SHA-256"),
        }
        for name, (text, fragment) in cases.items():
            with self.subTest(name):
                self.refused(text, fragment)

    def test_byte_outside_the_app_region_is_refused(self):
        outside = fwupdate.record_line(0, 0x7FFF, b"\x00")
        text = hex_text(good_image()).replace(
            fwupdate.record_line(1, 0, b""),
            fwupdate.record_line(4, 0, b"\x07\xFF") + "\n" + outside + "\n" + fwupdate.record_line(1, 0, b""),
        )
        self.refused(text, "outside the APP region")
        end_of_flash = hex_text(bytes(16), low=0x0808_0000)
        self.refused(end_of_flash, "outside the APP region")

    def test_other_file_faults(self):
        self.refused("", "end-of-file")
        self.refused("hello\n", "not an Intel HEX")
        self.refused(hex_text(good_image()) + fwupdate.record_line(0, 0, b"\x00") + "\n", "after the end")
        double = hex_text(good_image(), extra=[fwupdate.record_line(0, 0xC000, b"\x11")])
        self.refused(double, "twice")


class RechunkTests(unittest.TestCase):
    def test_aligned_records_boundaries_and_skipped_blank_chunks(self):
        image = good_image(0x400)
        lines = fwupdate.rechunk(LOW, image)
        records = fwupdate.parse_records("\n".join(lines))
        self.assertEqual(records[0][0], 4)
        self.assertEqual(records[-1][0], 1)
        data = [r for r in records if r[0] == 0]
        self.assertTrue(all(len(d) == 32 and (a & 31) == 0 for _, a, d in data))
        self.assertNotIn(LOW + 0x300 & 0xFFFF, [a for _, a, _ in data])  # all-FFh chunk skipped
        self.assertEqual(len(data), 0x400 // 32 - 2)
        self.assertEqual(bytes(fwupdate.build_image(fwupdate.build_memory(records))[1]), bytes(image))

    def test_04_record_at_every_64k_boundary_and_trimmed_ends(self):
        low = 0x0800_FFC0
        image = bytearray(range(1, 129))  # 128 bytes cross 0x0801_0000
        lines = fwupdate.rechunk(low, image)
        records = fwupdate.parse_records("\n".join(lines))
        self.assertEqual([r[0] for r in records if r[0] == 4], [4, 4])
        self.assertEqual([r[2] for r in records if r[0] == 4], [b"\x08\x00", b"\x08\x01"])
        self.assertEqual(bytes(fwupdate.build_image(fwupdate.build_memory(records))[1]), bytes(image))
        odd = fwupdate.rechunk(LOW + 4, bytearray(b"\x01" * 40))  # starts and ends inside a chunk
        data = [r for r in fwupdate.parse_records("\n".join(odd)) if r[0] == 0]
        self.assertEqual([(a, len(d)) for _, a, d in data], [(0xC004, 28), (0xC020, 12)])

    def test_every_record_line_fits_the_limit(self):
        longest = max(len(line) for line in fwupdate.rechunk(LOW, good_image()))
        self.assertLessEqual(longest, fwu_settings.line_max() - 1)


class SequenceTests(unittest.TestCase):
    def setUp(self):
        handle, self.path = tempfile.mkstemp(suffix=".hex")
        with os.fdopen(handle, "w") as out:
            out.write(hex_text(good_image()))
        self.addCleanup(os.remove, self.path)

    def test_update_sequence(self):
        fake = FakeBoot()
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 0, out)
        commands = [w for w in fake.written if not w.startswith(":")]
        self.assertEqual(commands, ["WAKEUP", "ERASE APP", "CONFIRM", "LOAD", "VERIFY", "RUN"])
        records = [w for w in fake.written if w.startswith(":")]
        self.assertEqual(records, fwupdate.rechunk(LOW, good_image()))
        self.assertEqual(fake.baud, fwu_settings.baud_for("app"))
        self.assertTrue(fake.closed)

    def test_no_run_and_field_send_no_run(self):
        for flags in (["--no-run"], ["--field"]):
            with self.subTest(flags):
                fake = FakeBoot()
                code, out = run_main(fake, "--port", "COM5", *flags, "update", self.path)
                self.assertEqual(code, 0, out)
                self.assertNotIn("RUN", fake.written)
        self.assertIn("remove the BOOTLOADER jumper and power-cycle the board", out)
        self.assertIn("power-cycle the board now", out)

    def test_resend_after_checksum_error(self):
        fake = FakeBoot(record_answers={3: ["ERR CHECKSUM", "OK"]})
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 0, out)
        records = [w for w in fake.written if w.startswith(":")]
        expected = fwupdate.rechunk(LOW, good_image())
        self.assertEqual(len(records), len(expected) + 1)
        self.assertEqual(records[2], records[3])
        self.assertIn("record 3: ERR CHECKSUM, sending it again", out)

    def test_abort_after_range_error_names_the_record(self):
        fake = FakeBoot(record_answers={5: ["ERR RANGE address 0800 outside"]})
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 1)
        self.assertTrue(any("record 5" in line and "ERR RANGE" in line for line in out), out)
        self.assertNotIn("VERIFY", fake.written)
        self.assertEqual(len([w for w in fake.written if w.startswith(":")]), 5)

    def test_retries_run_out(self):
        answers = ["ERR FORMAT"] * (fwu_settings.RECORD_RETRIES + 1)
        fake = FakeBoot(record_answers={2: answers})
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 1)
        self.assertTrue(any("record 2" in line and "ERR FORMAT" in line for line in out), out)

    def test_resume_after_err_aborted(self):
        fake = FakeBoot(record_answers={5: ["ERR ABORTED"]})
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 0, out)
        expected = fwupdate.rechunk(LOW, good_image())
        # record 5 is refused, LOAD starts again, the 04 record comes first, then record 5 again
        at = fake.written.index("LOAD", fake.written.index("LOAD") + 1)
        self.assertEqual(fake.written[at - 1], expected[4])
        self.assertEqual(fake.written[at + 1], expected[0])
        self.assertEqual(fake.written[at + 2], expected[4])
        self.assertEqual(fake.written[at + 3], expected[5])
        self.assertIn("record 5: ERR ABORTED, LOAD again from record 5", out)
        self.assertEqual(fake.written[-2:], ["VERIFY", "RUN"])

    def test_resumes_run_out(self):
        answers = ["ERR ABORTED"] * (fwu_settings.LOAD_RESUMES + 1)
        fake = FakeBoot(record_answers={5: answers})
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 1)
        self.assertTrue(any("record 5" in line and "ERR ABORTED" in line for line in out), out)
        self.assertEqual(fake.written.count("LOAD"), 1 + fwu_settings.LOAD_RESUMES)
        self.assertNotIn("VERIFY", fake.written)

    def test_precheck_refusal_sends_nothing_and_no_precheck_skips_it(self):
        image = good_image()
        image[HEADER] = 0
        with open(self.path, "w") as out_file:
            out_file.write(hex_text(image))
        fake = FakeBoot()
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 1)
        self.assertEqual(fake.written, [])
        code, _ = run_main(FakeBoot(), "--port", "COM5", "--no-precheck", "update", self.path)
        self.assertEqual(code, 0)

    def test_as_is_sends_the_file_records(self):
        fake = FakeBoot()
        code, out = run_main(fake, "--port", "COM5", "--as-is", "update", self.path)
        self.assertEqual(code, 0, out)
        sent = [w for w in fake.written if w.startswith(":")]
        self.assertEqual(sent, Path(self.path).read_text().split())

    def test_erase_timeout_exits_3(self):
        fake = FakeBoot(erase_answer=False)
        code, out = run_main(fake, "--port", "COM5", "update", self.path)
        self.assertEqual(code, 3)
        self.assertGreaterEqual(fake.now, fwu_settings.ERASE_TIMEOUT_S)

    def test_erase_actions_and_simple_commands(self):
        fake = FakeBoot()
        self.assertEqual(run_main(fake, "--port", "COM3", "--uart", "cli", "erase-fru")[0], 0)
        self.assertEqual(fake.written[-2:], ["ERASE FRU", "CONFIRM"])
        self.assertEqual(fake.baud, fwu_settings.baud_for("cli"))
        fake = FakeBoot()
        code, out = run_main(fake, "--port", "COM5", "info")
        self.assertEqual(code, 0)
        self.assertIn("Bootloader: ipmc-boot v0.1.0", out)

    def test_err_status_exits_1(self):
        fake = FakeBoot()
        fake._on_command = lambda line: fake._reply("ERR NOT_ERASED erase APP first")
        fake._awake = False
        code, _ = run_main(fake, "--port", "COM5", "info")
        self.assertEqual(code, 1)


class FakeSerial:
    """A pyserial stand-in: `chunks` are the reads; `.timeout` assignments are counted."""

    def __init__(self, chunks):
        self._chunks = list(chunks)
        self.timeout_sets = 0
        self._timeout = None
        self.in_waiting = 0

    @property
    def timeout(self):
        return self._timeout

    @timeout.setter
    def timeout(self, value):
        self.timeout_sets += 1
        self._timeout = value

    def read(self, size=1):
        if not self._chunks:
            return b""
        chunk = self._chunks.pop(0)
        self.in_waiting = len(self._chunks[0]) if self._chunks else 0
        return chunk

    def reset_input_buffer(self):
        self._chunks.clear()


class SerialTransportTests(unittest.TestCase):
    def make(self, chunks):
        transport = fwupdate.SerialTransport.__new__(fwupdate.SerialTransport)
        transport._serial = FakeSerial(chunks)
        transport._serial.timeout = fwu_settings.POLL_S  # the one assignment, at open
        transport._partial = b""
        return transport

    def test_timeout_is_assigned_once_and_lines_split_from_one_read(self):
        transport = self.make([b"OK\r\nBOOT", b"> ", b"x\r\ny\r\n"])
        self.assertEqual(transport.read_line(0.05), "OK")
        self.assertTrue(transport.wait_prompt("BOOT> ", 0.05))
        self.assertEqual(transport.read_line(0.05), "x")
        self.assertEqual(transport.read_line(0.05), "y")
        self.assertIsNone(transport.read_line(0.05))
        transport.settle(0.05)
        self.assertEqual(transport._serial.timeout_sets, 1)

    def test_two_lines_in_one_read_are_two_results(self):
        transport = self.make([b"one\r\ntwo\r\n"])
        self.assertEqual(transport.read_line(0.05), "one")
        self.assertEqual(transport.read_line(0.05), "two")

    def test_a_partial_line_stays_for_the_next_call(self):
        transport = self.make([b"par", b"tial\r\n"])
        self.assertIsNone(transport.read_line(0.0))
        self.assertEqual(transport.read_line(0.05), "partial")


class WakeupTests(unittest.TestCase):
    def test_wakeup_repeats_until_ok(self):
        fake = FakeBoot(wake_after=4)
        code, out = run_main(fake, "--port", "COM5", "wakeup")
        self.assertEqual(code, 0)
        self.assertEqual(fake.written, ["WAKEUP"] * 4)
        self.assertAlmostEqual(
            fake.now, 3 * fwu_settings.WAKEUP_INTERVAL_S + fwu_settings.SETTLE_S, places=6
        )

    def test_second_ok_in_flight_is_not_taken_for_the_next_status(self):
        fake = FakeBoot(double_ok=True)
        fake._on_command = lambda line: fake._reply("ERR BUSY")
        code, out = run_main(fake, "--port", "COM5", "info")
        self.assertEqual(code, 1, out)  # the command's own ERR, not the stale OK
        self.assertIn("refused: INFO: ERR BUSY", out)

    def test_wakeup_gives_up_with_exit_3(self):
        fake = FakeBoot(wake_after=10**6)
        code, _ = run_main(fake, "--port", "COM5", "wakeup")
        self.assertEqual(code, 3)
        self.assertGreaterEqual(fake.now, fwu_settings.WAKEUP_GIVE_UP_S)

    def test_countdown_line_is_seen_and_the_prompt_reported(self):
        fake = FakeBoot(
            preamble=["Send WAKEUP on CLI or APP within 5 s to stay in the bootloader", "5 4 3"]
        )
        code, out = run_main(fake, "--port", "COM5", "wakeup")
        self.assertEqual(code, 0)
        self.assertTrue(any(line.startswith("countdown seen: Send WAKEUP") for line in out), out)
        self.assertTrue(any("prompt" in line for line in out))

    def test_enter_sends_the_runtime_command_first(self):
        for uart, command in (("cli", "DEBUG BOOTLOADER"), ("app", "BOOTLOADER")):
            with self.subTest(uart):
                fake = FakeBoot(preamble=["OK BOOTLOADER"])
                fake._awake = False
                code, out = run_main(
                    fake, "--port", "COM5", "--enter", uart, "--enter-port", "COM9", "info"
                )
                self.assertEqual(code, 0, out)
                self.assertEqual(fake.written[0], command)

    def test_enter_prints_the_answers_of_each_runtime_port(self):
        # (uart, lines the runtime port sends, lines the tool prints)
        rows = (
            ("app", ["BOOTLOADER", "BOOTLOADER: restarting", "OK", "APP> "], ["BOOTLOADER: restarting", "OK"]),
            (
                "cli",
                ["Restarting into the bootloader", "Payload power drops at the reset"],
                ["Restarting into the bootloader", "Payload power drops at the reset"],
            ),
            ("cli", ["Restarting into the bootloader"], ["Restarting into the bootloader"]),
        )
        for uart, sent, printed in rows:
            with self.subTest(uart, sent=sent):
                fake = FakeBoot(preamble=sent)
                fake._awake = False
                code, out = run_main(
                    fake, "--port", "COM5", "--enter", uart, "--enter-port", "COM9", "info"
                )
                self.assertEqual(code, 0, out)
                self.assertEqual(out[: len(printed)], printed)

    def run_enter(self, uart, answers, typed, *flags):
        """`info` through a runtime port that answers each command as `answers` says; `typed` is
        what stdin gives (a string, or None for end of input)."""
        runtime = FakeRuntime(answers)
        boot = FakeBoot()
        asked = []

        def read_input(prompt):
            asked.append(prompt)
            if typed is None:
                raise EOFError
            return typed

        def factory(port, baud):
            return runtime if port == "COM9" else boot.factory(port, baud)

        out = []
        with contextlib.redirect_stdout(io.StringIO()):
            code = fwupdate.main(
                ["--port", "COM5", "--enter", uart, "--enter-port", "COM9", *flags, "info"],
                factory,
                out.append,
                boot.clock,
                read_input,
            )
        return code, out, runtime.written, asked

    def test_confirm_flag_sends_the_confirm_form_from_the_start(self):
        for uart, command in (("cli", "DEBUG BOOTLOADER"), ("app", "BOOTLOADER")):
            with self.subTest(uart):
                answers = {
                    f"{command} CONFIRM": [
                        "Restarting into the bootloader",
                        "Payload power drops at the reset",
                        "OK",
                    ]
                }
                code, out, written, asked = self.run_enter(uart, answers, None, "--confirm")
                self.assertEqual(code, 0, out)
                self.assertEqual(written, [f"{command} CONFIRM"])
                self.assertEqual(asked, [])

    def test_the_stdin_prompt_accepts_confirm_in_any_case(self):
        rows = (
            (
                "cli",
                "DEBUG BOOTLOADER",
                [
                    "Payload power is on and drops at the reset into the bootloader",
                    "Send DEBUG BOOTLOADER CONFIRM to restart",
                ],
            ),
            (
                "app",
                "BOOTLOADER",
                [
                    "ERROR CONFIRM: payload power is on and drops at the reset; "
                    "send BOOTLOADER CONFIRM"
                ],
            ),
        )
        for uart, command, ask in rows:
            for typed in ("CONFIRM", " confirm "):
                with self.subTest(uart, typed=typed):
                    answers = {
                        command: ask,
                        f"{command} CONFIRM": ["Restarting into the bootloader", "OK"],
                    }
                    code, out, written, asked = self.run_enter(uart, answers, typed)
                    self.assertEqual(code, 0, out)
                    self.assertEqual(written, [command, f"{command} CONFIRM"])
                    self.assertEqual(asked, [fwupdate.CONFIRM_PROMPT])
                    self.assertEqual(out[: len(ask)], ask)

    def test_other_input_and_end_of_input_refuse_the_entry(self):
        ask = [
            "Payload power is on and drops at the reset into the bootloader",
            "Send DEBUG BOOTLOADER CONFIRM to restart",
        ]
        for typed in ("yes", "", None):
            with self.subTest(typed=typed):
                code, out, written, _ = self.run_enter("cli", {"DEBUG BOOTLOADER": ask}, typed)
                self.assertEqual(code, 1)
                self.assertIn("refused: entry not confirmed", out)
                self.assertEqual(written, ["DEBUG BOOTLOADER"])

    def test_power_off_answers_need_no_prompt(self):
        rows = (
            ("cli", "DEBUG BOOTLOADER", ["Restarting into the bootloader"]),
            ("app", "BOOTLOADER", ["BOOTLOADER: restarting", "OK"]),
        )
        for uart, command, answer in rows:
            with self.subTest(uart):
                code, out, written, asked = self.run_enter(uart, {command: answer}, None)
                self.assertEqual(code, 0, out)
                self.assertEqual(written, [command])
                self.assertEqual(asked, [])

    def test_confirm_needs_enter(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            fwupdate.parse_args(["--port", "COM5", "--confirm", "info"])

    def test_usage_errors_exit_2(self):
        for argv in (["info"], ["--port", "COM5", "update"], ["--port", "COM5", "--enter", "app", "info"]):
            with self.subTest(argv), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as caught:
                    fwupdate.parse_args(argv)
                self.assertEqual(caught.exception.code, 2)


class SettingsTests(unittest.TestCase):
    def test_values_come_from_defs_rs(self):
        self.assertEqual(fwu_settings.prompt(), PROMPT)
        self.assertEqual(fwu_settings.baud_for("cli"), 115200)
        self.assertEqual(fwu_settings.baud_for("app"), 115200)
        self.assertEqual(fwu_settings.line_max(), 128)

    def test_values_follow_a_defs_file(self):
        text = (
            'pub const IPMC_CONSOLE_BAUD: u32 = 57_600;\n'
            'pub const IPMC_APP_UART_BAUD: u32 = 38_400;\n'
            'pub const IPMC_BOOT_PROMPT: &str = "BL> ";\n'
            'pub const IPMC_MENU_LINE_MAX: usize = 64;\n'
        )
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "defs.rs"
            path.write_text(text)
            self.assertEqual(fwu_settings.baud_for("cli", path), 57600)
            self.assertEqual(fwu_settings.baud_for("app", path), 38400)
            self.assertEqual(fwu_settings.prompt(path), "BL> ")
            self.assertEqual(fwu_settings.line_max(path), 64)

    def test_fallbacks_when_the_file_is_absent(self):
        missing = Path(tempfile.gettempdir()) / "no_such_dir_fwu" / "defs.rs"
        self.assertEqual(fwu_settings.baud_for("cli", missing), fwu_settings.BAUD_FALLBACK)
        self.assertEqual(fwu_settings.baud_for("app", missing), 115200)
        self.assertEqual(fwu_settings.prompt(missing), "BOOT> ")
        self.assertEqual(fwu_settings.line_max(missing), 128)


if __name__ == "__main__":
    unittest.main()
