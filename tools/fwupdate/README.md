# fwupdate

Host update tool for the IPMC bootloader (`crates/ipmc-boot`). It wakes the bootloader on the console
(CLI) port or the application (APP) port, erases the runtime firmware sectors, loads an Intel HEX file
record by record, verifies the image and starts it.

An update erases sectors 3 to 7 (the runtime firmware) only. The FRU store (sector 2) and the
bootloader (sectors 0-1) stay.

## Setup

Python 3 and `pip install pyserial` (imported on first use; the unit tests need no serial port).
Every value the tool chooses is in `fwu_settings.py`. The baud rates, the prompt and the line limit
come from `crates/ipmc-defs/src/defs.rs` (`IPMC_CONSOLE_BAUD`, `IPMC_APP_UART_BAUD`,
`IPMC_BOOT_PROMPT`, `IPMC_MENU_LINE_MAX`); the fallbacks apply when that file is absent.

## Bench ports

On this bench PC COM3 is the CLI port and COM5 is the APP port; any PC port works. `--port` is
required. `--uart cli|app` (default `app`) names what the port is and picks the baud rate;
`--baud N` overrides it.

## Usage

    python fwupdate.py --port COMx [--uart cli|app] [--baud N] [--enter cli|app --enter-port COMy]
                       [--confirm] [--field] [--no-precheck] [--as-is] [--no-run] <action> [file.hex]

Every action sends `WAKEUP` every 0.2 s first (60 s limit; `--field` has none) and prints when it
sees the countdown line and when the `BOOT> ` prompt arrives.

| Action | What it does |
|---|---|
| `wakeup` | wakes the bootloader and stops |
| `info`, `verify`, `run`, `reset`, `help` | one command; prints its output; exit 0 on `OK` |
| `erase-app`, `erase-fru` | the erase command, then `CONFIRM`; waits up to 20 s for the answer |
| `update file.hex` | pre-check, erase APP, load, verify, run |

Examples:

    python fwupdate.py --port COM5 info
    python fwupdate.py --port COM3 --uart cli verify
    python fwupdate.py --port COM5 update ipmc-nucleo-f401re.hex
    python fwupdate.py --port COM5 --enter app --enter-port COM5 update app.hex
    python fwupdate.py --port COM3 --uart cli --enter cli --enter-port COM3 update app.hex

`--enter` first opens `--enter-port` at the baud rate of that runtime port, sends the entry command
(`DEBUG BOOTLOADER` on the CLI, `BOOTLOADER` on the APP port), prints the answer and closes the port.
The console answers `Restarting into the bootloader` (and `Payload power drops at the reset` while
payload power is on); the APP port answers `BOOTLOADER: restarting` then `OK`.

The reset drops payload power, so while PWR_EN is asserted the plain command only warns (the console
prints `Send DEBUG BOOTLOADER CONFIRM to restart`, the APP port answers `ERROR CONFIRM`) and does not
restart. `--confirm` sends the command with ` CONFIRM` appended from the start (power on or off).
Without it, the tool prints the answer, asks `Payload power is on. Type CONFIRM to restart into the
bootloader:` on stdin and, when the input is `CONFIRM` in any case, sends the CONFIRM form; any other
input or the end of input stops the run (`refused: entry not confirmed`, exit 1).

## What update does

1. Reads the file (Intel HEX types 00, 01, 04, 05; 02 and 03 are refused) and builds the memory image
   with FFh in gaps.
2. Pre-check, refused before anything is sent, with one line naming the rule: every byte inside
   0x0800C000..0x08080000; header magic, format, size, length (multiple of 4, 0x240 to 475136), vector
   table address; vector word 0 (8-byte aligned, SRAM) and word 1 (odd, inside the code); the SHA-256
   recomputed with `hashlib` equals the stored digest. `--no-precheck` skips it.
3. `ERASE APP` and `CONFIRM`, `LOAD`.
4. Records, one at a time, each answered before the next. By default the image is re-chunked into aligned
   32-byte records with an 04 record at each 64 KB boundary; an all-FFh chunk is skipped (the flash is
   erased). `--as-is` sends the file's own records. An `ERR FORMAT`, `ERR CHECKSUM`, `ERR LINE_TOO_LONG` or `ERR LINE_DAMAGED`
   answer is sent again up to 3 times; any other `ERR` stops the run and names the record number and the
   answer. One progress line shows records, bytes and rate. An `ERR ABORTED` answer (a damaged leading `:`
   ended record mode; the erase stays in force and every earlier record is programmed) is resumed: the
   tool prints `record N: ERR ABORTED, LOAD again from record N`, sends `LOAD`, the last 04 record sent
   before record N, then record N again, and goes on. At most 3 resumes per update (`LOAD_RESUMES`); the
   next `ERR ABORTED` stops the run. Record numbers and the progress counter stay those of the file.
5. The end record, `VERIFY`, then `RUN` (not with `--no-run` or `--field`). The digest in the `OK VERIFY valid SHA-256 <hex>` answer must equal the digest in the image header of the file (a difference is a refusal, `VERIFY digest <got> differs from the file's <want>`); with `--as-is` or a file without a readable header the tool says it did not compare. A port that cannot be opened (missing or busy) is a refusal, `refused: <port>: <message>`, exit 1.

Exit codes: 0 success, 1 a refusal or an `ERR` answer, 2 usage, 3 a timeout.

Time: a full update of today's image (139 456 bytes, 4 354 records) takes about 75 s over APP and about 40 s over CLI at 115200 baud
(the five sectors 3-7 erase in up to 8.5 s by the datasheet maxima, about 3.5 s measured).

## Field procedure

Fit the jumper from PC10 (CN7-1) to 3.3 V (CN7-5), then:

    python fwupdate.py --port COM5 --field update app.hex

The tool prints `power-cycle the board now`, waits for the bootloader without limit, updates and
verifies, then prints `remove the BOOTLOADER jumper and power-cycle the board` and sends no `RUN`.

## Tests

    cd tools/fwupdate
    python -m unittest test_fwupdate -v

The tests use a fake bootloader; no board and no serial port.
