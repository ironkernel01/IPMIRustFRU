"""app_settings.py - the one place for every value `app_cli.py` chooses (tools rule, decided
10-23-2026: one settings module per tool family; command-line options override the run-level ones).

Values the IPMC and the tool must agree on live in `ipmi_defs.h` (the line length
IPMC_APP_LINE_MAX, the baud rate IPMC_APP_UART_BAUD); the tool reads them from the header when
it finds it, so the two cannot drift. Values fixed by the protocol (the line end, the tag rules)
sit in `app_cli.py` beside their rule.
"""

import re
from pathlib import Path

# The bench adapter on the application port (USART1 PB6/PB7 through a USB serial adapter).
PORT_DEFAULT = "COM5"

# Baud rate used when ipmi_defs.h is not found (a copy of the tool away from the repository).
BAUD_FALLBACK = 115200

# Longest request line used when ipmi_defs.h is not found.
LINE_MAX_FALLBACK = 128

# Longest wait for one answer line, seconds. The IPMC answers within one main-loop pass; a local
# IPMI request or a SET adds a few milliseconds, so two seconds only expires when nothing answers.
ANSWER_TIMEOUT_S = 2.0

# Tag the tool puts in front of each request so an answer is matched to its request (the IPMC
# echoes it). Disable with --no-tags to see the bare protocol.
TAGS_DEFAULT = True

# A tag is the prefix and a counter in hex; the counter wraps at this many requests (the tag
# stays within the protocol's 8 letters or digits).
TAG_PREFIX = "t"
TAG_COUNTER_MODULUS = 0x1_0000

# Prompt of the interactive mode.
PROMPT = "app> "

DEFS_PATH = Path(__file__).resolve().parents[2] / "ipmi_defs.h"


def define_from_header(name, default, path=DEFS_PATH):
    """The integer value of `#define name <integer>` in ipmi_defs.h, or `default`."""
    try:
        text = Path(path).read_text(encoding="ascii", errors="replace")
    except OSError:
        return default
    found = re.search(rf"^#define\s+{re.escape(name)}\s+(\d+)\b", text, re.MULTILINE)
    return int(found.group(1)) if found else default


def baud_default(path=DEFS_PATH):
    """IPMC_APP_UART_BAUD of the header, else BAUD_FALLBACK."""
    return define_from_header("IPMC_APP_UART_BAUD", BAUD_FALLBACK, path)


def line_max(path=DEFS_PATH):
    """IPMC_APP_LINE_MAX of the header (the longest request line), else LINE_MAX_FALLBACK."""
    return define_from_header("IPMC_APP_LINE_MAX", LINE_MAX_FALLBACK, path)
