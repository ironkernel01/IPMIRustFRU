# IPMB capture tool

`capture2pcap.py` turns the console output of the `DEBUG DUMP` command into a pcap file that
Wireshark opens, and into a CSV file that a spreadsheet opens. The IPMC keeps the capture log in
RAM (`crates/ipmc-log`): a ring of every whole IPMB frame it received (before the
parser) and sent (with the outcome), plus the trace and fault lines the console prints.

## Customer steps (on the target system)

1. Connect a terminal to the console (115200 8N1) and start a terminal log.
2. `DEBUG LOG ON` (the log is on at start-up when `IPMC_LOG_AT_BOOT` is 1).
3. Reproduce the problem.
4. `DEBUG DUMP` prints the log as CSV; `DEBUG DUMP TRACE` prints it as text lines. Use `DEBUG
   DUMP` (CSV) when the dump goes back to us.
5. Save the terminal log and mail it. `DEBUG CLEAR` empties the log before the next run.

The ring holds `IPMC_LOG_SIZE` bytes; when it is full the oldest records are dropped, and the
header line of the dump says how many (`overwritten`).

## Our steps

```powershell
python -I tools\capture\capture2pcap.py dump.txt -o dump          # dump.pcap
python -I tools\capture\capture2pcap.py dump.txt -o dump --csv    # dump.pcap and dump.csv
```

Open `dump.pcap` in Wireshark (link type 260, IPMI trace). Each record is one packet, stamped
with its `time_ms`; a failed transmission shows as the frame followed by a text packet `tx
failed: <status>`; trace lines show as text packets. The log file may hold several dumps and any
other terminal text: only the lines between `# IPMC capture:` and `# end of capture` are read.

The CSV (`dump.csv`) has the columns `rec,time_ms,type,bus,dir,status,len,summary,bytes`; it
opens in a spreadsheet as it is.

## Tests

```powershell
cd tools\capture
python -I -m unittest
```

The tshark check runs when `C:\Program Files\Wireshark\tshark.exe` exists and is skipped
otherwise.
