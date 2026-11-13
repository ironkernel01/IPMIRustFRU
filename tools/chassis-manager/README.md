# Chassis Manager emulator

A VITA 46.11 Tier-1 Chassis Manager written on python-ipmi (PLAN decision 5). It
discovers an IPMC, reads its FRU and SDRs, polls its sensors and applies the
requester rules of ANSI/VITA 46.11-2022 (250 ms response timeout, retries with
the same sequence number, IPMB-A/IPMB-B alternation). Two transports carry the
same layer:

| Transport | Peer | Buses |
|---|---|---|
| `--transport aardvark` | the board, through a Total Phase Aardvark (python-ipmi's own `aardvark` interface) | IPMB-A |
| `--transport emu` | `ipmc-emu serve`, the IPMC code on the PC, over TCP | IPMB-A and IPMB-B |

## Setup (clean virtual environment)

```powershell
python -m venv tools\python-ipmi\.venv
tools\python-ipmi\.venv\Scripts\python.exe -m pip install python-ipmi pyaardvark
```

The tool runs with this interpreter (Python 3.11 or later, python-ipmi 0.6.1,
pyaardvark 0.8.1). `pyaardvark` is needed only for `--transport aardvark`; see
`tools/python-ipmi/README.md` for the adapter.

## Run

```powershell
$py = "tools\python-ipmi\.venv\Scripts\python.exe"

# emulator: serve in one shell, the Chassis Manager in another
cargo run -q -p ipmc-emu -- serve --b-fitted 1
& $py tools\chassis-manager\chassis_manager.py --transport emu --duration-s 10 --json run.json

# failover: IPMB-A goes down halfway through the polling
& $py tools\chassis-manager\chassis_manager.py --transport emu --duration-s 10 --emu-bus-down A --json fail.json

# the board on IPMB-A (Aardvark; B is not fitted)
& $py tools\chassis-manager\chassis_manager.py --transport aardvark --duration-s 10 --json board.json

# cross-check a run against the captured Chassis Manager
& $py tools\chassis-manager\crosscheck.py run.json

# unit tests of the request policy
cd tools\chassis-manager; & ..\..\$py -m unittest test_cm_link
```

`serve` listens on the port `IPMC_EMU_PORT` in `crates/ipmc-defs/src/defs.rs` (the Chassis Manager
reads the same define as its default). The line protocol is documented in the
`ipmc-emu` crate documentation.

## Options

| Option | Default | Meaning |
|---|---|---|
| `--transport aardvark\|emu` | required | transport |
| `--host`, `--port` | 127.0.0.1, `IPMC_EMU_PORT` | emu: where `serve` listens |
| `--aardvark-port`, `--aardvark-serial`, `--aardvark-pullups` | 0, none, 1 | aardvark: adapter selection and I2C pull-ups (target power stays off) |
| `--cm-address` | 0x20 | requester IPMB address; `0x10` reproduces the captured Chassis Manager |
| `--slots` | 1..31 | slot numbers to probe (slot N is IPMB address 80h + 2N: slot 1 = 82h, slot 31 = BEh) |
| `--poll-s`, `--duration-s` | 1.0, 10.0 | polling interval and length |
| `--retries` | 3 | request retries, 3..10 (C1) |
| `--timeout-s` | 0.25 | response timeout, at most 0.25 (T3 max) |
| `--discovery-tries` | 2 | transmissions per discovery probe; an empty slot never answers |
| `--bus-fail-streak` | 2 | suspect timeouts in a row before a bus counts as failed |
| `--emu-bus-down A\|B`, `--emu-bus-down-at-s` | off, half the duration | emu: break a bus during polling |
| `--cm-tier 1\|2` | 2 | 2: Event Receiver, answers Platform Event with 00h; 1: not an Event Receiver (Rule 7-2), answers C1h |
| `--withhold-events N` | 0 | leave the first N Platform Event frames unanswered (retries count), so a run shows the IPMC retrying with the same sequence number |
| `--set-event-receiver` | off | after discovery, send Set Event Receiver (NetFn 04h, cmd 00h, data: this manager's address, 00h) to each IPMC found; the completion code is logged (a Tier-1 IPMC answers C1h, which is not an error) |
| `--cold-reset-at-s T` | off | T seconds into the polling, send Cold Reset (NetFn 06h, cmd 02h) once to each IPMC found |
| `--cold-reset-recovery-s T` | 5.0 | seconds after the Cold Reset request in which the IPMC must answer again; polling timeouts inside it are expected, later ones are lost readings, and the check "recovered after Cold Reset" needs one answered reading inside it |
| `--json PATH` | none | write the JSON summary |

Defaults that the tool chooses (failure streak, discovery tries, poll interval and
duration, chunk size, transport waits) live in `cm_settings.py`, the one settings
module of the bench tools (shared by the conformance adapter); the options above
override them.

Exit status 0 when every check of the run passed, 1 otherwise. The log prints each
check (`check <name> of <address>: PASS|FAIL`) and the summary lists them under
`checks`: an IPMC discovered (by the direct or the broadcast Get Device ID), Get
VSO Capabilities, Set IPMB State, Get Device Locator Record ID, Get FRU Inventory
Area Info, the FRU read complete and decoded, Get Device SDR Info, the reservation,
the SDR walk complete (ends at FFFFh, every record ID as asked, sensor count equal
to Get Device SDR Info), the locator record ID found in the SDRs, Get Self Test
Results, polling (cycles ran, no timeout, no error completion code) and no request to a
found IPMC timed out.

## Event Receiver (`cm_events.py`)

The Chassis Manager at 20h is the Event Receiver of a VITA 46.11 system (Rules 7-3, 7-5). A
request addressed to it (even NetFn, both checksums valid) is not a response: `Link` hands it
to `EventReceiver` while a request waits and between requests (the run loop receives in slices
of `SERVICE_PERIOD_S`, 10 ms, so a request is answered promptly). The answer goes back on the
bus the request arrived on, built with the same frame builder as the requests (rqSA of the
request, NetFn + 1, the request's sequence number and LUNs, our address as rsSA).

- Platform Event (NetFn 04h, cmd 02h): answered 00h at `--cm-tier 2`, C1h at `--cm-tier 1`
  (recorded as refused). Any other request to the Chassis Manager is answered C1h and listed
  under `other_requests`.
- The first `--withhold-events` Platform Event frames are not answered and are recorded as
  withheld.
- A repeat of a request (same rqSA, sequence number, command and data within T9, 5 s) is
  answered again and recorded as a duplicate of the first, not as a new event.
- The log prints one line per event: bus, requester, sequence number, EvMRev, sensor type and
  number, direction, event type, ED1..ED3 and a text for the VITA sensors: FRU State (F0h:
  current state, previous state, cause of Table 10.2.1-3), System IPMB Link (F1h: A/B on/off,
  400 kHz), FRU Health (F2h), FRU Voltage (02h), FRU Temperature (F3h: threshold, assert or
  deassert), Payload Test Results (F4h), Payload Test Status (F5h), Payload Mode (F7h), IPMC
  Reset Type (F8h). Any other sensor type prints its event data in hex.
- After Cold Reset the polling continues: a missing response to the reset, and the polling
  timeouts until the IPMC answers again, are logged as expected (`reset_lost_readings` under
  `poll`) and do not fail the run, but only within `--cold-reset-recovery-s` of the reset: after
  that a timeout is a lost reading, and the check "recovered after Cold Reset" fails unless a
  reading was answered inside that time. The run keeps receiving events.

## Steps

1. Discovery: Get Device ID to every slot address and the Broadcast Get Device
   ID form (I2C general call, built with python-ipmi's `encode_ipmb_msg`).
2. Get VSO Capabilities; with two IPMB interfaces reported and IPMB-B on the
   transport, Set IPMB State enables IPMB-B (Rule 4.1.1-4, Obs 4.1.1-2).
   Get Device Locator Record ID (FRU 0, section 10.1.3.11) follows, as the
   captured Chassis Manager sends it.
3. Get FRU Inventory Area Info, Read FRU Data in 16-byte chunks, decode the
   common header, board and product areas with python-ipmi's FRU decoder.
4. Get Device SDR Info, Reserve Device SDR Repository, every record by its
   next-ID chain (header, then 16-byte chunks), decoded with python-ipmi's SDR
   decoder. The record ID that Get Device Locator Record ID (step 2) returned
   must be the Management Controller Device Locator record read here; a
   mismatch or a missing answer makes the run exit 1.
5. Get Self Test Results once, then Get Sensor Reading for every sensor each
   `--poll-s`; only changes print.

## Request policy (`cm_link.py`)

| Constant | Value | Rule |
|---|---|---|
| `T3_MIN_S`, `T3_MAX_S` | 0.060 s, 0.250 s | response time-out, Rule 4.1.1-9; the Chassis Manager refuses a time-out outside the range (unit tests may shorten it with `test_mode`) |
| `T6_MIN_S` | 0.060 s | time between retries after a failed transmission, Table 4.1.1-1 |
| `C1_MIN`, `C1_MAX` | 3, 10 | request retry count, Rule 4.1.1-10 |
| retries keep the sequence number | | Obs 4.1.1-6 |
| `T9_S` | 5 s | a sequence number is not reused sooner for the same target and command (sequence value expiration, Table 4.1.1-1); such a request waits |
| retry on the other bus | | Rule 5.1.1-6 |
| messages alternate between buses | first on IPMB-A; the next bus follows the last transmission, retries included | Rule 5.1.1-5 |
| IPMB-A only until IPMB-B is enabled | per target: after its Get VSO Capabilities reported two interfaces and its Set IPMB State succeeded | Rule 4.1.8-2 |

A bus that times out for requests the other bus answers (`--bus-fail-streak`
times in a row) is marked failed for that target: its first tries go to the other
bus, and Set IPMB State puts the failed bus of that target in Override (section
9.3). A timeout for one target never marks a bus failed for another.

## Output

The run log prints `[seconds] command to address seq bus (round trip) tx <frame>
rx <frame> -> decoded result`. The JSON summary holds the totals, one entry per
command (requests, transmissions, responses, retries, timeouts, completion codes,
buses), the distinct request/response pairs and the run-length command sequence.
It also holds `events` (one entry per Platform Event frame received: `t_s`, `bus`, `rq_sa`,
`seq`, `ev_m_rev`, `sensor_type`, `sensor_number`, `direction`, `event_type`, `ed1`..`ed3`,
`text`, `duplicate`, `withheld`, `answered_cc` (null when withheld)), `other_requests` (requests
to the Chassis Manager that are not Platform Events, answered C1h) and the counts
`events_received` (distinct events), `events_duplicates`, `events_withheld` and
`events_refused` (Tier-1 answers).

## Cross-check (`crosscheck.py`)

Reads `local_only/vectors/sbc_ab.vec` (local only; absent: prints `skipped`, exit 0) and a
JSON summary. It compares the command mix and first-appearance order, the bus
of the responses, and the IPMC's responses to the same requests. Every
difference prints as `expected:<reason>` (the tables at the top of the file name
each reason) or `unexpected`; exit status 1 when any is unexpected. A reason
covers named bytes and bits of a response only; any other differing bit is
unexpected. A response may lack bytes only past the command's mandatory length
(`LENGTH_VARIES`); a request answered in one run and timed out in the other, and
any timeout of ours to the IPMC, is unexpected; a request-count difference is
expected only when the count it explains is verified from the run data
(`COUNT_CHECKS`: records and chunks, reservations, poll cycles times sensors,
FRU chunks); bus alternation is expected only when our consecutive transmissions
alternate after Set IPMB State. When our run's Get VSO Capabilities reports one IPMB interface
(IPMB-B not fitted, the bench build), the differences that follow from it (no
Set IPMB State, the interface count, the IPMB-B half of the IPMB Link sensor,
every response on IPMB-A) are expected; with two interfaces they are not.
