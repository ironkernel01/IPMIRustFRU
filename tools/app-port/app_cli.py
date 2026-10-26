"""app_cli.py - bench client for the IPMC application port (crates/ipmc-app).

Opens a serial port, sends one request line per command-line argument (or prompts for lines)
and prints the answers. The port speaks one readable format for a person and a payload CPU alike
(module comment of crates/ipmc-app/src/lib.rs): the IPMC prints the prompt `app> ` and echoes
what is typed; an answer is zero or more data lines `KEY: value` and one final line, `OK` or
`ERROR <CODE>: <sentence>`, with `#tag` after the `OK` or `ERROR` when the request had a tag.

The parsing rule this client follows (the rule a payload CPU follows): skip lines that start with
`app> ` (prompt and echo); collect `KEY: value` lines; the answer ends at the first line that is
`OK` or starts with `OK ` or `ERROR `.

    python app_cli.py --port COM5 INFO STATUS "FRU SET ASSET_TAG rack 7" "FRU SAVE"
    python app_cli.py --port COM5            # interactive: type a request, Enter sends it
    python app_cli.py --port COM5 --raw      # a terminal: every key goes to the port

By default each request gets a tag (`#t0001 ...`); the IPMC puts it on the final line, and the
client discards an answer with another tag (a late answer of an earlier request). The client
prints the data lines and the final line of each answer. The exit status is 0 when every request
got an `OK` answer, 1 when one got `ERROR` or no answer, and 2 when a request, tag included, is
longer than the IPMC accepts (it is not sent; in the prompt the line is refused and the prompt
goes on).
"""

import argparse
import re
import sys

import app_settings

# The protocol line end for requests (the IPMC accepts CR, LF or CR LF). Answers end in CR LF.
REQUEST_END = b"\r\n"
# The prompt the IPMC prints after the banner and after every answer; it also starts the echo line.
DEVICE_PROMPT = "app> "
# A tag is `#` and 1 to 8 letters or digits, then a space (crates/ipmc-app/src/engine.rs).
TAG_PATTERN = re.compile(r"^#[A-Za-z0-9]{1,8} ")
# A data line: KEY in capitals and underscores, a colon, a blank, the value (maybe empty).
DATA_PATTERN = re.compile(r"^([A-Z][A-Z0-9_]*): (.*)$")
# A final line: OK, or ERROR with the code and the sentence; the tag follows the first word.
FINAL_PATTERN = re.compile(r"^(OK|ERROR)(?: #([A-Za-z0-9]{1,8}))?(?: ([A-Z][A-Z0-9_]*): (.*))?$")
# Ctrl+] ends --raw (the telnet escape).
RAW_QUIT = b"\x1d"


class SerialTransport:
    """A serial port that writes bytes and reads one line (pyserial, imported on first use)."""

    def __init__(self, port, baud, timeout_s):
        import serial  # pylint: disable=import-outside-toplevel

        self._serial = serial.Serial(port, baud, timeout=timeout_s)

    def write(self, data):
        self._serial.write(data)
        self._serial.flush()

    def read_line(self):
        """One line without its CR LF, or None when the timeout passed with no complete line.

        The prompt `app> ` has no line end: it is returned joined with the echo that follows it,
        so a line that starts with the prompt is a prompt or echo line.
        """
        raw = self._serial.readline()
        if not raw.endswith(b"\n"):
            return None
        return raw.decode("ascii", errors="replace").rstrip("\r\n")

    def read_available(self):
        """The bytes that arrived, without waiting (empty when none)."""
        return self._serial.read(self._serial.in_waiting)

    def close(self):
        self._serial.close()


class LineTooLong(ValueError):
    """A request, tag included, is longer than the IPMC accepts (nothing was written)."""


class Answer:
    """One answer: the data lines, the final line and what its words are."""

    def __init__(self, lines, final):
        self.lines = lines  # every line before the final line, as received
        self.final = final  # the final line, as received
        match = FINAL_PATTERN.match(final)
        self.status = match.group(1) if match else "ERROR"
        self.tag = match.group(2) if match else None
        self.code = match.group(3) if match else None
        self.sentence = match.group(4) if match else None

    @property
    def ok(self):
        return self.status == "OK"

    @property
    def data(self):
        """The `KEY: value` lines as (key, value) pairs, in order."""
        found = (DATA_PATTERN.match(line) for line in self.lines)
        return [(m.group(1), m.group(2)) for m in found if m]


def is_final(line):
    """True for the line that ends an answer: `OK`, `OK ...` or `ERROR ...`."""
    return line == "OK" or line.startswith("OK ") or line.startswith("ERROR ")


class Client:
    """Sends request lines through a transport and returns the matching answers."""

    # Length of the tag text in front of a request: `#`, the prefix, four hex digits, a blank.
    TAG_LEN = len(f"#{app_settings.TAG_PREFIX}0000 ")

    def __init__(self, transport, use_tags=app_settings.TAGS_DEFAULT):
        self._transport = transport
        self._use_tags = use_tags
        self._counter = 0
        # Requests refused as too long (not sent); the exit status is 2 when any.
        self.refused = 0

    def _next_tag(self):
        self._counter = (self._counter + 1) % app_settings.TAG_COUNTER_MODULUS
        return f"{app_settings.TAG_PREFIX}{self._counter:04x}"

    @classmethod
    def encoded_len(cls, line, use_tags):
        """Bytes of the request as it goes out, tag included, without the line end."""
        return len(line) + (cls.TAG_LEN if use_tags else 0)

    def request(self, line):
        """Sends `line` and returns the Answer, or None on timeout.

        Raises LineTooLong, before anything is written, when the whole request (tag included)
        is longer than the IPMC accepts. With tags on, an answer whose final line carries another
        tag is an old answer and is skipped, data lines and all.
        """
        if self.encoded_len(line, self._use_tags) > app_settings.line_max():
            raise LineTooLong(
                f"a request is longer than {app_settings.line_max()} bytes"
                f"{' with its tag' if self._use_tags else ''}"
            )
        tag = self._next_tag() if self._use_tags else None
        text = f"#{tag} {line}" if tag else line
        self._transport.write(text.encode("ascii") + REQUEST_END)
        collected = []
        while True:
            received = self._transport.read_line()
            if received is None:
                return None
            if received.startswith(DEVICE_PROMPT):
                continue
            if not is_final(received):
                collected.append(received)
                continue
            answer = Answer(collected, received)
            if tag is None or answer.tag == tag:
                return answer
            collected = []


def run_requests(client, lines, out=print):
    """Sends each line and prints the data lines and the final line of its answer.

    Returns True when every answer is OK.
    """
    all_ok = True
    for line in lines:
        try:
            answer = client.request(line)
        except LineTooLong as error:
            out(f"(not sent: {error})")
            client.refused += 1
            all_ok = False
            continue
        if answer is None:
            out("(no answer)")
            all_ok = False
            continue
        for printed in answer.lines:
            out(printed)
        out(answer.final)
        all_ok = answer.ok and all_ok
    return all_ok


def run_interactive(client, read_line=input, out=print):
    """Prompts for request lines until end of input, `quit` or `exit`; True when all were OK."""
    all_ok = True
    while True:
        try:
            line = read_line(DEVICE_PROMPT)
        except EOFError:
            return all_ok
        line = line.strip()
        if line.lower() in ("quit", "exit"):
            return all_ok
        if line:
            all_ok = run_requests(client, [line], out) and all_ok


def run_raw(transport, next_key, out, idle=lambda: None):
    """A terminal: every key from `next_key()` (bytes, or None when no key is waiting) goes to
    the port, every byte that comes back goes to `out` (a function taking bytes).

    Ends when the key is Ctrl+] (RAW_QUIT, not sent) or `idle()` returns True.
    """
    while True:
        key = next_key()
        if key == RAW_QUIT:
            return
        if key:
            transport.write(key)
        received = transport.read_available()
        if received:
            out(received)
        if idle():
            return


def windows_key():
    """The next key typed at the Windows console as bytes, or None when none waits."""
    import msvcrt  # pylint: disable=import-outside-toplevel,import-error

    if not msvcrt.kbhit():
        return None
    char = msvcrt.getwch()
    return b"\x08" if char == "\x08" else char.encode("ascii", errors="replace")


def write_stdout(data):
    """Writes received bytes to the terminal as they are."""
    sys.stdout.write(data.decode("ascii", errors="replace"))
    sys.stdout.flush()


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description="Bench client for the IPMC application port.")
    parser.add_argument("--port", default=app_settings.PORT_DEFAULT, help="serial port (COM5)")
    parser.add_argument(
        "--baud", type=int, default=app_settings.baud_default(), help="baud rate (8N1)"
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=app_settings.ANSWER_TIMEOUT_S,
        help="seconds to wait for one answer line",
    )
    parser.add_argument("--no-tags", action="store_true", help="send requests without a tag")
    parser.add_argument(
        "--raw", action="store_true", help="a terminal: keys go to the port as typed, Ctrl+] ends"
    )
    parser.add_argument("requests", nargs="*", help="request lines; none starts the prompt")
    return parser.parse_args(argv)


def main(argv=None, transport_factory=SerialTransport, next_key=windows_key):
    args = parse_args(argv)
    longest = max((Client.encoded_len(r, not args.no_tags) for r in args.requests), default=0)
    if longest > app_settings.line_max():
        print(
            f"a request is longer than {app_settings.line_max()} bytes"
            f"{'' if args.no_tags else ' with its tag'}",
            file=sys.stderr,
        )
        return 2
    transport = transport_factory(args.port, args.baud, args.timeout)
    refused = 0
    try:
        if args.raw:
            run_raw(transport, next_key, write_stdout)
            return 0
        client = Client(transport, use_tags=not args.no_tags)
        if args.requests:
            ok = run_requests(client, args.requests)
        else:
            ok = run_interactive(client)
        refused = client.refused
    finally:
        transport.close()
    if refused:
        return 2
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
