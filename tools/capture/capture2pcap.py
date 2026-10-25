#!/usr/bin/env python3
"""capture2pcap.py - turns the console output of DEBUG DUMP into a pcap file for Wireshark.

Input: a console log holding one or more CSV blocks of the DEBUG DUMP command. Anything outside a
block ("# IPMC capture: ..." up to "# end of capture") is ignored, so a whole terminal session
can be passed in. A block holds the header line of the CSV (rec,time_ms,type,bus,dir,status,
len,summary,bytes) and one CSV row per record; a row that is not CSV (a DEBUG DUMP TRACE block)
is skipped.

Output: <out>.pcap, a classic little-endian pcap with LINKTYPE_IPMI_HPM_2 (260). One packet per
record; its pcap timestamp is the record's time_ms (seconds, microseconds), and the same value
is in the block. The block layout is Wireshark's packet-ipmi-trace.c:

    byte 0       (block type << 4) | channel      block type 0 frame, 2 text line; channel 0
    bytes 1..=4  seconds, u32 little-endian
    bytes 5..=6  milliseconds, u16 little-endian
    byte 7       data type 01h, IPMB-1.0
    bytes 8..=9  flags, u16 little-endian: bit 6 redundant channel (0 IPMB-A, 1 IPMB-B),
                 bit 7 direction (1 = to the IPM controller, an In frame)
    byte 10      data length
    bytes 11..   the data: the whole IPMB frame from rsSA, or the ASCII text

A failed Out frame is written as the frame followed by a text record "tx failed: <status>".
With --csv the CSV rows are also copied to <out>.csv.

Usage: python -I tools/capture/capture2pcap.py dump.txt -o dump [--csv]
"""

import argparse
import csv
import struct
import sys

BLOCK_START = "# IPMC capture:"
BLOCK_END = "# end of capture"
CSV_HEADER = "rec,time_ms,type,bus,dir,status,len,summary,bytes"

# pcap global header (little-endian magic A1B2C3D4, version 2.4, snaplen 65535).
PCAP_MAGIC = 0xA1B2C3D4
PCAP_VERSION = (2, 4)
PCAP_SNAPLEN = 65535
LINKTYPE_IPMI_HPM_2 = 260

# packet-ipmi-trace.c block fields.
BLOCK_TYPE_FRAME = 0
BLOCK_TYPE_TEXT = 2
CHANNEL = 0
DATA_TYPE_IPMB_1_0 = 0x01
FLAG_BUS_B = 1 << 6
FLAG_TO_CONTROLLER = 1 << 7
MS_PER_SECOND = 1000
US_PER_MS = 1000

# Column positions of a CSV row.
COL_INDEX, COL_TIME, COL_TYPE, COL_BUS, COL_DIR, COL_STATUS, COL_LEN, COL_SUMMARY, COL_BYTES = range(9)
COLUMNS = 9

# Status words that mean the frame went out (or came in) without a fault.
OK_STATUSES = {"rx", "inject", "ok"}


def csv_rows(text):
    """The CSV rows (lists of 9 fields) of every DEBUG DUMP block in `text`, in order."""
    rows = []
    inside = False
    for line in text.splitlines():
        line = line.rstrip("\r\n")
        if line.startswith(BLOCK_START):
            inside = True
        elif line.startswith(BLOCK_END):
            inside = False
        elif inside and line and line != CSV_HEADER:
            try:
                fields = next(csv.reader([line]))
            except csv.Error:
                continue
            if len(fields) >= COLUMNS and fields[COL_INDEX].isdigit():
                rows.append(fields[:COLUMNS])
    return rows


def block(block_type, time_ms, flags, data):
    """One ipmi-trace block."""
    seconds, millis = divmod(time_ms, MS_PER_SECOND)
    header = struct.pack(
        "<BIHBHB",
        (block_type << 4) | CHANNEL,
        seconds,
        millis,
        DATA_TYPE_IPMB_1_0,
        flags,
        len(data),
    )
    return header + data


def row_blocks(fields):
    """The (time_ms, block) pairs of one CSV row: one block, or two for a failed Out frame."""
    time_ms = int(fields[COL_TIME])
    if fields[COL_TYPE] == "TEXT":
        return [(time_ms, block(BLOCK_TYPE_TEXT, time_ms, 0, fields[COL_SUMMARY].encode("ascii", "replace")))]
    flags = 0
    if fields[COL_BUS] == "B":
        flags |= FLAG_BUS_B
    if fields[COL_DIR] == "IN":
        flags |= FLAG_TO_CONTROLLER
    frame = bytes(int(token, 16) for token in fields[COL_BYTES].split())
    blocks = [(time_ms, block(BLOCK_TYPE_FRAME, time_ms, flags, frame))]
    status = fields[COL_STATUS]
    if status not in OK_STATUSES:
        note = "tx failed: " + status
        blocks.append((time_ms, block(BLOCK_TYPE_TEXT, time_ms, flags, note.encode("ascii"))))
    return blocks


def pcap_bytes(rows):
    """The whole pcap file for the CSV rows."""
    out = bytearray(
        struct.pack(
            "<IHHiIII", PCAP_MAGIC, *PCAP_VERSION, 0, 0, PCAP_SNAPLEN, LINKTYPE_IPMI_HPM_2
        )
    )
    for fields in rows:
        for time_ms, data in row_blocks(fields):
            seconds, millis = divmod(time_ms, MS_PER_SECOND)
            out += struct.pack("<IIII", seconds, millis * US_PER_MS, len(data), len(data))
            out += data
    return bytes(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description="DEBUG DUMP console log to pcap")
    parser.add_argument("log", help="console log holding one or more DEBUG DUMP CSV blocks")
    parser.add_argument("-o", "--out", required=True, help="output name without extension")
    parser.add_argument("--csv", action="store_true", help="also write <out>.csv")
    args = parser.parse_args(argv)
    with open(args.log, encoding="utf-8", errors="replace") as handle:
        rows = csv_rows(handle.read())
    if not rows:
        print("error: no DEBUG DUMP CSV block found in " + args.log, file=sys.stderr)
        return 1
    with open(args.out + ".pcap", "wb") as handle:
        handle.write(pcap_bytes(rows))
    print("%d records -> %s.pcap" % (len(rows), args.out))
    if args.csv:
        with open(args.out + ".csv", "w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle, lineterminator="\n")
            writer.writerow(CSV_HEADER.split(","))
            writer.writerows(rows)
        print("%d records -> %s.csv" % (len(rows), args.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
