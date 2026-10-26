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

The client keeps in step with the IPMC's prompt. Before its first request, and before the next
one after a request that timed out (no answer, or a line that did not end), it sends one CR and
reads until the prompt arrives (`Transport.sync`, bounded by the answer timeout). A line counts as
the echo of a request only when it starts with the prompt followed by exactly the request text;
without tags a final line is trusted only after that echo (or when it is one of the untagged
errors of a lost request), so the echo of a request such as `OK` is never taken for its answer.

By default each request gets a tag (`#t0001 ...`); the IPMC puts it on the final line, and the
client discards an answer with another tag (a late answer of an earlier request); the tags of
a run start from the clock, so an answer of an earlier run is not matched either. The client
prints the data lines and the final line of each answer. The exit status is 0 when every request
got an `OK` answer, 1 when one got `ERROR` or no answer, and 2 when a request, tag included, is
longer than the IPMC accepts or holds a character that is not ASCII (it is not sent, a message
says why; in the prompt the line is refused and the prompt goes on).
"""

import argparse
import re
import sys

import app_settings

# The protocol line end for requests (the IPMC accepts CR, LF or CR LF). Answers end in CR LF.
REQUEST_END = b"\r\n"
# The one CR that synchronises the client with the prompt: the IPMC answers an empty line with the
# prompt alone.
SYNC_REQUEST = b"\r"
# The prompt the IPMC prints after the banner and after every answer; it also starts the echo line
# (defs.rs IPMC_APP_PROMPT, read through app_settings).
DEVICE_PROMPT = app_settings.prompt()
# A tag is `#` and 1 to 8 letters or digits, then a space (crates/ipmc-app/src/engine.rs).
TAG_PATTERN = re.compile(r"^#[A-Za-z0-9]{1,8} ")
# A data line: KEY in capitals and underscores, a colon, a blank, the value (maybe empty).
DATA_PATTERN = re.compile(r"^([A-Z][A-Z0-9_]*): (.*)$")
# A final line: OK, or ERROR with the code and the sentence; the tag follows the first word.
FINAL_PATTERN = re.compile(r"^(OK|ERROR)(?: #([A-Za-z0-9]{1,8}))?(?: ([A-Z][A-Z0-9_]*): (.*))?$")
# The codes of an ERROR final line the IPMC answers without a tag because the request was lost or
# could not be read (crates/ipmc-app/src/lib.rs): such a line ends the current request's answer.
UNTAGGED_ERROR_CODES = ("LINE_DAMAGED", "BAD_CHAR", "LINE_TOO_LONG")
# Ctrl+] ends --raw (the telnet escape).
RAW_QUIT = b"\x1d"


class SerialTransport:
    """A serial port that writes bytes and reads one line (pyserial, imported on first use)."""

    def __init__(self, port, baud, timeout_s):
        import serial  # pylint: disable=import-outside-toplevel

        self._serial = serial.Serial(port, baud, timeout=timeout_s)
        # Bytes the IPMC sent before this run (a banner, a late answer) are not ours.
        self._serial.reset_input_buffer()

    def write(self, data):
        self._serial.write(data)
        self._serial.flush()

    def sync(self, prompt):
        """Clears the input, sends one CR and reads until the prompt arrives (the IPMC answers an
        empty line with the prompt alone). True when the prompt came before the timeout."""
        self._serial.reset_input_buffer()
        self.write(SYNC_REQUEST)
        expected = prompt.encode("ascii")
        got = self._serial.read_until(expected, app_settings.SYNC_READ_MAX)
        return got.endswith(expected)

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


class RequestRefused(ValueError):
    """The client does not send the request (nothing was written)."""


class LineTooLong(RequestRefused):
    """A request, tag included, is longer than the IPMC accepts (nothing was written)."""


class NotAscii(RequestRefused):
    """A request holds a character that is not ASCII (the port is plain ASCII)."""


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

    def __init__(self, transport, use_tags=app_settings.TAGS_DEFAULT, counter_start=None):
        self._transport = transport
        self._use_tags = use_tags
        # The counter before the first tag; the first tag is the next value. Default: the clock.
        self._counter = (
            app_settings.tag_counter_start() if counter_start is None else counter_start
        )
        # Requests refused as too long (not sent); the exit status is 2 when any.
        self.refused = 0
        # The next request first synchronises with the prompt: true before the first request and
        # after a request that got no complete answer.
        self._needs_sync = True

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
        is longer than the IPMC accepts, and NotAscii when it holds a character that is not
        ASCII. With tags on, an answer whose final line carries another tag is an old answer and
        is skipped, data lines and all; the exception is an untagged ERROR line that says the
        request was lost or unreadable (`UNTAGGED_ERROR_CODES`): it ends this request's answer.
        """
        if not line.isascii():
            raise NotAscii("a request holds a character that is not ASCII")
        if self.encoded_len(line, self._use_tags) > app_settings.line_max():
            raise LineTooLong(
                f"a request is longer than {app_settings.line_max()} bytes"
                f"{' with its tag' if self._use_tags else ''}"
            )
        # sync reads the prompt, so the echo of the request sent right after it arrives without
        # the prompt in front.
        after_sync = self._needs_sync
        if self._needs_sync:
            if not self._transport.sync(DEVICE_PROMPT):
                return None
            self._needs_sync = False
        tag = self._next_tag() if self._use_tags else None
        text = f"#{tag} {line}" if tag else line
        self._transport.write(text.encode("ascii") + REQUEST_END)
        answer = self._read_answer(tag, text, after_sync)
        if answer is None:
            self._needs_sync = True
        return answer

    def _read_answer(self, tag, text, after_sync=False):
        """Reads lines until the final line of the answer to the request `text` (tag `tag`).

        `after_sync`: the prompt before this request was read by `sync`, so the first line is
        the bare echo of `text`.
        """
        collected = []
        echoed = False
        bare_echo = after_sync
        while True:
            received = self._transport.read_line()
            if received is None:
                return None
            if bare_echo and received:
                # The first line after a sync: the echo of the request when it equals it.
                bare_echo = False
                if received == text:
                    echoed = True
                    continue
            if received.startswith(DEVICE_PROMPT):
                # The prompt, alone or with the echo of a request: ours only when the text after
                # the prompt is exactly the request.
                echoed = echoed or received[len(DEVICE_PROMPT) :] == text
                continue
            if not received:
                continue
            if not is_final(received):
                if echoed or tag is not None:
                    collected.append(received)
                continue
            answer = Answer(collected, received)
            if answer.tag is None and answer.code in UNTAGGED_ERROR_CODES:
                return answer
            if tag is not None and answer.tag == tag:
                return answer
            if tag is None and echoed:
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
        except RequestRefused as error:
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
    if not all(request.isascii() for request in args.requests):
        print("a request holds a character that is not ASCII", file=sys.stderr)
        return 2
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
