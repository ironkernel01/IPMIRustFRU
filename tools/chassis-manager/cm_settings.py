"""cm_settings.py - the one place for every value the bench tools choose (decided 10-23-2026).

Chassis Manager (`chassis_manager.py`, `cm_link.py`, `ipmc_emu_interface.py`) and the conformance
adapter (`tools/conformance/adapter.py`) read their tool-chosen values here; command-line options
override the run-level ones. Values fixed by VITA 46.11 or IPMI (T3, T6, C1, T9, the sequence
width, the IPMB address of the Chassis Manager) stay in `cm_link.py` beside their rule. Values
the IPMC and a tool must agree on (IPMC_EMU_PORT) live in `ipmi_defs.h`. Each value below names
its reason; a person tuning the tools changes it here only.
"""

# ---- run behaviour (chassis_manager.py) ----

# A bus is marked failed after this many requests in a row that timed out on it while the other
# bus answered (one lost frame must not move traffic; two in a row is a pattern).
BUS_FAIL_STREAK_DEFAULT = 2
# Transmissions per discovery probe. An empty slot never answers, so one try is enough to rule it
# out, but a lost frame must not hide a present IPMC: one retry.
DISCOVERY_TRIES_DEFAULT = 2
# Sensor poll interval and run length (seconds): the captured manager polled about once a second;
# ten seconds is enough cycles to see the readings settle.
POLL_S_DEFAULT = 1.0
DURATION_S_DEFAULT = 10.0
# Read FRU Data and Get Device SDR chunk size in bytes (the captured Chassis Manager's).
CHUNK_BYTES = 16
# Bound on the SDR next-ID walk, so a looping repository ends the walk.
SDR_RECORDS_MAX = 256

# ---- transports ----

# Aardvark adapter selection and pull-ups (the bench adapter; target power stays off).
AARDVARK_PORT_DEFAULT = 0
AARDVARK_PULLUPS_DEFAULT = 1
# The IPMC's 8-bit IPMB address on the bench (hardware address strapped to slot 1).
OWN_ADDRESS_BENCH = 0x82
EMU_HOST = "127.0.0.1"
# Wait for the first `cargo run` build of ipmc-emu (a cold build is slow).
EMU_START_TIMEOUT_S = 180.0
# Wait for the new board's `#ready` after `#reset`.
EMU_RESET_TIMEOUT_S = 5.0
# After `#ready`, time for python-ipmi's receiver threads to hand the frames the old board sent
# before the reset to the collector, so the drain that follows removes them.
RESET_SETTLE_S = 0.05
# Socket read granularity of the emulator reader thread, and the connect / `#ready` waits.
EMU_RECV_TIMEOUT_S = 0.2
EMU_READY_TIMEOUT_S = 5.0
EMU_CONNECT_TIMEOUT_S = 5.0

# ---- conformance adapter ----

# Received frames held for `receive()` that no request claimed: drop the oldest half when the
# list passes the maximum (a bound on memory in a long run).
PENDING_FRAMES_MAX = 4096
PENDING_FRAMES_KEEP = 2048
# Default wait of Target.request / Target.receive in milliseconds: T3 max (VITA 46.11 Table
# 4.1.1-1), the longest a responder may take.
RECEIVE_TIMEOUT_MS_DEFAULT = 250
