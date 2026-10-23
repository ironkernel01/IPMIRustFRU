# trace2vec.py - converts two Saleae Logic I2C CSV exports (IPMB-A, IPMB-B) into one merged vector file.
# Usage: python -I tools/trace2vec.py <a.csv> <b.csv> <out.vec>
#
# Transactions: START -> one ADDRESS -> DATA* -> STOP, tracked as explicit states. A NAKed data
# byte, a second ADDRESS row without a START, or DATA before the ADDRESS makes the transaction
# malformed: it is dropped and counted, never merged or repaired.
# Kept: ACKed write transactions that carry data bytes, whose frame passes the length and both
# checksums BEFORE any transformation. An invalid captured frame is dropped and counted.
#
# Redaction of kept frames, in this order (every count goes in the vec header):
#   1. Read FRU Data responses (NetFn 0Bh, cmd 11h): everything after the count byte is blanked.
#   2. Get Device ID responses (NetFn 07h, cmd 01h): manufacturer ID (3 bytes) and product ID
#      (2 bytes) are blanked.
#   3. Printable ASCII runs of 4 or more bytes in the data portion become 'X'.
# Each change recomputes the second checksum.
import csv
import os
import sys

RUN_MIN = 4
FRAME_MIN_REQ = 7
FRAME_MAX = 32  # IPMB_MSG_MAX in ipmi_defs.h

# IPMB response frame offsets (IPMI v2.0 IPMB message format): rqSA, netFn<<2|rqLUN, chk1,
# rsSA, seq<<2|rsLUN, cmd, completion code, data..., chk2.
OFF_NETFN = 1
OFF_CMD = 5
OFF_CC = 6
OFF_DATA = 7
NETFN_SHIFT = 2

NETFN_APP_RSP = 0x07
NETFN_STORAGE_RSP = 0x0B
CMD_GET_DEVICE_ID = 0x01
CMD_READ_FRU_DATA = 0x11
# Get Device ID response data (IPMI v2.0 Table 20-2), index 0 = device ID: device ID, device
# revision, firmware rev 1, firmware rev 2, IPMI version, additional support, manufacturer ID
# x3 (indices 6..8), product ID x2 (indices 9..10), aux revision x4.
DEVID_MFR_START = OFF_DATA + 6
DEVID_PRODUCT_END = OFF_DATA + 11
DEVID_DATA_LEN = 15


def checksum(buf):
    """Two's-complement checksum: the sum of buf plus the result is 0 mod 256."""
    return (-sum(buf)) & 0xFF


def frame_valid(frame):
    """True when the length is in range and both IPMB checksums hold."""
    if not FRAME_MIN_REQ <= len(frame) <= FRAME_MAX:
        return False
    if checksum(frame[0:2]) != frame[2]:
        return False
    return checksum(frame[3:-1]) == frame[-1]


class Counts:
    """Counters reported in the summary and the vec header."""

    def __init__(self):
        self.dropped = 0  # well-formed but not a kept write (NAKed address, read, no data)
        self.malformed = 0  # broken START/ADDRESS/DATA/STOP sequence
        self.invalid = 0  # original frame fails length or checksum


def read_transactions(path, bus, counts):
    """Returns the list of (time, bus, frame bytes) for ACKed write transactions."""
    out = []
    state = "idle"  # idle -> started -> addressed
    cur = None
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            kind = row["type"]
            if kind == "start":
                if state != "idle" and cur["touched"]:
                    counts.malformed += 1  # previous transaction had no STOP row
                state = "started"
                cur = {"t": None, "ack": False, "addr": None, "read": False,
                       "data": [], "bad": False, "touched": False}
            elif state == "idle":
                continue  # a row outside any START
            elif kind == "address":
                cur["touched"] = True
                if state != "started":
                    cur["bad"] = True  # second address row without a START
                    continue
                state = "addressed"
                cur["t"] = float(row["start_time"])
                cur["ack"] = row["ack"].strip().lower() == "true"
                cur["addr"] = int(row["address"], 0)
                cur["read"] = row["read"].strip().lower() == "true"
            elif kind == "data":
                cur["touched"] = True
                if state != "addressed" or row["ack"].strip().lower() != "true":
                    cur["bad"] = True  # data before the address, or a NAKed data byte
                    continue
                cur["data"].append(int(row["data"], 0))
            elif kind == "stop":
                if cur["bad"]:
                    counts.malformed += 1
                elif cur["addr"] is not None:
                    if cur["ack"] and cur["data"] and not cur["read"]:
                        out.append((cur["t"], bus, bytes([cur["addr"] << 1]) + bytes(cur["data"])))
                    else:
                        counts.dropped += 1
                state, cur = "idle", None
    if state != "idle" and cur["touched"]:
        counts.malformed += 1  # capture ended inside a transaction
    return out


def blank_secrets(frame):
    """Returns (frame, kind): kind is 'fru', 'devid' or None (frame matched). Blanks identity bytes."""
    buf = bytearray(frame)
    netfn = frame[OFF_NETFN] >> NETFN_SHIFT
    cmd = frame[OFF_CMD]
    kind = None
    if netfn == NETFN_STORAGE_RSP and cmd == CMD_READ_FRU_DATA and len(frame) > OFF_DATA + 1:
        # Byte after the completion code is the count; the FRU bytes follow it up to chk2.
        for i in range(OFF_DATA + 1, len(frame) - 1):
            buf[i] = 0
        kind = "fru"
    elif (netfn == NETFN_APP_RSP and cmd == CMD_GET_DEVICE_ID and frame[OFF_CC] == 0
          and len(frame) >= OFF_DATA + DEVID_DATA_LEN + 1):
        for i in range(DEVID_MFR_START, DEVID_PRODUCT_END):
            buf[i] = 0
        kind = "devid"
    if kind is not None:
        buf[-1] = checksum(bytes(buf[3:-1]))
    return bytes(buf), kind


def mask_runs(frame):
    """Returns (frame, changed). Masks printable runs in the data portion, fixes the checksum."""
    is_resp = bool(frame[1] >> 2 & 1)
    start = 7 if is_resp else 6
    end = len(frame) - 1
    buf = bytearray(frame)
    changed = False
    i = start
    while i < end:
        j = i
        while j < end and 0x20 <= buf[j] <= 0x7E:
            j += 1
        if j - i >= RUN_MIN:
            for k in range(i, j):
                buf[k] = 0x58
            changed = True
            i = j
        else:
            i = max(j, i + 1)
    if changed:
        buf[-1] = checksum(bytes(buf[3:-1]))
    return bytes(buf), changed


def main(argv):
    if len(argv) != 4:
        print("usage: python -I tools/trace2vec.py <a.csv> <b.csv> <out.vec>")
        return 2
    a_path, b_path, out_path = argv[1:4]
    counts_a, counts_b = Counts(), Counts()
    a = read_transactions(a_path, "A", counts_a)
    b = read_transactions(b_path, "B", counts_b)
    merged = sorted(a + b, key=lambda r: r[0])
    frames = []
    invalid = blanked_fru = blanked_devid = masked = 0
    for t, bus, frame in merged:
        if not frame_valid(frame):
            invalid += 1  # never repaired: a corrupt capture must not become a passing vector
            continue
        frame, kind = blank_secrets(frame)
        blanked_fru += kind == "fru"
        blanked_devid += kind == "devid"
        frame, changed = mask_runs(frame)
        masked += changed
        frames.append((t, bus, frame))
    dropped = counts_a.dropped + counts_b.dropped
    malformed = counts_a.malformed + counts_b.malformed
    with open(out_path, "w", newline="\n") as fh:
        fh.write("# ipmb-vec 1\n")
        fh.write("# source: %s %s\n" % (os.path.basename(a_path), os.path.basename(b_path)))
        fh.write("# redacted-frames: %d\n" % masked)
        fh.write("# blanked-read-fru-data-frames: %d\n" % blanked_fru)
        fh.write("# blanked-get-device-id-frames: %d\n" % blanked_devid)
        fh.write("# dropped-invalid-frames: %d\n" % invalid)
        fh.write("# dropped-malformed-transactions: %d\n" % malformed)
        fh.write("# dropped-naked-read-or-empty-transactions: %d\n" % dropped)
        fh.write("# columns: time_s bus frame-hex (first byte = 8-bit destination address = 7-bit address << 1)\n")
        for t, bus, frame in frames:
            fh.write("%.6f %s %s\n" % (t, bus, " ".join("%02X" % x for x in frame)))
    print("frames written: %d (transactions kept: A %d, B %d)" % (len(frames), len(a), len(b)))
    print("dropped (NAKed, read, no data): A %d, B %d, total %d"
          % (counts_a.dropped, counts_b.dropped, dropped))
    print("malformed transactions dropped: A %d, B %d, total %d"
          % (counts_a.malformed, counts_b.malformed, malformed))
    print("invalid original frames dropped: %d" % invalid)
    print("Read FRU Data responses blanked: %d" % blanked_fru)
    print("Get Device ID responses blanked: %d" % blanked_devid)
    print("frames with masked printable runs: %d" % masked)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
