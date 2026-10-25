"""Tests of capture2pcap.py. Run: python -I -m unittest (from tools/capture)."""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture2pcap  # noqa: E402

TSHARK = r"C:\Program Files\Wireshark\tshark.exe"

# A terminal session around one dump: a request in, a response out, a failed out, a text line.
DUMP = '''ipmc> debug dump
# IPMC capture: ipmc-nucleo-f401re v0.1.0, ipmc-core v0.1.0, IPMB address 82h, Tier 3, uptime 130000 ms, 4 records, 0 overwritten
rec,time_ms,type,bus,dir,status,len,summary,bytes
0,1000,FRAME,A,IN,rx,7,"20h->82h App Req Get Device ID seq 01h",82 18 66 20 04 01 DB
1,1012,FRAME,A,OUT,ok,8,"82h->20h App Rsp Get Device ID seq 01h CC 00h",20 1C C4 82 04 01 00 79
2,2500,FRAME,A,OUT,addr-nack,14,"82h->20h S/E Req Platform Event seq 00h",20 10 D0 82 00 02 04 F0 00 6F A1 00 00 78
3,2510,TEXT,,,,,"say ""hi""",
# end of capture
ipmc>
'''

# The pcap file for DUMP, byte by byte: the pcap header, then per packet the record header
# (seconds, microseconds, included length, original length) and the ipmi-trace block (type and
# channel, seconds, milliseconds, data type, flags, data length, data).
EXPECTED = bytes.fromhex(
    "".join(
        part.replace(" ", "")
        for part in (
            # global header: magic, version 2.4, zone, sigfigs, snaplen 65535, network 260
            "d4c3b2a1 0200 0400 00000000 00000000 ffff0000 04010000",
            # packet 0: 1.000 s, 18 bytes; frame block, flags 0080 (In, IPMB-A), 7 bytes
            "01000000 00000000 12000000 12000000",
            "00 01000000 0000 01 8000 07 82186620040 1db",
            # packet 1: 1.012 s, 19 bytes; flags 0000 (Out, IPMB-A), 8 bytes
            "01000000 e02e0000 13000000 13000000",
            "00 01000000 0c00 01 0000 08 201cc48204010079",
            # packet 2: 2.500 s, 25 bytes; the failed frame, 14 bytes
            "02000000 20a10700 19000000 19000000",
            "00 02000000 f401 01 0000 0e 2010d082000204f0006fa1000078",
            # packet 3: the failure note, a text block (type 2), 20 bytes "tx failed: addr-nack"
            "02000000 20a10700 1f000000 1f000000",
            "20 02000000 f401 01 0000 14 7478206661696c65643a20616464722d6e61636b",
            # packet 4: 2.510 s, 19 bytes; a text block, 8 bytes: say "hi"
            "02000000 30c80700 13000000 13000000",
            "20 02000000 fe01 01 0000 08 7361792022686922",
        )
    )
)


class Capture2PcapTests(unittest.TestCase):
    def test_a_dump_converts_to_the_exact_pcap_bytes(self):
        rows = capture2pcap.csv_rows(DUMP)
        self.assertEqual([r[0] for r in rows], ["0", "1", "2", "3"])
        self.assertEqual(rows[3][capture2pcap.COL_SUMMARY], 'say "hi"')
        self.assertEqual(capture2pcap.pcap_bytes(rows), EXPECTED)

    def test_text_outside_a_block_and_trace_blocks_are_ignored(self):
        noise = "0,5,FRAME,A,IN,rx,7,x,00\n" + DUMP + "# IPMC capture: x\n#12   1.000  TEXT    t\n# end of capture\n"
        self.assertEqual(len(capture2pcap.csv_rows(noise)), 4)

    def test_main_writes_the_pcap_and_the_csv(self):
        with tempfile.TemporaryDirectory() as folder:
            log = os.path.join(folder, "dump.txt")
            with open(log, "w", encoding="utf-8") as handle:
                handle.write(DUMP)
            out = os.path.join(folder, "dump")
            self.assertEqual(capture2pcap.main([log, "-o", out, "--csv"]), 0)
            with open(out + ".pcap", "rb") as handle:
                self.assertEqual(handle.read(), EXPECTED)
            with open(out + ".csv", encoding="utf-8") as handle:
                lines = handle.read().splitlines()
            self.assertEqual(lines[0], capture2pcap.CSV_HEADER)
            self.assertEqual(len(lines), 5)
            self.assertEqual(lines[4], '3,2510,TEXT,,,,,"say ""hi""",')
            empty = os.path.join(folder, "empty.txt")
            with open(empty, "w", encoding="utf-8") as handle:
                handle.write("nothing here\n")
            self.assertEqual(capture2pcap.main([empty, "-o", os.path.join(folder, "none")]), 1)

    @unittest.skipUnless(os.path.exists(TSHARK), "tshark is not installed at " + TSHARK)
    def test_tshark_reads_the_commands(self):
        with tempfile.TemporaryDirectory() as folder:
            pcap = os.path.join(folder, "dump.pcap")
            with open(pcap, "wb") as handle:
                handle.write(capture2pcap.pcap_bytes(capture2pcap.csv_rows(DUMP)))
            listing = subprocess.run(
                [TSHARK, "-r", pcap], capture_output=True, text=True, check=True
            ).stdout
        self.assertIn("Get Device ID", listing)
        self.assertIn("Platform Event", listing)


if __name__ == "__main__":
    unittest.main()
