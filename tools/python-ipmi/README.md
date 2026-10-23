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
