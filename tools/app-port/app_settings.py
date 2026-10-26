"""app_settings.py - the one place for every value `app_cli.py` chooses (tools rule, decided
10-23-2026: one settings module per tool family; command-line options override the run-level ones).

Values the IPMC and the tool must agree on live in `crates/ipmc-defs/src/defs.rs` (the line
length IPMC_APP_LINE_MAX, the baud rate IPMC_APP_UART_BAUD, the prompt IPMC_APP_PROMPT); the tool reads them with
`tools/defs.py` when it finds the file, so the two cannot drift. Values fixed by the protocol (the line end, the tag rules)
sit in `app_cli.py` beside their rule.
"""

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import defs  # noqa: E402  (tools/defs.py, the one reader of the configuration)

# The bench adapter on the application port (USART1 PB6/PB7 through a USB serial adapter).
PORT_DEFAULT = "COM5"

# Baud rate used when defs.rs is not found (a copy of the tool away from the repository).
BAUD_FALLBACK = 115200

# Longest request line used when defs.rs is not found.
LINE_MAX_FALLBACK = 128

# Longest wait for one answer line, seconds. The IPMC answers within one main-loop pass; a local
# IPMI request or a SET adds a few milliseconds, so two seconds only expires when nothing answers.
ANSWER_TIMEOUT_S = 2.0

# Most bytes the client reads while it waits for the prompt after the CR that synchronises it with
# the IPMC (the banner or a late answer can come first).
SYNC_READ_MAX = 4096

# Tag the tool puts in front of each request so an answer is matched to its request (the IPMC
# echoes it). Disable with --no-tags to see the bare protocol.
TAGS_DEFAULT = True

# A tag is the prefix and a counter in hex; the counter wraps at this many requests (the tag
# stays within the protocol's 8 letters or digits).
TAG_PREFIX = "t"
TAG_COUNTER_MODULUS = 0x1_0000


def tag_counter_start():
    """The counter value the first tag of a run uses. It follows the clock (seconds), so a run
    does not repeat the tags of the run before it and a late answer of an earlier run, still in
    the IPMC output, is not taken for an answer of this one."""
    return int(time.time()) % TAG_COUNTER_MODULUS


DEFS_PATH = defs.DEFAULT_PATH


def define_from_header(name, default, path=DEFS_PATH):
    """The value of `pub const name` in defs.rs, or `default` when the file or the name is absent."""
    try:
        return defs.read(path).get(name, default)
    except OSError:
        return default


def baud_default(path=DEFS_PATH):
    """IPMC_APP_UART_BAUD of defs.rs, else BAUD_FALLBACK."""
    return define_from_header("IPMC_APP_UART_BAUD", BAUD_FALLBACK, path)


def line_max(path=DEFS_PATH):
    """IPMC_APP_LINE_MAX of defs.rs (the longest request line), else LINE_MAX_FALLBACK."""
    return define_from_header("IPMC_APP_LINE_MAX", LINE_MAX_FALLBACK, path)


# The prompt used when defs.rs is not found.
PROMPT_FALLBACK = "APP> "


def prompt(path=DEFS_PATH):
    """IPMC_APP_PROMPT of defs.rs (the prompt the IPMC prints and starts its echo line with),
    else PROMPT_FALLBACK."""
    return define_from_header("IPMC_APP_PROMPT", PROMPT_FALLBACK, path)
