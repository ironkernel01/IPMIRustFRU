# python-ipmi over the Aardvark

Third-party bench tools for driving the IPMC as a Chassis Manager would,
kept in a local virtual environment (`.venv/`, not committed).

| Package | Version | Role |
|---|---|---|
| python-ipmi | 0.6.1 | IPMI library and command-line tool; `aardvark` IPMB interface; `vita` (VITA 46.11) subcommands |
| pyaardvark | 0.8.1 | Python binding to the Total Phase Aardvark API (bundles the native library for win64, Total Phase EULA in `pyaardvark/ext/LICENSE.txt`) |

## Setup

```powershell
python -m venv tools\python-ipmi\.venv
tools\python-ipmi\.venv\Scripts\python.exe -m pip install python-ipmi pyaardvark
```

The Total Phase USB driver is installed on this PC. pyaardvark finds the
adapter as port 0, serial 2239-477438, firmware 3.51, API 6.00.

## How the `aardvark` interface works

python-ipmi enables the Aardvark as an I2C slave at its own IPMB address
(option `slave_address`, default 0x20, the VITA 46.11 Chassis Manager
address), master-writes each request frame to the target, and reads the
target's response frame from its slave buffer. One Aardvark is one bus.

## Commands

```powershell
$ipmi = "tools\python-ipmi\.venv\Scripts\ipmitool.exe"
& $ipmi -I aardvark -t 0x82 raw 0x06 0x01          # Get Device ID from the IPMC at 82h
& $ipmi -I aardvark -t 0x82 vita -h                # VITA 46.11 subcommands
& $ipmi -I aardvark -o slave_address=0x10 -t 0x82 raw 0x06 0x01   # Chassis Manager at 10h, as in the captured traffic
```

`-o` takes the interface options (`port`, `slave_address`, `serial_number`,
`enable_i2c_pullups`, `enable_target_power`, `enable_fastmode` for
400 kHz), as defined by `pyipmi/interfaces/aardvark.py`.
Keep target power off: the bench wiring carries only SCL, SDA and GND.

## Bench checks

| Date | Check | Result |
|---|---|---|
| 10-23-2026 | Address write to 41h (IPMB 82h), pull-ups on, 100 kHz, target power off | `I2C_STATUS_SLA_NACK`: bus free, no IPMC slave yet (expected before Phase 6) |

## Tier-1 commands from the command line

Every implemented Tier-1 command is reachable from `ipmitool.exe`. The prefix
of every line below is

```powershell
tools\python-ipmi\.venv\Scripts\ipmitool.exe -I aardvark -t 0x82
```

(`-t 0x82` is the IPMB address of slot 1: 80h + 2 x GA, GA = 1 on the bench
build.) The `raw` form is `raw <NetFn> <Cmd> <data bytes...>`; the printed answer is the
completion code followed by the response data, in hex. A non-zero completion code prints
alone (VITA commands add the VSO identifier `03`) and `ipmitool.exe` still exits 0, so a
script reads the printed bytes. Each process restarts the IPMB sequence number at 0, so wait
more than 3 s between two runs: the IPMC answers a repeated sequence number from its
duplicate cache.

`tools/board_test_report.py` reads this table: the generator runs the `Named form` and
`Raw form` cells of every row and checks the `Expected` cell.

| # | Command (NetFn/Cmd) | Named form | Raw form | Spec | Expected on the bench board |
|---|---|---|---|---|---|
| C01 | Get Device ID (06h/01h) | `bmc info` | `raw 0x06 0x01` | IPMI 2.0 20.1; VITA Table 10.1.1-1 p.172 | raw `00 01 80 00 01 02 09 00 00 00 4b 49 00 00 00 00`; `bmc info` shows Device ID 1, Firmware Revision 0.1, IPMI Version 2.0, Product ID 18763 (0x494b), Sensor Device, FRU Inventory Device |
| C02 | Get Self Test Results (06h/04h) | none | `raw 0x06 0x04` | IPMI 2.0 20.4; p.172 | `00 55 00` (55h no error) |
| C03 | Broadcast Get Device ID (06h/01h) | none | none | IPMI 2.0 20.9; p.172 | No command-line form: python-ipmi has no broadcast request. The Chassis Manager sends it in its discovery step (`tools\chassis-manager\chassis_manager.py`, README step 1) |
| C04 | Get Device SDR Info (04h/20h) | none | `raw 0x04 0x20` | IPMI 2.0 35.2; p.174 | `00 07 01` (7 sensors, static population) |
| C05 | Reserve Device SDR Repository (04h/22h) | none | `raw 0x04 0x22` | IPMI 2.0 35.4; p.174 | `00 xx yy`, a non-zero 16-bit reservation ID that changes with every request |
| C06 | Get Device SDR (04h/21h) | `sdr list`, `sdr showall` | `raw 0x04 0x21 0x00 0x00 0x07 0x00 0x00 0xff` (record 0007h, whole record) | IPMI 2.0 35.3; p.174 | raw `00 ff ff 07 00 51 12 0f 82 00 01 0b 00 00 00 a0 60 00 c4 49 50 4d 43` (the MC locator record, name `IPMC`); `sdr list` and `sdr showall` list the 7 Tier-1 sensors (FRU State, IPMB Link, FRU Health, FRU Voltage, FRU Temperature, Payload Results, Payload Status) and the `IPMC` locator |
| C07 | Get Sensor Reading (04h/2Dh) | `sdr showall` (reads every sensor) | `raw 0x04 0x2d 0x01` (sensor 1, IPMB Link) | IPMI 2.0 35.14; VITA Table 10.2.2-1 p.241 | `00 08 40 02` (link A enabled, B not fitted) |
| C08 | Get FRU Inventory Area Info (0Ah/10h) | none | `raw 0x0a 0x10 0x00` | IPMI 2.0 34.1; p.174 | `00 00 02 00` (512 bytes, accessed by bytes) |
| C09 | Read FRU Data (0Ah/11h) | `fru print` | `raw 0x0a 0x11 0x00 0x00 0x00 0x08` (FRU 0, offset 0, 8 bytes) | IPMI 2.0 34.2; p.174 | raw `00 08 01 00 00 01 0b 00 00 f3` (the common header); `fru print` decodes the board and product areas with the `ipmi_defs.h` strings (IronKernel, VPX Tier-1 IPMC, IK-T1-000001, IK-IPMC-T1-NUC, ipmc-fru-v1, Tier-1 IPMC Reference, IK-IPMC-T1, 0.1) |
| C10 | Write FRU Data (0Ah/12h) | none | `raw 0x0a 0x12 0x00 0x00 0x00 0x01 0x00` (FRU 0, offset 0, data `01 00`: the current first two header bytes) | IPMI 2.0 34.3; p.174 | `00 02` (2 bytes written) |
| C11 | Get VSO Capabilities (2Ch/00h) | `vita properties` | `raw 0x2c 0x00 0x03` | VITA 10.1.3.1 p.182 | raw `00 03 00 00 00 22 00 00`; `vita properties` shows Tier 1, Layer 1, 100kHz, 1 IPMB interface supported, VITA 46.11, Max FRU Device ID 0x00 |
| C12 | Set IPMB State (2Ch/09h) | none | `raw 0x2c 0x09 0x03 0xff 0xff` (no change to A or B) | VITA 10.1.3.7 p.189 | `00 03` |
| C13 | Get Device Locator Record ID (2Ch/0Dh) | none | `raw 0x2c 0x0d 0x03 0x00` (FRU 0) | VITA 10.1.3.11 p.194 | `00 03 07 00` (record 0007h) |
| C14 | Get FRU Address Info (2Ch/40h) | `vita addrinfo` | `raw 0x2c 0x40 0x03 0x00` (FRU 0) | VITA 10.1.3.3 p.186 | raw `00 03 41 82 ff 00 01 00`; `vita addrinfo` shows Hardware Address 0x41, IPMB-0 Address 0x82, FRU ID 0x00, Site ID 0x01 |

### Error probes

| # | Probe | Command | Answer |
|---|---|---|---|
| E01 | Request data too long (Get Device ID with a data byte) | `raw 0x06 0x01 0x00` | `c7` |
| E02 | VITA request data too long (Get VSO Capabilities with a data byte) | `raw 0x2c 0x00 0x03 0x00` | `c7 03` |
| E03 | FRU device ID 1 (a subsidiary FRU, Rule 4.2-3) | `raw 0x0a 0x10 0x01` | `c9` |
| E04 | FRU device ID 1 in Get FRU Address Info | `raw 0x2c 0x40 0x03 0x01` | `c9 03` |
| E05 | Command not applicable to a Tier-1 IPMC (Cold Reset, Rule 4.2-2) | `raw 0x06 0x02` | `c1` |
| E06 | Non-zero LUN (LUN 1 on Get Device ID) | `raw -l 1 0x06 0x01` | `c2` |

### Write FRU Data persists

`raw 0x0a 0x12 <fru id> <offset lo> <offset hi> <data...>` writes the FRU image.
The IPMC answers `00 <count written>` at once and saves the image to its flash log after
`IPMC_FRU_PERSIST_DELAY_MS` (1000 ms) without another write, so the bytes survive a reset.
The image is not checksum-checked on write: a write that changes a byte inside the board or
product area also has to write that area's checksum byte (the area bytes sum to 0 modulo 256),
or `fru print` and the firmware `fru show` report a bad checksum. Writes that cross the end
of the image return `c9`; with NVMRO asserted they return `80`. Console `fru restore`
puts the built image back. `board_test_report.py` section F runs this sequence.
