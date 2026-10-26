"""app_cli.py - bench client for the IPMC application port (crates/ipmc-app).

Opens a serial port, sends one request line per command-line argument (or prompts for lines)
and prints the answers. The protocol is one ASCII line per request and one answer line per
request, `OK ...` or `ERR <CODE> <text>`; see the module comment of crates/ipmc-app/src/lib.rs.

    python app_cli.py --port COM5 HELLO STATUS "SET ASSET_TAG rack 7" SAVE
    python app_cli.py --port COM5            # interactive: type a request, Enter sends it

By default each request gets a tag (`#t0001 ...`); the IPMC echoes it at the start of the answer,
and the client discards an answer with another tag (a late answer of an earlier request). The
exit status is 0 when every request got an `OK` answer, 1 when one got `ERR` or no answer, and 2
when a request, tag included, is longer than the IPMC accepts (it is not sent; in the prompt the
line is refused and the prompt goes on).
"""

import argparse
import re
import sys

import app_settings

# The protocol line end for requests (the IPMC accepts CR, LF or CR LF). Answers end in CR LF.
REQUEST_END = b"\r\n"
# A tag is `#` and 1 to 8 letters or digits, then a space (crates/ipmc-app/src/commands.rs).
TAG_PATTERN = re.compile(r"^#[A-Za-z0-9]{1,8} ")


class SerialTransport:
    """A serial port that writes bytes and reads one line (pyserial, imported on first use)."""

    def __init__(self, port, baud, timeout_s):
        import serial  # pylint: disable=import-outside-toplevel

        self._serial = serial.Serial(port, baud, timeout=timeout_s)

    def write(self, data):
        self._serial.write(data)
        self._serial.flush()

    def read_line(self):
        """One line without its CR LF, or None when the timeout passed with no complete line."""
        raw = self._serial.readline()
        if not raw.endswith(b"\n"):
            return None
        return raw.decode("ascii", errors="replace").rstrip("\r\n")

    def close(self):
        self._serial.close()


class LineTooLong(ValueError):
    """A request, tag included, is longer than the IPMC accepts (nothing was written)."""


class Client:
    """Sends request lines through a transport and returns the matching answer lines."""

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
        """Sends `line` and returns the answer line (tag included), or None on timeout.

        Raises LineTooLong, before anything is written, when the whole request (tag included)
        is longer than the IPMC accepts. With tags on, a line that starts with another tag is
        an old answer and is skipped.
        """
        if self.encoded_len(line, self._use_tags) > app_settings.line_max():
            raise LineTooLong(
                f"a request is longer than {app_settings.line_max()} bytes"
                f"{' with its tag' if self._use_tags else ''}"
            )
        tag = self._next_tag() if self._use_tags else None
        text = f"#{tag} {line}" if tag else line
        self._transport.write(text.encode("ascii") + REQUEST_END)
        while True:
            answer = self._transport.read_line()
            if answer is None:
                return None
            if tag is None or answer.startswith(f"#{tag} "):
                return answer


def is_ok(answer):
    """True for an `OK ...` answer, with or without a tag."""
    if answer is None:
        return False
    return TAG_PATTERN.sub("", answer, count=1).startswith("OK")


def run_requests(client, lines, out=print):
    """Sends each line, prints `> request` and `< answer`; returns True when every answer is OK."""
    all_ok = True
    for line in lines:
        out(f"> {line}")
        try:
            answer = client.request(line)
        except LineTooLong as error:
            out(f"< (not sent: {error})")
            client.refused += 1
            all_ok = False
            continue
        out(f"< {answer if answer is not None else '(no answer)'}")
        all_ok = is_ok(answer) and all_ok
    return all_ok


def run_interactive(client, read_line=input, out=print):
    """Prompts for request lines until end of input, `quit` or `exit`; True when all were OK."""
    all_ok = True
    while True:
        try:
            line = read_line(app_settings.PROMPT)
        except EOFError:
            return all_ok
        line = line.strip()
        if line.lower() in ("quit", "exit"):
            return all_ok
        if line:
            all_ok = run_requests(client, [line], out) and all_ok


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
        help="seconds to wait for one answer",
    )
    parser.add_argument("--no-tags", action="store_true", help="send requests without a tag")
    parser.add_argument("requests", nargs="*", help="request lines; none starts the prompt")
    return parser.parse_args(argv)


def main(argv=None, transport_factory=SerialTransport):
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
    try:
        client = Client(transport, use_tags=not args.no_tags)
        if args.requests:
            ok = run_requests(client, args.requests)
        else:
            ok = run_interactive(client)
    finally:
        transport.close()
    if client.refused:
        return 2
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
