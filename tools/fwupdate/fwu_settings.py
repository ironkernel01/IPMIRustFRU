"""fwu_settings.py - the one place for every value `fwupdate.py` chooses (tools rule: one settings
module per tool family; command-line options override the run-level ones).

Values the bootloader and the tool must agree on live in `crates/ipmc-defs/src/defs.rs` (the baud
rates IPMC_CONSOLE_BAUD and IPMC_APP_UART_BAUD, the prompt IPMC_BOOT_PROMPT, the line limit
IPMC_MENU_LINE_MAX); the tool reads them with `tools/defs.py` when it finds the file, so the two
cannot drift. Values fixed by the protocol or the image format sit in `fwupdate.py` beside their rule.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import defs  # noqa: E402  (tools/defs.py, the one reader of the configuration)

# No default port: --port is required (the bench PC uses COM3 for the CLI and COM5 for the APP port).
PORT_DEFAULT = None

# Seconds between two WAKEUP lines while the tool waits for the bootloader.
WAKEUP_INTERVAL_S = 0.2

# Seconds the wakeup action waits before it gives up. --field waits without limit.
WAKEUP_GIVE_UP_S = 60.0

# The one timeout the serial port is opened with, seconds. The transport never assigns `.timeout`
# again: on Windows that reconfigures the port, and the ST-Link virtual COM driver drops the bytes
# arriving at that moment (INFO lost about 6 of 700 bytes). Each call's own timeout is a deadline
# on the monotonic clock over reads of this length.
POLL_S = 0.02

# Most lines read after a runtime entry command (`DEBUG BOOTLOADER`, `BOOTLOADER`) while waiting
# for the line that ends it; the read stops earlier at the first ending line.
ENTER_LINES_MAX = 6

# Seconds to wait for the status line of one command.
ANSWER_TIMEOUT_S = 2.0

# Seconds to wait for the answer to one record line.
RECORD_TIMEOUT_S = 2.0

# Seconds to wait for the answer to CONFIRM after an erase (the five sectors 3-7 take up to 8.5 s
# by the datasheet maxima, about 3.5 s measured).
ERASE_TIMEOUT_S = 20.0

# Data bytes in one record the tool sends.
RECORD_BYTES = 32

# Times a record is sent again after ERR FORMAT, ERR CHECKSUM or ERR LINE_TOO_LONG.
RECORD_RETRIES = 3

# Times LOAD is started again after `ERR ABORTED` (a record line that lost its leading `:`) in
# one update; one more is a refusal.
LOAD_RESUMES = 3

# Seconds to wait for the prompt after the status line of WAKEUP.
PROMPT_TIMEOUT_S = 2.0

# Seconds the tool stays quiet after the first OK to WAKEUP, then discards what arrived: one
# repeat interval plus a margin, so an OK answering a WAKEUP already in flight is not taken for the
# status line of the next command.
SETTLE_S = 0.5

# Values used when defs.rs is not found (a copy of the tool away from the repository).
BAUD_FALLBACK = 115200
PROMPT_FALLBACK = "BOOT> "
LINE_MAX_FALLBACK = 128

DEFS_PATH = defs.DEFAULT_PATH


def define_from_header(name, default, path=DEFS_PATH):
    """The value of `pub const name` in defs.rs, or `default` when the file or the name is absent."""
    try:
        return defs.read(path).get(name, default)
    except OSError:
        return default


def baud_for(uart, path=DEFS_PATH):
    """The baud rate of the port `uart` ("cli": IPMC_CONSOLE_BAUD, "app": IPMC_APP_UART_BAUD)."""
    name = "IPMC_CONSOLE_BAUD" if uart == "cli" else "IPMC_APP_UART_BAUD"
    return define_from_header(name, BAUD_FALLBACK, path)


def prompt(path=DEFS_PATH):
    """IPMC_BOOT_PROMPT of defs.rs, else PROMPT_FALLBACK."""
    return define_from_header("IPMC_BOOT_PROMPT", PROMPT_FALLBACK, path)


def line_max(path=DEFS_PATH):
    """IPMC_MENU_LINE_MAX of defs.rs (a record line is at most this minus 1), else LINE_MAX_FALLBACK."""
    return define_from_header("IPMC_MENU_LINE_MAX", LINE_MAX_FALLBACK, path)
