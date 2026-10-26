# Application port bench client

`app_cli.py` talks to the application port of the IPMC (APP_UART, USART1 PB6 TX / PB7 RX)
through a USB serial adapter. The protocol is in `crates/ipmc-app` (module comment of
`src/lib.rs`): one ASCII request line, one `OK ...` or `ERR <CODE> <text>` answer line.

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
& $py tools\app-port\app_cli.py --port COM5 HELLO STATUS "FRU STATE" FIELDS
& $py tools\app-port\app_cli.py --port COM5 "SET BOARD_SERIAL 00:11:22:33:44:55" "GET BOARD_SERIAL" SAVE
& $py tools\app-port\app_cli.py --port COM5          # interactive prompt, quit or Ctrl+Z to leave
```

Options: `--baud` (default `IPMC_APP_UART_BAUD` of `ipmi_defs.h`), `--timeout` (seconds to
wait for an answer), `--no-tags` (send the bare protocol). Each request carries a tag
(`#t0001 STATUS`); the IPMC echoes it and the client skips an answer with another tag.

The exit status is 0 when every answer is `OK`, 1 for an `ERR` answer or no answer, 2 for a
request longer than `IPMC_APP_LINE_MAX`.

## Settings

`app_settings.py` holds every value the tool chooses (default port, answer timeout, tag
form, prompt). The baud rate and the line limit come from `ipmi_defs.h`, so the tool and the
IPMC agree.

## Tests

```powershell
cd tools\app-port
..\python-ipmi\.venv\Scripts\python.exe -m unittest test_app_cli -v
```

The tests use a fake transport; no port is opened.
