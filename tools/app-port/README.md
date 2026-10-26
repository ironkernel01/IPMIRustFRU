# Application port bench client

`app_cli.py` talks to the application port of the IPMC (APP_UART, USART1 PB6 TX / PB7 RX)
through a USB serial adapter. The protocol is in `crates/ipmc-app` (module comment of
`src/lib.rs`): one ASCII request line; the answer is zero or more `KEY: value` data lines and one
final line, `OK` or `ERROR <CODE>: <sentence>`. The IPMC also prints the prompt `app> ` and echoes
what it receives; the client skips both.

## Setup

The tool runs with the python-ipmi virtual environment and pyserial:

```powershell
tools\python-ipmi\.venv\Scripts\python.exe -m pip install pyserial
```

Wiring: adapter RX to PB6, adapter TX to PB7, ground to ground, 3.3 V logic. The bench
adapter is COM5; the board console stays on COM3 (one program holds a port at a time).

## Run

```powershell
$py = "tools\python-ipmi\.venv\Scripts\python.exe"
& $py tools\app-port\app_cli.py --port COM5 INFO STATUS "FRU STATE" "FRU FIELDS"
& $py tools\app-port\app_cli.py --port COM5 "FRU SET BOARD_SERIAL 00:11:22:33:44:55" "FRU GET BOARD_SERIAL" "FRU SAVE"
& $py tools\app-port\app_cli.py --port COM5          # interactive prompt, quit or Ctrl+Z to leave
& $py tools\app-port\app_cli.py --port COM5 --raw    # a terminal: keys go to the port, Ctrl+] ends
```

Options: `--baud` (default `IPMC_APP_UART_BAUD` of `crates/ipmc-defs/src/defs.rs`), `--timeout` (seconds to
wait for an answer line), `--no-tags` (send requests without a tag), `--raw` (a terminal, see
below). Each request carries a tag (`#t5a31 STATUS`); the IPMC puts it on the final line
(`OK #t5a31`) and the client skips an answer with another tag. The tags of a run count up from a
value taken from the clock, so a late answer of an earlier run is not matched. An untagged
`ERROR LINE_DAMAGED`, `BAD_CHAR` or `LINE_TOO_LONG` line (the IPMC lost the request or could not
read it) ends the answer of the request in progress and is printed. The client clears the bytes
waiting in the port when it opens it.

The client follows the parsing rule of the protocol: skip the lines that start with `app> `
(prompt and echo), collect `KEY: value` lines, end the answer at the first line that is `OK`,
starts with `OK ` or starts with `ERROR `. It prints the data lines and the final line of each
answer. `--raw` passes every key to the port and every received byte to the screen, like a
terminal program (Windows console; Ctrl+] ends it).

The exit status is 0 when every answer is `OK`, 1 for an `ERROR` answer or no answer, 2 for a
request, tag included, longer than `IPMC_APP_LINE_MAX` or holding a character that is not ASCII
(it is not sent and a message says why).

## Settings

`app_settings.py` holds every value the tool chooses (default port, answer timeout, tag
form). The baud rate and the line limit come from `crates/ipmc-defs/src/defs.rs`, so the tool and the
IPMC agree.

## Tests

```powershell
cd tools\app-port
..\python-ipmi\.venv\Scripts\python.exe -m unittest test_app_cli -v
```

The tests use a fake transport; no port is opened.
