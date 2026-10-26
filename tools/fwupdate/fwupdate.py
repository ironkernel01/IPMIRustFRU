"""fwupdate.py - host update tool for the IPMC bootloader (crates/ipmc-boot).

Talks to the bootloader on the console (CLI) port or the application (APP) port, erases the
runtime firmware sectors, loads an Intel HEX file record by record, verifies it and starts it.

    python fwupdate.py --port COM5 update app.hex
    python fwupdate.py --port COM3 --uart cli info
    python fwupdate.py --port COM5 --enter app --enter-port COM5 update app.hex
    python fwupdate.py --port COM5 --field update app.hex

Protocol (design Documentation/ipmc_boot_plan.md, brief 12f): lines end in CR LF; before the port
choice the bootloader answers only WAKEUP; in command mode it echoes what is typed and every
command ends with one status line that starts with `OK` or `ERR`; in record mode each `:` line is
answered by one status line. Exit codes: 0 success, 1 a refusal or an ERR answer, 2 usage,
3 a timeout.
"""

import argparse
import hashlib
import re
import sys
import time

import fwu_settings

LINE_END = b"\r\n"
APP_START = 0x0800_8000
APP_END = 0x0808_0000
HEADER_ADDR = 0x0800_8200
HEADER_SIZE = 64
HEADER_MAGIC = 0x434D_5049
HEADER_FORMAT = 1
LENGTH_MIN = 0x240
LENGTH_MAX = 491_520
# The digest field (header +0x20..+0x40) is not hashed: 0x0800_8220..0x0800_8240 is skipped.
HASH_GAP = (0x0800_8220, 0x0800_8240)
SRAM_LOW = 0x2000_0100
SRAM_HIGH = 0x2001_8000
BLANK = 0xFF
# Answers after which a record is sent again (damage on the line).
RETRY_CODES = ("FORMAT", "CHECKSUM", "LINE_TOO_LONG")
# Runtime entry commands, by the port they are typed on.
ENTER_COMMANDS = {"cli": "DEBUG BOOTLOADER", "app": "BOOTLOADER"}
STATUS_PATTERN = re.compile(r"^(OK|ERR)(?: |$)")

EXIT_OK, EXIT_REFUSED, EXIT_USAGE, EXIT_TIMEOUT = 0, 1, 2, 3


class Refused(Exception):
    """The tool refuses to go on (a bad file, an ERR answer); exit status 1."""


class BootTimeout(Exception):
    """No answer in time; exit status 3."""


# ---------------------------------------------------------------- Intel HEX and the image


def record_line(rec_type, address16, data):
    """One Intel HEX record as text (upper-case hex, checksum last)."""
    body = bytes([len(data), address16 >> 8, address16 & 0xFF, rec_type]) + bytes(data)
    checksum = (-sum(body)) & 0xFF
    return ":" + (body + bytes([checksum])).hex().upper()


def parse_records(text):
    """The records of an Intel HEX text as (type, address16, data) tuples. Refuses a bad line, a
    bad checksum or length, a type other than 00, 01, 04, 05, a missing end-of-file record."""
    records = []
    ended = False
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        if ended:
            raise Refused(f"line {number}: data after the end-of-file record")
        if not line.startswith(":") or len(line) < 11 or len(line) % 2 == 0:
            raise Refused(f"line {number}: not an Intel HEX record")
        try:
            raw_bytes = bytes.fromhex(line[1:])
        except ValueError as error:
            raise Refused(f"line {number}: not hexadecimal") from error
        if raw_bytes[0] != len(raw_bytes) - 5:
            raise Refused(f"line {number}: record length does not match the line")
        if sum(raw_bytes) & 0xFF:
            raise Refused(f"line {number}: checksum error")
        rec_type = raw_bytes[3]
        if rec_type in (2, 3):
            raise Refused(f"line {number}: record type {rec_type:02X} is not accepted")
        if rec_type not in (0, 1, 4, 5):
            raise Refused(f"line {number}: unknown record type {rec_type:02X}")
        address16 = (raw_bytes[1] << 8) | raw_bytes[2]
        data = raw_bytes[4:-1]
        if (rec_type == 4 and len(data) != 2) or (rec_type == 5 and len(data) != 4):
            raise Refused(f"line {number}: wrong data length for type {rec_type:02X}")
        if rec_type == 1:
            ended = True
        records.append((rec_type, address16, data))
    if not ended:
        raise Refused("no end-of-file record")
    return records


def build_memory(records):
    """{address: byte} of the data records (type 04 sets the upper 16 bits). Refuses an address
    given twice."""
    memory = {}
    upper = 0
    for rec_type, address16, data in records:
        if rec_type == 4:
            upper = int.from_bytes(data, "big") << 16
        elif rec_type == 0:
            for offset, value in enumerate(data):
                address = upper + address16 + offset
                if address in memory:
                    raise Refused(f"address {address:08X} is given twice")
                memory[address] = value
    return memory


def build_image(memory):
    """(low address, bytearray) of the memory from its first to its last byte, gaps FFh."""
    if not memory:
        raise Refused("the file holds no data")
    low, high = min(memory), max(memory) + 1
    image = bytearray([BLANK]) * (high - low)
    for address, value in memory.items():
        image[address - low] = value
    return low, image


def precheck(low, image):
    """Repeats the bootloader's image rules (design section 4); raises Refused naming the rule."""
    high = low + len(image)
    if low < APP_START or high > APP_END:
        raise Refused(
            f"data {low:08X}..{high:08X} lies outside the APP region {APP_START:08X}..{APP_END:08X}"
        )

    def span(start, stop):
        """Bytes start..stop of the address space; FFh outside the data."""
        head = bytes([BLANK]) * max(0, min(low, stop) - start)
        body = bytes(image[max(start, low) - low : max(min(stop, high) - low, 0)])
        tail = bytes([BLANK]) * (stop - start - len(head) - len(body))
        return head + body + tail

    def word(address, size):
        return int.from_bytes(span(address, address + size), "little")

    magic = word(HEADER_ADDR, 4)
    if magic != HEADER_MAGIC:
        raise Refused(f"header magic is {magic:08X}, expected {HEADER_MAGIC:08X}")
    if word(HEADER_ADDR + 4, 2) != HEADER_FORMAT:
        raise Refused(f"header format is {word(HEADER_ADDR + 4, 2)}, expected {HEADER_FORMAT}")
    if word(HEADER_ADDR + 6, 2) != HEADER_SIZE:
        raise Refused(f"header size is {word(HEADER_ADDR + 6, 2)}, expected {HEADER_SIZE}")
    length = word(HEADER_ADDR + 8, 4)
    if length % 4 or not LENGTH_MIN <= length <= LENGTH_MAX:
        raise Refused(f"image length {length} is not a multiple of 4 in {LENGTH_MIN}..{LENGTH_MAX}")
    end = APP_START + length
    if high > end:
        raise Refused(f"data reaches {high:08X}, beyond the image end {end:08X}")
    if word(HEADER_ADDR + 0x14, 4) != APP_START:
        raise Refused(f"header vector table address is {word(HEADER_ADDR + 0x14, 4):08X}")
    stack = word(APP_START, 4)
    if stack % 8 or not SRAM_LOW <= stack <= SRAM_HIGH:
        raise Refused(f"vector word 0 (stack) {stack:08X} is not 8-byte aligned in SRAM range")
    reset = word(APP_START + 4, 4)
    if not reset & 1 or not HASH_GAP[1] <= (reset & ~1) < end:
        raise Refused(f"vector word 1 (reset) {reset:08X} is not an odd address inside the code")
    digest = hashlib.sha256(span(APP_START, HASH_GAP[0]) + span(HASH_GAP[1], end)).digest()
    stored = span(HEADER_ADDR + 0x20, HEADER_ADDR + 0x40)
    if digest != stored:
        raise Refused(f"SHA-256 differs: computed {digest.hex()} stored {stored.hex()}")


def rechunk(low, image, size=fwu_settings.RECORD_BYTES):
    """Record lines for the image: aligned `size`-byte data records, an 04 record when the upper
    16 bits change (also before the first), the 01 record last. A chunk that is all FFh is
    skipped (the flash is erased); a chunk is trimmed to the image span."""
    high = low + len(image)
    lines = []
    upper = None
    base = low - low % size
    while base < high:
        start, stop = max(base, low), min(base + size, high)
        chunk = bytes(image[start - low : stop - low])
        if chunk.count(BLANK) != len(chunk):
            if start >> 16 != upper:
                upper = start >> 16
                lines.append(record_line(4, 0, upper.to_bytes(2, "big")))
            lines.append(record_line(0, start & 0xFFFF, chunk))
        base += size
    lines.append(record_line(1, 0, b""))
    return lines


# ---------------------------------------------------------------- the serial port


class SerialTransport:
    """A serial port that writes bytes and reads one line (pyserial, imported on first use)."""

    def __init__(self, port, baud):
        import serial  # pylint: disable=import-outside-toplevel

        self._serial = serial.Serial(port, baud, timeout=1.0)
        self._serial.reset_input_buffer()
        self._partial = b""

    def write(self, data):
        self._serial.write(data)
        self._serial.flush()

    def read_line(self, timeout):
        """One line without its CR LF, or None when `timeout` seconds passed with no complete
        line (a partial line stays for the next call)."""
        self._serial.timeout = timeout
        self._partial += self._serial.readline()
        if not self._partial.endswith(b"\n"):
            return None
        raw, self._partial = self._partial, b""
        return raw.decode("ascii", errors="replace").rstrip("\r\n")

    def wait_prompt(self, prompt, timeout):
        """Reads until the prompt (no line end) arrives; True when it came before the timeout."""
        expected = prompt.encode("ascii")
        self._serial.timeout = timeout
        got = self._partial + self._serial.read_until(expected)
        self._partial = b""
        return got.endswith(expected)

    def settle(self, seconds):
        """Reads and drops everything that arrives for `seconds`, then clears the input buffer."""
        self._serial.timeout = seconds
        self._serial.read_until(b"\x00")  # no NUL arrives: returns when the time is up
        self._serial.reset_input_buffer()
        self._partial = b""

    def close(self):
        self._serial.close()


# ---------------------------------------------------------------- the bootloader dialogue


def is_status(line):
    """True for a status line: `OK` or `ERR`, alone or followed by a space and text."""
    return bool(STATUS_PATTERN.match(line))


class Bootloader:
    """Sends commands and records through a transport and returns the status lines."""

    def __init__(self, transport, out=print, clock=time.monotonic):
        self._transport = transport
        self._out = out
        self._clock = clock
        self._prompt = fwu_settings.prompt()
        self.line_max = fwu_settings.line_max()

    def _send(self, text):
        self._transport.write(text.encode("ascii") + LINE_END)

    def _read_status(self, timeout, sent=None, collected=None):
        """Reads lines up to the status line and returns it. The echo of `sent` and prompt lines
        are skipped; other lines are appended to `collected`. BootTimeout when nothing ends it."""
        while True:
            line = self._transport.read_line(timeout)
            if line is None:
                raise BootTimeout("no answer")
            line = line.strip()
            if not line or line.startswith(self._prompt.strip()) or line == sent:
                continue
            if is_status(line):
                return line
            if collected is not None:
                collected.append(line)

    def wakeup(self, limit_s=fwu_settings.WAKEUP_GIVE_UP_S):
        """Sends WAKEUP every interval until an OK line; limit_s None waits without limit."""
        start = self._clock()
        seen_countdown = False
        while True:
            self._send("WAKEUP")
            interval_end = self._clock() + fwu_settings.WAKEUP_INTERVAL_S
            while True:
                remaining = interval_end - self._clock()
                line = self._transport.read_line(max(remaining, 0.01))
                if line is None:
                    break
                line = line.strip()
                if line.startswith("Send WAKEUP") and not seen_countdown:
                    seen_countdown = True
                    self._out(f"countdown seen: {line}")
                elif line.upper() == "OK":
                    if self._transport.wait_prompt(self._prompt, fwu_settings.PROMPT_TIMEOUT_S):
                        self._out(f"bootloader prompt {self._prompt.strip()} received")
                    self._transport.settle(fwu_settings.SETTLE_S)
                    return
            if limit_s is not None and self._clock() - start >= limit_s:
                raise BootTimeout(f"no OK to WAKEUP in {limit_s:g} s")

    def command(self, text, timeout=fwu_settings.ANSWER_TIMEOUT_S):
        """Sends a command; returns (output lines, status line)."""
        self._send(text)
        lines = []
        status = self._read_status(timeout, sent=text, collected=lines)
        return lines, status

    def run_ok(self, text, expect="OK", timeout=fwu_settings.ANSWER_TIMEOUT_S):
        """A command whose status line must start with `expect`; prints the output and status."""
        lines, status = self.command(text, timeout)
        for line in lines:
            self._out(line)
        self._out(status)
        if not (status == expect or status.startswith(expect + " ")):
            raise Refused(f"{text}: {status}")
        return lines, status

    def erase(self, which):
        """ERASE APP|FRU, then CONFIRM."""
        self.run_ok(f"ERASE {which}", expect="OK PENDING")
        self.run_ok("CONFIRM", expect="OK ERASED", timeout=fwu_settings.ERASE_TIMEOUT_S)

    def send_record(self, line, number):
        """One record, resent after a damage answer; Refused names the record and the answer."""
        if len(line) > self.line_max - 1:
            raise Refused(
                f"record {number} is {len(line)} characters, the limit is {self.line_max - 1}"
            )
        for attempt in range(fwu_settings.RECORD_RETRIES + 1):
            self._send(line)
            status = self._read_status(fwu_settings.RECORD_TIMEOUT_S)
            if status.startswith("OK"):
                return status
            words = status.split()
            code = words[1] if len(words) > 1 else ""
            if code not in RETRY_CODES or attempt == fwu_settings.RECORD_RETRIES:
                break
        raise Refused(f"record {number}: {status}")

    def load(self, lines, progress=lambda done, total, sent, rate: None):
        """LOAD, then the record lines stop-and-wait; the last one is the end record."""
        self.run_ok("LOAD", expect="OK LOAD")
        begin = self._clock()
        sent = 0
        status = ""
        for number, line in enumerate(lines, 1):
            status = self.send_record(line, number)
            sent += (len(line) - 11) // 2
            elapsed = max(self._clock() - begin, 1e-6)
            progress(number, len(lines), sent, sent / elapsed)
            last = number == len(lines)
            if last and not status.startswith("OK END"):
                raise Refused(f"record {number}: {status}")
            if not last and status != "OK":
                raise Refused(f"record {number}: {status}")
        self._out(status)


# ---------------------------------------------------------------- the actions


def plain_lines(path):
    """The record lines of the file as written (for --as-is)."""
    with open(path, encoding="ascii") as handle:
        return [line.strip() for line in handle if line.strip()]


def prepare(args, out):
    """The record lines to send for `update`, after the pre-check (before anything is sent)."""
    with open(args.file, encoding="ascii", errors="replace") as handle:
        records = parse_records(handle.read())
    low, image = build_image(build_memory(records))
    if not args.no_precheck:
        precheck(low, image)
        out("pre-check passed")
    return plain_lines(args.file) if args.as_is else rechunk(low, image)


def update(boot, args, lines, out, progress):
    """The update sequence: erase, load, verify, then run (or the field message)."""
    boot.erase("APP")
    boot.load(lines, progress)
    boot.run_ok("VERIFY", expect="OK VERIFY valid")
    if args.field:
        out("remove the BOOTLOADER jumper and power-cycle the board")
    elif not args.no_run:
        boot.run_ok("RUN", expect="OK RUN")


SIMPLE = {"info": "INFO", "verify": "VERIFY", "run": "RUN", "reset": "RESET", "help": "HELP"}


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description="Update tool for the IPMC bootloader.")
    parser.add_argument("--port", default=fwu_settings.PORT_DEFAULT, help="serial port (required)")
    parser.add_argument("--uart", choices=("cli", "app"), default="app", help="what --port is")
    parser.add_argument("--baud", type=int, help="baud rate (default from defs.rs)")
    parser.add_argument("--enter", choices=("cli", "app"), help="runtime port used to enter")
    parser.add_argument("--enter-port", help="serial port of the --enter runtime port")
    parser.add_argument("--field", action="store_true", help="field procedure (jumper, no RUN)")
    parser.add_argument("--no-precheck", action="store_true", help="skip the image pre-check")
    parser.add_argument("--as-is", action="store_true", help="send the file's own records")
    parser.add_argument("--no-run", action="store_true", help="no RUN after update")
    parser.add_argument("action", choices=["wakeup", "erase-app", "erase-fru", "update", *SIMPLE])
    parser.add_argument("file", nargs="?", help="Intel HEX file (update)")
    args = parser.parse_args(argv)
    if not args.port:
        parser.error("--port is required")
    if args.action == "update" and not args.file:
        parser.error("update needs a .hex file")
    if args.action != "update" and args.file:
        parser.error(f"{args.action} takes no file")
    if bool(args.enter) != bool(args.enter_port):
        parser.error("--enter and --enter-port go together")
    return args


def enter_runtime(args, factory, out):
    """Sends the runtime entry command on the runtime port and prints the answer."""
    port = factory(args.enter_port, fwu_settings.baud_for(args.enter))
    try:
        port.write(ENTER_COMMANDS[args.enter].encode("ascii") + LINE_END)
        for _ in range(2):
            line = port.read_line(fwu_settings.ANSWER_TIMEOUT_S)
            if line is None:
                break
            if line.strip():
                out(line.strip())
    finally:
        port.close()


def main(argv=None, transport_factory=SerialTransport, out=print, clock=time.monotonic):
    """Runs one action; returns the exit status."""
    args = parse_args(argv)
    start = clock()

    def progress(done, total, sent, rate):
        sys.stdout.write(f"\rrecords {done}/{total}  bytes {sent}  {rate:.0f} B/s ")
        if done == total:
            sys.stdout.write("\n")
        sys.stdout.flush()

    try:
        lines = prepare(args, out) if args.action == "update" else None
        if args.enter:
            enter_runtime(args, transport_factory, out)
        if args.field:
            out("power-cycle the board now")
        baud = args.baud or fwu_settings.baud_for(args.uart)
        transport = transport_factory(args.port, baud)
        try:
            boot = Bootloader(transport, out, clock)
            boot.wakeup(None if args.field else fwu_settings.WAKEUP_GIVE_UP_S)
            if args.action == "update":
                update(boot, args, lines, out, progress)
            elif args.action in ("erase-app", "erase-fru"):
                boot.erase(args.action[-3:].upper())
            elif args.action in SIMPLE:
                boot.run_ok(SIMPLE[args.action])
        finally:
            transport.close()
    except Refused as error:
        out(f"refused: {error}")
        return EXIT_REFUSED
    except BootTimeout as error:
        out(f"timeout: {error}")
        return EXIT_TIMEOUT
    out(f"done in {clock() - start:.1f} s")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
